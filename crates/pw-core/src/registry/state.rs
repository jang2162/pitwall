//! 프로젝트 상태(결정 M1-D19, M1-25): `$DATA/projects/<id>/state.json` — 프로젝트 창의 레이아웃과 워크트리별 탭 묶음
//! (열린 탭 · 활성 탭 · 커서 · 스크롤 · 트리 펼침). 내용의 형식은 UI 가 정한다(apps/desktop/src/layout/project-state.ts).
//! Host 는 JSON 객체 하나를 그대로 두고 꺼낼 뿐이다 — 원격 프로젝트에서도 같은 길이다(03-architecture.md 「데이터 저장」 표).
//!
//! - 경로: 프로젝트 id 를 폴더 이름으로 쓰므로 id 는 [`valid_id`] 의 글자만 받는다(`..` · 구분자 · 빈 값이 경로를 벗어나지 못하게).
//!   레지스트리에 있는 프로젝트일 때만 읽고 쓴다.
//! - 쓰기: 같은 폴더의 임시 파일에 쓰고 동기화한 뒤 rename 한다. 쓰는 도중에 앱이 죽어도 옛 파일이 남는다. 같은 파일을 두
//!   프로세스가 쓰면 나중 것이 이긴다(창 하나의 상태 전체를 한 번에 쓴다). 죽은 프로세스가 남긴 임시 파일(이름의 pid 가 이
//!   프로세스가 아닌 것)은 다음 쓰기가 지운다. 폴더는 fsync 하지 않는다 — 전원이 나가면 rename 이 빠질 수 있지만 옛 파일이 남는다.
//! - 크기: [`STATE_LIMIT`] 바이트까지. 넘는 쓰기는 거절하고, 넘는 파일은 상한까지만 읽고 버린다.
//! - 깨진 파일(읽기 실패 · JSON 이 아님 · 객체가 아님 · 너무 큼)은 없는 것처럼 `state: null` 과 이유(`problem`)를 돌려준다 — 창은
//!   기본 레이아웃으로 열고, 다음 쓰기가 덮는다.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use pw_rpc::{ConnectionScope, JsonValue};
use serde::{Deserialize, Serialize};

use super::{Registry, RegistryError};
use crate::config::robust;

/// `state.json` 의 크기 상한(바이트). 레이아웃과 탭 수십 개의 묶음 여럿은 수십 KB 다.
pub const STATE_LIMIT: usize = 1024 * 1024;

/// `$DATA` 아래의 프로젝트별 폴더(03-architecture.md 「데이터 저장」 표의 `projects/<id>/`).
pub const PROJECTS_DIR: &str = "projects";
pub const STATE_FILE: &str = "state.json";
/// 쓰는 중인 임시 파일: `.state.json.<pid>.<카운터>.<나노초>.pitwall-tmp`.
const TMP_SUFFIX: &str = ".pitwall-tmp";

#[cfg(test)]
type BeforeRename = Box<dyn Fn() -> std::io::Result<()>>;

#[cfg(test)]
thread_local! {
    /// 시험: 임시 파일을 다 쓴 뒤 · rename 직전에 부른다. 오류를 돌려주면 그 쓰기는 거기서 실패한다(쓰다 멈춘 것을 재현한다).
    static BEFORE_RENAME: std::cell::RefCell<Option<BeforeRename>> =
        const { std::cell::RefCell::new(None) };
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProjectState {
    /// 저장된 상태. 없거나 읽지 못했으면 `null`.
    #[specta(type = Option<JsonValue>)]
    pub state: Option<serde_json::Value>,
    /// 파일이 있는데 읽지 못한 이유. 창은 로그에 남기고 기본 레이아웃으로 연다.
    pub problem: Option<String>,
}

/// 경로 한 칸으로 쓸 수 있는 프로젝트 id: 영문자 · 숫자 · `-` · `_`, 1~64 글자. 레지스트리가 만드는 id(16자리 hex)는 늘 맞는다.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn checked_id(id: &str) -> Result<&str, RegistryError> {
    if valid_id(id) {
        Ok(id)
    } else {
        Err(RegistryError::NotFound(id.to_owned()))
    }
}

/// `$DATA/projects/<id>/state.json`. id 는 [`valid_id`] 를 지난 것이어야 한다.
pub fn state_file(data: &Path, id: &str) -> PathBuf {
    data.join(PROJECTS_DIR).join(id).join(STATE_FILE)
}

/// 파일을 읽는다. 없으면 `Ok(None)`, 읽을 수 없거나 형식이 틀리면 `Err(이유)`.
pub fn read_state(file: &Path) -> Result<Option<serde_json::Value>, String> {
    let cannot = |e: std::io::Error| format!("cannot read {}: {e}", file.display());
    let input = match fs::File::open(file) {
        Ok(input) => input,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(cannot(e)),
    };
    // 상한 너머는 읽지 않는다 — 손으로 넣은 거대한 파일을 메모리에 다 올리지 않게.
    let mut bytes = Vec::new();
    input
        .take(STATE_LIMIT as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(cannot)?;
    if bytes.len() > STATE_LIMIT {
        return Err(format!(
            "{} is over the {STATE_LIMIT} byte limit",
            file.display()
        ));
    }
    match serde_json::from_slice::<serde_json::Value>(&bytes) {
        Ok(value @ serde_json::Value::Object(_)) => Ok(Some(value)),
        Ok(_) => Err(format!("{} is not a JSON object", file.display())),
        Err(e) => Err(format!("{} is not JSON: {e}", file.display())),
    }
}

/// 상태를 원자적으로 쓴다(머리 주석). 폴더가 없으면 만든다. 객체가 아니거나 [`STATE_LIMIT`] 를 넘으면 쓰지 않는다.
pub fn write_state(file: &Path, state: &serde_json::Value) -> Result<(), RegistryError> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let invalid = |reason: String| RegistryError::InvalidPath {
        path: file.to_owned(),
        reason,
    };
    if !state.is_object() {
        return Err(invalid("the project state must be a JSON object".into()));
    }
    let text = serde_json::to_vec(state).map_err(|e| invalid(e.to_string()))?;
    if text.len() > STATE_LIMIT {
        return Err(invalid(format!(
            "the project state is {} bytes, over the {STATE_LIMIT} byte limit",
            text.len()
        )));
    }
    let io = |op: &str, path: &Path, e: std::io::Error| {
        RegistryError::Db(format!("cannot {op} {}: {e}", path.display()))
    };
    let dir = file.parent().expect("state file has a parent");
    fs::create_dir_all(dir).map_err(|e| io("create", dir, e))?;
    remove_stale_temps(dir);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    let (tmp, mut out) = loop {
        let tmp = dir.join(format!(
            ".{STATE_FILE}.{}.{}.{nanos}{TMP_SUFFIX}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed),
        ));
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
        {
            Ok(out) => break (tmp, out),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(io("create", &tmp, e)),
        }
    };
    let written = out
        .write_all(&text)
        .and_then(|()| out.sync_all())
        .map_err(|e| io("write", &tmp, e));
    drop(out);
    #[cfg(test)]
    let written = written.and_then(|()| {
        BEFORE_RENAME
            .with_borrow(|hook| hook.as_ref().map_or(Ok(()), |hook| hook()))
            .map_err(|e| io("write", &tmp, e))
    });
    let renamed = written.and_then(|()| {
        robust::retrying(|| fs::rename(&tmp, file)).map_err(|e| io("replace", file, e))
    });
    if renamed.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    renamed
}

/// 죽은 프로세스가 쓰다 남긴 임시 파일을 지운다(머리 주석). 이 프로세스의 것은 지금 쓰는 중일 수 있어 두고, 지우지 못해도 쓰기를
/// 막지 않는다.
fn remove_stale_temps(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let ours = format!(".{STATE_FILE}.{}.", std::process::id());
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if name.starts_with(&format!(".{STATE_FILE}."))
            && name.ends_with(TMP_SUFFIX)
            && !name.starts_with(&ours)
        {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// 이 연결이 그 프로젝트의 상태를 써도 되는가: 그 프로젝트 창의 연결만(M1-25 브리프) — 연결 범위가 그 프로젝트일 때다
/// ([`ConnectionScope::Project`]). 프로젝트 창은 자기 프로젝트로 연결하고(데스크톱 ipc.rs `caller_for`), Host 로 고른 연결(웰컴 ·
/// 설정 창, 창의 UI 로그) · 파일 범위 연결 · 다른 프로젝트로 연 연결은 쓰지 못한다.
pub fn connection_may_write(ctx: &pw_rpc::Ctx, project_id: &str) -> bool {
    matches!(ctx.scope(), ConnectionScope::Project(id) if id == project_id)
}

impl Registry {
    /// 프로젝트의 상태(머리 주석). 레지스트리에 없는 프로젝트면 [`RegistryError::NotFound`].
    pub fn project_state(&self, id: &str) -> Result<ProjectState, RegistryError> {
        let id = checked_id(id)?;
        self.get(id)?;
        Ok(match read_state(&state_file(&self.inner.paths.data, id)) {
            Ok(state) => ProjectState {
                state,
                problem: None,
            },
            Err(problem) => ProjectState {
                state: None,
                problem: Some(problem),
            },
        })
    }

    /// 프로젝트의 상태를 통째로 바꾼다.
    pub fn set_project_state(
        &self,
        id: &str,
        state: &serde_json::Value,
    ) -> Result<(), RegistryError> {
        let id = checked_id(id)?;
        self.get(id)?;
        write_state(&state_file(&self.inner.paths.data, id), state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 경로 조각이 될 수 없는 id 는 레지스트리에 묻기 전에 거른다 — 레지스트리 확인이 막아 주더라도 경로를 만들지 않는다.
    #[test]
    fn ids_that_could_leave_the_projects_folder_are_invalid() {
        for id in ["", ".", "..", "../x", "a/b", "a\\b", "a:b", "a b", "é"] {
            assert!(!valid_id(id), "{id:?}");
        }
        assert!(!valid_id(&"a".repeat(65)));
        for id in ["0123456789abcdef", "my-project_2", &"a".repeat(64)] {
            assert!(valid_id(id), "{id:?}");
        }
        assert_eq!(
            state_file(Path::new("/data"), "abc"),
            Path::new("/data")
                .join("projects")
                .join("abc")
                .join("state.json")
        );
    }

    fn temps(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .filter(|n| n.ends_with(TMP_SUFFIX))
            .collect();
        names.sort();
        names
    }

    /// 임시 파일을 다 쓴 뒤 rename 전에 멈추면 `state.json` 은 옛 내용 그대로이고 임시 파일도 남지 않는다.
    #[test]
    fn a_write_stopped_before_the_rename_keeps_the_old_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = state_file(dir.path(), "p1");
        write_state(&file, &serde_json::json!({ "n": 1 })).unwrap();
        BEFORE_RENAME.set(Some(Box::new(|| Err(std::io::Error::other("stopped")))));
        let stopped = write_state(&file, &serde_json::json!({ "n": 2 }));
        BEFORE_RENAME.set(None);
        assert!(stopped.is_err());
        assert_eq!(read_state(&file), Ok(Some(serde_json::json!({ "n": 1 }))));
        assert_eq!(temps(file.parent().unwrap()), Vec::<String>::new());
    }

    /// rename 이 실패해도(대상 자리에 비어 있지 않은 폴더) 임시 파일을 남기지 않는다.
    #[test]
    fn a_failed_rename_leaves_no_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = state_file(dir.path(), "p1");
        fs::create_dir_all(file.join("inside")).unwrap();
        assert!(write_state(&file, &serde_json::json!({ "n": 1 })).is_err());
        assert_eq!(temps(file.parent().unwrap()), Vec::<String>::new());
    }

    /// 죽은 프로세스(다른 pid)가 남긴 임시 파일은 다음 쓰기가 지운다. 이 프로세스의 것 · 다른 파일은 둔다.
    #[test]
    fn the_next_write_removes_temp_files_left_by_a_dead_process() {
        let dir = tempfile::tempdir().unwrap();
        let file = state_file(dir.path(), "p1");
        let folder = file.parent().unwrap();
        fs::create_dir_all(folder).unwrap();
        let other = std::process::id().wrapping_add(1);
        let stale = format!(".{STATE_FILE}.{other}.0.0{TMP_SUFFIX}");
        let ours = format!(".{STATE_FILE}.{}.999.0{TMP_SUFFIX}", std::process::id());
        for name in [&stale, &ours, &"notes.txt".to_owned()] {
            fs::write(folder.join(name), "x").unwrap();
        }
        write_state(&file, &serde_json::json!({ "n": 1 })).unwrap();
        assert_eq!(temps(folder), vec![ours]);
        assert!(folder.join("notes.txt").exists());
        assert_eq!(read_state(&file), Ok(Some(serde_json::json!({ "n": 1 }))));
    }

    /// 상한을 넘는 파일은 상한까지만 읽고 거절한다. 꼭 상한인 파일은 읽는다.
    #[test]
    fn a_file_over_the_limit_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(STATE_FILE);
        let mut text = vec![b' '; STATE_LIMIT];
        text.extend_from_slice(b"{}");
        fs::write(&file, &text).unwrap();
        assert!(read_state(&file).unwrap_err().contains("limit"));
        fs::write(&file, &text[2..]).unwrap();
        assert_eq!(read_state(&file), Ok(Some(serde_json::json!({}))));
    }
}
