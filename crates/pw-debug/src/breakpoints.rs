//! 브레이크포인트 저장(17-m6-plan M6-17 ③ — 메인 판정 M6-P11): `$DATA/projects/<id>/breakpoints.json`.
//!
//! - 자리는 프로젝트의 앱 데이터 폴더다(03 「데이터 저장」 표의 `projects/<id>/`). 커밋되는 `.pitwall/` 에 두지 않는다 — 개인 상태다.
//! - 파일 열쇠는 **프로젝트 루트 기준 상대 경로**(`/` 로 나눔)다. 워크트리는 같은 목록을 쓴다 — 세션에 보낼 때 그 세션의 워크트리
//!   루트에 잇는다.
//! - 한 줄에 하나(줄이 같으면 뒤의 것이 이긴다), 줄 차례로 둔다. 조건 · 로그 메시지 · hit count 는 디버그 대상 안에서 평가되는
//!   글이다 — 사람이 화면에서 넣는 것은 이 RPC(write)로, 에이전트는 `exec` 툴(M6-20 `breakpoint_set_expr`)로만 넣는다.
//! - 예외 필터(결정 M6-D5)는 어댑터마다 켠 필터 이름을 둔다 — 어댑터가 알린 것만 켤 수 있다(세션이 본다).
//!
//! 형식(`schema = 1`):
//!
//! ```json
//! { "schema": 1,
//!   "files": { "src/main.rs": [{ "line": 3, "enabled": true, "condition": "i > 3" }] },
//!   "exceptionFilters": { "codelldb": ["rust_panic"] } }
//! ```
//!
//! 쓰기는 같은 폴더의 임시 파일 → rename(원자적)이고 읽기와 같은 크기 상한([`FILE_LIMIT`])을 넘으면 거절한다. 읽지 못한 파일(형식
//! 오류 · 상한 초과)은 지우지 않고 `breakpoints.json.broken-<유닉스 초>` 로 옮긴 뒤 빈 목록으로 시작하고 그 까닭을 `debug.breakpoints.list`
//! 의 `problem` 으로 알린다. 옮기지 못하면(또는 읽기 자체가 실패하면) 그 파일을 덮어쓰지 않도록 쓰기를 막는다.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use pw_core::registry::state::{PROJECTS_DIR, valid_id};
use pw_rpc::{RpcError, code};
use serde::{Deserialize, Serialize};

/// 파일 이름.
pub const FILE: &str = "breakpoints.json";
/// 파일의 스키마 판.
pub const SCHEMA: u32 = 1;
/// 파일 크기 상한(바이트). 넘는 파일은 읽지 않는다.
pub const FILE_LIMIT: usize = 4 * 1024 * 1024;
/// 파일 하나의 브레이크포인트 수 상한.
pub const PER_FILE_LIMIT: usize = 1000;
/// 조건 · 로그 메시지 · hit count 글 하나의 상한(바이트).
pub const TEXT_LIMIT: usize = 4096;
/// 상대 경로의 상한(바이트).
pub const PATH_LIMIT: usize = 4096;

/// 브레이크포인트 하나(줄은 1부터).
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct Breakpoint {
    pub line: u32,
    /// 꺼 둔 것은 어댑터에 보내지 않는다.
    #[serde(default = "yes")]
    pub enabled: bool,
    /// 멈출 조건 — 디버그 대상 안에서 평가되는 식.
    #[serde(default)]
    pub condition: Option<String>,
    /// 멈추지 않고 이 글을 찍는다(로그포인트, `{식}` 은 대상 안에서 평가된다).
    #[serde(default)]
    pub log_message: Option<String>,
    /// 몇 번째에 멈추나(어댑터의 문법 — 대상 안에서 평가된다).
    #[serde(default)]
    pub hit_condition: Option<String>,
    /// 에이전트가 두었거나 바꾸었다(M6-20 의 툴 — 거터의 표지). 사람이 같은 줄을 다른 내용으로 저장하면 사람 것이 된다
    /// ([`Breakpoints::set_file`] — 같은 줄 · 같은 내용이면 화면이 이 칸을 되돌려 보내지 않아도 남긴다).
    #[serde(default)]
    pub agent: bool,
}

fn yes() -> bool {
    true
}

impl Breakpoint {
    pub fn at(line: u32) -> Self {
        Self {
            line,
            enabled: true,
            condition: None,
            log_message: None,
            hit_condition: None,
            agent: false,
        }
    }

    /// 표지(`agent`)를 빼고 같은가 — 사람의 저장이 그 줄을 바꾸었는지 볼 때.
    pub fn same_content(&self, other: &Self) -> bool {
        Self {
            agent: false,
            ..self.clone()
        } == Self {
            agent: false,
            ..other.clone()
        }
    }

    /// 대상 안에서 평가되는 글(조건 · 로그 메시지 · hit count)이 있는가.
    pub fn has_expressions(&self) -> bool {
        self.condition.is_some() || self.log_message.is_some() || self.hit_condition.is_some()
    }
}

/// 파일 하나의 브레이크포인트.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct FileBreakpoints {
    /// 프로젝트 루트 기준 상대 경로(`/` 로 나눔).
    pub path: String,
    pub breakpoints: Vec<Breakpoint>,
}

/// 한 프로젝트의 저장 내용.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Stored {
    #[serde(default)]
    pub schema: u32,
    #[serde(default)]
    pub files: BTreeMap<String, Vec<Breakpoint>>,
    #[serde(default)]
    pub exception_filters: BTreeMap<String, Vec<String>>,
    /// 읽지 못한 파일을 옆으로 옮겨 두었다(까닭 · 옮긴 자리). 다음 쓰기가 성공하면 지운다. 파일에는 쓰지 않는다.
    #[serde(skip)]
    pub problem: Option<String>,
    /// 파일이 그 자리에 남아 있는데 읽지 못했다(옮기지도 못함) — 덮어쓰지 않게 쓰기를 막는다.
    #[serde(skip)]
    pub locked: bool,
}

impl Stored {
    pub fn file_list(&self) -> Vec<FileBreakpoints> {
        self.files
            .iter()
            .map(|(path, breakpoints)| FileBreakpoints {
                path: path.clone(),
                breakpoints: breakpoints.clone(),
            })
            .collect()
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// 프로젝트 루트 기준 상대 경로를 검사하고 정규형(`/` 로 나눔)으로. 절대 경로 · `..` · `.` · 빈 마디 · 너무 긴 경로는 invalid
/// params.
pub fn relative_path(path: &str) -> Result<String, RpcError> {
    let bad = |why: &str| RpcError::invalid_params(format!("breakpoint path `{path}` {why}"));
    if path.is_empty() || path.len() > PATH_LIMIT {
        return Err(bad("must be a non-empty relative path"));
    }
    let unified = path.replace('\\', "/");
    if unified.starts_with('/') || Path::new(path).is_absolute() || unified.contains(':') {
        return Err(bad("must be relative to the project root"));
    }
    let parts: Vec<&str> = unified.split('/').collect();
    if parts
        .iter()
        .any(|p| p.is_empty() || *p == "." || *p == "..")
    {
        return Err(bad("must not have empty, `.` or `..` parts"));
    }
    if unified.contains('\0') {
        return Err(bad("must not contain NUL"));
    }
    Ok(parts.join("/"))
}

/// 목록을 정리한다: 줄 0 · 너무 긴 글 · 너무 많은 수는 invalid params, 같은 줄은 뒤의 것이 이기고, 줄 차례로.
pub fn normalized(list: Vec<Breakpoint>) -> Result<Vec<Breakpoint>, RpcError> {
    if list.len() > PER_FILE_LIMIT {
        return Err(RpcError::invalid_params(format!(
            "at most {PER_FILE_LIMIT} breakpoints per file"
        )));
    }
    let mut by_line = BTreeMap::new();
    for b in list {
        if b.line == 0 {
            return Err(RpcError::invalid_params("breakpoint lines start at 1"));
        }
        for text in [&b.condition, &b.log_message, &b.hit_condition]
            .into_iter()
            .flatten()
        {
            if text.len() > TEXT_LIMIT {
                return Err(RpcError::invalid_params(format!(
                    "a breakpoint condition, log message or hit count is at most {TEXT_LIMIT} bytes"
                )));
            }
        }
        let b = Breakpoint {
            condition: b.condition.filter(|s| !s.trim().is_empty()),
            log_message: b.log_message.filter(|s| !s.is_empty()),
            hit_condition: b.hit_condition.filter(|s| !s.trim().is_empty()),
            ..b
        };
        by_line.insert(b.line, b);
    }
    Ok(by_line.into_values().collect())
}

/// 저장소. 프로젝트마다 처음 쓸 때 파일을 읽어 두고, 바꿀 때마다 파일에 쓴다. 복제해도 같은 저장소다.
#[derive(Clone)]
pub struct Breakpoints {
    inner: Arc<Inner>,
}

struct Inner {
    data: PathBuf,
    projects: Mutex<HashMap<String, Arc<Mutex<Stored>>>>,
    /// 바뀔 때마다 오른다(세션이 시작 뒤 다시 보낼지 가른다).
    generation: AtomicU64,
}

impl Breakpoints {
    /// `$DATA`(앱 데이터 폴더) 아래에 둔다.
    pub fn new(data: PathBuf) -> Self {
        Self {
            inner: Arc::new(Inner {
                data,
                projects: Mutex::default(),
                generation: AtomicU64::new(0),
            }),
        }
    }

    /// `$DATA/projects/<id>/breakpoints.json`.
    pub fn file(&self, project_id: &str) -> Result<PathBuf, RpcError> {
        if !valid_id(project_id) {
            return Err(RpcError::invalid_params(format!(
                "`{project_id}` is not a project id"
            )));
        }
        Ok(self
            .inner
            .data
            .join(PROJECTS_DIR)
            .join(project_id)
            .join(FILE))
    }

    /// 바뀐 횟수.
    pub fn generation(&self) -> u64 {
        self.inner.generation.load(Ordering::SeqCst)
    }

    fn project(&self, project_id: &str) -> Result<Arc<Mutex<Stored>>, RpcError> {
        let file = self.file(project_id)?;
        let mut projects = lock(&self.inner.projects);
        if let Some(stored) = projects.get(project_id) {
            return Ok(Arc::clone(stored));
        }
        let stored = Arc::new(Mutex::new(read(&file)));
        projects.insert(project_id.to_owned(), Arc::clone(&stored));
        Ok(stored)
    }

    /// 그 프로젝트의 저장 내용.
    pub fn get(&self, project_id: &str) -> Result<Stored, RpcError> {
        let project = self.project(project_id)?;
        let stored = lock(&project).clone();
        Ok(stored)
    }

    /// `change` 로 고치고 파일에 쓴다. 쓰지 못하면 고친 것을 되돌리고 오류.
    fn change<T>(
        &self,
        project_id: &str,
        change: impl FnOnce(&mut Stored) -> Result<T, RpcError>,
    ) -> Result<T, RpcError> {
        let file = self.file(project_id)?;
        let project = self.project(project_id)?;
        let mut stored = lock(&project);
        if stored.locked {
            return Err(RpcError::internal(format!(
                "the breakpoints are not saved: {}",
                stored
                    .problem
                    .as_deref()
                    .unwrap_or("the file could not be read")
            )));
        }
        let before = stored.clone();
        let out = change(&mut stored)?;
        if *stored == before {
            return Ok(out);
        }
        stored.schema = SCHEMA;
        stored.files.retain(|_, list| !list.is_empty());
        let bytes = match serde_json::to_vec_pretty(&*stored) {
            Ok(bytes) => bytes,
            Err(e) => {
                *stored = before;
                return Err(RpcError::internal(format!(
                    "cannot save the breakpoints: {e}"
                )));
            }
        };
        // 읽기와 같은 상한 — 넘는 파일을 쓰면 다음 시작에 읽지 못한다.
        if bytes.len() > FILE_LIMIT {
            *stored = before;
            return Err(RpcError::invalid_params(format!(
                "the breakpoints of this project would take {} bytes, over the {FILE_LIMIT} byte limit — remove some breakpoints or shorten their conditions and log messages",
                bytes.len()
            )));
        }
        if let Err(e) = write(&file, &bytes) {
            *stored = before;
            return Err(RpcError::internal(format!(
                "cannot save the breakpoints: {e}"
            )));
        }
        stored.problem = None;
        self.inner.generation.fetch_add(1, Ordering::SeqCst);
        Ok(out)
    }

    /// 파일 하나의 목록을 통째로 바꾼다(빈 목록이면 지운다). 정리한 목록을 돌려준다.
    pub fn set_file(
        &self,
        project_id: &str,
        path: &str,
        list: Vec<Breakpoint>,
    ) -> Result<Vec<Breakpoint>, RpcError> {
        let path = relative_path(path)?;
        let list = normalized(list)?;
        self.change(project_id, |s| {
            // 같은 줄 · 같은 내용(표지 빼고)이던 에이전트의 것은 표지를 남긴다 — 화면은 이 칸을 되돌려 보내지 않아도 된다.
            let before = s.files.get(&path).cloned().unwrap_or_default();
            let list: Vec<Breakpoint> = list
                .into_iter()
                .map(|mut b| {
                    if before
                        .iter()
                        .any(|old| old.agent && old.line == b.line && old.same_content(&b))
                    {
                        b.agent = true;
                    }
                    b
                })
                .collect();
            s.files.insert(path, list.clone());
            Ok(list)
        })
    }

    /// 한 줄을 바꾼다 — 그 줄의 지금 것이 `expected` 일 때만(없음도 값이다). 다르면 `CONFLICT`(`data.reason = "changed"`)이고
    /// 아무것도 바꾸지 않는다. `new` 가 없으면 그 줄을 지운다(`new` 의 줄은 `line` 이어야 한다). 그 파일의 목록을 돌려준다.
    ///
    /// 툴(M6-20)이 승인 뒤에 쓴다: 계획 때 읽은 그 줄을 `expected` 로 넘겨, 승인을 기다리는 동안 사람이 그 줄을 바꾸었으면(조건을
    /// 넣음 · 지움) 승인 화면에 없던 것을 덮어쓰거나 켜지 않는다.
    pub fn replace_line(
        &self,
        project_id: &str,
        path: &str,
        line: u32,
        expected: Option<&Breakpoint>,
        new: Option<Breakpoint>,
    ) -> Result<Vec<Breakpoint>, RpcError> {
        let path = relative_path(path)?;
        if new.as_ref().is_some_and(|b| b.line != line) {
            return Err(RpcError::internal(
                "replace_line: the new breakpoint is on another line",
            ));
        }
        self.change(project_id, |s| {
            let mut list = s.files.get(&path).cloned().unwrap_or_default();
            let now = list.iter().find(|b| b.line == line);
            if now != expected {
                return Err(RpcError::new(
                    code::CONFLICT,
                    format!(
                        "the breakpoint at {path}:{line} changed after this call was planned; nothing was changed — \
                         read it again with breakpoint_list"
                    ),
                )
                .with_data(serde_json::json!({ "reason": "changed" })));
            }
            list.retain(|b| b.line != line);
            list.extend(new);
            let list = normalized(list)?;
            s.files.insert(path, list.clone());
            Ok(list)
        })
    }

    /// 하나를 넣거나 같은 줄의 것을 바꾼다. 그 파일의 목록을 돌려준다.
    pub fn upsert(
        &self,
        project_id: &str,
        path: &str,
        breakpoint: Breakpoint,
    ) -> Result<Vec<Breakpoint>, RpcError> {
        let path = relative_path(path)?;
        self.change(project_id, |s| {
            let mut list = s.files.get(&path).cloned().unwrap_or_default();
            list.push(breakpoint);
            let list = normalized(list)?;
            s.files.insert(path, list.clone());
            Ok(list)
        })
    }

    /// 한 줄(`line` 이 없으면 그 파일 전부)을 지운다. 그 파일의 남은 목록을 돌려준다.
    pub fn remove(
        &self,
        project_id: &str,
        path: &str,
        line: Option<u32>,
    ) -> Result<Vec<Breakpoint>, RpcError> {
        let path = relative_path(path)?;
        self.change(project_id, |s| {
            let list = s.files.entry(path).or_default();
            match line {
                Some(line) => list.retain(|b| b.line != line),
                None => list.clear(),
            }
            Ok(list.clone())
        })
    }

    /// 어댑터가 옮긴 줄을 저장에 반영한다: `from` 줄의 것을 `to` 로. `to` 에 이미 있으면 옮긴 것을 버린다(같은 줄에 둘은 두지 않는다).
    /// 그 파일의 목록을 돌려준다(바뀐 것이 없으면 `None`).
    pub fn moved(
        &self,
        project_id: &str,
        path: &str,
        moves: &[(u32, u32)],
    ) -> Result<Option<Vec<Breakpoint>>, RpcError> {
        let path = relative_path(path)?;
        self.change(project_id, |s| {
            let Some(list) = s.files.get_mut(&path) else {
                return Ok(None);
            };
            let before = list.clone();
            // 옮길 곳을 원래 목록 기준으로 한꺼번에 정한다 — 사슬(2→5, 5→7)을 차례로 적용하면 2 의 것이 「5 에 이미 있다」 로
            // 버려진다. 조건 · 로그 메시지 · hit count 는 브레이크포인트와 함께 간다.
            let target = |line: u32| {
                moves
                    .iter()
                    .find(|(from, to)| *from == line && *to != 0)
                    .map_or(line, |(_, to)| *to)
            };
            let mut placed: BTreeMap<u32, (bool, Breakpoint)> = BTreeMap::new();
            for b in before.iter().cloned() {
                let to = target(b.line);
                let stayed = to == b.line;
                let b = Breakpoint { line: to, ..b };
                match placed.get(&to) {
                    // 같은 줄에 둘이 오면 제자리에 있던 것이 이긴다(둘 다 옮겨 왔으면 먼저 온 것).
                    Some((true, _)) => {}
                    Some((false, _)) if !stayed => {}
                    _ => {
                        placed.insert(to, (stayed, b));
                    }
                }
            }
            *list = placed.into_values().map(|(_, b)| b).collect();
            Ok((*list != before).then(|| list.clone()))
        })
    }

    /// 어댑터 하나에 켠 예외 필터.
    pub fn exception_filters(&self, project_id: &str, adapter: &str) -> Vec<String> {
        self.get(project_id)
            .ok()
            .and_then(|s| s.exception_filters.get(adapter).cloned())
            .unwrap_or_default()
    }

    /// 어댑터 하나에 켤 예외 필터를 바꾼다(알린 것인지는 부른 쪽이 본다).
    pub fn set_exception_filters(
        &self,
        project_id: &str,
        adapter: &str,
        filters: Vec<String>,
    ) -> Result<(), RpcError> {
        self.change(project_id, |s| {
            if filters.is_empty() {
                s.exception_filters.remove(adapter);
            } else {
                s.exception_filters.insert(adapter.to_owned(), filters);
            }
            Ok(())
        })
    }
}

fn read(file: &Path) -> Stored {
    let bytes = match std::fs::read(file) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Stored::default(),
        Err(e) => {
            // 읽지 못한 것(권한 등)은 옮기지도 않는다 — 고쳐지면 다시 읽힌다. 다음 쓰기는 실패한다.
            tracing::warn!(target: "pw_debug", file = %file.display(), error = %e, "cannot read the breakpoints");
            return Stored {
                problem: Some(format!("cannot read {}: {e}", file.display())),
                locked: true,
                ..Stored::default()
            };
        }
    };
    if bytes.len() > FILE_LIMIT {
        return set_aside(
            file,
            format!("{} is over the {FILE_LIMIT} byte limit", file.display()),
        );
    }
    match serde_json::from_slice::<Stored>(&bytes) {
        Ok(mut stored) => {
            // 손으로 고친 파일도 같은 규칙으로 — 쓸 수 없는 열쇠 · 목록은 버린다.
            stored.files = std::mem::take(&mut stored.files)
                .into_iter()
                .filter_map(|(path, list)| {
                    Some((relative_path(&path).ok()?, normalized(list).ok()?))
                })
                .collect();
            stored
        }
        Err(e) => set_aside(file, format!("{} is not readable: {e}", file.display())),
    }
}

/// 읽지 못한 파일을 지우지 않고 옆(`breakpoints.json.broken-<유닉스 초>`)으로 옮기고, 빈 목록에 그 까닭을 달아 시작한다 — 다음
/// 쓰기가 사람의 파일을 덮어쓰지 않게. 옮기지 못하면 그 파일을 그대로 두고 쓰기를 막는다(`problem` 이 남는다).
fn set_aside(file: &Path, why: String) -> Stored {
    let at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let aside = file.with_file_name(format!("{FILE}.broken-{at}"));
    let (problem, locked) = match std::fs::rename(file, &aside) {
        Ok(()) => (format!("{why}; it was moved to {}", aside.display()), false),
        Err(e) => (format!("{why}; it could not be moved aside: {e}"), true),
    };
    tracing::warn!(target: "pw_debug", %problem, "the breakpoints file was not used");
    Stored {
        problem: Some(problem),
        locked,
        ..Stored::default()
    }
}

fn write(file: &Path, bytes: &[u8]) -> std::io::Result<()> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let dir = file.parent().expect("the file has a folder");
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(
        ".{FILE}.{}.{}.pitwall-tmp",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| {
        // 쓰기 핸들로 동기화한다 — Windows 의 `FlushFileBuffers` 는 읽기 전용 핸들(`File::open`)에 「Access is denied」 로
        // 실패해 저장이 늘 실패했다(CI run 45299304602).
        let mut out = std::fs::File::create(&tmp)?;
        std::io::Write::write_all(&mut out, bytes)?;
        out.sync_all()?;
        drop(out);
        std::fs::rename(&tmp, file)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_paths_are_checked() {
        assert_eq!(relative_path("src/main.rs").unwrap(), "src/main.rs");
        assert_eq!(relative_path("src\\a.rs").unwrap(), "src/a.rs");
        for bad in ["", "/etc/passwd", "../x", "a/../b", "a//b", "./a", "C:/x"] {
            assert!(relative_path(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_line_holds_one_breakpoint_and_lines_are_sorted() {
        let list = normalized(vec![
            Breakpoint::at(9),
            Breakpoint::at(3),
            Breakpoint {
                condition: Some("i > 3".into()),
                ..Breakpoint::at(9)
            },
        ])
        .unwrap();
        assert_eq!(list.iter().map(|b| b.line).collect::<Vec<_>>(), [3, 9]);
        assert_eq!(list[1].condition.as_deref(), Some("i > 3"));
        assert!(normalized(vec![Breakpoint::at(0)]).is_err());
    }

    #[test]
    fn a_moved_line_onto_an_existing_one_is_dropped() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Breakpoints::new(tmp.path().to_owned());
        store
            .set_file("p1", "a.rs", vec![Breakpoint::at(2), Breakpoint::at(5)])
            .unwrap();
        let after = store.moved("p1", "a.rs", &[(2, 5)]).unwrap().unwrap();
        assert_eq!(after, [Breakpoint::at(5)]);
        assert_eq!(store.moved("p1", "a.rs", &[(7, 8)]).unwrap(), None);
    }

    /// 리뷰 F2: 어댑터가 사슬로 옮겨도(2→5, 5→7) 둘 다 남고, 조건은 옮긴 브레이크포인트와 함께 간다.
    #[test]
    fn a_chain_of_moves_keeps_every_breakpoint_and_its_condition() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Breakpoints::new(tmp.path().to_owned());
        let conditional = Breakpoint {
            condition: Some("x".into()),
            log_message: Some("at {x}".into()),
            hit_condition: Some("3".into()),
            ..Breakpoint::at(2)
        };
        store
            .set_file("p1", "a.rs", vec![conditional.clone(), Breakpoint::at(5)])
            .unwrap();
        let after = store
            .moved("p1", "a.rs", &[(2, 5), (5, 7)])
            .unwrap()
            .unwrap();
        assert_eq!(
            after,
            [
                Breakpoint {
                    line: 5,
                    ..conditional
                },
                Breakpoint::at(7)
            ]
        );
    }

    fn big(lines: u32) -> Vec<Breakpoint> {
        (1..=lines)
            .map(|line| Breakpoint {
                condition: Some("c".repeat(TEXT_LIMIT)),
                ..Breakpoint::at(line)
            })
            .collect()
    }

    /// 리뷰 F1: 읽기 상한을 넘게 하는 쓰기는 거절되고(invalid params, 무엇이 넘는지), 전에 쓴 것은 그대로 남아 다시 읽힌다.
    #[test]
    fn a_write_over_the_read_limit_is_refused_and_keeps_what_was_there() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Breakpoints::new(tmp.path().to_owned());
        store
            .set_file("p1", "small.rs", vec![Breakpoint::at(1)])
            .unwrap();
        let e = store.set_file("p1", "a.rs", big(1000)).unwrap_err();
        assert_eq!(e.code, pw_rpc::code::INVALID_PARAMS);
        let said = format!("{:?}", e.data);
        assert!(said.contains("byte limit"), "{said}");
        assert_eq!(store.get("p1").unwrap().files.len(), 1);
        let again = Breakpoints::new(tmp.path().to_owned()).get("p1").unwrap();
        assert_eq!(again.files["small.rs"], [Breakpoint::at(1)]);
        assert_eq!(again.problem, None);
    }

    /// 상한을 넘거나 깨진 파일은 지우지 않고 옆으로 옮기고(`.broken-<시각>`) 그 까닭을 알린다. 다음 쓰기는 새 파일을 만든다.
    #[test]
    fn an_unreadable_file_is_set_aside_not_emptied() {
        for (name, bytes) in [
            // 상한만 넘는 올바른 JSON(뒤의 공백) — 형식 오류로 걸리지 않게.
            ("over", {
                let mut bytes =
                    br#"{ "schema": 1, "files": { "a.rs": [{ "line": 1 }] } }"#.to_vec();
                bytes.resize(FILE_LIMIT + 1, b' ');
                bytes
            }),
            ("broken", b"{ not json".to_vec()),
        ] {
            let tmp = tempfile::tempdir().unwrap();
            let store = Breakpoints::new(tmp.path().to_owned());
            let file = store.file("p1").unwrap();
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(&file, &bytes).unwrap();
            let stored = store.get("p1").unwrap();
            assert!(stored.files.is_empty(), "{name}");
            let problem = stored.problem.expect(name);
            assert!(problem.contains("moved to"), "{name}: {problem}");
            let aside: Vec<_> = std::fs::read_dir(file.parent().unwrap())
                .unwrap()
                .filter_map(|e| e.ok())
                .filter(|e| {
                    e.file_name()
                        .to_string_lossy()
                        .starts_with("breakpoints.json.broken-")
                })
                .collect();
            assert_eq!(aside.len(), 1, "{name}");
            assert_eq!(
                std::fs::read(aside[0].path()).unwrap(),
                bytes,
                "{name}: kept as it was"
            );
            store
                .set_file("p1", "a.rs", vec![Breakpoint::at(1)])
                .unwrap();
            assert_eq!(store.get("p1").unwrap().problem, None, "{name}");
        }
    }

    /// 리뷰 권고 1(C): 조건 · 로그 메시지 · hit count 글 하나는 상한까지만.
    /// 한 줄 바꾸기는 그 줄의 지금 것이 기대한 것일 때만이다(없음도 값이다) — 다르면 `CONFLICT` 이고 아무것도 바꾸지 않는다.
    #[test]
    fn a_line_is_replaced_only_when_it_is_what_was_expected() {
        let dir = tempfile::tempdir().unwrap();
        let store = Breakpoints::new(dir.path().to_path_buf());
        let plain = Breakpoint::at(4);
        let conditional = Breakpoint {
            condition: Some("n == 2".into()),
            ..Breakpoint::at(4)
        };
        store
            .set_file("p1", "a.rs", vec![conditional.clone()])
            .unwrap();
        // 계획 때는 없던 줄 · 평범한 것이었다 — 지금은 사람의 조건이 있다.
        for expected in [None, Some(&plain)] {
            let e = store
                .replace_line(
                    "p1",
                    "a.rs",
                    4,
                    expected,
                    Some(Breakpoint {
                        agent: true,
                        ..plain.clone()
                    }),
                )
                .unwrap_err();
            assert_eq!(e.code, code::CONFLICT);
            let e = store
                .replace_line("p1", "a.rs", 4, expected, None)
                .unwrap_err();
            assert_eq!(e.code, code::CONFLICT);
        }
        assert_eq!(
            store.get("p1").unwrap().files["a.rs"],
            std::slice::from_ref(&conditional)
        );
        let off = Breakpoint {
            enabled: false,
            agent: true,
            ..conditional.clone()
        };
        let saved = store
            .replace_line("p1", "a.rs", 4, Some(&conditional), Some(off.clone()))
            .unwrap();
        assert_eq!(saved, [off]);
    }

    #[test]
    fn a_text_over_its_limit_is_refused() {
        let over = Breakpoint {
            log_message: Some("x".repeat(TEXT_LIMIT + 1)),
            ..Breakpoint::at(1)
        };
        assert!(normalized(vec![over]).is_err());
        assert!(normalized(big(1)).is_ok());
    }

    /// 리뷰 권고 1(E): 손으로 고친 파일의 루트 밖 · 절대 경로 열쇠와 쓸 수 없는 목록(줄 0)은 버린다 — 나머지는 읽는다.
    #[test]
    fn a_hand_edited_file_keeps_only_usable_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Breakpoints::new(tmp.path().to_owned());
        let file = store.file("p1").unwrap();
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(
            &file,
            r#"{ "schema": 1, "files": {
                "../../outside.rs": [{ "line": 1 }],
                "/etc/passwd": [{ "line": 1 }],
                "zero.rs": [{ "line": 0 }],
                "src/ok.rs": [{ "line": 4 }]
            } }"#,
        )
        .unwrap();
        let files = store.get("p1").unwrap().files;
        assert_eq!(files.keys().collect::<Vec<_>>(), ["src/ok.rs"]);
    }
}
