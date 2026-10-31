//! 신뢰 게이트 `db_trust_gate`(M3-02, 결정 M3-D13)와 연결 설정을 드라이버의 `ConnectionSpec` 으로 푸는 자리.
//!
//! - **판정 함수 하나**: [`db_trust_gate`] 가 미신뢰 프로젝트의 연결 · 연결 테스트 · 터널 · 비밀 읽기를 `PERMISSION_DENIED`
//!   (`data.reason = "untrusted"`)로 막는다. 통과하면 [`TrustedProject`] 를 준다 — 이 값은 이 모듈 밖에서 만들 수 없고, 비밀을
//!   읽어 연결 설정을 푸는 [`Connections::resolve`] 가 그것을 받는다. 연결을 여는 RPC(M3-05 · M3-11 · M3-22)는 이 길을 지난다.
//!   목록(`db.connection.list`)은 신뢰 전에도 돌려준다. 신뢰한 뒤에도 프로젝트를 열 때 저절로 연결하지 않는다 — 이 모듈은 연결을
//!   열지 않고, 연결은 사람이 누른 RPC 로만 열린다.
//! - **비밀**: 키체인 항목은 대상이 지금 설정과 같을 때만 쓴다([`crate::secret`]). 아니면 [`code::SECRET_REQUIRED`].
//! - **`env` 참조**: 처음 쓸 때의 (변수 이름, 대상)을 프로젝트 상태 `$DATA/projects/<id>/db.json` 에 기억하고, 둘 중 하나라도
//!   바뀌면 변수를 읽기 전에 [`code::APPROVAL_REQUIRED`] 로 묻는다. 사람이 확인하면 창이 `db.connection.approve` 로 기록한다.
//! - **루트 밖 SQLite**: `path` 의 실제 경로(링크를 끝까지 푼 것)가 프로젝트 루트 밖이면 처음 열 때 확인을 받고 (프로젝트, 실제
//!   경로)를 같은 파일에 기억한다. 확인한 경로는 드라이버의 `ConnectionSpec::allowed_roots` 에도 들어간다(`ATTACH` 판정, M3-08).
//! - 기록 파일(`db.json`)은 창 상태(`state.json`, UI 가 통째로 쓴다)와 쓰는 길을 섞지 않는다.

use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use pw_core::registry::state::{PROJECTS_DIR, valid_id};
use pw_core::registry::{Project, Registry};
use pw_db_types::{ConnectionSpec, Secret, Target, TlsSettings};
use pw_rpc::{ConnectionScope, Router, RpcError, code as rpc_code, rpc_method};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::config::{
    Connections, DbConnectionConfig, DbConnectionState, DbPasswordRef, check_connection_id, lock,
    not_found, registry_error, require_project_window, write_file,
};
use crate::secret::{DbSecretKey, DbSecretTarget, SecretMissing, Stored};

/// DB 설정 RPC 의 애플리케이션 오류 코드(`pw_rpc::code` 의 범위 밖, `data` 는 객체). 실행 관문(M3-05)의 `3001` 과 겹치지 않게
/// 31xx 를 쓴다.
pub mod code {
    /// 비밀이 필요하다 — 키체인에 없거나 저장할 때의 대상과 지금 설정이 다르다(`data.reason`: `missing` · `targetChanged` ·
    /// `unavailable`, `data.key`). 창은 비밀번호를 물어 `db.secret.set` 한 뒤 다시 부른다.
    pub const SECRET_REQUIRED: i32 = 3101;
    /// 사람의 확인이 필요하다(`data.request` 가 [`super::DbApprovalRequest`]). 창은 그것을 보여 주고 `db.connection.approve`
    /// 한 뒤 다시 부른다.
    pub const APPROVAL_REQUIRED: i32 = 3102;
}

/// 확인 기록 파일(`$DATA/projects/<id>/` 안).
pub const APPROVALS_FILE: &str = "db.json";
/// 기록 파일의 크기 상한(바이트).
const APPROVALS_LIMIT: u64 = 1024 * 1024;

/// [`db_trust_gate`] 를 지난 프로젝트. 이 모듈 밖에서는 만들 수 없다.
#[derive(Debug, Clone)]
pub struct TrustedProject {
    project: Project,
}

impl TrustedProject {
    pub fn id(&self) -> &str {
        &self.project.id
    }

    pub fn project(&self) -> &Project {
        &self.project
    }
}

/// 미신뢰 프로젝트를 거절하는 오류(`PERMISSION_DENIED`, `data.reason = "untrusted"`).
pub fn untrusted(project_id: &str) -> RpcError {
    RpcError::new(
        rpc_code::PERMISSION_DENIED,
        "trust the project before opening its db connections",
    )
    .with_data(json!({ "reason": "untrusted", "projectId": project_id }))
}

/// 신뢰 게이트(결정 M3-D13): 연결 · 연결 테스트 · 터널 · 비밀 읽기 앞의 판정 하나. 모르는 프로젝트는 not found, 신뢰하지 않은
/// 프로젝트는 [`untrusted`]. 워크트리 프로젝트는 메인 저장소 프로젝트의 신뢰를 따른다(`Registry::is_trusted`).
pub fn db_trust_gate(projects: &Registry, project_id: &str) -> Result<TrustedProject, RpcError> {
    let project = projects.get(project_id).map_err(registry_error)?;
    if !projects.is_trusted(project_id) {
        tracing::info!(target: "pw_db", project = project_id, "db access refused: the project is not trusted");
        return Err(untrusted(project_id));
    }
    Ok(TrustedProject { project })
}

/// 확인을 받아야 열 수 있는 것.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum DbApprovalRequest {
    /// 환경 변수 `variable` 의 값을 `target` 으로 보낸다. `previous` 는 전에 확인한 (변수, 대상) — 처음이면 `null`.
    Env {
        variable: String,
        target: Box<DbSecretTarget>,
        previous: Option<Box<DbEnvBinding>>,
    },
    /// 프로젝트 루트 밖의 SQLite 파일(링크를 끝까지 푼 실제 경로)을 연다.
    File { real_path: String },
}

/// 확인한 것(창이 보여 준 그대로 돌려준다). Host 는 지금 설정과 같을 때만 기록한다 — 다르면 conflict.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum DbApproval {
    Env {
        variable: String,
        target: DbSecretTarget,
    },
    File {
        real_path: String,
    },
}

/// 확인한 (변수 이름, 대상).
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DbEnvBinding {
    pub variable: String,
    pub target: DbSecretTarget,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DbConnectionApproveParams {
    pub project_id: String,
    pub connection_id: String,
    pub approval: DbApproval,
}

rpc_method! {
    /// 사람이 확인한 것을 기록한다 — `env` 참조의 (변수, 대상), 루트 밖 SQLite 의 실제 경로. 지금 설정에서 나오는 것과 같을 때만
    /// 기록하고 다르면 conflict 오류(그 사이 설정이 바뀌었다). 그 프로젝트 창의 연결 + 신뢰한 프로젝트만.
    pub DbConnectionApprove: "db.connection.approve" {
        params: DbConnectionApproveParams,
        result: (),
        tier: Write,
        roles: [Owner],
    }
}

/// 프로젝트 상태의 확인 기록(`db.json`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Approvals {
    /// 연결 id → 확인한 (변수, 대상).
    #[serde(default)]
    env: BTreeMap<String, DbEnvBinding>,
    /// 확인한 루트 밖 실제 경로.
    #[serde(default)]
    files: Vec<String>,
}

/// 드라이버에 줄 연결과 그 바탕.
#[derive(Debug, Clone)]
pub struct ResolvedConnection {
    pub connection_id: String,
    /// 푼 설정(비밀 없음).
    pub config: DbConnectionConfig,
    /// 드라이버 입력. 비밀이 들어 있다 — 로그 · 응답으로 내보내지 않는다(`Debug` 는 가린다).
    pub spec: ConnectionSpec,
    /// 터널 목적지(M3-11 이 `spec` 의 `connect_to` 를 채운다).
    pub tunnel: Option<String>,
}

/// 풀지 못한 이유 — RPC 오류로 바꿔 창에 돌려준다.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Blocked {
    Secret(SecretMissing),
    Approval(DbApprovalRequest),
}

impl Blocked {
    fn into_error(self, connection_id: &str) -> RpcError {
        match self {
            Blocked::Secret(reason) => RpcError::new(
                code::SECRET_REQUIRED,
                format!("db connection `{connection_id}` needs its password"),
            )
            .with_data(json!({
                "connectionId": connection_id,
                "key": DbSecretKey::Password,
                "reason": reason,
            })),
            Blocked::Approval(request) => RpcError::new(
                code::APPROVAL_REQUIRED,
                format!("db connection `{connection_id}` needs your approval"),
            )
            .with_data(json!({ "connectionId": connection_id, "request": request })),
        }
    }
}

fn approvals_file(data: &Path, project_id: &str) -> Option<PathBuf> {
    valid_id(project_id).then(|| {
        data.join(PROJECTS_DIR)
            .join(project_id)
            .join(APPROVALS_FILE)
    })
}

/// 기록을 읽는다. 없거나 깨졌으면 빈 기록(다시 묻는다 — 안전한 쪽).
fn read_approvals(file: &Path) -> Approvals {
    let Ok(f) = fs::File::open(file) else {
        return Approvals::default();
    };
    let mut text = String::new();
    if f.take(APPROVALS_LIMIT).read_to_string(&mut text).is_err() {
        return Approvals::default();
    }
    serde_json::from_str(&text).unwrap_or_else(|e| {
        tracing::warn!(target: "pw_db", path = %file.display(), %e, "unreadable db approvals; asking again");
        Approvals::default()
    })
}

/// 실제 경로(링크를 끝까지 푼 것). 없는 파일은 연결할 수 없다 — 드라이버도 파일을 만들지 않는다.
fn real_path(path: &Path) -> Result<PathBuf, RpcError> {
    fs::canonicalize(path).map_err(|e| {
        RpcError::invalid_params(format!(
            "cannot open the sqlite file {}: {e}",
            path.display()
        ))
    })
}

impl Connections {
    /// [`db_trust_gate`] 를 이 Host 의 레지스트리로.
    pub fn trust_gate(&self, project_id: &str) -> Result<TrustedProject, RpcError> {
        db_trust_gate(&self.host()?.projects, project_id)
    }

    fn approvals_path(&self, project_id: &str) -> Result<PathBuf, RpcError> {
        approvals_file(&self.host()?.paths.data, project_id)
            .ok_or_else(|| RpcError::invalid_params(format!("`{project_id}` is not a project id")))
    }

    fn approvals(&self, project_id: &str) -> Result<Approvals, RpcError> {
        Ok(read_approvals(&self.approvals_path(project_id)?))
    }

    /// 기록을 고쳐 쓴다(설정 쓰기와 같은 줄에서).
    fn update_approvals(
        &self,
        project_id: &str,
        change: impl FnOnce(&mut Approvals),
    ) -> Result<(), RpcError> {
        let file = self.approvals_path(project_id)?;
        let mut approvals = read_approvals(&file);
        let before = approvals.clone();
        change(&mut approvals);
        if approvals == before {
            return Ok(());
        }
        let text = serde_json::to_string_pretty(&approvals)
            .map_err(|e| RpcError::internal(e.to_string()))?;
        write_file(&file, &text)
    }

    /// 연결을 지울 때 그 연결의 비밀 · `env` 확인을 지운다(경로 확인은 프로젝트 단위라 둔다).
    pub(crate) fn forget(&self, project_id: &str, connection_id: &str) -> Result<(), RpcError> {
        self.clear_all_secrets(project_id, connection_id);
        self.update_approvals(project_id, |a| {
            a.env.remove(connection_id);
        })
    }

    /// SQLite 파일의 실제 경로와 그것이 루트 밖인가.
    fn sqlite_path(
        project: &Project,
        config: &DbConnectionConfig,
    ) -> Result<(PathBuf, PathBuf, bool), RpcError> {
        let path = Path::new(config.path.as_deref().unwrap_or_default());
        let joined = if path.is_absolute() {
            path.to_path_buf()
        } else {
            project.root().join(path)
        };
        let real = real_path(&joined)?;
        let root = real_path(project.root())?;
        let outside = !real.starts_with(&root);
        Ok((real, root, outside))
    }

    /// 열 수 있는가 — 값을 읽지 않고(목록의 상태). 신뢰한 프로젝트에서만 부른다.
    pub(crate) fn readiness(
        &self,
        project: &Project,
        connection_id: &str,
        config: &DbConnectionConfig,
    ) -> DbConnectionState {
        match self.check(project, connection_id, config, false) {
            Ok(_) => DbConnectionState::Ready,
            Err(Ok(Blocked::Secret(reason))) => DbConnectionState::SecretRequired { reason },
            Err(Ok(Blocked::Approval(request))) => DbConnectionState::ApprovalRequired { request },
            Err(Err(e)) => DbConnectionState::Invalid {
                message: e
                    .data
                    .as_ref()
                    .and_then(|d| d.as_str())
                    .map_or_else(|| e.message.clone(), str::to_owned),
            },
        }
    }

    /// 판정과 (`read` 면) 값 읽기. 막히면 `Err(Ok(이유))`, 다른 실패는 `Err(Err(오류))`.
    fn check(
        &self,
        project: &Project,
        connection_id: &str,
        config: &DbConnectionConfig,
        read: bool,
    ) -> Result<(Option<Secret>, Vec<PathBuf>), Result<Blocked, RpcError>> {
        let approvals = self.approvals(&project.id).map_err(Err)?;
        let mut allowed_roots = Vec::new();
        if config.path.is_some() {
            let (real, root, outside) = Self::sqlite_path(project, config).map_err(Err)?;
            allowed_roots.push(root);
            let real_text = real.to_string_lossy().into_owned();
            let approved = approvals.files.contains(&real_text);
            if outside && !approved {
                return Err(Ok(Blocked::Approval(DbApprovalRequest::File {
                    real_path: real_text,
                })));
            }
            if outside {
                allowed_roots.push(real);
            }
            // 확인한 다른 루트 밖 경로도 붙일 수 있다(`ATTACH` — 판정은 드라이버의 `file_allowed`).
            for approved in approvals.files.iter().map(PathBuf::from) {
                if !allowed_roots.contains(&approved) {
                    allowed_roots.push(approved);
                }
            }
        }
        let password = match &config.password {
            None => None,
            Some(DbPasswordRef::Keychain) => {
                // 목록(`read` 아님)도 항목을 읽어 대상을 본다 — 신뢰한 프로젝트에서만 여기 온다. 값은 돌려주지 않는다.
                match self.stored_secret(&project.id, connection_id, DbSecretKey::Password, config)
                {
                    Stored::Usable(secret) => read.then_some(secret),
                    Stored::Unusable(reason) => return Err(Ok(Blocked::Secret(reason))),
                }
            }
            Some(DbPasswordRef::Env { variable }) => {
                let now = DbEnvBinding {
                    variable: variable.clone(),
                    target: DbSecretTarget::of(config),
                };
                match approvals.env.get(connection_id) {
                    Some(previous) if *previous != now => {
                        return Err(Ok(Blocked::Approval(DbApprovalRequest::Env {
                            variable: now.variable,
                            target: Box::new(now.target),
                            previous: Some(Box::new(previous.clone())),
                        })));
                    }
                    Some(_) => {}
                    None if read => {
                        // 처음 쓸 때의 (변수 이름, 대상)을 기억한다 — 이후 둘 중 하나가 바뀌면 묻는다.
                        let binding = now.clone();
                        let id = connection_id.to_owned();
                        let _writing = lock(&self.writing);
                        self.update_approvals(&project.id, move |a| {
                            a.env.entry(id).or_insert(binding);
                        })
                        .map_err(Err)?;
                        tracing::info!(target: "pw_db", project = %project.id, connection = connection_id, variable = %now.variable, "remembered the env reference of a db connection");
                    }
                    None => {}
                }
                if read {
                    match self.env.var(&project.id, project.root(), variable) {
                        Some(secret) => Some(secret),
                        None => {
                            return Err(Err(RpcError::invalid_params(format!(
                                "environment variable `{variable}` is not set in the project's runtime environment"
                            ))));
                        }
                    }
                } else {
                    None
                }
            }
        };
        Ok((password, allowed_roots))
    }

    /// 저장한 연결을 드라이버 입력으로 푼다. 신뢰 게이트를 지난 프로젝트만 받는다. 비밀이 없거나 대상이 바뀌었으면
    /// [`code::SECRET_REQUIRED`], 확인이 필요하면 [`code::APPROVAL_REQUIRED`] — 그때는 비밀 · 변수 값을 읽지 않는다. 블로킹이다
    /// (키체인 · 런타임 환경 — 로그인 셸을 기다릴 수 있다) — async 쪽은 `spawn_blocking` 에서 부른다.
    pub fn resolve(
        &self,
        trusted: &TrustedProject,
        connection_id: &str,
    ) -> Result<ResolvedConnection, RpcError> {
        check_connection_id(connection_id)?;
        let loaded = self.load(trusted.id())?;
        if let Err(problem) = &loaded.connections {
            return Err(RpcError::internal(format!(
                "cannot read the project config: {problem}"
            )));
        }
        let connection = loaded
            .get(connection_id)
            .ok_or_else(|| not_found(connection_id))?;
        let config = connection.config.clone().map_err(|e| {
            RpcError::invalid_params(format!("db connection `{connection_id}` is invalid: {e}"))
        })?;
        let (password, allowed_roots) = self
            .check(&loaded.project, connection_id, &config, true)
            .map_err(|blocked| match blocked {
            Ok(blocked) => blocked.into_error(connection_id),
            Err(e) => e,
        })?;
        let spec = build_spec(&config, trusted.project().root(), password, allowed_roots);
        Ok(ResolvedConnection {
            connection_id: connection_id.to_owned(),
            tunnel: config.ssh_tunnel.as_ref().map(|t| t.host_ref.clone()),
            config,
            spec,
        })
    }

    /// `db.connection.approve`.
    pub fn approve(
        &self,
        scope: &ConnectionScope,
        params: &DbConnectionApproveParams,
    ) -> Result<(), RpcError> {
        require_project_window(scope, &params.project_id)?;
        let trusted = self.trust_gate(&params.project_id)?;
        check_connection_id(&params.connection_id)?;
        let _writing = lock(&self.writing);
        let loaded = self.load(trusted.id())?;
        let connection = loaded
            .get(&params.connection_id)
            .ok_or_else(|| not_found(&params.connection_id))?;
        let config = connection.config.as_ref().map_err(|e| {
            RpcError::invalid_params(format!(
                "db connection `{}` is invalid: {e}",
                params.connection_id
            ))
        })?;
        let changed = || {
            RpcError::new(
                rpc_code::CONFLICT,
                "the connection config changed since the approval was asked; ask again",
            )
        };
        match &params.approval {
            DbApproval::Env { variable, target } => {
                let now = match &config.password {
                    Some(DbPasswordRef::Env { variable }) => DbEnvBinding {
                        variable: variable.clone(),
                        target: DbSecretTarget::of(config),
                    },
                    _ => return Err(changed()),
                };
                if now.variable != *variable || now.target != *target {
                    return Err(changed());
                }
                let id = params.connection_id.clone();
                self.update_approvals(trusted.id(), move |a| {
                    a.env.insert(id, now);
                })?;
                tracing::info!(target: "pw_db", project = %params.project_id, connection = %params.connection_id, %variable, "approved the env reference of a db connection");
            }
            DbApproval::File { real_path } => {
                if config.path.is_none() {
                    return Err(changed());
                }
                let (real, _, _) = Self::sqlite_path(trusted.project(), config)?;
                let real = real.to_string_lossy().into_owned();
                if real != *real_path {
                    return Err(changed());
                }
                self.update_approvals(trusted.id(), move |a| {
                    if !a.files.contains(&real) {
                        a.files.push(real);
                        a.files.sort();
                    }
                })?;
                tracing::info!(target: "pw_db", project = %params.project_id, path = %real_path, "approved a sqlite file outside the project");
            }
        }
        Ok(())
    }
}

/// 연결 설정 → 드라이버 입력. 터널은 아직 열지 않았다(`connect_to` 는 M3-11 이 채운다).
pub fn build_spec(
    config: &DbConnectionConfig,
    root: &Path,
    password: Option<Secret>,
    allowed_roots: Vec<PathBuf>,
) -> ConnectionSpec {
    let target = match &config.path {
        Some(path) => Target::File {
            path: if Path::new(path).is_absolute() {
                PathBuf::from(path)
            } else {
                root.join(path)
            },
        },
        None => Target::Network {
            host: config.host.clone().unwrap_or_default(),
            port: config.effective_port().unwrap_or_default(),
            connect_to: None,
        },
    };
    let mut spec = ConnectionSpec::new(config.driver.id(), target);
    spec.database = config.database.clone();
    spec.user = config.user.clone();
    spec.password = password;
    spec.tls = TlsSettings {
        mode: config.effective_ssl_mode(),
        root_cert: config.sslrootcert.as_ref().map(|p| root.join(p)),
        client_cert: config.sslcert.as_ref().map(|p| root.join(p)),
        client_key: config.sslkey.as_ref().map(|p| root.join(p)),
    };
    spec.read_only = config.read_only;
    spec.allowed_roots = allowed_roots;
    spec
}

pub(crate) fn register(router: &mut Router, connections: Arc<Connections>) {
    router.unary(
        DbConnectionApprove,
        move |ctx, params: DbConnectionApproveParams| {
            let (c, scope) = (Arc::clone(&connections), ctx.scope().clone());
            async move {
                tokio::task::spawn_blocking(move || c.approve(&scope, &params))
                    .await
                    .map_err(|e| RpcError::internal(e.to_string()))?
            }
        },
    );
}
