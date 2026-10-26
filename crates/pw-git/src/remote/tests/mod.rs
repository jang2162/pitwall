//! fetch · pull · push 시험(13-m2-plan.md M2-09 완료 기준의 원격 쪽). 원격은 시험 저장소 옆의 로컬 bare 저장소(`file://`,
//! `crate::testing::remote`)이고, 원격을 몰래 움직이는 것은 다른 클론이다. 진짜 git 을 PATH 에 둔 가짜 로그인 셸로 돈다(unix).

mod fetch;
mod pull;
mod push;

use crate::testing::remote::Remote;
use crate::testing::write::{Window, WriteFixture};
use crate::write::GitPromptAnswer;

/// 미저장 확인에 바로 「진행」 으로 답하는 프로젝트 창.
fn proceed(f: &WriteFixture) -> Window {
    Window::open(&f.write, &f.project, |_| Some(GitPromptAnswer::Proceed))
}

/// 원격(`origin`)이 있는 신뢰한 프로젝트.
fn with_remote() -> (WriteFixture, Remote) {
    let f = WriteFixture::new(true);
    let remote = f.repo.add_bare_remote("origin");
    (f, remote)
}
