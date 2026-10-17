//! `pitwall mcp [--project <path>]`(M0-17, `docs/design/05-mcp.md` §1)와 앱 쪽 MCP 소켓.
//!
//! - 앱이 떠 있으면(같은 단일 인스턴스 키의 소켓이 있으면) 그 앱의 로컬 소켓에 붙어 stdio 를 잇는 **얇은 프록시**다.
//!   세션 · 툴 · 감사 로그는 앱 안에서 돈다.
//! - 앱이 꺼져 있으면 **헤드리스**: 이 프로세스 안에 `LocalHost` 를 만들어 stdio 로 직접 서비스한다. 레지스트리는 읽기만 하고
//!   ([`pw_core::LocalHostBuilder::read_only_registry`]) 앱을 띄우지 않는다. 승인 경로가 없으므로 `read` 툴만 성공한다.
//! - Windows 는 아직 소켓이 없어 언제나 헤드리스다.
//!
//! **stdout 에는 MCP 바이트만 쓴다.** 로그는 `$STATE/logs/mcp.<날짜>.log`, 경고 이상은 stderr 에도 쓴다(MCP 클라이언트는
//! 서버의 stderr 를 자기 로그로 받는다). 확장은 읽지 않는다 — 지금 툴과 상관없고, 로드 진단이 섞이지 않게.

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Arc;

use pw_core::config::AppPaths;
use pw_core::{HostRouter, LocalHost};
use pw_mcp::audit::AuditLog;
use pw_mcp::policy::Approval;
use pw_mcp::{Catalog, Services, SessionRequest};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::{Layer, fmt};

/// 로그 파일의 구성요소 이름(`mcp.<날짜>.log`). 앱의 `core` 와 나눈다 — 여러 `pitwall mcp` 가 앱과 함께 돌 수 있다.
pub const LOG_COMPONENT: &str = "mcp";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Args {
    pub project: Option<PathBuf>,
}

/// `mcp` 뒤의 인자.
pub fn parse(args: &[OsString]) -> Result<Args, String> {
    let mut project = None;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        let text = arg.to_string_lossy();
        if text == "--project" {
            let value = iter.next().ok_or("--project needs a path")?;
            project = Some(PathBuf::from(value));
        } else if let Some(value) = text.strip_prefix("--project=") {
            project = Some(PathBuf::from(value));
        } else {
            return Err(format!("unknown argument `{text}`"));
        }
    }
    Ok(Args { project })
}

/// `pitwall mcp` 의 본체. 종료 코드를 돌려준다: 0 정상(클라이언트가 닫음), 1 앱이 도중에 닫힘 · 세션 실패, 2 사용법 · 시작 실패.
pub fn run(args: &[OsString], identifier: impl FnOnce(&AppPaths) -> String) -> i32 {
    let args = match parse(args) {
        Ok(args) => args,
        Err(e) => {
            eprintln!("pitwall mcp: {e}\nusage: pitwall mcp [--project <path>]");
            return 2;
        }
    };
    let paths = match AppPaths::from_env() {
        Ok(paths) => paths,
        Err(e) => {
            eprintln!("pitwall mcp: {e}");
            return 2;
        }
    };
    init_logging(&paths);
    let request = SessionRequest {
        project: args.project,
        cwd: std::env::current_dir().ok(),
    };
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(e) => {
            eprintln!("pitwall mcp: cannot start the async runtime: {e}");
            return 2;
        }
    };
    let key = identifier(&paths);
    let code = runtime.block_on(async move {
        #[cfg(unix)]
        if let Some(code) = proxy(&paths, &key, &request).await {
            return code;
        }
        #[cfg(not(unix))]
        let _ = &key;
        headless(paths, request).await
    });
    // 남은 블로킹 작업(감사 로그 쓰기)을 잠깐 기다린다. stdin 이 막힌 읽기는 기다리지 않는다.
    runtime.shutdown_timeout(std::time::Duration::from_secs(2));
    code
}

/// 앱에 붙어 본다. 붙으면 세션이 끝날 때까지 잇고 종료 코드를 돌려준다. 앱이 없으면 `None`(헤드리스로).
#[cfg(unix)]
async fn proxy(paths: &AppPaths, key: &str, request: &SessionRequest) -> Option<i32> {
    use pw_mcp::proxy::ProxyEnd;
    use pw_mcp::socket;

    let uid = socket::current_uid();
    let path = match socket::endpoint(key, &paths.state, &std::env::temp_dir(), uid) {
        Ok(path) => path,
        Err(e) => {
            tracing::warn!(%e, "no mcp socket path; serving headless");
            return None;
        }
    };
    let stream = match socket::connect(&path, uid, request.clone()).await {
        Ok(stream) => stream,
        Err(e)
            if matches!(
                e.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
            ) =>
        {
            tracing::info!(socket = %path.display(), "no running app; serving headless");
            return None;
        }
        Err(e) => {
            // 다른 사용자 소유 폴더 등. 붙지 않고 헤드리스로 — 그래도 조회는 된다.
            tracing::warn!(%e, socket = %path.display(), "not connecting to the app socket; serving headless");
            return None;
        }
    };
    tracing::info!(socket = %path.display(), "connected to the running app");
    let (from_app, to_app) = stream.into_split();
    let mut stdout = tokio::io::stdout();
    match pw_mcp::proxy::run(tokio::io::stdin(), &mut stdout, from_app, to_app).await {
        Ok(ProxyEnd::ClientClosed) => Some(0),
        Ok(ProxyEnd::AppClosed { pending }) => {
            tracing::warn!(pending, "the Pitwall app closed the MCP connection");
            eprintln!("pitwall mcp: the Pitwall app closed the connection");
            Some(1)
        }
        Err(e) => {
            tracing::error!(%e, "mcp proxy failed");
            eprintln!("pitwall mcp: {e}");
            Some(1)
        }
    }
}

async fn headless(paths: AppPaths, request: SessionRequest) -> i32 {
    let host = Arc::new(
        LocalHost::builder(crate::LOCAL_HOST_ID)
            .config(paths.clone())
            .read_only_registry()
            .build(),
    );
    if let Err(e) = host.watch_config(&tokio::runtime::Handle::current()) {
        tracing::warn!(%e, "config watch disabled; tools/list_changed will not follow settings");
    }
    let services = services(&host, &paths, "stdio");
    let cancel = tokio_util::sync::CancellationToken::new();
    match pw_mcp::serve(
        services,
        request,
        (tokio::io::stdin(), tokio::io::stdout()),
        cancel,
    )
    .await
    {
        Ok(()) => 0,
        Err(e) => {
            tracing::error!(%e, "headless mcp session failed");
            eprintln!("pitwall mcp: {e}");
            1
        }
    }
}

/// 세션들이 함께 쓰는 것. 앱과 헤드리스가 같은 규칙으로 만든다.
pub fn services(host: &Arc<LocalHost>, paths: &AppPaths, transport: &'static str) -> Arc<Services> {
    let router = Arc::new(HostRouter::new());
    router
        .add_host(host.clone())
        .expect("빈 라우터에 Host 를 처음 등록한다");
    let global = host
        .config()
        .expect("설정을 붙여 만든 Host 다")
        .global
        .clone();
    Arc::new(Services {
        router,
        host_id: crate::LOCAL_HOST_ID.into(),
        paths: paths.clone(),
        global,
        bus: host.bus().clone(),
        audit: Arc::new(AuditLog::under_state(&paths.state)),
        catalog: Arc::new(Catalog::linked()),
        approval: Approval::Unavailable,
        transport,
    })
}

/// 앱 쪽: 단일 인스턴스 키의 소켓을 열고 세션을 받는다. 열지 못해도 앱은 뜬다(그때 `pitwall mcp` 는 헤드리스로 돈다).
#[cfg(unix)]
pub fn serve_socket(host: Arc<LocalHost>, paths: AppPaths, key: String) {
    use pw_mcp::socket;

    tauri::async_runtime::spawn(async move {
        let uid = socket::current_uid();
        let bound = socket::endpoint(&key, &paths.state, &std::env::temp_dir(), uid)
            .and_then(|path| socket::bind(&path, uid));
        let listener = match bound {
            Ok(listener) => listener,
            Err(e) => {
                tracing::warn!(%e, "mcp socket disabled; `pitwall mcp` will serve headless");
                return;
            }
        };
        tracing::info!(socket = %listener.path().display(), "mcp socket listening");
        let services = services(&host, &paths, "socket");
        socket::accept_loop(
            listener,
            services,
            socket::Connections::default(),
            tokio_util::sync::CancellationToken::new(),
        )
        .await;
    });
}

fn init_logging(paths: &AppPaths) {
    let level = pw_diag::LogConfig::new(paths.state.join("logs"))
        .level_from_env()
        .level;
    let stderr = fmt::layer()
        .with_ansi(false)
        .with_writer(std::io::stderr)
        .with_filter(tracing::level_filters::LevelFilter::WARN.min(level));
    let file = pw_diag::RollingFile::open(
        paths.state.join("logs"),
        LOG_COMPONENT,
        pw_diag::logging::DEFAULT_RETENTION_DAYS,
        Arc::new(pw_diag::SystemClock),
    );
    let registry = tracing_subscriber::registry().with(stderr);
    let installed = match file {
        Ok(file) => tracing::subscriber::set_global_default(
            registry.with(
                fmt::layer()
                    .with_ansi(false)
                    .with_writer(file)
                    .with_filter(level),
            ),
        ),
        Err(e) => {
            eprintln!("pitwall mcp: no log file ({e}); warnings go to stderr only");
            tracing::subscriber::set_global_default(registry)
        }
    };
    if installed.is_ok() {
        tracing::info!(version = env!("CARGO_PKG_VERSION"), "pitwall mcp starting");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
    }

    #[test]
    fn parses_the_project_flag() {
        assert_eq!(parse(&os(&[])).unwrap(), Args { project: None });
        assert_eq!(
            parse(&os(&["--project", "a b"])).unwrap().project,
            Some(PathBuf::from("a b"))
        );
        assert_eq!(
            parse(&os(&["--project=/x"])).unwrap().project,
            Some(PathBuf::from("/x"))
        );
        assert!(parse(&os(&["--project"])).is_err());
        assert!(parse(&os(&["--verbose"])).is_err());
    }
}
