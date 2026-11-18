//! 터미널 · 실행 세션의 프로세스 트리 끝내기와 프로세스 정보(유닉스).
//!
//! PTY 의 셸은 `setsid` 로 새 세션의 리더가 되므로(portable-pty) 셸이 띄운 것은 모두 그 세션에 든다. 대화형 셸은 작업마다 프로세스
//! 그룹을 따로 만들므로(`sleep 100 &`) 셸의 프로세스 그룹만 죽이면 남는다 — 세션 id(`getsid`)로 찾는다. 셸이 먼저 죽어 자식이
//! init 으로 넘어가도 세션 id 는 그대로라 찾을 수 있다.
//!
//! 스스로 `setsid` 한 프로세스는 세션을 떠났다. 터미널([`Reach::Session`])은 그것을 건드리지 않는다 — 사람이 터미널에서 띄운 데몬은
//! 터미널보다 오래 살아야 한다. 실행([`Reach::SessionAndChildren`])은 끝내기를 시작할 때 세션의 프로세스의 **자손**(부모 pid 를 따라)도
//! 함께 끝낸다 — 실행이 띄운 것이 `setsid` 로 빠져나가 포트를 쥔 채 남지 않게. 한 번 찾은 자손은 부모가 먼저 죽어 init 으로
//! 넘어가도 계속 본다. 닿지 않는 것은 끝내기를 시작하기 **전에** 이미 부모를 떠난 것(두 번 fork 한 데몬)이다 — macOS 에는 cgroup 이
//! 없다.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use libc::pid_t;

/// 끝낼 범위.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reach {
    /// 세션의 프로세스만(터미널).
    Session,
    /// 세션의 프로세스와 그 자손(세션을 떠났어도 — 실행).
    SessionAndChildren,
}

/// 세션 `leaders` 의 프로세스에 SIGHUP(멈춘 것은 SIGCONT 로 깨운다)을 보내고, `grace` 안에 다 끝나지 않으면 남은 것에 SIGKILL 을
/// 보낸다(기다림은 한 번). 끝날 때까지 블로킹한다.
pub fn terminate_sessions(leaders: &[pid_t], grace: Duration) {
    end_sessions(leaders, &[(libc::SIGHUP, grace)], Reach::Session);
}

/// 세션 `leaders` 의 프로세스(`reach`)를 단계마다 신호 하나로 끝내 본다 — 단계의 신호(멈춘 것은 SIGCONT 로 깨운다)를 보내고 그
/// 시간 안에 다 끝나면 멈춘다. 모든 단계 뒤에 남은 것에는 SIGKILL 을 보낸다. 끝날 때까지 블로킹한다. 실행(`crate::runs`)의 중지는
/// `[(SIGTERM, 5 s)]`, compose 는 그 앞에 `(SIGINT, …)` 이고, 터미널은 `[(SIGHUP, 300 ms)]` 다.
pub fn end_sessions(leaders: &[pid_t], steps: &[(libc::c_int, Duration)], reach: Reach) {
    let mut known = BTreeSet::new();
    for &(signal, grace) in steps {
        let targets = targets(leaders, reach, &mut known);
        signal_all(leaders, &targets, signal);
        signal_all(leaders, &targets, libc::SIGCONT);
        let deadline = Instant::now() + grace;
        while Instant::now() < deadline {
            if targets_left(leaders, reach, &mut known) {
                std::thread::sleep(Duration::from_millis(10));
            } else {
                return;
            }
        }
    }
    let left = targets(leaders, reach, &mut known);
    if !left.is_empty() {
        tracing::debug!(target: "pw_run", ?leaders, ?left, "killing session processes that outlived the stop signals");
    }
    signal_all(leaders, &left, libc::SIGKILL);
}

fn targets_left(leaders: &[pid_t], reach: Reach, known: &mut BTreeSet<pid_t>) -> bool {
    !targets(leaders, reach, known).is_empty()
}

fn signal_all(leaders: &[pid_t], targets: &[pid_t], signal: libc::c_int) {
    // 셸의 프로세스 그룹은 목록을 못 읽는 OS 에서도 닿게 따로 보낸다.
    for &leader in leaders {
        // SAFETY: kill(2) 은 메모리를 건드리지 않는다. 음수 pid 는 그 프로세스 그룹이다.
        unsafe { libc::kill(-leader, signal) };
    }
    for &pid in targets {
        // SAFETY: 위와 같다.
        unsafe { libc::kill(pid, signal) };
    }
}

/// 끝낼 프로세스: 세션 `leaders` 의 것(좀비가 아닌 것, `getsid` 가 읽히는 것), [`Reach::SessionAndChildren`] 이면 그 자손과 앞서
/// 찾은 것(`known`) 가운데 아직 있는 것.
fn targets(leaders: &[pid_t], reach: Reach, known: &mut BTreeSet<pid_t>) -> Vec<pid_t> {
    let all = all_pids();
    let mut found: BTreeSet<pid_t> = all
        .iter()
        .copied()
        .filter(|&pid| pid > 0 && session_of(pid).is_some_and(|sid| leaders.contains(&sid)))
        .collect();
    if reach == Reach::SessionAndChildren {
        let parents: Vec<(pid_t, pid_t)> = all
            .iter()
            .copied()
            .filter(|&pid| pid > 0 && session_of(pid).is_some())
            .filter_map(|pid| parent_of(pid).map(|parent| (pid, parent)))
            .collect();
        loop {
            let before = found.len();
            for &(pid, parent) in &parents {
                if found.contains(&parent) {
                    found.insert(pid);
                }
            }
            if found.len() == before {
                break;
            }
        }
        found.extend(
            known
                .iter()
                .copied()
                .filter(|&pid| session_of(pid).is_some()),
        );
        known.extend(found.iter().copied());
    }
    found.into_iter().collect()
}

fn session_of(pid: pid_t) -> Option<pid_t> {
    // SAFETY: getsid(2) 는 메모리를 건드리지 않는다.
    let sid = unsafe { libc::getsid(pid) };
    (sid >= 0).then_some(sid)
}

#[cfg(target_os = "macos")]
fn all_pids() -> Vec<pid_t> {
    // 첫 호출은 크기만 본다. 그 사이에 늘 수 있으므로 넉넉히 잡는다.
    // SAFETY: 널 버퍼 · 크기 0 이면 프로세스 수만 돌려준다.
    let count = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
    let Ok(count) = usize::try_from(count) else {
        return Vec::new();
    };
    let mut pids: Vec<pid_t> = vec![0; count + 64];
    let bytes = libc::c_int::try_from(pids.len() * size_of::<pid_t>()).unwrap_or(libc::c_int::MAX);
    // SAFETY: 버퍼와 그 바이트 크기를 함께 넘긴다.
    let filled = unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast(), bytes) };
    pids.truncate(usize::try_from(filled).unwrap_or(0));
    pids
}

#[cfg(target_os = "linux")]
fn all_pids() -> Vec<pid_t> {
    std::fs::read_dir("/proc")
        .map(|entries| {
            entries
                .filter_map(|e| e.ok()?.file_name().to_str()?.parse().ok())
                .collect()
        })
        .unwrap_or_default()
}

/// 목록을 읽지 못하는 OS: 셸의 프로세스 그룹에만 신호가 간다.
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn all_pids() -> Vec<pid_t> {
    Vec::new()
}

/// 부모 pid. 읽지 못하면 `None`.
#[cfg(target_os = "macos")]
pub fn parent_of(pid: pid_t) -> Option<pid_t> {
    bsd_info(pid).and_then(|info| pid_t::try_from(info.pbi_ppid).ok())
}

/// 프로세스의 작업 폴더의 (장치, inode). 읽지 못하면 `None`.
#[cfg(target_os = "macos")]
pub fn cwd_identity(pid: pid_t) -> Option<(u64, u64)> {
    // SAFETY: 모두 0 인 C 구조체는 올바른 값이다.
    let mut info: libc::proc_vnodepathinfo = unsafe { std::mem::zeroed() };
    let size = libc::c_int::try_from(size_of::<libc::proc_vnodepathinfo>()).ok()?;
    // SAFETY: 구조체와 그 바이트 크기를 함께 넘긴다.
    let filled = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDVNODEPATHINFO,
            0,
            (&raw mut info).cast(),
            size,
        )
    };
    let stat = &info.pvi_cdir.vip_vi.vi_stat;
    (filled == size).then(|| (u64::from(stat.vst_dev), stat.vst_ino))
}

/// 폴더 메타데이터의 (장치, inode) — [`cwd_identity`] 와 같은 꼴(macOS 의 `st_dev` 는 32비트다).
#[cfg(target_os = "macos")]
pub fn dir_identity(meta: &std::fs::Metadata) -> (u64, u64) {
    use std::os::unix::fs::MetadataExt as _;
    #[allow(clippy::cast_possible_truncation)] // st_dev 는 32비트 값을 64비트로 늘려 준 것이다.
    (u64::from(meta.dev() as u32), meta.ino())
}

#[cfg(target_os = "macos")]
fn bsd_info(pid: pid_t) -> Option<libc::proc_bsdinfo> {
    // SAFETY: 모두 0 인 C 구조체는 올바른 값이다.
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = libc::c_int::try_from(size_of::<libc::proc_bsdinfo>()).ok()?;
    // SAFETY: 구조체와 그 바이트 크기를 함께 넘긴다.
    let filled =
        unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, (&raw mut info).cast(), size) };
    (filled == size).then_some(info)
}

#[cfg(target_os = "linux")]
pub fn parent_of(pid: pid_t) -> Option<pid_t> {
    proc_stat(pid)?.1.parse().ok()
}

#[cfg(target_os = "linux")]
pub fn cwd_identity(pid: pid_t) -> Option<(u64, u64)> {
    std::fs::metadata(format!("/proc/{pid}/cwd"))
        .ok()
        .map(|meta| dir_identity(&meta))
}

#[cfg(target_os = "linux")]
pub fn dir_identity(meta: &std::fs::Metadata) -> (u64, u64) {
    use std::os::unix::fs::MetadataExt as _;
    (meta.dev(), meta.ino())
}

/// `/proc/<pid>/stat` 의 (상태, 부모 pid). 이름(`(…)`)에 공백 · 괄호가 있을 수 있어 마지막 `)` 뒤를 읽는다.
#[cfg(target_os = "linux")]
fn proc_stat(pid: pid_t) -> Option<(String, String)> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let mut fields = stat[stat.rfind(')')? + 1..].split_whitespace();
    Some((fields.next()?.to_owned(), fields.next()?.to_owned()))
}

/// 정보를 읽지 못하는 OS: 자손을 따라가지 못하고 작업 폴더를 다시 보지 못한다.
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn parent_of(_pid: pid_t) -> Option<pid_t> {
    None
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn cwd_identity(_pid: pid_t) -> Option<(u64, u64)> {
    None
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn dir_identity(meta: &std::fs::Metadata) -> (u64, u64) {
    use std::os::unix::fs::MetadataExt as _;
    (meta.dev(), meta.ino())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_process_is_listed_in_its_own_session() {
        let me = pid_t::try_from(std::process::id()).unwrap();
        assert!(all_pids().contains(&me));
        let sid = session_of(me).unwrap();
        assert!(targets(&[sid], Reach::Session, &mut BTreeSet::new()).contains(&me));
        assert!(parent_of(me).is_some_and(|parent| parent > 0));
        let cwd = std::fs::metadata(std::env::current_dir().unwrap()).unwrap();
        assert_eq!(cwd_identity(me), Some(dir_identity(&cwd)));
    }
}
