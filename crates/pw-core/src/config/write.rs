//! 전역 설정 파일 한 키 쓰기: `$CONFIG/settings.toml` 의 `표.키` 하나만 고치거나 지운다. 언어 설정(`pw-i18n` 의 `ui.language`)과
//! 설정 창의 DB 페이지(`pw-db` 의 `db.settings.set`, 14-m3-plan.md M3-20)가 쓴다. 값의 판정은 부르는 쪽의 몫이다 — 여기는 파일만
//! 다룬다.
//!
//! - 주석 · 서식 · 다른 키는 그대로 둔다(toml_edit). 값이 이미 같으면 쓰지 않는다. 있던 값의 앞뒤 공백 · 꼬리 주석은 남긴다.
//! - 파일이 없으면 `schema = 1` 과 그 키만으로 만든다(지우기면 만들지 않는다). 파일을 읽거나 TOML 로 읽을 수 없으면 **쓰지 않고**
//!   오류다 — 사람이 고치다 만 파일을 덮지 않는다. 설정 계층이 손대지 않는 파일(`schema` 가 앱보다 새 버전이거나 정수가 아님,
//!   [`super::migrate`])도 쓰지 않는다 — 새 버전 Pitwall 이 쓴 파일에 옛 모양의 키를 섞지 않는다. 옛 버전(`schema` 없음)은 고쳐
//!   쓴다 — 다시 읽을 때 설정 계층이 마이그레이션한다.
//! - 심링크는 링크가 아니라 대상 파일을 고친다(dotfiles 연결을 끊지 않는다). 대상이 없는 링크는 오류다.
//! - 같은 파일을 다른 프로세스(편집기 · 다른 Pitwall)가 쓰는 경합: 읽은 내용으로 새 내용을 만든 뒤 쓰기 직전에 다시 읽어
//!   달라졌으면 새로 읽은 것으로 다시 만든다([`ATTEMPTS`] 번까지). 다시 읽은 뒤 rename 까지의 아주 짧은 틈은 막지 못한다 — 그
//!   사이에 쓴 쪽의 변경은 이 쓰기에 덮인다. 파일 잠금은 두지 않는다(편집기 · dotfiles 도구가 잠금을 보지 않는다).

use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

use toml_edit::{DocumentMut, Item, Table};

use super::migrate::{Schemas, schema_of, write_atomic};

/// 전역 설정 파일 이름(`docs/design/03-architecture.md` 「설정 파일」 표).
pub const SETTINGS_FILE: &str = "settings.toml";

/// 설정 계층이 `settings.toml` 을 부르는 종류 이름(`Schemas::builtin`).
const SETTINGS_KIND: &str = "settings";

/// 경합으로 다시 만드는 횟수.
pub const ATTEMPTS: usize = 3;

#[derive(Debug)]
pub enum WriteError {
    Io {
        path: PathBuf,
        error: io::Error,
    },
    /// 지금 파일이 TOML 이 아니거나, 고칠 표가 표가 아니거나, 앱보다 새 `schema` 다.
    Invalid {
        path: PathBuf,
        message: String,
    },
    /// 몇 번 다시 읽어도 쓰는 사이에 파일이 바뀌었다.
    Busy {
        path: PathBuf,
    },
}

impl std::fmt::Display for WriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WriteError::Io { path, error } => write!(f, "{}: {error}", path.display()),
            WriteError::Invalid { path, message } => write!(f, "{}: {message}", path.display()),
            WriteError::Busy { path } => write!(
                f,
                "{}: the file kept changing while it was being written; try again",
                path.display()
            ),
        }
    }
}

impl std::error::Error for WriteError {}

/// `config_dir/settings.toml` 의 `key`(`표.키` — 점 하나)를 `value` 로 둔다. `None` 이면 그 키를 지운다(기본값으로 돌린다).
/// 바꿨으면 참, 이미 그 값(지우기면 이미 없음)이면 거짓.
pub fn write_setting(
    config_dir: &Path,
    key: &str,
    value: Option<toml_edit::Value>,
) -> Result<bool, WriteError> {
    write_setting_with(config_dir, key, value, &mut |_| {})
}

/// [`write_setting`] 본체. `before_recheck` 는 새 내용을 만든 뒤 · 다시 읽기 직전에 불린다 — 시험이 그 틈에 다른 쓰기를
/// 끼워 경합을 만든다.
pub fn write_setting_with(
    config_dir: &Path,
    key: &str,
    value: Option<toml_edit::Value>,
    before_recheck: &mut dyn FnMut(&Path),
) -> Result<bool, WriteError> {
    let path = config_dir.join(SETTINGS_FILE);
    let io_err = |error| WriteError::Io {
        path: path.clone(),
        error,
    };
    for _ in 0..ATTEMPTS {
        let current = read(&path).map_err(io_err)?;
        if current.is_none() && value.is_none() {
            return Ok(false);
        }
        let next = with_key(current.as_deref(), key, value.as_ref()).map_err(|message| {
            WriteError::Invalid {
                path: path.clone(),
                message,
            }
        })?;
        if current.as_deref() == Some(next.as_str()) {
            return Ok(false);
        }
        // 쓰기 직전에 다시 본다. 그 사이 바뀌었으면 새 내용으로 다시 만든다.
        before_recheck(&path);
        if read(&path).map_err(io_err)? != current {
            continue;
        }
        match current {
            Some(_) => write_atomic(&path, &next).map_err(io_err)?,
            None => match create(&path, &next) {
                Ok(()) => {}
                // 다른 쪽이 먼저 만들었다 — 그 파일을 읽어 다시 고친다.
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(io_err(e)),
            },
        }
        return Ok(true);
    }
    Err(WriteError::Busy { path })
}

/// 파일 내용. 없으면 `None`. 대상이 없는 심링크는 없는 파일이 아니라 오류다(만들면 링크를 덮는다).
fn read(path: &Path) -> io::Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            if fs::symlink_metadata(path).is_ok() {
                Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    "symlink target does not exist",
                ))
            } else {
                Ok(None)
            }
        }
        Err(e) => Err(e),
    }
}

fn create(path: &Path, contents: &str) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    file.write_all(contents.as_bytes())?;
    file.sync_all()
}

/// `current`(없으면 새 파일)의 `key` 를 `value` 로 바꾸거나(`None` 이면 지운) 내용.
fn with_key(
    current: Option<&str>,
    key: &str,
    value: Option<&toml_edit::Value>,
) -> Result<String, String> {
    let mut doc: DocumentMut = match current {
        Some(text) => text
            .parse()
            .map_err(|e: toml_edit::TomlError| format!("not valid TOML, not changing it: {e}"))?,
        None => "schema = 1\n".parse().expect("constant TOML"),
    };
    let supported = Schemas::builtin()
        .get(SETTINGS_KIND)
        .map_or(0, |m| m.current());
    match schema_of(&doc) {
        Ok(version) if version <= supported => {}
        Ok(version) => {
            return Err(format!(
                "`schema = {version}` is newer than this app reads (up to {supported}), not changing it"
            ));
        }
        Err(e) => return Err(format!("{e}, not changing it")),
    }
    let Some((table, name)) = key.split_once('.') else {
        return Err(format!("`{key}` is not a `table.key` setting"));
    };
    let Some(value) = value else {
        match doc.get_mut(table) {
            None => {}
            Some(item) => {
                let Some(t) = item.as_table_like_mut() else {
                    return Err(format!("`{table}` is not a table, not changing it"));
                };
                t.remove(name);
            }
        }
        return Ok(doc.to_string());
    };
    let item = doc
        .entry(table)
        .or_insert_with(|| Item::Table(Table::new()));
    let Some(t) = item.as_table_like_mut() else {
        return Err(format!("`{table}` is not a table, not changing it"));
    };
    let mut next = value.clone();
    if let Some(old) = t.get(name).and_then(Item::as_value) {
        *next.decor_mut() = old.decor().clone();
    }
    t.insert(name, Item::Value(next));
    Ok(doc.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn int(n: i64) -> Option<toml_edit::Value> {
        Some(toml_edit::Value::from(n))
    }

    #[test]
    fn writes_a_key_into_its_table_and_keeps_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(SETTINGS_FILE);
        fs::write(
            &file,
            "schema = 1\n[ui]\nlanguage = \"ko\"\n\n[db]\ngrid_max_rows = 10 # 내 값\n",
        )
        .unwrap();
        assert!(write_setting(dir.path(), "db.grid_max_rows", int(500)).unwrap());
        assert_eq!(
            fs::read_to_string(&file).unwrap(),
            "schema = 1\n[ui]\nlanguage = \"ko\"\n\n[db]\ngrid_max_rows = 500 # 내 값\n"
        );
        assert!(!write_setting(dir.path(), "db.grid_max_rows", int(500)).unwrap());
    }

    #[test]
    fn removes_a_key_and_does_not_create_a_file_to_remove_from() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(SETTINGS_FILE);
        assert!(!write_setting(dir.path(), "db.grid_max_rows", None).unwrap());
        assert!(!file.exists());
        fs::write(
            &file,
            "schema = 1\n[db]\ngrid_max_rows = 10\nconnect_timeout = 5\n",
        )
        .unwrap();
        assert!(write_setting(dir.path(), "db.grid_max_rows", None).unwrap());
        assert_eq!(
            fs::read_to_string(&file).unwrap(),
            "schema = 1\n[db]\nconnect_timeout = 5\n"
        );
        assert!(!write_setting(dir.path(), "db.grid_max_rows", None).unwrap());
    }

    #[test]
    fn refuses_a_key_without_a_table_and_a_table_that_is_not_one() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(SETTINGS_FILE);
        fs::write(&file, "schema = 1\ndb = 3\n").unwrap();
        for (key, value) in [
            ("grid_max_rows", int(1)),
            ("db.grid_max_rows", int(1)),
            ("db.grid_max_rows", None),
        ] {
            let e = write_setting(dir.path(), key, value).unwrap_err();
            assert!(matches!(e, WriteError::Invalid { .. }), "{e}");
        }
        assert_eq!(fs::read_to_string(&file).unwrap(), "schema = 1\ndb = 3\n");
    }
}
