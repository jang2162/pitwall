//! 줄 단위(완료 기준 ① ~ ③): 픽스처(CRLF · 파일 끝 개행 없음 · 더하기와 빼기가 섞인 hunk · 바이너리 · 이름 바뀜)에서 hunk 안 한
//! 줄만 스테이징한 뒤 `git diff --cached` 가 기대 패치와 바이트로 같고, 같은 줄을 언스테이징하면 index 가 원래대로, 줄 하나 버리기
//! 뒤 작업 트리 바이트가 기대값(CRLF 유지). 보낸 hunk 가 지금 diff 와 다르면 아무것도 쓰지 않는다.

use pw_rpc::code as rpc_code;

use super::{bytes, diff, index, proceed, select};
use crate::diff::methods::{DiffSide, FileStatus, LineKind};
use crate::stage::methods::{GitDiscardLinesParams, GitLinesParams, HunkSelection};
use crate::testing::write::WriteFixture;
use crate::write::WriteError;

fn params(f: &WriteFixture, path: &str, hunks: Vec<HunkSelection>) -> GitLinesParams {
    GitLinesParams {
        project_id: f.project.clone(),
        worktree: None,
        path: path.to_owned(),
        old_path: None,
        hunks,
        ignore_whitespace: false,
    }
}

fn discard(f: &WriteFixture, path: &str, hunks: Vec<HunkSelection>) -> GitDiscardLinesParams {
    GitDiscardLinesParams {
        project_id: f.project.clone(),
        worktree: None,
        path: path.to_owned(),
        hunks,
        ignore_whitespace: false,
        confirmed: true,
    }
}

/// `git hash-object` 로 본 내용의 blob 해시.
fn blob_id(f: &WriteFixture, contents: &[u8]) -> String {
    let file = f.repo.base().join("hash-input");
    std::fs::write(&file, contents).unwrap();
    let id = f
        .repo
        .git(&["hash-object", "--no-filters", file.to_str().unwrap()]);
    id.trim()[..7].to_owned()
}

fn code(e: WriteError) -> i32 {
    match e {
        WriteError::Rpc(e) => e.code,
        other => panic!("not an rpc error: {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn one_line_of_a_mixed_hunk_stages_to_the_expected_patch_and_unstages_back() {
    let f = WriteFixture::new(true);
    f.repo
        .commit_file("mixed.txt", "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n", "mixed");
    let original = index(&f);
    f.repo
        .write("mixed.txt", "1\n2\n3\n4\nfive\n6\nnew\n8\n9\n10\n");
    let file = diff(&f, DiffSide::WorkTree, "mixed.txt");
    assert_eq!(file.hunks.len(), 1, "one hunk with added and removed lines");
    let hunk = &file.hunks[0];
    let new_line = select(hunk, |l| l.kind == LineKind::Added && l.text == "new");

    f.write
        .stage_lines(params(&f, "mixed.txt", vec![new_line]))
        .await
        .unwrap();
    let staged = b"1\n2\n3\n4\n5\n6\n7\nnew\n8\n9\n10\n";
    assert_eq!(bytes(&f, &["show", ":mixed.txt"]), staged);
    let expected = format!(
        "diff --git a/mixed.txt b/mixed.txt\nindex {}..{} 100644\n--- a/mixed.txt\n+++ b/mixed.txt\n@@ -5,6 +5,7 @@\n 5\n 6\n 7\n+new\n 8\n 9\n 10\n",
        blob_id(&f, b"1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n"),
        blob_id(&f, staged),
    );
    assert_eq!(
        String::from_utf8(bytes(&f, &["diff", "--cached", "--", "mixed.txt"])).unwrap(),
        expected
    );
    // 작업 트리는 그대로다.
    assert_eq!(
        std::fs::read(f.repo.root().join("mixed.txt")).unwrap(),
        b"1\n2\n3\n4\nfive\n6\nnew\n8\n9\n10\n"
    );

    let staged_diff = diff(&f, DiffSide::Index, "mixed.txt");
    let same_line = select(&staged_diff.hunks[0], |l| {
        l.kind == LineKind::Added && l.text == "new"
    });
    f.write
        .unstage_lines(params(&f, "mixed.txt", vec![same_line]))
        .await
        .unwrap();
    assert_eq!(index(&f), original, "the index is back");
    assert!(bytes(&f, &["diff", "--cached"]).is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn crlf_lines_stay_crlf_when_staged_and_when_one_is_discarded() {
    let f = WriteFixture::new(true);
    f.repo.commit_file("crlf.txt", "a\r\nb\r\nc\r\n", "crlf");
    f.repo.write("crlf.txt", "a\r\nB\r\nc\r\nD\r\n");
    let file = diff(&f, DiffSide::WorkTree, "crlf.txt");
    let hunk = &file.hunks[0];

    // `-b` · `+B` 만 스테이징한다(`+D` 는 두고).
    let swap = select(hunk, |l| l.text == "b\r" || l.text == "B\r");
    f.write
        .stage_lines(params(&f, "crlf.txt", vec![swap]))
        .await
        .unwrap();
    assert_eq!(bytes(&f, &["show", ":crlf.txt"]), b"a\r\nB\r\nc\r\n");

    // 남은 작업 트리 diff(`+D`)의 그 줄 하나를 버린다.
    let file = diff(&f, DiffSide::WorkTree, "crlf.txt");
    let d = select(&file.hunks[0], |l| {
        l.kind == LineKind::Added && l.text == "D\r"
    });
    let _window = proceed(&f);
    f.write
        .discard_lines(discard(&f, "crlf.txt", vec![d]))
        .await
        .unwrap();
    assert_eq!(
        std::fs::read(f.repo.root().join("crlf.txt")).unwrap(),
        b"a\r\nB\r\nc\r\n"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn one_line_is_discarded_from_the_work_tree_and_the_rest_stays() {
    let f = WriteFixture::new(true);
    f.repo.commit_file("crlf.txt", "a\r\nb\r\nc\r\n", "crlf");
    f.repo.write("crlf.txt", "a\r\nB\r\nc\r\nD\r\n");
    let file = diff(&f, DiffSide::WorkTree, "crlf.txt");
    // `-b` 만 버린다 — `b` 가 되살아나고 `B` 와 `D` 는 남는다.
    let b = select(&file.hunks[0], |l| l.kind == LineKind::Removed);
    let _window = proceed(&f);
    f.write
        .discard_lines(discard(&f, "crlf.txt", vec![b]))
        .await
        .unwrap();
    assert_eq!(
        std::fs::read(f.repo.root().join("crlf.txt")).unwrap(),
        b"a\r\nb\r\nB\r\nc\r\nD\r\n"
    );
    assert_eq!(
        bytes(&f, &["show", ":crlf.txt"]),
        b"a\r\nb\r\nc\r\n",
        "the index is untouched"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_missing_final_newline_is_kept_in_the_patch() {
    let f = WriteFixture::new(true);
    f.repo.commit_file("noeol.txt", "one\ntwo", "noeol");
    let original = index(&f);
    f.repo.write("noeol.txt", "one\ntwo\nthree");
    let file = diff(&f, DiffSide::WorkTree, "noeol.txt");
    // 줄바꿈만 더하는 쪽(`-two` · `+two`)을 스테이징하고 `+three` 는 둔다.
    let newline = select(&file.hunks[0], |l| l.text == "two");
    f.write
        .stage_lines(params(&f, "noeol.txt", vec![newline]))
        .await
        .unwrap();
    assert_eq!(bytes(&f, &["show", ":noeol.txt"]), b"one\ntwo\n");
    let expected = format!(
        "diff --git a/noeol.txt b/noeol.txt\nindex {}..{} 100644\n--- a/noeol.txt\n+++ b/noeol.txt\n@@ -1,2 +1,2 @@\n one\n-two\n\\ No newline at end of file\n+two\n",
        blob_id(&f, b"one\ntwo"),
        blob_id(&f, b"one\ntwo\n"),
    );
    assert_eq!(
        String::from_utf8(bytes(&f, &["diff", "--cached"])).unwrap(),
        expected
    );

    // 스테이징된 diff 의 hunk 전체를 언스테이징하면 index 가 원래대로다.
    let staged = diff(&f, DiffSide::Index, "noeol.txt");
    f.write
        .unstage_lines(params(
            &f,
            "noeol.txt",
            vec![HunkSelection {
                hunk: staged.hunks[0].clone(),
                lines: None,
            }],
        ))
        .await
        .unwrap();
    assert_eq!(index(&f), original);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_renamed_file_unstages_lines_against_its_old_path() {
    let f = WriteFixture::new(true);
    let original_text = "1\n2\n3\n4\n5\n6\n";
    f.repo.commit_file("old.txt", original_text, "old");
    f.repo.git(&["mv", "old.txt", "new.txt"]);
    f.repo.write("new.txt", "1\n2\nthree\n4\n5\n6\n");
    f.repo.git(&["add", "new.txt"]);
    // 이름 바뀜은 거른 경로들 사이에서만 찾는다 — 두 경로를 다 거른다(`git diff -- old new` 와 같다).
    let staged = f
        .write
        .git()
        .diff(&crate::diff::methods::GitDiffParams {
            project_id: f.project.clone(),
            worktree: None,
            from: None,
            to: DiffSide::Index,
            paths: Some(vec!["old.txt".into(), "new.txt".into()]),
            ignore_whitespace: false,
        })
        .unwrap()
        .files
        .remove(0);
    assert_eq!(staged.status, FileStatus::Renamed);
    assert_eq!(staged.old_path.as_deref(), Some("old.txt"));

    let mut request = params(
        &f,
        "new.txt",
        vec![HunkSelection {
            hunk: staged.hunks[0].clone(),
            lines: None,
        }],
    );
    request.old_path = Some("old.txt".into());
    f.write.unstage_lines(request).await.unwrap();
    assert_eq!(bytes(&f, &["show", ":new.txt"]), original_text.as_bytes());
    // 이름 바뀜은 그대로 스테이징돼 있고 내용 변경만 풀렸다.
    assert_eq!(
        f.repo.git(&["diff", "--cached", "--name-status", "-M"]),
        "R100\told.txt\tnew.txt\n"
    );
    // 작업 트리의 변경은 그대로다.
    assert_eq!(
        std::fs::read_to_string(f.repo.root().join("new.txt")).unwrap(),
        "1\n2\nthree\n4\n5\n6\n"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_binary_file_is_refused_line_by_line_and_staged_whole_byte_for_byte() {
    let f = WriteFixture::new(true);
    let path = f.repo.root().join("bin.dat");
    std::fs::write(&path, b"\x00\x01\r\n\xff text").unwrap();
    f.repo.commit_all("binary");
    let original = index(&f);
    let changed = b"\x00\x02\r\n\xfe text\x00";
    std::fs::write(&path, changed).unwrap();
    let file = diff(&f, DiffSide::WorkTree, "bin.dat");
    assert!(file.binary && file.hunks.is_empty());

    // 바이너리에는 hunk 가 없다 — 지어낸 hunk 를 보내도 줄 단위는 거절한다.
    let made_up = crate::diff::methods::DiffHunk {
        old_start: 1,
        old_lines: 1,
        new_start: 1,
        new_lines: 1,
        header: String::new(),
        lines: Vec::new(),
    };
    let refused = f
        .write
        .stage_lines(params(
            &f,
            "bin.dat",
            vec![HunkSelection {
                hunk: made_up,
                lines: None,
            }],
        ))
        .await
        .unwrap_err();
    assert_eq!(code(refused), rpc_code::INVALID_PARAMS);
    assert_eq!(index(&f), original);

    let paths = crate::stage::methods::GitPathsParams {
        project_id: f.project.clone(),
        worktree: None,
        paths: vec!["bin.dat".into()],
    };
    f.write.stage(paths.clone()).await.unwrap();
    assert_eq!(bytes(&f, &["show", ":bin.dat"]), changed);
    f.write.unstage(paths).await.unwrap();
    assert_eq!(index(&f), original);
    assert_eq!(std::fs::read(&path).unwrap(), changed);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stale_or_whitespace_blind_selection_writes_nothing() {
    let f = WriteFixture::new(true);
    f.repo.commit_file("f.txt", "1\n2\n3\n", "f");
    f.repo.write("f.txt", "1\ntwo\n3\n");
    let original = index(&f);
    let hunk = diff(&f, DiffSide::WorkTree, "f.txt").hunks[0].clone();
    let all = || HunkSelection {
        hunk: hunk.clone(),
        lines: None,
    };

    // 그 사이 파일이 바뀌었다 — 보낸 hunk 는 지금 diff 에 없다.
    f.repo.write("f.txt", "1\nTWO\n3\n");
    let stale = f
        .write
        .stage_lines(params(&f, "f.txt", vec![all()]))
        .await
        .unwrap_err();
    assert_eq!(code(stale), rpc_code::CONFLICT);
    assert_eq!(index(&f), original);
    f.repo.write("f.txt", "1\ntwo\n3\n");

    let mut blind = params(&f, "f.txt", vec![all()]);
    blind.ignore_whitespace = true;
    assert_eq!(
        code(f.write.stage_lines(blind).await.unwrap_err()),
        rpc_code::INVALID_PARAMS
    );
    let empty = HunkSelection {
        hunk: hunk.clone(),
        lines: Some(Vec::new()),
    };
    let context = HunkSelection {
        hunk: hunk.clone(),
        lines: Some(vec![0]),
    };
    let outside = HunkSelection {
        hunk: hunk.clone(),
        lines: Some(vec![99]),
    };
    for bad in [empty, context, outside] {
        assert_eq!(
            code(
                f.write
                    .stage_lines(params(&f, "f.txt", vec![bad.clone()]))
                    .await
                    .unwrap_err()
            ),
            rpc_code::INVALID_PARAMS,
            "{bad:?}"
        );
    }
    let mut unconfirmed = discard(&f, "f.txt", vec![all()]);
    unconfirmed.confirmed = false;
    assert_eq!(
        code(f.write.discard_lines(unconfirmed).await.unwrap_err()),
        rpc_code::INVALID_PARAMS
    );
    assert_eq!(index(&f), original);
    assert_eq!(
        std::fs::read_to_string(f.repo.root().join("f.txt")).unwrap(),
        "1\ntwo\n3\n"
    );

    // 같은 hunk 가 그대로면 된다.
    f.write
        .stage_lines(params(&f, "f.txt", vec![all()]))
        .await
        .unwrap();
    assert_eq!(bytes(&f, &["show", ":f.txt"]), b"1\ntwo\n3\n");
}

/// 저장소가 `core.autocrlf` 를 켜도(`input` · `true`) CRLF 로 커밋된 파일의 줄은 CRLF 로 스테이징 · 버려진다 — git 처럼 index 에
/// CRLF 가 있으면 작업 트리 쪽을 바꾸지 않고 diff 를 만든다(`git.diff` 와 같은 판정이라 보낸 hunk 와도 맞는다).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn autocrlf_does_not_turn_staged_or_discarded_crlf_lines_into_lf() {
    for autocrlf in ["input", "true"] {
        let f = WriteFixture::new(true);
        f.repo.commit_file("crlf.txt", "a\r\nb\r\nc\r\n", "crlf");
        f.repo.git(&["config", "core.autocrlf", autocrlf]);
        f.repo.write("crlf.txt", "a\r\nB\r\nc\r\nD\r\n");
        let file = diff(&f, DiffSide::WorkTree, "crlf.txt");
        assert!(
            file.hunks[0].lines.iter().all(|l| l.text.ends_with('\r')),
            "{autocrlf}: {:?}",
            file.hunks
        );
        let swap = select(&file.hunks[0], |l| l.text == "b\r" || l.text == "B\r");
        f.write
            .stage_lines(params(&f, "crlf.txt", vec![swap]))
            .await
            .unwrap();
        assert_eq!(
            bytes(&f, &["show", ":crlf.txt"]),
            b"a\r\nB\r\nc\r\n",
            "{autocrlf}"
        );

        let file = diff(&f, DiffSide::WorkTree, "crlf.txt");
        let d = select(&file.hunks[0], |l| l.kind == LineKind::Added);
        let _window = proceed(&f);
        f.write
            .discard_lines(discard(&f, "crlf.txt", vec![d]))
            .await
            .unwrap();
        assert_eq!(
            std::fs::read(f.repo.root().join("crlf.txt")).unwrap(),
            b"a\r\nB\r\nc\r\n",
            "{autocrlf}"
        );
    }
}

/// 작업 트리에서 지운 파일: 줄 단위 스테이징은 지운 줄을 index 에 담고(빈 내용까지 — 지움 자체는 파일 단위로), 줄 단위 버리기는
/// 거절한다(되살리기는 파일 단위 `git.discard`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_file_deleted_in_the_work_tree_stages_removed_lines_and_refuses_line_discard() {
    let f = WriteFixture::new(true);
    f.repo.commit_file("gone.txt", "1\n2\n3\n", "gone");
    std::fs::remove_file(f.repo.root().join("gone.txt")).unwrap();
    let file = diff(&f, DiffSide::WorkTree, "gone.txt");
    assert_eq!(file.status, FileStatus::Deleted);
    let two = select(&file.hunks[0], |l| l.text == "2");

    let _window = proceed(&f);
    let refused = f
        .write
        .discard_lines(discard(&f, "gone.txt", vec![two.clone()]))
        .await
        .unwrap_err();
    assert_eq!(code(refused), rpc_code::INVALID_PARAMS);

    f.write
        .stage_lines(params(&f, "gone.txt", vec![two]))
        .await
        .unwrap();
    assert_eq!(bytes(&f, &["show", ":gone.txt"]), b"1\n3\n");
    assert!(!f.repo.root().join("gone.txt").exists());
}

/// 사람의 설정이 공백 오류를 고치게 해 둬도(`apply.whitespace=fix`) 고른 줄은 바이트 그대로 스테이징된다(`--whitespace=nowarn`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn trailing_whitespace_is_staged_as_is_whatever_apply_whitespace_says() {
    for setting in ["fix", "error"] {
        let f = WriteFixture::new(true);
        f.repo.git(&["config", "apply.whitespace", setting]);
        f.repo.commit_file("ws.txt", "1\n2\n", "ws");
        f.repo.write("ws.txt", "1\ntail  \t\n2\n");
        let file = diff(&f, DiffSide::WorkTree, "ws.txt");
        let tail = select(&file.hunks[0], |l| l.kind == LineKind::Added);
        f.write
            .stage_lines(params(&f, "ws.txt", vec![tail]))
            .await
            .unwrap();
        assert_eq!(
            bytes(&f, &["show", ":ws.txt"]),
            b"1\ntail  \t\n2\n",
            "{setting}"
        );
    }
}
