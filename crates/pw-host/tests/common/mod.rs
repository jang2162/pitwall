//! 시험 도우미: 임시 폴더의 부품으로 조립한 Host. 사람의 키체인 · 설정 · 「Shell 명령」 대상을 건드리지 않는다.

#![allow(dead_code, reason = "each test binary uses a part of the helpers")]

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use pw_core::config::AppPaths;
use pw_host::{Assembled, CliParts, HostParts, SecretStore};
use serde_json::{Value, json};

/// `dir` 아래 앱 폴더로, 주어진 비밀 저장소와 런타임 핸들로 조립한다.
pub fn assemble(
    dir: &Path,
    secrets: Arc<dyn SecretStore>,
    tokio: tokio::runtime::Handle,
) -> Assembled {
    pw_host::assemble(HostParts {
        id: "local".into(),
        paths: AppPaths::under(dir),
        diagnostics: Arc::new(pw_diag::Diagnostics::new(dir.join("logs"), "0.0.0")),
        extensions: Arc::new(pw_ext::Registry::default()),
        i18n: Arc::new(pw_i18n::I18n::new(pw_i18n::Catalog::default(), Vec::new())),
        fs: Arc::new(pw_fs::Fs::new()),
        askpass: pw_git::write::Askpass::default(),
        open_requests: pw_core::open::OpenRequests::default(),
        mcp_sessions: pw_mcp::sessions::Sessions::default(),
        secrets,
        db_drivers: pw_host::db::drivers(),
        cli: CliParts {
            target: Err("not in tests".into()),
            app: Err("not in tests".into()),
            source: pw_open_with::cli::CliSource::Ok,
        },
        settings_window: Arc::new(|_: &str| false),
        tokio,
    })
}

/// 응답 하나를 기다린다(사이의 알림은 건너뛴다). 성공이면 `Ok(result)`, 오류면 `Err(error)`.
pub async fn call(
    connection: &mut pw_core::Connection,
    id: i64,
    method: &str,
    params: Value,
) -> Result<Value, Value> {
    connection
        .sink
        .send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
    loop {
        let frame = tokio::time::timeout(Duration::from_secs(20), connection.source.recv())
            .await
            .unwrap_or_else(|_| panic!("{method}: no reply"))
            .expect("connection closed");
        if frame["id"] == json!(id) {
            return match frame.get("error") {
                Some(error) => Err(error.clone()),
                None => Ok(frame["result"].clone()),
            };
        }
    }
}
