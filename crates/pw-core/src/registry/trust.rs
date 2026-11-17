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

use std::path::{Path, PathBuf};

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

    /// `id` 가 연결 워크트리를 따로 등록한 프로젝트면 그 저장소의 메인 체크아웃 쪽 등록 프로젝트 — 신뢰가 따르는 프로젝트(
    /// [`Self::set_trust`] 가 쓰는 행의 주인)다. 아니면 `None`. 에이전트의 DB 관문이 「그 저장소의 메인 체크아웃」 의 연결 설정을
    /// 읽는 데 쓴다(16-m5-plan.md 결정 M5-D37). 워크트리 판정은 디스크(`.git` 파일)를 읽는다.
    pub fn main_repository_of(&self, id: &str) -> Result<Option<Project>, RegistryError> {
        let projects = self.read(|conn| load(conn, None))?;
        let project = projects
            .iter()
            .find(|p| p.id == id)
            .ok_or_else(|| RegistryError::NotFound(id.to_owned()))?;
        Ok(main_repository_project(project, &projects).cloned())
    }

    /// 루트 `root` 를 가진 프로젝트를 새로 더하면 신뢰 판정 · 경로 해석이 어떻게 바뀌는가([`TrustShift`]). 판정은 [`Self::trust_owner`] 와
    /// 같은 규칙([`main_repository_project`])을 새 프로젝트를 더한 목록에 다시 돌린 것이다 — 안쪽 · 바깥쪽 · 메인 체크아웃 ·
    /// 워크트리 어느 방향이든 신뢰 주인이 바뀌는 것은 모두 여기서 나온다. 레지스트리에 쓰지 않는다. 에이전트의 등록
    /// (`project_register`, [`super::tool`])이 기존 프로젝트의 신뢰를 흔드는 등록을 거절하는 데 쓴다.
    pub fn trust_shift(&self, root: &Path) -> Result<TrustShift, RegistryError> {
        let mut projects = self.read(|conn| load(conn, None))?;
        let before: Vec<(Project, String)> = projects
            .iter()
            .map(|p| (p.clone(), owner_of(p, &projects)))
            .collect();
        let candidate = Project {
            // 레지스트리 id 는 16진수 16자라 이 id 와 겹치지 않는다.
            id: NEW_PROJECT.to_owned(),
            name: String::new(),
            host: LOCAL_HOST.to_owned(),
            roots: vec![root.display().to_string()],
            favorite: false,
            tags: Vec::new(),
            config_mode: super::ConfigMode::default(),
            slug: String::new(),
            created_at: 0,
            opened_at: None,
        };
        projects.push(candidate.clone());
        let moved = before
            .into_iter()
            .filter(|(p, owner)| owner_of(p, &projects) != *owner)
            .map(|(p, _)| p)
            .collect();
        let follows = main_repository_project(&candidate, &projects).cloned();
        // 해석이 바뀌는 프로젝트: 새 루트 안쪽, 그리고 새 루트가 연결 워크트리 안이면 메인 쪽 대응 자리 안쪽의 등록 루트(그
        // 루트들은 지금 이 폴더 안을 활성 워크트리로 연다 — `super::resolve` 의 2 단계). 리뷰 D1.
        let link = worktree_link(root);
        let mapped = link.as_ref().and_then(|l| Some((l, l.map_to_main(root)?)));
        projects.pop();
        let shadowed = projects
            .into_iter()
            .filter(|p| p.host == LOCAL_HOST)
            .filter(|p| {
                p.roots.iter().map(PathBuf::from).any(|r| {
                    r.starts_with(root)
                        || mapped.as_ref().is_some_and(|(l, main)| {
                            r.starts_with(&l.main_top) && r.starts_with(main)
                        })
                })
            })
            .collect();
        Ok(TrustShift {
            moved,
            follows,
            shadowed,
        })
    }

    /// 신뢰를 가진 프로젝트의 id. 워크트리 판정은 디스크(`.git` 파일)를 읽으므로 DB 잠금 밖에서 한다.
    fn trust_owner(&self, id: &str) -> Result<String, RegistryError> {
        let projects = self.read(|conn| load(conn, None))?;
        let project = projects
            .iter()
            .find(|p| p.id == id)
            .ok_or_else(|| RegistryError::NotFound(id.to_owned()))?;
        Ok(owner_of(project, &projects))
    }
}

/// [`Registry::trust_shift`] 가 새 프로젝트에 붙이는 임시 id.
const NEW_PROJECT: &str = "(new project)";

/// 프로젝트 하나를 더했을 때의 신뢰 판정 변화([`Registry::trust_shift`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustShift {
    /// 신뢰 주인이 바뀌는 기존 프로젝트(새 프로젝트가 그 주인이 된다) — 그 프로젝트의 신뢰가 새 프로젝트의 행을 따르게 된다.
    pub moved: Vec<Project>,
    /// 새 프로젝트가 신뢰를 따르게 될 기존 프로젝트(새 루트가 그 저장소의 연결 워크트리 안이다).
    pub follows: Option<Project>,
    /// 루트가 새 루트 안쪽(새 루트가 연결 워크트리 안이면 메인 체크아웃 쪽 대응 자리 안쪽)에 있는 기존 프로젝트 — 그 안의
    /// 경로가 지금 그 프로젝트로 풀린다. 새 루트가 그 해석을 가로챌 수 있다(워크트리 꼭대기를 등록하면 `W/app/…` 이 메인 쪽
    /// `M/app` 대신 새 프로젝트로 풀린다).
    pub shadowed: Vec<Project>,
}

/// `project` 의 신뢰 주인 id(`all` 기준).
fn owner_of(project: &Project, all: &[Project]) -> String {
    main_repository_project(project, all).map_or_else(|| project.id.clone(), |main| main.id.clone())
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
