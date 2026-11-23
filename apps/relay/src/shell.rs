//! 원격 셸이 부르는 하위 명령(18-m7-plan M7-15 · 메인 판정 M7-P12): `askpass` · `open` · `link-command`, 그리고 바이너리를
//! `pitwall` 이름(사람이 고른 `~/.local/bin/pitwall` 링크)으로 불렀을 때의 해석.
//!
//! `askpass` · `open` 은 데몬 소켓의 좁은 명령이다(`pw_relay::command`) — 데몬이 없으면 띄우지 않는다(붙은 Desktop 이 없으면 어차피
//! 물을 데가 없다).

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use pw_relay::command::Command;
use serde_json::{Value, json};

/// `pitwall` 이름으로 불렀을 때 읽는 하위 명령(메인 판정 M7-P12). 그 밖의 첫 인자는 열 경로다(`pitwall README.md`).
pub const PITWALL_SUBCOMMANDS: &[&str] = &["open", "mcp", "tool"];

/// 바이너리를 부른 이름이 `pitwall` 인가(`argv[0]` 의 마지막 마디).
pub fn called_as_pitwall(argv0: Option<&OsString>) -> bool {
    argv0
        .map(Path::new)
        .and_then(Path::file_name)
        .is_some_and(|name| name == "pitwall")
}

/// `pitwall …` 의 인자를 `pitwall-relay` 의 인자로 바꾼다. `None` 이면 쓰는 법을 찍는다.
pub fn pitwall_args(args: &[String]) -> Option<Vec<String>> {
    match args.first().map(String::as_str) {
        None => None,
        Some("--version" | "version") => Some(args.to_vec()),
        Some(first) if PITWALL_SUBCOMMANDS.contains(&first) => Some(args.to_vec()),
        Some(_) => Some(
            std::iter::once("open".to_owned())
                .chain(args.iter().cloned())
                .collect(),
        ),
    }
}

pub const PITWALL_USAGE: &str = "usage: pitwall <file>[:line[:col]]... | pitwall open <file>... | pitwall mcp | pitwall tool <name> [<json>]";

/// `mcp` · `tool`(M7-20 이 채운다).
pub fn not_yet(command: &str) -> ExitCode {
    eprintln!("pitwall {command}: not available on this relay yet");
    ExitCode::from(2)
}

/// 좁은 명령 하나를 이 판의 데몬에 보낸다. 데몬이 없으면 띄우지 않는다.
fn ask_daemon(
    socket: &Path,
    command: Command,
    body: Value,
    timeout: Duration,
) -> Result<pw_relay::command::Reply, String> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    let uid = pw_relay::socket::current_uid();
    rt.block_on(async {
        match tokio::time::timeout(
            timeout,
            pw_relay::command::request(socket, uid, command, body),
        )
        .await
        {
            Ok(Ok(reply)) => Ok(reply),
            Ok(Err(e)) => Err(format!("cannot reach the relay daemon: {e}")),
            Err(_) => Err(format!("no answer in {} s", timeout.as_secs())),
        }
    })
}

/// `askpass <질문>`: Relay 의 git · ssh 가 부르는 도우미(`pw_git::write::askpass` 의 `client_main` 과 같은 약속 — 0 답을 stdout
/// 에, 1 답이 없다, 2 git 이 띄운 것이 아니다). 소켓은 환경(`PITWALL_ASKPASS_SOCKET` — 데몬 소켓)으로 받는다.
pub fn askpass(args: &[String]) -> ExitCode {
    use pw_git::write::askpass::{CLIENT_TIMEOUT, SOCKET_ENV, SSH_PROMPT_ENV, TOKEN_ENV, hint_of};
    use std::io::Write as _;

    let prompt = args.first().cloned().unwrap_or_default();
    let (Some(socket), Some(token)) = (
        std::env::var_os(SOCKET_ENV),
        std::env::var_os(TOKEN_ENV).and_then(|t| t.into_string().ok()),
    ) else {
        eprintln!(
            "pitwall-relay askpass: git runs this helper for Pitwall; {SOCKET_ENV} and {TOKEN_ENV} are not set"
        );
        return ExitCode::from(2);
    };
    let hint = hint_of(std::env::var_os(SSH_PROMPT_ENV).as_deref());
    let body = pw_git::write::Askpass::request_body(&token, &prompt, hint);
    match ask_daemon(Path::new(&socket), Command::Askpass, body, CLIENT_TIMEOUT) {
        Ok(Ok(answer)) => {
            let Some(value) = answer.get("value").and_then(Value::as_str) else {
                eprintln!("pitwall-relay askpass: the relay answered without a value");
                return ExitCode::from(1);
            };
            let mut out = std::io::stdout().lock();
            let written = out
                .write_all(value.as_bytes())
                .and_then(|()| out.write_all(b"\n"))
                .and_then(|()| out.flush());
            if written.is_ok() {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            }
        }
        Ok(Err(e)) | Err(e) => {
            eprintln!("pitwall-relay askpass: {e}");
            ExitCode::from(1)
        }
    }
}

/// 열기 요청 하나가 붙은 Desktop 에 닿기까지 기다리는 시간(데몬은 묻기 2초 + 열기 10초).
const OPEN_TIMEOUT: Duration = Duration::from_secs(20);

/// `open <경로>[:줄[:열]]...`: 붙은 Desktop 가운데 마지막으로 사람 입력이 있던 것에 연다(결정 M7-D3). 상대 경로는 이 프로세스의
/// cwd 기준이다. 하나라도 열지 못하면 그 까닭을 찍고 1.
pub fn open(socket: &Path, args: &[String]) -> ExitCode {
    if args.is_empty() {
        eprintln!("usage: pitwall-relay open <file>[:line[:col]]...");
        return ExitCode::from(2);
    }
    let cwd = match std::env::current_dir() {
        Ok(cwd) => cwd,
        Err(e) => {
            eprintln!("pitwall: cannot read the current folder: {e}");
            return ExitCode::from(1);
        }
    };
    let mut failed = false;
    for arg in args {
        let (path, line, column) = target(arg, &cwd, |p| p.exists());
        let body = json!({ "path": path, "line": line, "column": column });
        match ask_daemon(socket, Command::Open, body, OPEN_TIMEOUT) {
            Ok(Ok(_)) => {}
            Ok(Err(e)) | Err(e) => {
                eprintln!("pitwall: cannot open {}: {e}", path.display());
                failed = true;
            }
        }
    }
    if failed {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

/// `path[:line[:col]]` 을 가르고 `cwd` 로 절대화한다. 가르는 규칙은 Desktop 의 `pitwall <path>` 와 같다
/// (`apps/desktop/src-tauri/src/cli.rs` 의 `location` — 덜 가른 것부터 있는 경로를, 없으면 가장 많이 가른 것을).
pub fn target(
    arg: &str,
    cwd: &Path,
    exists: impl Fn(&Path) -> bool,
) -> (PathBuf, Option<u32>, Option<u32>) {
    let absolute = |p: &str| {
        let p = Path::new(p);
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            cwd.join(p)
        }
    };
    let mut readings = vec![(arg, None, None)];
    if let Some((rest, last)) = split_number(arg) {
        readings.push((rest, Some(last), None));
        if let Some((path, line)) = split_number(rest) {
            readings.push((path, Some(line), Some(last)));
        }
    }
    let (path, line, col) = readings
        .iter()
        .find(|(path, ..)| exists(&absolute(path)))
        .or(readings.last())
        .copied()
        .unwrap_or((arg, None, None));
    (absolute(path), line, col)
}

fn split_number(s: &str) -> Option<(&str, u32)> {
    let (rest, digits) = s.rsplit_once(':')?;
    if rest.is_empty() || digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let n: u32 = digits.parse().ok()?;
    (n >= 1).then_some((rest, n))
}

/// `link-command [--remove]`(메인 판정 M7-P12): `~/.local/bin/pitwall` 을 이 바이너리로 잇는 심링크를 두거나 걷는다. 설치 화면에서
/// 사람이 골랐을 때만 Desktop 이 SSH 로 부른다(M7-07b) — 사람의 PATH 를 말없이 고치지 않는다. 그 자리에 Relay 의 것이 아닌 파일이
/// 있으면 건드리지 않고 거절한다. stdout 에 JSON 한 줄(`path` · `linked`).
pub fn link_command(args: &[String]) -> ExitCode {
    let remove = match args {
        [] => false,
        [flag] if flag == "--remove" => true,
        _ => {
            eprintln!("usage: pitwall-relay link-command [--remove]");
            return ExitCode::from(2);
        }
    };
    let Some(home) = pw_core::config::PathEnv::current().home else {
        eprintln!("pitwall-relay link-command: no home folder");
        return ExitCode::from(1);
    };
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(e) => {
            eprintln!("pitwall-relay link-command: {e}");
            return ExitCode::from(1);
        }
    };
    let link = home.join(".local").join("bin").join("pitwall");
    let result = if remove {
        unlink(&link).map(|()| false)
    } else {
        make_link(&link, &exe).map(|()| true)
    };
    match result {
        Ok(linked) => {
            println!("{}", json!({ "path": link, "linked": linked }));
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("pitwall-relay link-command: {e}");
            ExitCode::from(1)
        }
    }
}

/// 그 자리의 것이 Relay 가 둔 링크인가(대상 파일 이름이 `pitwall-relay` 로 시작하는 심링크).
fn ours(link: &Path) -> std::io::Result<Option<bool>> {
    match std::fs::symlink_metadata(link) {
        Ok(meta) if meta.file_type().is_symlink() => {
            let target = std::fs::read_link(link)?;
            Ok(Some(target.file_name().is_some_and(|name| {
                name.to_string_lossy().starts_with("pitwall-relay")
            })))
        }
        Ok(_) => Ok(Some(false)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

fn foreign(link: &Path) -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        format!(
            "{} exists and was not made by Pitwall; leaving it alone",
            link.display()
        ),
    )
}

/// `link` 을 `exe` 로 잇는다. 이미 Relay 의 링크면 새 판으로 바꾼다.
pub fn make_link(link: &Path, exe: &Path) -> std::io::Result<()> {
    match ours(link)? {
        Some(false) => return Err(foreign(link)),
        Some(true) => std::fs::remove_file(link)?,
        None => {}
    }
    if let Some(dir) = link.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::os::unix::fs::symlink(exe, link)
}

/// Relay 의 링크면 걷는다. 없으면 그대로 성공.
pub fn unlink(link: &Path) -> std::io::Result<()> {
    match ours(link)? {
        Some(false) => Err(foreign(link)),
        Some(true) => std::fs::remove_file(link),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `pitwall` 이름으로 부르면 `open` · `mcp` · `tool` 은 그 하위 명령, 그 밖의 첫 인자는 열 경로다 — `daemon` · `pair` 같은 관리
    /// 명령은 `pitwall` 이름으로 닿지 않는다(열 경로로 읽힌다).
    #[test]
    fn the_pitwall_name_reads_open_mcp_and_tool() {
        assert!(called_as_pitwall(Some(&OsString::from(
            "/home/me/.local/bin/pitwall"
        ))));
        assert!(called_as_pitwall(Some(&OsString::from("pitwall"))));
        assert!(!called_as_pitwall(Some(&OsString::from("pitwall-relay"))));
        assert!(!called_as_pitwall(None));
        let read =
            |args: &[&str]| pitwall_args(&args.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>());
        assert_eq!(
            read(&["README.md"]),
            Some(vec!["open".into(), "README.md".into()])
        );
        assert_eq!(read(&["open", "a"]), Some(vec!["open".into(), "a".into()]));
        assert_eq!(read(&["mcp"]), Some(vec!["mcp".into()]));
        assert_eq!(read(&["tool", "x"]), Some(vec!["tool".into(), "x".into()]));
        assert_eq!(
            read(&["daemon"]),
            Some(vec!["open".into(), "daemon".into()])
        );
        assert_eq!(
            read(&["pair", "--from-ssh"]),
            Some(vec!["open".into(), "pair".into(), "--from-ssh".into()])
        );
        assert_eq!(read(&[]), None);
    }

    #[test]
    fn targets_are_absolute_with_line_and_column() {
        let cwd = Path::new("/work");
        assert_eq!(
            target("a.rs:3:2", cwd, |_| false),
            (PathBuf::from("/work/a.rs"), Some(3), Some(2))
        );
        assert_eq!(
            target("/x/b:12", cwd, |p| p == Path::new("/x/b:12")),
            (PathBuf::from("/x/b:12"), None, None)
        );
        assert_eq!(
            target("c", cwd, |_| false),
            (PathBuf::from("/work/c"), None, None)
        );
    }

    /// 링크는 Relay 의 것만 바꾸고 걷는다 — 사람이 둔 파일 · 다른 링크는 그대로 두고 거절한다.
    #[test]
    fn the_command_link_touches_only_its_own_link() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("pitwall-relay-dev-1.0.0");
        std::fs::write(&exe, "").unwrap();
        let link = dir.path().join("bin").join("pitwall");
        make_link(&link, &exe).unwrap();
        assert_eq!(std::fs::read_link(&link).unwrap(), exe);
        let newer = dir.path().join("pitwall-relay-dev-1.1.0");
        make_link(&link, &newer).unwrap();
        assert_eq!(std::fs::read_link(&link).unwrap(), newer);
        unlink(&link).unwrap();
        assert!(std::fs::symlink_metadata(&link).is_err());
        unlink(&link).unwrap();

        std::fs::write(&link, "#!/bin/sh\n").unwrap();
        assert!(make_link(&link, &exe).is_err());
        assert!(unlink(&link).is_err());
        assert_eq!(std::fs::read_to_string(&link).unwrap(), "#!/bin/sh\n");
        std::fs::remove_file(&link).unwrap();
        std::os::unix::fs::symlink("/usr/bin/true", &link).unwrap();
        assert!(make_link(&link, &exe).is_err());
        assert_eq!(
            std::fs::read_link(&link).unwrap(),
            PathBuf::from("/usr/bin/true")
        );
    }
}
