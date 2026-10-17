//! 언어 설정 쓰기: 전역 `$CONFIG/settings.toml` 의 `ui.language` 한 키만 고친다. 설정 계층의 다른 쓰기는 아직 없다 — 일반
//! 쓰기는 설정 화면(10-ui §2.7)이 키 스키마와 함께 정한다.
//!
//! - 주석 · 서식 · 다른 키는 그대로 둔다(toml_edit). 값이 이미 같으면 쓰지 않는다.
//! - 파일이 없으면 `schema = 1` 과 그 키만으로 만든다. 파일을 읽거나 TOML 로 읽을 수 없으면 **쓰지 않고** 오류다 — 사람이
//!   고치다 만 파일을 덮지 않는다.
//! - 심링크는 링크가 아니라 대상 파일을 고친다(dotfiles 연결을 끊지 않는다). 대상이 없는 링크는 오류다.
//! - 같은 파일을 다른 프로세스(편집기 · 다른 Pitwall)가 쓰는 경합: 읽은 내용으로 새 내용을 만든 뒤 쓰기 직전에 다시 읽어
//!   달라졌으면 새로 읽은 것으로 다시 만든다(몇 번까지). 다시 읽은 뒤 rename 까지의 아주 짧은 틈은 막지 못한다 — 그 사이에
//!   쓴 쪽의 변경은 이 쓰기에 덮인다. 파일 잠금은 두지 않는다(편집기 · dotfiles 도구가 잠금을 보지 않는다).

use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

use pw_core::config::migrate::write_atomic;
use toml_edit::{DocumentMut, Item, Table};

use crate::language::LANGUAGE_KEY;

/// 전역 설정 파일 이름(`docs/design/03-architecture.md` 「설정 파일」 표).
pub const SETTINGS_FILE: &str = "settings.toml";

/// 경합으로 다시 만드는 횟수.
const ATTEMPTS: usize = 3;

#[derive(Debug)]
pub enum WriteError {
    Io {
        path: PathBuf,
        error: io::Error,
    },
    /// 지금 파일이 TOML 이 아니거나 `ui` 가 표가 아니다.
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

/// `config_dir/settings.toml` 의 `ui.language` 를 `value` 로 둔다. 바꿨으면 참, 이미 그 값이면 거짓.
pub fn write_language(config_dir: &Path, value: &str) -> Result<bool, WriteError> {
    let path = config_dir.join(SETTINGS_FILE);
    let io_err = |error| WriteError::Io {
        path: path.clone(),
        error,
    };
    for _ in 0..ATTEMPTS {
        let current = read(&path).map_err(io_err)?;
        let next =
            with_language(current.as_deref(), value).map_err(|message| WriteError::Invalid {
                path: path.clone(),
                message,
            })?;
        if current.as_deref() == Some(next.as_str()) {
            return Ok(false);
        }
        // 쓰기 직전에 다시 본다. 그 사이 바뀌었으면 새 내용으로 다시 만든다.
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

/// `current`(없으면 새 파일)의 `ui.language` 를 `value` 로 바꾼 내용.
fn with_language(current: Option<&str>, value: &str) -> Result<String, String> {
    let mut doc: DocumentMut = match current {
        Some(text) => text
            .parse()
            .map_err(|e: toml_edit::TomlError| format!("not valid TOML, not changing it: {e}"))?,
        None => "schema = 1\n".parse().expect("constant TOML"),
    };
    let (table, key) = LANGUAGE_KEY.split_once('.').expect("dotted key");
    let ui = doc
        .entry(table)
        .or_insert_with(|| Item::Table(Table::new()));
    let Some(ui) = ui.as_table_like_mut() else {
        return Err(format!("`{table}` is not a table, not changing it"));
    };
    // 있던 값의 앞뒤 공백 · 꼬리 주석(`language = "en" # 언어`)은 남긴다.
    let mut next = toml_edit::Value::from(value);
    if let Some(old) = ui.get(key).and_then(Item::as_value) {
        *next.decor_mut() = old.decor().clone();
    }
    ui.insert(key, Item::Value(next));
    Ok(doc.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_the_file_with_a_schema_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config");
        assert!(write_language(&config, "ko").unwrap());
        assert_eq!(
            fs::read_to_string(config.join(SETTINGS_FILE)).unwrap(),
            "schema = 1\n\n[ui]\nlanguage = \"ko\"\n"
        );
    }

    #[test]
    fn changes_only_the_language_key_and_keeps_comments() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(SETTINGS_FILE);
        fs::write(
            &file,
            "schema = 1\n# 내 설정\n[ui]\nlanguage = \"en\" # 언어\nfont = 13\n\n[diagnostics]\ncrash_reports = true\n",
        )
        .unwrap();
        assert!(write_language(dir.path(), "ko").unwrap());
        assert_eq!(
            fs::read_to_string(&file).unwrap(),
            "schema = 1\n# 내 설정\n[ui]\nlanguage = \"ko\" # 언어\nfont = 13\n\n[diagnostics]\ncrash_reports = true\n"
        );
        assert!(
            !write_language(dir.path(), "ko").unwrap(),
            "같은 값은 쓰지 않는다"
        );
    }

    #[test]
    fn refuses_a_file_it_cannot_read_as_toml() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(SETTINGS_FILE);
        for text in ["schema = = 1\n", "schema = 1\nui = 3\n"] {
            fs::write(&file, text).unwrap();
            let e = write_language(dir.path(), "ko").unwrap_err();
            assert!(matches!(e, WriteError::Invalid { .. }), "{e}");
            assert_eq!(fs::read_to_string(&file).unwrap(), text);
        }
    }

    #[cfg(unix)]
    #[test]
    fn writes_through_a_symlink_and_refuses_a_dangling_one() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("dotfiles/settings.toml");
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(&target, "schema = 1\n").unwrap();
        let config = dir.path().join("config");
        fs::create_dir_all(&config).unwrap();
        std::os::unix::fs::symlink(&target, config.join(SETTINGS_FILE)).unwrap();
        write_language(&config, "ko").unwrap();
        assert!(
            fs::symlink_metadata(config.join(SETTINGS_FILE))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert!(
            fs::read_to_string(&target)
                .unwrap()
                .contains("language = \"ko\"")
        );

        fs::remove_file(&target).unwrap();
        let e = write_language(&config, "en").unwrap_err();
        assert!(matches!(e, WriteError::Io { .. }), "{e}");
        assert!(!target.exists(), "끊긴 링크 자리에 새 파일을 만들지 않는다");
    }
}
