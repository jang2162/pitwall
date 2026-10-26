//! 충돌 갈래 픽스처(M2-11): 한 merge 로 index 단계의 모든 갈래(양쪽 수정 · 양쪽 추가 · 우리/그쪽 삭제 · 우리/그쪽만 추가 · 양쪽
//! 삭제)와 바이너리 · 서브모듈 · 심링크 충돌을 만든다.

use super::repo::TestRepo;

/// 텍스트 양쪽 수정 충돌의 세 쪽: 2 번째 줄은 충돌, 6 번째 줄은 그쪽만, 9 번째 줄은 우리 쪽만 바꿨다.
pub const BOTH_BASE: &str = "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n";
pub const BOTH_OURS: &str = "1\n2 ours\n3\n4\n5\n6\n7\n8\n9 ours\n10\n";
pub const BOTH_THEIRS: &str = "1\n2 theirs\n3\n4\n5\n6 theirs\n7\n8\n9\n10\n";

/// 서브모듈 포인터(저장소에 없는 커밋 — git 은 「체크아웃되지 않은 서브모듈」 충돌로 둔다).
pub const SUB_BASE: &str = "1111111111111111111111111111111111111111";
pub const SUB_OURS: &str = "3333333333333333333333333333333333333333";
pub const SUB_THEIRS: &str = "2222222222222222222222222222222222222222";

/// [`TestRepo::start_conflict_kinds`] 가 만든 충돌 경로와 그 `git status --porcelain` 두 글자.
pub const KINDS: &[(&str, &str)] = &[
    ("added.txt", "AA"),
    ("both.txt", "UU"),
    ("deleted-by-them.txt", "UD"),
    ("deleted-by-us.txt", "DU"),
    ("image.bin", "UU"),
    #[cfg(unix)]
    ("link", "UU"),
    ("old.txt", "DD"),
    ("renamed-ours.txt", "AU"),
    ("renamed-theirs.txt", "UA"),
    ("sub", "UU"),
];

impl TestRepo {
    /// 첫 커밋이 있는 저장소에서 갈래마다 충돌하는 두 브랜치를 만들고 `theirs` 를 `main` 에 merge 해 멈춘다. 경로는 [`KINDS`].
    /// `git add --all` 을 쓰지 않는다 — 체크아웃되지 않은 서브모듈 자리를 지운 것으로 담는다.
    pub fn start_conflict_kinds(&self) {
        self.write("both.txt", BOTH_BASE);
        self.write("deleted-by-us.txt", "keep\n");
        self.write("deleted-by-them.txt", "keep\n");
        self.write("image.bin", "\0base");
        self.write("old.txt", "renamed on both sides\n");
        self.symlink("link", "target-base");
        self.git(&["add", "--", "."]);
        self.cacheinfo(SUB_BASE);
        self.commit("base");

        self.git(&["checkout", "--quiet", "-b", "theirs"]);
        self.write("both.txt", BOTH_THEIRS);
        self.write("added.txt", "added by theirs\n");
        self.write("deleted-by-us.txt", "keep theirs\n");
        self.git(&["rm", "--quiet", "--", "deleted-by-them.txt"]);
        self.write("image.bin", "\0theirs");
        self.git(&["mv", "old.txt", "renamed-theirs.txt"]);
        self.symlink("link", "target-theirs");
        self.git(&["add", "--", "."]);
        self.cacheinfo(SUB_THEIRS);
        self.commit("theirs");

        self.checkout("main");
        self.write("both.txt", BOTH_OURS);
        self.write("added.txt", "added by ours\n");
        self.git(&["rm", "--quiet", "--", "deleted-by-us.txt"]);
        self.write("deleted-by-them.txt", "keep ours\n");
        self.write("image.bin", "\0ours");
        self.git(&["mv", "old.txt", "renamed-ours.txt"]);
        self.symlink("link", "target-ours");
        self.git(&["add", "--", "."]);
        self.cacheinfo(SUB_OURS);
        self.commit("ours");

        let out = self.try_git_in(self.root(), &["merge", "theirs"]);
        assert!(
            !out.status.success(),
            "the kinds merge should stop on conflicts: {}",
            String::from_utf8_lossy(&out.stdout)
        );
    }

    /// merge 없이 `path` 를 충돌 상태로 둔다: 그 경로의 index 항목을 지우고 있는 쪽(base · ours · theirs)마다 단계 1 · 2 · 3 의 일반
    /// 파일(`100644`)을 심는다. 작업 트리에는 ours(없으면 theirs)를 쓴다. 대용량 · BOM · 경로 시험처럼 merge 가 만들기 번거로운 세
    /// 쪽을 그대로 둔다.
    pub fn stage_conflict(
        &self,
        path: &str,
        base: Option<&[u8]>,
        ours: Option<&[u8]>,
        theirs: Option<&[u8]>,
    ) {
        let mut info = String::new();
        for (stage, data) in [(1, base), (2, ours), (3, theirs)] {
            let Some(data) = data else { continue };
            let blob = self.base().join(format!("stage-{stage}.blob"));
            std::fs::write(&blob, data).unwrap();
            let id = self.git(&[
                "hash-object",
                "-w",
                "--",
                blob.to_str().expect("utf-8 temp path"),
            ]);
            info.push_str(&format!("100644 {} {stage}\t{path}\n", id.trim()));
        }
        self.git(&["update-index", "--force-remove", "--", path]);
        let list = self.base().join("index-info");
        std::fs::write(&list, info).unwrap();
        let out = self
            .command_in(self.root())
            .args(["update-index", "--index-info"])
            .stdin(std::fs::File::open(&list).unwrap())
            .output()
            .expect("run git update-index");
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        if let Some(data) = ours.or(theirs) {
            let file = self.root().join(path);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, data).unwrap();
        }
    }

    /// 지금 index 의 `sub` 를 서브모듈 포인터 `commit` 으로.
    fn cacheinfo(&self, commit: &str) {
        self.git(&[
            "update-index",
            "--add",
            "--cacheinfo",
            &format!("160000,{commit},sub"),
        ]);
    }

    fn commit(&self, message: &str) {
        self.git(&["commit", "--quiet", "-m", message]);
    }

    /// 작업 트리의 `rel` 을 `target` 을 가리키는 심링크로(unix). 다른 OS 에서는 아무것도 하지 않는다.
    pub fn symlink(&self, rel: &str, target: &str) {
        #[cfg(unix)]
        {
            let path = self.root().join(rel);
            let _ = std::fs::remove_file(&path);
            std::os::unix::fs::symlink(target, &path).unwrap();
        }
        #[cfg(not(unix))]
        let _ = (rel, target);
    }
}
