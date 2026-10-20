//! 프로젝트 신뢰(F1-7 최소판, M1-05 · 결정 M1-D16). 미신뢰 프로젝트에서는 프로젝트 코드를 실행하는 런타임 제공자(mise 등)를
//! 돌리지 않는다(03-architecture 「런타임 환경 (D26)」 「신뢰」). 막는 쪽은 `pw_runtime::Runtime::with_trust` 이고, 이 모듈은
//! 신뢰를 저장하고 판정한다.
//!
//! - 저장: `state.db` 의 `project_trust`(v2). 행이 없으면 **아직 묻지 않음**, `trusted` 0 · 1 이 미신뢰 · 신뢰다. 묻지 않은
//!   프로젝트도 미신뢰다 — v1 의 기존 프로젝트는 행 없이 올라온다.
//! - **워크트리는 메인 저장소 프로젝트의 신뢰를 따른다.** 등록되지 않은 워크트리는 [`super::resolve`] 가 이미 메인 저장소
//!   프로젝트로 연다. 워크트리 자체를 프로젝트로 등록했으면 그 루트를 메인 체크아웃 쪽으로 옮겨 그곳을 포함하는 등록 루트(가장
//!   깊은 것, 메인 체크아웃 안의 것만 — 해석과 같은 규칙)의 프로젝트가 신뢰를 가진다([`ProjectTrust::owner_id`]). 읽기 · 바꾸기 ·
//!   묻기 모두 그 프로젝트의 행을 쓴다.
//! - **확인 다이얼로그는 한 번만 뜬다.** 창은 띄우기 전에 [`Registry::claim_trust_prompt`] 를 부른다: 쓰기 트랜잭션 안에서
//!   「묻지 않음」을 「미신뢰」로 바꾸고 바꾼 호출에만 `true` 를 준다. 같은 프로젝트의 창 둘(워크트리 프로젝트의 창, 다른
//!   프로세스의 창)이 동시에 열려도 하나만 묻는다. 답하지 않고 창을 닫아도 다시 묻지 않는다 — 상태바의 신뢰 액션이 남는다.

use std::path::PathBuf;

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use super::path::worktree_link;
use super::{
    ChangeKind, LOCAL_HOST, Project, Registry, RegistryError, load, load_one, now_ms, store,
};

/// 프로젝트의 신뢰 상태. `trusted` 가 아니면 런타임 제공자를 돌리지 않는다.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum TrustState {
    /// 아직 묻지 않았다(미신뢰로 다룬다). 창을 열면 확인 다이얼로그가 한 번 뜬다.
    Unknown,
    Untrusted,
    Trusted,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct ProjectTrust {
    pub project_id: String,
    /// 신뢰를 가진 프로젝트. 메인 저장소 프로젝트가 등록된 워크트리 프로젝트면 그 프로젝트, 아니면 `project_id` 와 같다.
    pub owner_id: String,
    pub state: TrustState,
}

impl ProjectTrust {
    pub fn trusted(&self) -> bool {
        self.state == TrustState::Trusted
    }
}

impl Registry {
    /// 프로젝트의 신뢰. 워크트리 프로젝트는 메인 저장소 프로젝트의 것이다(머리 주석).
    pub fn trust(&self, id: &str) -> Result<ProjectTrust, RegistryError> {
        let owner_id = self.trust_owner(id)?;
        let state = self.read(|conn| read_state(conn, &owner_id))?;
        Ok(ProjectTrust {
            project_id: id.to_owned(),
            owner_id,
            state,
        })
    }

    /// 런타임 게이트가 부르는 판정(`pw_runtime::Runtime::with_trust`). 모르는 프로젝트 · 읽기 실패는 미신뢰다.
    pub fn is_trusted(&self, id: &str) -> bool {
        self.trust(id).is_ok_and(|t| t.trusted())
    }

    /// 신뢰하거나 해제한다. 신뢰를 가진 프로젝트(워크트리면 메인 저장소 프로젝트)의 행을 쓰고 그 프로젝트로
    /// [`ChangeKind::Trust`] 를 낸다. 이미 그 값이면 쓰지 않고 이벤트도 내지 않는다 — 확인 창을 「신뢰하지 않음」 으로 닫으면
    /// 묻기 차례를 잡을 때 쓴 미신뢰와 같은 값이다. `readOnly` · `memory` 면 [`RegistryError::Unavailable`].
    pub fn set_trust(&self, id: &str, trusted: bool) -> Result<ProjectTrust, RegistryError> {
        self.ensure_writable()?;
        let owner_id = self.trust_owner(id)?;
        let changed = self.write(|tx| {
            load_one(tx, &owner_id)?;
            let before = read_state(tx, &owner_id)?;
            let after = if trusted {
                TrustState::Trusted
            } else {
                TrustState::Untrusted
            };
            if before == after {
                return Ok(false);
            }
            tx.execute(
                "INSERT INTO project_trust (project_id, trusted, decided_at) VALUES (?1, ?2, ?3)
                 ON CONFLICT (project_id) DO UPDATE SET trusted = ?2, decided_at = ?3",
                params![owner_id, trusted, now_ms()],
            )?;
            Ok(true)
        })?;
        if changed {
            self.publish(ChangeKind::Trust, &owner_id);
        }
        Ok(ProjectTrust {
            project_id: id.to_owned(),
            state: if trusted {
                TrustState::Trusted
            } else {
                TrustState::Untrusted
            },
            owner_id,
        })
    }

    /// 확인 다이얼로그를 띄울 차례인가. 아직 묻지 않은 프로젝트면 미신뢰로 기록하고 `true` — 이 호출만 다이얼로그를 띄운다.
    /// 이미 물었거나 신뢰가 정해졌으면 `false`. 기록이 미신뢰라 상태 표시는 그대로다. `readOnly` · `memory` 면
    /// [`RegistryError::Unavailable`] — 답을 저장할 수 없으니 묻지 않는다.
    pub fn claim_trust_prompt(&self, id: &str) -> Result<bool, RegistryError> {
        self.ensure_writable()?;
        let owner_id = self.trust_owner(id)?;
        self.write(|tx| {
            load_one(tx, &owner_id)?;
            let inserted = tx.execute(
                "INSERT OR IGNORE INTO project_trust (project_id, trusted, decided_at)
                 VALUES (?1, 0, ?2)",
                params![owner_id, now_ms()],
            )?;
            Ok(inserted == 1)
        })
    }

    /// `owner_id` 의 신뢰를 쓰는 프로젝트: 그 프로젝트와, 그 신뢰를 따르는 등록된 워크트리 프로젝트. 신뢰가 바뀌면 이 프로젝트들의
    /// 런타임 환경을 다시 읽는다(`pw_runtime::Runtime::trust_changed`).
    pub fn trust_followers(&self, owner_id: &str) -> Result<Vec<String>, RegistryError> {
        let projects = self.read(|conn| load(conn, None))?;
        Ok(projects
            .iter()
            .filter(|p| {
                p.id == owner_id
                    || main_repository_project(p, &projects).is_some_and(|m| m.id == owner_id)
            })
            .map(|p| p.id.clone())
            .collect())
    }

    /// 신뢰를 가진 프로젝트의 id. 워크트리 판정은 디스크(`.git` 파일)를 읽으므로 DB 잠금 밖에서 한다.
    fn trust_owner(&self, id: &str) -> Result<String, RegistryError> {
        let projects = self.read(|conn| load(conn, None))?;
        let project = projects
            .iter()
            .find(|p| p.id == id)
            .ok_or_else(|| RegistryError::NotFound(id.to_owned()))?;
        Ok(main_repository_project(project, &projects)
            .map_or_else(|| project.id.clone(), |main| main.id.clone()))
    }
}

/// `project` 의 루트가 연결 워크트리 안이면 메인 체크아웃 쪽 같은 자리를 포함하는 등록 프로젝트([`super::resolve`] 의 2 단계와
/// 같은 규칙: 메인 체크아웃 안의 루트 중 가장 깊은 것). 루트가 워크트리 밖(워크트리를 감싸는 폴더)이면 `None`.
fn main_repository_project<'a>(project: &Project, all: &'a [Project]) -> Option<&'a Project> {
    if project.host != LOCAL_HOST {
        return None;
    }
    let root = project.root();
    let link = worktree_link(root)?;
    let main = link.map_to_main(root)?;
    all.iter()
        .filter(|p| p.id != project.id && p.host == LOCAL_HOST)
        .flat_map(|p| p.roots.iter().map(move |r| (p, PathBuf::from(r))))
        .filter(|(_, r)| r.starts_with(&link.main_top) && main.starts_with(r))
        .max_by_key(|(_, r)| r.components().count())
        .map(|(p, _)| p)
}

fn read_state(conn: &Connection, owner_id: &str) -> Result<TrustState, RegistryError> {
    let trusted: Option<bool> = conn
        .query_row(store::SELECT_TRUST, [owner_id], |row| row.get(0))
        .optional()?;
    Ok(match trusted {
        None => TrustState::Unknown,
        Some(false) => TrustState::Untrusted,
        Some(true) => TrustState::Trusted,
    })
}
