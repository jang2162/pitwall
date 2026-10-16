//! 프로젝트 레지스트리(F1-1, F2, F2-5): `$DATA/state.db` 의 프로젝트 목록 · 최근 목록, 경로 → 프로젝트 해석, 루트 후보 추천.
//!
//! - [`store`]: SQLite 파일 · 스키마 버전 · 마이그레이션 · 열기 실패 처리.
//! - [`path`]: 경로 비교 규칙(정규화).
//! - [`resolve`]: 경로 → 프로젝트(가장 깊은 루트, 워크트리 → 메인 저장소).
//! - [`candidates`]: 루트 후보 추천과 그 순서.
//! - [`slug`]: 개인 전용 설정 폴더 이름.
//! - [`methods`]: 레지스트리 RPC(`project.*`).
//!
//! 레지스트리는 처음 쓰일 때 파일을 연다. Host 를 만들기만 해서는 `$DATA` 에 아무것도 생기지 않는다.
//! 변경은 [`Scope::Host`] 의 [`TOPIC_CHANGED`] 이벤트로 나간다.

pub mod candidates;
pub mod methods;
pub mod path;
pub mod resolve;
pub mod slug;
pub mod store;

use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};

use crate::bus::{EventBus, Scope};
use crate::config::{AppPaths, ConfigError, ConfigSource};
use path::{PathRejected, normalize};
pub use store::StorageStatus;

/// 레지스트리가 바뀌면 [`Scope::Host`] 로 나가는 이벤트. payload 는 [`ProjectChanged`].
pub const TOPIC_CHANGED: &str = "project.changed";
/// 지금 레지스트리가 다루는 Host. 원격 프로젝트(F9)는 이 값이 relay 의 id 가 된다.
pub const LOCAL_HOST: &str = "local";

/// 프로젝트 설정을 어디에 두는가(설계 D9).
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
pub enum ConfigMode {
    /// 저장소의 `<root>/.pitwall/`.
    #[default]
    Repository,
    /// 개인 전용 `$CONFIG/projects/<slug>/`. 저장소의 `.pitwall/` 은 읽지 않는다.
    Personal,
}

impl ConfigMode {
    fn as_str(self) -> &'static str {
        match self {
            ConfigMode::Repository => "repository",
            ConfigMode::Personal => "personal",
        }
    }

    fn parse(s: &str) -> rusqlite::Result<Self> {
        match s {
            "repository" => Ok(ConfigMode::Repository),
            "personal" => Ok(ConfigMode::Personal),
            other => Err(rusqlite::Error::InvalidColumnType(
                0,
                format!("config_mode `{other}`"),
                rusqlite::types::Type::Text,
            )),
        }
    }
}

/// 등록된 프로젝트. 경로는 [`path::normalize`] 로 정규화된 절대 경로다.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    /// `$DATA/projects/<id>/` 에도 쓰이는 id(16자리 16진수).
    pub id: String,
    pub name: String,
    /// 프로젝트가 있는 Host. 지금은 언제나 `local`.
    pub host: String,
    /// 첫 번째가 주 루트다. 지금은 하나뿐이고, 여럿은 멀티 루트(F1-6)가 연다.
    pub roots: Vec<String>,
    pub favorite: bool,
    /// 정렬 · 중복 제거된 태그.
    pub tags: Vec<String>,
    pub config_mode: ConfigMode,
    /// 개인 전용 설정 폴더 이름. 만들 때 정하고 바뀌지 않는다([`slug`]).
    pub slug: String,
    /// 만든 시각(유닉스 밀리초).
    #[specta(type = pw_rpc::JsNumber)]
    pub created_at: i64,
    /// 마지막으로 연 시각(유닉스 밀리초). 한 번도 안 열었으면 `null`.
    #[specta(type = Option<pw_rpc::JsNumber>)]
    pub opened_at: Option<i64>,
}

impl Project {
    /// 주 루트.
    pub fn root(&self) -> &Path {
        Path::new(&self.roots[0])
    }

    /// 이 프로젝트의 설정을 읽을 곳. `root` 는 실제로 연 폴더 — 주 루트, 또는 워크트리로 열었으면
    /// [`ActiveWorktree::root`] 다(그래야 워크트리의 `.local.toml` 폴백이 선다). 개인 전용이면 `root` 를 보지 않는다.
    pub fn config_source(
        &self,
        paths: &AppPaths,
        root: &Path,
    ) -> Result<ConfigSource, ConfigError> {
        match self.config_mode {
            ConfigMode::Repository => Ok(ConfigSource::project(root)),
            ConfigMode::Personal => ConfigSource::personal(paths, &self.slug),
        }
    }
}

/// 새 프로젝트.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NewProject {
    /// 절대 경로. 있는 폴더여야 한다.
    pub root: PathBuf,
    /// 없거나 비면 루트 폴더 이름.
    pub name: Option<String>,
    pub config_mode: ConfigMode,
    pub favorite: bool,
    pub tags: Vec<String>,
}

/// 고칠 것만 채운다.
#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct ProjectPatch {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub favorite: Option<bool>,
    /// 통째로 바꾼다.
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    #[serde(default)]
    pub config_mode: Option<ConfigMode>,
}

/// 경로를 포함하는 프로젝트.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct Resolution {
    pub project: Project,
    /// 경로를 포함한 등록 루트.
    pub root: String,
    /// 정규화한 입력 경로.
    pub path: String,
    /// 등록되지 않은 연결 워크트리를 거쳐 메인 저장소 프로젝트로 왔으면 그 워크트리(F2-5). 창은 이것을 활성 워크트리로 연다.
    pub worktree: Option<ActiveWorktree>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct ActiveWorktree {
    /// 워크트리 꼭대기.
    pub top: String,
    /// 프로젝트 루트에 대응하는 워크트리 안의 폴더. 파일 트리 · 터미널 · 설정의 기준.
    pub root: String,
}

/// 루트 후보 하나. 순서와 이유의 규칙은 [`candidates`].
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct RootCandidate {
    pub path: String,
    /// 연결 워크트리의 메인 체크아웃 꼭대기.
    pub worktree_main: bool,
    /// 가장 가까운 VCS 루트(`.git`).
    pub vcs: bool,
    /// 이 폴더에 있는 빌드 파일 이름.
    pub build_files: Vec<String>,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum ChangeKind {
    Created,
    Updated,
    Deleted,
    Opened,
}

/// [`TOPIC_CHANGED`] 의 payload.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct ProjectChanged {
    pub kind: ChangeKind,
    pub project_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistryError {
    NotFound(String),
    /// 같은 루트가 이미 다른 프로젝트에 등록돼 있다.
    RootTaken {
        root: PathBuf,
        project_id: String,
    },
    InvalidPath {
        path: PathBuf,
        reason: String,
    },
    InvalidName,
    Db(String),
}

impl fmt::Display for RegistryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RegistryError::NotFound(id) => write!(f, "unknown project `{id}`"),
            RegistryError::RootTaken { root, project_id } => write!(
                f,
                "{} is already the root of project `{project_id}`",
                root.display()
            ),
            RegistryError::InvalidPath { path, reason } => {
                write!(f, "{}: {reason}", path.display())
            }
            RegistryError::InvalidName => f.write_str("project name must not be empty"),
            RegistryError::Db(e) => write!(f, "project registry: {e}"),
        }
    }
}

impl std::error::Error for RegistryError {}

impl From<rusqlite::Error> for RegistryError {
    fn from(e: rusqlite::Error) -> Self {
        RegistryError::Db(e.to_string())
    }
}

/// 프로젝트 레지스트리. 복제하면 같은 레지스트리를 가리킨다.
#[derive(Clone)]
pub struct Registry {
    inner: Arc<Inner>,
}

struct Inner {
    file: PathBuf,
    paths: AppPaths,
    home: Option<PathBuf>,
    busy: Duration,
    bus: EventBus,
    db: Mutex<Option<store::Opened>>,
}

impl Registry {
    /// `$DATA/state.db` 의 레지스트리. 파일은 처음 쓰일 때 연다. `home` 은 후보 추천이 너무 넓은 폴더를 거르는 데 쓴다.
    pub fn new(paths: AppPaths, home: Option<PathBuf>, bus: EventBus) -> Self {
        Self::with_busy_timeout(paths, home, bus, store::BUSY_TIMEOUT)
    }

    /// 잠긴 파일을 기다리는 시간을 정해 만든다(시험용).
    pub fn with_busy_timeout(
        paths: AppPaths,
        home: Option<PathBuf>,
        bus: EventBus,
        busy: Duration,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                file: paths.data.join(store::FILE_NAME),
                paths,
                home,
                busy,
                bus,
                db: Mutex::new(None),
            }),
        }
    }

    pub fn file(&self) -> &Path {
        &self.inner.file
    }

    /// 저장소 상태. 아직 열지 않았으면 연다.
    pub fn status(&self) -> StorageStatus {
        let mut guard = self.lock();
        Self::opened(&mut guard, &self.inner).status.clone()
    }

    /// 이름순(대소문자 무시), 같으면 만든 순.
    pub fn list(&self) -> Result<Vec<Project>, RegistryError> {
        self.read(|conn| {
            let mut projects = load(conn, None)?;
            projects.sort_by(|a, b| {
                a.name
                    .to_lowercase()
                    .cmp(&b.name.to_lowercase())
                    .then(a.created_at.cmp(&b.created_at))
            });
            Ok(projects)
        })
    }

    /// 연 적 있는 프로젝트를 최근에 연 것부터 `limit` 개.
    pub fn recent(&self, limit: usize) -> Result<Vec<Project>, RegistryError> {
        self.read(|conn| {
            let mut projects: Vec<Project> = load(conn, None)?
                .into_iter()
                .filter(|p| p.opened_at.is_some())
                .collect();
            projects.sort_by_key(|p| std::cmp::Reverse(p.opened_at));
            projects.truncate(limit);
            Ok(projects)
        })
    }

    pub fn get(&self, id: &str) -> Result<Project, RegistryError> {
        self.read(|conn| load_one(conn, id))
    }

    pub fn create(&self, new: NewProject) -> Result<Project, RegistryError> {
        let root = existing_dir(&new.root)?;
        let root_text = utf8(&root)?.to_owned();
        let name = match new.name.as_deref().map(str::trim) {
            Some(name) if !name.is_empty() => name.to_owned(),
            Some(_) => return Err(RegistryError::InvalidName),
            None => root
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| root_text.clone()),
        };
        let paths = self.inner.paths.clone();
        let project = self.write(|tx| {
            if let Some(project_id) = root_owner(tx, &root_text)? {
                return Err(RegistryError::RootTaken { root, project_id });
            }
            // 레지스트리에 없어도 개인 전용 폴더가 디스크에 남아 있으면(지운 프로젝트의 것 등) 쓰지 않는다 — 남의 설정을 물려받는다.
            let mut db_error = None;
            let slug = slug::unique(&name, |candidate| {
                let in_db = tx
                    .query_row(
                        "SELECT 1 FROM projects WHERE slug = ?1",
                        [candidate],
                        |_| Ok(()),
                    )
                    .optional();
                match in_db {
                    Ok(found) => found.is_some() || paths.personal_project_dir(candidate).exists(),
                    Err(e) => {
                        db_error.get_or_insert(e);
                        false
                    }
                }
            });
            if let Some(e) = db_error {
                return Err(e.into());
            }
            let id: String =
                tx.query_row("SELECT lower(hex(randomblob(8)))", [], |row| row.get(0))?;
            tx.execute(
                "INSERT INTO projects (id, name, host, favorite, config_mode, slug, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    id,
                    name,
                    LOCAL_HOST,
                    new.favorite,
                    new.config_mode.as_str(),
                    slug,
                    now_ms()
                ],
            )?;
            tx.execute(
                "INSERT INTO project_roots (project_id, position, path) VALUES (?1, 0, ?2)",
                params![id, root_text],
            )?;
            set_tags(tx, &id, &new.tags)?;
            load_one(tx, &id)
        })?;
        self.publish(ChangeKind::Created, &project.id);
        Ok(project)
    }

    pub fn update(&self, id: &str, patch: ProjectPatch) -> Result<Project, RegistryError> {
        let project = self.write(|tx| {
            load_one(tx, id)?;
            if let Some(name) = &patch.name {
                let name = name.trim();
                if name.is_empty() {
                    return Err(RegistryError::InvalidName);
                }
                tx.execute(
                    "UPDATE projects SET name = ?2 WHERE id = ?1",
                    params![id, name],
                )?;
            }
            if let Some(favorite) = patch.favorite {
                tx.execute(
                    "UPDATE projects SET favorite = ?2 WHERE id = ?1",
                    params![id, favorite],
                )?;
            }
            if let Some(mode) = patch.config_mode {
                tx.execute(
                    "UPDATE projects SET config_mode = ?2 WHERE id = ?1",
                    params![id, mode.as_str()],
                )?;
            }
            if let Some(tags) = &patch.tags {
                tx.execute("DELETE FROM project_tags WHERE project_id = ?1", [id])?;
                set_tags(tx, id, tags)?;
            }
            load_one(tx, id)
        })?;
        self.publish(ChangeKind::Updated, id);
        Ok(project)
    }

    /// 레지스트리에서만 지운다. 프로젝트 폴더 · `.pitwall/` · 개인 전용 설정 폴더는 그대로 둔다.
    pub fn delete(&self, id: &str) -> Result<(), RegistryError> {
        self.write(|tx| {
            let removed = tx.execute("DELETE FROM projects WHERE id = ?1", [id])?;
            if removed == 0 {
                return Err(RegistryError::NotFound(id.to_owned()));
            }
            Ok(())
        })?;
        self.publish(ChangeKind::Deleted, id);
        Ok(())
    }

    /// 「열었음」을 기록한다(최근 목록). 시각은 앞서 기록한 어느 것보다도 크게 잡아 같은 밀리초에 연 두 프로젝트의 순서도 남긴다.
    pub fn mark_opened(&self, id: &str) -> Result<Project, RegistryError> {
        let project = self.write(|tx| {
            let latest: Option<i64> =
                tx.query_row("SELECT max(opened_at) FROM projects", [], |row| row.get(0))?;
            let at = latest.map_or(now_ms(), |latest| now_ms().max(latest + 1));
            let changed = tx.execute(
                "UPDATE projects SET opened_at = ?2 WHERE id = ?1",
                params![id, at],
            )?;
            if changed == 0 {
                return Err(RegistryError::NotFound(id.to_owned()));
            }
            load_one(tx, id)
        })?;
        self.publish(ChangeKind::Opened, id);
        Ok(project)
    }

    /// `path` 를 포함하는 프로젝트(규칙은 [`resolve`]). 없으면 `None` — 그때 앱은 [`Self::candidates`] 를 보여 준다.
    pub fn resolve(&self, path: &Path) -> Result<Option<Resolution>, RegistryError> {
        let path = absolute(path)?;
        // 디스크를 읽는 워크트리 판정은 DB 락 밖에서 한다.
        let projects: Vec<Project> = self
            .read(|conn| load(conn, None))?
            .into_iter()
            .filter(|p| p.host == LOCAL_HOST)
            .collect();
        let roots: Vec<(usize, PathBuf)> = projects
            .iter()
            .enumerate()
            .flat_map(|(i, p)| p.roots.iter().map(move |r| (i, PathBuf::from(r))))
            .collect();
        let entries: Vec<resolve::RootEntry<'_, Project>> = roots
            .iter()
            .map(|(i, root)| resolve::RootEntry {
                project: &projects[*i],
                root,
            })
            .collect();
        Ok(resolve::resolve(&path, &entries).map(|m| Resolution {
            project: m.project.clone(),
            root: m.root.display().to_string(),
            path: path.display().to_string(),
            worktree: m.worktree.map(|w| ActiveWorktree {
                top: w.top.display().to_string(),
                root: w.root.display().to_string(),
            }),
        }))
    }

    /// `path` 의 루트 후보. 순서 규칙은 [`candidates`].
    pub fn candidates(&self, path: &Path) -> Result<Vec<RootCandidate>, RegistryError> {
        let path = absolute(path)?;
        let home = self.inner.home.as_deref().and_then(|h| normalize(h).ok());
        Ok(candidates::candidates(&path, home.as_deref())
            .into_iter()
            .map(|c| RootCandidate {
                path: c.path.display().to_string(),
                worktree_main: c.worktree_main,
                vcs: c.vcs,
                build_files: c.build_files,
            })
            .collect())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Option<store::Opened>> {
        self.inner.db.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn opened<'g>(slot: &'g mut Option<store::Opened>, inner: &Inner) -> &'g mut store::Opened {
        slot.get_or_insert_with(|| store::open(&inner.file, inner.busy))
    }

    fn read<T>(
        &self,
        f: impl FnOnce(&Connection) -> Result<T, RegistryError>,
    ) -> Result<T, RegistryError> {
        let mut guard = self.lock();
        f(&Self::opened(&mut guard, &self.inner).conn)
    }

    fn write<T>(
        &self,
        f: impl FnOnce(&Transaction<'_>) -> Result<T, RegistryError>,
    ) -> Result<T, RegistryError> {
        let mut guard = self.lock();
        let conn = &mut Self::opened(&mut guard, &self.inner).conn;
        let tx = conn.transaction()?;
        let out = f(&tx)?;
        tx.commit()?;
        Ok(out)
    }

    fn publish(&self, kind: ChangeKind, id: &str) {
        let payload = ProjectChanged {
            kind,
            project_id: id.to_owned(),
        };
        // 구독자가 없어도 실패가 아니다. 직렬화는 위 타입이라 실패하지 않는다.
        let _ = self.inner.bus.publish(Scope::Host, TOPIC_CHANGED, &payload);
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

fn absolute(path: &Path) -> Result<PathBuf, RegistryError> {
    normalize(path).map_err(|PathRejected::Relative| RegistryError::InvalidPath {
        path: path.to_path_buf(),
        reason: "path must be absolute".into(),
    })
}

fn existing_dir(path: &Path) -> Result<PathBuf, RegistryError> {
    let root = absolute(path)?;
    if !fs::metadata(&root).is_ok_and(|m| m.is_dir()) {
        return Err(RegistryError::InvalidPath {
            path: path.to_path_buf(),
            reason: "project root must be an existing directory".into(),
        });
    }
    Ok(root)
}

fn utf8(path: &Path) -> Result<&str, RegistryError> {
    path.to_str().ok_or_else(|| RegistryError::InvalidPath {
        path: path.to_path_buf(),
        reason: "path is not valid UTF-8".into(),
    })
}

fn root_owner(conn: &Connection, root: &str) -> rusqlite::Result<Option<String>> {
    conn.query_row(
        "SELECT project_id FROM project_roots WHERE path = ?1",
        [root],
        |row| row.get(0),
    )
    .optional()
}

fn set_tags(conn: &Connection, id: &str, tags: &[String]) -> rusqlite::Result<()> {
    for tag in tags.iter().map(|t| t.trim()).filter(|t| !t.is_empty()) {
        conn.execute(
            "INSERT OR IGNORE INTO project_tags (project_id, tag) VALUES (?1, ?2)",
            params![id, tag],
        )?;
    }
    Ok(())
}

fn load_one(conn: &Connection, id: &str) -> Result<Project, RegistryError> {
    load(conn, Some(id))?
        .pop()
        .ok_or_else(|| RegistryError::NotFound(id.to_owned()))
}

/// `only` 가 있으면 그 프로젝트만.
fn load(conn: &Connection, only: Option<&str>) -> Result<Vec<Project>, RegistryError> {
    let mut stmt = conn.prepare(
        "SELECT id, name, host, favorite, config_mode, slug, created_at, opened_at
         FROM projects WHERE ?1 IS NULL OR id = ?1",
    )?;
    let mut projects: Vec<Project> = stmt
        .query_map([only], |row| {
            Ok(Project {
                id: row.get(0)?,
                name: row.get(1)?,
                host: row.get(2)?,
                favorite: row.get(3)?,
                config_mode: ConfigMode::parse(&row.get::<_, String>(4)?)?,
                slug: row.get(5)?,
                created_at: row.get(6)?,
                opened_at: row.get(7)?,
                roots: Vec::new(),
                tags: Vec::new(),
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    let index: HashMap<String, usize> = projects
        .iter()
        .enumerate()
        .map(|(i, p)| (p.id.clone(), i))
        .collect();
    let mut roots = conn.prepare(
        "SELECT project_id, path FROM project_roots
         WHERE ?1 IS NULL OR project_id = ?1 ORDER BY project_id, position",
    )?;
    for row in roots.query_map([only], |row| Ok((row.get::<_, String>(0)?, row.get(1)?)))? {
        let (id, path) = row?;
        if let Some(&i) = index.get(&id) {
            projects[i].roots.push(path);
        }
    }
    let mut tags = conn.prepare(
        "SELECT project_id, tag FROM project_tags
         WHERE ?1 IS NULL OR project_id = ?1 ORDER BY project_id, tag",
    )?;
    for row in tags.query_map([only], |row| Ok((row.get::<_, String>(0)?, row.get(1)?)))? {
        let (id, tag) = row?;
        if let Some(&i) = index.get(&id) {
            projects[i].tags.push(tag);
        }
    }
    // 루트가 없는 행은 스키마상 생기지 않지만, 생기면 `Project::root` 가 패닉하므로 목록에서 뺀다.
    projects.retain(|p| !p.roots.is_empty());
    Ok(projects)
}

#[cfg(test)]
mod tests;
