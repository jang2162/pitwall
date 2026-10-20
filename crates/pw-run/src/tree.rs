//! 터미널 세션의 프로세스 트리 끝내기(유닉스).
//!
//! PTY 의 셸은 `setsid` 로 새 세션의 리더가 되므로(portable-pty) 셸이 띄운 것은 모두 그 세션에 든다. 대화형 셸은 작업마다 프로세스
//! 그룹을 따로 만들므로(`sleep 100 &`) 셸의 프로세스 그룹만 죽이면 남는다 — 세션 id(`getsid`)로 찾는다. 셸이 먼저 죽어 자식이
//! init 으로 넘어가도 세션 id 는 그대로라 찾을 수 있다. 스스로 `setsid` 한 데몬은 세션을 떠났으므로 건드리지 않는다.

use std::time::{Duration, Instant};

use libc::pid_t;

/// 세션 `leader` 의 프로세스에 SIGHUP(멈춘 것은 SIGCONT 로 깨운다)을 보내고, `grace` 안에 다 끝나지 않으면 남은 것에 SIGKILL 을
/// 보낸다. 끝날 때까지 블로킹한다.
pub fn terminate_session(leader: pid_t, grace: Duration) {
    terminate_sessions(&[leader], grace);
}

/// [`terminate_session`] 을 여러 세션에 한꺼번에 한다(기다림은 한 번).
pub fn terminate_sessions(leaders: &[pid_t], grace: Duration) {
    signal_sessions(leaders, libc::SIGHUP);
    signal_sessions(leaders, libc::SIGCONT);
    let deadline = Instant::now() + grace;
    while Instant::now() < deadline {
        if members(leaders).is_empty() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let left = members(leaders);
    if !left.is_empty() {
        tracing::debug!(target: "pw_run", ?leaders, ?left, "killing terminal processes that ignored SIGHUP");
    }
    signal_sessions(leaders, libc::SIGKILL);
}

fn signal_sessions(leaders: &[pid_t], signal: libc::c_int) {
    // 셸의 프로세스 그룹은 목록을 못 읽는 OS 에서도 닿게 따로 보낸다.
    for &leader in leaders {
        // SAFETY: kill(2) 은 메모리를 건드리지 않는다. 음수 pid 는 그 프로세스 그룹이다.
        unsafe { libc::kill(-leader, signal) };
    }
    for pid in members(leaders) {
        // SAFETY: 위와 같다.
        unsafe { libc::kill(pid, signal) };
    }
}

/// 세션 `leaders` 중 하나에 든 프로세스(좀비가 아닌 것, `getsid` 가 읽히는 것).
fn members(leaders: &[pid_t]) -> Vec<pid_t> {
    all_pids()
        .into_iter()
        .filter(|&pid| pid > 0 && session_of(pid).is_some_and(|sid| leaders.contains(&sid)))
        .collect()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_process_is_listed_in_its_own_session() {
        let me = pid_t::try_from(std::process::id()).unwrap();
        assert!(all_pids().contains(&me));
        let sid = session_of(me).unwrap();
        assert!(members(&[sid]).contains(&me));
    }
}
