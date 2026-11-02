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
//!   달라졌으면 새로 읽은 것으로 다시 만든다([`ATTEMPTS`] 번까지). 파일 잠금은 두지 않는다(편집기 · dotfiles 도구가 잠금을 보지
//!   않는다).
//! - 같은 프로세스 안의 쓰기(설정 창의 DB 페이지 · 다른 창의 언어 쓰기)는 프로세스 전역 잠금([`IN_PROCESS`])으로 한 줄로 세운다 —
//!   다시 읽은 뒤 rename 까지는 임시 파일 쓰기 · fsync 가 들어 짧지 않아, 잠금 없이 두 키를 동시에 쓰면 거의 늘 한쪽이 사라진다
//!   (M3-20 리뷰 1번 — 300/300). 잠금이 막지 못하는 것은 다른 프로세스(편집기 · 다른 Pitwall)와의 그 틈뿐이고, 그 사이에 쓴 쪽의
//!   변경은 이 쓰기에 덮인다.

use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use toml_edit::{DocumentMut, Item, Table};

use super::migrate::{Schemas, schema_of, write_atomic};

/// 전역 설정 파일 이름(`docs/design/03-architecture.md` 「설정 파일」 표).
pub const SETTINGS_FILE: &str = "settings.toml";

/// 설정 계층이 `settings.toml` 을 부르는 종류 이름(`Schemas::builtin`).
const SETTINGS_KIND: &str = "settings";

/// 경합으로 다시 만드는 횟수.
pub const ATTEMPTS: usize = 3;

/// 이 프로세스의 쓰기를 한 줄로 세우는 잠금(모듈 문서). 읽기 → 고치기 → 다시 읽기 → rename 전체를 쥔다.
static IN_PROCESS: Mutex<()> = Mutex::new(());

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
    let Some((table, name)) = key.split_once('.') else {
        return Err(WriteError::Invalid {
            path,
            message: format!("`{key}` is not a `table.key` setting"),
        });
    };
    write_in(&path, SETTINGS_KIND, &[table, name], value, before_recheck)
}

/// 설정 파일 `file`(종류 `kind` — `project` · `settings` 등, `schema` 판정에 쓴다)의 키 `path`(표 이름들 + 키, 조각마다 그대로 —
/// 점 · 따옴표가 든 조각도 한 조각이다. TOML 로 쓸 때 따옴표 규칙대로 감싼다)를 `value` 로 둔다. `None` 이면 그 키를 지운다.
/// [`write_setting`] 과 같은 규칙(주석 · 다른 키 보존, 없는 파일은 `schema = 1` 로 만듦, 경합 · 심링크 · 새 `schema` 거절, 같은
/// 프로세스 잠금)이다. 처음 만드는 중간 표는 머리글을 따로 두지 않는다(`[lsp.ruff]` 한 줄). 프로젝트의 개인 파일
/// (`ConfigSource::local_file`)을 고칠 때 쓴다(15-m4-plan M4-17).
pub fn write_key(
    file: &Path,
    kind: &str,
    path: &[&str],
    value: Option<toml_edit::Value>,
) -> Result<bool, WriteError> {
    write_in(file, kind, path, value, &mut |_| {})
}

fn write_in(
    path: &Path,
    kind: &str,
    keys: &[&str],
    value: Option<toml_edit::Value>,
    before_recheck: &mut dyn FnMut(&Path),
) -> Result<bool, WriteError> {
    // 쓰다 패닉한 쪽이 있어도 파일은 rename 으로만 바뀌므로 반쯤 쓴 내용은 없다 — 독은 무시한다.
    let _one_at_a_time = IN_PROCESS.lock().unwrap_or_else(PoisonError::into_inner);
    let path = path.to_path_buf();
    let io_err = |error| WriteError::Io {
        path: path.clone(),
        error,
    };
    for _ in 0..ATTEMPTS {
        let current = read(&path).map_err(io_err)?;
        if current.is_none() && value.is_none() {
            return Ok(false);
        }
        let next = with_key(current.as_deref(), kind, keys, value.as_ref()).map_err(|message| {
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

/// `current`(없으면 새 파일)의 키 `keys` 를 `value` 로 바꾸거나(`None` 이면 지운) 내용.
fn with_key(
    current: Option<&str>,
    kind: &str,
    keys: &[&str],
    value: Option<&toml_edit::Value>,
) -> Result<String, String> {
    let mut doc: DocumentMut = match current {
        Some(text) => text
            .parse()
            .map_err(|e: toml_edit::TomlError| format!("not valid TOML, not changing it: {e}"))?,
        None => "schema = 1\n".parse().expect("constant TOML"),
    };
    let supported = Schemas::builtin().get(kind).map_or(0, |m| m.current());
    match schema_of(&doc) {
        Ok(version) if version <= supported => {}
        Ok(version) => {
            return Err(format!(
                "`schema = {version}` is newer than this app reads (up to {supported}), not changing it"
            ));
        }
        Err(e) => return Err(format!("{e}, not changing it")),
    }
    let Some((name, tables)) = keys.split_last().filter(|(_, t)| !t.is_empty()) else {
        return Err(format!("`{}` is not a `table.key` setting", keys.join(".")));
    };
    let shown = |upto: usize| tables[..=upto].join(".");
    let Some(value) = value else {
        let mut table: &mut dyn toml_edit::TableLike = doc.as_table_mut();
        for (i, t) in tables.iter().enumerate() {
            match table.get_mut(t) {
                None => return Ok(doc.to_string()),
                Some(item) => {
                    let Some(next) = item.as_table_like_mut() else {
                        return Err(format!("`{}` is not a table, not changing it", shown(i)));
                    };
                    table = next;
                }
            }
        }
        table.remove(name);
        return Ok(doc.to_string());
    };
    let mut table: &mut dyn toml_edit::TableLike = doc.as_table_mut();
    for (i, t) in tables.iter().enumerate() {
        // 맨 바깥 표는 머리글을 둔다(`[db]`). 안쪽에 새로 만드는 표의 바깥 것은 머리글 없이 둔다(`[lsp.ruff]` 만 — 빈 `[lsp]` 없이).
        let last = i + 1 == tables.len();
        let item = table.entry(t).or_insert_with(|| {
            let mut new = Table::new();
            new.set_implicit(!last);
            Item::Table(new)
        });
        let Some(next) = item.as_table_like_mut() else {
            return Err(format!("`{}` is not a table, not changing it", shown(i)));
        };
        table = next;
    }
    let mut next = value.clone();
    if let Some(old) = table.get(name).and_then(Item::as_value) {
        *next.decor_mut() = old.decor().clone();
    }
    table.insert(name, Item::Value(next));
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

    /// 같은 프로세스에서 두 키를 동시에 써도 둘 다 남는다(M3-20 리뷰 1번 — 잠금이 없으면 거의 매번 한쪽이 사라졌다, 300/300).
    #[test]
    fn concurrent_writes_in_one_process_keep_both_keys() {
        use std::sync::{Arc, Barrier};
        const ROUNDS: usize = 100;
        let mut lost = 0;
        for _ in 0..ROUNDS {
            let dir = tempfile::tempdir().unwrap();
            fs::write(dir.path().join(SETTINGS_FILE), "schema = 1\n").unwrap();
            let start = Arc::new(Barrier::new(2));
            let writers: Vec<_> = [
                ("db.grid_max_rows", toml_edit::Value::from(5)),
                ("ui.language", toml_edit::Value::from("ko")),
            ]
            .into_iter()
            .map(|(key, value)| {
                let (dir, start) = (dir.path().to_owned(), Arc::clone(&start));
                std::thread::spawn(move || {
                    start.wait();
                    write_setting(&dir, key, Some(value)).unwrap()
                })
            })
            .collect();
            for w in writers {
                assert!(
                    w.join().unwrap(),
                    "each write reports that it changed the file"
                );
            }
            let text = fs::read_to_string(dir.path().join(SETTINGS_FILE)).unwrap();
            if !(text.contains("grid_max_rows = 5") && text.contains("language = \"ko\"")) {
                lost += 1;
            }
        }
        assert_eq!(lost, 0, "{lost} of {ROUNDS} rounds lost a key");
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

    /// 키 조각은 그대로 한 조각이다 — 점 · 따옴표 · 제어 문자가 든 서버 id 도 `[lsp."<id>"]` 하나로 쓰고 다시 읽으면 같은 이름이다.
    /// 새 파일은 `schema = 1` 과 그 표 하나(빈 `[lsp]` 머리글 없이), 있던 키 · 주석은 그대로(15-m4-plan M4-17).
    #[test]
    fn write_key_quotes_each_piece_and_nests_tables() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(".pitwall/project.local.toml");
        let f = Some(toml_edit::Value::from(false));
        assert!(write_key(&file, "project", &["lsp", "ruff", "enabled"], f.clone()).unwrap());
        assert_eq!(
            fs::read_to_string(&file).unwrap(),
            "schema = 1\n\n[lsp.ruff]\nenabled = false\n"
        );
        assert!(!write_key(&file, "project", &["lsp", "ruff", "enabled"], f.clone()).unwrap());
        for id in ["a.b", "q\"uote", "tab\there", "새.서버"] {
            assert!(
                write_key(&file, "project", &["lsp", id, "install_prompt"], f.clone()).unwrap()
            );
            let doc: toml_edit::DocumentMut = fs::read_to_string(&file).unwrap().parse().unwrap();
            assert_eq!(
                doc["lsp"][id]["install_prompt"].as_bool(),
                Some(false),
                "{id}: {doc}"
            );
            // 조각이 점에서 갈라지지 않았다.
            assert!(doc["lsp"].get("a").is_none(), "{doc}");
        }
        assert!(write_key(&file, "project", &["lsp", "a.b", "install_prompt"], None).unwrap());
        let doc: toml_edit::DocumentMut = fs::read_to_string(&file).unwrap().parse().unwrap();
        assert!(doc["lsp"]["a.b"].get("install_prompt").is_none());
        assert_eq!(doc["lsp"]["ruff"]["enabled"].as_bool(), Some(false));
        // 지울 것이 없으면 바꾸지 않는다.
        assert!(!write_key(&file, "project", &["lsp", "none", "enabled"], None).unwrap());
    }

    #[test]
    fn write_key_keeps_comments_and_refuses_non_tables_and_newer_schemas() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("project.local.toml");
        fs::write(
            &file,
            "schema = 1\n# 내 메모\n[lsp.ruff]\nenabled = true # 켬\n",
        )
        .unwrap();
        assert!(
            write_key(
                &file,
                "project",
                &["lsp", "ruff", "enabled"],
                Some(false.into())
            )
            .unwrap()
        );
        assert_eq!(
            fs::read_to_string(&file).unwrap(),
            "schema = 1\n# 내 메모\n[lsp.ruff]\nenabled = false # 켬\n"
        );
        fs::write(&file, "schema = 1\nlsp = 3\n").unwrap();
        let e = write_key(
            &file,
            "project",
            &["lsp", "ruff", "enabled"],
            Some(false.into()),
        )
        .unwrap_err();
        assert!(matches!(e, WriteError::Invalid { .. }), "{e}");
        fs::write(&file, "schema = 99\n").unwrap();
        let e = write_key(
            &file,
            "project",
            &["lsp", "ruff", "enabled"],
            Some(false.into()),
        )
        .unwrap_err();
        assert!(matches!(e, WriteError::Invalid { .. }), "{e}");
        assert_eq!(fs::read_to_string(&file).unwrap(), "schema = 99\n");
    }
}
