//! DB 연결 쓰기 툴 메서드(16-m5-plan.md M5-17, `docs/design/05-mcp.md` 「DB」 표, F10-2). 그룹 `db`, 둘 다 `write`.
//!
//! - `db_connection_upsert` ← `db.tool.connectionUpsert`: 연결 설정 파일 `.pitwall/db/<id>.toml`(공유) 또는 `<id>.local.toml`(개인)을
//!   만들거나 고친다 — `db.connection.upsert` 와 같은 함수(`Connections::plan_upsert` · `apply_upsert`). 계획은 그 파일의 앞뒤
//!   내용(diff)이고, 승인 뒤에는 계획 때 읽은 내용과 같을 때만 쓴다. **비밀은 인자로 받지 않는다** — 입력의 비밀번호 칸은 출처
//!   (`keychain` — 사람이 Pitwall 에서 넣는다, `env` — 변수 이름)뿐이다. 저장이 확인(`crate::trust` 의 터널 목적지 · 인증서 · `env`
//!   확인)을 대신하지 않는다 — 열 때 사람이 따로 확인한다(14-m3-plan.md M3-11). **표지는 조이기만 한다**(메인 판정 2026-11-17
//!   — 리뷰 B1): 기존 연결의 프로덕션 · read-only 를 내리거나, 표지가 붙은 연결의 대상을 바꾸거나, 이번 실행에서 표지가 기억된
//!   대상을 표지 없이 가리키는 쓰기는 묻지 않고 거절한다(`crate::gate::check_agent_write`).
//! - `db_connection_test` ← `db.tool.connectionTest`(결정 M5-D21): 입력은 `{ projectId, worktree?, connectionId }` 뿐이다 — 밑의
//!   RPC(`db.connection.test`)가 받는 `draft` · `secret` 은 없다(초안은 터널 목적지 · 인증서 확인을 묻지 않는 예외를 탄다). 관문은
//!   [`db_grant_to_open`]: 사람이 연 연결(또는 이 세션이 승인받은 연결)이면 `write` 의 보통 규칙으로 묻고, 아니면 **「이 연결 열기」
//!   승인**을 청한다 — 계획이 푼 대상을 보이고, 규칙은 `auto_approve` 없음 · 「이 세션 동안 허용」 은 (그 연결 id, 푼 대상)의 열쇠다.
//!   승인을 지나면 어느 답이든 그 세션 · 그 연결 id · 그 대상을 「승인받은 연결」 로 기록하고(`DbGrant::remember_open_approval` — 그
//!   세션의 `db_schema` · `db_query` 등도 쓴다), 세션이 끝나면 지운다. 프로덕션 대상은 승인을 묻지 않고 거절한다(관문).
//! - **비밀 입력 요청**: 연결에 비밀이 필요하면(키체인에 없거나 대상이 바뀜 — `crate::trust::code::SECRET_REQUIRED`) 그 프로젝트
//!   창에 [`TOPIC_SECRET_REQUESTED`] 를 내고(창은 알림을 띄워 연결 편집 화면의 비밀 칸으로 보낸다), 에이전트에게는 사람이 Pitwall 에서
//!   넣어야 한다고 답한다.
//!
//! 둘 다 MCP 세션의 연결 범위만 받는다(`agent_grant`).

use std::sync::Arc;

use pw_core::bus::Scope;
use pw_core::registry::agent::agent_grant;
use pw_core::registry::plan::{PlanChange, PlanRules, Planned, Remember, ToolPlan};
use pw_db_types::{DriverKind, ServerInfo, TlsStatus};
use pw_rpc::{Ctx, Router, RpcError, Tier, rpc_method};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::config::{
    DbConfigLocation, DbConnectionConfig, DbConnectionParams, DbConnectionState,
    DbConnectionUpsertParams, DbPasswordRef, DbSshTunnel, check_connection_id,
};
use crate::connection::DbConnectionTestParams;
use crate::gate::{DbGrant, DbTarget, db_grant_to_open};
use crate::schema::check_text;
use crate::session::{DbSessions, blocking};
use crate::tool_plan::{EVERY_TIME, db_plan_with};
use crate::trust::code as trust_code;

/// 연결에 비밀이 필요하다 — 그 프로젝트 창이 알림을 띄워 연결 편집 화면의 비밀 칸으로 보낸다. payload [`DbSecretRequested`],
/// 그 프로젝트 범위.
pub const TOPIC_SECRET_REQUESTED: &str = "db.secretRequested";

/// [`TOPIC_SECRET_REQUESTED`] 의 payload. 비밀 값은 없다.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DbSecretRequested {
    pub connection_id: String,
    /// 연결 이름(설정의 `name`).
    pub name: String,
    /// 그 연결 설정을 읽은 워크트리 꼭대기(없으면 메인 체크아웃).
    pub worktree: Option<String>,
}

// --- db_connection_upsert -------------------------------------------------------------------------------------------

/// Where the password comes from. There is no place for the password itself: the person enters it in Pitwall
/// (`keychain`), or it is read from an environment variable of the project's runtime environment (`env`).
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum DbToolPasswordRef {
    /// The person enters the password in Pitwall (it is kept in the OS keychain). After saving, Pitwall asks the
    /// person in the project window.
    Keychain,
    /// The value of this environment variable (only its name is saved). Pitwall asks the person to confirm the
    /// variable before it first sends its value.
    Env { variable: String },
}

/// TLS mode (libpq meaning).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum DbToolSslMode {
    /// No TLS.
    Disable,
    /// Plain first, TLS if the server asks (not verified).
    Allow,
    /// TLS first, plain if not (not verified).
    Prefer,
    /// TLS required, certificate not verified.
    Require,
    /// TLS required, certificate chain verified (not the name).
    VerifyCa,
    /// TLS required, chain and name verified.
    VerifyFull,
}

impl DbToolSslMode {
    fn into_mode(self) -> pw_db_types::SslMode {
        use pw_db_types::SslMode;
        match self {
            Self::Disable => SslMode::Disable,
            Self::Allow => SslMode::Allow,
            Self::Prefer => SslMode::Prefer,
            Self::Require => SslMode::Require,
            Self::VerifyCa => SslMode::VerifyCa,
            Self::VerifyFull => SslMode::VerifyFull,
        }
    }
}

/// SSH tunnel: one destination only — a host name from `~/.ssh/config` or `ssh://[user@]host[:port]`.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DbToolSshTunnel {
    pub host_ref: String,
}

/// Connection settings (the whole connection — keys left out are removed from the file). No secret values.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DbToolConnectionConfig {
    /// Name shown to the person.
    pub name: String,
    pub driver: DriverKind,
    /// DB host (not for SQLite). With a tunnel, the name as seen from the SSH host.
    #[serde(default)]
    pub host: Option<String>,
    /// The driver's default if not set.
    #[serde(default)]
    pub port: Option<u16>,
    #[serde(default)]
    pub database: Option<String>,
    #[serde(default)]
    pub user: Option<String>,
    /// Where the password comes from (no value). None means no password.
    #[serde(default)]
    pub password: Option<DbToolPasswordRef>,
    #[serde(default)]
    pub read_only: bool,
    /// Production mark. Agent DB tools do not use production targets; a mark once seen for a target stays for this
    /// app run even if removed from the file.
    #[serde(default)]
    pub production: bool,
    /// Color token or `#rrggbb`.
    #[serde(default)]
    pub color: Option<String>,
    /// Without it: `disable` for a loopback host, `verify-full` otherwise.
    #[serde(default)]
    pub sslmode: Option<DbToolSslMode>,
    /// CA certificate file (relative to the project root).
    #[serde(default)]
    pub sslrootcert: Option<String>,
    /// Client certificate file.
    #[serde(default)]
    pub sslcert: Option<String>,
    /// Client key file.
    #[serde(default)]
    pub sslkey: Option<String>,
    #[serde(default)]
    pub ssh_tunnel: Option<DbToolSshTunnel>,
    /// SQLite file (relative to the project root).
    #[serde(default)]
    pub path: Option<String>,
    /// Seconds to wait when connecting (1-600).
    #[serde(default)]
    pub connect_timeout: Option<u32>,
}

impl DbToolConnectionConfig {
    fn into_config(self) -> DbConnectionConfig {
        DbConnectionConfig {
            name: self.name,
            driver: self.driver,
            host: self.host,
            port: self.port,
            database: self.database,
            user: self.user,
            password: self.password.map(|p| match p {
                DbToolPasswordRef::Keychain => DbPasswordRef::Keychain,
                DbToolPasswordRef::Env { variable } => DbPasswordRef::Env { variable },
            }),
            read_only: self.read_only,
            production: self.production,
            color: self.color,
            sslmode: self.sslmode.map(DbToolSslMode::into_mode),
            sslrootcert: self.sslrootcert,
            sslcert: self.sslcert,
            sslkey: self.sslkey,
            ssh_tunnel: self.ssh_tunnel.map(|t| DbSshTunnel {
                host_ref: t.host_ref,
            }),
            path: self.path,
            connect_timeout: self.connect_timeout,
        }
    }
}

/// Which file to write.
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    specta::Type,
    schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum DbToolConfigLocation {
    /// `.pitwall/db/<id>.toml` in the worktree (committed, shared with the team).
    #[default]
    Shared,
    /// `.pitwall/db/<id>.local.toml` in the main checkout (personal, not committed).
    Local,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DbToolConnectionUpsertParams {
    pub project_id: String,
    /// Top of the worktree (absolute path). The shared file is written in that worktree. Empty means the session's
    /// worktree, or else the main checkout.
    #[serde(default)]
    pub worktree: Option<String>,
    /// Connection id (= file name): letters, digits, `-`, `_`, 1-64 characters.
    pub connection_id: String,
    pub config: DbToolConnectionConfig,
    #[serde(default)]
    pub location: DbToolConfigLocation,
}

/// Whether the connection can be opened now, after saving.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum DbToolSavedState {
    /// The project is not trusted — no connection opens until the person trusts it.
    Untrusted,
    /// The merged settings (shared and personal file) are wrong — see the reason in Pitwall.
    Invalid,
    /// The password is needed — the person enters it in Pitwall (a request was shown in the project window).
    SecretRequired,
    /// The person's confirmation is needed (an environment variable, SSH tunnel destination, certificate files or a
    /// SQLite file outside the project) — the person confirms it in Pitwall when the connection is opened.
    ApprovalRequired,
    Ready,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DbToolConnectionUpsert {
    pub connection_id: String,
    /// The file written (absolute path).
    pub file: String,
    pub state: DbToolSavedState,
    /// What the person has to do next, if anything.
    pub notice: Option<String>,
}

// --- db_connection_test ---------------------------------------------------------------------------------------------

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DbToolConnectionTestParams {
    pub project_id: String,
    /// Top of the worktree (absolute path). Connection settings are read from that worktree. Empty means the
    /// session's worktree, or else the main checkout.
    #[serde(default)]
    pub worktree: Option<String>,
    /// A saved connection's id (see `db_connection_list`).
    pub connection_id: String,
}

/// TLS of the test connection.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum DbToolTls {
    /// TLS, certificate verified.
    Verified,
    /// TLS, certificate not verified (or its name not checked).
    Unverified,
    /// No TLS.
    Plaintext,
    /// A file DB (SQLite) — no network.
    NotApplicable,
}

impl DbToolTls {
    fn of(tls: TlsStatus) -> Self {
        match tls {
            TlsStatus::Verified => Self::Verified,
            TlsStatus::Unverified => Self::Unverified,
            TlsStatus::Plaintext => Self::Plaintext,
            TlsStatus::NotApplicable => Self::NotApplicable,
        }
    }
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DbToolConnectionTest {
    /// Version the server reported.
    pub version: Option<String>,
    pub tls: DbToolTls,
    /// This call was approved to open a connection the person had not opened: the other DB tools of this agent
    /// session may use it too (same connection id and target) until the session ends.
    pub approved_for_session: bool,
}

rpc_method! {
    /// Create or change a DB connection's settings file — `.pitwall/db/<id>.toml` (shared, `location: "shared"`, the
    /// default) or `<id>.local.toml` (personal, `location: "local"`). `config` is the whole connection: keys left out
    /// are removed from that file. The person sees the file's diff and approves it in Pitwall first. **No secret is
    /// taken**: `password` only says where the password comes from — `{ "kind": "keychain" }` (the person enters it in
    /// Pitwall; a request is shown in the project window) or `{ "kind": "env", "variable": "NAME" }`. Saving does not
    /// approve anything: SSH tunnel destinations, certificate files and environment variables are still confirmed by
    /// the person when the connection is opened. Agent DB tools do not use a connection until the person opened it in
    /// Pitwall, or approved it through `db_connection_test`.
    pub DbToolConnectionUpsertMethod: "db.tool.connectionUpsert" {
        params: Planned<DbToolConnectionUpsertParams>,
        result: DbToolConnectionUpsert,
        tier: Write,
        roles: [Owner],
        mcp: { group: "db", name: "db_connection_upsert" },
    }

    /// Test a saved DB connection: connect, report the server version and TLS, and close. The input is the connection
    /// id only (no draft settings, no secrets). If the person opened this connection (with its current target) in
    /// Pitwall's DB explorer during this app run, it follows the usual approval rules for `write`. Otherwise this asks
    /// the person to **open this connection for this agent session**: the request shows its target (host · port ·
    /// database · user · SSH tunnel, or the SQLite file), and once approved the other DB tools of this session
    /// (`db_schema`, `db_query`, …) may use the same connection id and target until the session ends. Connections
    /// whose target is marked production are refused without asking — only the person can open them. If the
    /// connection needs a password or a confirmation, the person has to give it in Pitwall (a request is shown in the
    /// project window); call again after that.
    pub DbToolConnectionTestMethod: "db.tool.connectionTest" {
        params: Planned<DbToolConnectionTestParams>,
        result: DbToolConnectionTest,
        tier: Write,
        roles: [Owner],
        mcp: { group: "db", name: "db_connection_test" },
    }
}

/// `db.tool.connectionUpsert` 의 RPC 이름.
pub const UPSERT_METHOD: &str = "db.tool.connectionUpsert";
/// `db.tool.connectionTest` 의 RPC 이름.
pub const TEST_METHOD: &str = "db.tool.connectionTest";

/// 「이 연결 열기」 의 세션 허용 열쇠: 연결 id 와 푼 대상의 해시. 같은 id 가 다른 대상을 가리키면 다른 열쇠다.
pub fn open_key(connection_id: &str, target: &DbTarget) -> String {
    let mut hasher = Sha256::new();
    hasher.update(serde_json::to_vec(target).unwrap_or_default());
    let digest = hasher.finalize();
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    format!("{connection_id}@sha256:{hex}")
}

/// 사람이 승인 다이얼로그에서 보는 대상(푼 값 — 같은 대상의 판정에 쓰는 값 그대로).
fn describe(target: &DbTarget, worktree: Option<&str>) -> String {
    let mut lines = vec![format!("driver: {}", target.driver.id())];
    let mut add = |label: &str, value: Option<String>| {
        if let Some(value) = value {
            lines.push(format!("{label}: {value}"));
        }
    };
    add("host", target.host.clone());
    add("port", target.port.map(|p| p.to_string()));
    add("database", target.database.clone());
    add("user", target.user.clone());
    add("ssh tunnel", target.tunnel.clone());
    add(
        "file",
        target.file.as_ref().map(|f| f.display().to_string()),
    );
    add("worktree", worktree.map(str::to_owned));
    lines.join("\n")
}

/// 비밀이 필요하다고 그 프로젝트 창에 알린다.
fn request_secret(sessions: &DbSessions, project_id: &str, payload: &DbSecretRequested) {
    let Ok(host) = sessions.connections.host() else {
        return;
    };
    tracing::info!(target: "pw_db", project = project_id, connection = %payload.connection_id, "asking the project window for a db connection's password");
    if let Err(e) = host.bus.publish(
        Scope::Project(project_id.to_owned()),
        TOPIC_SECRET_REQUESTED,
        payload,
    ) {
        tracing::warn!(target: "pw_db", %e, "cannot publish db.secretRequested");
    }
}

/// 에이전트에게: 비밀은 사람이 Pitwall 에서 넣는다(코드 3101 그대로, 대상 값은 싣지 않는다).
fn secret_required(connection_id: &str) -> RpcError {
    RpcError::new(
        trust_code::SECRET_REQUIRED,
        format!(
            "db connection `{connection_id}` needs its password, which only the person can enter in Pitwall; a request \
             was shown in the project window — call again after the person has entered it"
        ),
    )
    .with_data(json!({ "reason": "secretRequired", "connectionId": connection_id }))
}

/// 에이전트에게: 사람의 확인이 필요하다(코드 3102 그대로, 확인할 값은 싣지 않는다).
fn confirmation_required(connection_id: &str) -> RpcError {
    RpcError::new(
        trust_code::APPROVAL_REQUIRED,
        format!(
            "db connection `{connection_id}` needs the person's confirmation in Pitwall (an environment variable, SSH \
             tunnel destination, certificate files or a SQLite file outside the project); ask the person to open it once \
             in Pitwall's DB explorer, then call again"
        ),
    )
    .with_data(json!({ "reason": "confirmationRequired", "connectionId": connection_id }))
}

impl DbSessions {
    /// `db_connection_upsert`.
    async fn tool_connection_upsert(
        self: &Arc<Self>,
        ctx: &Ctx,
        planned: Planned<DbToolConnectionUpsertParams>,
    ) -> Result<DbToolConnectionUpsert, RpcError> {
        let p = planned.params.clone();
        check_text("worktree", p.worktree.as_deref())?;
        check_connection_id(&p.connection_id)?;
        let (s, scope) = (Arc::clone(self), ctx.scope().clone());
        let ticket = planned.ticket.clone();
        blocking(move || {
            let c = &s.connections;
            let agent = agent_grant(&scope, &c.host()?.projects, &p.project_id)?;
            let params = DbConnectionUpsertParams {
                project_id: agent.project_id().to_owned(),
                worktree: p.worktree.clone(),
                connection_id: p.connection_id.clone(),
                config: p.config.clone().into_config(),
                location: match p.location {
                    DbToolConfigLocation::Shared => DbConfigLocation::Shared,
                    DbToolConfigLocation::Local => DbConfigLocation::Local,
                },
            };
            let file = {
                // 판정 · 계획 · 승인 · 쓰기를 한 잠금 아래에 — 계획 때 읽은 내용과 쓰기 직전의 내용을 견준다(`apply_upsert`).
                let _writing = crate::config::lock(&c.writing);
                // 에이전트의 쓰기는 표지를 조이기만 한다 — 묻기 전에 거절한다(메인 판정 2026-11-17,
                // `crate::gate::check_agent_write`).
                crate::gate::check_agent_write(
                    &s,
                    &params.project_id,
                    params.worktree.as_deref(),
                    &params.connection_id,
                    &params.config,
                )?;
                let plan = c.plan_upsert(&params)?;
                let path = plan.file.display().to_string();
                let tool_plan = ToolPlan::builder(UPSERT_METHOD, Tier::Write, &p)
                    .summary(format!(
                        "Save DB connection `{}` ({})",
                        p.connection_id, p.config.name
                    ))
                    .target("before", &plan.before)
                    .change(PlanChange::File {
                        path: path.clone(),
                        before: plan.before.clone(),
                        after: Some(plan.after.clone()),
                    })
                    .build();
                pw_core::registry::plan::approve(ticket.as_ref(), tool_plan)?;
                c.apply_upsert(&params, &plan)?;
                path
            };
            c.refresh(&params.project_id, params.worktree.as_deref());
            let entry = c.get(&DbConnectionParams {
                project_id: params.project_id.clone(),
                worktree: params.worktree.clone(),
                connection_id: params.connection_id.clone(),
            })?;
            let (state, notice) = match &entry.state {
                DbConnectionState::Untrusted => (
                    DbToolSavedState::Untrusted,
                    Some("The project is not trusted; the person has to trust it in Pitwall before any connection opens.".to_owned()),
                ),
                DbConnectionState::Invalid { .. } => (
                    DbToolSavedState::Invalid,
                    Some(crate::gate::invalid_settings(&params.connection_id)),
                ),
                DbConnectionState::SecretRequired { .. } => {
                    request_secret(
                        &s,
                        &params.project_id,
                        &DbSecretRequested {
                            connection_id: params.connection_id.clone(),
                            name: params.config.name.clone(),
                            worktree: params.worktree.clone(),
                        },
                    );
                    (
                        DbToolSavedState::SecretRequired,
                        Some("This connection needs its password, which only the person can enter in Pitwall; a request was shown in the project window.".to_owned()),
                    )
                }
                DbConnectionState::ApprovalRequired { .. } => (
                    DbToolSavedState::ApprovalRequired,
                    Some("The person has to confirm this connection's settings in Pitwall when it is opened.".to_owned()),
                ),
                DbConnectionState::Ready => (DbToolSavedState::Ready, None),
            };
            Ok(DbToolConnectionUpsert {
                connection_id: params.connection_id,
                file,
                state,
                notice,
            })
        })
        .await
    }

    /// `db_connection_test`.
    async fn tool_connection_test(
        self: &Arc<Self>,
        ctx: &Ctx,
        planned: Planned<DbToolConnectionTestParams>,
    ) -> Result<DbToolConnectionTest, RpcError> {
        let p = planned.params.clone();
        check_text("worktree", p.worktree.as_deref())?;
        let session = ctx.session();
        let (s, scope, params) = (Arc::clone(self), ctx.scope().clone(), p.clone());
        let (mut grant, state): (DbGrant, DbConnectionState) = blocking(move || {
            let c = &s.connections;
            let agent = agent_grant(&scope, &c.host()?.projects, &params.project_id)?;
            let grant = db_grant_to_open(
                &s,
                &agent,
                session,
                params.worktree.as_deref(),
                &params.connection_id,
            )?;
            // 비밀 · 확인이 필요한 연결은 승인을 묻기 전에 사람에게 넘긴다(값을 읽지 않는 판정 — 목록과 같다).
            let entry = c.get(&DbConnectionParams {
                project_id: params.project_id.clone(),
                worktree: params.worktree.clone(),
                connection_id: params.connection_id.clone(),
            })?;
            Ok((grant, entry.state))
        })
        .await?;
        let name = grant.agent_connection_name().unwrap_or("?").to_owned();
        match state {
            DbConnectionState::SecretRequired { .. } => {
                request_secret(
                    self,
                    &p.project_id,
                    &DbSecretRequested {
                        connection_id: p.connection_id.clone(),
                        name,
                        worktree: p.worktree.clone(),
                    },
                );
                return Err(secret_required(&p.connection_id));
            }
            DbConnectionState::ApprovalRequired { .. } => {
                return Err(confirmation_required(&p.connection_id));
            }
            DbConnectionState::Untrusted | DbConnectionState::Invalid { .. } => {
                // 관문이 이미 거절했다(신뢰 · 틀린 설정) — 여기 오지 않는다.
            }
            DbConnectionState::Ready => {}
        }
        let driver = grant.agent_driver();
        let dialect = driver
            .and_then(|kind| self.drivers.get(kind.id()))
            .and_then(|d| d.dialect());
        let target = grant
            .plan_target()
            .map(|t| describe(t.target, t.worktree))
            .unwrap_or_default();
        let opens = grant.needs_open_approval();
        let (summary, rules) = if opens {
            let key = grant
                .plan_target()
                .map(|t| open_key(t.connection_id, t.target))
                .unwrap_or_default();
            (
                format!(
                    "Open this DB connection for this agent session: `{name}` ({})",
                    p.connection_id
                ),
                PlanRules {
                    auto_approve: false,
                    remember: Remember::Key { key },
                },
            )
        } else if grant.needs_approval() {
            (
                format!(
                    "Test the production DB connection `{name}` ({})",
                    p.connection_id
                ),
                EVERY_TIME,
            )
        } else {
            (
                format!("Test the DB connection `{name}` ({})", p.connection_id),
                PlanRules::default(),
            )
        };
        let plan = db_plan_with(
            TEST_METHOD,
            Tier::Write,
            &p,
            &grant,
            dialect,
            summary,
            Some(PlanChange::Text {
                label: "Connection".to_owned(),
                text: target,
            }),
            rules,
        );
        let approved = planned.approve(plan)?;
        grant.approve(&approved);
        let approved_for_session =
            grant.remember_open_approval(self, session, ctx.connection_closed());
        let tested = self
            .test_connection(
                &grant,
                DbConnectionTestParams {
                    project_id: p.project_id.clone(),
                    worktree: p.worktree.clone(),
                    connection_id: Some(p.connection_id.clone()),
                    draft: None,
                    secret: None,
                },
            )
            .await;
        let server: ServerInfo = match tested {
            Ok(server) => server,
            Err(e) if e.code == trust_code::SECRET_REQUIRED => {
                request_secret(
                    self,
                    &p.project_id,
                    &DbSecretRequested {
                        connection_id: p.connection_id.clone(),
                        name: grant.agent_connection_name().unwrap_or("?").to_owned(),
                        worktree: p.worktree.clone(),
                    },
                );
                return Err(secret_required(&p.connection_id));
            }
            Err(e) if e.code == trust_code::APPROVAL_REQUIRED => {
                return Err(confirmation_required(&p.connection_id));
            }
            Err(e) => return Err(e),
        };
        Ok(DbToolConnectionTest {
            version: server.version,
            tls: DbToolTls::of(server.tls),
            approved_for_session,
        })
    }
}

pub(crate) fn register(router: &mut Router, sessions: Arc<DbSessions>) {
    router
        .unary(DbToolConnectionUpsertMethod, {
            let sessions = Arc::clone(&sessions);
            move |ctx, p: Planned<DbToolConnectionUpsertParams>| {
                let s = Arc::clone(&sessions);
                async move { s.tool_connection_upsert(&ctx, p).await }
            }
        })
        .unary(
            DbToolConnectionTestMethod,
            move |ctx, p: Planned<DbToolConnectionTestParams>| {
                let s = Arc::clone(&sessions);
                async move { s.tool_connection_test(&ctx, p).await }
            },
        );
}
