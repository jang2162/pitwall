//! 좁은 명령 `askpass` · `open` 의 몸통(18-m7-plan M7-15 ① ②). 데몬 소켓으로 같은 사용자 프로세스가 기기 키 없이 부르는 길이라,
//! 이 둘이 할 수 있는 것은 「붙은 Desktop 의 사람 화면에 원격이 쓴 글을 띄우는 것」 까지다 — 그 범위를 여기서 좁힌다.
//!
//! - **askpass**: 원격 git · ssh 가 Relay 의 쓰기 중에 묻는 암호. 도우미(`pitwall-relay askpass` — 메인 판정 M7-P12)가 그 쓰기의
//!   토큰과 질문을 넘기면 Relay Host 의 askpass(`pw_git::write::Askpass::answer`)가 지금처럼 `git.prompts` 스트림에 낸다 — 그
//!   프로젝트 창 연결(링크로 연 Desktop 의 그 묶음 창)만 듣고 답한다. 토큰이 없는 · 끝난 쓰기의 질문은 거절이고(그래서 같은 uid
//!   프로세스도 도는 쓰기 없이는 다이얼로그를 띄우지 못한다), 듣는 창이 없으면 기다리지 않고 실패한다. 질문 글은 제어 · 양방향
//!   글자를 빼고 512 자에서 자른다(`askpass_text`). 질문은 한 번에 [`MAX_WAITING`] 개까지 기다린다(홍수).
//! - **open**: `pitwall-relay open <경로>`. 경로는 절대 경로(도우미가 cwd 로 푼다)이고 [`MAX_PATH`] 바이트 · 제어 글자 없음.
//!   그 경로를 품은 이 머신의 프로젝트(레지스트리 — `relay.toml` 의 `roots` 안에서 등록된 것)를 찾아 붙은 Desktop 에 넘긴다
//!   (`pw_core::open::desks` — 행선지 · 거절 규칙). 에이전트의 `project_open` 과 빈도 한 벌을 나눈다(10초에 5번).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use pw_core::open::{OpenRequest, OpenRequests};
use pw_core::registry::Registry;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::Semaphore;

use crate::command::{BoxReply, NarrowHandler};

/// 한 번에 기다리는 좁은 명령(`askpass` · `open`) 수.
pub const MAX_WAITING: usize = 8;
/// `open` 경로의 최대 바이트.
pub const MAX_PATH: usize = 4096;

/// Relay 의 좁은 명령 몸통.
pub struct RelayNarrow {
    askpass: pw_git::write::Askpass,
    requests: OpenRequests,
    registry: Option<Registry>,
    waiting: Arc<Semaphore>,
}

impl RelayNarrow {
    pub fn new(
        askpass: pw_git::write::Askpass,
        requests: OpenRequests,
        registry: Option<Registry>,
    ) -> Self {
        Self {
            askpass,
            requests,
            registry,
            waiting: Arc::new(Semaphore::new(MAX_WAITING)),
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OpenBody {
    path: String,
    #[serde(default)]
    line: Option<u32>,
    #[serde(default)]
    column: Option<u32>,
}

impl NarrowHandler for RelayNarrow {
    fn askpass(&self, body: Value) -> BoxReply<'_> {
        Box::pin(async move {
            let Ok(_permit) = self.waiting.try_acquire() else {
                return Err("too many relay commands are waiting".to_owned());
            };
            self.askpass.answer(body).await
        })
    }

    fn open(&self, body: Value) -> BoxReply<'_> {
        Box::pin(async move {
            let Ok(_permit) = self.waiting.try_acquire() else {
                return Err("too many relay commands are waiting".to_owned());
            };
            let body: OpenBody =
                serde_json::from_value(body).map_err(|e| format!("bad open request: {e}"))?;
            let registry = self
                .registry
                .clone()
                .ok_or_else(|| "this relay has no project registry".to_owned())?;
            let requests = self.requests.clone();
            tokio::task::spawn_blocking(move || {
                let request = prepare(&registry, &body)?;
                let opened = requests.open(&request).map_err(|e| e.to_string())?;
                Ok(json!({
                    "openedIn": opened,
                    "path": request.file.map(|f| f.display().to_string()),
                }))
            })
            .await
            .map_err(|e| e.to_string())?
        })
    }
}

/// 경로 → 이 머신 프로젝트의 열기 요청. 사람(원격 셸)이 읽는 영어 오류.
fn prepare(registry: &Registry, body: &OpenBody) -> Result<OpenRequest, String> {
    let path = &body.path;
    if path.len() > MAX_PATH {
        return Err(format!("the path is longer than {MAX_PATH} bytes"));
    }
    if path.chars().any(char::is_control) {
        return Err("the path has control characters".to_owned());
    }
    if !Path::new(path).is_absolute() {
        return Err("the path must be absolute".to_owned());
    }
    if body.line == Some(0) || body.column == Some(0) {
        return Err("line and column start at 1".to_owned());
    }
    if body.line.is_none() && body.column.is_some() {
        return Err("a column needs a line".to_owned());
    }
    let resolution = registry
        .resolve(Path::new(path))
        .map_err(|e| format!("cannot read the project registry: {e}"))?
        .ok_or_else(|| {
            "the path is not inside a project registered on this machine (add it from Pitwall first)"
                .to_owned()
        })?;
    let file = PathBuf::from(&resolution.path);
    let file = (!file.is_dir()).then_some(file);
    Ok(OpenRequest {
        project_id: resolution.project.id,
        worktree: resolution.worktree.map(|w| w.top),
        line: file.as_ref().and(body.line),
        column: file.as_ref().and(body.column),
        file,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(path: &str) -> OpenBody {
        OpenBody {
            path: path.to_owned(),
            line: None,
            column: None,
        }
    }

    /// 등록된 프로젝트 안의 경로만 연다. 밖 · 상대 경로 · 제어 글자 · 너무 긴 경로는 거절한다.
    #[test]
    fn only_paths_inside_a_registered_project_are_opened() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let root = base.join("work");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("README.md"), "x").unwrap();
        std::fs::write(base.join("secret"), "x").unwrap();
        let registry = Registry::new(
            pw_core::config::AppPaths::under(&base.join("app")),
            None,
            pw_core::EventBus::new(16),
        );
        let id = registry
            .create(pw_core::registry::NewProject {
                root: root.clone(),
                ..Default::default()
            })
            .unwrap()
            .id;

        let mut file = body(&root.join("README.md").display().to_string());
        file.line = Some(4);
        let request = prepare(&registry, &file).unwrap();
        assert_eq!(request.project_id, id);
        assert_eq!(request.worktree, None);
        assert_eq!(request.file, Some(root.join("README.md")));
        assert_eq!(request.line, Some(4));

        let folder = prepare(&registry, &body(&root.join("src").display().to_string())).unwrap();
        assert_eq!((folder.file, folder.line), (None, None));

        for refused in [
            base.join("secret").display().to_string(),
            "work/README.md".to_owned(),
            format!("{}/READ\u{1b}[2JME.md", root.display()),
            format!("/{}", "a".repeat(MAX_PATH)),
        ] {
            assert!(prepare(&registry, &body(&refused)).is_err(), "{refused}");
        }
    }
}
