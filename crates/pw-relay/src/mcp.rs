//! Relay 의 MCP 엔드포인트(18-m7-plan M7-20 — `docs/design/05-mcp.md` 「원격 머신」, 08 「원격 기능 범위」 의 MCP 행).
//!
//! - **소켓**: `pw_mcp::socket` 그대로(그 머신 사용자만 — 폴더 0700 · 소켓 0600 · 상대 uid). 자리는 Relay 의 `$STATE/relay/run/`
//!   아래다(M7-04 ⑦ — Desktop 의 MCP 소켓과 겹치지 않는다). 이름의 열쇠는 (채널, 판)이다([`socket_key`]) — 데몬 소켓처럼 판마다 따로다.
//! - **헤드리스는 없다** — Relay 데몬이 늘 Host 다. 원격의 에이전트는 `pitwall-relay mcp`(또는 사람이 고른 `pitwall` 링크)로 붙고,
//!   데몬이 없으면 그 명령이 띄운다.
//! - **세션은 그 머신만**: 다른 머신을 모른다(`Services::machines` 가 `None`) — `checkout` 에 다른 머신을 주면 `machine_unavailable`
//!   로 거절된다(05 「원격 머신 Relay 의 세션은 그 머신의 체크아웃만」).
//! - **승인**: 승인 허브(`pw_mcp::approval`)는 Relay Host 의 `mcp.approvals` 를 연 창에 보인다 — 링크로 붙은 Desktop 의 그 묶음 창이
//!   그 머신 연결로 연다. role 은 `mcp.*` 선언이 owner 만이라 `readonly` · `mobile` 기기의 링크는 듣지도 답하지도 못한다(M7-02 ②).
//!   여러 Desktop 이면 그 프로젝트를 듣는 창 모두에 보이고 먼저 답한 것(결정 M7-D3).
//! - **자동 승인**(결정 M7-D10 ④): 그 머신의 로컬 규칙(`$CONFIG/relay/settings.toml` 의 `auto_approve`, Relay `state.db` 의 「사람이 본
//!   구성」 으로 정책 자동)이다. 다만 **붙은 owner Desktop 이 없는 동안은 헤드리스와 같다** — `write` 이상은 거절한다([`services`] 가
//!   승인 허브에 `Approvals::require_attendance` 를 건다).
//! - **감사**: `$STATE/relay/mcp/audit.db`(M7-P11). 「에이전트 활동」 패널은 그 머신 연결로 `mcp.audit.list` 를 읽는다.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use pw_core::config::AppPaths;
use pw_core::{HostRouter, LocalHost};
use pw_mcp::audit::AuditLog;
use pw_mcp::policy::Approval;
use pw_mcp::sessions::Sessions;
use pw_mcp::{Catalog, HostKind, Services};

/// 감사 · 로그에 남는 연결 종류.
pub const TRANSPORT: &str = "relay-socket";

/// MCP 소켓 이름의 열쇠 — `pw_mcp::socket::endpoint` 가 `$STATE/relay` 와 함께 해시한다. 판마다 따로다(데몬 소켓과 같다 — 같은
/// 판의 `pitwall-relay mcp` 가 같은 판의 데몬에 붙는다).
pub fn socket_key(channel: &str, version: &str) -> String {
    format!(
        "pitwall-relay-{channel}-{}",
        crate::socket::version_slug(version)
    )
}

/// 이 판 데몬의 MCP 소켓 자리. `paths` 는 Relay 의 앱 경로([`crate::paths::relay_paths`]).
pub fn endpoint(
    paths: &AppPaths,
    channel: &str,
    version: &str,
    temp: &Path,
    uid: u32,
) -> std::io::Result<PathBuf> {
    pw_mcp::socket::endpoint(&socket_key(channel, version), &paths.state, temp, uid)
}

/// Relay 의 MCP 세션들이 함께 쓰는 것. `sessions` 는 Relay Host 에 넣은 등록부(`pw_host::HostParts::mcp_sessions`)와 같은 것이어야
/// 한다 — 링크로 붙은 Desktop 의 `mcp.approvals` · `mcp.approval.answer` 가 그 승인 허브에 닿는다. `attended` 는 「붙은 owner
/// Desktop 이 있는가」(데몬의 `Presence::has_owner`) — 거짓인 동안 `write` 이상은 헤드리스처럼 거절된다(결정 M7-D10 ④).
pub fn services(
    host: &Arc<LocalHost>,
    paths: &AppPaths,
    sessions: Sessions,
    attended: pw_mcp::approval::Attendance,
) -> Arc<Services> {
    services_for(host, paths, sessions, attended, TRANSPORT)
}

/// [`services`] 를 다른 전송 이름으로 — Streamable HTTP(`pw_mcp::http::TRANSPORT`, 18-m7-plan M7-25)가 쓴다. 등록부 · 승인 허브 ·
/// 「붙은 owner」 관문 · 감사는 소켓의 것과 같다 — 전송만 다르다.
pub fn services_for(
    host: &Arc<LocalHost>,
    paths: &AppPaths,
    sessions: Sessions,
    attended: pw_mcp::approval::Attendance,
    transport: &'static str,
) -> Arc<Services> {
    let router = Arc::new(HostRouter::new());
    router
        .add_host(host.clone())
        .expect("빈 라우터에 Host 를 처음 등록한다");
    let global = host
        .config()
        .expect("Relay Host 는 설정을 붙여 만든다")
        .global
        .clone();
    sessions.approvals().require_attendance(attended);
    Arc::new(Services {
        router,
        host_id: crate::host::HOST_ID.into(),
        paths: paths.clone(),
        global,
        bus: host.bus().clone(),
        audit: Arc::new(AuditLog::under_state(&paths.state)),
        catalog: Arc::new(Catalog::linked().routed_by(host.router())),
        // Relay 는 늘 Host 다(헤드리스가 없다). 창은 링크로 붙은 Desktop 의 것이다.
        approval: Approval::for_host(HostKind::App, &sessions),
        transport,
        host_kind: HostKind::App,
        sessions,
        // 세션은 그 머신만이다 — 다른 머신을 모른다.
        machines: None,
    })
}

/// Relay 의 Streamable HTTP(18-m7-plan M7-25): 그 머신 `$CONFIG/relay/settings.toml` 의 `[mcp.http] enabled` 일 때만 루프백에
/// 듣는다(손으로 켠다 — Desktop 화면은 없다). 토큰은 `$STATE/relay/mcp/http-token`(Relay 의 `paths.state`). `connections` 는 MCP
/// 소켓과 같은 것을 넣는다 — 인증을 지나 `initialize` 를 마친 HTTP 세션만 유휴 판정에 든다. `attended` 는 소켓과 같은 관문이다
/// (붙은 owner Desktop 이 없으면 `write` 이상 거절 — 결정 M7-D10 ④). 등록부에 붙여 링크의 `mcp.http.*` 가 닿게 한다. tokio 런타임
/// 안에서 부른다.
pub fn serve_http(
    host: &Arc<LocalHost>,
    paths: &AppPaths,
    sessions: Sessions,
    attended: pw_mcp::approval::Attendance,
    connections: pw_mcp::Connections,
    cancel: tokio_util::sync::CancellationToken,
) -> pw_mcp::http::Http {
    use pw_mcp::http;
    let services = services_for(host, paths, sessions.clone(), attended, http::TRANSPORT);
    let endpoint = http::Http::start(
        http::HttpParts {
            services,
            tokens: http::TokenFile::under_state(&paths.state),
            default_port: http::RELAY_PORT,
            connections,
            limits: http::Limits::default(),
        },
        cancel,
    );
    sessions.attach_http(endpoint.clone());
    endpoint
}

/// `pitwall-relay mcp` 가 붙는다: 데몬이 없으면 데몬 소켓으로 띄우고(`crate::proxy::connect_or_spawn` — 데몬 소켓에 붙은 연결은
/// 곧바로 닫는다), MCP 소켓이 받을 때까지 [`crate::proxy::SPAWN_DEADLINE`] 기다려 핸드셰이크까지 한다. 헤드리스로 떨어지지 않는다.
pub async fn connect_or_spawn(
    daemon_socket: &Path,
    mcp_socket: &Path,
    uid: u32,
    request: pw_mcp::SessionRequest,
    spawn: impl FnOnce() -> std::io::Result<()>,
) -> Result<tokio::net::UnixStream, String> {
    if let Ok(stream) = pw_mcp::socket::connect(mcp_socket, uid, request.clone()).await {
        return Ok(stream);
    }
    drop(crate::proxy::connect_or_spawn(daemon_socket, uid, spawn).await?);
    // 데몬은 데몬 소켓을 연 뒤 MCP 소켓을 연다 — 그 사이를 기다린다.
    let deadline = tokio::time::Instant::now() + crate::proxy::SPAWN_DEADLINE;
    loop {
        match pw_mcp::socket::connect(mcp_socket, uid, request.clone()).await {
            Ok(stream) => return Ok(stream),
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                return Err(format!("cannot use the relay mcp socket: {e}"));
            }
            Err(e) if tokio::time::Instant::now() >= deadline => {
                return Err(format!(
                    "the relay daemon did not open its mcp socket {} within {} s ({e})",
                    mcp_socket.display(),
                    crate::proxy::SPAWN_DEADLINE.as_secs()
                ));
            }
            Err(_) => tokio::time::sleep(std::time::Duration::from_millis(20)).await,
        }
    }
}
