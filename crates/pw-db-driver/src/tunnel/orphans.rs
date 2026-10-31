//! 앱이 강제로 끝나(SIGKILL · 크래시 · Tauri 가 잡지 않는 SIGTERM) 남은 ssh 를 다음 시작 때 끈다(M3-11 리뷰 M1).
//!
//! ssh 는 `setsid` 로 자기 세션에 있고 stdin 이 `/dev/null` · `-N` 이라 앱이 죽어도 스스로 끝날 계기가 없다 — 127.0.0.1 포워드가
//! 계속 열려 있다. 그래서 띄운 ssh 마다 상태 디렉터리에 기록([`Record`])을 두고, 앱이 시작할 때 [`reap_orphans`] 가 기록을 읽어
//! **세 가지가 모두 맞는 것만** 끈다:
//!
//! 1. pid 의 **시작 시각**이 기록과 같다 — pid 는 다시 쓰인다. 같은 pid 의 다른 프로세스(사람의 것)를 끄지 않는다.
//! 2. 그 프로세스가 **Pitwall 이 띄우는 꼴의 ssh** 다: 기록의 인자가 우리 고정 argv([`super::ssh_args`] 가 만드는 것 — `-N`,
//!    고정 `-o` 셋(`ControlPath=none` 포함), 기록한 포트의 `-L 127.0.0.1:<포트>:<호스트>:<포트>`, `--`, 허용 글자의 목적지)와 한
//!    글자도 다르지 않고, 프로세스의 argv 끝이 그 인자 그대로이며, 그 앞에 이름이 `ssh` 인 실행 파일이 있다. 기록 파일은
//!    누구든 쓸 수 있는 자리일 수 있으니 기록이 적은 인자를 믿지 않는다 — 위조 기록(남의 프로세스의 argv 끝을 적은 것)으로 남의
//!    프로세스 그룹을 끄지 않게(M3-11 재리뷰).
//! 3. 기록한 **앱이 이미 끝났다**(그 pid 가 없거나 시작 시각이 다르다) — 같은 상태 디렉터리를 쓰는 살아 있는 앱(두 번째 실행이 첫
//!    실행에 넘기기 전 등)의 터널을 끄지 않는다.
//!
//! 맞지 않는 기록은 지우기만 한다. 시작 시각 · argv 를 읽지 못하면 끄지 않는다(모르면 남의 것으로 본다).
//!
//! Linux 는 이것과 별도로 ssh 에 `PR_SET_PDEATHSIG` 를 건다(앱이 죽으면 그 자리에서 SIGTERM). macOS · Windows 는 기록뿐이라 앱이 죽은
//! 뒤 다음 시작까지 포워드가 남는다(14 M3-11 칸).

use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::{STOP_GRACE, Signal, alive, signal_group};

/// 기록 파일의 첫 줄(형식이 바뀌면 숫자를 올린다 — 모르는 형식은 끄지 않고 지운다).
pub(super) const MAGIC: &str = "pitwall-ssh-tunnel 1";

/// 띄운 ssh 하나의 기록. 버리면 파일을 지운다(ssh 가 끝났거나 끝낼 것이다).
pub(super) struct Record {
    path: PathBuf,
}

impl Record {
    /// `dir/<pid>` 에 적는다. 적지 못하면 경고만 남긴다(터널은 그대로 쓴다 — 강제 종료 뒤 정리만 못 한다).
    pub(super) fn write(dir: &Path, pid: u32, port: u16, args: &[OsString]) -> Option<Self> {
        let Some(start) = start_time(pid) else {
            tracing::warn!(target: "pw_db", pid, "cannot read the ssh tunnel's start time; it will not be cleaned up after a crash");
            return None;
        };
        let owner = std::process::id();
        let mut text = format!(
            "{MAGIC}\npid {pid}\nstart {start}\nowner {owner} {}\nport {port}\n",
            start_time(owner).unwrap_or_default()
        );
        for arg in args {
            // 인자는 허용 글자 목록을 지난 값과 고정 글자뿐이라 줄바꿈이 없다.
            text.push_str("arg ");
            text.push_str(&arg.to_string_lossy());
            text.push('\n');
        }
        let path = dir.join(pid.to_string());
        let written = fs::create_dir_all(dir).and_then(|()| {
            let partial = dir.join(format!(".{pid}.partial"));
            let mut file = fs::File::create(&partial)?;
            file.write_all(text.as_bytes())?;
            file.sync_all()?;
            fs::rename(&partial, &path)
        });
        match written {
            Ok(()) => Some(Self { path }),
            Err(e) => {
                tracing::warn!(target: "pw_db", pid, %e, "cannot record the ssh tunnel; it will not be cleaned up after a crash");
                None
            }
        }
    }
}

impl Drop for Record {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// 읽은 기록.
#[derive(Debug, PartialEq, Eq)]
struct Parsed {
    pid: u32,
    start: String,
    owner: u32,
    owner_start: String,
    port: u16,
    args: Vec<String>,
}

fn parse(text: &str) -> Option<Parsed> {
    let mut lines = text.lines();
    if lines.next()? != MAGIC {
        return None;
    }
    let pid = lines.next()?.strip_prefix("pid ")?.parse().ok()?;
    let start = lines.next()?.strip_prefix("start ")?.to_owned();
    let (owner, owner_start) = lines.next()?.strip_prefix("owner ")?.split_once(' ')?;
    let port = lines.next()?.strip_prefix("port ")?.parse().ok()?;
    let args = lines
        .map(|l| l.strip_prefix("arg ").map(str::to_owned))
        .collect::<Option<Vec<_>>>()?;
    if start.is_empty() || !our_shape(port, &args) {
        return None;
    }
    Some(Parsed {
        pid,
        start,
        owner: owner.parse().ok()?,
        owner_start: owner_start.to_owned(),
        port,
        args,
    })
}

/// `args` 가 포트 `port` 로 [`super::ssh_args`] 가 만드는 인자 그대로인가. `-L` 에서 DB 호스트 · 포트를 읽어 같은 함수로 다시 만들어
/// 비교한다 — 꼴의 정본은 `ssh_args` 하나다.
fn our_shape(port: u16, args: &[String]) -> bool {
    let [.., forward, _, destination] = args else {
        return false;
    };
    let Some((host, db_port)) = forward
        .strip_prefix(&format!("{}:{port}:", super::LOOPBACK))
        .and_then(|rest| rest.rsplit_once(':'))
    else {
        return false;
    };
    let Ok(db_port) = db_port.parse() else {
        return false;
    };
    super::ssh_args(port, host, db_port, destination).is_ok_and(|built| {
        built.len() == args.len() && built.iter().zip(args).all(|(b, a)| b.to_str() == Some(a))
    })
}

/// 실행 파일 경로의 이름이 `ssh` 인가(Windows 는 `ssh.exe`, 대소문자 무시).
fn is_ssh(program: &str) -> bool {
    let name = program.rsplit(['/', '\\']).next().unwrap_or(program);
    name == "ssh"
        || (cfg!(windows)
            && (name.eq_ignore_ascii_case("ssh") || name.eq_ignore_ascii_case("ssh.exe")))
}

/// 앱 시작 때: `dir` 의 기록 가운데 앞 실행이 남긴 ssh(머리 주석의 세 가지가 맞는 것)를 프로세스 그룹째 끝내고, 기록을 지운다.
/// 끈 수를 돌려준다. 블로킹이다(신호 뒤 [`STOP_GRACE`] 까지 기다린다) — 지금 실행이 띄우는 ssh 와 겹쳐 돌아도 된다(그 기록은
/// 기록한 앱이 살아 있어 건너뛴다).
pub fn reap_orphans(dir: &Path) -> usize {
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };
    let mut ended = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') {
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
        if start_time(record.pid).as_deref() == Some(record.start.as_str())
            && runs_ssh_with(record.pid, &record.args)
        {
            tracing::info!(target: "pw_db", pid = record.pid, "ending an ssh tunnel left by a previous run");
            signal_group(record.pid, Signal::Term);
            ended.push((record.pid, record.start));
        }
        let _ = fs::remove_file(&path);
    }
    if ended.is_empty() {
        return 0;
    }
    let deadline = std::time::Instant::now() + STOP_GRACE;
    while std::time::Instant::now() < deadline && ended.iter().any(|(pid, _)| alive(*pid)) {
        std::thread::sleep(Duration::from_millis(10));
    }
    for (pid, start) in &ended {
        // 그 사이 pid 가 다시 쓰였을 수 있다 — 같은 프로세스일 때만.
        if alive(*pid) && start_time(*pid).as_ref() == Some(start) {
            signal_group(*pid, Signal::Kill);
        }
    }
    ended.len()
}

/// 기록한 앱이 아직 살아 있는가. 살아 있는데 시작 시각을 읽지 못하면 살아 있다고 본다(끄지 않는 쪽).
fn owner_lives(record: &Parsed) -> bool {
    if !alive(record.owner) {
        return false;
    }
    start_time(record.owner).is_none_or(|start| start == record.owner_start)
}

// --- 프로세스의 시작 시각 · argv(OS 마다) ---------------------------------------------------------------------------

/// 프로세스의 시작 시각(같은 pid 의 다른 프로세스면 다르다). 없거나 읽지 못하면 `None`.
#[cfg(target_os = "macos")]
pub(super) fn start_time(pid: u32) -> Option<String> {
    let pid = libc::c_int::try_from(pid).ok()?;
    // SAFETY: proc_bsdinfo 는 C 구조체(모두 0 이 올바른 값)이고, proc_pidinfo 는 준 크기까지만 쓴다.
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = libc::c_int::try_from(std::mem::size_of::<libc::proc_bsdinfo>()).ok()?;
    let written =
        unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, (&raw mut info).cast(), size) };
    (written == size).then(|| format!("{}.{:06}", info.pbi_start_tvsec, info.pbi_start_tvusec))
}

/// 프로세스의 argv(`argv[0]` 포함).
#[cfg(target_os = "macos")]
fn argv(pid: u32) -> Option<Vec<String>> {
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
    // argc(i32) · 실행 파일 경로 · NUL 채움 · argv 문자열들.
    let argc = usize::try_from(i32::from_ne_bytes(buf.get(..4)?.try_into().ok()?)).ok()?;
    let rest = buf.get(4..)?;
    let path_end = rest.iter().position(|&b| b == 0)?;
    let mut strings = rest[path_end..]
        .split(|&b| b == 0)
        .filter(|s| !s.is_empty());
    let mut args = Vec::with_capacity(argc);
    for _ in 0..argc {
        args.push(String::from_utf8_lossy(strings.next()?).into_owned());
    }
    Some(args)
}

#[cfg(target_os = "linux")]
pub(super) fn start_time(pid: u32) -> Option<String> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // `pid (comm) state …` — comm 에 공백 · 괄호가 있을 수 있어 마지막 `)` 뒤를 센다. starttime 은 22번째 필드다.
    let after = &stat[stat.rfind(')')? + 1..];
    after.split_whitespace().nth(19).map(str::to_owned)
}

#[cfg(target_os = "linux")]
fn argv(pid: u32) -> Option<Vec<String>> {
    let raw = fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    Some(
        raw.split(|&b| b == 0)
            .filter(|s| !s.is_empty())
            .map(|s| String::from_utf8_lossy(s).into_owned())
            .collect(),
    )
}

#[cfg(all(unix, not(any(target_os = "macos", target_os = "linux"))))]
pub(super) fn start_time(_pid: u32) -> Option<String> {
    None
}

/// argv 끝이 `args` 그대로이고 그 앞에 `ssh` 실행 파일이 있는가 — 진짜 ssh 면 `argv[0]`, 시험의 가짜 스크립트면
/// `/bin/sh <…/ssh>` 의 스크립트 경로, 실 sshd 시험의 감싸개(`exec ssh -F <설정> "$@"`)면 `argv[0]` 과 우리 인자 사이에
/// `-F <설정>` 이 낀다.
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn runs_ssh_with(pid: u32, args: &[String]) -> bool {
    argv(pid).is_some_and(|argv| {
        argv.len() > args.len()
            && argv.ends_with(args)
            && argv[..argv.len() - args.len()].iter().any(|a| is_ssh(a))
    })
}

#[cfg(all(unix, not(any(target_os = "macos", target_os = "linux"))))]
fn runs_ssh_with(_pid: u32, _args: &[String]) -> bool {
    false
}

/// 열린 프로세스 핸들(버리면 닫는다).
#[cfg(windows)]
struct Handle(windows_sys::Win32::Foundation::HANDLE);

#[cfg(windows)]
impl Handle {
    fn open(pid: u32) -> Option<Self> {
        use windows_sys::Win32::System::Threading::{
            OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        };
        // SAFETY: OpenProcess 는 핸들을 돌려줄 뿐이다. null 이면 실패.
        let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        (!handle.is_null()).then_some(Self(handle))
    }
}

#[cfg(windows)]
impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: open 이 연 핸들을 한 번 닫는다.
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

#[cfg(windows)]
pub(super) fn start_time(pid: u32) -> Option<String> {
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::Threading::GetProcessTimes;
    let handle = Handle::open(pid)?;
    let zero = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let (mut created, mut exited, mut kernel, mut user) = (zero, zero, zero, zero);
    // SAFETY: 네 FILETIME 에 쓴다.
    let ok = unsafe {
        GetProcessTimes(
            handle.0,
            &raw mut created,
            &raw mut exited,
            &raw mut kernel,
            &raw mut user,
        )
    };
    (ok != 0).then(|| {
        (u64::from(created.dwHighDateTime) << 32 | u64::from(created.dwLowDateTime)).to_string()
    })
}

/// 명령줄이 `<ssh 실행 파일> <args 를 공백으로 이은 것>` 인가. 인자에 공백 · 따옴표가 없어 `Command` 가 따옴표를 붙이지 않는다.
/// 실행 파일은 `Command` 가 따옴표로 감싼다.
#[cfg(windows)]
fn runs_ssh_with(pid: u32, args: &[String]) -> bool {
    use windows_sys::Wdk::System::Threading::{
        NtQueryInformationProcess, ProcessCommandLineInformation,
    };
    use windows_sys::Win32::Foundation::UNICODE_STRING;
    let Some(handle) = Handle::open(pid) else {
        return false;
    };
    // UNICODE_STRING 머리와 그 뒤의 글자들. 명령줄은 32767 UTF-16 글자를 넘지 않는다.
    let mut buf = vec![0u64; (std::mem::size_of::<UNICODE_STRING>() + 65536) / 8 + 1];
    let len = u32::try_from(buf.len() * 8).unwrap_or(u32::MAX);
    let mut returned = 0u32;
    // SAFETY: 준 길이의 버퍼에 쓴다. 성공하면 앞이 UNICODE_STRING 이고 그 Buffer 는 이 버퍼 안을 가리킨다.
    let status = unsafe {
        NtQueryInformationProcess(
            handle.0,
            ProcessCommandLineInformation,
            buf.as_mut_ptr().cast(),
            len,
            &raw mut returned,
        )
    };
    if status != 0 {
        return false;
    }
    let text = unsafe {
        let head = &*buf.as_ptr().cast::<UNICODE_STRING>();
        if head.Buffer.is_null() {
            return false;
        }
        String::from_utf16_lossy(std::slice::from_raw_parts(
            head.Buffer,
            usize::from(head.Length) / 2,
        ))
    };
    text.trim_end()
        .strip_suffix(&format!(" {}", args.join(" ")))
        .is_some_and(|program| is_ssh(program.trim().trim_matches('"')))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ours(port: u16, db_host: &str, destination: &str) -> Vec<String> {
        super::super::ssh_args(port, db_host, 5432, destination)
            .unwrap()
            .into_iter()
            .map(|a| a.into_string().unwrap())
            .collect()
    }

    fn record(port: u16, args: &[String]) -> String {
        let mut text = format!("{MAGIC}\npid 42\nstart 1.000001\nowner 7 9.5\nport {port}\n");
        for arg in args {
            text.push_str(&format!("arg {arg}\n"));
        }
        text
    }

    #[test]
    fn a_record_round_trips_and_unknown_shapes_are_refused() {
        let args = ours(40001, "db.internal", "bastion");
        assert_eq!(
            parse(&record(40001, &args)),
            Some(Parsed {
                pid: 42,
                start: "1.000001".into(),
                owner: 7,
                owner_start: "9.5".into(),
                port: 40001,
                args: args.clone(),
            })
        );
        let v6 = ours(40001, "fd00::5", "ssh://deploy@[fd00::1]:2222");
        assert!(parse(&record(40001, &v6)).is_some(), "{v6:?}");
        for bad in [
            "pitwall-ssh-tunnel 2\npid 42\nstart 1\nowner 7 9\nport 1\narg -N\n".to_owned(),
            format!("{MAGIC}\npid x\nstart 1\nowner 7 9\nport 40001\narg -N\n"),
            format!("{MAGIC}\npid 42\nstart \nowner 7 9\nport 40001\narg -N\n"),
            format!("{MAGIC}\npid 42\nstart 1\nowner 7 9\nport 40001\n"),
            format!("{MAGIC}\npid 42\nstart 1\nowner 7 9\nport 40001\nrm -rf\n"),
            // 포트 줄이 없다(앞 형식).
            format!("{MAGIC}\npid 42\nstart 1\nowner 7 9\narg -N\n"),
        ] {
            assert_eq!(parse(&bad), None, "{bad:?}");
        }
    }

    /// 기록의 인자는 우리 고정 꼴과 한 글자도 다르지 않아야 한다 — 남의 프로세스의 argv 끝을 적은 위조 기록은 읽지 않는다.
    #[test]
    fn a_record_with_args_other_than_ours_is_refused() {
        let good = ours(40001, "db.internal", "bastion");
        let with = |i: usize, value: &str| {
            let mut args = good.clone();
            args[i] = value.to_owned();
            args
        };
        let forged: Vec<(&str, u16, Vec<String>)> = vec![
            ("another program's argv tail", 40001, vec!["30".into()]),
            ("no -N", 40001, with(0, "-T")),
            ("ControlPath kept", 40001, with(6, "ControlPath=/tmp/m")),
            ("another option", 40001, with(4, "ProxyCommand=sh")),
            ("-R in place of -L", 40001, with(7, "-R")),
            (
                "forward off loopback",
                40001,
                with(8, "0.0.0.0:40001:db.internal:5432"),
            ),
            ("another port than recorded", 40002, good.clone()),
            ("db port 0", 40001, with(8, "127.0.0.1:40001:db.internal:0")),
            (
                "db host outside the allowlist",
                40001,
                with(8, "127.0.0.1:40001:a;id:5432"),
            ),
            ("no --", 40001, with(9, "-v")),
            (
                "destination outside the allowlist",
                40001,
                with(10, "a$(id)"),
            ),
            (
                "an extra argument",
                40001,
                [good.clone(), vec!["x".into()]].concat(),
            ),
            ("a missing argument", 40001, good[..good.len() - 1].to_vec()),
        ];
        for (why, port, args) in forged {
            assert_eq!(parse(&record(port, &args)), None, "{why}: {args:?}");
        }
    }

    #[test]
    fn only_a_program_named_ssh_counts() {
        for ok in ["ssh", "/usr/bin/ssh", "/tmp/pw-fake-ssh-1/ssh"] {
            assert!(is_ssh(ok), "{ok}");
        }
        for not in [
            "sleep",
            "/usr/bin/sshd",
            "/bin/sh",
            "ssh-agent",
            "/opt/ssh/bash",
            "",
        ] {
            assert!(!is_ssh(not), "{not}");
        }
    }
}
