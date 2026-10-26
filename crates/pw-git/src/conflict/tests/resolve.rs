//! `git.conflict.resolve`: 해결한 경로는 목록에서 빠지고 index 에 단계 0 으로만 있다(지운 쪽이면 없다). 갈래마다 한쪽 고르기가
//! 되고(삭제/수정 충돌의 양쪽 방향 포함), 모든 충돌을 풀면 `git.operation.continue` 가 머지 커밋을 만든다. 내용 쓰기는 경로 규칙
//! A 를 지키고, 한쪽 고르기는 루트 밖으로 링크된 앞 폴더를 따라 쓰지 않는다. 미저장 확인은 그 경로 하나를 묻는다.

use std::fs;
use std::path::Path;

use pw_rpc::code;

use super::*;
use crate::conflict::methods::{ConflictPick, GitConflictResolveParams, GitConflicts};
use crate::testing::conflict::{BOTH_OURS, BOTH_THEIRS, SUB_OURS, SUB_THEIRS};
use crate::testing::write::{Window, WriteFixture};
use crate::write::{Askpass, GitPromptAnswer, GitPromptRequest, OperationAction, WriteError};

/// 신뢰한 프로젝트의 쓰기 실행기. 런타임 안에서 부른다.
fn fixture(repo: TestRepo) -> WriteFixture {
    WriteFixture::with_repo(repo, true, Askpass::default())
}

fn proceed(f: &WriteFixture) -> Window {
    Window::open(&f.write, &f.project, |_| Some(GitPromptAnswer::Proceed))
}

fn params(f: &WriteFixture, path: &str) -> GitConflictResolveParams {
    GitConflictResolveParams {
        project_id: f.project.clone(),
        path: path.to_owned(),
        ..GitConflictResolveParams::default()
    }
}

async fn pick(
    f: &WriteFixture,
    path: &str,
    side: ConflictPick,
) -> Result<GitConflicts, WriteError> {
    f.write
        .resolve(GitConflictResolveParams {
            side: Some(side),
            ..params(f, path)
        })
        .await
}

async fn content(f: &WriteFixture, path: &str, text: &str) -> Result<GitConflicts, WriteError> {
    f.write
        .resolve(GitConflictResolveParams {
            content: Some(text.to_owned()),
            ..params(f, path)
        })
        .await
}

/// index 의 그 경로 항목(단계, 모드, 해시).
fn index(f: &WriteFixture, path: &str) -> Vec<(String, String, String)> {
    f.repo
        .git(&["--literal-pathspecs", "ls-files", "-s", "-z", "--", path])
        .split('\0')
        .filter(|e| !e.is_empty())
        .map(|e| {
            let (meta, _) = e.split_once('\t').unwrap();
            let meta: Vec<&str> = meta.split(' ').collect();
            (meta[2].to_owned(), meta[0].to_owned(), meta[1].to_owned())
        })
        .collect()
}

fn hash(f: &WriteFixture, data: &[u8]) -> String {
    let file = f.repo.base().join("hash-me");
    fs::write(&file, data).unwrap();
    f.repo
        .git(&["hash-object", "--", file.to_str().unwrap()])
        .trim()
        .to_owned()
}

fn paths(list: &GitConflicts) -> Vec<&str> {
    list.files.iter().map(|f| f.path.as_str()).collect()
}

#[test]
fn content_is_written_staged_and_leaves_the_list() {
    let repo = TestRepo::with_commit();
    repo.start_conflict_kinds();
    isolate(repo.base());
    block_on(async {
        let f = fixture(repo);
        let window = proceed(&f);
        let left = content(&f, "both.txt", "resolved\n").await.unwrap();
        assert!(!paths(&left).contains(&"both.txt"));
        assert!(paths(&left).contains(&"added.txt"), "the others stay");
        assert_eq!(
            fs::read(f.repo.root().join("both.txt")).unwrap(),
            b"resolved\n"
        );
        assert_eq!(
            index(&f, "both.txt"),
            [("0".to_owned(), "100644".to_owned(), hash(&f, b"resolved\n"))]
        );
        assert_eq!(
            window.seen(),
            [GitPromptRequest::Unsaved {
                work_tree: f.repo.root().display().to_string(),
                paths: Some(vec!["both.txt".to_owned()]),
            }],
            "the unsaved check asks about that path only"
        );

        // BOM 은 `bom` 으로 붙인다.
        f.write
            .resolve(GitConflictResolveParams {
                content: Some("with bom\n".to_owned()),
                bom: true,
                ..params(&f, "added.txt")
            })
            .await
            .unwrap();
        assert_eq!(
            fs::read(f.repo.root().join("added.txt")).unwrap(),
            "\u{feff}with bom\n".as_bytes()
        );
    });
}

/// 고른 뒤의 기대: 그쪽 내용이 단계 0 으로 남거나(`Keep`) 경로가 사라진다(`Gone`).
#[derive(Debug, Clone, Copy)]
enum After {
    Keep(&'static [u8]),
    Link(&'static str),
    Sub(&'static str),
    Gone,
}

/// 갈래마다 한쪽을 고르고(두 벌 — 삭제/수정 충돌은 양쪽 방향) 모든 충돌을 푼 뒤 계속하면 머지 커밋이 그 내용으로 생긴다.
#[test]
fn picking_a_side_resolves_every_kind_and_continue_commits_the_merge() {
    use After::{Gone, Keep, Link, Sub};
    use ConflictPick::{Ours, Theirs};
    let sets: [&[(&str, ConflictPick, After)]; 2] = [
        &[
            ("added.txt", Theirs, Keep(b"added by theirs\n")),
            ("both.txt", Ours, Keep(BOTH_OURS.as_bytes())),
            ("deleted-by-them.txt", Ours, Keep(b"keep ours\n")),
            ("deleted-by-us.txt", Ours, Gone),
            ("image.bin", Theirs, Keep(b"\0theirs")),
            ("link", Theirs, Link("target-theirs")),
            ("old.txt", Ours, Gone),
            ("renamed-ours.txt", Ours, Keep(b"renamed on both sides\n")),
            (
                "renamed-theirs.txt",
                Theirs,
                Keep(b"renamed on both sides\n"),
            ),
            ("sub", Theirs, Sub(SUB_THEIRS)),
        ],
        &[
            ("added.txt", Ours, Keep(b"added by ours\n")),
            ("both.txt", Theirs, Keep(BOTH_THEIRS.as_bytes())),
            ("deleted-by-them.txt", Theirs, Gone),
            ("deleted-by-us.txt", Theirs, Keep(b"keep theirs\n")),
            ("image.bin", Ours, Keep(b"\0ours")),
            ("link", Ours, Link("target-ours")),
            ("old.txt", Theirs, Gone),
            ("renamed-ours.txt", Theirs, Gone),
            ("renamed-theirs.txt", Ours, Gone),
            ("sub", Ours, Sub(SUB_OURS)),
        ],
    ];
    for set in sets {
        let repo = TestRepo::with_commit();
        repo.start_conflict_kinds();
        isolate(repo.base());
        block_on(async {
            let f = fixture(repo);
            let _window = proceed(&f);
            for &(path, side, after) in set {
                let left = pick(&f, path, side)
                    .await
                    .unwrap_or_else(|e| panic!("{path}: {e}"));
                assert!(!paths(&left).contains(&path), "{path} left the list");
                let file = f.repo.root().join(path);
                match after {
                    Keep(bytes) => {
                        assert_eq!(fs::read(&file).unwrap(), bytes, "{path}");
                        assert_eq!(
                            index(&f, path),
                            [("0".to_owned(), "100644".to_owned(), hash(&f, bytes))],
                            "{path}"
                        );
                    }
                    Link(target) => {
                        assert_eq!(fs::read_link(&file).unwrap(), Path::new(target));
                        assert_eq!(index(&f, path)[0].1, "120000");
                    }
                    Sub(commit) => assert_eq!(
                        index(&f, path),
                        [("0".to_owned(), "160000".to_owned(), commit.to_owned())]
                    ),
                    Gone => {
                        assert!(fs::symlink_metadata(&file).is_err(), "{path} is removed");
                        assert_eq!(index(&f, path), [], "{path} left the index");
                    }
                }
            }
            assert!(
                f.write
                    .git()
                    .conflicts(&crate::conflict::methods::GitConflictsParams {
                        project_id: f.project.clone(),
                        worktree: None,
                    })
                    .unwrap()
                    .files
                    .is_empty()
            );

            let done = f
                .write
                .act(&f.project, None, OperationAction::Continue)
                .await
                .unwrap();
            assert_eq!((done.operation, done.stopped), (None, false));
            let parents = f.repo.git(&["rev-list", "--parents", "-n", "1", "HEAD"]);
            assert_eq!(parents.split_whitespace().count(), 3, "a merge commit");
            let tree = f.repo.git(&["ls-tree", "-r", "HEAD"]);
            for &(path, _, after) in set {
                let entry = tree.lines().find(|l| l.ends_with(&format!("\t{path}")));
                match after {
                    Keep(bytes) => assert!(entry.unwrap().contains(&hash(&f, bytes)), "{path}"),
                    Link(target) => assert!(entry.unwrap().contains(&hash(&f, target.as_bytes()))),
                    Sub(commit) => assert!(entry.unwrap().contains(commit)),
                    Gone => assert_eq!(entry, None, "{path}"),
                }
            }
        });
    }
}

#[test]
fn requests_that_do_not_fit_are_refused_before_asking_the_window() {
    let repo = TestRepo::with_commit();
    repo.start_conflict_kinds();
    isolate(repo.base());
    block_on(async {
        let f = fixture(repo);
        let window = proceed(&f);
        let invalid = |r: Result<GitConflicts, WriteError>| match r {
            Err(WriteError::Rpc(e)) => e.code,
            other => panic!("expected an RPC error, got {other:?}"),
        };
        let both = GitConflictResolveParams {
            content: Some("x".into()),
            side: Some(ConflictPick::Ours),
            ..params(&f, "both.txt")
        };
        assert_eq!(invalid(f.write.resolve(both).await), code::INVALID_PARAMS);
        assert_eq!(
            invalid(f.write.resolve(params(&f, "both.txt")).await),
            code::INVALID_PARAMS,
            "neither content nor side"
        );
        let bom_side = GitConflictResolveParams {
            bom: true,
            side: Some(ConflictPick::Ours),
            ..params(&f, "both.txt")
        };
        assert_eq!(
            invalid(f.write.resolve(bom_side).await),
            code::INVALID_PARAMS
        );
        assert_eq!(
            invalid(content(&f, "../x", "x").await),
            code::INVALID_PARAMS
        );
        assert_eq!(
            invalid(content(&f, "README.md", "x").await),
            code::NOT_FOUND
        );
        assert_eq!(invalid(content(&f, "sub", "x").await), code::INVALID_PARAMS);
        assert_eq!(
            invalid(content(&f, "link", "x").await),
            code::INVALID_PARAMS
        );
        assert_eq!(window.seen(), [], "nothing reached the window");
        assert_eq!(
            fs::read(f.repo.root().join("README.md")).unwrap(),
            b"hello\n"
        );
    });
}

#[test]
fn a_cancelled_unsaved_check_leaves_the_conflict() {
    let repo = TestRepo::with_commit();
    repo.start_conflict_kinds();
    isolate(repo.base());
    block_on(async {
        let f = fixture(repo);
        let before = fs::read(f.repo.root().join("both.txt")).unwrap();
        let _window = Window::open(&f.write, &f.project, |_| Some(GitPromptAnswer::Cancel));
        assert_eq!(
            pick(&f, "both.txt", ConflictPick::Theirs).await,
            Err(WriteError::Cancelled)
        );
        assert_eq!(fs::read(f.repo.root().join("both.txt")).unwrap(), before);
        assert_eq!(index(&f, "both.txt").len(), 3, "still three stages");
    });
}

/// 앞 폴더를 루트 밖 폴더로의 링크로 바꾼 충돌 경로: 내용 쓰기는 거절하고, 한쪽 고르기(git checkout)도 링크를 따라 밖에 쓰지 않는다.
#[test]
fn a_leading_folder_linked_outside_is_not_written_through() {
    let repo = TestRepo::with_commit();
    repo.stage_conflict("dir/file.txt", Some(b"a\n"), Some(b"o\n"), Some(b"t\n"));
    let outside = repo.base().join("outside");
    fs::create_dir_all(&outside).unwrap();
    fs::remove_dir_all(repo.root().join("dir")).unwrap();
    std::os::unix::fs::symlink(&outside, repo.root().join("dir")).unwrap();
    isolate(repo.base());
    block_on(async {
        let f = fixture(repo);
        let _window = proceed(&f);
        let refused = content(&f, "dir/file.txt", "resolved\n").await;
        assert!(
            matches!(&refused, Err(WriteError::Rpc(e)) if e.code == code::INVALID_PARAMS),
            "{refused:?}"
        );
        let picked = pick(&f, "dir/file.txt", ConflictPick::Theirs).await;
        assert_eq!(
            fs::read_dir(&outside).unwrap().count(),
            0,
            "nothing was written outside the work tree (pick: {picked:?})"
        );
    });
}

/// 그 항목이 심링크면 대상으로 판정한다: 루트 밖을 가리키면 거절하고, 안을 가리키면 링크 자체를 일반 파일로 바꾼다(대상은 그대로).
#[test]
fn a_symlinked_item_is_judged_by_its_target_and_replaced_itself() {
    let repo = TestRepo::with_commit();
    repo.stage_conflict("x.txt", Some(b"a\n"), Some(b"o\n"), Some(b"t\n"));
    let secret = repo.base().join("secret.txt");
    fs::write(&secret, "secret\n").unwrap();
    repo.write("other.txt", "other\n");
    let item = repo.root().join("x.txt");
    fs::remove_file(&item).unwrap();
    std::os::unix::fs::symlink(&secret, &item).unwrap();
    isolate(repo.base());
    block_on(async {
        let f = fixture(repo);
        let _window = proceed(&f);
        let refused = content(&f, "x.txt", "resolved\n").await;
        assert!(
            matches!(&refused, Err(WriteError::Rpc(e)) if e.code == code::INVALID_PARAMS),
            "{refused:?}"
        );
        assert_eq!(fs::read_to_string(&secret).unwrap(), "secret\n");

        fs::remove_file(&item).unwrap();
        std::os::unix::fs::symlink("other.txt", &item).unwrap();
        content(&f, "x.txt", "resolved\n").await.unwrap();
        assert!(
            !fs::symlink_metadata(&item)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read_to_string(&item).unwrap(), "resolved\n");
        assert_eq!(
            fs::read_to_string(f.repo.root().join("other.txt")).unwrap(),
            "other\n"
        );
        assert_eq!(index(&f, "x.txt")[0].1, "100644");
    });
}

/// 경로는 글자 그대로다 — pathspec 글롭 `[x]` 가 든 이름을 풀어도 같은 폴더의 다른 충돌은 그대로다.
#[test]
fn paths_with_pathspec_characters_resolve_only_themselves() {
    let repo = TestRepo::with_commit();
    repo.stage_conflict("dir/한글 [x].txt", Some(b"a\n"), Some(b"o\n"), Some(b"t\n"));
    repo.stage_conflict("dir/한글 x.txt", Some(b"a\n"), Some(b"o\n"), Some(b"t\n"));
    isolate(repo.base());
    block_on(async {
        let f = fixture(repo);
        let _window = proceed(&f);
        let left = pick(&f, "dir/한글 [x].txt", ConflictPick::Theirs)
            .await
            .unwrap();
        assert_eq!(paths(&left), ["dir/한글 x.txt"]);
        assert_eq!(
            fs::read(f.repo.root().join("dir/한글 [x].txt")).unwrap(),
            b"t\n"
        );
        let left = pick(&f, "dir/한글 x.txt", ConflictPick::Ours)
            .await
            .unwrap();
        assert!(left.files.is_empty());
    });
}
