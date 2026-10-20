//! 「Orca 에서 Pitwall 열기」 안내(`docs/design/04-features.md` F8-6)의 검사: Orca 가 부를 `pitwall` 명령이 이 앱을 띄우는가.
//!
//! - 설치 위치: 데스크톱의 「Shell 명령 설치」(`cli.install`)가 만드는 심링크(macOS `/usr/local/bin/pitwall`)가 이 앱 바이너리를
//!   가리키는가. 다른 곳을 가리키면 그 대상을 알린다.
//! - PATH: 로그인 셸의 PATH 로 찾은 첫 `pitwall` 이 이 앱인가. Orca 는 시작할 때 로그인 셸 PATH 를 가져와 명령을 찾는다(04
//!   「Orca "Open in" 동작」 표의 PATH 행). 디렉터리의 제공자(mise 등)는 덮지 않는다 — Orca 는 그것을 모른다.
//!
//! 「이 앱인가」 는 심링크를 끝까지 푼 실제 경로로 비교한다. 결과는 남기지 않는다 — 설치한 뒤 다시 부르면 새로 본다.

use std::ffi::OsStr;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use pw_rpc::{Router, RpcError, rpc_method};
use pw_runtime::Runtime;
use pw_runtime::methods::{RuntimeSource, RuntimeStatus};
use serde::{Deserialize, Serialize};

use crate::launch::find_program;

/// Orca 에 등록할 명령이자 PATH 에서 찾는 이름.
pub const COMMAND: &str = "pitwall";

/// 설치 위치의 상태.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum CliInstall {
    /// 아무것도 없다.
    Missing,
    /// 이 앱을 가리키는 심링크.
    ThisApp,
    /// 다른 곳을 가리키는 심링크(끊긴 링크 포함). `target` 은 링크가 가리키는 경로 — 상대 경로면 링크의 폴더에 붙인 것.
    OtherLink { target: String },
    /// 심링크가 아닌 파일 · 폴더. 「Shell 명령 설치」 는 이것을 건드리지 않는다.
    NotALink,
}

/// 로그인 셸 PATH 로 찾은 `pitwall`.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum CliOnPath {
    NotFound,
    /// 찾은 첫 실행 파일이 이 앱이다.
    ThisApp {
        path: String,
    },
    /// 찾은 첫 실행 파일이 다른 것이다. `resolved` 는 심링크를 푼 실제 경로(풀지 못했으면 없다).
    Other {
        path: String,
        resolved: Option<String>,
    },
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct CliStatus {
    /// 설치 위치(절대 경로). PATH 에 없을 때 Orca 에 이 경로를 그대로 등록한다.
    pub target: String,
    /// 이 앱 바이너리의 실제 경로.
    pub app: String,
    pub install: CliInstall,
    pub on_path: CliOnPath,
    /// 찾아본 PATH 의 출처. 로그인 셸을 읽지 못했으면 앱 프로세스의 것이다(`path_status` 가 그 이유).
    pub path_source: RuntimeSource,
    pub path_status: RuntimeStatus,
}

rpc_method! {
    /// 「Orca 에서 Pitwall 열기」 안내의 검사(04 F8-6): 설치 위치의 심링크가 이 앱을 가리키는가, 로그인 셸 PATH 로 찾은
    /// `pitwall` 이 이 앱인가. 처음이면 로그인 셸을 기다린다(최대 5초 남짓).
    pub CliStatusMethod: "cli.status" {
        params: (),
        result: CliStatus,
        tier: Read,
        roles: [Owner],
    }
}

/// 검사에 쓰는 것. 설치 위치 · 앱 경로를 구하지 못했으면 그 이유를 들고 있다가 부를 때 오류로 준다.
pub struct CliCheck {
    runtime: Arc<Runtime>,
    target: Result<PathBuf, String>,
    app: Result<PathBuf, String>,
}

impl CliCheck {
    /// `target`: 「Shell 명령 설치」 의 대상(절대 경로). `app`: 이 앱 바이너리.
    pub fn new(
        runtime: Arc<Runtime>,
        target: Result<PathBuf, String>,
        app: Result<PathBuf, String>,
    ) -> Self {
        Self {
            runtime,
            target,
            app,
        }
    }

    pub fn status(&self) -> Result<CliStatus, RpcError> {
        let target = self.target.as_ref().map_err(RpcError::internal)?;
        let app = self.app.as_ref().map_err(RpcError::internal)?;
        let app = fs::canonicalize(app).map_err(|e| {
            RpcError::internal(format!(
                "cannot locate the app binary {}: {e}",
                app.display()
            ))
        })?;
        let (layer, failure) = self.runtime.login_shell_env();
        Ok(CliStatus {
            target: target.display().to_string(),
            app: app.display().to_string(),
            install: install_state(target, &app),
            on_path: on_path(&layer.env.path, &app),
            path_source: layer.source,
            path_status: failure.map_or(RuntimeStatus::Ok, |reason| RuntimeStatus::Fallback {
                reason,
            }),
        })
    }
}

/// `target` 의 상태. `app` 은 심링크를 푼 실제 경로.
pub fn install_state(target: &Path, app: &Path) -> CliInstall {
    match fs::symlink_metadata(target) {
        Ok(meta) if meta.file_type().is_symlink() => {
            if is_app(target, app) {
                return CliInstall::ThisApp;
            }
            let points_to = fs::read_link(target).map_or_else(
                |_| String::new(),
                |link| match target.parent() {
                    Some(dir) if link.is_relative() => dir.join(link).display().to_string(),
                    _ => link.display().to_string(),
                },
            );
            CliInstall::OtherLink { target: points_to }
        }
        Ok(_) => CliInstall::NotALink,
        Err(e) if e.kind() == ErrorKind::NotFound => CliInstall::Missing,
        // 폴더를 읽을 권한이 없는 등 — 없다고 보고 설치를 권한다(설치가 실패 이유를 알린다).
        Err(e) => {
            tracing::warn!(target = %target.display(), %e, "cannot inspect the shell command");
            CliInstall::Missing
        }
    }
}

/// PATH(`path`, PATH 형식)로 찾은 첫 [`COMMAND`].
pub fn on_path(path: &OsStr, app: &Path) -> CliOnPath {
    let Some(found) = find_program(COMMAND, path) else {
        return CliOnPath::NotFound;
    };
    let shown = found.display().to_string();
    match fs::canonicalize(&found) {
        Ok(real) if real == app => CliOnPath::ThisApp { path: shown },
        Ok(real) => CliOnPath::Other {
            resolved: (real != found).then(|| real.display().to_string()),
            path: shown,
        },
        Err(_) => CliOnPath::Other {
            path: shown,
            resolved: None,
        },
    }
}

fn is_app(path: &Path, app: &Path) -> bool {
    fs::canonicalize(path).is_ok_and(|real| real == app)
}

/// 핸들러를 붙인다. 로그인 셸 기다리기와 디스크 읽기는 블로킹 스레드에서 한다.
pub fn register(router: &mut Router, check: Arc<CliCheck>) {
    router.unary(CliStatusMethod, move |_ctx, ()| {
        let check = Arc::clone(&check);
        async move {
            tokio::task::spawn_blocking(move || check.status())
                .await
                .map_err(|e| RpcError::internal(e.to_string()))?
        }
    });
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::time::Duration;

    use pw_runtime::LoginShell;

    use super::*;

    /// 가짜 로그인 셸의 PATH 는 `install`(설치 위치의 폴더) · `other` 다. 앱 바이너리는 PATH 밖의 `app/pitwall`.
    struct Fixture {
        tmp: tempfile::TempDir,
        app: PathBuf,
        target: PathBuf,
        other: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let tmp = tempfile::tempdir().unwrap();
            // macOS 의 임시 폴더(`/var/…`)는 `/private/var/…` 의 심링크다 — 결과의 경로와 비교하도록 푼 경로로 둔다.
            let root = fs::canonicalize(tmp.path()).unwrap();
            let app = executable(&root.join("app"), "pitwall");
            let install = root.join("install");
            let other = root.join("other");
            fs::create_dir_all(&install).unwrap();
            fs::create_dir_all(&other).unwrap();
            Self {
                target: install.join("pitwall"),
                app,
                other,
                tmp,
            }
        }

        /// PATH 를 `dirs` 로 정하는 가짜 로그인 셸. 빈 `dirs` 면 셸이 실패한다.
        fn check(&self, dirs: &[&Path]) -> CliCheck {
            let shell = self.tmp.path().join("fake-shell");
            let body = if dirs.is_empty() {
                "exit 7".to_owned()
            } else {
                let path: Vec<String> = dirs.iter().map(|d| d.display().to_string()).collect();
                format!("PATH='{}'; export PATH\neval \"$4\"", path.join(":"))
            };
            fs::write(&shell, format!("#!/bin/sh\n{body}\n")).unwrap();
            fs::set_permissions(&shell, fs::Permissions::from_mode(0o755)).unwrap();
            let runtime = Runtime::new(LoginShell::new(Some(shell), Duration::from_secs(5)));
            CliCheck::new(
                Arc::new(runtime),
                Ok(self.target.clone()),
                Ok(self.app.clone()),
            )
        }

        fn install_dir(&self) -> PathBuf {
            self.target.parent().unwrap().to_path_buf()
        }
    }

    fn executable(dir: &Path, name: &str) -> PathBuf {
        fs::create_dir_all(dir).unwrap();
        let path = dir.join(name);
        fs::write(&path, "#!/bin/sh\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        fs::canonicalize(path).unwrap()
    }

    /// 설치 전: 설치 위치에 아무것도 없고 PATH 에서도 못 찾는다. 같은 검사를 설치 뒤 다시 부르면(로그인 셸 환경은 남아 있다)
    /// 「설치됨 · PATH 에 있음」 이다.
    #[test]
    fn missing_before_install_and_found_on_the_path_after() {
        let f = Fixture::new();
        let check = f.check(&[&f.install_dir()]);
        let before = check.status().unwrap();
        assert_eq!(before.install, CliInstall::Missing);
        assert_eq!(before.on_path, CliOnPath::NotFound);
        assert_eq!(before.path_status, RuntimeStatus::Ok);
        assert!(matches!(
            before.path_source,
            RuntimeSource::LoginShell { .. }
        ));
        assert_eq!(before.target, f.target.display().to_string());
        assert_eq!(before.app, f.app.display().to_string());

        symlink(&f.app, &f.target).unwrap();
        let after = check.status().unwrap();
        assert_eq!(after.install, CliInstall::ThisApp);
        assert_eq!(
            after.on_path,
            CliOnPath::ThisApp {
                path: f.target.display().to_string()
            }
        );
    }

    /// 다른 곳을 가리키는 심링크면 그 대상 경로를 준다. 상대 링크는 링크의 폴더에 붙이고, 끊긴 링크도 다른 곳이다.
    #[test]
    fn a_link_to_somewhere_else_names_its_target() {
        let f = Fixture::new();
        let elsewhere = executable(&f.tmp.path().join("old build"), "pitwall");
        symlink(&elsewhere, &f.target).unwrap();
        let check = f.check(&[&f.install_dir()]);
        let status = check.status().unwrap();
        assert_eq!(
            status.install,
            CliInstall::OtherLink {
                target: elsewhere.display().to_string()
            }
        );
        assert_eq!(
            status.on_path,
            CliOnPath::Other {
                path: f.target.display().to_string(),
                resolved: Some(elsewhere.display().to_string()),
            }
        );

        fs::remove_file(&f.target).unwrap();
        symlink("../gone/pitwall", &f.target).unwrap();
        assert_eq!(
            check.status().unwrap().install,
            CliInstall::OtherLink {
                target: f
                    .install_dir()
                    .join("../gone/pitwall")
                    .display()
                    .to_string()
            }
        );
    }

    /// 상대 경로 · 여러 단계의 심링크라도 끝이 이 앱이면 이 앱이다.
    #[test]
    fn a_relative_link_chain_to_the_app_is_this_app() {
        let f = Fixture::new();
        let hop = f.tmp.path().join("hop");
        symlink(&f.app, &hop).unwrap();
        symlink("../hop", &f.target).unwrap();
        let status = f.check(&[&f.install_dir()]).status().unwrap();
        assert_eq!(status.install, CliInstall::ThisApp);
        assert!(matches!(status.on_path, CliOnPath::ThisApp { .. }));
    }

    /// 심링크가 아닌 파일은 따로 알린다(「Shell 명령 설치」 가 건드리지 않는다).
    #[test]
    fn a_plain_file_is_not_a_link() {
        let f = Fixture::new();
        fs::write(&f.target, "#!/bin/sh\n").unwrap();
        let status = f.check(&[&f.install_dir()]).status().unwrap();
        assert_eq!(status.install, CliInstall::NotALink);
    }

    /// PATH 의 앞 폴더에 다른 `pitwall` 이 있으면 Orca 는 그것을 띄운다 — 설치 위치가 이 앱이어도 「다른 것」 이다.
    #[test]
    fn an_earlier_pitwall_on_the_path_shadows_the_installed_one() {
        let f = Fixture::new();
        symlink(&f.app, &f.target).unwrap();
        let shadow = executable(&f.other, "pitwall");
        let status = f.check(&[&f.other, &f.install_dir()]).status().unwrap();
        assert_eq!(status.install, CliInstall::ThisApp);
        assert_eq!(
            status.on_path,
            CliOnPath::Other {
                path: shadow.display().to_string(),
                resolved: None,
            }
        );
    }

    /// 설치 위치가 PATH 밖이면 이 앱을 가리켜도 PATH 에서는 못 찾는다(절대 경로 등록 안내 자리).
    #[test]
    fn installed_outside_the_path_is_not_found() {
        let f = Fixture::new();
        symlink(&f.app, &f.target).unwrap();
        let status = f.check(&[&f.other]).status().unwrap();
        assert_eq!(status.install, CliInstall::ThisApp);
        assert_eq!(status.on_path, CliOnPath::NotFound);
    }

    /// 로그인 셸을 읽지 못하면 앱 프로세스의 PATH 로 찾고, 그 이유를 준다.
    #[test]
    fn a_failing_login_shell_falls_back_to_the_process_path() {
        let f = Fixture::new();
        let status = f.check(&[]).status().unwrap();
        assert_eq!(status.path_source, RuntimeSource::Process);
        assert!(
            matches!(&status.path_status, RuntimeStatus::Fallback { reason } if !reason.is_empty()),
            "{:?}",
            status.path_status
        );
    }

    /// 설치 위치를 구하지 못했으면(`PITWALL_SHELL_COMMAND_TARGET` 이 상대 경로 등) 오류다.
    #[test]
    fn an_unknown_target_is_an_error() {
        let f = Fixture::new();
        let check = CliCheck::new(
            Arc::new(Runtime::new(LoginShell::new(None, Duration::from_secs(1)))),
            Err("PITWALL_SHELL_COMMAND_TARGET must be an absolute path: x".to_owned()),
            Ok(f.app.clone()),
        );
        let e = check.status().unwrap_err();
        assert_eq!(e.code, pw_rpc::code::INTERNAL_ERROR);
        assert!(
            format!("{e:?}").contains("must be an absolute path"),
            "{e:?}"
        );
    }
}
