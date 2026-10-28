//! 스테이징 · 되돌리기 · stash 시험(13-m2-plan.md M2-07 완료 기준). 진짜 git · 임시 폴더의 시험 저장소 · 진짜 git 을 PATH 에 둔
//! 가짜 로그인 셸(`crate::testing::write`)로 돈다(unix). 패치 만들기는 git 없이 [`patch`] 가 본다.

mod patch;

#[cfg(unix)]
mod files;
#[cfg(unix)]
mod gutter;
#[cfg(unix)]
mod rpc;
#[cfg(unix)]
mod select;
#[cfg(unix)]
mod stash;
#[cfg(unix)]
mod untrusted;

#[cfg(unix)]
use crate::diff::methods::{DiffFile, DiffHunk, DiffLine, DiffSide, GitDiffParams};
#[cfg(unix)]
use crate::stage::methods::HunkSelection;
#[cfg(unix)]
use crate::testing::write::{Window, WriteFixture};
#[cfg(unix)]
use crate::write::GitPromptAnswer;

/// 저장소 꼭대기에서 git 을 돌려 stdout 바이트를 돌려준다(실패하면 패닉).
#[cfg(unix)]
fn bytes(f: &WriteFixture, args: &[&str]) -> Vec<u8> {
    let out = f.repo.try_git_in(f.repo.root(), args);
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    out.stdout
}

/// index 의 모든 항목(모드 · 해시 · 단계 · 경로).
#[cfg(unix)]
fn index(f: &WriteFixture) -> String {
    f.repo.git(&["ls-files", "-s"])
}

/// `git.diff` 의 파일 하나(공백을 보는 diff).
#[cfg(unix)]
fn diff(f: &WriteFixture, to: DiffSide, path: &str) -> DiffFile {
    let mut files = f
        .write
        .git()
        .diff(&GitDiffParams {
            project_id: f.project.clone(),
            worktree: None,
            from: None,
            to,
            paths: Some(vec![path.to_owned()]),
            ignore_whitespace: false,
        })
        .unwrap()
        .files;
    assert_eq!(files.len(), 1, "{files:?}");
    files.remove(0)
}

/// hunk 에서 `pick` 이 고른 줄.
#[cfg(unix)]
fn select(hunk: &DiffHunk, pick: impl Fn(&DiffLine) -> bool) -> HunkSelection {
    let lines: Vec<u32> = (0..hunk.lines.len() as u32)
        .filter(|&i| pick(&hunk.lines[i as usize]))
        .collect();
    assert!(!lines.is_empty(), "nothing picked in {hunk:?}");
    HunkSelection {
        hunk: hunk.clone(),
        lines: Some(lines),
    }
}

/// 미저장 확인에 늘 진행으로 답하는 창.
#[cfg(unix)]
pub(super) fn proceed(f: &WriteFixture) -> Window {
    Window::open(&f.write, &f.project, |_| Some(GitPromptAnswer::Proceed))
}

/// 시험 프로세스 안의 gix 는 사람의 전역 · 시스템 git 설정을 읽지 않는다(`testing::repo::isolate_git_config` — `TestRepo::new` 가
/// 부른다): 연 저장소의 설정에 저장소 밖 파일의 절이 없다.
#[test]
fn the_tests_gix_reads_no_git_config_of_the_person() {
    let repo = crate::testing::TestRepo::with_commit();
    let opened = gix::open(repo.root()).unwrap();
    let snapshot = opened.config_snapshot();
    let outside: Vec<String> = snapshot
        .plumbing()
        .sections()
        .filter(|s| {
            matches!(
                s.meta().source.kind(),
                gix::config::source::Kind::Global | gix::config::source::Kind::System
            )
        })
        .map(|s| format!("{:?} {}", s.meta().source, s.header().name()))
        .collect();
    assert_eq!(outside, Vec::<String>::new());
}
