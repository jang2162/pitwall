//! 브랜치 · 체크아웃 · 태그 시험(13-m2-plan.md M2-09 완료 기준의 브랜치 쪽). 진짜 git · 임시 폴더의 시험 저장소 · 진짜 git 을
//! PATH 에 둔 가짜 로그인 셸(`crate::testing::write`)로 돈다(unix). 원격은 로컬 bare 저장소다(`crate::testing::remote`).

mod branch;
mod checkout;
mod untrusted;
mod wire;

use crate::testing::write::{Window, WriteFixture};
use crate::write::{GitPromptAnswer, GitPromptRequest};

/// 미저장 확인에 바로 「진행」 으로 답하는 프로젝트 창.
fn proceed(f: &WriteFixture) -> Window {
    Window::open(&f.write, &f.project, |_| Some(GitPromptAnswer::Proceed))
}

/// 창이 받은 미저장 확인의 경로들(`None` 은 작업 트리 전체).
fn unsaved(window: &Window) -> Vec<Option<Vec<String>>> {
    window
        .seen()
        .into_iter()
        .filter_map(|request| match request {
            GitPromptRequest::Unsaved { paths, .. } => Some(paths),
            GitPromptRequest::Askpass { .. } => None,
        })
        .collect()
}

/// `git.*` 오류의 git 원문(`2004` 의 `data.stderr`).
fn stderr_of(e: &crate::write::WriteError) -> String {
    match e {
        crate::write::WriteError::Failed { stderr, .. } => stderr.clone(),
        other => panic!("expected a git failure, got {other:?}"),
    }
}
