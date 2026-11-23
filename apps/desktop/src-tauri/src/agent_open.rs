//! 에이전트의 `project_open`(16-m5-plan M5-09 ③ · 결정 M5-D24)을 창으로 옮긴다. Host 의 툴 메서드(`pw_core::open`)가 판정한
//! 요청을 `pitwall <path>` 와 같은 길(창별 보류 큐 · `project://open` — [`launch::open_project_as`])로 연다. 다른 점은 하나다 —
//! **앞으로 가져오지 않는다**([`windows::Activation::Background`]): 그 프로젝트 창이 열려 있으면 큐에만 넣고(창이 탭을 연다),
//! 없으면 포커스 없이 만든다. 새 창도 신뢰 관문은 그대로다 — 미신뢰 프로젝트의 창은 제한 모드라 런타임 제공자 · 언어 서버가
//! 뜨지 않는다(신뢰는 사람만 바꾼다 — `project.setTrust` 는 MCP 세션을 거절한다).

use pw_core::open::{OpenRequest, OpenedIn, Opener};
use tauri::{AppHandle, Runtime};

use crate::launch;
use crate::windows::{Activation, ProjectOpen};

/// 앱의 setup 에서 [`pw_core::open::OpenRequests::attach`] 로 붙인다.
pub struct WindowOpener<R: Runtime>(AppHandle<R>);

impl<R: Runtime> WindowOpener<R> {
    pub fn new(app: AppHandle<R>) -> Self {
        Self(app)
    }
}

impl<R: Runtime> Opener for WindowOpener<R> {
    fn open(&self, request: &OpenRequest) -> Result<OpenedIn, String> {
        let open = ProjectOpen {
            worktree: request.worktree.clone(),
            file: request.file.as_ref().map(|f| f.display().to_string()),
            line: request.line,
            col: request.column,
            checkout: None,
        };
        let created =
            launch::open_project_as(&self.0, &request.project_id, &open, Activation::Background)?;
        Ok(if created {
            OpenedIn::NewWindow
        } else {
            OpenedIn::ExistingWindow
        })
    }
}
