//! 진행 중 작업(충돌로 멈춘 merge · cherry-pick · rebase · revert, bisect)을 만든다.

use super::repo::TestRepo;

/// [`TestRepo::diverge`] 의 결과: 같은 파일을 서로 다르게 고친 두 브랜치.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diverged {
    /// 충돌하는 파일(작업 트리 기준 상대 경로).
    pub path: String,
    /// 지금 체크아웃한 브랜치(`main`).
    pub ours: String,
    /// 갈라진 브랜치.
    pub theirs: String,
    /// `theirs` 끝 커밋.
    pub theirs_commit: String,
}

impl TestRepo {
    /// `conflict.txt` 를 공통 조상에서 만들고, `theirs` 와 `main` 에서 각각 다르게 고친다. 끝나면 `main` 에 있다. 커밋이 하나는
    /// 있어야 한다([`TestRepo::with_commit`]).
    pub fn diverge(&self) -> Diverged {
        let path = "conflict.txt";
        self.commit_file(path, "base\n", "base");
        self.git(&["checkout", "--quiet", "-b", "theirs"]);
        let theirs_commit = self.commit_file(path, "theirs\n", "theirs");
        self.checkout("main");
        self.commit_file(path, "ours\n", "ours");
        Diverged {
            path: path.to_owned(),
            ours: "main".to_owned(),
            theirs: "theirs".to_owned(),
            theirs_commit,
        }
    }

    /// `theirs` 를 `main` 에 merge 해 충돌로 멈춘다(`MERGE_HEAD` 가 있다).
    pub fn start_conflicting_merge(&self) -> Diverged {
        let d = self.diverge();
        self.expect_conflict(&["merge", &d.theirs]);
        d
    }

    /// `theirs` 의 끝 커밋을 cherry-pick 해 충돌로 멈춘다(`CHERRY_PICK_HEAD`).
    pub fn start_conflicting_cherry_pick(&self) -> Diverged {
        let d = self.diverge();
        self.expect_conflict(&["cherry-pick", &d.theirs_commit]);
        d
    }

    /// `main` 을 `theirs` 위로 rebase 해 충돌로 멈춘다(`rebase-merge/`).
    pub fn start_conflicting_rebase(&self) -> Diverged {
        let d = self.diverge();
        self.expect_conflict(&["rebase", &d.theirs]);
        d
    }

    /// `ours` 커밋(`main` 의 끝)의 부모 쪽 변경을 되돌리려다 충돌로 멈춘다(`REVERT_HEAD`). `base` 커밋을 되돌리면 그 뒤의 `ours`
    /// 가 같은 줄을 바꿨으므로 충돌한다.
    pub fn start_conflicting_revert(&self) -> Diverged {
        let d = self.diverge();
        self.expect_conflict(&["revert", "--no-edit", "HEAD~1"]);
        d
    }

    /// bisect 를 시작한다(`BISECT_LOG`). 좋은 쪽은 첫 커밋, 나쁜 쪽은 HEAD.
    pub fn start_bisect(&self) {
        let good = self.git(&["rev-list", "--max-parents=0", "HEAD"]);
        self.git(&["bisect", "start", "HEAD", good.trim()]);
    }

    fn expect_conflict(&self, args: &[&str]) {
        let out = self.try_git_in(self.root(), args);
        assert!(
            !out.status.success(),
            "git {args:?} should stop on a conflict: {}",
            String::from_utf8_lossy(&out.stdout)
        );
    }
}
