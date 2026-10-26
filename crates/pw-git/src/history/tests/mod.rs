//! 이력 쓰기 시험(13-m2-plan.md M2-10 완료 기준). 쓰기 실행기 시험과 같이 진짜 git · 임시 폴더의 시험 저장소 · 진짜 git 을 PATH 에
//! 둔 가짜 로그인 셸로 돈다(unix — `crate::testing::write`). 입력 검사는 OS 와 상관없이 돈다.

mod input;

#[cfg(unix)]
mod lost;
#[cfg(unix)]
mod merge;
#[cfg(unix)]
mod pick;
#[cfg(unix)]
mod rebase;
#[cfg(unix)]
mod reset;
#[cfg(unix)]
mod rpc;
#[cfg(unix)]
mod undo;
#[cfg(unix)]
mod untrusted;

#[cfg(unix)]
use crate::testing::write::{Window, WriteFixture};
#[cfg(unix)]
use crate::write::GitPromptAnswer;

/// 미저장 확인에 바로 「진행」 으로 답하는 프로젝트 창(작업 트리를 바꾸는 쓰기가 묻는다).
#[cfg(unix)]
fn window(f: &WriteFixture) -> Window {
    Window::open(&f.write, &f.project, |_| Some(GitPromptAnswer::Proceed))
}

/// HEAD 커밋의 제목.
#[cfg(unix)]
fn subject(f: &WriteFixture, rev: &str) -> String {
    f.repo
        .git(&["log", "-1", "--format=%s", rev])
        .trim()
        .to_owned()
}

/// 커밋의 부모들.
#[cfg(unix)]
fn parents(f: &WriteFixture, rev: &str) -> Vec<String> {
    f.repo
        .git(&["rev-list", "--parents", "-n1", rev])
        .split_whitespace()
        .skip(1)
        .map(str::to_owned)
        .collect()
}
