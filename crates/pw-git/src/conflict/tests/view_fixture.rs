//! M2-19 의 창 시험 픽스처: merge 충돌 하나의 `git.conflicts` · `git.conflict.get` 과 rebase 충돌의 `git.operation` 을 JSON 으로
//! 남긴다(`apps/desktop/src/git/merge/fixtures/merge.json`). 창 시험(`apps/desktop/src/git/merge/merge-window.test.tsx`)이 이 JSON 을
//! 가짜 Host 의 답으로 쓰고 — 이 시험은 그 JSON 이 지금 pw-git 의 출력과 같은지 본다(다르면 실패). 다시 만들 때는
//! `PW_UPDATE_MERGE_FIXTURE=1` 로 이 시험을 돌린다.
//!
//! 「창에서 끝까지」(M2-19 완료 기준)의 Host 쪽도 여기서 본다: 창 시험이 머지 문서에서 만든 결과(충돌 하나는 `≫`, 하나는 직접 친
//! 글 — `expected`)를 같은 저장소에 `git.conflict.resolve` 로 쓰고 바이너리 충돌은 [내 것] 으로 고른 뒤 `git.operation.continue` 하면
//! 머지 커밋이 생기고 그 파일이 `expected` 바이트다. 창 시험은 저장 때 보낸 글이 `expected` 와 같은지 본다 — 둘을 이어 창에서 커밋까지다.
//! rebase 쪽은 [중단] 뒤 HEAD 가 시작 전으로 돌아오는 것을 본다(`history::tests::rebase` 와 같은 판정을 이 픽스처의 저장소로).
//!
//! 작업 트리 경로는 `/repo` 로, 파일 `version`(mtime 을 담는다)은 `fixture-version` 으로 바꿔 적는다. 커밋 해시는 `TestRepo` 의 시계가
//! 고정이라 늘 같다.

use std::fs;
use std::path::PathBuf;

use super::*;
use crate::conflict::methods::{
    ConflictChoice, ConflictContent, ConflictPick, GitConflictParams, GitConflictResolveParams,
    GitConflictsParams,
};
use crate::history::methods::{GitRebaseParams, rebase};
use crate::testing::write::{Window, WriteFixture};
use crate::write::{Askpass, GitPromptAnswer, OperationAction};

const FIXTURE: &str = "../../apps/desktop/src/git/merge/fixtures/merge.json";
const PATH: &str = "notes.txt";

/// 충돌 둘(「one」 · 「four」 줄을 두 쪽이 다르게 고쳤다) · 그쪽만 고친 줄(mid2) · 우리 쪽만 고친 줄(omega).
const BASE: &str =
    "alpha\none\ntwo\nthree\nmid1\nmid2\nmid3\nmid4\nmid5\nfour\nfive\nsix\nseven\nomega\n";
const OURS: &str = "alpha\none ours\ntwo\nthree\nmid1\nmid2\nmid3\nmid4\nmid5\nfour ours\nfive\nsix\nseven\nomega ours\n";
const THEIRS: &str = "alpha\none theirs\ntwo\nthree\nmid1\nmid2 theirs\nmid3\nmid4\nmid5\nfour theirs\nfive\nsix\nseven\nomega\n";
/// 창 시험이 둘째 충돌 자리에 직접 치는 글.
const TYPED: &str = "넷 — 손으로 고침\n";
/// 첫 충돌은 `≫`(ours), 둘째는 `TYPED`.
const EXPECTED: &str = "alpha\none ours\ntwo\nthree\nmid1\nmid2 theirs\nmid3\nmid4\nmid5\n넷 — 손으로 고침\nfive\nsix\nseven\nomega ours\n";

fn start_merge(r: &TestRepo) {
    r.write(PATH, BASE);
    fs::write(r.root().join("image.bin"), b"\0base").unwrap();
    r.commit_all("base");
    r.git(&["checkout", "--quiet", "-b", "theirs"]);
    r.write(PATH, THEIRS);
    fs::write(r.root().join("image.bin"), b"\0theirs").unwrap();
    r.commit_all("theirs");
    r.checkout("main");
    r.write(PATH, OURS);
    fs::write(r.root().join("image.bin"), b"\0ours").unwrap();
    r.commit_all("ours");
    let out = r.try_git_in(r.root(), &["merge", "theirs"]);
    assert!(!out.status.success(), "the merge should stop on conflicts");
}

fn relocate(value: &mut serde_json::Value, root: &str) {
    match value {
        serde_json::Value::String(s) if s == root => *s = "/repo".to_owned(),
        serde_json::Value::Array(items) => items.iter_mut().for_each(|v| relocate(v, root)),
        serde_json::Value::Object(map) => {
            for (key, v) in map.iter_mut() {
                // 파일 `version` 은 mtime 을 담아 실행마다 다르다 — 창은 받은 값을 그대로 돌려줄 뿐이라 자리 값으로 둔다.
                if key == "version" && v.is_string() {
                    *v = serde_json::Value::String("fixture-version".to_owned());
                } else {
                    relocate(v, root);
                }
            }
        }
        _ => {}
    }
}

/// 픽스처 값과 그 저장소에서 창이 할 일을 한 뒤의 판정.
fn fixture_value() -> serde_json::Value {
    let repo = TestRepo::with_commit();
    start_merge(&repo);
    block_on(async {
        let f = WriteFixture::with_repo(repo, true, Askpass::default());
        let _window = Window::open(&f.write, &f.project, |_| Some(GitPromptAnswer::Proceed));
        let root = f.repo.root().display().to_string();
        let git = f.write.git();
        let conflicts = git
            .conflicts(&GitConflictsParams {
                project_id: f.project.clone(),
                worktree: None,
            })
            .unwrap();
        let get = |path: &str| {
            git.conflict(&GitConflictParams {
                project_id: f.project.clone(),
                worktree: None,
                path: path.to_owned(),
            })
            .unwrap()
        };
        let detail = get(PATH);
        let image = get("image.bin");
        let mut value = serde_json::json!({
            "conflicts": conflicts,
            "detail": detail,
            "image": image,
            "typed": TYPED,
            "expected": EXPECTED,
        });
        relocate(&mut value, &root);

        // 창이 하는 대로: get 의 `version` 을 실어 쓴다.
        f.write
            .resolve(
                f.caller(),
                GitConflictResolveParams {
                    project_id: f.project.clone(),
                    path: PATH.to_owned(),
                    content: Some(ConflictContent {
                        text: EXPECTED.to_owned(),
                        bom: false,
                        expected_version: detail.version.clone(),
                    }),
                    ..GitConflictResolveParams::default()
                },
            )
            .await
            .unwrap();
        let left = f
            .write
            .resolve(
                f.caller(),
                GitConflictResolveParams {
                    project_id: f.project.clone(),
                    path: "image.bin".to_owned(),
                    side: Some(ConflictChoice {
                        pick: ConflictPick::Ours,
                        expected_version: get("image.bin").version,
                    }),
                    ..GitConflictResolveParams::default()
                },
            )
            .await
            .unwrap();
        assert!(left.files.is_empty());
        let done = f
            .write
            .act(f.caller(), &f.project, None, OperationAction::Continue)
            .await
            .unwrap();
        assert_eq!((done.operation, done.stopped), (None, false));
        let parents = f.repo.git(&["rev-list", "--parents", "-n", "1", "HEAD"]);
        assert_eq!(parents.split_whitespace().count(), 3, "a merge commit");
        assert_eq!(
            f.repo.git(&["show", &format!("HEAD:{PATH}")]),
            EXPECTED,
            "the committed file is the result the window saved"
        );
        assert_eq!(
            fs::read(f.repo.root().join(PATH)).unwrap(),
            EXPECTED.as_bytes()
        );
        value
    })
}

/// rebase 충돌의 `git.operation` 과 [중단] 뒤 HEAD.
fn rebase_value() -> serde_json::Value {
    let repo = TestRepo::with_commit();
    block_on(async {
        let f = WriteFixture::with_repo(repo, true, Askpass::default());
        let _window = Window::open(&f.write, &f.project, |_| Some(GitPromptAnswer::Proceed));
        let d = f.repo.diverge();
        let tip = f.repo.head();
        let stopped = rebase(
            f.write.clone(),
            f.caller(),
            GitRebaseParams {
                project_id: f.project.clone(),
                worktree: None,
                upstream: d.theirs.clone(),
                branch: None,
            },
        )
        .await
        .unwrap();
        let operation = stopped.operation.expect("the rebase stops on the conflict");
        let aborted = f
            .write
            .act(f.caller(), &f.project, None, OperationAction::Abort)
            .await
            .unwrap();
        assert_eq!(aborted.operation, None);
        assert_eq!(f.repo.head(), tip, "abort puts HEAD back");
        serde_json::json!({ "operation": operation, "abort": aborted })
    })
}

#[test]
fn merge_view_fixture_matches_pw_git_and_resolves_to_a_merge_commit() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(FIXTURE);
    let mut got = fixture_value();
    got["rebase"] = rebase_value();
    if std::env::var_os("PW_UPDATE_MERGE_FIXTURE").is_some() {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let text = serde_json::to_string_pretty(&got).unwrap();
        fs::write(&path, format!("{text}\n")).unwrap();
        return;
    }
    let want: serde_json::Value = fs::read_to_string(&path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();
    assert!(
        got == want,
        "{} 이 지금 pw-git 의 출력과 다르다 — PW_UPDATE_MERGE_FIXTURE=1 로 이 시험을 돌려 다시 만든다",
        path.display()
    );
}
