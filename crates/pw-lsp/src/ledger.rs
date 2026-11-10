//! Host 가 강제로 끝난 뒤(SIGKILL · 크래시) 남은 서버 프로세스 그룹을 다음 시작 때 끈다(M4-01 리뷰 L1 — M3-11 의 ssh 기록
//! `pw_db_driver::tunnel::orphans` 와 같은 기준: 기록 파일의 내용만으로 끄지 않는다).
//!
//! Host 가 정상으로 끝나면 서버 관리자가 [`crate::LanguageServer::shutdown`] 으로 그룹째 끝낸다. 강제로 끝나면 그 길이 돌지 않는다 —
//! 실 서버 대부분은 stdin 의 끝 · `processId` 로 스스로 끝나지만 서버가 띄운 손자(빌드 · Gradle 데몬 등)는 남을 수 있다. 그래서
//! 띄운 서버마다 상태 디렉터리([`ledger_dir`])에 기록을 두고(서버가 끝나면 지운다), Host 가 시작할 때 [`Ledger::reap_orphans`] 가
//! 기록을 읽어 **맞는 것만** 그룹째(SIGKILL) 끈다.
//!
//! **표지**(M4-01 재리뷰 N1): 띄울 때 128비트 무작위 값([`Ledger::marker`])을 서버 환경의 [`MARKER_ENV`] 로 넣고(자손이 물려받는다)
//! 기록에도 적는다. 기록 파일은 같은 사용자라면 누구든 쓸 수 있는 자리라 기록이 적은 pid · 인자는 믿지 않는다 — 끄는 프로세스가
//! **그 표지를 환경에 지니고 있어야** 우리가 띄운 것으로 본다. 위조 기록은 남의 프로세스 환경에 표지를 넣을 수 없다.
//!
//! 끄는 조건(모두 맞아야 한다):
//!
//! 1. 기록한 **Host 가 이미 끝났다**(그 pid 가 없거나 시작 시각이 다르다) — 같은 상태 디렉터리를 쓰는 살아 있는 Host 의 서버를 끄지
//!    않는다.
//! 2. **부팅 시각이 같다** — 재부팅을 건너면 pid 와 시작 시각을 견주는 뜻이 없다(macOS 의 시작 시각은 벽시계라 재부팅 뒤 모두 「기록
//!    이후」 이고, Linux 는 부팅 뒤 틱이다). 다르면 지우기만 한다.
//! 3. 그룹의 리더(서버, pid = 그룹 id)가 **살아 있으면**: 그 시작 시각이 기록과 같고, argv 가 기록한 실행 파일 · 인자 그대로이고(끝이
//!    인자 그대로이고 그 앞에 실행 파일 경로가 있다 — `#!` 스크립트면 인터프리터가 앞에 붙는다), 환경에 표지가 있다.
//! 4. 리더가 **없으면**(서버는 끝나고 손자만 남았다): 그 그룹 id 를 가진 프로세스가 **모두** 기록한 시작 시각 이후에 시작했고 **모두**
//!    환경에 표지가 있다. 하나라도 표지가 없거나(환경을 비우고 띄운 자손 포함) 읽을 수 없으면(다른 사용자 소유 등) 끄지 않는다.
//!
//! 맞지 않는 기록은 지우기만 한다. 시작 시각 · argv · 환경을 읽지 못하면 끄지 않는다(모르면 남의 것으로 본다).
//!
//! Linux 는 이것과 별도로 서버에 `PR_SET_PDEATHSIG`(SIGTERM)를 건다(`crate::process`) — Host 가 죽으면 그 자리에서 서버가 신호를
//! 받는다(손자는 서버가 정리하거나 다음 시작의 거두기가). Windows 는 Job Object 가 Host 와 함께 닫혀 트리째 끝나므로 기록하지 않는다.

use std::ffi::OsString;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// 기록 파일의 첫 줄(형식이 바뀌면 숫자를 올린다 — 모르는 형식은 끄지 않고 지운다).
const MAGIC: &str = "pitwall-lsp-server 2";

/// 서버 환경에 넣는 표지의 이름(머리 주석 「표지」).
pub const MARKER_ENV: &str = "PITWALL_LSP_RUN";

/// 부팅 시각을 같다고 보는 차(초). 커널이 계산하는 부팅 시각은 시계 보정으로 조금 흔들린다 — 재부팅은 이보다 훨씬 길다.
const BOOT_SLACK_SECS: u64 = 2;

/// `$STATE/lsp/running`.
pub fn ledger_dir(state: &Path) -> PathBuf {
    state.join("lsp").join("running")
}

/// 띄운 서버의 기록부(상태 디렉터리 하나).
#[derive(Debug)]
pub struct Ledger {
    dir: PathBuf,
}

/// 띄운 서버 하나의 기록. 버리면 파일을 지운다(서버 그룹이 끝났다).
#[derive(Debug)]
pub(crate) struct Record {
    path: PathBuf,
}

impl Drop for Record {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

impl Ledger {
    pub fn new(dir: impl Into<PathBuf>) -> Arc<Self> {
        Arc::new(Self { dir: dir.into() })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// 띄울 서버에 넣을 표지(128비트 무작위, 16진수). 만들지 못하면(Windows · 난수를 읽지 못함) `None` — 그 서버는 기록하지 않는다.
    pub(crate) fn marker() -> Option<String> {
        if cfg!(windows) {
            return None;
        }
        let mut bytes = [0u8; 16];
        let read = fs::File::open("/dev/urandom")
            .and_then(|mut f| std::io::Read::read_exact(&mut f, &mut bytes));
        match read {
            Ok(()) => Some(bytes.iter().map(|b| format!("{b:02x}")).collect()),
            Err(e) => {
                tracing::warn!(target: "pw_lsp", %e, "cannot make a language server marker; it will not be cleaned up after a crash");
                None
            }
        }
    }

    /// 서버(pid = 그룹 id)와 그 argv(실행 파일 + 인자) · 표지를 적는다. 적지 못하면 경고만 남긴다(서버는 그대로 쓴다 — 강제 종료 뒤
    /// 정리만 못 한다). Windows 는 적지 않는다(머리 주석).
    pub(crate) fn record(&self, pid: u32, argv: &[OsString], marker: &str) -> Option<Record> {
        if cfg!(windows) {
            return None;
        }
        let (Some(start), Some(boot)) = (start_time(pid), boot_time()) else {
            tracing::warn!(target: "pw_lsp", pid, "cannot read the language server's start or boot time; it will not be cleaned up after a crash");
            return None;
        };
        let owner = std::process::id();
        let mut text = format!(
            "{MAGIC}\npid {pid}\nstart {start}\nboot {boot}\nmark {marker}\nowner {owner} {}\n",
            start_time(owner).unwrap_or_default()
        );
        for arg in argv {
            let Some(arg) = arg.to_str().filter(|a| !a.contains(['\n', '\r'])) else {
                tracing::warn!(target: "pw_lsp", pid, "a language server argument cannot be recorded; it will not be cleaned up after a crash");
                return None;
            };
            text.push_str("arg ");
            text.push_str(arg);
            text.push('\n');
        }
        let path = self.dir.join(pid.to_string());
        let written = fs::create_dir_all(&self.dir).and_then(|()| {
            let partial = self.dir.join(format!(".{pid}.partial"));
            let mut file = fs::File::create(&partial)?;
            file.write_all(text.as_bytes())?;
            file.sync_all()?;
            fs::rename(&partial, &path)
        });
        match written {
            Ok(()) => Some(Record { path }),
            Err(e) => {
                tracing::warn!(target: "pw_lsp", pid, %e, "cannot record the language server; it will not be cleaned up after a crash");
                None
            }
        }
    }

    /// Host 시작 때: 앞 실행이 남긴 서버 그룹(머리 주석의 셋이 맞는 것)을 SIGKILL 로 끝내고 기록을 지운다. 끈 그룹 수를 돌려준다.
    /// 블로킹이다(짧다).
    pub fn reap_orphans(&self) -> usize {
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return 0;
        };
        let mut ended = 0;
        for entry in entries.flatten() {
            let path = entry.path();
            if entry.file_name().to_string_lossy().starts_with('.') {
                // 쓰다 만 기록(앞 실행이 쓰는 도중에 죽었다).
                let _ = fs::remove_file(&path);
                continue;
            }
            let Some(record) = fs::read_to_string(&path).ok().as_deref().and_then(parse) else {
                let _ = fs::remove_file(&path);
                continue;
            };
            if owner_lives(&record) {
                continue;
            }
            if same_boot(&record) && ours(&record) {
                tracing::info!(target: "pw_lsp", pid = record.pid, "ending a language server process group left by a previous run");
                kill_group(record.pid);
                ended += 1;
            }
            let _ = fs::remove_file(&path);
        }
        ended
    }
}

/// 읽은 기록.
#[derive(Debug, PartialEq, Eq)]
struct Parsed {
    pid: u32,
    start: u128,
    boot: u64,
    marker: String,
    owner: u32,
    owner_start: Option<u128>,
    argv: Vec<String>,
}

fn parse(text: &str) -> Option<Parsed> {
    let mut lines = text.lines();
    if lines.next()? != MAGIC {
        return None;
    }
    let pid = lines.next()?.strip_prefix("pid ")?.parse().ok()?;
    let start = lines.next()?.strip_prefix("start ")?.parse().ok()?;
    let boot = lines.next()?.strip_prefix("boot ")?.parse().ok()?;
    let marker = lines.next()?.strip_prefix("mark ")?.to_owned();
    let (owner, owner_start) = lines.next()?.strip_prefix("owner ")?.split_once(' ')?;
    let owner = owner.parse().ok()?;
    let owner_start = owner_start.parse().ok();
    let argv: Vec<String> = lines
        .map(|l| l.strip_prefix("arg ").map(str::to_owned))
        .collect::<Option<_>>()?;
    // 표지가 짧으면(손으로 쓴 기록) 맞힐 수 있다 — 우리가 쓰는 꼴(32 자 16진수)만 받는다.
    let marker_ok = marker.len() == 32 && marker.bytes().all(|b| b.is_ascii_hexdigit());
    (!argv.is_empty() && pid > 1 && marker_ok).then_some(Parsed {
        pid,
        start,
        boot,
        marker,
        owner,
        owner_start,
        argv,
    })
}

/// 기록한 Host 가 아직 살아 있는가. 살아 있는데 시작 시각을 읽지 못하면 살아 있다고 본다(끄지 않는 쪽).
fn owner_lives(record: &Parsed) -> bool {
    match start_time(record.owner) {
        None => alive(record.owner),
        Some(start) => record.owner_start.is_none_or(|s| s == start),
    }
}

/// 머리 주석의 2.
fn same_boot(record: &Parsed) -> bool {
    boot_time().is_some_and(|now| now.abs_diff(record.boot) <= BOOT_SLACK_SECS)
}

/// 머리 주석의 3 · 4.
fn ours(record: &Parsed) -> bool {
    let marked =
        |pid: u32| process_args(pid).is_some_and(|args| carries(&args.env, &record.marker));
    if alive(record.pid) {
        return start_time(record.pid) == Some(record.start)
            && process_args(record.pid).is_some_and(|args| {
                runs(&args.argv, &record.argv) && carries(&args.env, &record.marker)
            });
    }
    let members = group_members(record.pid);
    !members.is_empty()
        && members
            .iter()
            .all(|&m| start_time(m).is_some_and(|s| s >= record.start) && marked(m))
}

/// 환경(`이름=값` 들)에 표지가 있는가.
fn carries(env: &[String], marker: &str) -> bool {
    env.iter().any(|kv| {
        kv.strip_prefix(MARKER_ENV)
            .and_then(|rest| rest.strip_prefix('='))
            == Some(marker)
    })
}

/// 프로세스 argv 가 기록한 실행 파일 · 인자를 돌리는가: 끝이 인자 그대로이고 그 앞에 실행 파일 경로가 있다.
fn runs(argv: &[String], recorded: &[String]) -> bool {
    let (program, args) = recorded.split_first().expect("parse keeps argv non-empty");
    argv.len() > args.len()
        && argv.ends_with(args)
        && argv[..argv.len() - args.len()].iter().any(|a| a == program)
}

// --- 프로세스 조회 · 신호(OS 마다) ---------------------------------------------------------------------------------------

#[cfg(unix)]
fn alive(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    // SAFETY: 신호 0 은 보낼 수 있는지만 본다. EPERM 은 「있지만 남의 것」 이다.
    let sent = unsafe { libc::kill(pid, 0) } == 0;
    sent || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(not(unix))]
fn alive(_pid: u32) -> bool {
    false
}

#[cfg(unix)]
fn kill_group(group: u32) {
    if let Ok(group) = libc::pid_t::try_from(group) {
        // SAFETY: killpg(2) 는 신호를 보낼 뿐이다.
        unsafe {
            libc::killpg(group, libc::SIGKILL);
        }
    }
}

#[cfg(not(unix))]
fn kill_group(_group: u32) {}

/// 그룹 id 가 `group` 인 프로세스들.
#[cfg(unix)]
fn group_members(group: u32) -> Vec<u32> {
    let Ok(group) = libc::pid_t::try_from(group) else {
        return Vec::new();
    };
    all_pids()
        .into_iter()
        // SAFETY: getpgid(2) 는 메모리를 건드리지 않는다.
        .filter(|&pid| pid > 0 && unsafe { libc::getpgid(pid) } == group)
        .filter_map(|pid| u32::try_from(pid).ok())
        .collect()
}

#[cfg(not(unix))]
fn group_members(_group: u32) -> Vec<u32> {
    Vec::new()
}

#[cfg(target_os = "macos")]
fn all_pids() -> Vec<libc::pid_t> {
    // SAFETY: 널 버퍼 · 크기 0 이면 프로세스 수만 돌려준다.
    let count = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
    let Ok(count) = usize::try_from(count) else {
        return Vec::new();
    };
    let mut pids: Vec<libc::pid_t> = vec![0; count + 64];
    let bytes =
        libc::c_int::try_from(pids.len() * size_of::<libc::pid_t>()).unwrap_or(libc::c_int::MAX);
    // SAFETY: 버퍼와 그 바이트 크기를 함께 넘긴다.
    let filled = unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast(), bytes) };
    pids.truncate(usize::try_from(filled).unwrap_or(0));
    pids
}

#[cfg(target_os = "linux")]
fn all_pids() -> Vec<libc::pid_t> {
    fs::read_dir("/proc")
        .map(|entries| {
            entries
                .filter_map(|e| e.ok()?.file_name().to_str()?.parse().ok())
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(all(unix, not(any(target_os = "macos", target_os = "linux"))))]
fn all_pids() -> Vec<libc::pid_t> {
    Vec::new()
}

/// 프로세스의 시작 시각(크기를 견줄 수 있는 수 — macOS 는 마이크로초, Linux 는 부팅 뒤 틱). 같은 pid 의 다른 프로세스면 다르다.
#[cfg(target_os = "macos")]
fn start_time(pid: u32) -> Option<u128> {
    let pid = libc::c_int::try_from(pid).ok()?;
    // SAFETY: proc_bsdinfo 는 C 구조체(모두 0 이 올바른 값)이고, proc_pidinfo 는 준 크기까지만 쓴다.
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = libc::c_int::try_from(size_of::<libc::proc_bsdinfo>()).ok()?;
    // SAFETY: 위와 같다.
    let written =
        unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, (&raw mut info).cast(), size) };
    (written == size)
        .then(|| u128::from(info.pbi_start_tvsec) * 1_000_000 + u128::from(info.pbi_start_tvusec))
}

#[cfg(target_os = "linux")]
fn start_time(pid: u32) -> Option<u128> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // `pid (comm) state …` — comm 에 공백 · 괄호가 있을 수 있어 마지막 `)` 뒤를 센다. starttime 은 22번째 필드다.
    let after = &stat[stat.rfind(')')? + 1..];
    after.split_whitespace().nth(19)?.parse().ok()
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn start_time(_pid: u32) -> Option<u128> {
    None
}

/// 프로세스의 argv(`argv[0]` 포함)와 환경(`이름=값`).
struct ProcessArgs {
    argv: Vec<String>,
    env: Vec<String>,
}

#[cfg(target_os = "macos")]
fn process_args(pid: u32) -> Option<ProcessArgs> {
    let pid = libc::c_int::try_from(pid).ok()?;
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
    let mut size: libc::size_t = 0;
    // SAFETY: 먼저 크기만 묻고(버퍼 null), 그 크기의 버퍼에 받는다.
    let asked = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            3,
            std::ptr::null_mut(),
            &raw mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if asked != 0 || size < 4 {
        return None;
    }
    let mut buf = vec![0u8; size];
    // SAFETY: 위와 같다.
    let got = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            3,
            buf.as_mut_ptr().cast(),
            &raw mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if got != 0 {
        return None;
    }
    buf.truncate(size);
    // argc(i32) · 실행 파일 경로 · NUL 채움 · argv 문자열들 · 환경 문자열들(그 뒤 `apple[]` 문자열 — 표지와 겹치지 않는다).
    let argc = usize::try_from(i32::from_ne_bytes(buf.get(..4)?.try_into().ok()?)).ok()?;
    let rest = buf.get(4..)?;
    let path_end = rest.iter().position(|&b| b == 0)?;
    let mut strings = rest[path_end..]
        .split(|&b| b == 0)
        .filter(|s| !s.is_empty())
        .map(|s| String::from_utf8_lossy(s).into_owned());
    let mut argv = Vec::with_capacity(argc);
    for _ in 0..argc {
        argv.push(strings.next()?);
    }
    Some(ProcessArgs {
        argv,
        env: strings.collect(),
    })
}

#[cfg(target_os = "linux")]
fn process_args(pid: u32) -> Option<ProcessArgs> {
    let split = |raw: Vec<u8>| -> Vec<String> {
        raw.split(|&b| b == 0)
            .filter(|s| !s.is_empty())
            .map(|s| String::from_utf8_lossy(s).into_owned())
            .collect()
    };
    Some(ProcessArgs {
        argv: split(fs::read(format!("/proc/{pid}/cmdline")).ok()?),
        env: split(fs::read(format!("/proc/{pid}/environ")).ok()?),
    })
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn process_args(_pid: u32) -> Option<ProcessArgs> {
    None
}

/// 부팅 시각(유닉스 초).
#[cfg(target_os = "macos")]
fn boot_time() -> Option<u64> {
    let mut mib = [libc::CTL_KERN, libc::KERN_BOOTTIME];
    // SAFETY: timeval 은 C 구조체(모두 0 이 올바른 값)이고, sysctl 은 준 크기까지만 쓴다.
    let mut tv: libc::timeval = unsafe { std::mem::zeroed() };
    let mut size = size_of::<libc::timeval>();
    // SAFETY: 위와 같다.
    let got = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            2,
            (&raw mut tv).cast(),
            &raw mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    (got == 0 && size == size_of::<libc::timeval>())
        .then(|| u64::try_from(tv.tv_sec).ok())
        .flatten()
        .filter(|&s| s > 0)
}

#[cfg(target_os = "linux")]
fn boot_time() -> Option<u64> {
    fs::read_to_string("/proc/stat")
        .ok()?
        .lines()
        .find_map(|l| l.strip_prefix("btime ")?.trim().parse().ok())
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn boot_time() -> Option<u64> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const MARK: &str = "0123456789abcdef0123456789abcdef";

    fn text(pid: u32, start: u128, owner: u32, args: &[&str]) -> String {
        let mut t =
            format!("{MAGIC}\npid {pid}\nstart {start}\nboot 5\nmark {MARK}\nowner {owner} 7\n");
        for a in args {
            t.push_str(&format!("arg {a}\n"));
        }
        t
    }

    #[test]
    fn records_round_trip_and_unknown_shapes_are_refused() {
        let parsed = parse(&text(40, 123, 9, &["/x/rust-analyzer", "--stdio"])).unwrap();
        assert_eq!(parsed.pid, 40);
        assert_eq!(parsed.start, 123);
        assert_eq!(parsed.boot, 5);
        assert_eq!(parsed.marker, MARK);
        assert_eq!(parsed.owner_start, Some(7));
        assert_eq!(parsed.argv, ["/x/rust-analyzer", "--stdio"]);
        assert_eq!(parse(&text(40, 1, 9, &[])), None);
        assert_eq!(parse(&text(1, 1, 9, &["/x"])), None);
        assert_eq!(parse("pitwall-lsp-server 2\npid 4\n"), None);
        assert_eq!(parse(&format!("{}junk\n", text(40, 1, 9, &["/x"]))), None);
        // 우리가 쓰지 않는 꼴의 표지(짧아 맞힐 수 있다)는 받지 않는다.
        assert_eq!(parse(&text(40, 1, 9, &["/x"]).replace(MARK, "ab")), None);
    }

    #[test]
    fn the_marker_must_be_an_exact_environment_entry() {
        let env = |v: &[&str]| v.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        assert!(carries(
            &env(&["A=1", &format!("{MARKER_ENV}={MARK}")]),
            MARK
        ));
        assert!(!carries(&env(&[&format!("{MARKER_ENV}={MARK}x")]), MARK));
        assert!(!carries(&env(&[&format!("X{MARKER_ENV}={MARK}")]), MARK));
        assert!(!carries(&env(&[MARK]), MARK));
        assert!(!carries(&[], MARK));
    }

    #[test]
    fn this_machine_reports_a_boot_time() {
        if cfg!(any(target_os = "macos", target_os = "linux")) {
            let boot = boot_time().expect("boot time");
            assert!(boot > 1_000_000_000, "{boot}");
        }
    }

    #[test]
    fn argv_must_end_with_the_recorded_args_after_the_program() {
        let rec = ["/s/tls".to_owned(), "--stdio".to_owned()];
        let own = |v: &[&str]| v.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        assert!(runs(&own(&["/s/tls", "--stdio"]), &rec));
        // `#!` 스크립트: 인터프리터가 앞에 붙는다.
        assert!(runs(&own(&["/bin/sh", "/s/tls", "--stdio"]), &rec));
        assert!(!runs(&own(&["/s/tls", "--stdio", "x"]), &rec));
        assert!(!runs(&own(&["/other", "--stdio"]), &rec));
        assert!(!runs(&own(&["--stdio"]), &rec));
    }

    /// 유닉스만: Windows 는 Job Object 가 Host 와 함께 닫혀 서버 트리가 끝나므로 기록하지 않고(머리 주석), 그 OS 의 Host 생존 ·
    /// 시작 시각 판정도 없다 — 아래 `forged` 와 같은 이유.
    #[cfg(unix)]
    #[test]
    fn a_record_of_a_living_owner_or_a_stranger_is_not_reaped() {
        let tmp = tempfile::tempdir().unwrap();
        let ledger = Ledger::new(tmp.path());
        fs::create_dir_all(tmp.path()).unwrap();
        // 살아 있는 Host(이 시험 프로세스)의 기록: 남는다.
        let me = std::process::id();
        let boot = boot_time().unwrap_or(0);
        let mine = format!(
            "{MAGIC}\npid {me}\nstart {}\nboot {boot}\nmark {MARK}\nowner {me} {}\narg x\n",
            start_time(me).unwrap_or(0),
            start_time(me).map(|s| s.to_string()).unwrap_or_default()
        );
        fs::write(tmp.path().join("a"), mine).unwrap();
        // 끝난 Host 의 기록이지만 pid 의 시작 시각이 다르다(남의 프로세스 — 이 시험 프로세스): 끄지 않고 지운다.
        let stranger = format!(
            "{MAGIC}\npid {me}\nstart 1\nboot {boot}\nmark {MARK}\nowner 99999999 1\narg x\n"
        );
        fs::write(tmp.path().join("b"), stranger).unwrap();
        fs::write(tmp.path().join(".c.partial"), "half").unwrap();
        fs::write(tmp.path().join("d"), "garbage").unwrap();
        assert_eq!(ledger.reap_orphans(), 0);
        let mut left: Vec<String> = fs::read_dir(tmp.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(left, ["a"]);
    }

    /// 재리뷰 N1 의 재현과 그 짝 — 남의 프로세스를 가리키는 기록(위조 · 재사용)으로는 끄지 않고, 표지를 지닌 우리 그룹만 끈다.
    #[cfg(unix)]
    mod forged {
        use std::os::unix::process::CommandExt as _;
        use std::process::{Command, Stdio};
        use std::time::{Duration, Instant};

        use super::*;

        /// 끝난 Host 가 남긴 것처럼 보이는 기록을 쓴다.
        fn write_record(dir: &Path, pid: u32, start: u128, mark: &str, argv: &[&str]) {
            fs::create_dir_all(dir).unwrap();
            let mut t = format!(
                "{MAGIC}\npid {pid}\nstart {start}\nboot {}\nmark {mark}\nowner 99999999 1\n",
                boot_time().unwrap()
            );
            for a in argv {
                t.push_str(&format!("arg {a}\n"));
            }
            fs::write(dir.join(pid.to_string()), t).unwrap();
        }

        /// 자기 그룹으로 띄운다(`marker` 가 있으면 환경에 넣는다).
        fn spawn_group(program: &str, args: &[&str], marker: Option<&str>) -> std::process::Child {
            let mut c = Command::new(program);
            c.args(args)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .process_group(0);
            if let Some(m) = marker {
                c.env(MARKER_ENV, m);
            }
            c.spawn().unwrap()
        }

        /// `sh -c "<script>"` 을 자기 그룹으로 띄우고 리더(sh)가 끝나기를 기다린다 — 백그라운드 자식만 남은 리더 없는 그룹.
        fn leaderless_group(script: &str, marker: Option<&str>, members: usize) -> u32 {
            let mut sh = spawn_group("/bin/sh", &["-c", script], marker);
            let group = sh.id();
            sh.wait().unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            while group_members(group).len() < members {
                assert!(
                    Instant::now() < deadline,
                    "the group's members did not start"
                );
                std::thread::sleep(Duration::from_millis(20));
            }
            group
        }

        /// 그룹이 비었는가(잠시 기다린다 — SIGKILL 뒤 사라지기까지).
        fn emptied(group: u32) -> bool {
            let deadline = Instant::now() + Duration::from_secs(3);
            while Instant::now() < deadline {
                if group_members(group).is_empty() {
                    return true;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            false
        }

        fn me_start() -> u128 {
            start_time(std::process::id()).unwrap()
        }

        const SLEEPER: &str = "PW_LSP_LEDGER_SLEEPER";
        const SLEEPER_ARGS: [&str; 4] = [
            "ledger::tests::forged::sleeper",
            "--exact",
            "--ignored",
            "--quiet",
        ];

        /// 도우미: 환경 변수가 있으면 그만큼(초) 잔다. 시험 바이너리 자신을 그룹원으로 쓴다 — `/bin/sleep` · `/bin/sh` 같은 플랫폼
        /// 바이너리는 macOS 가 환경을 가려(같은 사용자여도) 표지를 읽을 수 없으므로, 판정이 표지로 갈리는지 보려면 환경을 읽을 수
        /// 있는 프로세스여야 한다.
        #[test]
        #[ignore = "helper process for the ledger tests; does nothing on its own"]
        fn sleeper() {
            if let Some(secs) = std::env::var(SLEEPER).ok().and_then(|s| s.parse().ok()) {
                std::thread::sleep(Duration::from_secs(secs));
            }
        }

        fn exe() -> String {
            std::env::current_exe()
                .unwrap()
                .to_str()
                .unwrap()
                .to_owned()
        }

        /// 도우미를 부르는 셸 명령(`sh -c` 안에 넣는다).
        fn sleeper_cmd(secs: u32) -> String {
            format!("{SLEEPER}={secs} '{}' {}", exe(), SLEEPER_ARGS.join(" "))
        }

        /// 재현 ①: Pitwall 과 무관한 리더 없는 그룹을 가리키는 `start 0` 기록 — 끄지 않는다.
        #[test]
        fn a_start_zero_record_does_not_end_a_stranger_leaderless_group() {
            let tmp = tempfile::tempdir().unwrap();
            let group = leaderless_group(&format!("{} & exit 0", sleeper_cmd(300)), None, 1);
            write_record(tmp.path(), group, 0, MARK, &["/not/a/language/server"]);
            let ended = Ledger::new(tmp.path()).reap_orphans();
            let left = group_members(group);
            kill_group(group);
            assert_eq!(ended, 0);
            assert_eq!(left.len(), 1, "the stranger's process was ended");
        }

        /// 재현 ②: 산 리더의 시작 시각 · argv 를 그대로 적은 위조 기록 — 표지가 없으므로 끄지 않는다. 짝: 같은 기록이라도 리더가
        /// 그 표지를 지니면 끈다(시험이 다른 까닭으로 0 을 내는 것이 아님을 보인다).
        #[test]
        fn a_record_copying_a_living_leader_needs_its_marker() {
            let exe = exe();
            for (marker, expected) in [(None, 0), (Some(MARK), 1)] {
                let tmp = tempfile::tempdir().unwrap();
                let mut c = Command::new(&exe);
                c.args(SLEEPER_ARGS)
                    .env(SLEEPER, "31")
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .process_group(0);
                if let Some(m) = marker {
                    c.env(MARKER_ENV, m);
                }
                let mut sleeper = c.spawn().unwrap();
                let pid = sleeper.id();
                // 도우미가 실행 파일을 다 올린 뒤(argv · 환경을 읽을 수 있게) 기록한다.
                std::thread::sleep(Duration::from_millis(300));
                let mut argv = vec![exe.as_str()];
                argv.extend(SLEEPER_ARGS);
                write_record(tmp.path(), pid, start_time(pid).unwrap(), MARK, &argv);
                let ended = Ledger::new(tmp.path()).reap_orphans();
                let status = if expected == 0 {
                    sleeper.kill().unwrap();
                    None
                } else {
                    Some(sleeper.wait().unwrap())
                };
                let _ = sleeper.wait();
                assert_eq!(ended, expected, "marker {marker:?}");
                if let Some(status) = status {
                    use std::os::unix::process::ExitStatusExt as _;
                    assert_eq!(status.signal(), Some(libc::SIGKILL));
                }
            }
        }

        /// 리더 없는 그룹에 표지 없는 그룹원이 하나라도 섞이면 끄지 않는다. 짝: 모두 표지를 지니면 끈다.
        #[test]
        fn a_leaderless_group_is_ended_only_when_every_member_carries_the_marker() {
            for (script, expected) in [
                (
                    format!(
                        "{} & env -u {MARKER_ENV} {} & exit 0",
                        sleeper_cmd(300),
                        sleeper_cmd(301)
                    ),
                    0,
                ),
                (
                    format!("{} & {} & exit 0", sleeper_cmd(300), sleeper_cmd(301)),
                    1,
                ),
            ] {
                let tmp = tempfile::tempdir().unwrap();
                let start = me_start();
                let group = leaderless_group(&script, Some(MARK), 2);
                std::thread::sleep(Duration::from_millis(300));
                write_record(tmp.path(), group, start, MARK, &["/bin/sh"]);
                let ended = Ledger::new(tmp.path()).reap_orphans();
                let empty = emptied(group);
                kill_group(group);
                assert_eq!(ended, expected, "{script}");
                assert_eq!(empty, expected == 1, "{script}");
            }
        }

        /// 기록과 부팅 시각이 다르면(재부팅을 건넜다) 끄지 않고 지운다.
        #[test]
        fn a_record_from_another_boot_is_only_removed() {
            let tmp = tempfile::tempdir().unwrap();
            let start = me_start();
            let group = leaderless_group(&format!("{} & exit 0", sleeper_cmd(300)), Some(MARK), 1);
            std::thread::sleep(Duration::from_millis(300));
            write_record(tmp.path(), group, start, MARK, &["/bin/sh"]);
            let path = tmp.path().join(group.to_string());
            let other_boot = fs::read_to_string(&path).unwrap().replace(
                &format!("boot {}", boot_time().unwrap()),
                &format!("boot {}", boot_time().unwrap() - 3600),
            );
            fs::write(&path, other_boot).unwrap();
            let ended = Ledger::new(tmp.path()).reap_orphans();
            let left = group_members(group);
            kill_group(group);
            assert_eq!(ended, 0);
            assert_eq!(left.len(), 1);
            assert!(!path.exists(), "the record should be removed");
        }
    }
}
