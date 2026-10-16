//! 설정 파일을 찾아 읽고(마이그레이션 포함) 레이어를 병합한 스냅숏을 만든다.
//!
//! 한 설정 파일 id(`project`, `run/dev-server`, `settings` …)의 레이어 순서:
//! 내장 기본값 → `<id>.toml` → `<id>.local.toml` (`docs/design/03-architecture.md` 「5. 데이터 저장」 병합 순서).
//! 병합 규칙은 [`super::merge::merge`].

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use toml_edit::DocumentMut;

use super::gitdir::linked_worktree;
use super::merge::{merge, to_json};
use super::migrate::{
    FileMigrateError, MigrateError, SCHEMA_KEY, Schemas, migrate_file, schema_of,
};
use super::paths::AppPaths;

/// 프로젝트 설정 폴더 이름.
pub const PROJECT_DIR: &str = ".pitwall";
/// 프로젝트 설정 폴더 안의 마이그레이션 원본 보관 폴더(설계 D23 `.pitwall/.backup/<파일>.v<N>`).
pub const PROJECT_BACKUP_DIR: &str = ".backup";
/// 프로젝트 설정 폴더에서 한 단계 아래까지 읽는 하위 폴더.
const PROJECT_SUBDIRS: &[&str] = &["run", "db"];

/// 내장 기본값 — 키를 정한 기능은 자기 기본값을 여기에 둔다. 아직 키가 없는 자리는 **예시 키**(`sample`)만 있다.
/// - `diagnostics.crash_reports`: 크래시 리포트 전송(opt-in, 설계 D22). 데스크톱이 읽는다(`apps/desktop/src-tauri/src/diag.rs`).
const BUILTIN_PROJECT: &str = "schema = 1\n\n[sample]\ngreeting = \"hello\"\n";
const BUILTIN_SETTINGS: &str =
    "schema = 1\n\n[sample]\ngreeting = \"hello\"\n\n[diagnostics]\ncrash_reports = false\n";

/// 스냅숏: 파일 id 마다 병합된 값과, 이번 읽기에서 생긴 알림.
#[derive(
    Debug, Clone, Default, PartialEq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct ConfigSnapshot {
    /// id 순으로 정렬돼 있다.
    pub files: Vec<ConfigFile>,
    pub notices: Vec<ConfigNotice>,
}

impl ConfigSnapshot {
    pub fn file(&self, id: &str) -> Option<&ConfigFile> {
        self.files.iter().find(|f| f.id == id)
    }

    /// `id` 파일에서 점으로 이은 키(`sample.greeting`)의 값.
    pub fn get(&self, id: &str, dotted: &str) -> Option<&Value> {
        dotted
            .split('.')
            .try_fold(&self.file(id)?.value, |v, key| v.get(key))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ConfigFile {
    /// 설정 폴더 기준 경로에서 `.toml`/`.local.toml` 을 뗀 것. 구분자는 항상 `/`.
    pub id: String,
    /// 레이어를 병합한 값(JSON 객체). `schema` 키는 뺀다.
    #[specta(type = pw_rpc::JsonValue)]
    pub value: Value,
    /// 아래(먼저 적용)부터 위 순서.
    pub layers: Vec<ConfigLayer>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct ConfigLayer {
    pub origin: LayerOrigin,
    /// 파일 경로. 내장 기본값이면 `null`.
    pub path: Option<String>,
    pub schema: u32,
    /// 앱보다 새 스키마라 읽기만 하고 쓰지 않는 파일.
    pub read_only: bool,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum LayerOrigin {
    Builtin,
    /// `<id>.toml` — 커밋되는 공유 설정(전역이면 그냥 설정 파일).
    Shared,
    /// `<id>.local.toml` — 개인 덮어쓰기. 워크트리면 메인 체크아웃의 것일 수 있다.
    Local,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ConfigNotice {
    /// 구버전 파일을 올려 썼다. 커밋되는 파일이면 「변경 사항 커밋 필요」 안내 대상이다(설계 D23).
    Migrated {
        path: String,
        from: u32,
        to: u32,
        backup: String,
    },
    /// 앱보다 새 버전이라 읽기 전용으로 열었다(업데이트 안내 대상).
    FutureSchema {
        path: String,
        found: u32,
        supported: u32,
    },
    /// 가리키는 파일이 없는 심링크라 건너뛰었다(dotfiles 가 아직 동기화되지 않은 경우 등).
    BrokenLink { path: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    Io {
        path: PathBuf,
        message: String,
    },
    Parse {
        path: PathBuf,
        message: String,
    },
    Migrate {
        path: PathBuf,
        error: MigrateError,
    },
    /// 개인 전용 slug 가 경로 한 칸이 아니다.
    InvalidSlug(String),
}

impl ConfigError {
    /// 문제가 난 파일. 파일과 무관한 오류면 `None`.
    pub fn path(&self) -> Option<&Path> {
        match self {
            ConfigError::Io { path, .. }
            | ConfigError::Parse { path, .. }
            | ConfigError::Migrate { path, .. } => Some(path),
            ConfigError::InvalidSlug(_) => None,
        }
    }
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::Io { path, message } | ConfigError::Parse { path, message } => {
                write!(f, "{}: {message}", path.display())
            }
            ConfigError::Migrate { path, error } => write!(f, "{}: {error}", path.display()),
            ConfigError::InvalidSlug(slug) => write!(f, "invalid project slug `{slug}`"),
        }
    }
}

impl std::error::Error for ConfigError {}

impl From<FileMigrateError> for ConfigError {
    fn from(e: FileMigrateError) -> Self {
        match e {
            FileMigrateError::Io { path, error } => ConfigError::Io {
                path,
                message: error.to_string(),
            },
            FileMigrateError::Parse { path, message } => ConfigError::Parse { path, message },
            FileMigrateError::Migrate { path, error } => ConfigError::Migrate { path, error },
        }
    }
}

/// 설정 파일을 읽는 폴더와, 그 폴더 파일의 마이그레이션 원본을 보관할 폴더.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ScanDir {
    dir: PathBuf,
    backup: PathBuf,
}

/// 어디서 무엇을 읽는가. [`ConfigSource::load`] 가 매번 디스크를 새로 읽는다.
#[derive(Clone)]
pub struct ConfigSource {
    primary: ScanDir,
    /// `<id>.local.toml` 이 `primary` 에 없을 때 대신 읽는 폴더(워크트리 → 메인 체크아웃).
    local_fallback: Option<ScanDir>,
    subdirs: &'static [&'static str],
    defaults: BTreeMap<String, String>,
    schemas: Arc<Schemas>,
}

impl ConfigSource {
    /// 저장소에 두는 프로젝트 설정: `<root>/.pitwall/`.
    ///
    /// `root` 가 연결 워크트리(`git worktree add`) 안이면 메인 체크아웃의 같은 위치 `.pitwall/` 을 `.local.toml` 폴백으로 쓴다.
    /// 폴백은 **파일 단위**다 — 워크트리에 `<id>.local.toml` 이 있으면 그것만 쓰고 메인 쪽 같은 id 는 읽지 않는다.
    pub fn project(root: &Path) -> Self {
        let dir = root.join(PROJECT_DIR);
        // 워크트리 판정은 실제 경로로 한다. 워크트리 하위 폴더를 가리키는 심링크로 열면 조상에서 `.git` 을 못 찾는다.
        let real_root = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
        let local_fallback = linked_worktree(&real_root)
            .and_then(|link| link.map_to_main(&real_root))
            .map(|main_root| {
                let dir = main_root.join(PROJECT_DIR);
                ScanDir {
                    backup: dir.join(PROJECT_BACKUP_DIR),
                    dir,
                }
            });
        Self {
            primary: ScanDir {
                backup: dir.join(PROJECT_BACKUP_DIR),
                dir,
            },
            local_fallback,
            subdirs: PROJECT_SUBDIRS,
            defaults: BTreeMap::from([("project".to_owned(), BUILTIN_PROJECT.to_owned())]),
            schemas: Arc::new(Schemas::builtin()),
        }
    }

    /// 「개인 전용」 프로젝트 설정: `.pitwall/` 과 같은 구조를 `$CONFIG/projects/<slug>/` 에 둔다(설계 §5).
    /// slug 는 프로젝트 레지스트리(M0-09)가 정한다. 경로 한 칸(`[A-Za-z0-9._-]`, `.` 로 시작하지 않음)만 받는다.
    ///
    /// 원본 보관은 `$STATE/backup/projects/<slug>/` — `$CONFIG` 에는 사람이 편집하는 TOML 만 두므로(§5-0) 그 안에 두지 않는다.
    pub fn personal(paths: &AppPaths, slug: &str) -> Result<Self, ConfigError> {
        let valid = !slug.is_empty()
            && !slug.starts_with('.')
            && slug
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
        if !valid {
            return Err(ConfigError::InvalidSlug(slug.to_owned()));
        }
        Ok(Self {
            primary: ScanDir {
                dir: paths.personal_project_dir(slug),
                backup: paths.backup_dir().join("projects").join(slug),
            },
            local_fallback: None,
            subdirs: PROJECT_SUBDIRS,
            defaults: BTreeMap::from([("project".to_owned(), BUILTIN_PROJECT.to_owned())]),
            schemas: Arc::new(Schemas::builtin()),
        })
    }

    /// 전역 설정: `$CONFIG/*.toml`(하위 폴더 `projects/` · `themes/` 는 읽지 않는다). 원본 보관은 `$STATE/backup/`.
    pub fn global(paths: &AppPaths) -> Self {
        Self {
            primary: ScanDir {
                dir: paths.config.clone(),
                backup: paths.backup_dir(),
            },
            local_fallback: None,
            subdirs: &[],
            defaults: BTreeMap::from([("settings".to_owned(), BUILTIN_SETTINGS.to_owned())]),
            schemas: Arc::new(Schemas::builtin()),
        }
    }

    /// 내장 기본값을 바꾼다(id → TOML 원문).
    #[must_use]
    pub fn with_defaults<I, K, V>(mut self, defaults: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        self.defaults = defaults
            .into_iter()
            .map(|(k, v)| (k.into(), v.into()))
            .collect();
        self
    }

    #[must_use]
    pub fn with_schemas(mut self, schemas: Schemas) -> Self {
        self.schemas = Arc::new(schemas);
        self
    }

    pub fn dir(&self) -> &Path {
        &self.primary.dir
    }

    pub fn local_fallback_dir(&self) -> Option<&Path> {
        self.local_fallback.as_ref().map(|s| s.dir.as_path())
    }

    /// 감시할 폴더와 재귀 여부. 없는 폴더는 생기는 것을 보도록 가장 가까운 있는 조상을 비재귀로 본다.
    pub(crate) fn watch_targets(&self) -> Vec<(PathBuf, bool)> {
        let recursive = !self.subdirs.is_empty();
        std::iter::once(&self.primary)
            .chain(self.local_fallback.as_ref())
            .filter_map(|scan| {
                if scan.dir.is_dir() {
                    Some((scan.dir.clone(), recursive))
                } else {
                    scan.dir
                        .ancestors()
                        .skip(1)
                        .find(|a| a.is_dir())
                        .map(|a| (a.to_path_buf(), false))
                }
            })
            .collect()
    }

    /// 디스크에서 다시 읽는다. 구버전 파일은 이 자리에서 마이그레이션해 쓴다(원본 보관 후).
    /// 파일 하나라도 읽기 · 파싱 · 마이그레이션에 실패하면 스냅숏 전체가 오류다 — 일부만 반영된 설정을 만들지 않는다.
    pub fn load(&self) -> Result<ConfigSnapshot, ConfigError> {
        let mut entries: BTreeMap<String, Entry<'_>> = BTreeMap::new();
        let mut broken = Vec::new();
        for found in list(&self.primary.dir, self.subdirs, &mut broken)? {
            let entry = entries.entry(found.id.clone()).or_default();
            if found.local {
                entry.local = Some((found, &self.primary));
            } else {
                entry.shared = Some((found, &self.primary));
            }
        }
        if let Some(fallback) = &self.local_fallback {
            for found in list(&fallback.dir, self.subdirs, &mut broken)? {
                if found.local {
                    let entry = entries.entry(found.id.clone()).or_default();
                    if entry.local.is_none() {
                        entry.local = Some((found, fallback));
                    }
                }
            }
        }
        for id in self.defaults.keys() {
            entries.entry(id.clone()).or_default();
        }

        let mut snapshot = ConfigSnapshot::default();
        snapshot
            .notices
            .extend(broken.into_iter().map(|p| ConfigNotice::BrokenLink {
                path: p.display().to_string(),
            }));
        for (id, entry) in entries {
            let mut value = Map::new();
            let mut layers = Vec::new();
            self.apply_builtin(&id, &mut value, &mut layers);
            let kind = id.split('/').next().unwrap_or(&id);
            let migrator = self.schemas.get(kind);
            for (origin, found) in [
                (LayerOrigin::Shared, entry.shared),
                (LayerOrigin::Local, entry.local),
            ] {
                let Some((found, scan)) = found else { continue };
                let path = scan.dir.join(&found.rel);
                let display = path.display().to_string();
                let (doc, read_only) = match migrate_file(&path, &found.rel, migrator, &scan.backup)
                {
                    Ok(m) => {
                        if let Some(backup) = &m.backup {
                            snapshot.notices.push(ConfigNotice::Migrated {
                                path: display.clone(),
                                from: m.migrated.from,
                                to: m.migrated.to,
                                backup: backup.display().to_string(),
                            });
                        }
                        (m.doc, false)
                    }
                    Err(FileMigrateError::Migrate {
                        error: MigrateError::FutureVersion { found, supported },
                        ..
                    }) => {
                        snapshot.notices.push(ConfigNotice::FutureSchema {
                            path: display.clone(),
                            found,
                            supported,
                        });
                        (read_doc(&path)?, true)
                    }
                    Err(e) => return Err(e.into()),
                };
                layers.push(ConfigLayer {
                    origin,
                    path: Some(display),
                    schema: schema_of(&doc).unwrap_or(0),
                    read_only,
                });
                merge(&mut value, without_schema(&doc));
            }
            snapshot.files.push(ConfigFile {
                id,
                value: Value::Object(value),
                layers,
            });
        }
        Ok(snapshot)
    }
}

impl ConfigSource {
    /// 파일을 읽지 않고 내장 기본값만으로 만든 스냅숏. 처음 읽기가 실패했을 때 쓴다.
    pub fn builtin_snapshot(&self) -> ConfigSnapshot {
        let files = self
            .defaults
            .keys()
            .map(|id| {
                let mut value = Map::new();
                let mut layers = Vec::new();
                self.apply_builtin(id, &mut value, &mut layers);
                ConfigFile {
                    id: id.clone(),
                    value: Value::Object(value),
                    layers,
                }
            })
            .collect();
        ConfigSnapshot {
            files,
            notices: Vec::new(),
        }
    }

    fn apply_builtin(
        &self,
        id: &str,
        value: &mut Map<String, Value>,
        layers: &mut Vec<ConfigLayer>,
    ) {
        let Some(text) = self.defaults.get(id) else {
            return;
        };
        // 내장 원문이 깨진 것은 빌드 결함이다. 테스트(`builtin_defaults_parse`)가 잡는다.
        let doc: DocumentMut = text.parse().expect("builtin defaults are valid TOML");
        layers.push(ConfigLayer {
            origin: LayerOrigin::Builtin,
            path: None,
            schema: schema_of(&doc).unwrap_or(0),
            read_only: false,
        });
        merge(value, without_schema(&doc));
    }
}

#[derive(Default)]
struct Entry<'a> {
    shared: Option<(Found, &'a ScanDir)>,
    local: Option<(Found, &'a ScanDir)>,
}

struct Found {
    id: String,
    /// 설정 폴더 기준 경로.
    rel: PathBuf,
    local: bool,
}

/// `dir/*.toml` 과 `dir/<sub>/*.toml`. 숨김 파일(`.` 으로 시작)은 건너뛴다. 폴더가 없으면 빈 목록.
/// 심링크는 따라가서 판정한다 — dotfiles 도구(stow · yadm · chezmoi 링크 모드)가 `$CONFIG/*.toml` 을 링크로 둔다(§5-0).
/// 대상이 없는 심링크는 `broken` 에 모은다(조용히 빠지면 사용자에게는 「설정이 안 먹는다」 로만 보인다).
fn list(
    dir: &Path,
    subdirs: &[&str],
    broken: &mut Vec<PathBuf>,
) -> Result<Vec<Found>, ConfigError> {
    let mut out = Vec::new();
    let mut scan = |rel_dir: Option<&str>, out: &mut Vec<Found>| -> Result<(), ConfigError> {
        let here = rel_dir.map_or_else(|| dir.to_path_buf(), |r| dir.join(r));
        let read = match fs::read_dir(&here) {
            Ok(read) => read,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => {
                return Err(ConfigError::Io {
                    path: here,
                    message: e.to_string(),
                });
            }
        };
        for item in read {
            let item = item.map_err(|e| ConfigError::Io {
                path: here.clone(),
                message: e.to_string(),
            })?;
            let name = item.file_name();
            let Some(name) = name.to_str() else { continue };
            let Some(stem) = name.strip_suffix(".toml") else {
                continue;
            };
            if name.starts_with('.') {
                continue;
            }
            match fs::metadata(item.path()) {
                Ok(meta) if meta.is_file() => {}
                Ok(_) => continue,
                Err(e)
                    if e.kind() == std::io::ErrorKind::NotFound
                        && item.file_type().is_ok_and(|t| t.is_symlink()) =>
                {
                    broken.push(item.path());
                    continue;
                }
                Err(e) => {
                    return Err(ConfigError::Io {
                        path: item.path(),
                        message: e.to_string(),
                    });
                }
            }
            let (stem, local) = match stem.strip_suffix(".local") {
                Some(s) => (s, true),
                None => (stem, false),
            };
            let (id, rel) = match rel_dir {
                Some(r) => (format!("{r}/{stem}"), Path::new(r).join(name)),
                None => (stem.to_owned(), PathBuf::from(name)),
            };
            out.push(Found { id, rel, local });
        }
        Ok(())
    };
    scan(None, &mut out)?;
    for sub in subdirs {
        scan(Some(sub), &mut out)?;
    }
    Ok(out)
}

fn read_doc(path: &Path) -> Result<DocumentMut, ConfigError> {
    let text = fs::read_to_string(path).map_err(|e| ConfigError::Io {
        path: path.to_path_buf(),
        message: e.to_string(),
    })?;
    text.parse()
        .map_err(|e: toml_edit::TomlError| ConfigError::Parse {
            path: path.to_path_buf(),
            message: e.to_string(),
        })
}

fn without_schema(doc: &DocumentMut) -> Map<String, Value> {
    let mut map = to_json(doc.as_table());
    map.remove(SCHEMA_KEY);
    map
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    #[test]
    fn builtin_defaults_parse() {
        for text in [BUILTIN_PROJECT, BUILTIN_SETTINGS] {
            text.parse::<DocumentMut>().unwrap();
        }
    }

    #[test]
    fn layers_merge_builtin_then_shared_then_local() {
        let tmp = tempfile::tempdir().unwrap();
        let pw = tmp.path().join(PROJECT_DIR);
        write(
            &pw.join("project.toml"),
            "schema = 1\nname = \"shared\"\n[sample]\ngreeting = \"from shared\"\nport = 1\n",
        );
        write(
            &pw.join("project.local.toml"),
            "schema = 1\n[sample]\nport = 2\n",
        );
        write(&pw.join("run/dev.toml"), "schema = 1\ncommand = \"a\"\n");
        write(
            &pw.join("run/mine.local.toml"),
            "schema = 1\ncommand = \"b\"\n",
        );
        write(&pw.join("notes.txt"), "ignored");

        let snap = ConfigSource::project(tmp.path()).load().unwrap();
        let ids: Vec<_> = snap.files.iter().map(|f| f.id.as_str()).collect();
        assert_eq!(ids, ["project", "run/dev", "run/mine"]);
        assert_eq!(
            snap.file("project").unwrap().value,
            json!({"name": "shared", "sample": {"greeting": "from shared", "port": 2}})
        );
        let origins: Vec<_> = snap
            .file("project")
            .unwrap()
            .layers
            .iter()
            .map(|l| l.origin)
            .collect();
        assert_eq!(
            origins,
            [
                LayerOrigin::Builtin,
                LayerOrigin::Shared,
                LayerOrigin::Local
            ]
        );
        assert_eq!(snap.get("run/mine", "command"), Some(&json!("b")));
        assert!(snap.notices.is_empty());
    }

    #[test]
    fn missing_folder_yields_builtin_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        let snap = ConfigSource::project(tmp.path()).load().unwrap();
        assert_eq!(
            snap.get("project", "sample.greeting"),
            Some(&json!("hello"))
        );
    }

    #[test]
    fn parse_error_names_the_file() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join(PROJECT_DIR).join("project.toml");
        write(&file, "name = \n");
        let err = ConfigSource::project(tmp.path()).load().unwrap_err();
        assert!(
            matches!(&err, ConfigError::Parse { path, .. } if *path == file),
            "{err}"
        );
    }

    /// 실제 git 워크트리와 같은 모양(`.git` 파일 → `<main>/.git/worktrees/<이름>` + `commondir`).
    fn fake_worktree(root: &Path) -> (PathBuf, PathBuf) {
        let main = root.join("main");
        let wt = root.join("wt");
        let admin = main.join(".git/worktrees/wt");
        fs::create_dir_all(&admin).unwrap();
        fs::create_dir_all(&wt).unwrap();
        fs::write(admin.join("commondir"), "../..\n").unwrap();
        fs::write(wt.join(".git"), format!("gitdir: {}\n", admin.display())).unwrap();
        (main, wt)
    }

    #[test]
    fn worktree_falls_back_to_main_checkout_local_files() {
        let tmp = tempfile::tempdir().unwrap();
        let (main, wt) = fake_worktree(tmp.path());
        write(
            &wt.join(".pitwall/project.toml"),
            "schema = 1\nname = \"wt\"\n",
        );
        write(
            &main.join(".pitwall/project.toml"),
            "schema = 1\nname = \"main shared\"\n",
        );
        write(
            &main.join(".pitwall/project.local.toml"),
            "schema = 1\nport = 5432\n",
        );
        write(
            &main.join(".pitwall/db/pg.local.toml"),
            "schema = 1\nuser = \"me\"\n",
        );

        let source = ConfigSource::project(&wt);
        assert!(source.local_fallback_dir().is_some());
        let snap = source.load().unwrap();
        // 공유 파일은 워크트리 것만, local 은 메인 체크아웃 것을 쓴다.
        assert_eq!(
            snap.file("project").unwrap().value,
            json!({"name": "wt", "port": 5432, "sample": {"greeting": "hello"}})
        );
        assert_eq!(snap.get("db/pg", "user"), Some(&json!("me")));

        // 워크트리에 자기 local 이 있으면 그 파일만 쓴다(메인 쪽과 병합하지 않는다).
        write(
            &wt.join(".pitwall/project.local.toml"),
            "schema = 1\nport = 1\n",
        );
        let snap = source.load().unwrap();
        assert_eq!(snap.get("project", "port"), Some(&json!(1)));
        let local = snap.file("project").unwrap().layers.last().unwrap().clone();
        let own = wt.join(".pitwall/project.local.toml");
        assert_eq!(local.path, Some(own.display().to_string()));
    }

    #[test]
    fn old_files_are_migrated_with_project_backup_and_notice() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join(".pitwall/run/dev.toml");
        write(&file, "# 개발 서버\ncommand = \"x\"\n");
        let snap = ConfigSource::project(tmp.path()).load().unwrap();
        let backup = tmp.path().join(".pitwall/.backup/run/dev.toml.v0");
        assert_eq!(
            snap.notices,
            [ConfigNotice::Migrated {
                path: file.display().to_string(),
                from: 0,
                to: 1,
                backup: backup.display().to_string(),
            }]
        );
        assert_eq!(
            fs::read_to_string(&backup).unwrap(),
            "# 개발 서버\ncommand = \"x\"\n"
        );
        assert!(
            fs::read_to_string(&file)
                .unwrap()
                .starts_with("schema = 1\n")
        );
        // 보관 폴더(`.backup`)는 설정으로 읽히지 않는다.
        assert_eq!(snap.files.len(), 2);
    }

    #[test]
    fn future_schema_is_read_only_and_not_rewritten() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join(".pitwall/project.toml");
        write(&file, "schema = 7\nname = \"new\"\n");
        let snap = ConfigSource::project(tmp.path()).load().unwrap();
        assert_eq!(snap.get("project", "name"), Some(&json!("new")));
        assert!(snap.file("project").unwrap().layers[1].read_only);
        assert!(matches!(
            snap.notices[..],
            [ConfigNotice::FutureSchema {
                found: 7,
                supported: 1,
                ..
            }]
        ));
        assert_eq!(
            fs::read_to_string(&file).unwrap(),
            "schema = 7\nname = \"new\"\n"
        );
    }

    #[test]
    fn personal_mode_reads_config_projects_slug_and_backs_up_to_state() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::under(tmp.path());
        let dir = paths.config.join("projects/billing");
        write(&dir.join("project.toml"), "name = \"personal\"\n");
        let source = ConfigSource::personal(&paths, "billing").unwrap();
        assert_eq!(source.dir(), dir);
        let snap = source.load().unwrap();
        assert_eq!(snap.get("project", "name"), Some(&json!("personal")));
        assert!(
            paths
                .state
                .join("backup/projects/billing/project.toml.v0")
                .is_file()
        );
        assert!(!dir.join(PROJECT_BACKUP_DIR).exists());

        for bad in ["", "..", ".hidden", "a/b", "a\\b"] {
            assert_eq!(
                ConfigSource::personal(&paths, bad).err(),
                Some(ConfigError::InvalidSlug(bad.to_owned()))
            );
        }
    }

    /// dotfiles 도구가 둔 심링크를 따라 읽고, 대상이 없는 링크는 알림으로 남긴다(§5-0 dotfiles 동기화).
    #[cfg(unix)]
    #[test]
    fn symlinked_files_are_read_and_broken_links_are_reported() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::under(tmp.path());
        let dotfiles = tmp.path().join("dotfiles");
        write(
            &dotfiles.join("settings.toml"),
            "schema = 1\n[sample]\ngreeting = \"from dotfiles\"\n",
        );
        fs::create_dir_all(&paths.config).unwrap();
        std::os::unix::fs::symlink(
            dotfiles.join("settings.toml"),
            paths.config.join("settings.toml"),
        )
        .unwrap();
        let missing = paths.config.join("keymap.toml");
        std::os::unix::fs::symlink(dotfiles.join("absent.toml"), &missing).unwrap();

        let snap = ConfigSource::global(&paths).load().unwrap();
        assert_eq!(
            snap.get("settings", "sample.greeting"),
            Some(&json!("from dotfiles"))
        );
        assert_eq!(
            snap.notices,
            [ConfigNotice::BrokenLink {
                path: missing.display().to_string()
            }]
        );
        assert!(snap.file("keymap").is_none());
    }

    /// 같은 v0 파일을 여러 로더가 동시에 읽어도(메인 체크아웃과 워크트리가 같은 메인 `.local.toml` 을 여는 경우) 실패하지 않는다.
    #[test]
    fn concurrent_loads_migrating_the_same_file_all_succeed() {
        const THREADS: usize = 8;
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join(".pitwall/project.toml");
        for round in 0..20 {
            write(&file, &format!("name = \"r{round}\"\n"));
            let barrier = std::sync::Barrier::new(THREADS);
            let errors: Vec<_> = std::thread::scope(|s| {
                let handles: Vec<_> = (0..THREADS)
                    .map(|_| {
                        s.spawn(|| {
                            barrier.wait();
                            ConfigSource::project(tmp.path()).load().err()
                        })
                    })
                    .collect();
                handles
                    .into_iter()
                    .filter_map(|h| h.join().unwrap())
                    .collect()
            });
            assert!(errors.is_empty(), "round {round}: {errors:?}");
            assert_eq!(
                fs::read_to_string(&file).unwrap(),
                format!("schema = 1\nname = \"r{round}\"\n")
            );
        }
        // 임시 파일이 남지 않는다.
        let leftovers: Vec<_> = fs::read_dir(tmp.path().join(".pitwall"))
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .filter(|n| n.to_string_lossy().ends_with(".pitwall-tmp"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }

    /// 워크트리 하위 폴더를 가리키는 심링크로 열어도 메인 체크아웃 폴백을 찾는다.
    #[cfg(unix)]
    #[test]
    fn symlink_into_a_worktree_subfolder_keeps_the_fallback() {
        let tmp = tempfile::tempdir().unwrap();
        let (main, wt) = fake_worktree(tmp.path());
        fs::create_dir_all(wt.join("sub")).unwrap();
        write(
            &main.join("sub/.pitwall/project.local.toml"),
            "schema = 1\nport = 7\n",
        );
        let link = tmp.path().join("link");
        std::os::unix::fs::symlink(wt.join("sub"), &link).unwrap();

        let snap = ConfigSource::project(&link).load().unwrap();
        assert_eq!(snap.get("project", "port"), Some(&json!(7)));
    }

    #[test]
    fn global_reads_top_level_only_and_backs_up_to_state() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::under(tmp.path());
        write(
            &paths.config.join("settings.toml"),
            "[sample]\ngreeting = \"hi\"\n",
        );
        write(
            &paths.config.join("settings.local.toml"),
            "schema = 1\nmachine = \"laptop\"\n",
        );
        write(
            &paths.config.join("projects/x/project.toml"),
            "schema = 1\n",
        );
        let snap = ConfigSource::global(&paths).load().unwrap();
        let ids: Vec<_> = snap.files.iter().map(|f| f.id.as_str()).collect();
        assert_eq!(ids, ["settings"]);
        assert_eq!(
            snap.file("settings").unwrap().value,
            json!({
                "sample": {"greeting": "hi"},
                "machine": "laptop",
                "diagnostics": {"crash_reports": false}
            })
        );
        assert!(paths.state.join("backup/settings.toml.v0").is_file());
    }
}
