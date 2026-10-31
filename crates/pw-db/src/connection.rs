//! 연결 관리자(M3-05): 관문을 지난 값으로만 드라이버 연결을 연다.
//!
//! 순서는 하나다 — [`DbGrant`](연결 범위, 결정 M3-D18) → 신뢰 게이트와 연결 설정 풀기(M3-02 `Connections::trust_gate` ·
//! `resolve` · `resolve_draft`, 비밀 · 확인 판정 포함 — 터널 목적지 · 인증서 파일의 확인도) → 터널(M3-11 의
//! [`crate::session::DbTunnels`] — 없으면 터널 연결은 열지 않는다) → 등록부의 드라이버(연결 타임아웃 10 s). 터널을 쓰는 연결은
//! [`OpenedConnection::tunnel`] 을 연결과 함께 쥔다. 풀은 드라이버가 갖는다. 연결을 여는 다른 길(M3-10 메타데이터 · M3-12 편집 · M3-22 Redis)도
//! [`connect`] 를 지난다.
//!
//! **연결 테스트** `db.connection.test { projectId, worktree?, connectionId | draft, secret? }`: 저장한 연결이나 저장 전 초안으로 붙어
//! 서버 판본 · TLS 상태를 돌려주고 닫는다. 초안의 비밀은 이 요청으로만 받고 저장하지 않으며 로그에서 가린다(`Secret` 의 `Debug`).

use std::fmt;
use std::sync::Arc;

use pw_db_driver::budget::within;
use pw_db_driver::{Budget, DbConnection, DbDriver};
use pw_db_types::{DbError, DbErrorKind, DriverInfo, Secret, ServerInfo};
use pw_rpc::{Router, RpcError, rpc_method};
use serde::{Deserialize, Serialize};

use crate::config::DbConnectionConfig;
use crate::gate::{DbGrant, db_grant};
use crate::session::{DbSessions, DbTunnelLease, blocking, db_error};
use crate::trust::ResolvedConnection;

/// 연 연결과 그 바탕.
pub struct OpenedConnection {
    pub connection: DbConnection,
    pub server: ServerInfo,
    pub driver: DriverInfo,
    /// 연 때의 연결 설정(비밀 없음).
    pub config: DbConnectionConfig,
    /// SSH 터널(M3-11). **연결을 쥐는 동안 이것도 쥔다** — 먼저 버리면 터널이 닫혀 연결이 끊긴다.
    pub tunnel: Option<DbTunnelLease>,
}

/// 저장한 연결 하나를 연다(머리 주석의 순서). `database` 가 있으면 설정의 DB 대신 그것을 고른다.
pub async fn connect(
    sessions: &DbSessions,
    grant: &DbGrant,
    worktree: Option<&str>,
    connection_id: &str,
    database: Option<String>,
) -> Result<OpenedConnection, RpcError> {
    let c = Arc::clone(&sessions.connections);
    let (project, worktree, id) = (
        grant.project_id().to_owned(),
        worktree.map(str::to_owned),
        connection_id.to_owned(),
    );
    let resolved = blocking(move || {
        let trusted = c.trust_gate(&project, worktree.as_deref())?;
        c.resolve(&trusted, &id)
    })
    .await?;
    let config = resolved.config.clone();
    let (info, connection, server, tunnel) =
        open_resolved(sessions, grant, resolved, database).await?;
    Ok(OpenedConnection {
        connection,
        server,
        driver: info,
        config,
        tunnel,
    })
}

type Opened = (DriverInfo, DbConnection, ServerInfo, Option<DbTunnelLease>);

/// 푼 연결을 드라이버로 연다. 터널이 있으면 [`DbTunnels`] 를 지난다.
async fn open_resolved(
    sessions: &DbSessions,
    grant: &DbGrant,
    resolved: ResolvedConnection,
    database: Option<String>,
) -> Result<Opened, RpcError> {
    let driver = driver_for(sessions, &resolved.config)?;
    let mut spec = resolved.spec;
    if database.is_some() {
        spec.database = database;
    }
    let tunnel = match &resolved.tunnel {
        Some(host_ref) => {
            let Some(tunnels) = sessions.tunnels.get() else {
                return Err(db_error(DbError::unsupported("ssh tunnels")));
            };
            Some(
                tunnels
                    .open(grant.project_id(), &resolved.root, host_ref, &mut spec)
                    .await?,
            )
        }
        None => None,
    };
    let info = DriverInfo {
        id: driver.id().to_owned(),
        dialect: driver.dialect(),
        capabilities: driver.capabilities(),
    };
    let connected = within(Budget::CONSOLE.connect, "connect", driver.connect(spec))
        .await
        .map_err(db_error)?;
    if !connected.connection.matches(&info.capabilities) {
        let _ = connected.connection.close().await;
        return Err(RpcError::internal(format!(
            "db driver `{}` returned a connection of the wrong kind",
            info.id
        )));
    }
    Ok((info, connected.connection, connected.server, tunnel))
}

fn driver_for(
    sessions: &DbSessions,
    config: &DbConnectionConfig,
) -> Result<Arc<dyn DbDriver>, RpcError> {
    sessions.drivers.get(config.driver.id()).ok_or_else(|| {
        db_error(DbError::new(
            DbErrorKind::Unsupported,
            format!("no db driver `{}` is installed", config.driver.id()),
        ))
    })
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DbConnectionTestParams {
    pub project_id: String,
    /// 활성 워크트리의 꼭대기(`ActiveWorktree.top`, 절대 경로).
    #[serde(default)]
    pub worktree: Option<String>,
    /// 저장한 연결. `draft` 와 함께 주면 그 연결을 고치는 중인 초안이다(그 연결의 기억한 확인 · 저장한 비밀을 대상이 같을 때 쓴다).
    #[serde(default)]
    pub connection_id: Option<String>,
    /// 저장하지 않은 초안.
    #[serde(default)]
    pub draft: Option<DbConnectionConfig>,
    /// 초안의 비밀번호 — 이 요청에만 쓰고 저장하지 않는다.
    #[serde(default)]
    #[specta(type = Option<String>)]
    #[schemars(with = "Option<String>")]
    pub secret: Option<Secret>,
}

impl fmt::Debug for DbConnectionTestParams {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DbConnectionTestParams")
            .field("project_id", &self.project_id)
            .field("worktree", &self.worktree)
            .field("connection_id", &self.connection_id)
            .field("draft", &self.draft)
            .field("secret", &self.secret)
            .finish()
    }
}

rpc_method! {
    /// 연결 테스트: 붙어 보고 서버 판본 · TLS 상태를 돌려준 뒤 닫는다. `connectionId`(저장한 연결) 또는 `draft`(저장 전 초안)를 준다.
    /// 초안의 비밀(`secret`)은 저장하지 않는다. 그 프로젝트 창의 연결 + 신뢰한 프로젝트만. 비밀 · 확인이 필요하면 3101 · 3102,
    /// 드라이버가 실패하면 3003(`data.error`).
    pub DbConnectionTest: "db.connection.test" {
        params: DbConnectionTestParams,
        result: ServerInfo,
        tier: Exec,
        roles: [Owner],
    }
}

impl DbSessions {
    /// `db.connection.test`.
    pub async fn test_connection(
        &self,
        grant: &DbGrant,
        params: DbConnectionTestParams,
    ) -> Result<ServerInfo, RpcError> {
        let c = Arc::clone(&self.connections);
        let resolved = blocking(move || {
            let trusted = c.trust_gate(&params.project_id, params.worktree.as_deref())?;
            match (&params.draft, &params.connection_id) {
                (Some(draft), id) => {
                    c.resolve_draft(&trusted, id.as_deref(), draft, params.secret.clone())
                }
                (None, Some(id)) => c.resolve(&trusted, id),
                (None, None) => Err(RpcError::invalid_params(
                    "give connectionId or draft to test",
                )),
            }
        })
        .await?;
        let (info, connection, server, tunnel) = open_resolved(self, grant, resolved, None).await?;
        if let Err(e) = within(Budget::CONSOLE.connect, "close", connection.close()).await {
            tracing::info!(target: "pw_db", driver = %info.id, %e, "closing the test connection failed");
        }
        // 터널은 연결을 닫은 뒤에 놓는다.
        drop(tunnel);
        tracing::info!(target: "pw_db", project = grant.project_id(), driver = %info.id, "db connection test succeeded");
        Ok(server)
    }
}

pub(crate) fn register(router: &mut Router, sessions: Arc<DbSessions>) {
    router.unary(
        DbConnectionTest,
        move |ctx, params: DbConnectionTestParams| {
            let s = Arc::clone(&sessions);
            async move {
                let grant = db_grant(ctx.scope(), &params.project_id)?;
                s.test_connection(&grant, params).await
            }
        },
    );
}
