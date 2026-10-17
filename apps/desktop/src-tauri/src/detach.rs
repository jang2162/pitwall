//! 콜드 스타트한 CLI 는 앱을 떼어 띄우고 곧바로 끝난다(docs/design/04-features.md F2 「명령줄 규칙」 — `code` · `zed` 런처와
//! 같다). 터미널의 `pitwall .` 은 프롬프트를 곧바로 돌려주고, 그 뒤 Ctrl+C · 탭 닫기(SIGINT · SIGHUP)나 Orca 의
//! `spawn` 부모가 끝나도 앱은 살아 있다.
//!
//! 언제: 단일 인스턴스 플러그인이 「기존 앱이 없다」고 판정한 뒤(setup 첫머리). 기존 앱이 있으면 플러그인이 인자를 넘기고
//! 먼저 끝내므로 이 경로에 오지 않는다.
//!
//! 어떻게: 같은 실행 파일을 같은 인자 · cwd 로 다시 띄운다. 자식은 `setsid` 로 새 세션이 되어 제어 터미널이 없고(터미널의
//! SIGHUP · SIGINT 가 닿지 않는다), stdio 는 `/dev/null` 이다. 자식에게는 [`MARKER_ENV`] 를 줘 다시 떼지 않게 한다.
//! release 번들도 같은 방법이다 — `open -a`(LaunchServices)는 cwd 와 `PITWALL_*_DIR` 같은 환경을 넘기지 않고 번들이 아닌
//! debug 바이너리에는 쓸 수 없다.
//!
//! 떼지 않는 것: 부모가 launchd(pid 1)면 — Finder · Dock · LaunchServices 로 뜬 실행 — 그 프로세스가 앱이다. 표식이 있는
//! 자식도 그대로 앱이 된다. 개발 서버 아래(`mise run dev`, [`DEV_SERVER_ENV`] 가 있다)도 떼지 않는다 — `tauri dev` 는 자기가
//! 띄운 프로세스가 끝나면 앱이 끝난 것으로 본다. Windows 는 떼지 않는다: release 는 GUI 서브시스템이라 cmd · PowerShell 이 기다리지 않고 콘솔에도
//! 붙지 않는다(debug 콘솔 빌드는 이월).

/// 떼어 띄운 자식이라는 표식. 자식은 시작하자마자 지운다 — 앱이 띄우는 터미널 · 프로세스가 물려받으면 그 안의 `pitwall` 이
/// 떼지 않게 된다.
pub const MARKER_ENV: &str = "PITWALL_DETACHED";

/// `apps/desktop/scripts/dev.mjs` 가 `tauri dev` 에 주는 개발 서버 포트.
pub const DEV_SERVER_ENV: &str = "PITWALL_DEV_PORT";

/// 떼어 띄울지. `parent` 는 부모 pid(모르면 `None` — 떼지 않는다), `marked` 는 [`MARKER_ENV`] 가 있었는지,
/// `dev_server` 는 [`DEV_SERVER_ENV`] 가 있는지.
pub fn should_detach(parent: Option<u32>, marked: bool, dev_server: bool) -> bool {
    !marked && !dev_server && parent.is_some_and(|pid| pid != 1)
}

/// 이 프로세스의 부모 pid. Windows 에서는 `None`(위 머리 주석).
#[cfg(unix)]
pub fn parent_pid() -> Option<u32> {
    Some(std::os::unix::process::parent_id())
}

#[cfg(not(unix))]
pub fn parent_pid() -> Option<u32> {
    None
}

/// 같은 실행 파일 · 인자 · cwd 로 떼어 낸 자식을 띄운다. 자식의 pid 를 돌려준다.
#[cfg(unix)]
pub fn respawn() -> std::io::Result<u32> {
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};

    // 심링크(`/usr/local/bin/pitwall`)로 실행했어도 앱의 명령줄은 실제 바이너리 경로가 되게 푼다 — pid 를 빌드 경로로
    // 찾는다(AGENTS.md 「데스크톱 앱은 이름이 아니라 pid 로」).
    let exe = std::env::current_exe()?;
    let mut command = Command::new(std::fs::canonicalize(&exe).unwrap_or(exe));
    command
        .args(std::env::args_os().skip(1))
        .env(MARKER_ENV, "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // SAFETY: fork 와 exec 사이에서 async-signal-safe 인 setsid(2) 하나만 부른다.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    Ok(command.spawn()?.id())
}

#[cfg(not(unix))]
pub fn respawn() -> std::io::Result<u32> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "detaching is unix only",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cli_started_from_a_shell_or_another_process_detaches() {
        assert!(should_detach(Some(4242), false, false));
    }

    #[test]
    fn a_launch_from_launchd_stays_the_app() {
        // Finder · Dock · `open -a` 의 부모는 launchd.
        assert!(!should_detach(Some(1), false, false));
    }

    #[test]
    fn the_detached_child_does_not_detach_again() {
        assert!(!should_detach(Some(4242), true, false));
    }

    #[test]
    fn the_app_under_tauri_dev_stays_its_child() {
        assert!(!should_detach(Some(4242), false, true));
    }

    #[test]
    fn an_unknown_parent_stays_the_app() {
        assert!(!should_detach(None, false, false));
    }
}
