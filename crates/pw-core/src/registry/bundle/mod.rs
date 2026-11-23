//! 묶음(08-remote-mobile.md 「묶음」, 18-m7-plan M7-10): 한 저장소의 체크아웃을 가진 멤버 프로젝트들의 모음. 사람에게는 이것이
//! 「프로젝트」 다. 묶음은 Desktop 이 가진다 — Host 는 다른 머신의 체크아웃을 모른다(D57 ④).
//!
//! - **저장**(메인 판정 M7-P7): `state.db` v5 의 `bundles` · `bundle_members`([`super::store`]). 로컬 프로젝트 하나뿐인 묶음은
//!   **행 없는 암묵 묶음**이다 — id 가 그 프로젝트 id 라 창 label(`project-<id>`) · `project.state.*` 가 M7 전과 같다(이전 없음).
//!   멤버가 둘이 되는 때 행을 만들고(id 는 [`BUNDLE_ID_PREFIX`] 로 시작), 둘 이상 · 원격만인 묶음이 행을 갖는다. 행 있는 묶음이
//!   로컬 멤버 하나로 줄면 행을 지워 다시 암묵 묶음이 된다.
//! - **머신 id** 는 불투명한 글이다. 이 머신은 [`LOCAL_MACHINE`], 다른 머신은 M7-P6 의 꼴(Relay 기기 공개키 지문)이고 이
//!   모듈은 꼴을 보지 않는다. 멤버는 (머신 id, 그 머신 레지스트리의 프로젝트 id)만 기억한다(M7-P18).
//! - **한 머신의 멤버는 하나**(M7-P20), **멤버 프로젝트는 묶음 하나에만** 든다 — 표의 열쇠가 지킨다. 더하기의 판정(같은 저장소 ·
//!   두 번째 클론 · 다른 묶음에서 옮겨 오기)은 [`add`].
//! - **체크아웃 식별자**는 [`CheckoutRef`] `{ machine, worktree? }` 다(M7-P10 의 `checkout` 입력과 같은 꼴). 멤버마다 「이 멤버를
//!   원본으로 한 마지막 보내기 대상」 을 하나 둔다([`Registry::set_last_send_target`]).
//! - 묶음이 바뀌면 [`super::TOPIC_CHANGED`] 를 `kind: bundle`, `projectId: <묶음 id>` 로 낸다.
//!
//! 뒤 행(M7-11 · 17 · 18 · 19)은 읽기([`Registry::bundles`] · [`Registry::bundle`] · [`Registry::bundle_of`])와 마지막 대상
//! 갱신만 한다.

pub mod add;
pub mod history;
pub mod machine;
pub mod methods;

use std::collections::HashSet;

use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};

use super::{ChangeKind, LOCAL_HOST, Registry, RegistryError, load, now_ms};

/// 이 머신(그 레지스트리를 가진 머신)의 머신 id. 레지스트리의 프로젝트는 모두 이 머신의 것이다.
pub const LOCAL_MACHINE: &str = LOCAL_HOST;
/// 행 있는 묶음 id 의 머리. 프로젝트 id 는 16자리 hex 라 `-` 가 없어 겹치지 않는다.
pub const BUNDLE_ID_PREFIX: &str = "b-";
/// 묶음 창의 label 머리(`project-<묶음 id>`). 암묵 묶음은 묶음 id 가 프로젝트 id 라 M7 전의 프로젝트 창 label 과 같다.
pub const WINDOW_LABEL_PREFIX: &str = "project-";

/// 묶음 창의 label.
pub fn window_label(bundle_id: &str) -> String {
    format!("{WINDOW_LABEL_PREFIX}{bundle_id}")
}

/// 행 있는 묶음의 id 인가(꼴만 본다).
pub fn is_bundle_row_id(id: &str) -> bool {
    id.starts_with(BUNDLE_ID_PREFIX)
}

/// 체크아웃 식별자: 머신과 그 머신의 워크트리. `worktree` 가 없으면 멤버 프로젝트의 메인 체크아웃이다.
#[derive(
    Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct CheckoutRef {
    /// 머신 id(이 머신이면 `local`).
    pub machine: String,
    /// 워크트리 꼭대기의 절대 경로(그 머신의 경로). 없으면 메인 체크아웃.
    #[serde(default)]
    pub worktree: Option<String>,
}

/// 묶음의 멤버: 한 머신의 프로젝트 하나.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct ProjectBundleMember {
    /// 머신 id(이 머신이면 `local`).
    pub machine: String,
    /// 그 머신 레지스트리의 프로젝트 id. 머신 사이에 유일하지 않다.
    pub project_id: String,
    /// 이 멤버를 원본으로 한 마지막 보내기 대상. 없으면 `null`.
    pub last_target: Option<CheckoutRef>,
}

/// 묶음 하나. 사람에게는 「프로젝트」 한 줄이다.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct ProjectBundle {
    /// 묶음 id. 암묵 묶음이면 그 로컬 프로젝트 id, 행 있는 묶음이면 `b-` 로 시작한다. 창 label 은 `project-<id>`.
    pub id: String,
    pub name: String,
    pub favorite: bool,
    /// 정렬 · 중복 없음.
    pub tags: Vec<String>,
    /// 마지막으로 연 시각(Unix 밀리초). 연 적 없으면 `null`.
    #[specta(type = Option<pw_rpc::JsNumber>)]
    pub opened_at: Option<i64>,
    /// 행 없는 암묵 묶음(로컬 프로젝트 하나)인가.
    pub implicit: bool,
    /// 더한 순서. 암묵 묶음은 로컬 멤버 하나.
    pub members: Vec<ProjectBundleMember>,
}

impl ProjectBundle {
    /// 그 머신의 멤버.
    pub fn member(&self, machine: &str) -> Option<&ProjectBundleMember> {
        self.members.iter().find(|m| m.machine == machine)
    }

    /// 로컬 멤버의 프로젝트 id.
    pub fn local_project(&self) -> Option<&str> {
        self.member(LOCAL_MACHINE).map(|m| m.project_id.as_str())
    }
}

/// 행 있는 묶음의 목록 정보(멤버 제외).
struct BundleRow {
    id: String,
    name: String,
    favorite: bool,
    tags: Vec<String>,
    opened_at: Option<i64>,
}

fn parse_tags(text: &str) -> rusqlite::Result<Vec<String>> {
    serde_json::from_str(text).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
    })
}

fn tags_text(tags: &[String]) -> String {
    let mut tags: Vec<String> = tags
        .iter()
        .map(|t| t.trim().to_owned())
        .filter(|t| !t.is_empty())
        .collect();
    tags.sort();
    tags.dedup();
    serde_json::to_string(&tags).expect("글 배열은 직렬화된다")
}

fn load_rows(conn: &Connection, only: Option<&str>) -> Result<Vec<ProjectBundle>, RegistryError> {
    let mut stmt = conn.prepare(
        "SELECT id, name, favorite, tags, opened_at FROM bundles WHERE ?1 IS NULL OR id = ?1",
    )?;
    let rows: Vec<BundleRow> = stmt
        .query_map([only], |row| {
            Ok(BundleRow {
                id: row.get(0)?,
                name: row.get(1)?,
                favorite: row.get(2)?,
                tags: parse_tags(&row.get::<_, String>(3)?)?,
                opened_at: row.get(4)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    let mut members = conn.prepare(
        "SELECT machine, project_id, last_target_machine, last_target_worktree FROM bundle_members
         WHERE bundle_id = ?1 ORDER BY position",
    )?;
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let list = members
            .query_map([&row.id], |r| {
                let target: Option<String> = r.get(2)?;
                let worktree: Option<String> = r.get(3)?;
                Ok(ProjectBundleMember {
                    machine: r.get(0)?,
                    project_id: r.get(1)?,
                    last_target: target.map(|machine| CheckoutRef { machine, worktree }),
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        out.push(ProjectBundle {
            id: row.id,
            name: row.name,
            favorite: row.favorite,
            tags: row.tags,
            opened_at: row.opened_at,
            implicit: false,
            members: list,
        });
    }
    Ok(out)
}

/// 로컬 프로젝트의 암묵 묶음.
fn implicit(project: &super::Project) -> ProjectBundle {
    ProjectBundle {
        id: project.id.clone(),
        name: project.name.clone(),
        favorite: project.favorite,
        tags: project.tags.clone(),
        opened_at: project.opened_at,
        implicit: true,
        members: vec![ProjectBundleMember {
            machine: LOCAL_MACHINE.to_owned(),
            project_id: project.id.clone(),
            last_target: None,
        }],
    }
}

/// 그 멤버가 든 행 있는 묶음의 id.
fn row_of(conn: &Connection, machine: &str, project_id: &str) -> rusqlite::Result<Option<String>> {
    conn.query_row(
        "SELECT bundle_id FROM bundle_members WHERE machine = ?1 AND project_id = ?2",
        params![machine, project_id],
        |row| row.get(0),
    )
    .optional()
}

/// 묶음 하나(행 있는 것, 아니면 어느 묶음에도 들지 않은 로컬 프로젝트의 암묵 묶음).
fn load_bundle(conn: &Connection, id: &str) -> Result<ProjectBundle, RegistryError> {
    if is_bundle_row_id(id) {
        return load_rows(conn, Some(id))?
            .pop()
            .ok_or_else(|| RegistryError::NotFound(id.to_owned()));
    }
    let project = super::load_one(conn, id)?;
    // 행 있는 묶음의 멤버가 된 로컬 프로젝트의 id 는 더는 묶음 id 가 아니다.
    if row_of(conn, LOCAL_MACHINE, id)?.is_some() {
        return Err(RegistryError::NotFound(id.to_owned()));
    }
    Ok(implicit(&project))
}

/// 멤버 하나를 행 있는 묶음에 넣을 때 무엇을 했는가.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Attached {
    Bundle(ProjectBundle),
    /// 그 멤버가 이미 멤버 둘 이상인 다른 묶음에 있다.
    InOtherBundle {
        bundle_id: String,
    },
    /// 판정한 뒤 그 묶음의 멤버가 바뀌었다(다른 더하기가 먼저 붙었다 · 빠졌다). `bundle_id` 는 지금 그 묶음(암묵 묶음이 행 있는
    /// 묶음이 됐으면 새 id).
    BundleChanged {
        bundle_id: String,
    },
}

/// 판정한 멤버 목록과 지금 목록이 같은가(머신 · 프로젝트 id, 순서 무관).
fn same_members(judged: &[ProjectBundleMember], now: &[ProjectBundleMember]) -> bool {
    let key = |m: &ProjectBundleMember| (m.machine.clone(), m.project_id.clone());
    let mut a: Vec<_> = judged.iter().map(key).collect();
    let mut b: Vec<_> = now.iter().map(key).collect();
    a.sort();
    b.sort();
    a == b
}

impl Registry {
    /// 묶음 전부: 행 있는 묶음과, 어느 묶음에도 들지 않은 로컬 프로젝트의 암묵 묶음. 이름순.
    pub fn bundles(&self) -> Result<Vec<ProjectBundle>, RegistryError> {
        let mut out = self.read(|conn| {
            let rows = load_rows(conn, None)?;
            let taken: HashSet<&str> = rows
                .iter()
                .filter_map(ProjectBundle::local_project)
                .collect();
            let mut out: Vec<ProjectBundle> = load(conn, None)?
                .iter()
                .filter(|p| !taken.contains(p.id.as_str()))
                .map(implicit)
                .collect();
            out.extend(rows);
            Ok(out)
        })?;
        out.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));
        Ok(out)
    }

    /// 묶음 하나. 없으면(행 있는 묶음의 멤버가 된 로컬 프로젝트의 id 도) [`RegistryError::NotFound`].
    pub fn bundle(&self, id: &str) -> Result<ProjectBundle, RegistryError> {
        self.read(|conn| load_bundle(conn, id))
    }

    /// 그 머신 프로젝트가 든 묶음. 로컬 프로젝트가 행 있는 묶음에 없으면 그 암묵 묶음이다. 다른 머신의 프로젝트가 어느 묶음에도
    /// 없으면 `None`.
    pub fn bundle_of(
        &self,
        machine: &str,
        project_id: &str,
    ) -> Result<Option<ProjectBundle>, RegistryError> {
        self.read(|conn| match row_of(conn, machine, project_id)? {
            Some(id) => Ok(load_rows(conn, Some(&id))?.pop()),
            None if machine == LOCAL_MACHINE => match load(conn, Some(project_id))?.pop() {
                Some(project) => Ok(Some(implicit(&project))),
                None => Ok(None),
            },
            None => Ok(None),
        })
    }

    /// 더하기를 시작할 때의 묶음([`add`]). 암묵 묶음이던 로컬 프로젝트가 그사이 행 있는 묶음의 멤버가 됐으면 `Err(그 묶음 id)`.
    pub(crate) fn bundle_for_add(
        &self,
        id: &str,
    ) -> Result<Result<ProjectBundle, String>, RegistryError> {
        self.read(|conn| bundle_for_add(conn, id))
    }

    /// 묶음 멤버의 머신 id 를 `old` 에서 `new` 로 옮긴다 — 멤버와 마지막 보내기 대상 둘 다, 한 트랜잭션으로. 메인 판정 M7-P6 의
    /// 「이 머신을 새로 믿기」(그 호스트의 머신 id 가 바뀜) 때 M7-08 이 부른다. `local` 은 옮기지도 옮겨 오지도 않는다. 한 묶음에
    /// 두 id 의 멤버가 함께 있으면(같은 머신이 두 멤버가 된다 — M7-P20) 아무것도 바꾸지 않고 거절한다. 바뀐 묶음 id 들을 돌려준다.
    pub fn rename_bundle_machine(
        &self,
        old: &str,
        new: &str,
    ) -> Result<Vec<String>, RegistryError> {
        self.ensure_writable()?;
        if old == LOCAL_MACHINE || new == LOCAL_MACHINE || old == new || new.is_empty() {
            return Err(RegistryError::InvalidPath {
                path: new.into(),
                reason: format!("cannot move bundle members from machine `{old}` to `{new}`"),
            });
        }
        let changed = self.write(|tx| {
            let both: Option<String> = tx
                .query_row(
                    "SELECT a.bundle_id FROM bundle_members a JOIN bundle_members b
                       ON a.bundle_id = b.bundle_id WHERE a.machine = ?1 AND b.machine = ?2",
                    params![old, new],
                    |row| row.get(0),
                )
                .optional()?;
            if let Some(bundle_id) = both {
                return Err(RegistryError::InvalidPath {
                    path: bundle_id.into(),
                    reason: format!("the bundle already has a member on machine `{new}`"),
                });
            }
            let ids: Vec<String> = tx
                .prepare(
                    "SELECT DISTINCT bundle_id FROM bundle_members
                     WHERE machine = ?1 OR last_target_machine = ?1 ORDER BY bundle_id",
                )?
                .query_map([old], |row| row.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            tx.execute(
                "UPDATE bundle_members SET machine = ?2 WHERE machine = ?1",
                params![old, new],
            )?;
            tx.execute(
                "UPDATE bundle_members SET last_target_machine = ?2 WHERE last_target_machine = ?1",
                params![old, new],
            )?;
            Ok(ids)
        })?;
        for id in &changed {
            self.publish(ChangeKind::Bundle, id);
        }
        Ok(changed)
    }

    /// 멤버를 묶음에서 뺀다. 그 머신의 프로젝트 등록과 파일은 건드리지 않는다(08 「묶음」). 행 있는 묶음이 로컬 멤버 하나로
    /// 줄면 행을 지워 암묵 묶음으로 돌아가고, 멤버가 없어지면 묶음이 사라진다. 암묵 묶음의 하나뿐인 멤버는 뺄 수 없다
    /// (프로젝트를 지우는 것은 `project.delete`).
    pub fn remove_bundle_member(
        &self,
        bundle_id: &str,
        machine: &str,
    ) -> Result<(), RegistryError> {
        self.ensure_writable()?;
        if !is_bundle_row_id(bundle_id) {
            return Err(RegistryError::InvalidPath {
                path: bundle_id.into(),
                reason: "a bundle with a single local project has no member to remove".into(),
            });
        }
        self.write(|tx| {
            let removed = tx.execute(
                "DELETE FROM bundle_members WHERE bundle_id = ?1 AND machine = ?2",
                params![bundle_id, machine],
            )?;
            if removed == 0 {
                return Err(RegistryError::NotFound(format!("{bundle_id}:{machine}")));
            }
            collapse(tx, bundle_id)
        })?;
        self.publish(ChangeKind::Bundle, bundle_id);
        Ok(())
    }

    /// `source` 머신 멤버를 원본으로 한 마지막 보내기 대상을 적는다(`None` 이면 지운다). 대상 머신은 그 묶음의 멤버여야 한다.
    /// 암묵 묶음은 다른 머신이 없어 대상이 없다(거절).
    pub fn set_last_send_target(
        &self,
        bundle_id: &str,
        source: &str,
        target: Option<&CheckoutRef>,
    ) -> Result<ProjectBundle, RegistryError> {
        self.ensure_writable()?;
        let bundle = self.write(|tx| {
            let bundle = load_bundle(tx, bundle_id)?;
            if bundle.implicit {
                return Err(RegistryError::InvalidPath {
                    path: bundle_id.into(),
                    reason: "a bundle with a single member has no send target".into(),
                });
            }
            if bundle.member(source).is_none() {
                return Err(RegistryError::NotFound(format!("{bundle_id}:{source}")));
            }
            if let Some(t) = target
                && bundle.member(&t.machine).is_none()
            {
                return Err(RegistryError::NotFound(format!(
                    "{bundle_id}:{}",
                    t.machine
                )));
            }
            tx.execute(
                "UPDATE bundle_members SET last_target_machine = ?3, last_target_worktree = ?4
                 WHERE bundle_id = ?1 AND machine = ?2",
                params![
                    bundle_id,
                    source,
                    target.map(|t| t.machine.as_str()),
                    target.and_then(|t| t.worktree.as_deref())
                ],
            )?;
            load_bundle(tx, bundle_id)
        })?;
        self.publish(ChangeKind::Bundle, bundle_id);
        Ok(bundle)
    }

    /// 멤버 `(machine, project_id)` 를 `bundle_id` 묶음에 넣는다. 판정([`add`])을 지난 뒤에 부른다 — 여기서는 표의 규칙만 다시
    /// 본다. 암묵 묶음(로컬 프로젝트 id)이면 행을 만들어 그 로컬 프로젝트를 첫 멤버로 둔다. `bundle_id` 가 `None` 이면 새 묶음
    /// (원격만)을 만든다. 넣으려는 멤버가 멤버 하나뿐인 다른 묶음에 있으면 옮겨 오고(그 묶음은 사라진다), 둘 이상인 묶음에 있으면
    /// [`Attached::InOtherBundle`].
    ///
    /// `judged` 는 판정 때 본 멤버 목록이다. 지금 목록이 그와 다르면(그사이 다른 더하기가 붙었거나 멤버가 빠졌다) 붙이지 않고
    /// [`Attached::BundleChanged`] — 새 멤버는 지금 멤버들과의 같은 저장소 판정을 지나지 않았다.
    pub(crate) fn attach_bundle_member(
        &self,
        bundle_id: Option<&str>,
        judged: &[ProjectBundleMember],
        machine: &str,
        project_id: &str,
        name: &str,
    ) -> Result<Attached, RegistryError> {
        self.ensure_writable()?;
        let attached = self.write(|tx| {
            let current = match bundle_id {
                Some(id) => match changed_since(tx, id, judged)? {
                    Ok(b) => Some(b),
                    Err(now) => return Ok(Attached::BundleChanged { bundle_id: now }),
                },
                None => None,
            };
            if let Some(b) = &current
                && b.member(machine).is_some()
            {
                return Err(RegistryError::InvalidPath {
                    path: project_id.into(),
                    reason: format!("the bundle already has a member on machine `{machine}`"),
                });
            }
            // 다른 묶음에서 옮겨 오기. 로컬 프로젝트의 암묵 묶음은 행이 없어 옮길 것이 없다.
            if let Some(other) = row_of(tx, machine, project_id)? {
                let count: i64 = tx.query_row(
                    "SELECT count(*) FROM bundle_members WHERE bundle_id = ?1",
                    [&other],
                    |row| row.get(0),
                )?;
                if count >= 2 {
                    return Ok(Attached::InOtherBundle { bundle_id: other });
                }
                tx.execute("DELETE FROM bundles WHERE id = ?1", [&other])?;
            }
            let id = match &current {
                Some(b) if !b.implicit => b.id.clone(),
                _ => {
                    let id = new_bundle_id(tx)?;
                    let seed = current.clone();
                    let (name, favorite, tags, opened_at) = match &seed {
                        Some(b) => (b.name.clone(), b.favorite, b.tags.clone(), b.opened_at),
                        None => (name.to_owned(), false, Vec::new(), None),
                    };
                    tx.execute(
                        "INSERT INTO bundles (id, name, favorite, tags, created_at, opened_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                        params![id, name, favorite, tags_text(&tags), now_ms(), opened_at],
                    )?;
                    if let Some(b) = seed {
                        for m in &b.members {
                            insert_member(tx, &id, &m.machine, &m.project_id)?;
                        }
                    }
                    id
                }
            };
            insert_member(tx, &id, machine, project_id)?;
            Ok(Attached::Bundle(load_bundle(tx, &id)?))
        })?;
        if let Attached::Bundle(b) = &attached {
            self.publish(ChangeKind::Bundle, &b.id);
        }
        Ok(attached)
    }
}

/// 더하기를 시작할 때의 묶음. 암묵 묶음이던 로컬 프로젝트가 그사이 행 있는 묶음의 멤버가 됐으면 `Err(그 묶음 id)`.
fn bundle_for_add(
    conn: &Connection,
    id: &str,
) -> Result<Result<ProjectBundle, String>, RegistryError> {
    if !is_bundle_row_id(id)
        && let Some(row) = row_of(conn, LOCAL_MACHINE, id)?
    {
        return Ok(Err(row));
    }
    Ok(Ok(load_bundle(conn, id)?))
}

/// 붙이기 직전의 묶음. 판정 때와 멤버가 다르거나 묶음이 사라졌으면 `Err(지금 묶음 id)`.
fn changed_since(
    conn: &Connection,
    id: &str,
    judged: &[ProjectBundleMember],
) -> Result<Result<ProjectBundle, String>, RegistryError> {
    match bundle_for_add(conn, id) {
        Ok(Ok(b)) if same_members(judged, &b.members) => Ok(Ok(b)),
        Ok(Ok(b)) => Ok(Err(b.id)),
        Ok(Err(row)) => Ok(Err(row)),
        Err(RegistryError::NotFound(_)) => Ok(Err(id.to_owned())),
        Err(e) => Err(e),
    }
}

fn insert_member(
    tx: &Transaction<'_>,
    bundle_id: &str,
    machine: &str,
    project_id: &str,
) -> Result<(), RegistryError> {
    tx.execute(
        "INSERT INTO bundle_members (bundle_id, machine, project_id, position)
         VALUES (?1, ?2, ?3, (SELECT coalesce(max(position) + 1, 0) FROM bundle_members WHERE bundle_id = ?1))",
        params![bundle_id, machine, project_id],
    )?;
    Ok(())
}

/// 새 묶음 id: `b-` + 16자리 hex. 프로젝트 id 와 겹치지 않는다(`-` 가 없다). 겹치는 행이 있으면 다시 뽑는다.
fn new_bundle_id(tx: &Transaction<'_>) -> Result<String, RegistryError> {
    loop {
        let hex: String = tx.query_row("SELECT lower(hex(randomblob(8)))", [], |row| row.get(0))?;
        let id = format!("{BUNDLE_ID_PREFIX}{hex}");
        let taken = tx
            .query_row("SELECT 1 FROM bundles WHERE id = ?1", [&id], |_| Ok(()))
            .optional()?
            .is_some();
        if !taken {
            return Ok(id);
        }
    }
}

/// 멤버가 빠진 뒤 묶음을 정리한다: 멤버가 없거나 로컬 멤버 하나만 남으면 행을 지운다(로컬 하나면 암묵 묶음으로 돌아간다).
/// 원격 멤버 하나만 남으면 원격만인 묶음으로 남는다.
fn collapse(tx: &Transaction<'_>, bundle_id: &str) -> Result<(), RegistryError> {
    let machines: Vec<String> = tx
        .prepare("SELECT machine FROM bundle_members WHERE bundle_id = ?1")?
        .query_map([bundle_id], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    if machines.is_empty() || machines == [LOCAL_MACHINE] {
        tx.execute("DELETE FROM bundles WHERE id = ?1", [bundle_id])?;
    } else {
        // 빠진 머신을 대상으로 기억한 칸을 지운다.
        tx.execute(
            "UPDATE bundle_members SET last_target_machine = NULL, last_target_worktree = NULL
             WHERE bundle_id = ?1 AND last_target_machine IS NOT NULL
               AND last_target_machine NOT IN (SELECT machine FROM bundle_members WHERE bundle_id = ?1)",
            [bundle_id],
        )?;
    }
    Ok(())
}

/// 로컬 프로젝트를 레지스트리에서 지울 때(`Registry::delete` 의 트랜잭션 안) 그 멤버 행도 지운다.
pub(crate) fn forget_local_project(
    tx: &Transaction<'_>,
    project_id: &str,
) -> Result<(), RegistryError> {
    if let Some(bundle_id) = row_of(tx, LOCAL_MACHINE, project_id)? {
        tx.execute(
            "DELETE FROM bundle_members WHERE bundle_id = ?1 AND machine = ?2",
            params![bundle_id, LOCAL_MACHINE],
        )?;
        collapse(tx, &bundle_id)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
