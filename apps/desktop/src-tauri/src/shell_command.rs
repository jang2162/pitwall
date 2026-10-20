//! 「Shell 명령 설치」(docs/design/04-features.md F2-1): 실행 중인 앱 바이너리를 가리키는 심링크를 PATH 의 폴더에 만든다.
//!
//! - 대상: macOS `/usr/local/bin/pitwall`. `PITWALL_SHELL_COMMAND_TARGET`(절대 경로)로 바꿀 수 있다 — 개발 · 시험에서 사람의
//!   시스템 경로를 건드리지 않고 확인하는 자리다(CONTRIBUTING.md 「명령」).
//! - 이미 있을 때: 이 앱을 가리키는 심링크면 그대로 둔다. **다른 곳을 가리키는 심링크면 바꾼다** — 사람이 이 앱에서 「설치」를
//!   골랐으므로 `pitwall` 이 이 앱을 띄우게 하는 것이 요청이다(옮긴 앱 · 다른 빌드의 옛 링크). 무엇을 바꿨는지 알린다.
//!   **심링크가 아닌 파일 · 폴더는 건드리지 않는다** — Pitwall 이 만든 것이 아니다.
//! - 권한이 없을 때(`/usr/local/bin` 은 대개 root 소유이고, Apple Silicon 의 새 macOS 에는 폴더가 없기도 하다): `osascript`
//!   의 `with administrator privileges` 로 macOS 관리자 암호 창을 띄워 같은 일을 root 셸에서 한다(VS Code 의 「Install
//!   'code' command」 와 같은 방법, [`ELEVATED_SH`]). 암호 창이 떠 있는 동안 대상이 바뀔 수 있으므로 **비링크 검사를 그 셸
//!   안에서 다시 한다.** 사람이 취소하면 아무것도 바꾸지 않고 끝낸다.
//! - 바꿔 끼우기: 두 경로 모두 같은 폴더의 임시 링크를 `rename`(권한 상승 경로는 `mv -fh`)으로 옮긴다 — 바꾸는 동안
//!   `pitwall` 이 없는 순간이 없다. 비링크 검사와 바꿔 끼우기 사이는 원자가 아니다(그 틈에 생긴 파일은 덮인다). 틈은 검사 직후
//!   한 번의 `symlink` 호출 길이다.
//! - 출처: 실행 중인 바이너리가 읽기 전용 볼륨(`/Volumes/…`, 배포 DMG)이나 App Translocation 아래면 거절한다 — 꺼내기 ·
//!   재시작 뒤 링크가 끊긴다. Applications 로 옮긴 뒤 다시 하라고 알린다. 쓰기 가능한 디스크 이미지는 놓친다([`check_source`]).

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use tauri::{AppHandle, Manager, Runtime};

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
    /// 관리자 권한 요청을 사람이 취소했다. 권한 상승(암호 창)은 macOS 에만 있다.
    #[cfg(target_os = "macos")]
    Cancelled,
    /// 실행 중인 위치가 사라질 곳이다(디스크 이미지 · App Translocation).
    TransientSource(PathBuf),
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
            #[cfg(target_os = "macos")]
            InstallError::Cancelled => f.write_str("cancelled"),
            InstallError::TransientSource(path) => write!(
                f,
                "Pitwall is running from {}, which goes away when the disk image is ejected or the Mac restarts. \
                 Move Pitwall to the Applications folder, open it from there, and install the command again.",
                path.display()
            ),
            InstallError::Failed(reason) => f.write_str(reason),
        }
    }
}

fn failed(what: &str, path: &Path, e: std::io::Error) -> InstallError {
    InstallError::Failed(format!("{what} {}: {e}", path.display()))
}

/// 링크가 가리켜도 되는 출처인가. 거절하는 것:
/// - App Translocation(격리 속성이 붙은 채 다운로드 폴더에서 연 앱을 macOS 가 읽기 전용 임시 위치에서 돌린다) — 재시작 뒤 사라진다.
/// - `/Volumes/…` 의 **읽기 전용** 볼륨 — 배포 DMG 처럼 꺼내면 사라진다. 외장 디스크에 영구 설치한 앱(쓰기 가능)은 받는다.
///   쓰기 가능한 디스크 이미지(UDRW 등)에서 돌리면 놓친다 — 드물고, 그때는 꺼낸 뒤 링크가 끊긴다.
///
/// `read_only` 는 경로가 있는 볼륨이 읽기 전용인지([`read_only_volume`], 시험은 가짜를 넣는다).
pub fn check_source(source: &Path, read_only: impl Fn(&Path) -> bool) -> Result<(), InstallError> {
    let translocated = source
        .components()
        .any(|c| c.as_os_str() == "AppTranslocation");
    let disk_image = source.starts_with("/Volumes") && read_only(source);
    if translocated || disk_image {
        Err(InstallError::TransientSource(source.to_path_buf()))
    } else {
        Ok(())
    }
}

/// `path` 가 있는 볼륨이 읽기 전용으로 마운트됐는가(`statfs` 의 `MNT_RDONLY`). 알 수 없으면 거짓.
#[cfg(target_os = "macos")]
pub fn read_only_volume(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;

    let Ok(c_path) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    let mut buf = std::mem::MaybeUninit::<libc::statfs>::uninit();
    // SAFETY: `c_path` 는 NUL 로 끝나는 경로이고 `buf` 는 statfs 하나를 담을 쓰기 가능한 공간이다. 성공(0)일 때만 읽는다.
    if unsafe { libc::statfs(c_path.as_ptr(), buf.as_mut_ptr()) } != 0 {
        return false;
    }
    let flags = unsafe { buf.assume_init() }.f_flags;
    flags & libc::MNT_RDONLY as u32 != 0
}

#[cfg(not(target_os = "macos"))]
pub fn read_only_volume(_path: &Path) -> bool {
    false
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

/// 관리자 권한으로 도는 셸 스크립트. `$1` 출처, `$2` 대상, `$3` 대상의 폴더. 경로는 스크립트 글자에 넣지 않고 위치 인자로
/// 넘긴다(AppleScript 의 `quoted form of` 로 인용) — 경로의 따옴표 · 공백 · `$(…)` 가 명령이 되지 않는다.
/// 비링크면 3 으로 끝난다(암호 창이 떠 있는 동안 다른 설치기가 놓은 파일을 root 가 지우지 않게).
#[cfg(target_os = "macos")]
pub const ELEVATED_SH: &str = r#"set -e
/bin/mkdir -p "$3"
if [ -e "$2" ] && [ ! -L "$2" ]; then exit 3; fi
tmp="$3/.pitwall-link-$$"
/bin/rm -f "$tmp"
/bin/ln -s "$1" "$tmp"
/bin/mv -fh "$tmp" "$2" || { /bin/rm -f "$tmp"; exit 1; }"#;

/// [`ELEVATED_SH`] 가 「비링크」 로 끝낸 종료 코드.
#[cfg(target_os = "macos")]
const NOT_A_LINK_STATUS: &str = "(3)";

/// [`ELEVATED_SH`] 를 `do shell script` 로 돌리는 AppleScript. `admin` 이 거짓이면 권한 상승 없이 돈다(시험용).
/// argv: 출처, 대상, 폴더, 셸 스크립트.
#[cfg(target_os = "macos")]
fn osascript_lines(admin: bool) -> [String; 3] {
    let privileges = if admin {
        " with prompt \"Pitwall wants to install the 'pitwall' shell command.\" with administrator privileges"
    } else {
        ""
    };
    [
        "on run argv".to_owned(),
        format!(
            "do shell script \"/bin/sh -c \" & quoted form of (item 4 of argv) & \" pitwall \" & quoted form of (item 1 of argv) & \" \" & quoted form of (item 2 of argv) & \" \" & quoted form of (item 3 of argv){privileges}"
        ),
        "end run".to_owned(),
    ]
}

#[cfg(target_os = "macos")]
fn run_osascript(source: &Path, target: &Path, admin: bool) -> Result<(), InstallError> {
    let parent = target.parent().unwrap_or(Path::new("/"));
    let mut command = std::process::Command::new("/usr/bin/osascript");
    for line in osascript_lines(admin) {
        command.arg("-e").arg(line);
    }
    let output = command
        .arg(source)
        .arg(target)
        .arg(parent)
        .arg(ELEVATED_SH)
        .output()
        .map_err(|e| InstallError::Failed(format!("cannot run osascript: {e}")))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    // 암호 창의 취소는 AppleScript 오류 -128(「User canceled.」). 셸의 종료 코드 N 은 「… (N)」 으로 끝난다.
    if stderr.contains("(-128)") {
        Err(InstallError::Cancelled)
    } else if stderr.trim_end().ends_with(NOT_A_LINK_STATUS) {
        Err(InstallError::NotALink(target.to_path_buf()))
    } else {
        Err(InstallError::Failed(stderr.trim().to_owned()))
    }
}

#[cfg(target_os = "macos")]
fn elevate_with_osascript(source: &Path, target: &Path) -> Result<(), InstallError> {
    run_osascript(source, target, true)
}

#[cfg(not(target_os = "macos"))]
fn elevate_with_osascript(_source: &Path, _target: &Path) -> Result<(), InstallError> {
    Err(InstallError::Failed(
        "administrator privileges are needed".into(),
    ))
}

/// 「Shell 명령 설치」 의 대상. 「Orca 에서 Pitwall 열기」 안내의 검사(`cli.status`)도 이 경로를 본다.
pub fn target_path() -> Result<PathBuf, InstallError> {
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
    check_source(&source, read_only_volume)?;
    let installed = install(&source, &target, elevate_with_osascript)?;
    tracing::info!(source = %source.display(), target = %target.display(), ?installed, "shell command installed");
    Ok((target, installed))
}

/// 설치하고 결과를 네이티브 대화상자로 알린다. 관리자 암호 창을 기다릴 수 있으므로 블로킹 스레드에서 부른다.
fn install_and_report<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    use pw_i18n::FluentArgs;
    use tauri_plugin_dialog::{DialogExt, MessageDialogKind};

    // 대화상자 문장은 UI 언어로 만든다(extensions/i18n-default/i18n/*/menu.ftl). 언어 상태가 없으면(시험) 메시지 id 가 보인다.
    let l = app
        .try_state::<std::sync::Arc<pw_i18n::I18n>>()
        .map(|i18n| i18n.localizer());
    let say = |id: &str, args: &[(&str, String)]| {
        let mut fluent = FluentArgs::new();
        for (name, value) in args {
            fluent.set(*name, value.clone());
        }
        l.as_ref()
            .map_or_else(|| id.to_owned(), |l| l.format(id, Some(&fluent)))
    };
    let (text, kind, result) = match install_current_exe() {
        Ok((target, installed)) => {
            let target = ("target", target.display().to_string());
            let text = match installed {
                Installed::New => say("shell-command-installed", &[target]),
                Installed::Already => say("shell-command-already", &[target]),
                Installed::Replaced { previous } => say(
                    "shell-command-replaced",
                    &[target, ("previous", previous.display().to_string())],
                ),
            };
            (text, MessageDialogKind::Info, Ok(()))
        }
        #[cfg(target_os = "macos")]
        Err(InstallError::Cancelled) => {
            tracing::info!("shell command install cancelled");
            return Err(InstallError::Cancelled.to_string());
        }
        Err(e) => {
            tracing::error!(error = %e, "shell command install failed");
            (
                say("shell-command-failed", &[("error", e.to_string())]),
                MessageDialogKind::Error,
                Err(e.to_string()),
            )
        }
    };
    let mut dialog = app
        .dialog()
        .message(text)
        .title(say("shell-command-title", &[]))
        .kind(kind);
    if let Some(parent) = crate::windows::dialog_parent(app) {
        dialog = dialog.parent(&parent);
    }
    dialog.show(|_| {});
    result
}

/// 메뉴 경로(포커스된 창이 없을 때). 메인 스레드를 막지 않게 블로킹 스레드로 보낸다. 메뉴 항목은 macOS 앱 메뉴에만 있다.
#[cfg(target_os = "macos")]
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
        #[cfg(target_os = "macos")]
        {
            assert_eq!(
                install(&d.app, &target, |_, _| Err(InstallError::Cancelled)),
                Err(InstallError::Cancelled)
            );
            assert!(
                fs::symlink_metadata(&target).is_err(),
                "취소하면 아무것도 만들지 않는다"
            );
        }

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

    #[test]
    fn a_read_only_volume_or_translocated_app_is_refused() {
        let dmg = "/Volumes/Pitwall/Pitwall.app/Contents/MacOS/pitwall";
        assert_eq!(
            check_source(Path::new(dmg), |_| true),
            Err(InstallError::TransientSource(dmg.into()))
        );
        let translocated =
            "/private/var/folders/x/T/AppTranslocation/1234/d/Pitwall.app/Contents/MacOS/pitwall";
        assert_eq!(
            check_source(Path::new(translocated), |_| false),
            Err(InstallError::TransientSource(translocated.into())),
            "읽기 전용 여부와 상관없이 거절"
        );
    }

    #[test]
    fn a_writable_external_volume_and_ordinary_places_are_accepted() {
        assert_eq!(
            check_source(
                Path::new("/Volumes/External/Applications/Pitwall.app/Contents/MacOS/pitwall"),
                |_| false
            ),
            Ok(())
        );
        for kept in [
            "/Applications/Pitwall.app/Contents/MacOS/pitwall",
            "/Users/me/code/pitwall/target/debug/pitwall",
            "/VolumesX/pitwall",
        ] {
            assert_eq!(check_source(Path::new(kept), |_| true), Ok(()), "{kept}");
        }
    }

    /// 실제 statfs: macOS 의 시스템 볼륨(`/`)은 읽기 전용으로 마운트되고 임시 폴더는 쓸 수 있다.
    #[cfg(target_os = "macos")]
    #[test]
    fn statfs_reports_read_only_volumes() {
        assert!(read_only_volume(Path::new("/")));
        let tmp = tempfile::tempdir().unwrap();
        assert!(!read_only_volume(tmp.path()));
        assert!(!read_only_volume(Path::new("/no/such/path")));
    }

    /// 경로에 셸 · AppleScript 가 특별히 보는 글자를 넣는다. 명령으로 해석되면 `PWNED` 가 생긴다.
    #[cfg(target_os = "macos")]
    fn awkward(d: &Dirs) -> (PathBuf, PathBuf) {
        let dir = d.bin.join("it's \"a\" $(touch PWNED) `touch PWNED`");
        (dir.join("pitwall"), d.bin.join("PWNED"))
    }

    /// 실제 osascript 와 셸 스크립트를 권한 상승만 빼고 돌린다(암호 창이 뜨지 않는다).
    #[cfg(target_os = "macos")]
    #[test]
    fn the_elevated_script_links_and_replaces_without_running_paths() {
        let d = dirs();
        let (target, pwned) = awkward(&d);
        assert_eq!(run_osascript(&d.app, &target, false), Ok(()));
        assert_eq!(fs::read_link(&target).unwrap(), d.app);
        // 디렉터리를 가리키는 링크도 그 안으로 들어가지 않고 링크 자체를 바꾼다(`mv -h`).
        fs::remove_file(&target).unwrap();
        std::os::unix::fs::symlink(&d.bin, &target).unwrap();
        assert_eq!(run_osascript(&d.app, &target, false), Ok(()));
        assert_eq!(fs::read_link(&target).unwrap(), d.app);
        assert!(!pwned.exists(), "경로가 명령으로 실행됐다");
        let parent = target.parent().unwrap();
        assert_eq!(
            fs::read_dir(parent).unwrap().count(),
            1,
            "임시 링크가 남지 않는다"
        );
        assert_eq!(fs::read_dir(&d.bin).unwrap().count(), 1);
    }

    /// 암호 창이 떠 있는 동안 비링크가 생긴 경우: root 셸이 다시 검사해 건드리지 않는다.
    #[cfg(target_os = "macos")]
    #[test]
    fn the_elevated_script_refuses_a_file_that_appeared() {
        let d = dirs();
        let (target, _) = awkward(&d);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(&target, "#!/bin/sh\n").unwrap();
        assert_eq!(
            run_osascript(&d.app, &target, false),
            Err(InstallError::NotALink(target.clone()))
        );
        assert_eq!(fs::read_to_string(&target).unwrap(), "#!/bin/sh\n");
    }

    /// 권한 상승 판 AppleScript 가 컴파일되는지(실행하면 관리자 암호 창이 뜨므로 컴파일만 한다).
    #[cfg(target_os = "macos")]
    #[test]
    fn the_elevated_script_compiles() {
        let tmp = tempfile::tempdir().unwrap();
        let mut command = std::process::Command::new("/usr/bin/osacompile");
        command.arg("-o").arg(tmp.path().join("install.scpt"));
        for line in osascript_lines(true) {
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
