//! `pitwall tool`(`docs/design/05-mcp.md` 「CLI (`pitwall tool`)」, 16-m5-plan M5-04 · 결정 M5-D12). 몸통(플래그 · 출력 · 종료
//! 코드 · 세션의 일)은 `pw_mcp::cli` 다 — 원격 머신의 `pitwall-relay tool` 과 같은 것이다(18-m7-plan M7-20). 여기에는 Desktop 의
//! 세션을 여는 길만 있다: 앱의 소켓이 받으면 앱에(`mcp::connect_app`), 아니면 이 프로세스 안의 헤드리스 Host(`mcp::Headless`)에
//! 프로세스 안 duplex 로 붙는다.

use std::ffi::OsString;
use std::time::Duration;

use pw_core::config::AppPaths;
pub use pw_mcp::cli::*;
use pw_mcp::{Catalog, SessionRequest};

use crate::mcp;

/// `pitwall tool` 의 본체. 종료 코드를 돌려준다. `identifier` 는 단일 인스턴스 키(앱의 소켓 이름)를 계산한다.
pub fn run(args: &[OsString], identifier: impl FnOnce(&AppPaths) -> String) -> i32 {
    let (invocation, input) = match begin(args) {
        Ok(begun) => begun,
        Err(code) => return code,
    };
    let paths = match AppPaths::from_env() {
        Ok(paths) => paths,
        Err(e) => {
            eprintln!("pitwall tool: {e}");
            return exit::USAGE;
        }
    };
    mcp::init_logging(&paths, LOG_COMPONENT, "pitwall tool");
    let session = SessionRequest {
        project: invocation.project.clone(),
        cwd: std::env::current_dir().ok(),
    };
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(e) => {
            eprintln!("pitwall tool: cannot start the async runtime: {e}");
            return exit::FAILED;
        }
    };
    let key = identifier(&paths);
    let linked = Catalog::linked();
    let code = runtime.block_on(async {
        let interrupt = interrupted();
        let mut stdout = std::io::stdout();
        let mut stderr = std::io::stderr();
        #[cfg(unix)]
        if let Some(stream) = mcp::connect_app(&paths, &key, &session).await {
            let request = Request {
                action: &invocation.action,
                input,
                linked: &linked,
                headless: None,
            };
            return execute(request, stream, interrupt, &mut stdout, &mut stderr).await;
        }
        #[cfg(not(unix))]
        let _ = &key;
        let headless = mcp::Headless::start(&paths, "cli");
        let (server_io, client_io) = tokio::io::duplex(64 * 1024);
        let cancel = tokio_util::sync::CancellationToken::new();
        let server = tokio::spawn(pw_mcp::serve(
            std::sync::Arc::clone(&headless.services),
            session,
            tokio::io::split(server_io),
            cancel.clone(),
        ));
        let request = Request {
            action: &invocation.action,
            input,
            linked: &linked,
            headless: Some(&headless.services.catalog),
        };
        let code = execute(request, client_io, interrupt, &mut stdout, &mut stderr).await;
        // 클라이언트가 전송을 닫았으니 세션도 끝난다. 남은 감사 기록까지 기다리되 오래 걸리면 끊는다.
        match tokio::time::timeout(Duration::from_secs(2), server).await {
            Ok(Ok(Err(e))) => tracing::warn!(%e, "headless mcp session failed"),
            Ok(_) => {}
            Err(_) => cancel.cancel(),
        }
        headless.close();
        code
    });
    runtime.shutdown_timeout(Duration::from_secs(2));
    code
}

#[cfg(test)]
#[path = "tool_cli_tests.rs"]
mod tests;
