//! 「Shell 명령 설치」(docs/design/04-features.md F2-1): 실행 중인 앱 바이너리를 가리키는 심링크를 PATH 의 폴더에 만든다.
//!
//! - 대상: macOS `/usr/local/bin/pitwall`. `PITWALL_SHELL_COMMAND_TARGET`(절대 경로)로 바꿀 수 있다 — 개발 · 시험에서 사람의
//!   시스템 경로를 건드리지 않고 확인하는 자리다(CONTRIBUTING.md 「명령」).
//! - 이미 있을 때: 이 앱을 가리키는 심링크면 그대로 둔다. **다른 곳을 가리키는 심링크면 바꾼다** — 사람이 이 앱에서 「설치」를
//!   골랐으므로 `pitwall` 이 이 앱을 띄우게 하는 것이 요청이다(옮긴 앱 · 다른 빌드의 옛 링크). 무엇을 바꿨는지 알린다.
//!   **심링크가 아닌 파일 · 폴더는 건드리지 않는다** — Pitwall 이 만든 것이 아니다.
//! - 권한이 없을 때(`/usr/local/bin` 은 대개 root 소유이고, Apple Silicon 의 새 macOS 에는 폴더가 없기도 하다): `osascript`
//!   의 `with administrator privileges` 로 macOS 관리자 암호 창을 띄워 `mkdir -p` · `ln -sfn` 을 한다(VS Code 의 「Install
//!   'code' command」 와 같은 방법). 사람이 취소하면 아무것도 바꾸지 않고 끝낸다.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use tauri::{AppHandle, Runtime};

pub const DEFAULT_TARGET: &str = "/usr/local/bin/pitwall";
pub const TARGET_ENV: &str = "PITWALL_SHELL_COMMAND_TARGET";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Installed {
    New,
    /// 이미 이 앱을 가리키고 있었다.
    Already,
    /// 다른 곳을 가리키던 심링크를 바꿨다.
    Replaced {
        previous: PathBuf,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallError {
    /// 대상이 심링크가 아닌 파일 · 폴더다.
    NotALink(PathBuf),
    /// 관리자 권한 요청을 사람이 취소했다.
    Cancelled,
    Failed(String),
}

impl std::fmt::Display for InstallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InstallError::NotALink(path) => write!(
                f,
                "{} already exists and is not a link Pitwall created. Remove it and try again.",
                path.display()
            ),
            InstallError::Cancelled => f.write_str("cancelled"),
            InstallError::Failed(reason) => f.write_str(reason),
        }
    }
}

fn failed(what: &str, path: &Path, e: std::io::Error) -> InstallError {
    InstallError::Failed(format!("{what} {}: {e}", path.display()))
}

/// `target` 에 `source` 를 가리키는 심링크를 둔다. 권한이 없으면 `elevate(source, target)` 로 다시 한다.
pub fn install(
    source: &Path,
    target: &Path,
    elevate: impl FnOnce(&Path, &Path) -> Result<(), InstallError>,
) -> Result<Installed, InstallError> {
    let previous = match fs::symlink_metadata(target) {
        Ok(meta) if meta.file_type().is_symlink() => {
            let current = fs::read_link(target).map_err(|e| failed("cannot read", target, e))?;
            if current == source {
                return Ok(Installed::Already);
            }
            Some(current)
        }
        Ok(_) => return Err(InstallError::NotALink(target.to_path_buf())),
        Err(e) if e.kind() == ErrorKind::NotFound => None,
        Err(e) => return Err(failed("cannot inspect", target, e)),
    };
    match link(source, target) {
        Ok(()) => {}
        Err(e) if e.kind() == ErrorKind::PermissionDenied => elevate(source, target)?,
        Err(e) => return Err(failed("cannot create", target, e)),
    }
    // 권한 상승 경로는 다른 프로세스가 했으므로 결과를 디스크에서 확인한다.
    match fs::read_link(target) {
        Ok(now) if now == source => {}
        Ok(now) => {
            return Err(InstallError::Failed(format!(
                "{} points to {} after installing",
                target.display(),
                now.display()
            )));
        }
        Err(e) => return Err(failed("cannot read", target, e)),
    }
    Ok(previous.map_or(Installed::New, |previous| Installed::Replaced { previous }))
}

/// 같은 폴더의 임시 이름에 링크를 만들고 `rename` 으로 바꿔 끼운다 — 바꾸는 동안 `pitwall` 이 없는 순간이 없다.
fn link(source: &Path, target: &Path) -> std::io::Result<()> {
    let parent = target
        .parent()
        .ok_or_else(|| std::io::Error::new(ErrorKind::InvalidInput, "target has no parent"))?;
    fs::create_dir_all(parent)?;
    let temp = parent.join(format!(".pitwall-link-{}", std::process::id()));
    let _ = fs::remove_file(&temp);
    symlink(source, &temp)?;
    fs::rename(&temp, target).inspect_err(|_| {
        let _ = fs::remove_file(&temp);
    })
}

#[cfg(unix)]
fn symlink(source: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(source, link)
}

#[cfg(not(unix))]
fn symlink(_source: &Path, _link: &Path) -> std::io::Result<()> {
    Err(std::io::Error::new(
        ErrorKind::Unsupported,
        "the shell command is installed only on macOS",
    ))
}

/// 관리자 권한으로 `mkdir -p <폴더> && ln -sfn <source> <target>`. 경로는 스크립트 글자에 넣지 않고 `argv` 로 넘겨
/// `quoted form of` 로 셸 인용한다 — 경로의 따옴표 · 공백이 스크립트나 셸 명령을 깨지 않는다.
#[cfg(target_os = "macos")]
pub const ELEVATED_SCRIPT: [&str; 3] = [
    "on run argv",
    "do shell script \"/bin/mkdir -p \" & quoted form of (item 3 of argv) & \" && /bin/ln -sfn \" & quoted form of (item 1 of argv) & \" \" & quoted form of (item 2 of argv) with prompt \"Pitwall wants to install the 'pitwall' shell command.\" with administrator privileges",
    "end run",
];

#[cfg(target_os = "macos")]
fn elevate_with_osascript(source: &Path, target: &Path) -> Result<(), InstallError> {
    let parent = target.parent().unwrap_or(Path::new("/"));
    let mut command = std::process::Command::new("/usr/bin/osascript");
    for line in ELEVATED_SCRIPT {
        command.arg("-e").arg(line);
    }
    let output = command
        .arg(source)
        .arg(target)
        .arg(parent)
        .output()
        .map_err(|e| InstallError::Failed(format!("cannot run osascript: {e}")))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    // 암호 창의 취소는 AppleScript 오류 -128(「User canceled.」).
    if stderr.contains("(-128)") {
        Err(InstallError::Cancelled)
    } else {
        Err(InstallError::Failed(stderr.trim().to_owned()))
    }
}

#[cfg(not(target_os = "macos"))]
fn elevate_with_osascript(_source: &Path, _target: &Path) -> Result<(), InstallError> {
    Err(InstallError::Failed(
        "administrator privileges are needed".into(),
    ))
}

fn target_path() -> Result<PathBuf, InstallError> {
    let Some(value) = std::env::var_os(TARGET_ENV) else {
        return Ok(PathBuf::from(DEFAULT_TARGET));
    };
    let path = PathBuf::from(value);
    if path.is_absolute() {
        Ok(path)
    } else {
        Err(InstallError::Failed(format!(
            "{TARGET_ENV} must be an absolute path: {}",
            path.display()
        )))
    }
}

/// 실행 중인 바이너리(심링크를 푼 실제 경로)를 대상에 설치한다.
fn install_current_exe() -> Result<(PathBuf, Installed), InstallError> {
    let target = target_path()?;
    let source = std::env::current_exe()
        .and_then(fs::canonicalize)
        .map_err(|e| InstallError::Failed(format!("cannot locate the app binary: {e}")))?;
    let installed = install(&source, &target, elevate_with_osascript)?;
    tracing::info!(source = %source.display(), target = %target.display(), ?installed, "shell command installed");
    Ok((target, installed))
}

/// 설치하고 결과를 네이티브 대화상자로 알린다. 관리자 암호 창을 기다릴 수 있으므로 블로킹 스레드에서 부른다.
fn install_and_report<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    use tauri_plugin_dialog::{DialogExt, MessageDialogKind};

    let (text, kind, result) = match install_current_exe() {
        Ok((target, installed)) => {
            let target = target.display();
            let text = match installed {
                Installed::New => format!(
                    "Installed the 'pitwall' command at {target}.\n\nRun `pitwall <path>` in a terminal to open it in Pitwall."
                ),
                Installed::Already => {
                    format!("The 'pitwall' command at {target} already opens this app.")
                }
                Installed::Replaced { previous } => format!(
                    "Installed the 'pitwall' command at {target}.\n\nIt replaced a link to {}.",
                    previous.display()
                ),
            };
            (text, MessageDialogKind::Info, Ok(()))
        }
        Err(InstallError::Cancelled) => {
            tracing::info!("shell command install cancelled");
            return Err(InstallError::Cancelled.to_string());
        }
        Err(e) => {
            tracing::error!(error = %e, "shell command install failed");
            (
                format!("Could not install the 'pitwall' command.\n\n{e}"),
                MessageDialogKind::Error,
                Err(e.to_string()),
            )
        }
    };
    app.dialog()
        .message(text)
        .title("Install Shell Command")
        .kind(kind)
        .show(|_| {});
    result
}

/// 메뉴 경로(포커스된 창이 없을 때). 메인 스레드를 막지 않게 블로킹 스레드로 보낸다.
pub fn install_in_background<R: Runtime>(app: &AppHandle<R>) {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _ = install_and_report(&app);
    });
}

/// 웹뷰의 `cli.install` 액션(src/keymap/actions.ts).
#[tauri::command]
pub async fn install_shell_command<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || install_and_report(&app))
        .await
        .map_err(|e| e.to_string())?
}

#[cfg(all(test, unix))]
mod tests {
    use std::cell::Cell;
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    struct Dirs {
        _tmp: tempfile::TempDir,
        app: PathBuf,
        bin: PathBuf,
    }

    fn dirs() -> Dirs {
        let tmp = tempfile::tempdir().unwrap();
        let base = fs::canonicalize(tmp.path()).unwrap();
        let app = base.join("Pitwall.app/Contents/MacOS/pitwall");
        fs::create_dir_all(app.parent().unwrap()).unwrap();
        fs::write(&app, "").unwrap();
        Dirs {
            _tmp: tmp,
            app,
            bin: base.join("usr/local/bin"),
        }
    }

    fn no_elevation(_: &Path, _: &Path) -> Result<(), InstallError> {
        panic!("권한 상승이 필요 없어야 한다")
    }

    #[test]
    fn creates_the_link_and_its_folder() {
        let d = dirs();
        let target = d.bin.join("pitwall");
        assert_eq!(install(&d.app, &target, no_elevation), Ok(Installed::New));
        assert_eq!(fs::read_link(&target).unwrap(), d.app);
        assert_eq!(
            fs::read_dir(&d.bin).unwrap().count(),
            1,
            "임시 링크가 남지 않는다"
        );
    }

    #[test]
    fn a_link_to_this_app_is_left_alone() {
        let d = dirs();
        let target = d.bin.join("pitwall");
        install(&d.app, &target, no_elevation).unwrap();
        assert_eq!(
            install(&d.app, &target, no_elevation),
            Ok(Installed::Already)
        );
    }

    #[test]
    fn a_link_elsewhere_is_replaced_and_reported() {
        let d = dirs();
        let target = d.bin.join("pitwall");
        fs::create_dir_all(&d.bin).unwrap();
        std::os::unix::fs::symlink("/old/Pitwall.app/Contents/MacOS/pitwall", &target).unwrap();
        assert_eq!(
            install(&d.app, &target, no_elevation),
            Ok(Installed::Replaced {
                previous: "/old/Pitwall.app/Contents/MacOS/pitwall".into()
            })
        );
        assert_eq!(fs::read_link(&target).unwrap(), d.app);
    }

    #[test]
    fn a_file_or_folder_that_is_not_a_link_is_not_touched() {
        let d = dirs();
        fs::create_dir_all(&d.bin).unwrap();
        let file = d.bin.join("pitwall");
        fs::write(&file, "#!/bin/sh\n").unwrap();
        assert_eq!(
            install(&d.app, &file, no_elevation),
            Err(InstallError::NotALink(file.clone()))
        );
        assert_eq!(fs::read_to_string(&file).unwrap(), "#!/bin/sh\n");
        let folder = d.bin.join("pitwall-dir");
        fs::create_dir(&folder).unwrap();
        assert_eq!(
            install(&d.app, &folder, no_elevation),
            Err(InstallError::NotALink(folder.clone()))
        );
    }

    /// 쓸 수 없는 폴더(root 소유 `/usr/local/bin` 대신 읽기 전용 폴더). root 로 돌면 권한 검사가 없어 이 시험은 뜻이 없다.
    #[test]
    fn permission_denied_falls_back_to_elevation() {
        let d = dirs();
        fs::create_dir_all(&d.bin).unwrap();
        let readonly = || fs::set_permissions(&d.bin, fs::Permissions::from_mode(0o555)).unwrap();
        let writable = || fs::set_permissions(&d.bin, fs::Permissions::from_mode(0o755)).unwrap();
        let target = d.bin.join("pitwall");

        readonly();
        assert_eq!(
            install(&d.app, &target, |_, _| Err(InstallError::Cancelled)),
            Err(InstallError::Cancelled)
        );
        assert!(
            fs::symlink_metadata(&target).is_err(),
            "취소하면 아무것도 만들지 않는다"
        );

        let called = Cell::new(false);
        let result = install(&d.app, &target, |source, target| {
            called.set(true);
            // 관리자 권한으로 한 것을 흉내 낸다.
            writable();
            std::os::unix::fs::symlink(source, target).unwrap();
            Ok(())
        });
        assert!(called.get());
        assert_eq!(result, Ok(Installed::New));
        assert_eq!(fs::read_link(&target).unwrap(), d.app);
    }

    #[test]
    fn elevation_that_did_not_link_is_an_error() {
        let d = dirs();
        fs::create_dir_all(&d.bin).unwrap();
        fs::set_permissions(&d.bin, fs::Permissions::from_mode(0o555)).unwrap();
        let result = install(&d.app, &d.bin.join("pitwall"), |_, _| Ok(()));
        fs::set_permissions(&d.bin, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(matches!(result, Err(InstallError::Failed(_))), "{result:?}");
    }

    /// 권한 상승 스크립트가 AppleScript 로 컴파일되는지(실행하면 관리자 암호 창이 뜨므로 컴파일만 한다).
    #[cfg(target_os = "macos")]
    #[test]
    fn the_elevated_script_compiles() {
        let tmp = tempfile::tempdir().unwrap();
        let mut command = std::process::Command::new("/usr/bin/osacompile");
        command.arg("-o").arg(tmp.path().join("install.scpt"));
        for line in ELEVATED_SCRIPT {
            command.arg("-e").arg(line);
        }
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
