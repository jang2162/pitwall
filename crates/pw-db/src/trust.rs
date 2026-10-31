//! 신뢰 게이트 `db_trust_gate`(M3-02, 결정 M3-D13)와 연결 설정을 드라이버의 `ConnectionSpec` 으로 푸는 자리.
//!
//! - **판정 함수 하나**: [`db_trust_gate`] 가 미신뢰 프로젝트의 연결 · 연결 테스트 · 터널 · 비밀 읽기를 `PERMISSION_DENIED`
//!   (`data.reason = "untrusted"`)로 막는다. 통과하면 [`TrustedProject`](활성 워크트리 포함)를 준다 — 이 값은 이 모듈 밖에서 만들 수
//!   없고, 비밀을 읽어 연결 설정을 푸는 [`Connections::resolve`] · [`Connections::resolve_draft`] 가 그것을 받는다. 두 입구는 받은 값과
//!   별개로 **부를 때마다 신뢰를 다시 본다** — 게이트를 지난 뒤 신뢰를 거두면 들고 있던 값으로도 풀리지 않는다. 드라이버 입력을 만드는
//!   `build_spec` 은 크레이트 밖에 열지 않는다. 연결을 여는 RPC(M3-05 · M3-11 · M3-22)는 이 길을 지난다. 목록(`db.connection.list`)은
//!   신뢰 전에도 돌려준다. 신뢰한 뒤에도 프로젝트를 열 때 저절로 연결하지 않는다 — 이 모듈은 연결을 열지 않는다.
//! - **확인 규칙 하나**: 기억한 확인이 지금 것과 **정확히 같을 때만** 묻지 않는다. 그 밖(처음 · 기록이 깨짐 · 연결을 지웠다 다시 만듦 ·
//!   워크트리 프로젝트의 첫 사용 · 대상이 바뀜)은 모두 묻는다.
//!   - **비밀**: 키체인 항목은 대상이 지금 설정과 같을 때만 쓴다([`crate::secret`]). 아니면 [`code::SECRET_REQUIRED`](지금 대상과, 바뀐
//!     것이면 저장할 때의 대상을 싣는다).
//!   - **`env` 참조**: 사람이 확인한 (변수 이름, 대상)을 프로젝트 상태 `$DATA/projects/<id>/db.json` 에 기억하고, 그것과 지금이 다르면 —
//!     처음 쓸 때도 — 변수를 읽기 전에 [`code::APPROVAL_REQUIRED`] 로 묻는다. 확인하면 창이 `db.connection.approve` 로 기록한다.
//!   - **루트 밖 SQLite**: `path` 의 실제 경로(링크를 끝까지 푼 것)가 프로젝트 루트(활성 워크트리 안의 루트와 주 루트) 밖이면 확인을 받고
//!     (프로젝트, 실제 경로)를 같은 파일에 기억한다. 드라이버에는 판정한 **실제 경로**를 넘긴다 — 판정 뒤 링크를 바꿔 끼워도 여는 파일이
//!     바뀌지 않는다. 확인한 경로는 `ConnectionSpec::allowed_roots` 에도 들어간다(`ATTACH` 판정, M3-08).
//!
//! 드라이버 입력은 판정을 지나야만 얻는다 — 크레이트 밖에서는 `build_spec` 을 부를 수 없고:
//!
//! ```compile_fail
//! let _ = pw_db::trust::build_spec;
//! ```
//!
//! 게이트를 지나지 않은 [`TrustedProject`] 도 만들 수 없다:
//!
//! ```compile_fail
//! fn forge(project: pw_core::registry::Project) -> pw_db::trust::TrustedProject {
//!     pw_db::trust::TrustedProject { project, worktree: None, root: std::path::PathBuf::new() }
//! }
//! ```
//!
//! - **기록 파일**(`db.json`)은 창 상태(`state.json`, UI 가 통째로 쓴다)와 쓰는 길을 섞지 않는다. 프로젝트 하나에 하나다 — 워크트리를
//!   바꿔도 확인이 따라가지만, 확인은 (변수, 대상)에 묶이므로 워크트리마다 대상이 다르면 다시 묻는다. 깨진 기록은 「확인 없음」 으로 읽고
//!   (모두 다시 묻는다) 덮어쓰지 않으며, 목록의 `stateProblem` 과 쓰기의 `STORAGE_UNAVAILABLE` 로 알린다.

use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use pw_core::registry::path::{WorktreeRootError, simplify, worktree_root};
use pw_core::registry::state::{PROJECTS_DIR, valid_id};
use pw_core::registry::{Project, Registry};
use pw_db_types::spec::path_within;
use pw_db_types::{ConnectionSpec, Secret, Target, TlsSettings};
use pw_rpc::{ConnectionScope, Router, RpcError, code as rpc_code, rpc_method};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::config::{
    Connections, DbConnectionConfig, DbConnectionState, DbPasswordRef, Loaded, check_connection_id,
    lock, not_found, registry_error, require_project_window, validate, write_file,
};
use crate::secret::{DbSecretKey, DbSecretTarget, SecretMissing, Stored};

/// DB 설정 RPC 의 애플리케이션 오류 코드(`pw_rpc::code` 의 범위 밖, `data` 는 객체). 실행 관문(M3-05)의 `3001` 과 겹치지 않게
/// 31xx 를 쓴다.
pub mod code {
    /// 비밀이 필요하다 — 키체인에 없거나 저장할 때의 대상과 지금 설정이 다르다(`data`: `connectionId` · `key` · `reason`
    /// (`missing` · `targetChanged` · `unavailable`) · `target`(지금 대상) · `previous`(대상이 바뀐 것이면 저장할 때의 대상, 아니면
    /// `null`)). 창은 이전 → 지금 대상을 보이고 비밀번호를 물어 `db.secret.set` 한 뒤 다시 부른다.
    pub const SECRET_REQUIRED: i32 = 3101;
    /// 사람의 확인이 필요하다(`data.request` 가 [`super::DbApprovalRequest`]). 창은 그것을 보여 주고 `db.connection.approve`
    /// 한 뒤 다시 부른다.
    pub const APPROVAL_REQUIRED: i32 = 3102;
}

/// 확인 기록 파일(`$DATA/projects/<id>/` 안).
pub const APPROVALS_FILE: &str = "db.json";
/// 기록 파일의 크기 상한(바이트). 넘으면 깨진 기록과 같다.
const APPROVALS_LIMIT: u64 = 1024 * 1024;

/// [`db_trust_gate`] 를 지난 프로젝트와 그 활성 워크트리. 이 모듈 밖에서는 만들 수 없다. 들고 있어도 신뢰가 이어진다는 뜻은 아니다 —
/// 푸는 입구가 부를 때마다 다시 본다.
#[derive(Debug, Clone)]
pub struct TrustedProject {
    project: Project,
    worktree: Option<String>,
    root: PathBuf,
}

impl TrustedProject {
    pub fn id(&self) -> &str {
        &self.project.id
    }

    pub fn project(&self) -> &Project {
        &self.project
    }

    /// 활성 워크트리의 꼭대기(창이 준 그대로). 없으면 주 루트.
    pub fn worktree(&self) -> Option<&str> {
        self.worktree.as_deref()
    }

    /// 활성 워크트리 안의 프로젝트 루트(없으면 주 루트).
    pub fn root(&self) -> &Path {
        &self.root
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

/// 활성 워크트리(`ActiveWorktree::top`) 안의 프로젝트 루트. 없으면 주 루트. 다른 프로젝트 RPC(`fs.*` · `openWith.*` 등)와 같은 계산이다.
pub(crate) fn active_root(project: &Project, worktree: Option<&str>) -> Result<PathBuf, RpcError> {
    let Some(top) = worktree else {
        return Ok(project.root().to_path_buf());
    };
    worktree_root(project.root(), Path::new(top)).map_err(|e| match e {
        WorktreeRootError::Relative => {
            RpcError::invalid_params(format!("worktree `{top}` must be an absolute path"))
        }
        WorktreeRootError::NotOurs => RpcError::invalid_params(format!(
            "`{top}` is not a worktree of project `{}`",
            project.id
        )),
    })
}

/// 신뢰 게이트(결정 M3-D13): 연결 · 연결 테스트 · 터널 · 비밀 읽기 앞의 판정 하나. 모르는 프로젝트는 not found, 신뢰하지 않은
/// 프로젝트는 [`untrusted`], 그 프로젝트의 것이 아닌 워크트리는 invalid params. 워크트리 프로젝트는 메인 저장소 프로젝트의 신뢰를 따른다
/// (`Registry::is_trusted`).
pub fn db_trust_gate(
    projects: &Registry,
    project_id: &str,
    worktree: Option<&str>,
) -> Result<TrustedProject, RpcError> {
    let project = projects.get(project_id).map_err(registry_error)?;
    if !projects.is_trusted(project_id) {
        tracing::info!(target: "pw_db", project = project_id, "db access refused: the project is not trusted");
        return Err(untrusted(project_id));
    }
    let root = active_root(&project, worktree)?;
    Ok(TrustedProject {
        project,
        worktree: worktree.map(str::to_owned),
        root,
    })
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
    /// 환경 변수 `variable` 의 값을 `target` 으로 보낸다. `previous` 는 전에 확인한 (변수, 대상) — 처음(기록 없음)이면 `null`.
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
        target: Box<DbSecretTarget>,
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
    /// 활성 워크트리의 꼭대기(`ActiveWorktree.top`, 절대 경로). 공유 설정을 그 워크트리에서 읽는다.
    #[serde(default)]
    pub worktree: Option<String>,
    pub connection_id: String,
    pub approval: DbApproval,
}

rpc_method! {
    /// 사람이 확인한 것을 기록한다 — `env` 참조의 (변수, 대상), 루트 밖 SQLite 의 실제 경로. 지금 설정에서 나오는 것과 같을 때만
    /// 기록하고 다르면 conflict 오류(그 사이 설정이 바뀌었다). 확인 기록이 깨져 있으면 덮지 않고 storage unavailable. 그 프로젝트 창의
    /// 연결 + 신뢰한 프로젝트만.
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
pub(crate) struct Approvals {
    /// 연결 id → 확인한 (변수, 대상).
    #[serde(default)]
    pub env: BTreeMap<String, DbEnvBinding>,
    /// 확인한 루트 밖 실제 경로.
    #[serde(default)]
    pub files: Vec<String>,
    /// 연결 id → 키체인에 넣은 비밀번호의 대상(사본 — 정본은 키체인 항목). 목록이 키체인을 읽지 않고 상태를 정한다.
    #[serde(default)]
    pub secrets: BTreeMap<String, DbSecretTarget>,
}

/// 드라이버에 줄 연결과 그 바탕.
#[derive(Debug, Clone)]
pub struct ResolvedConnection {
    /// 초안이면 `None`.
    pub connection_id: Option<String>,
    /// 푼 설정(비밀 없음).
    pub config: DbConnectionConfig,
    /// 드라이버 입력. 비밀이 들어 있다 — 로그 · 응답으로 내보내지 않는다(`Debug` 는 가린다).
    pub spec: ConnectionSpec,
    /// 터널 목적지(M3-11 이 `spec` 의 `connect_to` 를 채운다).
    pub tunnel: Option<String>,
}

/// 풀지 못한 이유 — RPC 오류로 바꿔 창에 돌려준다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Blocked {
    Secret {
        reason: SecretMissing,
        target: Box<DbSecretTarget>,
        previous: Option<Box<DbSecretTarget>>,
    },
    Approval(DbApprovalRequest),
}

impl Blocked {
    fn into_error(self, connection_id: Option<&str>) -> RpcError {
        let name = connection_id.unwrap_or("(draft)");
        match self {
            Blocked::Secret {
                reason,
                target,
                previous,
            } => RpcError::new(
                code::SECRET_REQUIRED,
                format!("db connection `{name}` needs its password"),
            )
            .with_data(json!({
                "connectionId": connection_id,
                "key": DbSecretKey::Password,
                "reason": reason,
                "target": target,
                "previous": previous,
            })),
            Blocked::Approval(request) => RpcError::new(
                code::APPROVAL_REQUIRED,
                format!("db connection `{name}` needs your approval"),
            )
            .with_data(json!({ "connectionId": connection_id, "request": request })),
        }
    }
}

/// 판정을 지난 것. `build_spec` 의 입력이다.
pub(crate) struct Checked {
    password: Option<Secret>,
    allowed_roots: Vec<PathBuf>,
    /// SQLite 의 실제 경로(판정한 그 경로).
    file: Option<PathBuf>,
}

fn approvals_file(data: &Path, project_id: &str) -> Option<PathBuf> {
    valid_id(project_id).then(|| {
        data.join(PROJECTS_DIR)
            .join(project_id)
            .join(APPROVALS_FILE)
    })
}

/// 기록을 읽는다. 없으면 빈 기록, 읽을 수 없거나(크기 상한 초과 포함) 깨졌으면 그 이유.
fn read_approvals(file: &Path) -> Result<Approvals, String> {
    let f = match fs::File::open(file) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Approvals::default()),
        Err(e) => return Err(format!("cannot read {}: {e}", file.display())),
    };
    let mut text = String::new();
    f.take(APPROVALS_LIMIT + 1)
        .read_to_string(&mut text)
        .map_err(|e| format!("cannot read {}: {e}", file.display()))?;
    if text.len() as u64 > APPROVALS_LIMIT {
        return Err(format!(
            "{} is larger than {APPROVALS_LIMIT} bytes",
            file.display()
        ));
    }
    serde_json::from_str(&text).map_err(|e| format!("{} is unreadable: {e}", file.display()))
}

/// 실제 경로(링크를 끝까지 푼 것). 없는 파일은 연결할 수 없다 — 드라이버도 파일을 만들지 않는다. Windows 에서 `canonicalize` 가
/// 붙이는 verbatim 접두(`\\?\`)는 걷는다 — 확인 요청(`realPath`)으로 사람에게 보이고 프로젝트 상태에 저장하는 경로다
/// (`pw_core::registry::path` 머리 주석의 규칙).
fn real_path(path: &Path) -> Result<PathBuf, RpcError> {
    fs::canonicalize(path).map(simplify).map_err(|e| {
        RpcError::invalid_params(format!(
            "cannot open the sqlite file {}: {e}",
            path.display()
        ))
    })
}

impl Connections {
    /// [`db_trust_gate`] 를 이 Host 의 레지스트리로.
    pub fn trust_gate(
        &self,
        project_id: &str,
        worktree: Option<&str>,
    ) -> Result<TrustedProject, RpcError> {
        db_trust_gate(&self.host()?.projects, project_id, worktree)
    }

    /// 들고 온 [`TrustedProject`] 가 아직 신뢰받는가 — 게이트를 지난 뒤 신뢰를 거뒀으면 거절한다.
    fn still_trusted(&self, trusted: &TrustedProject) -> Result<(), RpcError> {
        if self.trusted(trusted.id()) {
            Ok(())
        } else {
            tracing::info!(target: "pw_db", project = trusted.id(), "db access refused: trust was revoked");
            Err(untrusted(trusted.id()))
        }
    }

    fn approvals_path(&self, project_id: &str) -> Result<PathBuf, RpcError> {
        approvals_file(&self.host()?.paths.data, project_id)
            .ok_or_else(|| RpcError::invalid_params(format!("`{project_id}` is not a project id")))
    }

    /// 판정에 쓰는 기록과, 깨졌으면 그 이유. 깨진 기록은 빈 기록(「확인 없음」 — 모두 다시 묻는다)으로 쓴다.
    pub(crate) fn approvals(
        &self,
        project_id: &str,
    ) -> Result<(Approvals, Option<String>), RpcError> {
        let file = self.approvals_path(project_id)?;
        Ok(match read_approvals(&file) {
            Ok(approvals) => (approvals, None),
            Err(problem) => {
                tracing::warn!(target: "pw_db", project = project_id, %problem, "unreadable db approvals; asking for everything again");
                (Approvals::default(), Some(problem))
            }
        })
    }

    /// 쓰기 전에 기록을 읽는다. 깨졌으면 덮지 않고 `STORAGE_UNAVAILABLE`.
    pub(crate) fn approvals_for_write(&self, project_id: &str) -> Result<Approvals, RpcError> {
        let file = self.approvals_path(project_id)?;
        read_approvals(&file).map_err(|problem| {
            RpcError::new(
                rpc_code::STORAGE_UNAVAILABLE,
                "the db approvals file is unreadable; fix or remove it",
            )
            .with_data(json!({ "path": file, "reason": problem }))
        })
    }

    /// 기록을 고쳐 쓴다. 부르는 쪽이 `writing` 잠금을 쥔다. 깨진 기록은 덮지 않는다.
    pub(crate) fn update_approvals(
        &self,
        project_id: &str,
        change: impl FnOnce(&mut Approvals),
    ) -> Result<(), RpcError> {
        let file = self.approvals_path(project_id)?;
        let mut approvals = self.approvals_for_write(project_id)?;
        let before = approvals.clone();
        change(&mut approvals);
        if approvals == before {
            return Ok(());
        }
        let text = serde_json::to_string_pretty(&approvals)
            .map_err(|e| RpcError::internal(e.to_string()))?;
        write_file(&file, &text)
    }

    /// 연결을 지울 때 그 연결의 비밀 · `env` 확인 · 비밀 대상 사본을 지운다(경로 확인은 프로젝트 단위라 둔다). 기록이 깨졌어도 키체인은
    /// 지운다.
    pub(crate) fn forget(&self, project_id: &str, connection_id: &str) -> Result<(), RpcError> {
        self.clear_all_secrets(project_id, connection_id);
        self.update_approvals(project_id, |a| {
            a.env.remove(connection_id);
            a.secrets.remove(connection_id);
        })
    }

    /// SQLite 파일의 실제 경로, 실제 경로로 푼 프로젝트의 루트들(활성 워크트리 안의 루트 · 주 루트), 그리고 그 밖인가.
    fn sqlite_path(
        loaded: &Loaded,
        config: &DbConnectionConfig,
    ) -> Result<(PathBuf, Vec<PathBuf>, bool), RpcError> {
        let path = Path::new(config.path.as_deref().unwrap_or_default());
        let joined = if path.is_absolute() {
            path.to_path_buf()
        } else {
            loaded.root.join(path)
        };
        let real = real_path(&joined)?;
        let mut roots = vec![real_path(&loaded.root)?];
        if let Ok(main) = real_path(loaded.project.root())
            && !roots.contains(&main)
        {
            roots.push(main);
        }
        // 걷지 못한 긴 경로는 verbatim 으로 남는다 — 접두의 꼴과 상관없이 견준다.
        let outside = !roots.iter().any(|root| path_within(&real, root));
        Ok((real, roots, outside))
    }

    /// 열 수 있는가 — 값을 읽지 않고(목록의 상태). 신뢰한 프로젝트에서만 부른다. 키체인도 읽지 않는다(대상 사본으로 정한다).
    pub(crate) fn readiness(
        &self,
        loaded: &Loaded,
        connection_id: &str,
        config: &DbConnectionConfig,
        approvals: &Approvals,
    ) -> DbConnectionState {
        match self.check(loaded, Some(connection_id), config, approvals, false, None) {
            Ok(_) => DbConnectionState::Ready,
            Err(Ok(Blocked::Secret {
                reason,
                target,
                previous,
            })) => DbConnectionState::SecretRequired {
                reason,
                target,
                previous,
            },
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

    /// 판정과 (`read` 면) 값 읽기. 막히면 `Err(Ok(이유))`, 다른 실패는 `Err(Err(오류))`. `connection_id` 가 없으면 초안이다 —
    /// 기억한 확인 · 저장한 비밀이 없으므로 `draft_secret` 이 없는 비밀은 묻고, `env` 참조는 늘 묻는다(저장한 뒤 확인한다).
    fn check(
        &self,
        loaded: &Loaded,
        connection_id: Option<&str>,
        config: &DbConnectionConfig,
        approvals: &Approvals,
        read: bool,
        draft_secret: Option<Secret>,
    ) -> Result<Checked, Result<Blocked, RpcError>> {
        let project = &loaded.project;
        let mut allowed_roots = Vec::new();
        let mut file = None;
        if config.path.is_some() {
            let (real, roots, outside) = Self::sqlite_path(loaded, config).map_err(Err)?;
            let real_text = real.to_string_lossy().into_owned();
            if outside && !approvals.files.contains(&real_text) {
                return Err(Ok(Blocked::Approval(DbApprovalRequest::File {
                    real_path: real_text,
                })));
            }
            allowed_roots = roots;
            if outside {
                allowed_roots.push(real.clone());
            }
            // 확인한 다른 루트 밖 경로도 붙일 수 있다(`ATTACH` — 판정은 드라이버의 `file_allowed`).
            for approved in approvals.files.iter().map(PathBuf::from) {
                if !allowed_roots.contains(&approved) {
                    allowed_roots.push(approved);
                }
            }
            file = Some(real);
        }
        let target = DbSecretTarget::of(config);
        let password = match &config.password {
            None => draft_secret,
            Some(DbPasswordRef::Keychain) => match (draft_secret, connection_id) {
                (Some(secret), _) => read.then_some(secret),
                (None, None) => {
                    return Err(Ok(Blocked::Secret {
                        reason: SecretMissing::Missing,
                        target: Box::new(target),
                        previous: None,
                    }));
                }
                (None, Some(id)) if !read => {
                    // 목록: 키체인을 읽지 않고 저장할 때의 대상 사본으로 정한다.
                    match approvals.secrets.get(id) {
                        Some(saved) if *saved == target => None,
                        saved => {
                            return Err(Ok(Blocked::Secret {
                                reason: if saved.is_some() {
                                    SecretMissing::TargetChanged
                                } else {
                                    SecretMissing::Missing
                                },
                                target: Box::new(target),
                                previous: saved.cloned().map(Box::new),
                            }));
                        }
                    }
                }
                (None, Some(id)) => {
                    match self.stored_secret(&project.id, id, DbSecretKey::Password, config) {
                        Stored::Usable(secret) => Some(secret),
                        Stored::Unusable(reason, previous) => {
                            return Err(Ok(Blocked::Secret {
                                reason,
                                target: Box::new(target),
                                previous: previous.map(Box::new),
                            }));
                        }
                    }
                }
            },
            Some(DbPasswordRef::Env { variable }) => {
                let now = DbEnvBinding {
                    variable: variable.clone(),
                    target,
                };
                // 기억한 확인이 지금과 정확히 같을 때만 묻지 않는다 — 처음(기록 없음)도 묻는다.
                let previous = connection_id.and_then(|id| approvals.env.get(id));
                if previous != Some(&now) {
                    return Err(Ok(Blocked::Approval(DbApprovalRequest::Env {
                        variable: now.variable,
                        target: Box::new(now.target),
                        previous: previous.cloned().map(Box::new),
                    })));
                }
                if read {
                    match self.env.var(&project.id, &loaded.root, variable) {
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
        Ok(Checked {
            password,
            allowed_roots,
            file,
        })
    }

    /// 저장한 연결을 드라이버 입력으로 푼다. 신뢰 게이트를 지난 프로젝트만 받고, 부를 때 신뢰를 다시 본다. 비밀이 없거나 대상이
    /// 바뀌었으면 [`code::SECRET_REQUIRED`], 확인이 필요하면 [`code::APPROVAL_REQUIRED`] — 그때는 비밀 · 변수 값을 읽지 않는다. 블로킹이다
    /// (키체인 · 런타임 환경 — 로그인 셸을 기다릴 수 있다) — async 쪽은 `spawn_blocking` 에서 부른다.
    pub fn resolve(
        &self,
        trusted: &TrustedProject,
        connection_id: &str,
    ) -> Result<ResolvedConnection, RpcError> {
        self.still_trusted(trusted)?;
        check_connection_id(connection_id)?;
        let loaded = self.load(trusted.id(), trusted.worktree())?;
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
        self.resolve_with(&loaded, Some(connection_id), config, None)
    }

    /// 저장하지 않은 초안(연결 테스트 — M3-05 의 `db.connection.test { draft, secret }`)을 드라이버 입력으로 푼다. 저장한 연결과 같은
    /// 판정(신뢰 재확인 · 루트 밖 SQLite 확인 · `env` 확인)을 지난다. `secret` 은 이 요청에만 쓰고 저장하지 않는다. `connection_id` 가
    /// 있으면(저장한 연결을 고치는 중) 그 연결의 기억한 확인 · 저장한 비밀을 쓴다 — 대상이 같을 때만.
    pub fn resolve_draft(
        &self,
        trusted: &TrustedProject,
        connection_id: Option<&str>,
        draft: &DbConnectionConfig,
        secret: Option<Secret>,
    ) -> Result<ResolvedConnection, RpcError> {
        self.still_trusted(trusted)?;
        if let Some(id) = connection_id {
            check_connection_id(id)?;
        }
        validate(draft).map_err(RpcError::invalid_params)?;
        let loaded = self.load(trusted.id(), trusted.worktree())?;
        self.resolve_with(&loaded, connection_id, draft.clone(), secret)
    }

    fn resolve_with(
        &self,
        loaded: &Loaded,
        connection_id: Option<&str>,
        config: DbConnectionConfig,
        draft_secret: Option<Secret>,
    ) -> Result<ResolvedConnection, RpcError> {
        let (approvals, _) = self.approvals(&loaded.project.id)?;
        let checked = self
            .check(
                loaded,
                connection_id,
                &config,
                &approvals,
                true,
                draft_secret,
            )
            .map_err(|blocked| match blocked {
                Ok(blocked) => blocked.into_error(connection_id),
                Err(e) => e,
            })?;
        let spec = build_spec(&config, &loaded.root, checked);
        Ok(ResolvedConnection {
            connection_id: connection_id.map(str::to_owned),
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
        let trusted = self.trust_gate(&params.project_id, params.worktree.as_deref())?;
        check_connection_id(&params.connection_id)?;
        let _writing = lock(&self.writing);
        let loaded = self.load(trusted.id(), trusted.worktree())?;
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
                if now.variable != *variable || now.target != **target {
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
                let (real, _, _) = Self::sqlite_path(&loaded, config)?;
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

/// 연결 설정 → 드라이버 입력. 판정(`Connections::check`)을 지난 값으로만 만든다 — 크레이트 밖에 열지 않는다. SQLite 는 판정한 실제
/// 경로를 연다. 상대 인증서 경로는 활성 워크트리 안의 루트 기준. 터널은 아직 열지 않았다(`connect_to` 는 M3-11 이 채운다).
fn build_spec(config: &DbConnectionConfig, root: &Path, checked: Checked) -> ConnectionSpec {
    let target = match checked.file {
        Some(path) => Target::File { path },
        None => Target::Network {
            host: config.host.clone().unwrap_or_default(),
            port: config.effective_port().unwrap_or_default(),
            connect_to: None,
        },
    };
    let mut spec = ConnectionSpec::new(config.driver.id(), target);
    spec.database = config.database.clone();
    spec.user = config.user.clone();
    spec.password = checked.password;
    spec.tls = TlsSettings {
        mode: config.effective_ssl_mode(),
        root_cert: config.sslrootcert.as_ref().map(|p| root.join(p)),
        client_cert: config.sslcert.as_ref().map(|p| root.join(p)),
        client_key: config.sslkey.as_ref().map(|p| root.join(p)),
    };
    spec.read_only = config.read_only;
    spec.allowed_roots = checked.allowed_roots;
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
