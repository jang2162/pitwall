//! 시험 저장소 도우미. 이 크레이트의 시험과, `test-support` 기능을 켠 다른 크레이트의 시험이 쓴다.
//!
//! 저장소는 언제나 임시 폴더에 git CLI 로 만든다 — 사람의 저장소 · 설정을 읽거나 바꾸지 않는다([`repo::git_command`] 의 격리 환경).
//! 픽스처 함수를 더할 때는 파일을 나눈다(`docs/design/13-m2-plan.md` 「공유 부품의 주인」): 저장소 만들기 · 커밋 · 브랜치 ·
//! 워크트리는 [`repo`], 진행 중 작업(충돌 상태)은 [`state`], 가짜 `git` 실행 파일은 [`fake`].

#[cfg(unix)]
pub mod fake;
pub mod repo;
pub mod state;

#[cfg(unix)]
pub use fake::FakeGit;
pub use repo::TestRepo;
