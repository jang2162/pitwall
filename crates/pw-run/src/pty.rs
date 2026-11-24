//! PTY 하나: 셸을 띄우고, 출력을 상한 있는 버퍼로 읽고, 입력 · 크기 바꾸기 · 끝내기를 받는다.
//!
//! 스레드 셋이 블로킹 IO 를 맡는다 — 읽기(PTY → 버퍼), 쓰기(입력 큐 → PTY), 종료 기다리기(셸을 거둔다). 읽기 스레드는 버퍼가
//! [`OUTPUT_BUFFER`] 에 닿으면 소비자가 비울 때까지 PTY 를 읽지 않는다. 그동안 커널의 PTY 버퍼가 차서 셸 쪽 쓰기가 멈춘다 —
//! 백프레셔가 프로세스까지 간다.
//!
//! 유닉스에서는 끝내기가 다 끝나지 않은 셸의 세션을 프로세스 전체 목록에 둔다 — 앱이 끝날 때 Drop 없이 [`shutdown_all`] 로
//! 정리한다.

use std::collections::VecDeque;
use std::ffi::OsString;
use std::io::{ErrorKind, Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};
use tokio::sync::{Notify, watch};

/// 소비자가 읽지 않을 때 터미널 하나가 쌓는 출력의 상한(12-m1-plan.md M1-08 「credit 상한(권장 1 MiB)」): Host 버퍼
/// [`OUTPUT_BUFFER`] + credit 을 받아 이미 내보낸 청크([`CREDIT_WINDOW`] × [`CHUNK`]).
pub const OUTPUT_LIMIT: usize = 1024 * 1024;

/// 클라이언트가 한 번에 주는 credit(청크 수). TS 클라이언트의 기본 `window`(`packages/rpc-client/src/client.ts`)와 같다.
/// credit 은 바이트가 아니라 청크 수라(03-architecture.md 「RPC 설계」) 이 수와 [`CHUNK`] 가 내보낸 바이트를 묶는다.
pub const CREDIT_WINDOW: usize = 32;

/// 청크 하나의 상한. 소비자가 느리면 버퍼에 쌓인 것을 이만큼씩 묶어 보낸다.
pub const CHUNK: usize = 16 * 1024;

/// Host 쪽 출력 버퍼의 상한. 소비자에게 보내는 중인 청크도 여기 든다. 나머지 몫은 내보낸 청크의 것이다([`OUTPUT_LIMIT`]).
pub const OUTPUT_BUFFER: usize = OUTPUT_LIMIT - CREDIT_WINDOW * CHUNK;

/// 끝낸 뒤에도 PTY 를 읽어 버리는 시간의 상한. 읽히지 않은 출력이 남은 tty 를 닫는 프로세스는 그것이 비워지기를 기다리며
/// 끝나지 못한다(macOS 에서 SIGKILL 을 받고도 E · U 상태로 남는다). 세션의 프로세스는 SIGKILL(300 ms) 뒤 곧 끝나므로 넉넉하다.
const DRAIN_AFTER_CLOSE: Duration = Duration::from_secs(2);

/// 끝낼 때 SIGHUP 을 보낸 뒤 SIGKILL 까지 기다리는 시간.
#[cfg(unix)]
const HANGUP_GRACE: Duration = Duration::from_millis(300);

/// 이 프로세스가 띄웠고 아직 끝내기가 끝나지 않은 셸(세션 리더)의 pid. 끝내기가 끝나야 빠진다 — 그 전에는 세션 id 가 살아 있어
/// pid 가 다른 프로세스에 다시 쓰이지 않는다.
#[cfg(unix)]
static LEADERS: Mutex<std::collections::BTreeSet<libc::pid_t>> =
    Mutex::new(std::collections::BTreeSet::new());

#[cfg(unix)]
fn forget_leader(leader: libc::pid_t) {
    lock(&LEADERS).remove(&leader);
}

/// 이 프로세스가 띄운 모든 터미널의 세션을 끝낸다(SIGHUP, [`HANGUP_GRACE`] 안에 안 끝난 것은 SIGKILL). 끝날 때까지 블로킹한다 —
/// 프로세스가 곧 끝나므로(Drop 이 돌지 않는다) 백그라운드 스레드에 맡기지 않는다. Windows 는 아무것도 하지 않는다(프로세스가 끝나면
/// ConPTY 가 닫혀 붙은 콘솔 프로세스도 끝난다).
pub fn shutdown_all() {
    #[cfg(unix)]
    {
        let leaders: Vec<libc::pid_t> = lock(&LEADERS).iter().copied().collect();
        if leaders.is_empty() {
            return;
        }
        tracing::info!(target: "pw_run", ?leaders, "ending terminal sessions before the app exits");
        crate::tree::terminate_sessions(&leaders, HANGUP_GRACE);
        for leader in leaders {
            forget_leader(leader);
        }
    }
}

/// 끝내는 방법([`Pty::end`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ending {
    /// 터미널 닫기: SIGHUP, [`HANGUP_GRACE`] 뒤 남은 것에 SIGKILL. 출력은 곧바로 닫는다(남은 출력은 읽어 버린다).
    Hangup,
    /// 실행 중지(17-m6-plan M6-02 ④): SIGTERM, `grace` 뒤 남은 것에 SIGKILL. 출력은 세션이 끝날 때까지 계속 읽는다 — 프로그램이
    /// 끝나며 찍는 것도 로그에 남는다.
    Terminate { grace: Duration },
    /// compose 의 중지(M6-01): SIGINT 로 compose 가 컨테이너를 멈추게 하고, `interrupt` 안에 끝나지 않으면 [`Ending::Terminate`].
    InterruptThenTerminate {
        interrupt: Duration,
        grace: Duration,
    },
}

impl Ending {
    /// 세션에 보낼 (신호, 기다림) 단계(`tree::end_sessions`).
    #[cfg(unix)]
    fn steps(self) -> Vec<(libc::c_int, Duration)> {
        match self {
            Self::Hangup => vec![(libc::SIGHUP, HANGUP_GRACE)],
            Self::Terminate { grace } => vec![(libc::SIGTERM, grace)],
            Self::InterruptThenTerminate { interrupt, grace } => {
                vec![(libc::SIGINT, interrupt), (libc::SIGTERM, grace)]
            }
        }
    }

    /// 터미널은 세션만, 실행은 세션과 그 자손(`crate::tree` 머리 주석).
    #[cfg(unix)]
    fn reach(self) -> crate::tree::Reach {
        match self {
            Self::Hangup => crate::tree::Reach::Session,
            Self::Terminate { .. } | Self::InterruptThenTerminate { .. } => {
                crate::tree::Reach::SessionAndChildren
            }
        }
    }
}

/// 셸 · 프로그램이 끝난 꼴.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PtyExit {
    /// 끝 코드. 신호로 끝났으면 `None`.
    pub code: Option<i32>,
    /// 끝낸 신호의 이름(`strsignal` — 예: `Terminated: 15`). 코드로 끝났으면 `None`.
    pub signal: Option<String>,
}

/// 띄울 것.
#[derive(Debug, Clone)]
pub struct Spec {
    pub argv: Vec<OsString>,
    pub cwd: PathBuf,
    /// 자식의 환경 전체. 앱 프로세스의 환경은 물려주지 않는다.
    pub env: Vec<(OsString, OsString)>,
    pub cols: u16,
    pub rows: u16,
}

pub struct Pty {
    master: Mutex<Box<dyn MasterPty + Send>>,
    /// 쓰기 스레드의 큐. 끝내면 `None` 이 되어 그 스레드도 끝난다. 읽기 스레드(Windows 의 시작 질의 답)는 이 칸을 함께 보고
    /// 송신자를 따로 복제해 쥐지 않는다 — 질의가 오기 전에 끝내도 큐가 그 자리에서 닫힌다.
    input: Arc<Mutex<Option<mpsc::Sender<Vec<u8>>>>>,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
    pid: Option<u32>,
    output: Arc<Output>,
    /// 셸이 끝나면 `Some`.
    exit: watch::Receiver<Option<PtyExit>>,
    terminated: AtomicBool,
    /// 끝내기([`Self::end`])가 다 끝났다 — 세션의 프로세스가 모두 끝났거나 SIGKILL 을 보냈다.
    ended: watch::Sender<bool>,
}

impl Pty {
    pub fn spawn(spec: &Spec) -> Result<Self, String> {
        let pair = native_pty_system()
            .openpty(size(spec.cols, spec.rows))
            .map_err(|e| format!("cannot open a pty: {e:#}"))?;
        let mut cmd = CommandBuilder::from_argv(spec.argv.clone());
        cmd.env_clear();
        for (name, value) in &spec.env {
            cmd.env(name, value);
        }
        cmd.cwd(&spec.cwd);
        let mut child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| format!("cannot start {:?}: {e:#}", spec.argv))?;
        // 우리 쪽 slave 를 닫아야 셸과 그 자식이 모두 끝났을 때 읽기가 EOF 를 본다.
        drop(pair.slave);
        let pid = child.process_id();
        let killer = child.clone_killer();
        let readable = Readable::of(&*pair.master);
        let reader = pair.master.try_clone_reader();
        let writer = pair.master.take_writer();
        let (reader, writer) = match (reader, writer) {
            (Ok(r), Ok(w)) => (r, w),
            (Err(e), _) | (_, Err(e)) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("cannot use the pty: {e:#}"));
            }
        };

        let output = Arc::new(Output::new(OUTPUT_BUFFER));
        let (exit_tx, exit) = watch::channel(None);
        let (input, queue) = mpsc::channel::<Vec<u8>>();
        let input = Arc::new(Mutex::new(Some(input)));
        // Windows: ConPTY 의 시작 질의에 이 PTY 가 답한다([`StartupQuery`]). 유닉스 PTY 는 묻지 않는다.
        let answer = cfg!(windows).then(|| Arc::clone(&input));
        let started = [
            thread("pty-read", {
                let output = Arc::clone(&output);
                move || read_loop(reader, &readable, &output, answer)
            }),
            thread("pty-write", move || write_loop(writer, &queue)),
            thread("pty-wait", move || {
                // 셸을 여기서 거둔다 — 좀비로 남아 pid 가 살아 있는 것처럼 보이지 않게.
                let exit = match child.wait() {
                    Ok(status) => match status.signal() {
                        None => PtyExit {
                            code: i32::try_from(status.exit_code()).ok(),
                            signal: None,
                        },
                        Some(signal) => PtyExit {
                            code: None,
                            signal: Some(signal.to_owned()),
                        },
                    },
                    Err(e) => {
                        tracing::warn!(target: "pw_run", %e, "cannot wait for the shell");
                        PtyExit::default()
                    }
                };
                let _ = exit_tx.send(Some(exit));
            }),
        ];
        #[cfg(unix)]
        if let Some(leader) = pid.and_then(|p| libc::pid_t::try_from(p).ok()) {
            lock(&LEADERS).insert(leader);
        }
        let pty = Self {
            master: Mutex::new(pair.master),
            input,
            killer: Mutex::new(killer),
            pid,
            output,
            exit,
            terminated: AtomicBool::new(false),
            ended: watch::channel(false).0,
        };
        if let Some(e) = started.into_iter().find_map(Result::err) {
            pty.terminate();
            return Err(format!("cannot start a pty thread: {e}"));
        }
        Ok(pty)
    }

    pub fn pid(&self) -> Option<u32> {
        self.pid
    }

    /// 입력을 큐에 넣는다. 부른 순서대로 쓰인다. 끝난 터미널이면 `false`.
    pub fn write(&self, data: Vec<u8>) -> bool {
        lock(&self.input)
            .as_ref()
            .is_some_and(|queue| queue.send(data).is_ok())
    }

    pub fn resize(&self, cols: u16, rows: u16) -> Result<(), String> {
        lock(&self.master)
            .resize(size(cols, rows))
            .map_err(|e| format!("cannot resize the pty: {e:#}"))
    }

    /// 커널이 지금 가진 크기 `(rows, cols)` — 시험의 실패 메시지에 싣는다.
    #[cfg(test)]
    pub fn size(&self) -> (u16, u16) {
        let s = lock(&self.master).get_size().unwrap();
        (s.rows, s.cols)
    }

    /// 버퍼 앞의 출력을 최대 `max` 바이트 복사한다(버퍼에서 빼지 않는다 — 보낸 뒤 [`Self::consume`]). 버퍼가 비었으면 출력이
    /// 오기를 기다리고, 읽기가 끝났으면(EOF) `None`. 취소해도 잃는 것이 없다.
    pub async fn peek(&self, max: usize) -> Option<Vec<u8>> {
        self.output.peek(max).await
    }

    pub fn consume(&self, n: usize) {
        self.output.consume(n);
    }

    /// 셸이 끝날 때까지 기다린다. 신호로 끝났으면 `None`.
    pub async fn exited(&self) -> Option<i32> {
        self.exit().await.code
    }

    /// 셸이 끝날 때까지 기다려 끝난 꼴(코드 · 신호)을 준다.
    pub async fn exit(&self) -> PtyExit {
        let mut exit = self.exit.clone();
        match exit.wait_for(Option::is_some).await {
            Ok(exit) => exit.clone().unwrap_or_default(),
            // 보내는 쪽(종료 스레드)이 값 없이 사라졌다 — 띄우기가 실패한 경우뿐이다.
            Err(_) => PtyExit::default(),
        }
    }

    /// 끝내기([`Self::end`] · [`Self::terminate`])가 다 끝날 때까지 기다린다 — 세션의 프로세스가 모두 끝났거나 남은 것에
    /// SIGKILL 을 보냈다. 끝내기를 시작하지 않았으면 시작될 때까지 기다린다.
    pub async fn ended(&self) {
        let mut ended = self.ended.subscribe();
        let _ = ended.wait_for(|done| *done).await;
    }

    /// 버퍼가 가장 많이 찼을 때의 크기.
    pub fn peak_buffered(&self) -> usize {
        self.output.peak()
    }

    /// 셸과 그 세션의 프로세스를 모두 끝낸다(유닉스: SIGHUP, 남으면 [`HANGUP_GRACE`] 뒤 SIGKILL). 기다리지 않는다. 두 번째
    /// 부터는 아무것도 하지 않는다.
    pub fn terminate(&self) {
        self.end(Ending::Hangup);
    }

    /// 셸(프로그램)과 그 세션의 프로세스를 `ending` 대로 끝낸다. 기다리지 않는다 — 다 끝나면 [`Self::ended`]. 두 번째부터는
    /// 아무것도 하지 않는다(먼저 고른 방법이 이어진다).
    pub fn end(&self, ending: Ending) {
        self.end_known(ending, Vec::new());
    }

    /// [`Self::end`] 에 앞서 찾은 자손 `known`(pid, 시작 시각 — `crate::tree::end_sessions_known`)을 더한다. 실행의 중지가 쓴다.
    pub fn end_known(&self, ending: Ending, known: Vec<(u32, u64)>) {
        if self.terminated.swap(true, Ordering::SeqCst) {
            return;
        }
        // 입력은 바로 막되(`write` 가 거짓) 큐는 셸을 끝낸 뒤 버린다. 큐가 닫히면 쓰기 스레드가 portable-pty 의 writer 를 버리고,
        // 그 writer 는 버려질 때 셸에 `\n` + ^D(EOF)를 쓴다 — 신호보다 먼저 가면 셸이 그것을 읽고 `exit 0` 으로 끝나 신호 종료가
        // 정상 종료로 보인다(시험 `closing_does_not_feed_the_shell_an_eof_before_ending_it`).
        let input = lock(&self.input).take();
        // 세션 · 자손을 찾지 못하는 OS 는 앞서 찾은 자손도 쓰지 않는다.
        #[cfg(not(unix))]
        drop(known);
        if ending == Ending::Hangup {
            self.output.close();
        }
        #[cfg(unix)]
        let leader = self.pid.and_then(|p| libc::pid_t::try_from(p).ok());
        #[cfg(unix)]
        let input = if let Some(leader) = leader {
            let input = Arc::new(Mutex::new(input));
            let held = Arc::clone(&input);
            let ended = self.ended.clone();
            let known: Vec<(libc::pid_t, u64)> = known
                .into_iter()
                .filter_map(|(pid, start)| Some((libc::pid_t::try_from(pid).ok()?, start)))
                .collect();
            let spawned = thread("pty-terminate", move || {
                crate::tree::end_sessions_known(&[leader], &ending.steps(), ending.reach(), &known);
                drop(lock(&held).take());
                forget_leader(leader);
                ended.send_replace(true);
            });
            if spawned.is_ok() {
                return;
            }
            lock(&input).take()
        } else {
            input
        };
        // Windows(ConPTY 를 닫으면 붙은 콘솔 프로세스도 끝난다) · 스레드를 못 띄운 경우: 셸만 죽인다.
        let _ = lock(&self.killer).kill();
        drop(input);
        #[cfg(unix)]
        if let Some(leader) = leader {
            forget_leader(leader);
        }
        self.ended.send_replace(true);
    }

    /// 출력을 닫는다 — 읽기 스레드는 남은 출력을 [`DRAIN_AFTER_CLOSE`] 까지 읽어 버리고 끝난다. [`Ending::Hangup`] 밖의 끝내기
    /// 뒤에 세션이 끝났는데 EOF 가 오지 않을 때(세션을 떠난 데몬이 PTY 를 쥐고 있다) 쓴다.
    pub fn close_output(&self) {
        self.output.close();
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        self.terminate();
    }
}

fn size(cols: u16, rows: u16) -> PtySize {
    PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    }
}

fn thread(name: &str, f: impl FnOnce() + Send + 'static) -> std::io::Result<()> {
    std::thread::Builder::new()
        .name(name.to_owned())
        .spawn(f)
        .map(drop)
}

/// ConPTY 가 시작할 때 묻는 커서 위치(DSR).
const CURSOR_QUERY: &[u8] = b"\x1b[6n";
/// 그 질의의 답 — 새 화면의 맨 위 왼쪽.
const CURSOR_AT_HOME: &[u8] = b"\x1b[1;1R";
/// 시작 질의를 찾아보는 출력의 앞부분. 그 뒤의 `ESC[6n` 은 프로그램의 것이라 터미널(xterm.js)이 답하게 그대로 둔다.
const STARTUP_SCAN: usize = 4 * 1024;

/// Windows ConPTY 의 시작 질의. portable-pty 는 ConPTY 를 `PSEUDOCONSOLE_INHERIT_CURSOR` 로 열고, 그러면 ConPTY 는 출력 맨 앞에
/// `ESC[6n` 을 쓰고 **답이 올 때까지 명령을 돌리지 않는다**. 답하는 것은 원래 터미널(xterm.js)인데, 화면이 붙지 않은 실행(에이전트의
/// `run_start` · 사슬의 단계)은 아무도 답하지 않아 Windows 에서 영영 시작하지 않았다(CI run 45299304602 — 사슬이 끝나지 않고 시험이
/// 멈췄다). 그래서 PTY 가 답하고, 그 질의는 출력에서 뺀다 — 화면이 붙어 있어도 두 번 답하지 않는다(두 번째 답은 셸의 입력이 된다).
#[derive(Default)]
struct StartupQuery {
    /// 앞 읽기 끝에 걸린 질의의 앞 조각.
    carry: Vec<u8>,
    /// 지금까지 본 출력의 바이트.
    seen: usize,
    done: bool,
}

impl StartupQuery {
    /// 읽은 `bytes` 에서 내보낼 것과, 지금 답해야 하는지.
    fn filter(&mut self, bytes: &[u8]) -> (Vec<u8>, bool) {
        if self.done {
            return (bytes.to_vec(), false);
        }
        let mut data = std::mem::take(&mut self.carry);
        // `data` 의 첫 바이트가 출력에서 몇 번째인가 — 질의는 출력 앞 [`STARTUP_SCAN`] 바이트 안에서 시작해야 한다(한 번의 읽기가
        // 그보다 커도 그 뒤는 찾지 않는다).
        let offset = self.seen - data.len();
        data.extend_from_slice(bytes);
        self.seen += bytes.len();
        let starts = STARTUP_SCAN.saturating_sub(offset);
        if let Some(at) = data
            .windows(CURSOR_QUERY.len())
            .take(starts)
            .position(|w| w == CURSOR_QUERY)
        {
            self.done = true;
            data.drain(at..at + CURSOR_QUERY.len());
            return (data, true);
        }
        // 끝에 걸린 질의의 앞 조각은 다음 읽기와 이어 본다 — 그 조각이 앞부분 안에서 시작할 때만.
        let keep = (1..CURSOR_QUERY.len())
            .rev()
            .find(|&n| data.len() >= n && data.ends_with(&CURSOR_QUERY[..n]))
            .unwrap_or(0);
        if self.seen - keep >= STARTUP_SCAN {
            self.done = true;
            return (data, false);
        }
        self.carry = data.split_off(data.len() - keep);
        (data, false)
    }
}

/// 끝낸 뒤([`Output::close`])에는 읽은 것을 버리며 [`DRAIN_AFTER_CLOSE`] 까지만 더 읽는다. `answer` 가 있으면(Windows) 시작 질의에
/// 답한다([`StartupQuery`]) — 답했거나 더 찾지 않게 되면 놓는다(입력 큐를 쥐고 있지 않게).
fn read_loop(
    mut reader: Box<dyn Read + Send>,
    readable: &Readable,
    output: &Output,
    mut answer: Option<Arc<Mutex<Option<mpsc::Sender<Vec<u8>>>>>>,
) {
    let mut buf = vec![0u8; CHUNK];
    let mut drain_until: Option<Instant> = None;
    let mut startup = StartupQuery::default();
    loop {
        let room = output.wait_room();
        let max = match room {
            Some(room) => room.min(CHUNK),
            None => {
                if drain_until.is_none() {
                    output.finish();
                }
                let until = *drain_until.get_or_insert_with(|| Instant::now() + DRAIN_AFTER_CLOSE);
                let left = until.saturating_duration_since(Instant::now());
                if left.is_zero() || !readable.wait(left) {
                    break;
                }
                CHUNK
            }
        };
        match reader.read(&mut buf[..max]) {
            Ok(0) => break,
            Ok(n) if answer.is_some() => {
                let (bytes, reply) = startup.filter(&buf[..n]);
                if reply
                    && let Some(input) = &answer
                    && let Some(queue) = lock(input).as_ref()
                {
                    let _ = queue.send(CURSOR_AT_HOME.to_vec());
                }
                if startup.done {
                    answer = None;
                }
                if room.is_some() && !bytes.is_empty() {
                    output.push(&bytes);
                }
            }
            Ok(n) if room.is_some() => output.push(&buf[..n]),
            Ok(_) => {}
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            // Linux 는 slave 가 모두 닫히면 EIO 로 끝난다.
            Err(_) => break,
        }
    }
    // 끝에 걸려 있던 앞 조각(질의가 아니었다)도 출력이다.
    if !startup.carry.is_empty() {
        output.push(&std::mem::take(&mut startup.carry));
    }
    output.finish();
}

/// 읽기 스레드가 끝낸 뒤 PTY 에 읽을 것이 생기기를 시간 제한을 두고 기다리는 데 쓰는 master 의 복제본(유닉스). 기다릴 수 없는
/// 플랫폼(Windows)은 끝내면 곧바로 그만 읽는다.
struct Readable {
    #[cfg(unix)]
    fd: Option<std::os::fd::OwnedFd>,
}

impl Readable {
    #[cfg(unix)]
    fn of(master: &dyn MasterPty) -> Self {
        use std::os::fd::FromRawFd as _;
        let fd = master.as_raw_fd().and_then(|fd| {
            // SAFETY: dup(2) 은 메모리를 건드리지 않는다. 돌려준 fd 는 여기서만 갖는다.
            let dup = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 0) };
            // SAFETY: 방금 만든, 아무도 갖지 않은 fd 다.
            (dup >= 0).then(|| unsafe { std::os::fd::OwnedFd::from_raw_fd(dup) })
        });
        Self { fd }
    }

    #[cfg(not(unix))]
    fn of(_master: &dyn MasterPty) -> Self {
        Self {}
    }

    /// 읽을 것(또는 hangup)이 `timeout` 안에 생겼는가.
    #[cfg(unix)]
    fn wait(&self, timeout: Duration) -> bool {
        use std::os::fd::AsRawFd as _;
        let Some(fd) = &self.fd else {
            return false;
        };
        let mut poll = libc::pollfd {
            fd: fd.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        let ms = libc::c_int::try_from(timeout.as_millis())
            .unwrap_or(libc::c_int::MAX)
            .max(1);
        // SAFETY: pollfd 하나와 그 수를 넘긴다.
        unsafe { libc::poll(&raw mut poll, 1, ms) > 0 }
    }

    #[cfg(not(unix))]
    fn wait(&self, _timeout: Duration) -> bool {
        false
    }
}

fn write_loop(mut writer: Box<dyn Write + Send>, queue: &mpsc::Receiver<Vec<u8>>) {
    while let Ok(data) = queue.recv() {
        if let Err(e) = writer.write_all(&data).and_then(|()| writer.flush()) {
            tracing::debug!(target: "pw_run", %e, "terminal input stopped");
            break;
        }
    }
}

/// 읽기 스레드(하나)와 소비자(하나)가 함께 쓰는 출력 버퍼.
struct Output {
    state: Mutex<OutputState>,
    /// 소비자가 비웠다 · 닫혔다(읽기 스레드가 기다린다).
    room: Condvar,
    /// 출력이 왔다 · EOF(소비자가 기다린다). `notify_one` 은 기다리는 이가 없으면 허가를 남기므로 놓치지 않는다.
    ready: Notify,
    cap: usize,
}

#[derive(Default)]
struct OutputState {
    buf: VecDeque<u8>,
    eof: bool,
    /// 끝냈다. 버퍼가 차 있어도 읽기 스레드가 기다리지 않고 끝난다.
    closed: bool,
    peak: usize,
}

impl Output {
    fn new(cap: usize) -> Self {
        Self {
            state: Mutex::default(),
            room: Condvar::new(),
            ready: Notify::new(),
            cap,
        }
    }

    /// 빈자리가 생길 때까지 기다려 그 크기를 준다. 닫혔으면 `None`.
    fn wait_room(&self) -> Option<usize> {
        let mut state = lock(&self.state);
        loop {
            if state.closed {
                return None;
            }
            let free = self.cap.saturating_sub(state.buf.len());
            if free > 0 {
                return Some(free);
            }
            state = self
                .room
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }

    fn push(&self, bytes: &[u8]) {
        {
            let mut state = lock(&self.state);
            state.buf.extend(bytes);
            state.peak = state.peak.max(state.buf.len());
        }
        self.ready.notify_one();
    }

    fn finish(&self) {
        lock(&self.state).eof = true;
        self.ready.notify_one();
    }

    fn close(&self) {
        lock(&self.state).closed = true;
        self.room.notify_all();
    }

    async fn peek(&self, max: usize) -> Option<Vec<u8>> {
        loop {
            {
                let state = lock(&self.state);
                if !state.buf.is_empty() {
                    return Some(state.buf.iter().take(max).copied().collect());
                }
                if state.eof {
                    return None;
                }
            }
            self.ready.notified().await;
        }
    }

    fn consume(&self, n: usize) {
        {
            let mut state = lock(&self.state);
            let n = n.min(state.buf.len());
            state.buf.drain(..n);
        }
        self.room.notify_all();
    }

    fn peak(&self) -> usize {
        lock(&self.state).peak
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    /// ConPTY 의 시작 질의는 한 번만 답하고 출력에서 빠진다 — 읽기 둘에 걸쳐 와도. 앞부분 뒤의 질의는 프로그램의 것이라 그대로다.
    #[test]
    fn the_startup_cursor_query_is_answered_once_and_kept_out_of_the_output() {
        let mut q = StartupQuery::default();
        assert_eq!(q.filter(b"\x1b[?25l\x1b["), (b"\x1b[?25l".to_vec(), false));
        assert_eq!(q.filter(b"6nhello"), (b"hello".to_vec(), true));
        assert_eq!(q.filter(b"\x1b[6n"), (b"\x1b[6n".to_vec(), false));

        let mut q = StartupQuery::default();
        assert_eq!(q.filter(b"a\x1b[6nb"), (b"ab".to_vec(), true));

        let mut q = StartupQuery::default();
        let plain = vec![b'x'; STARTUP_SCAN];
        assert_eq!(q.filter(&plain), (plain.clone(), false));
        assert_eq!(q.filter(b"\x1b[6n"), (b"\x1b[6n".to_vec(), false));

        // 한 번의 읽기가 앞부분보다 커도 그 뒤의 질의는 프로그램의 것이다 — 앞부분 끝에서 시작한 것까지만 답한다.
        let mut q = StartupQuery::default();
        let mut big = vec![b'x'; STARTUP_SCAN];
        big.extend_from_slice(CURSOR_QUERY);
        assert_eq!(q.filter(&big), (big.clone(), false));
        let mut q = StartupQuery::default();
        let mut edge = vec![b'x'; STARTUP_SCAN - 1];
        edge.extend_from_slice(CURSOR_QUERY);
        assert_eq!(q.filter(&edge), (vec![b'x'; STARTUP_SCAN - 1], true));
        // 앞 조각을 걸쳐 둔 뒤에도 셈은 출력 처음부터다.
        let mut q = StartupQuery::default();
        let mut head = vec![b'x'; STARTUP_SCAN - 2];
        head.extend_from_slice(b"\x1b[");
        assert_eq!(q.filter(&head), (vec![b'x'; STARTUP_SCAN - 2], false));
        assert_eq!(q.filter(b"6n"), (Vec::new(), true));

        // 끝에 걸렸던 앞 조각이 질의가 아니었으면 다음 읽기 앞에 그대로 붙어 나온다.
        let mut q = StartupQuery::default();
        assert_eq!(q.filter(b"ab\x1b"), (b"ab".to_vec(), false));
        assert_eq!(q.filter(b"[1m"), (b"\x1b[1m".to_vec(), false));
    }

    /// 리뷰 R3: 질의가 오기 전에 끝내면(입력 칸을 비우면) 읽기 스레드가 송신자를 따로 쥐고 있지 않다 — 쓰기 스레드의 큐가 그
    /// 자리에서 닫히고, 뒤에 온 질의에는 답하지 않는다.
    #[test]
    fn ending_before_the_startup_query_closes_the_input_queue_at_once() {
        struct Feed {
            /// 첫 읽기에 들어왔다는 신호와, 끝내기가 끝났다는 신호 — 읽기 스레드가 돌기 시작한 뒤에 끝낸다.
            reading: mpsc::Sender<()>,
            ended: mpsc::Receiver<()>,
            queue: Arc<Mutex<mpsc::Receiver<Vec<u8>>>>,
            closed_at_first_read: Arc<Mutex<Option<bool>>>,
            reads: usize,
        }
        impl Read for Feed {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                self.reads += 1;
                if self.reads > 1 {
                    return Ok(0);
                }
                let _ = self.reading.send(());
                let _ = self.ended.recv_timeout(Duration::from_secs(10));
                let closed = matches!(
                    lock(&self.queue).try_recv(),
                    Err(mpsc::TryRecvError::Disconnected)
                );
                *lock(&self.closed_at_first_read) = Some(closed);
                buf[..CURSOR_QUERY.len()].copy_from_slice(CURSOR_QUERY);
                Ok(CURSOR_QUERY.len())
            }
        }
        let (sender, queue) = mpsc::channel::<Vec<u8>>();
        let input = Arc::new(Mutex::new(Some(sender)));
        let queue = Arc::new(Mutex::new(queue));
        let closed = Arc::new(Mutex::new(None));
        let (end, ended) = mpsc::channel();
        let (reading, started) = mpsc::channel();
        let feed = Feed {
            reading,
            ended,
            queue: Arc::clone(&queue),
            closed_at_first_read: Arc::clone(&closed),
            reads: 0,
        };
        let reader = std::thread::spawn({
            let input = Arc::clone(&input);
            move || {
                let output = Output::new(1024);
                let readable = Readable {
                    #[cfg(unix)]
                    fd: None,
                };
                read_loop(Box::new(feed), &readable, &output, Some(input));
            }
        });
        // 읽기 스레드가 돈 뒤에 끝낸다 — `Pty::end_known` 이 하는 것처럼 칸을 비운다.
        started.recv_timeout(Duration::from_secs(10)).unwrap();
        drop(lock(&input).take());
        end.send(()).unwrap();
        reader.join().unwrap();
        assert_eq!(
            *lock(&closed),
            Some(true),
            "the reader kept the input queue open"
        );
        assert!(
            lock(&queue).try_recv().is_err(),
            "nothing is answered after the end"
        );
    }

    #[tokio::test]
    async fn the_reader_waits_for_room_and_peek_does_not_consume() {
        let output = Output::new(4);
        assert_eq!(output.wait_room(), Some(4));
        output.push(b"abcd");
        assert_eq!(output.peek(3).await.as_deref(), Some(&b"abc"[..]));
        assert_eq!(output.peek(8).await.as_deref(), Some(&b"abcd"[..]));
        output.consume(3);
        assert_eq!(output.wait_room(), Some(3));
        assert_eq!(output.peak(), 4);
        output.consume(1);
        output.finish();
        assert_eq!(output.peek(8).await, None);
        output.close();
        assert_eq!(output.wait_room(), None);
    }

    /// 플랫폼의 PTY(유닉스 openpty · Windows ConPTY)로 명령 하나를 돌려 출력과 종료 코드를 받는다. Windows 는 이 시험만 돈다
    /// (CI 스모크, 결정 M1-D14).
    #[tokio::test(flavor = "multi_thread")]
    async fn a_command_runs_on_the_platform_pty() {
        let argv: Vec<OsString> = if cfg!(windows) {
            vec!["cmd.exe".into(), "/c".into(), "echo pitwall-smoke".into()]
        } else {
            vec!["/bin/sh".into(), "-c".into(), "echo pitwall-smoke".into()]
        };
        let pty = Pty::spawn(&Spec {
            argv,
            cwd: std::env::temp_dir(),
            env: std::env::vars_os().collect(),
            cols: 80,
            rows: 24,
        })
        .unwrap();
        // 끝나기를 기다리기 전에 출력을 읽는다. ConPTY 의 시작 질의(`ESC[6n`)는 PTY 가 답하고 출력에서 뺀다([`StartupQuery`]) —
        // 출력에 남았으면 PTY 가 답하지 않은 것이라 실패한다. ConPTY 는 셸이 끝나도 PTY 를 닫기 전에는 EOF 가 오지 않으므로 그
        // 글자가 보일 때까지만 읽는다.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        let mut text = Vec::new();
        while !String::from_utf8_lossy(&text).contains("pitwall-smoke") {
            let read = tokio::time::timeout_at(deadline, pty.peek(CHUNK)).await;
            let Ok(Some(bytes)) = read else {
                panic!("no smoke output: {:?}", String::from_utf8_lossy(&text));
            };
            pty.consume(bytes.len());
            text.extend(bytes);
        }
        assert!(
            !text.windows(CURSOR_QUERY.len()).any(|w| w == CURSOR_QUERY),
            "the startup query reached the output: {:?}",
            String::from_utf8_lossy(&text)
        );
        let code = tokio::time::timeout_at(deadline, pty.exited())
            .await
            .unwrap();
        assert_eq!(code, Some(0));
    }
}
