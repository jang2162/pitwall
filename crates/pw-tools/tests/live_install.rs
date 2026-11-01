//! 실제 네트워크로 기본 번들 서버를 이 설치기로 받는다(15-m4-plan.md M4-04 live — `#[ignore]`). CI `lsp-live` 의 받기를 이 설치기로
//! 바꾸는 것은 M4-18 이 한다(그때 이 시험 이름을 잡에 더한다). 로컬:
//!
//! ```text
//! cargo nextest run -p pw-tools --run-ignored only -E 'test(/^live_tools_install_/)'
//! ```
//!
//! rust-analyzer 는 고정 해시로 확인한 `.gz` 를 풀고 `--version` 이 돈다. typescript-language-server 는 이 프로세스의 PATH 의 npm 으로
//! 받되 — 그 환경에 출처를 바꾸려는 `NPM_CONFIG_REGISTRY` · 프로젝트의 `.npmrc` 가 있어도 — 고정 레지스트리에서 받고 `integrity` 를
//! 고정 값과 대조한다.

use std::path::Path;
use std::sync::Arc;

use pw_core::bus::{EventBus, Scope};
use pw_core::task::TaskSpawner;
use pw_lsp_servers::{Located, ToolLocator};
use pw_rpc::ConnectionScope;
use pw_runtime::RuntimeEnv;
use pw_tools::install::methods::{ToolConfirmRequired, ToolConfirmed};
use pw_tools::install::{InstallerHost, Toolchains, code, tool_grant};
use pw_tools::methods::ToolChecksum;
use pw_tools::{ToolsInstaller, ToolsRegistry};
use tokio_util::sync::CancellationToken;

/// 이 프로세스의 환경을 바탕으로, 「프로젝트」 환경에는 출처를 바꾸려는 변수를 더한다.
struct HostileProject;

impl Toolchains for HostileProject {
    fn login_shell(&self) -> RuntimeEnv {
        RuntimeEnv::process()
    }

    fn lookup(&self, _project: Option<&str>) -> Result<RuntimeEnv, String> {
        let mut env = RuntimeEnv::process();
        env.vars
            .insert("NPM_CONFIG_REGISTRY".into(), "https://127.0.0.1:9/".into());
        env.vars
            .insert("npm_config_registry".into(), "https://127.0.0.1:9/".into());
        Ok(env)
    }
}

async fn install(
    name: &str,
    root: &Path,
) -> (
    Arc<ToolsInstaller>,
    pw_tools::install::methods::ToolInstallResult,
) {
    let installer = Arc::new(ToolsInstaller::new(Arc::new(ToolsRegistry::new())));
    installer.attach(InstallerHost {
        tools_dir: root.join("data/tools"),
        state_dir: root.join("state"),
        tasks: TaskSpawner::new(EventBus::new(64), Scope::Host, CancellationToken::new()),
        toolchains: Arc::new(HostileProject),
        settings_window: Arc::new(|_: &str| false),
    });
    let grant = tool_grant(
        &ConnectionScope::Project("live".into()),
        None,
        &|_: &str| false,
    )
    .unwrap();
    let err = installer
        .install(&grant, name, None, None, false)
        .await
        .expect_err("asks first");
    assert_eq!(err.code, code::CONFIRM_REQUIRED);
    let ask: ToolConfirmRequired = serde_json::from_value(err.data.unwrap()).unwrap();
    assert_eq!(
        ask.checksum,
        ToolChecksum::Pinned,
        "a bundled package has pinned hashes"
    );
    let confirmed = ToolConfirmed {
        version: ask.version,
        licenses: ask.licenses,
        source_domains: ask.source_domains,
    };
    let result = installer
        .install(&grant, name, None, Some(&confirmed), false)
        .await
        .unwrap_or_else(|e| {
            panic!(
                "{e:?}\n{}",
                installer.read_log(name).ok().flatten().unwrap_or_default()
            )
        });
    (installer, result)
}

fn run(path: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new(path)
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    assert!(out.status.success(), "{}: {:?}", path.display(), out);
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "live: downloads rust-analyzer from GitHub"]
async fn live_tools_install_rust_analyzer() {
    let tmp = tempfile::tempdir().unwrap();
    let (installer, result) = install("rust-analyzer", tmp.path()).await;
    assert_eq!(result.checksum, ToolChecksum::Pinned);
    let Located::Installed { path, version } = installer
        .locator()
        .unwrap()
        .locate("rust-analyzer", "rust-analyzer")
    else {
        panic!("not located");
    };
    assert_eq!(version, result.version);
    let out = run(&path, &["--version"]);
    assert!(out.starts_with("rust-analyzer"), "{out}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "live: installs typescript-language-server with npm"]
async fn live_tools_install_typescript_language_server() {
    let tmp = tempfile::tempdir().unwrap();
    // 「프로젝트」 의 .npmrc — cwd 가 설치 폴더라 읽히지 않는다(읽히면 닿을 수 없는 레지스트리라 실패한다).
    std::fs::write(tmp.path().join(".npmrc"), "registry=https://127.0.0.1:9/\n").unwrap();
    let (installer, result) = install("typescript-language-server", tmp.path()).await;
    assert_eq!(result.checksum, ToolChecksum::Pinned);
    let Located::Installed { path, .. } = installer
        .locator()
        .unwrap()
        .locate("typescript-language-server", "typescript-language-server")
    else {
        panic!("not located");
    };
    let out = run(&path, &["--version"]);
    assert_eq!(out.trim(), result.version);
}
