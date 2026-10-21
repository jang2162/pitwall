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
    /// 쓰기 스레드의 큐. 끝내면 `None` 이 되어 그 스레드도 끝난다.
    input: Mutex<Option<mpsc::Sender<Vec<u8>>>>,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
    pid: Option<u32>,
    output: Arc<Output>,
    /// 셸이 끝나면 `Some(code)`. 신호로 끝났으면 code 가 `None`.
    exit: watch::Receiver<Option<Option<i32>>>,
    terminated: AtomicBool,
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
        let started = [
            thread("pty-read", {
                let output = Arc::clone(&output);
                move || read_loop(reader, &readable, &output)
            }),
            thread("pty-write", move || write_loop(writer, &queue)),
            thread("pty-wait", move || {
                // 셸을 여기서 거둔다 — 좀비로 남아 pid 가 살아 있는 것처럼 보이지 않게.
                let code = match child.wait() {
                    Ok(status) if status.signal().is_none() => {
                        i32::try_from(status.exit_code()).ok()
                    }
                    Ok(_) => None,
                    Err(e) => {
                        tracing::warn!(target: "pw_run", %e, "cannot wait for the shell");
                        None
                    }
                };
                let _ = exit_tx.send(Some(code));
            }),
        ];
        #[cfg(unix)]
        if let Some(leader) = pid.and_then(|p| libc::pid_t::try_from(p).ok()) {
            lock(&LEADERS).insert(leader);
        }
        let pty = Self {
            master: Mutex::new(pair.master),
            input: Mutex::new(Some(input)),
            killer: Mutex::new(killer),
            pid,
            output,
            exit,
            terminated: AtomicBool::new(false),
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
        let mut exit = self.exit.clone();
        match exit.wait_for(Option::is_some).await {
            Ok(code) => code.flatten(),
            // 보내는 쪽(종료 스레드)이 값 없이 사라졌다 — 띄우기가 실패한 경우뿐이다.
            Err(_) => None,
        }
    }

    /// 버퍼가 가장 많이 찼을 때의 크기.
    pub fn peak_buffered(&self) -> usize {
        self.output.peak()
    }

    /// 셸과 그 세션의 프로세스를 모두 끝낸다(유닉스: SIGHUP, 남으면 [`HANGUP_GRACE`] 뒤 SIGKILL). 기다리지 않는다. 두 번째
    /// 부터는 아무것도 하지 않는다.
    pub fn terminate(&self) {
        if self.terminated.swap(true, Ordering::SeqCst) {
            return;
        }
        // 입력은 바로 막되(`write` 가 거짓) 큐는 셸을 끝낸 뒤 버린다. 큐가 닫히면 쓰기 스레드가 portable-pty 의 writer 를 버리고,
        // 그 writer 는 버려질 때 셸에 `\n` + ^D(EOF)를 쓴다 — 신호보다 먼저 가면 셸이 그것을 읽고 `exit 0` 으로 끝나 신호 종료가
        // 정상 종료로 보인다(시험 `closing_does_not_feed_the_shell_an_eof_before_ending_it`).
        let input = lock(&self.input).take();
        self.output.close();
        #[cfg(unix)]
        let leader = self.pid.and_then(|p| libc::pid_t::try_from(p).ok());
        #[cfg(unix)]
        let input = if let Some(leader) = leader {
            let input = Arc::new(Mutex::new(input));
            let held = Arc::clone(&input);
            let spawned = thread("pty-terminate", move || {
                crate::tree::terminate_session(leader, HANGUP_GRACE);
                drop(lock(&held).take());
                forget_leader(leader);
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

/// 끝낸 뒤([`Output::close`])에는 읽은 것을 버리며 [`DRAIN_AFTER_CLOSE`] 까지만 더 읽는다.
fn read_loop(mut reader: Box<dyn Read + Send>, readable: &Readable, output: &Output) {
    let mut buf = vec![0u8; CHUNK];
    let mut drain_until: Option<Instant> = None;
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
            Ok(n) if room.is_some() => output.push(&buf[..n]),
            Ok(_) => {}
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            // Linux 는 slave 가 모두 닫히면 EIO 로 끝난다.
            Err(_) => break,
        }
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
        let code = tokio::time::timeout(Duration::from_secs(30), pty.exited())
            .await
            .unwrap();
        assert_eq!(code, Some(0));
        // ConPTY 는 셸이 끝나도 PTY 를 닫기 전에는 EOF 가 오지 않는다 — 출력이 멎을 때까지만 읽는다.
        let mut text = Vec::new();
        while let Ok(Some(bytes)) =
            tokio::time::timeout(Duration::from_secs(2), pty.peek(CHUNK)).await
        {
            pty.consume(bytes.len());
            text.extend(bytes);
            if String::from_utf8_lossy(&text).contains("pitwall-smoke") {
                break;
            }
        }
        assert!(
            String::from_utf8_lossy(&text).contains("pitwall-smoke"),
            "{:?}",
            String::from_utf8_lossy(&text)
        );
    }
}
