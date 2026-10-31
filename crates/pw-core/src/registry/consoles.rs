//! SQL 콘솔 파일(14-m3-plan.md M3-15, 결정 D36): `$DATA/projects/<id>/consoles/<콘솔 id>.sql`. 콘솔 탭의 글을 미저장 개념 없이
//! 바로 저장하는 자리다 — 창이 편집을 모아 쓰고(`project.console.write`), 탭을 복원하면 읽는다(`project.console.read`). Host 는
//! 글을 해석하지 않는다.
//!
//! - 이름: 콘솔 id 는 [`valid_console_id`] 의 글자만(영소문자 · 숫자 · `-`, 1~64자) — 경로를 벗어나지 못하고 대소문자를 가리지
//!   않는 파일 시스템에서도 둘이 한 파일이 되지 않는다. 파일은 `<id>.sql` 이다. 목록은 이 꼴의 파일만 센다.
//! - 쓰기는 상태 파일과 같은 원자적 쓰기(`super::state::write_atomically`)다. 크기 상한은 [`CONSOLE_LIMIT`](`db.sql.split` 이
//!   받는 SQL 상한과 같다) — 넘는 쓰기는 거절하고, 넘는 파일 · UTF-8 이 아닌 파일은 읽지 않고 invalid params 로 이유를 준다.
//! - 쓰기 · 지우기는 그 프로젝트 창의 연결만(`project.state.set` 과 같은 판정 — [`super::state::connection_may_write`]).

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use pw_rpc::JsNumber;
use serde::{Deserialize, Serialize};

use super::state::{PROJECTS_DIR, checked_id, write_atomically};
use super::{Registry, RegistryError};

/// `$DATA/projects/<id>/` 아래 콘솔 파일의 폴더.
pub const CONSOLES_DIR: &str = "consoles";
/// 콘솔 파일의 확장자.
pub const CONSOLE_EXTENSION: &str = "sql";
/// 콘솔 글의 상한(UTF-8 바이트) — `pw_db_sql::split::MAX_SQL_BYTES`(16 MiB)와 같다. 이보다 큰 글은 콘솔이 나누지도 못한다.
pub const CONSOLE_LIMIT: usize = 16 * 1024 * 1024;

/// 콘솔 id 로 쓸 수 있는 이름: 영소문자 · 숫자 · `-`, 1~64자.
pub fn valid_console_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// `$DATA/projects/<프로젝트 id>/consoles`.
pub fn consoles_dir(data: &Path, project_id: &str) -> PathBuf {
    data.join(PROJECTS_DIR).join(project_id).join(CONSOLES_DIR)
}

/// 콘솔 파일 하나.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct ConsoleFile {
    pub console_id: String,
    /// 바이트.
    #[specta(type = JsNumber)]
    pub size: u64,
    /// 마지막으로 쓴 시각(유닉스 밀리초). 파일 시스템이 주지 않으면 `null`.
    #[specta(type = Option<JsNumber>)]
    pub modified_ms: Option<u64>,
}

/// 읽은 콘솔. 파일이 없으면 `text` 가 `null` 이다(새 콘솔).
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct ConsoleText {
    pub text: Option<String>,
}

fn checked_console(id: &str) -> Result<&str, RegistryError> {
    if valid_console_id(id) {
        Ok(id)
    } else {
        Err(RegistryError::InvalidPath {
            path: PathBuf::from(id),
            reason: "a console id is 1-64 lowercase letters, digits or `-`".into(),
        })
    }
}

fn console_file(dir: &Path, console_id: &str) -> PathBuf {
    dir.join(format!("{console_id}.{CONSOLE_EXTENSION}"))
}

/// 파일 하나를 읽는다. 없으면 `Ok(None)`.
fn read_console_file(file: &Path) -> Result<Option<String>, RegistryError> {
    let invalid = |reason: String| RegistryError::InvalidPath {
        path: file.to_owned(),
        reason,
    };
    let input = match fs::File::open(file) {
        Ok(input) => input,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(RegistryError::Db(format!(
                "cannot read {}: {e}",
                file.display()
            )));
        }
    };
    let mut bytes = Vec::new();
    input
        .take(CONSOLE_LIMIT as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| RegistryError::Db(format!("cannot read {}: {e}", file.display())))?;
    if bytes.len() > CONSOLE_LIMIT {
        return Err(invalid(format!(
            "the console is over the {CONSOLE_LIMIT} byte limit"
        )));
    }
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|_| invalid("the console is not UTF-8 text".into()))
}

impl Registry {
    /// 프로젝트의 콘솔 파일들, 이름순. 폴더가 없으면 빈 목록.
    pub fn consoles(&self, project_id: &str) -> Result<Vec<ConsoleFile>, RegistryError> {
        let id = checked_id(project_id)?;
        self.get(id)?;
        let dir = consoles_dir(&self.inner.paths.data, id);
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => {
                return Err(RegistryError::Db(format!(
                    "cannot list {}: {e}",
                    dir.display()
                )));
            }
        };
        let mut files: Vec<ConsoleFile> = entries
            .flatten()
            .filter_map(|entry| {
                let name = entry.file_name().into_string().ok()?;
                let console_id = name.strip_suffix(&format!(".{CONSOLE_EXTENSION}"))?;
                let meta = entry.metadata().ok().filter(fs::Metadata::is_file)?;
                valid_console_id(console_id).then(|| ConsoleFile {
                    console_id: console_id.to_owned(),
                    size: meta.len(),
                    modified_ms: meta
                        .modified()
                        .ok()
                        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                        .map(|d| d.as_millis() as u64),
                })
            })
            .collect();
        files.sort_by(|a, b| a.console_id.cmp(&b.console_id));
        Ok(files)
    }

    /// 콘솔 하나의 글. 파일이 없으면 `text: null`.
    pub fn read_console(
        &self,
        project_id: &str,
        console_id: &str,
    ) -> Result<ConsoleText, RegistryError> {
        let id = checked_id(project_id)?;
        let console_id = checked_console(console_id)?;
        self.get(id)?;
        let file = console_file(&consoles_dir(&self.inner.paths.data, id), console_id);
        Ok(ConsoleText {
            text: read_console_file(&file)?,
        })
    }

    /// 콘솔 하나를 통째로 쓴다(원자적). [`CONSOLE_LIMIT`] 를 넘으면 쓰지 않는다.
    pub fn write_console(
        &self,
        project_id: &str,
        console_id: &str,
        text: &str,
    ) -> Result<(), RegistryError> {
        let id = checked_id(project_id)?;
        let console_id = checked_console(console_id)?;
        self.get(id)?;
        let file = console_file(&consoles_dir(&self.inner.paths.data, id), console_id);
        if text.len() > CONSOLE_LIMIT {
            return Err(RegistryError::InvalidPath {
                path: file,
                reason: format!(
                    "the console is {} bytes, over the {CONSOLE_LIMIT} byte limit",
                    text.len()
                ),
            });
        }
        write_atomically(&file, text.as_bytes())
    }

    /// 콘솔 하나를 지운다. 없으면 아무것도 하지 않는다.
    pub fn delete_console(&self, project_id: &str, console_id: &str) -> Result<(), RegistryError> {
        let id = checked_id(project_id)?;
        let console_id = checked_console(console_id)?;
        self.get(id)?;
        let file = console_file(&consoles_dir(&self.inner.paths.data, id), console_id);
        match fs::remove_file(&file) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(RegistryError::Db(format!(
                "cannot remove {}: {e}",
                file.display()
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 콘솔 id 는 경로 한 칸이고 대소문자가 갈리지 않는 꼴만 받는다.
    #[test]
    fn console_ids_cannot_leave_the_consoles_folder() {
        for id in [
            "",
            ".",
            "..",
            "../x",
            "a/b",
            "a\\b",
            "A",
            "a b",
            "a.sql",
            "é",
            &"a".repeat(65),
        ] {
            assert!(!valid_console_id(id), "{id:?}");
        }
        for id in ["c1", "console-2", &"a".repeat(64)] {
            assert!(valid_console_id(id), "{id:?}");
        }
    }

    /// 상한을 넘는 파일 · UTF-8 이 아닌 파일은 읽지 않고 이유를 준다. 없는 파일은 `None`.
    #[test]
    fn a_console_over_the_limit_or_not_utf8_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("c.sql");
        assert!(matches!(read_console_file(&file), Ok(None)));
        fs::write(&file, [0xff, 0xfe]).unwrap();
        assert!(
            read_console_file(&file)
                .unwrap_err()
                .to_string()
                .contains("UTF-8")
        );
        fs::write(&file, vec![b' '; CONSOLE_LIMIT + 1]).unwrap();
        assert!(
            read_console_file(&file)
                .unwrap_err()
                .to_string()
                .contains("limit")
        );
        fs::write(&file, "SELECT 1;\r\n").unwrap();
        assert_eq!(
            read_console_file(&file).unwrap().as_deref(),
            Some("SELECT 1;\r\n")
        );
    }
}
