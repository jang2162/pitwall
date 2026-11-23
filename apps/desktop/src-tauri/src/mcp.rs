//! `pitwall mcp [--project <path>]`(M0-17, `docs/design/05-mcp.md` §1)와 앱 쪽 MCP 소켓.
//!
//! - 앱이 떠 있으면(같은 단일 인스턴스 키의 소켓이 있으면) 그 앱의 로컬 소켓에 붙어 stdio 를 잇는 **얇은 프록시**다.
//!   세션 · 툴 · 감사 로그는 앱 안에서 돈다.
//! - 앱이 꺼져 있으면 **헤드리스**: 이 프로세스 안에 `LocalHost` 를 만들어 stdio 로 직접 서비스한다. 레지스트리는 읽기만 하고
//!   ([`pw_core::LocalHostBuilder::read_only_registry`]) 앱을 띄우지 않는다. 헤드리스에는 코어 툴만 있고 승인 경로가 없어 그 가운데 `read` 툴만 성공한다.
//! - 헤드리스로 시작한 뒤 앱이 뜨면 그 뒤의 요청을 앱으로 넘긴다([`handover`] — `tools/list` · `tools/call` 을 받을 때마다 소켓을
//!   한 번 본다).
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
use pw_mcp::sessions::Sessions;
use pw_mcp::{Catalog, HostKind, Services, SessionRequest};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::{Layer, fmt};

#[cfg(unix)]
pub mod handover;
/// 다른 머신을 겨누는 부품. 소켓이 있는 곳에서만 앱 세션이 있다(Windows 는 늘 헤드리스 — 05 「로컬 소켓과 헤드리스」).
#[cfg(unix)]
pub mod machines;

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
    init_logging(&paths, LOG_COMPONENT, "pitwall mcp");
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
        headless(paths, &key, request).await
    });
    // 남은 블로킹 작업(감사 로그 쓰기)을 잠깐 기다린다. stdin 이 막힌 읽기는 기다리지 않는다.
    runtime.shutdown_timeout(std::time::Duration::from_secs(2));
    code
}

/// 실행 중인 앱의 MCP 소켓에 붙는다(핸드셰이크까지). 앱이 없거나 붙을 수 없으면 `None` — 부르는 쪽은 헤드리스로 간다.
/// `pitwall mcp` 와 `pitwall tool` 이 같은 길로 붙는다(05-mcp.md 「CLI」 — 서버 쪽에 CLI 전용 경로를 두지 않는다).
#[cfg(unix)]
pub async fn connect_app(
    paths: &AppPaths,
    key: &str,
    request: &SessionRequest,
) -> Option<tokio::net::UnixStream> {
    match dial_app(paths, key, request).await {
        handover::Dial::Connected(stream) => Some(stream),
        handover::Dial::Absent => {
            tracing::info!("no running app; serving headless");
            None
        }
        handover::Dial::Failed(why) => {
            // 다른 사용자 소유 폴더 등. 붙지 않고 헤드리스로 — 그래도 조회는 된다.
            tracing::warn!(why, "not connecting to the app socket; serving headless");
            None
        }
    }
}

/// [`connect_app`] 의 본체 — 로그를 남기지 않고 「앱이 없다」 와 「앱 자리에 붙지 못했다」 를 가른다. 넘겨 붙기(`handover`)는
/// 이것으로 찾아 실패한 뒤 물러나고 그 경고를 한 번만 남긴다.
#[cfg(unix)]
pub async fn dial_app(
    paths: &AppPaths,
    key: &str,
    request: &SessionRequest,
) -> handover::Dial<tokio::net::UnixStream> {
    use pw_mcp::socket;

    let uid = socket::current_uid();
    let path = match socket::endpoint(key, &paths.state, &std::env::temp_dir(), uid) {
        Ok(path) => path,
        Err(e) => return handover::Dial::Failed(format!("no mcp socket path: {e}")),
    };
    match socket::connect(&path, uid, request.clone()).await {
        Ok(stream) => {
            tracing::info!(socket = %path.display(), "connected to the running app");
            handover::Dial::Connected(stream)
        }
        Err(e)
            if matches!(
                e.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
            ) =>
        {
            tracing::debug!(socket = %path.display(), "no running app");
            handover::Dial::Absent
        }
        Err(e) => handover::Dial::Failed(format!("{e} ({})", path.display())),
    }
}

/// 앱에 붙어 본다. 붙으면 세션이 끝날 때까지 잇고 종료 코드를 돌려준다. 앱이 없으면 `None`(헤드리스로).
#[cfg(unix)]
async fn proxy(paths: &AppPaths, key: &str, request: &SessionRequest) -> Option<i32> {
    use pw_mcp::proxy::ProxyEnd;

    let stream = connect_app(paths, key, request).await?;
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

/// 앱 없이 뜬 `pitwall mcp` · `pitwall tool` 의 Host: 코어 메서드(`host.*` · `config.*` · `project.*`)만, 레지스트리는 읽기 전용.
pub fn headless_host(paths: &AppPaths) -> Arc<LocalHost> {
    Arc::new(
        LocalHost::builder(crate::LOCAL_HOST_ID)
            .config(paths.clone())
            .read_only_registry()
            .build(),
    )
}

/// 앱 없이 이 프로세스 안에서 도는 세션 자리: 헤드리스 Host 와 그 세션들이 함께 쓰는 것. `pitwall mcp`(stdio)와 앱 없는
/// `pitwall tool`(프로세스 안 duplex)이 같은 것을 만든다 — 세션의 판정(카탈로그 · 노출 · 정책 · 감사)이 한 벌이다.
/// 끝낼 때 [`Self::close`].
pub struct Headless {
    pub host: Arc<LocalHost>,
    pub services: Arc<Services>,
}

impl Headless {
    /// `transport` 는 로그에 남는 연결 종류다(판정에는 쓰지 않는다 — `HostKind::Headless`).
    pub fn start(paths: &AppPaths, transport: &'static str) -> Self {
        let host = headless_host(paths);
        // 헤드리스 세션은 보는 화면이 없다 — 등록부는 이 프로세스의 것이다.
        let services = services(
            &host,
            paths,
            transport,
            HostKind::Headless,
            Sessions::default(),
        );
        Self { host, services }
    }

    /// 세션 태스크가 Host 의 복제본을 쥔 채 런타임 종료 시간(`run` 의 2초)을 넘기면 `Drop` 없이 프로세스가 끝나 SQLite 가 닫히지
    /// 않는다 — `state.db-wal` · `-shm` 이 남는다. 종료 전에 직접 닫는다.
    pub fn close(&self) {
        if let Some(registry) = self.host.registry() {
            registry.close();
        }
    }
}

async fn headless(paths: AppPaths, key: &str, request: SessionRequest) -> i32 {
    let headless = Headless::start(&paths, "stdio");
    if let Err(e) = headless
        .host
        .watch_config(&tokio::runtime::Handle::current())
    {
        tracing::warn!(%e, "config watch disabled; tools/list_changed will not follow settings");
    }
    #[cfg(unix)]
    let code = {
        use handover::End;

        // 앱이 뜨면 넘겨 붙는다 — 시작할 때와 같은 길(`dial_app` — `connect_app` 의 본체)로, 시작할 때와 같은 세션 요청(프로젝트 ·
        // cwd)으로 붙는다. 앱 쪽 세션도 같은 범위 · 같은 세션 프로젝트로 열려 프로젝트의 `[mcp]` 좁히기가 그대로 걸린다.
        let (paths, request_ref) = (&paths, &request);
        let outcome = handover::serve_headless(
            Arc::clone(&headless.services),
            request.clone(),
            tokio::io::stdin(),
            tokio::io::stdout(),
            move || dial_app(paths, key, request_ref),
        )
        .await;
        match (outcome.end, outcome.session) {
            (Ok(End::AppClosed { pending }), _) => {
                tracing::warn!(pending, "the Pitwall app closed the MCP connection");
                eprintln!("pitwall mcp: the Pitwall app closed the connection");
                1
            }
            (Err(e), _) => {
                tracing::error!(%e, "mcp handover failed");
                eprintln!("pitwall mcp: {e}");
                1
            }
            (Ok(End::ClientClosed | End::HeadlessClosed), Ok(())) => 0,
            (Ok(End::ClientClosed | End::HeadlessClosed), Err(e)) => {
                tracing::error!(%e, "headless mcp session failed");
                eprintln!("pitwall mcp: {e}");
                1
            }
        }
    };
    #[cfg(not(unix))]
    let code = {
        let _ = key;
        match pw_mcp::serve(
            Arc::clone(&headless.services),
            request,
            (tokio::io::stdin(), tokio::io::stdout()),
            tokio_util::sync::CancellationToken::new(),
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
    };
    headless.close();
    code
}

/// 세션들이 함께 쓰는 것. 앱과 헤드리스가 같은 규칙으로 만든다. `host_kind` 는 `host` 가 앱의 Host 인지 [`headless_host`] 인지다.
/// `sessions` 는 세션 등록부 — 앱은 `mcp.*` RPC(`pw_mcp::methods`)와 같은 것을 넘긴다.
pub fn services(
    host: &Arc<LocalHost>,
    paths: &AppPaths,
    transport: &'static str,
    host_kind: HostKind,
    sessions: Sessions,
) -> Arc<Services> {
    services_with(host, paths, transport, host_kind, sessions, None)
}

/// [`services`] 에 다른 머신 부품(`machines` — 앱만, 18-m7-plan M7-18)을 꽂은 것.
pub fn services_with(
    host: &Arc<LocalHost>,
    paths: &AppPaths,
    transport: &'static str,
    host_kind: HostKind,
    sessions: Sessions,
    machines: Option<Arc<dyn pw_mcp::checkout::Machines>>,
) -> Arc<Services> {
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
        // 실행 파일에 링크된 선언 중 이 Host 가 실제로 처리하는 것만. 헤드리스는 코어 메서드만 등록한다. 세션 툴(`checkout_list` ·
        // `checkout_send` — 18-m7-plan M7-19)은 다른 머신을 아는 앱 세션에만 있다.
        catalog: Arc::new({
            let catalog = Catalog::linked().routed_by(host.router());
            if machines.is_some() && host_kind == HostKind::App {
                catalog
                    .with_session_tools()
                    .unwrap_or_else(|e| panic!("MCP tool catalog: {e}"))
            } else {
                catalog
            }
        }),
        approval: Approval::for_host(host_kind, &sessions),
        transport,
        host_kind,
        sessions,
        machines,
    })
}

/// 앱 쪽: 단일 인스턴스 키의 소켓을 열고 세션을 받는다. 열지 못해도 앱은 뜬다(그때 `pitwall mcp` 는 헤드리스로 돈다).
#[cfg(unix)]
/// `machines` 는 다른 머신을 겨누는 호출의 부품(묶음 · 별명 · 링크 — [`machines::DesktopMachines`]).
pub fn serve_socket(
    host: Arc<LocalHost>,
    paths: AppPaths,
    key: String,
    sessions: Sessions,
    machines: Option<Arc<dyn pw_mcp::checkout::Machines>>,
) {
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
        let services = services_with(&host, &paths, "socket", HostKind::App, sessions, machines);
        socket::accept_loop(
            listener,
            services,
            socket::Connections::default(),
            tokio_util::sync::CancellationToken::new(),
        )
        .await;
    });
}

/// 앱 없이 도는 서브커맨드(`pitwall mcp` · `pitwall tool`)의 로그: `$STATE/logs/<component>.<날짜>.log` 회전 파일, 경고 이상은
/// stderr 에도. stdout 은 쓰지 않는다 — 두 명령 모두 stdout 이 계약이다(MCP 바이트 · 툴 결과). `program` 은 stderr 문장의 머리다.
pub fn init_logging(paths: &AppPaths, component: &str, program: &str) {
    let level = pw_diag::LogConfig::new(paths.state.join("logs"))
        .level_from_env()
        .level;
    let stderr = fmt::layer()
        .with_ansi(false)
        .with_writer(std::io::stderr)
        .with_filter(tracing::level_filters::LevelFilter::WARN.min(level));
    let file = pw_diag::RollingFile::open(
        paths.state.join("logs"),
        component,
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
            eprintln!("{program}: no log file ({e}); warnings go to stderr only");
            tracing::subscriber::set_global_default(registry)
        }
    };
    if installed.is_ok() {
        tracing::info!(version = env!("CARGO_PKG_VERSION"), "{program} starting");
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
