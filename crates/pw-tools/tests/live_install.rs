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

/// 고정 판 전체(`tests/fixtures/registry.json.zip` — 고정 해시와 같은지 본다)를 정의로, 고정 표 · 덮어쓰기는 앱과 같다.
fn pinned_full_registry() -> ToolsRegistry {
    let zip = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/registry.json.zip"
    ))
    .unwrap();
    let pinned = pw_tools::Pinned::builtin();
    assert_eq!(pw_tools::fetch::sha256_hex(&zip), pinned.registry.sha256);
    let json = pw_tools::source::unzip_registry(&zip).unwrap();
    let index = pw_tools::Index::from_registry_json(
        &json,
        &pinned.registry.version,
        pw_tools::RegistryOrigin::Cache,
        None,
    )
    .unwrap();
    ToolsRegistry::with_parts(pinned, pw_tools::Overrides::builtin(), index)
}

async fn install(
    name: &str,
    root: &Path,
) -> (
    Arc<ToolsInstaller>,
    pw_tools::install::methods::ToolInstallResult,
) {
    install_checked(name, root, ToolChecksum::Pinned).await
}

/// `checksum`: 확인 다이얼로그가 보일 체크섬 상태(번들 밖 패키지는 `None`).
async fn install_checked(
    name: &str,
    root: &Path,
    checksum: ToolChecksum,
) -> (
    Arc<ToolsInstaller>,
    pw_tools::install::methods::ToolInstallResult,
) {
    // 번들 밖 패키지(디버거 — M6-19 전)는 내장 스냅숏에 없다 — 고정 판 전체(시험 고정물)를 쓴다. 번들 패키지는 같은 정의다.
    let installer = Arc::new(ToolsInstaller::new(Arc::new(pinned_full_registry())));
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
        ask.checksum, checksum,
        "{name}: the checksum the dialog shows"
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

// ---- 디버거(17-m6-plan.md M6-16 — 실제로 한 번 받아 본다). 번들에 들고 고정 해시를 더하는 것은 M6-19 다 — 그때 `checksum` 을
// `Pinned` 로 바꾼다. 로컬: `cargo nextest run -p pw-tools --run-ignored only -E 'test(/^live_tools_install_dap_/)'`.

fn located(installer: &ToolsInstaller, package: &str, bin: &str) -> std::path::PathBuf {
    match installer.locator().unwrap().locate(package, bin) {
        Located::Installed { path, .. } => path,
        other => panic!("{package} {bin}: {other:?}"),
    }
}

/// 띄운 뒤 `wait` 만큼 기다려 표준 출력 · 오류의 처음을 읽고 끝낸다(포트를 열고 기다리는 어댑터).
fn first_output(path: &Path, args: &[&str], wait: std::time::Duration) -> String {
    use std::io::Read as _;
    let mut child = std::process::Command::new(path)
        .args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    std::thread::sleep(wait);
    let _ = child.kill();
    let _ = child.wait();
    let mut out = String::new();
    child
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut out)
        .unwrap();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut out)
        .unwrap();
    out
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "live: downloads codelldb from GitHub"]
async fn live_tools_install_dap_codelldb() {
    let tmp = tempfile::tempdir().unwrap();
    let (installer, _) = install_checked("codelldb", tmp.path(), ToolChecksum::None).await;
    let path = located(&installer, "codelldb", "codelldb");
    let out = first_output(&path, &["--help"], std::time::Duration::from_secs(3));
    assert!(out.to_lowercase().contains("port"), "{out}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "live: downloads js-debug from GitHub, runs it with node on PATH"]
async fn live_tools_install_dap_js_debug_adapter() {
    let tmp = tempfile::tempdir().unwrap();
    let (installer, _) = install_checked("js-debug-adapter", tmp.path(), ToolChecksum::None).await;
    let path = located(&installer, "js-debug-adapter", "js-debug-adapter");
    let out = first_output(
        &path,
        &["0", "127.0.0.1"],
        std::time::Duration::from_secs(5),
    );
    assert!(out.contains("127.0.0.1"), "{out}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "live: installs debugpy with pip into its own venv"]
async fn live_tools_install_dap_debugpy() {
    let tmp = tempfile::tempdir().unwrap();
    let (installer, _) = install_checked("debugpy", tmp.path(), ToolChecksum::None).await;
    let path = located(&installer, "debugpy", "debugpy-adapter");
    let out = run(&path, &["--help"]);
    assert!(out.contains("--port"), "{out}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "live: downloads java-debug from Open VSX"]
async fn live_tools_install_dap_java_debug_adapter() {
    let tmp = tempfile::tempdir().unwrap();
    let (installer, _) =
        install_checked("java-debug-adapter", tmp.path(), ToolChecksum::None).await;
    let jar = match installer.locator().unwrap().locate_share(
        "java-debug-adapter",
        "java-debug-adapter/com.microsoft.java.debug.plugin.jar",
    ) {
        Located::Installed { path, .. } => path,
        other => panic!("{other:?}"),
    };
    assert!(
        std::fs::metadata(&jar).unwrap().len() > 100_000,
        "{}",
        jar.display()
    );
}
