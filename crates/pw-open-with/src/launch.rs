//! 치환한 argv 를 셸 없이 띄운다.
//!
//! - 실행 파일: 이름이면 로그인 셸 PATH(`pw_runtime`)에서 찾고, 경로면 절대 경로여야 한다. 찾은 절대 경로를 띄운다 — 못 찾은 것과
//!   띄우다 실패한 것을 갈라 알린다.
//! - 자식은 PATH 를 그 PATH 로 받는다(`code` 처럼 셸 스크립트인 런처가 `node` 등을 찾게).
//! - Pitwall 과 따로 산다: unix 는 `setsid` 로 새 세션이 되어 Pitwall 의 터미널 · 프로세스 그룹의 신호(Ctrl+C, 탭 닫기)를 받지 않고,
//!   Pitwall 이 끝나면 launchd · init 이 넘겨받는다. stdio 는 모두 `/dev/null` 이다 — 파이프를 주면 Pitwall 이 끝난 뒤 쓰기에서
//!   SIGPIPE 로 죽는다. Windows 는 `DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP`.
//! - 좀비를 남기지 않는다: 스레드 하나가 자식을 `wait` 한다.
//! - 곧바로 실패로 끝난 런처(`open -a` 가 앱을 못 찾음, CLI 가 인자를 거절함)는 [`EARLY_EXIT`] 안에 0 이 아닌 코드로 끝난 것으로
//!   알린다. 그 시간이 지나도 도는 프로세스는 띄운 것으로 본다. Windows 는 보지 않는다 — `explorer.exe` 는 성공해도 1 로 끝난다.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

/// 띄운 뒤 곧바로 실패하는지 지켜보는 시간.
pub const EARLY_EXIT: Duration = Duration::from_millis(1500);

/// `program` 을 실행할 절대 경로. 이름이면 `path`(PATH 형식)에서 찾는다. 상대 경로(`./x`, `bin/x`)는 받지 않는다 — 무엇의
/// 상대인지 사람이 정할 수 없다.
pub fn find_program(program: &str, path: &OsStr) -> Option<PathBuf> {
    let as_path = Path::new(program);
    if as_path.is_absolute() {
        return is_executable(as_path).then(|| as_path.to_path_buf());
    }
    if program.contains('/') || (cfg!(windows) && program.contains('\\')) {
        return None;
    }
    std::env::split_paths(path)
        .filter(|dir| dir.is_absolute())
        .flat_map(|dir| candidates(&dir, program))
        .find(|p| is_executable(p))
}

#[cfg(windows)]
fn candidates(dir: &Path, program: &str) -> Vec<PathBuf> {
    let exts = std::env::var("PATHEXT").unwrap_or_else(|_| DEFAULT_PATHEXT.to_owned());
    windows_names(program, &exts)
        .into_iter()
        .map(|name| dir.join(name))
        .collect()
}

#[cfg(not(windows))]
fn candidates(dir: &Path, program: &str) -> Vec<PathBuf> {
    vec![dir.join(program)]
}

/// 실행 파일이 macOS 의 `open` 인가. `open` 은 받은 경로를 LaunchServices 로 다시 해석해, 번들 폴더([`is_bundle`])는 열지 않고
/// **실행한다**(`open Tool.app`). `-R`(골라 보여 주기) · `-a <앱>`(그 앱에 문서로 넘기기)은 번들을 실행하지 않는 것을 실측했지만
/// (2026-10, macOS 27.0.1), 사용자가 덮어쓴 명령의 옵션까지 보지 않고 `open` 이면 번들 경로를 넘기지 않는다.
pub fn is_launch_services_open(program: &Path) -> bool {
    cfg!(target_os = "macos") && program.file_name().is_some_and(|n| n == "open")
}

/// 번들 확장자(대소문자 무시). LaunchServices 가 패키지로 여는 흔한 것.
const BUNDLE_EXTENSIONS: [&str; 13] = [
    "app",
    "appex",
    "bundle",
    "framework",
    "plugin",
    "xpc",
    "prefpane",
    "saver",
    "kext",
    "workflow",
    "action",
    "qlgenerator",
    "mdimporter",
];

/// `open` 에 넘기면 실행될 수 있는 번들 폴더인가: 폴더이고 ① 이름이 번들 확장자로 끝나거나 ② `Contents/Info.plist` 가 있다
/// (확장자 없이 번들 비트로 패키지가 되는 폴더까지 보수적으로 잡는다). 폴더가 아니면 번들이 아니다.
pub fn is_bundle(path: &Path) -> bool {
    if !path.is_dir() {
        return false;
    }
    let by_extension = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| BUNDLE_EXTENSIONS.iter().any(|b| b.eq_ignore_ascii_case(e)));
    by_extension || path.join("Contents/Info.plist").is_file()
}

/// PATHEXT 가 없을 때.
#[cfg_attr(not(windows), allow(dead_code))]
const DEFAULT_PATHEXT: &str = ".COM;.EXE;.BAT;.CMD";

/// Windows 에서 `program` 이름으로 찾아볼 파일 이름(cmd.exe 의 찾기 규칙). 이름이 PATHEXT 확장자로 끝나면 그 이름만, 아니면
/// PATHEXT 를 붙인 이름만 본다 — 확장자 없는 이름 그대로는 보지 않는다. VS Code · Cursor 의 `bin` 에는 WSL · Git Bash 용
/// 확장자 없는 셸 스크립트 `code` 와 `code.cmd` 가 함께 있어, 그대로 보면 셸 스크립트가 잡혀 띄우기에 실패한다.
/// 플랫폼 무관한 순수 함수라 macOS 에서도 시험한다.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn windows_names(program: &str, pathext: &str) -> Vec<String> {
    let exts: Vec<&str> = pathext.split(';').filter(|e| !e.is_empty()).collect();
    let lower = program.to_ascii_lowercase();
    if exts
        .iter()
        .any(|e| lower.ends_with(&e.to_ascii_lowercase()))
    {
        return vec![program.to_owned()];
    }
    exts.iter().map(|e| format!("{program}{e}")).collect()
}

/// 자식에게 줄 PATH: 절대 경로 항목만 남긴다. 상대 · 빈 항목(`.`, `node_modules/.bin`)은 자식의 cwd(남의 저장소일 수 있는
/// 프로젝트 루트) 기준으로 풀려, 셸 스크립트 런처가 찾는 보조 명령으로 저장소 안의 파일이 잡힐 수 있다.
pub fn absolute_path_entries(path: &OsStr) -> std::ffi::OsString {
    std::env::join_paths(std::env::split_paths(path).filter(|p| p.is_absolute()))
        .unwrap_or_default()
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|m| m.is_file())
}

/// 띄운 결과.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Spawned {
    /// 아직 돌거나 성공(0)으로 끝났다.
    Running { pid: u32 },
    /// [`EARLY_EXIT`] 안에 0 이 아닌 코드로 끝났다. 신호로 끝났으면 `code` 가 없다.
    Exited { pid: u32, code: Option<i32> },
}

/// 떼어 띄우고, `early_exit` 동안 곧바로 실패하는지 본다. 띄우지 못하면 그 IO 오류.
pub fn spawn_detached(
    program: &Path,
    args: &[String],
    cwd: &Path,
    path: &OsStr,
    early_exit: Duration,
) -> std::io::Result<Spawned> {
    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(cwd)
        .env("PATH", absolute_path_entries(path))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    detach(&mut command);
    let mut child = command.spawn()?;
    let pid = child.id();
    let (tx, rx) = mpsc::channel();
    // 자식을 거둔다. 오래 도는 도구(IDE 를 직접 띄우는 런처)면 이 스레드는 그동안 기다리기만 한다.
    let reaper = std::thread::Builder::new()
        .name(format!("reap-{pid}"))
        .spawn(move || {
            let status = child.wait();
            let _ = tx.send(status);
        });
    if let Err(e) = reaper {
        tracing::warn!(target: "pw_open_with", %e, pid, "cannot start the reaper thread; the tool may stay a zombie");
        return Ok(Spawned::Running { pid });
    }
    if cfg!(windows) {
        return Ok(Spawned::Running { pid });
    }
    Ok(match rx.recv_timeout(early_exit) {
        Ok(Ok(status)) if !status.success() => Spawned::Exited {
            pid,
            code: status.code(),
        },
        _ => Spawned::Running { pid },
    })
}

#[cfg(unix)]
fn detach(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    // SAFETY: fork 와 exec 사이에서 async-signal-safe 인 setsid(2) 하나만 부른다.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

#[cfg(windows)]
fn detach(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
}

#[cfg(not(any(unix, windows)))]
fn detach(_command: &mut Command) {}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::time::Instant;

    use super::*;

    fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[test]
    fn finds_programs_on_the_given_path_only() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("my bin");
        std::fs::create_dir(&bin).unwrap();
        let tool = script(&bin, "tool", "true");
        std::fs::write(bin.join("not-exec"), "x").unwrap();
        let path = std::env::join_paths(["relative/dir".into(), bin.clone()]).unwrap();
        assert_eq!(find_program("tool", &path), Some(tool.clone()));
        assert_eq!(find_program("not-exec", &path), None);
        assert_eq!(find_program("tool", OsStr::new("/usr/bin:/bin")), None);
        assert_eq!(
            find_program(tool.to_str().unwrap(), OsStr::new("")),
            Some(tool)
        );
        assert_eq!(find_program("my bin/tool", &path), None);
        // PATH 의 상대 폴더는 보지 않는다 — 무엇의 상대인지(앱의 cwd)에 따라 다른 실행 파일이 잡힌다. 이 상대 경로는 지금 cwd 에서
        // 보면 `bin` 을 가리킨다.
        let cwd = std::env::current_dir().unwrap();
        let up = "../".repeat(cwd.components().count() - 1);
        let relative = PathBuf::from(format!(
            "{up}{}",
            bin.display().to_string().trim_start_matches('/')
        ));
        assert!(
            relative.is_relative() && relative.join("tool").is_file(),
            "{}",
            relative.display()
        );
        assert_eq!(find_program("tool", relative.as_os_str()), None);
        assert_eq!(find_program("./tool", &path), None);
    }

    #[test]
    fn recognizes_bundle_folders() {
        let dir = tempfile::tempdir().unwrap();
        let app = dir.path().join("Tool.APP");
        std::fs::create_dir(&app).unwrap();
        assert!(is_bundle(&app));
        let plain = dir.path().join("plain");
        std::fs::create_dir_all(plain.join("Contents")).unwrap();
        assert!(!is_bundle(&plain));
        std::fs::write(plain.join("Contents/Info.plist"), "<plist/>").unwrap();
        assert!(
            is_bundle(&plain),
            "a folder with Contents/Info.plist is a bundle"
        );
        let file = dir.path().join("notes.app");
        std::fs::write(&file, "x").unwrap();
        assert!(!is_bundle(&file), "a file is not a bundle folder");
        assert_eq!(
            is_launch_services_open(Path::new("/usr/bin/open")),
            cfg!(target_os = "macos")
        );
        assert!(!is_launch_services_open(Path::new("/usr/bin/xdg-open")));
    }

    #[test]
    fn windows_tries_only_pathext_names() {
        let ext = ".COM;.EXE;.BAT;.CMD";
        assert_eq!(
            windows_names("code", ext),
            ["code.COM", "code.EXE", "code.BAT", "code.CMD"]
        );
        assert_eq!(windows_names("code.cmd", ext), ["code.cmd"]);
        assert_eq!(windows_names("Explorer.EXE", ext), ["Explorer.EXE"]);
        // PATHEXT 에 없는 확장자는 확장자가 아니다.
        assert_eq!(windows_names("my.tool", ".EXE"), ["my.tool.EXE"]);
    }

    #[test]
    fn the_child_path_keeps_only_absolute_entries() {
        let path =
            std::env::join_paths([".", "/usr/bin", "node_modules/.bin", "", "/bin"]).unwrap();
        assert_eq!(absolute_path_entries(&path), OsStr::new("/usr/bin:/bin"));
    }

    #[test]
    fn reports_a_launcher_that_fails_right_away() {
        let dir = tempfile::tempdir().unwrap();
        let tool = script(dir.path(), "fail", "exit 7");
        let spawned = spawn_detached(
            &tool,
            &[],
            dir.path(),
            OsStr::new("/usr/bin:/bin"),
            EARLY_EXIT,
        )
        .unwrap();
        assert!(
            matches!(spawned, Spawned::Exited { code: Some(7), .. }),
            "{spawned:?}"
        );
    }

    #[test]
    fn a_tool_that_keeps_running_counts_as_launched_without_waiting_for_it() {
        let dir = tempfile::tempdir().unwrap();
        let tool = script(dir.path(), "slow", "sleep 5");
        let started = Instant::now();
        let spawned = spawn_detached(
            &tool,
            &[],
            dir.path(),
            OsStr::new("/usr/bin:/bin"),
            Duration::from_millis(200),
        )
        .unwrap();
        assert!(started.elapsed() < Duration::from_secs(2));
        let Spawned::Running { pid } = spawned else {
            panic!("{spawned:?}")
        };
        // 새 세션의 리더다 — Pitwall 의 프로세스 그룹 · 터미널 신호와 갈렸다.
        // SAFETY: getsid(2) 는 조회만 한다.
        assert_eq!(
            unsafe { libc::getsid(pid as libc::pid_t) },
            pid as libc::pid_t
        );
        unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) };
    }

    /// 성공한 도구는 거둬진다 — 끝난 뒤 좀비(`ps` 의 Z)로 남지 않는다.
    #[test]
    fn reaps_the_child() {
        let dir = tempfile::tempdir().unwrap();
        let tool = script(dir.path(), "quick", "exit 0");
        let spawned = spawn_detached(
            &tool,
            &[],
            dir.path(),
            OsStr::new("/usr/bin:/bin"),
            EARLY_EXIT,
        )
        .unwrap();
        let Spawned::Running { pid } = spawned else {
            panic!("{spawned:?}")
        };
        let deadline = Instant::now() + Duration::from_secs(3);
        // 거둔 뒤에는 pid 가 없다(kill 0 이 ESRCH). 좀비면 kill 0 이 성공한다.
        // SAFETY: 신호 0 은 존재만 묻는다.
        while unsafe { libc::kill(pid as libc::pid_t, 0) } == 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_ne!(
            unsafe { libc::kill(pid as libc::pid_t, 0) },
            0,
            "{pid} is still there (zombie?)"
        );
    }
}
