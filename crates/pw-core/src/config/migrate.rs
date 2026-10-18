//! 설정 파일 스키마 버전과 마이그레이션 — `docs/design/03-architecture.md` 「설정 파일 스키마 버전 (D23)」.
//!
//! - 파일마다 최상위 `schema = <정수>`. 키가 없으면 v0 이다.
//! - 단계는 `v0→v1→v2…` 순서의 함수이고, [`Migrator::migrate`] 가 파일 버전부터 앱 버전까지 차례로 적용한다.
//! - 단계 함수는 `schema` 를 만지지 않는다. 각 단계 뒤에 드라이버가 올려 쓴다.
//! - 문서는 `toml_edit` 로 다룬다. 단계가 건드리지 않은 키의 주석 · 순서 · 서식은 그대로 남는다.
//!   새로 넣는 `schema` 는 최상위 값들의 맨 앞에 둔다 — 그래서 파일 머리 주석이 `schema` 줄 **아래**로 밀린다.

use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use toml_edit::{DocumentMut, Item, value};

use super::robust::labeled;

pub const SCHEMA_KEY: &str = "schema";

/// 한 단계: 버전 `n` 문서를 `n+1` 형식으로 바꾼다.
pub type Step = fn(&mut DocumentMut) -> Result<(), String>;

/// 한 종류의 파일(`project`, `settings` …)의 마이그레이션 단계 목록. `steps[n]` 이 `v{n}→v{n+1}` 이다.
#[derive(Clone, Default)]
pub struct Migrator {
    steps: Vec<Step>,
}

impl Migrator {
    /// 단계가 없는 Migrator. 현재 버전은 0 이다.
    pub fn new() -> Self {
        Self::default()
    }

    /// 다음 단계를 더한다. 더할 때마다 현재 버전이 1 오른다.
    #[must_use]
    pub fn step(mut self, step: Step) -> Self {
        self.steps.push(step);
        self
    }

    /// 앱이 쓰는(쓰는 쪽이 만들) 버전.
    pub fn current(&self) -> u32 {
        u32::try_from(self.steps.len()).expect("steps fit in u32")
    }

    /// 문서를 현재 버전으로 올린다. 이미 현재 버전이면 문서를 바꾸지 않는다.
    /// 앱보다 새 버전이면 [`MigrateError::FutureVersion`] 이고 문서를 바꾸지 않는다.
    pub fn migrate(&self, doc: &mut DocumentMut) -> Result<Migrated, MigrateError> {
        let from = schema_of(doc)?;
        let current = self.current();
        if from > current {
            return Err(MigrateError::FutureVersion {
                found: from,
                supported: current,
            });
        }
        for version in from..current {
            self.steps[version as usize](doc).map_err(|message| MigrateError::Step {
                from: version,
                message,
            })?;
            set_schema(doc, version + 1);
        }
        Ok(Migrated { from, to: current })
    }
}

/// 마이그레이션 결과. `from == to` 면 아무것도 하지 않았다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Migrated {
    pub from: u32,
    pub to: u32,
}

impl Migrated {
    pub fn changed(&self) -> bool {
        self.from != self.to
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MigrateError {
    /// `schema` 가 0 이상의 정수가 아니다.
    InvalidSchema(String),
    /// 파일이 앱보다 새 버전이다. 파일은 그대로 둔다.
    FutureVersion { found: u32, supported: u32 },
    /// `v{from}→v{from+1}` 단계가 실패했다.
    Step { from: u32, message: String },
}

impl fmt::Display for MigrateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MigrateError::InvalidSchema(found) => {
                write!(f, "`schema` must be a non-negative integer, got {found}")
            }
            MigrateError::FutureVersion { found, supported } => write!(
                f,
                "schema {found} is newer than this app supports ({supported})"
            ),
            MigrateError::Step { from, message } => {
                write!(f, "migration v{from}→v{} failed: {message}", from + 1)
            }
        }
    }
}

impl std::error::Error for MigrateError {}

/// 파일의 스키마 버전. 키가 없으면 0.
pub fn schema_of(doc: &DocumentMut) -> Result<u32, MigrateError> {
    match doc.get(SCHEMA_KEY) {
        None => Ok(0),
        Some(item) => item
            .as_integer()
            .and_then(|n| u32::try_from(n).ok())
            .ok_or_else(|| MigrateError::InvalidSchema(item.to_string().trim().to_owned())),
    }
}

fn set_schema(doc: &mut DocumentMut, version: u32) {
    let table = doc.as_table_mut();
    match table.get_mut(SCHEMA_KEY) {
        // 값만 바꾸면 그 줄의 주석 · 위치가 남는다.
        Some(Item::Value(v)) => {
            let decor = v.decor().clone();
            *v = toml_edit::Value::from(i64::from(version));
            *v.decor_mut() = decor;
        }
        _ => {
            table.insert(SCHEMA_KEY, value(i64::from(version)));
            table.sort_values_by(|k1, _, k2, _| {
                (k2.get() == SCHEMA_KEY).cmp(&(k1.get() == SCHEMA_KEY))
            });
        }
    }
}

/// 파일 종류 → Migrator.
#[derive(Clone, Default)]
pub struct Schemas {
    by_kind: HashMap<String, Migrator>,
}

impl Schemas {
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn with(mut self, kind: impl Into<String>, migrator: Migrator) -> Self {
        self.by_kind.insert(kind.into(), migrator);
        self
    }

    pub fn get(&self, kind: &str) -> Option<&Migrator> {
        self.by_kind.get(kind)
    }

    /// 앱이 아는 설정 파일 종류(설계 D23 의 목록). 아직 키 스키마가 없으므로 모두 v1 이고, v0→v1 은 `schema` 를 다는 것뿐이다.
    pub fn builtin() -> Self {
        ["project", "run", "db", "settings", "keymap", "relay"]
            .into_iter()
            .fold(Self::new(), |s, kind| {
                s.with(kind, Migrator::new().step(v0_to_v1))
            })
    }
}

/// v0 은 `schema` 키가 생기기 전의 파일이다. 키 구조는 v1 과 같다.
fn v0_to_v1(_doc: &mut DocumentMut) -> Result<(), String> {
    Ok(())
}

/// 파일 하나를 마이그레이션한 결과.
#[derive(Debug, Clone)]
pub struct FileMigration {
    pub doc: DocumentMut,
    pub migrated: Migrated,
    /// 원본을 보관한 경로. 바꾸지 않았으면 `None`.
    pub backup: Option<PathBuf>,
}

#[derive(Debug)]
pub enum FileMigrateError {
    Io {
        path: PathBuf,
        error: std::io::Error,
    },
    Parse {
        path: PathBuf,
        message: String,
    },
    Migrate {
        path: PathBuf,
        error: MigrateError,
    },
}

impl fmt::Display for FileMigrateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FileMigrateError::Io { path, error } => write!(f, "{}: {error}", path.display()),
            FileMigrateError::Parse { path, message } => {
                write!(f, "{}: {message}", path.display())
            }
            FileMigrateError::Migrate { path, error } => write!(f, "{}: {error}", path.display()),
        }
    }
}

impl std::error::Error for FileMigrateError {}

/// `path` 를 읽어 현재 버전으로 올린다. 바뀌었으면 **원본을 `backup_dir/<rel>.v<원래 버전>` 에 먼저 보관한 뒤**
/// 새 내용을 원자적으로(같은 폴더의 임시 파일 → rename) 쓴다. 보관이 실패하면 파일을 쓰지 않는다.
/// 같은 이름의 보관본이 이미 있으면 덮지 않고 `.1`, `.2` … 를 붙인다.
/// `migrator` 가 없으면(모르는 종류) 읽기만 한다.
pub fn migrate_file(
    path: &Path,
    rel: &Path,
    migrator: Option<&Migrator>,
    backup_dir: &Path,
) -> Result<FileMigration, FileMigrateError> {
    migrate_file_with(path, rel, migrator, Some(backup_dir))
}

/// [`migrate_file`] 과 같게 읽고 올리지만 **디스크에 아무것도 쓰지 않는다**(보관본 · 새 내용 모두). 올린 결과는 돌려준
/// 문서에만 있다. 설정을 소유하지 않는 읽기(MCP 세션의 노출 판정)가 쓴다.
pub fn migrate_in_memory(
    path: &Path,
    migrator: Option<&Migrator>,
) -> Result<FileMigration, FileMigrateError> {
    migrate_file_with(path, Path::new(""), migrator, None)
}

fn migrate_file_with(
    path: &Path,
    rel: &Path,
    migrator: Option<&Migrator>,
    backup_dir: Option<&Path>,
) -> Result<FileMigration, FileMigrateError> {
    let io = |error| FileMigrateError::Io {
        path: path.to_path_buf(),
        error,
    };
    let original = super::robust::retrying(|| fs::read_to_string(path))
        .map_err(labeled("read", path))
        .map_err(io)?;
    let mut doc: DocumentMut =
        original
            .parse()
            .map_err(|e: toml_edit::TomlError| FileMigrateError::Parse {
                path: path.to_path_buf(),
                message: e.to_string(),
            })?;
    let Some(migrator) = migrator else {
        let version = schema_of(&doc).unwrap_or(0);
        return Ok(FileMigration {
            doc,
            migrated: Migrated {
                from: version,
                to: version,
            },
            backup: None,
        });
    };
    let migrated = migrator
        .migrate(&mut doc)
        .map_err(|error| FileMigrateError::Migrate {
            path: path.to_path_buf(),
            error,
        })?;
    if !migrated.changed() {
        return Ok(FileMigration {
            doc,
            migrated,
            backup: None,
        });
    }
    let Some(backup_dir) = backup_dir else {
        return Ok(FileMigration {
            doc,
            migrated,
            backup: None,
        });
    };
    let backup = write_backup(backup_dir, rel, migrated.from, &original).map_err(io)?;
    write_atomic(path, &doc.to_string()).map_err(io)?;
    Ok(FileMigration {
        doc,
        migrated,
        backup: Some(backup),
    })
}

fn write_backup(dir: &Path, rel: &Path, version: u32, contents: &str) -> std::io::Result<PathBuf> {
    let base = dir.join(rel);
    if let Some(parent) = base.parent() {
        fs::create_dir_all(parent).map_err(labeled("create the backup folder", parent))?;
    }
    let name = base
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let stem = base.with_file_name(format!("{name}.v{version}"));
    let mut candidate = stem.clone();
    for n in 1.. {
        // create_new 로 열어 이미 있는 보관본을 덮지 않는다.
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(mut file) => {
                file.write_all(contents.as_bytes())
                    .and_then(|()| file.sync_all())
                    .map_err(labeled("write the backup", &candidate))?;
                return Ok(candidate);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                candidate = stem.with_file_name(format!("{name}.v{version}.{n}"));
            }
            Err(e) => return Err(labeled("create the backup", &candidate)(e)),
        }
    }
    unreachable!("1.. is unbounded")
}

/// 같은 폴더의 임시 파일에 쓰고 rename 한다. `path` 는 이미 있어야 한다. 설정 파일을 고치는 기능(언어 설정 쓰기 등)도 이것을 쓴다.
/// - `path` 가 심링크면 **링크가 가리키는 파일**을 고친다. 링크를 rename 으로 덮으면 dotfiles 연결이 끊긴다.
/// - 원래 권한을 임시 파일에 옮긴다(0600 인 파일이 0644 가 되지 않게).
/// - 임시 이름은 pid · 카운터 · 시각으로 고유하게 만든다. 고정 이름이면 같은 파일을 동시에 마이그레이션하는 두 로더
///   (메인 체크아웃과 그 워크트리가 같은 메인 `.local.toml` 을 읽는 경우, 두 앱이 `$CONFIG` 를 함께 쓰는 경우)가
///   서로의 임시 파일을 rename 해 한쪽이 ENOENT 로 실패한다.
pub fn write_atomic(path: &Path, contents: &str) -> std::io::Result<()> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    // 다른 로더가 같은 파일을 바꿔 끼우는 순간이면 Windows 에서 잠깐 「없음」 · 접근 거부가 난다(`robust`).
    let target =
        super::robust::retrying(|| fs::canonicalize(path)).map_err(labeled("resolve", path))?;
    let permissions = super::robust::retrying(|| fs::metadata(&target))
        .map_err(labeled("read the permissions of", &target))?
        .permissions();
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    loop {
        // 확장자가 `.toml` 이 아니어야 파일 감시가 임시 파일을 설정 변경으로 보지 않는다.
        let tmp = target.with_file_name(format!(
            ".{name}.{}.{}.{nanos}.pitwall-tmp",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed),
        ));
        let mut file = match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
        {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(labeled("create the temporary file", &tmp)(e)),
        };
        let written = file
            .write_all(contents.as_bytes())
            .and_then(|()| file.sync_all())
            .map_err(labeled("write the temporary file", &tmp))
            .and_then(|()| {
                fs::set_permissions(&tmp, permissions.clone())
                    .map_err(labeled("set the permissions of", &tmp))
            })
            .and_then(|()| {
                drop(file);
                super::robust::retrying(|| fs::rename(&tmp, &target)).map_err(|e| {
                    std::io::Error::new(
                        e.kind(),
                        format!("replace {} with {}: {e}", target.display(), tmp.display()),
                    )
                })
            });
        if written.is_err() {
            let _ = fs::remove_file(&tmp);
        }
        return written;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rename_old_key(doc: &mut DocumentMut) -> Result<(), String> {
        let old = doc.remove("old_key").ok_or("old_key missing")?;
        doc.insert("new_key", old);
        Ok(())
    }

    fn doc(text: &str) -> DocumentMut {
        text.parse().unwrap()
    }

    /// 완료 기준(`docs/design/11-m0-plan.md` M0-08): v0 파일이 v1 이 되고 원본이 보관되며 주석 · 순서가 남는다.
    #[test]
    fn sample_v0_to_v1_migrates_file_keeps_comments_and_backs_up() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("project.toml");
        let original = "# 팀 설정\nname = \"billing\" # 이름\n\n[languages]\n# 켜는 언어\nenabled = [\"kotlin\"]\n";
        fs::write(&file, original).unwrap();
        let backups = tmp.path().join("backup");
        let schemas = Schemas::builtin();

        let result = migrate_file(
            &file,
            Path::new("project.toml"),
            schemas.get("project"),
            &backups,
        )
        .unwrap();

        assert_eq!(result.migrated, Migrated { from: 0, to: 1 });
        let written = fs::read_to_string(&file).unwrap();
        assert_eq!(
            written,
            "schema = 1\n# 팀 설정\nname = \"billing\" # 이름\n\n[languages]\n# 켜는 언어\nenabled = [\"kotlin\"]\n"
        );
        let backup = result.backup.unwrap();
        assert_eq!(backup, backups.join("project.toml.v0"));
        assert_eq!(fs::read_to_string(&backup).unwrap(), original);

        // 다시 읽으면 할 일이 없다 — 보관본도 더 생기지 않는다.
        let again = migrate_file(
            &file,
            Path::new("project.toml"),
            schemas.get("project"),
            &backups,
        )
        .unwrap();
        assert!(!again.migrated.changed());
        assert_eq!(again.backup, None);
        assert_eq!(fs::read_dir(&backups).unwrap().count(), 1);
    }

    #[test]
    fn steps_run_in_order_from_the_file_version() {
        let migrator = Migrator::new().step(v0_to_v1).step(rename_old_key);
        assert_eq!(migrator.current(), 2);

        let mut v0 = doc("old_key = 1\n");
        assert_eq!(
            migrator.migrate(&mut v0).unwrap(),
            Migrated { from: 0, to: 2 }
        );
        assert_eq!(v0.to_string(), "schema = 2\nnew_key = 1\n");

        // v1 파일은 v1→v2 만 거친다. 기존 schema 줄의 주석은 남는다.
        let mut v1 = doc("schema = 1 # 버전\nold_key = 1\n");
        assert_eq!(
            migrator.migrate(&mut v1).unwrap(),
            Migrated { from: 1, to: 2 }
        );
        assert_eq!(v1.to_string(), "schema = 2 # 버전\nnew_key = 1\n");
    }

    #[test]
    fn failing_step_reports_its_version() {
        let migrator = Migrator::new().step(v0_to_v1).step(rename_old_key);
        let mut d = doc("schema = 1\n");
        assert_eq!(
            migrator.migrate(&mut d),
            Err(MigrateError::Step {
                from: 1,
                message: "old_key missing".into()
            })
        );
    }

    #[test]
    fn future_version_is_refused_and_file_is_left_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("settings.toml");
        fs::write(&file, "schema = 9\nx = 1\n").unwrap();
        let schemas = Schemas::builtin();
        let err = migrate_file(
            &file,
            Path::new("settings.toml"),
            schemas.get("settings"),
            &tmp.path().join("backup"),
        )
        .unwrap_err();
        assert!(matches!(
            err,
            FileMigrateError::Migrate {
                error: MigrateError::FutureVersion {
                    found: 9,
                    supported: 1
                },
                ..
            }
        ));
        assert_eq!(fs::read_to_string(&file).unwrap(), "schema = 9\nx = 1\n");
        assert!(!tmp.path().join("backup").exists());
    }

    #[test]
    fn invalid_schema_values_are_errors() {
        for text in ["schema = \"1\"\n", "schema = -1\n", "schema = 1.0\n"] {
            assert!(matches!(
                schema_of(&doc(text)),
                Err(MigrateError::InvalidSchema(_))
            ));
        }
    }

    /// 설계 D23: 원본 보관이 실패하면 파일을 쓰지 않는다.
    #[test]
    fn failed_backup_leaves_the_original_untouched() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("project.toml");
        fs::write(&file, "name = \"v0\"\n").unwrap();
        // 보관 폴더 자리에 파일을 둬서 create_dir_all 이 실패하게 한다.
        let backups = tmp.path().join("backup");
        fs::write(&backups, "not a dir").unwrap();
        let err = migrate_file(
            &file,
            Path::new("project.toml"),
            Schemas::builtin().get("project"),
            &backups,
        )
        .unwrap_err();
        assert!(matches!(err, FileMigrateError::Io { .. }), "{err}");
        assert_eq!(fs::read_to_string(&file).unwrap(), "name = \"v0\"\n");
    }

    #[cfg(unix)]
    #[test]
    fn rewrite_keeps_permissions_and_symlinks() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let dotfiles = tmp.path().join("dotfiles");
        fs::create_dir_all(&dotfiles).unwrap();
        let target = dotfiles.join("settings.toml");
        fs::write(&target, "x = 1\n").unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
        let config = tmp.path().join("config");
        fs::create_dir_all(&config).unwrap();
        let link = config.join("settings.toml");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let result = migrate_file(
            &link,
            Path::new("settings.toml"),
            Schemas::builtin().get("settings"),
            &tmp.path().join("backup"),
        )
        .unwrap();
        assert!(result.migrated.changed());
        // 링크는 링크로 남고, 고쳐진 것은 대상 파일이다.
        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read_to_string(&target).unwrap(), "schema = 1\nx = 1\n");
        assert_eq!(
            fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::read_to_string(result.backup.unwrap()).unwrap(),
            "x = 1\n"
        );
        // 임시 파일이 남지 않는다.
        assert_eq!(fs::read_dir(&dotfiles).unwrap().count(), 1);
    }

    #[test]
    fn existing_backup_is_not_overwritten() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("b");
        let rel = Path::new("run/dev.toml");
        let first = write_backup(&dir, rel, 0, "first").unwrap();
        let second = write_backup(&dir, rel, 0, "second").unwrap();
        assert_eq!(first, dir.join("run/dev.toml.v0"));
        assert_eq!(second, dir.join("run/dev.toml.v0.1"));
        assert_eq!(fs::read_to_string(first).unwrap(), "first");
    }

    /// 설정 파일 하나의 오류는 모두 그 파일 경로로 보고되므로, io 오류 메시지에 실패한 연산과 실제 경로를 남긴다(Windows CI 의
    /// 「파일을 찾을 수 없다」가 어느 연산에서 났는지 알 수 없었다). 종류는 그대로다.
    #[test]
    fn io_errors_name_the_operation_and_the_real_path() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("missing.toml");
        let error = write_atomic(&missing, "x").unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
        assert!(
            error
                .to_string()
                .starts_with(&format!("resolve {}: ", missing.display())),
            "{error}"
        );

        // 보관 폴더 자리에 파일이 있다.
        let blocked = tmp.path().join("backup");
        fs::write(&blocked, "").unwrap();
        let error = write_backup(&blocked, Path::new("run/dev.toml"), 0, "x").unwrap_err();
        assert!(
            error.to_string().starts_with(&format!(
                "create the backup folder {}: ",
                blocked.join("run").display()
            )),
            "{error}"
        );
    }
}
