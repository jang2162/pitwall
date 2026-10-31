//! 앱이 강제로 끝나(SIGKILL · 크래시 · Tauri 가 잡지 않는 SIGTERM) 남은 ssh 를 다음 시작 때 끈다(M3-11 리뷰 M1).
//!
//! ssh 는 `setsid` 로 자기 세션에 있고 stdin 이 `/dev/null` · `-N` 이라 앱이 죽어도 스스로 끝날 계기가 없다 — 127.0.0.1 포워드가
//! 계속 열려 있다. 그래서 띄운 ssh 마다 상태 디렉터리에 기록([`Record`])을 두고, 앱이 시작할 때 [`reap_orphans`] 가 기록을 읽어
//! **세 가지가 모두 맞는 것만** 끈다:
//!
//! 1. pid 의 **시작 시각**이 기록과 같다 — pid 는 다시 쓰인다. 같은 pid 의 다른 프로세스(사람의 것)를 끄지 않는다.
//! 2. 그 프로세스의 **argv 끝이 Pitwall 이 준 ssh 인자 그대로**다(`-N … -o ControlPath=none -L 127.0.0.1:<포트>:… -- <목적지>`).
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
    pub(super) fn write(dir: &Path, pid: u32, args: &[OsString]) -> Option<Self> {
        let Some(start) = start_time(pid) else {
            tracing::warn!(target: "pw_db", pid, "cannot read the ssh tunnel's start time; it will not be cleaned up after a crash");
            return None;
        };
        let owner = std::process::id();
        let mut text = format!(
            "{MAGIC}\npid {pid}\nstart {start}\nowner {owner} {}\n",
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
    let args = lines
        .map(|l| l.strip_prefix("arg ").map(str::to_owned))
        .collect::<Option<Vec<_>>>()?;
    if start.is_empty() || args.is_empty() {
        return None;
    }
    Some(Parsed {
        pid,
        start,
        owner: owner.parse().ok()?,
        owner_start: owner_start.to_owned(),
        args,
    })
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
            && args_end_with(record.pid, &record.args)
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

/// argv 끝이 `args` 그대로인가(앞은 실행 파일 — 진짜 ssh 면 `ssh`, 시험의 스크립트면 `/bin/sh <스크립트>`).
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn args_end_with(pid: u32, args: &[String]) -> bool {
    argv(pid).is_some_and(|argv| argv.len() > args.len() && argv.ends_with(args))
}

#[cfg(all(unix, not(any(target_os = "macos", target_os = "linux"))))]
fn args_end_with(_pid: u32, _args: &[String]) -> bool {
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

/// 명령줄이 ` <args 를 공백으로 이은 것>` 으로 끝나는가. 인자에 공백 · 따옴표가 없어 `Command` 가 따옴표를 붙이지 않는다.
#[cfg(windows)]
fn args_end_with(pid: u32, args: &[String]) -> bool {
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
    text.trim_end().ends_with(&format!(" {}", args.join(" ")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_record_round_trips_and_unknown_shapes_are_refused() {
        let text =
            format!("{MAGIC}\npid 42\nstart 1.000001\nowner 7 9.5\narg -N\narg --\narg bastion\n");
        assert_eq!(
            parse(&text),
            Some(Parsed {
                pid: 42,
                start: "1.000001".into(),
                owner: 7,
                owner_start: "9.5".into(),
                args: vec!["-N".into(), "--".into(), "bastion".into()],
            })
        );
        for bad in [
            "pitwall-ssh-tunnel 2\npid 42\nstart 1\nowner 7 9\narg -N\n".to_owned(),
            format!("{MAGIC}\npid x\nstart 1\nowner 7 9\narg -N\n"),
            format!("{MAGIC}\npid 42\nstart \nowner 7 9\narg -N\n"),
            format!("{MAGIC}\npid 42\nstart 1\nowner 7 9\n"),
            format!("{MAGIC}\npid 42\nstart 1\nowner 7 9\nrm -rf\n"),
        ] {
            assert_eq!(parse(&bad), None, "{bad:?}");
        }
    }
}
