//! `git.conflict.resolve`: 해결한 경로는 목록에서 빠지고 index 에 단계 0 으로만 있다(지운 쪽이면 없다). 갈래마다 한쪽 고르기가
//! 되고(삭제/수정 충돌의 양쪽 방향 포함), 모든 충돌을 풀면 `git.operation.continue` 가 머지 커밋을 만든다. 내용 쓰기는 경로 규칙
//! A 를 지키고(앞 폴더가 링크면 안을 가리켜도 거절), 한쪽 고르기는 루트 밖으로 링크된 앞 폴더를 따라 쓰지 않는다. 내용 쓰기는
//! `git.conflict.get` 이 준 `version` 이 지금 파일과 같을 때만 한다. 미저장 확인은 그 경로 하나를 묻는다.

use std::fs;
use std::path::Path;

use pw_rpc::code;

use super::*;
use crate::conflict::methods::{
    ConflictChoice, ConflictContent, ConflictPick, GitConflictParams, GitConflictResolveParams,
    GitConflicts, GitConflictsParams,
};
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

/// get 이 준 `version` 으로 한쪽 고르기(창이 하는 대로).
async fn pick(
    f: &WriteFixture,
    path: &str,
    side: ConflictPick,
) -> Result<GitConflicts, WriteError> {
    let expected_version = version(f, path);
    pick_at(f, path, side, expected_version).await
}

async fn pick_at(
    f: &WriteFixture,
    path: &str,
    pick: ConflictPick,
    expected_version: Option<String>,
) -> Result<GitConflicts, WriteError> {
    f.write
        .resolve(
            f.caller(),
            GitConflictResolveParams {
                side: Some(ConflictChoice {
                    pick,
                    expected_version,
                }),
                ..params(f, path)
            },
        )
        .await
}

/// `git.conflict.get` 의 `version`(창이 하는 대로). get 이 실패하는 경로면 `None`.
fn version(f: &WriteFixture, path: &str) -> Option<String> {
    f.write
        .git()
        .conflict(&GitConflictParams {
            project_id: f.project.clone(),
            worktree: None,
            path: path.to_owned(),
        })
        .ok()
        .and_then(|d| d.version)
}

/// get 이 준 `version` 으로 내용 해결(창이 하는 대로).
async fn content(f: &WriteFixture, path: &str, text: &str) -> Result<GitConflicts, WriteError> {
    let expected_version = version(f, path);
    content_at(f, path, text, expected_version).await
}

async fn content_at(
    f: &WriteFixture,
    path: &str,
    text: &str,
    expected_version: Option<String>,
) -> Result<GitConflicts, WriteError> {
    f.write
        .resolve(
            f.caller(),
            GitConflictResolveParams {
                content: Some(ConflictContent {
                    text: text.to_owned(),
                    bom: false,
                    expected_version,
                }),
                ..params(f, path)
            },
        )
        .await
}

fn conflicts(f: &WriteFixture) -> GitConflicts {
    f.write
        .git()
        .conflicts(&GitConflictsParams {
            project_id: f.project.clone(),
            worktree: None,
        })
        .unwrap()
}

fn rpc_code(r: &Result<GitConflicts, WriteError>) -> i32 {
    match r {
        Err(WriteError::Rpc(e)) => e.code,
        other => panic!("expected an RPC error, got {other:?}"),
    }
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
            .resolve(
                f.caller(),
                GitConflictResolveParams {
                    content: Some(ConflictContent {
                        text: "with bom\n".to_owned(),
                        bom: true,
                        expected_version: version(&f, "added.txt"),
                    }),
                    ..params(&f, "added.txt")
                },
            )
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
            assert!(conflicts(&f).files.is_empty());

            let done = f
                .write
                .act(f.caller(), &f.project, None, OperationAction::Continue)
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
    block_on(async {
        let f = fixture(repo);
        let window = proceed(&f);
        let invalid = |r: Result<GitConflicts, WriteError>| match r {
            Err(WriteError::Rpc(e)) => e.code,
            other => panic!("expected an RPC error, got {other:?}"),
        };
        let both = GitConflictResolveParams {
            content: Some(ConflictContent {
                text: "x".into(),
                bom: false,
                expected_version: version(&f, "both.txt"),
            }),
            side: Some(ConflictChoice {
                pick: ConflictPick::Ours,
                expected_version: version(&f, "both.txt"),
            }),
            ..params(&f, "both.txt")
        };
        assert_eq!(
            invalid(f.write.resolve(f.caller(), both).await),
            code::INVALID_PARAMS
        );
        assert_eq!(
            invalid(f.write.resolve(f.caller(), params(&f, "both.txt")).await),
            code::INVALID_PARAMS,
            "neither content nor side"
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

/// `expectedVersion` 은 빠뜨릴 수 없다 — 없던 파일이면 `null` 을 적어 보낸다.
#[test]
fn the_expected_version_cannot_be_left_out() {
    let parse = |content: serde_json::Value| {
        serde_json::from_value::<GitConflictResolveParams>(serde_json::json!({
            "projectId": "p",
            "path": "a.txt",
            "content": content,
        }))
    };
    let err = parse(serde_json::json!({ "text": "x" })).unwrap_err();
    assert!(err.to_string().contains("expectedVersion"), "{err}");
    let none = parse(serde_json::json!({ "text": "x", "expectedVersion": null })).unwrap();
    assert_eq!(none.content.unwrap().expected_version, None);
    let some = parse(serde_json::json!({ "text": "x", "expectedVersion": "v" })).unwrap();
    assert_eq!(some.content.unwrap().expected_version.as_deref(), Some("v"));

    let side = |side: serde_json::Value| {
        serde_json::from_value::<GitConflictResolveParams>(serde_json::json!({
            "projectId": "p",
            "path": "a.txt",
            "side": side,
        }))
    };
    let err = side(serde_json::json!({ "pick": "ours" })).unwrap_err();
    assert!(err.to_string().contains("expectedVersion"), "{err}");
    let none = side(serde_json::json!({ "pick": "theirs", "expectedVersion": null })).unwrap();
    assert_eq!(
        none.side,
        Some(ConflictChoice {
            pick: ConflictPick::Theirs,
            expected_version: None,
        })
    );
}

/// get 뒤에 작업 트리 파일이 바뀌었으면(터미널에서 고침) 내용 해결은 `CONFLICT` 이고 그 편집과 충돌이 그대로다. 없던 파일을
/// 기대(`null`)했는데 있어도 같다. 지금 `version` 으로 다시 보내면 풀린다.
#[test]
fn content_over_a_file_changed_since_get_is_a_conflict() {
    let repo = TestRepo::with_commit();
    repo.stage_conflict("x.txt", Some(b"a\n"), Some(b"o\n"), Some(b"t\n"));
    block_on(async {
        let f = fixture(repo);
        let window = proceed(&f);
        let file = f.repo.root().join("x.txt");
        let seen = version(&f, "x.txt");
        assert!(seen.is_some(), "the work tree file has a version");
        fs::write(&file, "edited by hand\n").unwrap();

        for expected in [seen, None] {
            let refused = content_at(&f, "x.txt", "resolved\n", expected.clone()).await;
            assert_eq!(rpc_code(&refused), code::CONFLICT, "{expected:?}");
            let Err(WriteError::Rpc(e)) = &refused else {
                unreachable!()
            };
            let current = e.data.as_ref().unwrap()["current"]
                .as_str()
                .map(str::to_owned);
            assert_eq!(
                current,
                version(&f, "x.txt"),
                "the error carries the current version"
            );
        }
        assert_eq!(fs::read_to_string(&file).unwrap(), "edited by hand\n");
        assert_eq!(index(&f, "x.txt").len(), 3, "still three stages");
        assert_eq!(window.seen(), [], "refused before asking the window");

        content(&f, "x.txt", "resolved\n").await.unwrap();
        assert_eq!(fs::read_to_string(&file).unwrap(), "resolved\n");

        // 지워진 파일: get 은 `null` 을 주고, 그것으로 풀면 파일을 만든다.
        f.repo
            .stage_conflict("gone.txt", Some(b"a\n"), Some(b"o\n"), Some(b"t\n"));
        fs::remove_file(f.repo.root().join("gone.txt")).unwrap();
        assert_eq!(version(&f, "gone.txt"), None);
        content(&f, "gone.txt", "made\n").await.unwrap();
        assert_eq!(
            fs::read_to_string(f.repo.root().join("gone.txt")).unwrap(),
            "made\n"
        );
    });
}

/// 앞 폴더가 작업 트리 **안** 의 다른 폴더를 가리키는 링크여도 내용 쓰기는 거절한다 — 따라가면 다른 추적 파일을 덮는다(git 의
/// 「beyond a symbolic link」). 거절은 창에 묻기 전이다.
#[test]
fn a_leading_folder_linked_inside_is_not_written_through() {
    let repo = TestRepo::with_commit();
    repo.write("other/f.txt", "OTHER ORIGINAL\n");
    repo.git(&["add", "--", "other/f.txt"]);
    repo.git(&["commit", "--quiet", "-m", "other"]);
    repo.stage_conflict("sub/f.txt", Some(b"a\n"), Some(b"o\n"), Some(b"t\n"));
    fs::remove_dir_all(repo.root().join("sub")).unwrap();
    std::os::unix::fs::symlink("other", repo.root().join("sub")).unwrap();
    // 덮이면 사라질, 커밋하지 않은 편집.
    repo.write("other/f.txt", "OTHER EDITED\n");
    block_on(async {
        let f = fixture(repo);
        let window = proceed(&f);
        assert_eq!(version(&f, "sub/f.txt"), None, "no version through a link");
        let refused = content_at(&f, "sub/f.txt", "RESOLVED\n", None).await;
        assert_eq!(rpc_code(&refused), code::INVALID_PARAMS, "{refused:?}");
        assert_eq!(
            fs::read_to_string(f.repo.root().join("other/f.txt")).unwrap(),
            "OTHER EDITED\n"
        );
        assert_eq!(index(&f, "sub/f.txt").len(), 3, "still three stages");
        assert_eq!(window.seen(), [], "refused before asking the window");
    });
}

/// index 의 UTF-8 이 아닌 경로(Linux 에서 만든 저장소)는 목록에 U+FFFD 로 보이지만 그 이름으로 get · resolve 하면 invalid params
/// 다 — 그 이름의 새 파일을 만들거나 stage 하지 않는다. 그 글자 그대로의 이름(UTF-8)이 따로 충돌 중이면 그것을 고른다.
#[test]
fn a_path_that_is_not_utf8_cannot_be_named() {
    let repo = TestRepo::with_commit();
    let blob = repo.base().join("blob");
    fs::write(&blob, "x\n").unwrap();
    let id = repo.git(&["hash-object", "-w", "--", blob.to_str().unwrap()]);
    let mut info = Vec::new();
    for stage in 1..=3 {
        info.extend_from_slice(format!("100644 {} {stage}\t", id.trim()).as_bytes());
        info.extend_from_slice(b"caf\xe9.txt\n");
    }
    let list = repo.base().join("index-info");
    fs::write(&list, info).unwrap();
    let out = repo
        .command_in(repo.root())
        .args(["update-index", "--index-info"])
        .stdin(fs::File::open(&list).unwrap())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let ls = |r: &TestRepo| {
        r.command_in(r.root())
            .args(["ls-files", "-s", "-z"])
            .output()
            .unwrap()
            .stdout
    };
    let names = |r: &TestRepo| {
        let mut n: Vec<_> = fs::read_dir(r.root())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        n.sort();
        n
    };
    let (index_before, files_before) = (ls(&repo), names(&repo));
    block_on(async {
        let f = fixture(repo);
        let window = proceed(&f);
        let shown = "caf\u{FFFD}.txt";
        assert_eq!(paths(&conflicts(&f)), [shown]);
        let err = f
            .write
            .git()
            .conflict(&GitConflictParams {
                project_id: f.project.clone(),
                worktree: None,
                path: shown.to_owned(),
            })
            .unwrap_err();
        assert_eq!(err.code, code::INVALID_PARAMS, "{err:?}");
        let by_content = content_at(&f, shown, "resolved\n", None).await;
        assert_eq!(
            rpc_code(&by_content),
            code::INVALID_PARAMS,
            "{by_content:?}"
        );
        let by_side = pick(&f, shown, ConflictPick::Theirs).await;
        assert_eq!(rpc_code(&by_side), code::INVALID_PARAMS, "{by_side:?}");
        assert_eq!(names(&f.repo), files_before, "no file was made");
        assert_eq!(ls(&f.repo), index_before, "the index is as it was");
        assert_eq!(window.seen(), []);

        // 같은 글자로 보이는 진짜 이름: 바이트가 같은 그것을 고르고, UTF-8 이 아닌 쪽은 그대로다.
        f.repo
            .stage_conflict(shown, Some(b"a\n"), Some(b"real ours\n"), Some(b"t\n"));
        let real = f
            .write
            .git()
            .conflict(&GitConflictParams {
                project_id: f.project.clone(),
                worktree: None,
                path: shown.to_owned(),
            })
            .unwrap();
        assert_eq!(real.ours.unwrap().size, b"real ours\n".len() as u64);
        let left = content(&f, shown, "resolved\n").await.unwrap();
        assert_eq!(paths(&left), [shown], "the one that is not UTF-8 stays");
        assert_eq!(index(&f, shown).len(), 1, "the real name is resolved");
        assert_eq!(
            fs::read_to_string(f.repo.root().join(shown)).unwrap(),
            "resolved\n"
        );
    });
}

/// 미저장 확인을 기다리는 사이 파일이 바뀌어도(창이 저장했거나 터미널에서 고침) 잠금 안에서 다시 견주어 `CONFLICT` 다.
#[test]
fn content_over_a_file_changed_while_asking_is_a_conflict() {
    let repo = TestRepo::with_commit();
    repo.stage_conflict("x.txt", Some(b"a\n"), Some(b"o\n"), Some(b"t\n"));
    let file = repo.root().join("x.txt");
    block_on(async {
        let f = fixture(repo);
        let edited = file.clone();
        let _window = Window::open(&f.write, &f.project, move |_| {
            fs::write(&edited, "saved while asking\n").unwrap();
            Some(GitPromptAnswer::Proceed)
        });
        let refused = content(&f, "x.txt", "resolved\n").await;
        assert_eq!(rpc_code(&refused), code::CONFLICT, "{refused:?}");
        assert_eq!(fs::read_to_string(&file).unwrap(), "saved while asking\n");
        assert_eq!(index(&f, "x.txt").len(), 3, "still three stages");
    });
}

/// 한쪽 고르기도 get 뒤에 바뀐 작업 트리 파일을 덮지 않는다: `git checkout` 이 덮을 파일도, `git rm` 이 지울 파일도 `CONFLICT` 이고
/// 편집과 충돌이 그대로다. 지금 `version` 으로 다시 보내면 풀린다.
#[test]
fn a_side_over_a_file_changed_since_get_is_a_conflict() {
    let repo = TestRepo::with_commit();
    repo.stage_conflict("x.txt", Some(b"a\n"), Some(b"o\n"), Some(b"t\n"));
    repo.stage_conflict("d.txt", Some(b"a\n"), Some(b"o\n"), None);
    block_on(async {
        let f = fixture(repo);
        let window = proceed(&f);
        let (x, d) = (f.repo.root().join("x.txt"), f.repo.root().join("d.txt"));
        let (seen_x, seen_d) = (version(&f, "x.txt"), version(&f, "d.txt"));
        assert!(seen_x.is_some() && seen_d.is_some());
        fs::write(&x, "edited by hand\n").unwrap();
        fs::write(&d, "kept by hand\n").unwrap();

        for expected in [seen_x, None] {
            let refused = pick_at(&f, "x.txt", ConflictPick::Theirs, expected.clone()).await;
            assert_eq!(rpc_code(&refused), code::CONFLICT, "{expected:?}");
        }
        // 그쪽은 지웠다 — 고르면 `git rm`.
        let refused = pick_at(&f, "d.txt", ConflictPick::Theirs, seen_d).await;
        assert_eq!(rpc_code(&refused), code::CONFLICT);
        assert_eq!(fs::read_to_string(&x).unwrap(), "edited by hand\n");
        assert_eq!(fs::read_to_string(&d).unwrap(), "kept by hand\n");
        assert_eq!(index(&f, "x.txt").len(), 3, "still three stages");
        assert_eq!(index(&f, "d.txt").len(), 2, "still two stages");
        assert_eq!(window.seen(), [], "refused before asking the window");

        pick(&f, "x.txt", ConflictPick::Theirs).await.unwrap();
        assert_eq!(fs::read_to_string(&x).unwrap(), "t\n");
        pick(&f, "d.txt", ConflictPick::Theirs).await.unwrap();
        assert!(!d.exists());
    });
}

/// 미저장 확인을 기다리는 사이 바뀐 파일도 한쪽 고르기가 잠금 안에서 다시 견주어 `CONFLICT` 다.
#[test]
fn a_side_over_a_file_changed_while_asking_is_a_conflict() {
    let repo = TestRepo::with_commit();
    repo.stage_conflict("x.txt", Some(b"a\n"), Some(b"o\n"), Some(b"t\n"));
    let file = repo.root().join("x.txt");
    block_on(async {
        let f = fixture(repo);
        let edited = file.clone();
        let _window = Window::open(&f.write, &f.project, move |_| {
            fs::write(&edited, "saved while asking\n").unwrap();
            Some(GitPromptAnswer::Proceed)
        });
        let refused = pick(&f, "x.txt", ConflictPick::Ours).await;
        assert_eq!(rpc_code(&refused), code::CONFLICT, "{refused:?}");
        assert_eq!(fs::read_to_string(&file).unwrap(), "saved while asking\n");
        assert_eq!(index(&f, "x.txt").len(), 3, "still three stages");
    });
}

/// 50 MiB 를 넘는 작업 트리 파일: get 은 내용을 읽지 않은 `large-` version 을 주고, 그것으로 한쪽 고르기는 되지만 내용 쓰기는
/// invalid params 다. 같은 크기 · mtime 의 다른 파일로 바꿔 끼워도(원자 쓰기) inode 로 `CONFLICT` 다.
#[test]
fn a_large_work_tree_file_is_compared_by_its_stat_for_a_side() {
    let repo = TestRepo::with_commit();
    repo.stage_conflict("big.bin", Some(b"a\n"), Some(b"o\n"), Some(b"t\n"));
    let file = repo.root().join("big.bin");
    let size = usize::try_from(pw_fs::limits::READ_ONLY_MAX).unwrap() + 1;
    fs::write(&file, vec![b'x'; size]).unwrap();
    block_on(async {
        let f = fixture(repo);
        let _window = proceed(&f);
        let seen = version(&f, "big.bin").expect("a large file has a version");
        assert!(seen.starts_with("large-"), "{seen}");
        let refused = content_at(&f, "big.bin", "small\n", Some(seen.clone())).await;
        assert_eq!(rpc_code(&refused), code::INVALID_PARAMS, "{refused:?}");

        #[cfg(unix)]
        {
            let mtime = fs::metadata(&file).unwrap().modified().unwrap();
            let swap = f.repo.root().join("big.swap");
            fs::write(&swap, vec![b'y'; size]).unwrap();
            fs::File::options()
                .write(true)
                .open(&swap)
                .unwrap()
                .set_modified(mtime)
                .unwrap();
            fs::rename(&swap, &file).unwrap();
            let refused = pick_at(&f, "big.bin", ConflictPick::Theirs, Some(seen)).await;
            assert_eq!(rpc_code(&refused), code::CONFLICT, "{refused:?}");
            assert_eq!(fs::metadata(&file).unwrap().len(), size as u64);
        }

        pick(&f, "big.bin", ConflictPick::Theirs).await.unwrap();
        assert_eq!(fs::read(&file).unwrap(), b"t\n");
    });
}

/// 내용 해결은 `fs.write` 와 같은 경로별 쓰기 잠금 안에서 확인하고 쓴다: 해결이 확인을 지난 뒤 같은 `version` 으로 온 저장은
/// 해결이 쓴 뒤에야 확인해 `CONFLICT` 다(둘 다 확인을 지나 하나가 말없이 사라지지 않는다).
#[test]
fn content_and_an_fs_write_of_the_same_file_take_turns() {
    use std::sync::mpsc;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use pw_fs::methods::{FsContent, FsWriteParams, FsWritten};
    use pw_rpc::RpcError;

    let repo = TestRepo::with_commit();
    repo.stage_conflict("x.txt", Some(b"a\n"), Some(b"o\n"), Some(b"t\n"));
    let file = repo.root().join("x.txt");
    block_on(async {
        let f = fixture(repo);
        let _window = proceed(&f);
        let seen = version(&f, "x.txt");
        let save = FsWriteParams {
            project_id: f.project.clone(),
            worktree: None,
            path: file.display().to_string(),
            content: FsContent::Text {
                text: "saved\n".into(),
            },
            bom: false,
            expected_version: seen,
        };
        let (tx, rx) = mpsc::channel::<Result<FsWritten, RpcError>>();
        let (tx, rx) = (Mutex::new(Some(tx)), Arc::new(Mutex::new(rx)));
        let (fs_service, waiting) = (Arc::clone(f.write.fs()), Arc::clone(&rx));
        // 그 프로젝트 창의 저장이다(`fs.write` 는 그 연결만 쓴다).
        let scope = pw_rpc::ConnectionScope::Project(f.project.clone());
        let saved = Arc::new(Mutex::new(None));
        let slot = Arc::clone(&saved);
        // 해결이 확인을 지나 쓰기 직전: 같은 파일의 저장을 띄우고 끝나기를 잠시 기다린다. 잠금을 나누면 저장은 해결이 쓸 때까지
        // 기다리므로 여기서는 끝나지 않는다.
        *f.write.before_conflict_write.lock().unwrap() = Some(Arc::new(move || {
            let (fs_service, save, scope) = (Arc::clone(&fs_service), save.clone(), scope.clone());
            let tx = tx.lock().unwrap().take().expect("one write");
            std::thread::spawn(move || tx.send(fs_service.write_in(&scope, &save)).unwrap());
            if let Ok(done) = waiting.lock().unwrap().recv_timeout(Duration::from_secs(1)) {
                *slot.lock().unwrap() = Some(done);
            }
        }));
        content(&f, "x.txt", "resolved\n").await.unwrap();
        let saved = saved
            .lock()
            .unwrap()
            .take()
            .unwrap_or_else(|| rx.lock().unwrap().recv().unwrap());
        let code = saved.as_ref().map_err(|e| e.code).err();
        assert_eq!(code, Some(code::CONFLICT), "{saved:?}");
        assert_eq!(fs::read_to_string(&file).unwrap(), "resolved\n");
    });
}
