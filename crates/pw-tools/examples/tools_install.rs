//! 앱과 같은 설치기로 도구 하나를 설치한다 — CI `lsp-live` 가 서버를 받는 자리(15-m4-plan.md M4-04 · M4-18).
//!
//! ```text
//! cargo run -q -p pw-tools --example tools_install -- <패키지> <$DATA 폴더> <$STATE 폴더>
//! ```
//!
//! registry 는 이 Pitwall 판의 고정 스냅숏(`registry.toml` · 내장 스냅숏)이고, 고정 해시 · 압축 풀기 검사 · npm · pip 격리가 앱과
//! 같다. npm · python 은 이 프로세스의 PATH 에서 찾는다(`ProcessToolchains`). 라이선스 · 출처 확인(4001)은 이 명령을 부른 것을
//! 확인으로 보고 그 값을 표준 오류에 찍은 뒤 받아들인다 — 사람의 앱에서는 창이 묻는다. 끝나면 실행 파일의 절대 경로를 JSON 으로
//! 표준 출력에 낸다: `{"name", "version", "checksum", "bins": {"<이름>": "<경로>"}}`.

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use pw_core::bus::{EventBus, Scope};
use pw_core::task::TaskSpawner;
use pw_lsp_servers::{Located, ToolLocator};
use pw_rpc::ConnectionScope;
use pw_tools::install::methods::{ToolConfirmRequired, ToolConfirmed};
use pw_tools::install::{InstallerHost, ProcessToolchains, code, tool_grant};
use pw_tools::{ToolsInstaller, ToolsRegistry};
use serde_json::json;
use tokio_util::sync::CancellationToken;

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [name, data, state] = args.as_slice() else {
        eprintln!("usage: tools_install <package> <data-dir> <state-dir>");
        return ExitCode::from(2);
    };
    let installer = Arc::new(ToolsInstaller::new(Arc::new(ToolsRegistry::new())));
    let tasks = TaskSpawner::new(EventBus::new(64), Scope::Host, CancellationToken::new());
    installer.attach(InstallerHost {
        tools_dir: PathBuf::from(data).join("tools"),
        state_dir: PathBuf::from(state),
        tasks,
        toolchains: Arc::new(ProcessToolchains),
        settings_window: Arc::new(|label: &str| label == "cli"),
    });
    let grant = tool_grant(&ConnectionScope::Host, Some("cli"), &|l: &str| l == "cli")
        .expect("the cli grant");
    let mut confirmed: Option<ToolConfirmed> = None;
    let result = loop {
        match installer
            .install(&grant, name, None, confirmed.as_ref(), false)
            .await
        {
            Err(e) if e.code == code::CONFIRM_REQUIRED && confirmed.is_none() => {
                let ask: ToolConfirmRequired =
                    serde_json::from_value(e.data.unwrap_or_default()).expect("4001 data");
                eprintln!(
                    "confirming {} {}: licenses {:?}, sources {:?}, checksum {:?}",
                    ask.name, ask.version, ask.licenses, ask.source_domains, ask.checksum
                );
                confirmed = Some(ToolConfirmed {
                    version: ask.version,
                    licenses: ask.licenses,
                    source_domains: ask.source_domains,
                });
            }
            other => break other,
        }
    };
    let result = match result {
        Ok(r) => r,
        Err(e) => {
            eprintln!("install failed ({}): {} {:?}", e.code, e.message, e.data);
            if let Ok(Some(log)) = installer.read_log(name) {
                eprintln!("--- install log ---\n{log}");
            }
            return ExitCode::FAILURE;
        }
    };
    let locator = installer.locator().expect("attached");
    let record = installer
        .record(name)
        .ok()
        .flatten()
        .expect("an install record");
    let mut bins = serde_json::Map::new();
    for bin in record
        .current_entry()
        .map(|e| e.bins.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default()
    {
        if let Located::Installed { path, .. } = locator.locate(name, &bin) {
            bins.insert(bin, json!(path));
        }
    }
    println!(
        "{}",
        json!({
            "name": result.name,
            "version": result.version,
            "checksum": result.checksum,
            "bins": bins,
        })
    );
    ExitCode::SUCCESS
}
