//! 파일 히스토리 시험: 결과를 같은 리비전의 `git log --follow --name-status` 와 커밋 · 순서 · 상태 · 경로까지 맞춘다.

use std::path::Path;

use super::methods::GitFileHistoryParams;
use super::*;
use crate::blame::tests::rpc::Fx;
use crate::testing::TestRepo;
use crate::testing::blame::{load, random};
use crate::testing::repo::git_command;
use pw_rpc::protocol::StreamOutcome;

mod view_fixture;

/// 행 하나를 `git log --name-status` 의 꼴로: (해시, 상태 글자, 경로들 — 이름 바뀜 · 복사는 원래 경로가 앞).
type Entry = (String, char, Vec<String>);

fn git_follow(dir: &Path, ceiling: &Path, rev: &str, path: &str) -> Vec<Entry> {
    let out = git_command(dir, ceiling)
        .args([
            "-c",
            "core.quotePath=false",
            "log",
            "--follow",
            "--name-status",
            "--format=%x01%H",
            rev,
            "--",
            path,
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8(out.stdout).unwrap();
    text.split('\x01')
        .filter(|r| !r.is_empty())
        .map(|record| {
            let mut lines = record.lines().filter(|l| !l.is_empty());
            let hash = lines.next().unwrap().to_owned();
            let status: Vec<&str> = lines
                .next()
                .expect("a name-status line")
                .split('\t')
                .collect();
            assert!(lines.next().is_none(), "one path per commit: {record:?}");
            let letter = status[0].chars().next().unwrap();
            (
                hash,
                letter,
                status[1..].iter().map(|s| (*s).to_owned()).collect(),
            )
        })
        .collect()
}

fn entry(row: &FileHistoryRow) -> Entry {
    let letter = match row.change {
        FileChangeKind::Added => 'A',
        FileChangeKind::Modified => 'M',
        FileChangeKind::Deleted => 'D',
        FileChangeKind::Renamed => 'R',
        FileChangeKind::Copied => 'C',
        FileChangeKind::TypeChanged => 'T',
    };
    let mut paths: Vec<String> = row.old_path.iter().cloned().collect();
    paths.push(row.path.clone());
    (row.hash.clone(), letter, paths)
}

fn history(dir: &Path, rev: &str, path: &str) -> Vec<FileHistoryRow> {
    let repo = crate::blame::tests::open(dir);
    let start = crate::blame::start_commit(&repo, Some(rev))
        .unwrap()
        .unwrap();
    let mut rows = Vec::new();
    let done = run(
        &repo,
        false,
        start,
        path.into(),
        &AtomicBool::new(false),
        &mut |c| {
            rows.extend(c.rows);
            true
        },
    )
    .unwrap();
    assert!(done);
    rows
}

fn assert_matches(dir: &Path, ceiling: &Path, rev: &str, path: &str) -> Vec<FileHistoryRow> {
    let rows = history(dir, rev, path);
    let ours: Vec<Entry> = rows.iter().map(entry).collect();
    let theirs = git_follow(dir, ceiling, rev, path);
    if ours != theirs {
        // 첫 차이 언저리만 보인다(전체는 수백 행이다).
        let at = (0..ours.len().max(theirs.len()))
            .find(|&i| ours.get(i) != theirs.get(i))
            .unwrap();
        let window = |v: &[Entry]| v[at.saturating_sub(2)..(at + 3).min(v.len())].to_vec();
        panic!(
            "git log --follow {rev} -- {path}: first difference at row {at} (ours {}, git {})\nours: {:?}\ngit:  {:?}",
            ours.len(),
            theirs.len(),
            window(&ours),
            window(&theirs)
        );
    }
    rows
}

#[test]
fn the_fixture_history_follows_both_renames_like_git() {
    let repo = TestRepo::new();
    let fx = repo.blame_fixture();
    let rows = assert_matches(repo.root(), repo.base(), "HEAD", fx.path);
    let changes: Vec<(FileChangeKind, &str, Option<&str>)> = rows
        .iter()
        .map(|r| (r.change, r.path.as_str(), r.old_path.as_deref()))
        .collect();
    use FileChangeKind::*;
    assert_eq!(
        changes,
        [
            (Modified, "dir/c.txt", None),
            (Renamed, "dir/c.txt", Some("b.txt")),
            (Modified, "b.txt", None),
            (Modified, "b.txt", None),
            (Modified, "b.txt", None),
            (Renamed, "b.txt", Some("a.txt")),
            (Modified, "a.txt", None),
            (Added, "a.txt", None),
        ]
    );
    assert!(
        rows.iter().all(|r| r.hash != fx.merge),
        "merges are not shown"
    );
    assert_eq!(rows[0].subject, "append");
    assert_eq!(rows[0].author_name, "Pitwall Test");
    // 앞 리비전에서 시작해도 같다.
    assert_matches(repo.root(), repo.base(), &fx.merge, fx.paths[1]);
}

#[test]
fn a_deleted_file_has_a_history_and_a_directory_is_refused() {
    let repo = TestRepo::new();
    repo.commit_file("dir/gone.txt", "one\n", "add");
    repo.commit_file("dir/gone.txt", "two\n", "edit");
    repo.git(&["rm", "--quiet", "dir/gone.txt"]);
    repo.commit_file("dir/keep.txt", "k\n", "remove gone");
    let rows = assert_matches(repo.root(), repo.base(), "HEAD", "dir/gone.txt");
    assert_eq!(rows[0].change, FileChangeKind::Deleted);
    assert_eq!(rows.len(), 3);
    let r = crate::blame::tests::open(repo.root());
    let head = crate::blame::start_commit(&r, None).unwrap().unwrap();
    assert_eq!(
        check_start(&r, head, "dir".into()).unwrap_err().code,
        pw_rpc::code::INVALID_PARAMS
    );
}

/// 서브모듈 안의 파일은 이 저장소의 이력에 없다 — 빈 목록 대신 invalid params 로 그 서브모듈을 알린다(`data.submodule`). 서브모듈
/// 자리 자체(gitlink)는 파일처럼 따라간다(`git log --follow -- sub` 와 같다).
#[test]
fn a_file_inside_a_submodule_is_refused_with_the_submodule() {
    let repo = TestRepo::new();
    repo.commit_file("a.txt", "one\n", "add");
    let gitlink = repo.head();
    repo.git(&[
        "update-index",
        "--add",
        "--cacheinfo",
        &format!("160000,{gitlink},libs/sub"),
    ]);
    repo.git(&["commit", "-q", "-m", "submodule"]);
    let r = crate::blame::tests::open(repo.root());
    let head = crate::blame::start_commit(&r, None).unwrap().unwrap();
    let err = check_start(&r, head, "libs/sub/src/x.txt".into()).unwrap_err();
    assert_eq!(err.code, pw_rpc::code::INVALID_PARAMS);
    let data = err.data.unwrap();
    assert_eq!(data["submodule"], "libs/sub");
    assert!(data["detail"].as_str().unwrap().contains("submodule"));
    // 서브모듈 자리 · 그 밖의 경로는 받는다.
    check_start(&r, head, "libs/sub".into()).unwrap();
    check_start(&r, head, "libs/other/x.txt".into()).unwrap();
    assert_matches(repo.root(), repo.base(), "HEAD", "libs/sub");
}

/// 무작위 이력(이름 바뀜 · 지웠다가 다른 파일의 복사로 다시 만들기 · 갈래와 머지 · 시각 동률 · 시계 어긋남)에서 대상 파일이 거친
/// 모든 이름의 히스토리가 git 과 같다.
#[test]
fn random_histories_follow_like_git() {
    let seeds: u64 = std::env::var("PW_BLAME_SEEDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(8);
    let mut renames = 0;
    for seed in 1..=seeds {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("repo");
        let summary = random(&dir, tmp.path(), 150, seed).unwrap();
        for name in &summary.names {
            let rows = std::panic::catch_unwind(|| assert_matches(&dir, tmp.path(), "main", name));
            let rows = rows.unwrap_or_else(|_| panic!("seed {seed}: {name} differs"));
            renames += rows
                .iter()
                .filter(|r| matches!(r.change, FileChangeKind::Renamed | FileChangeKind::Copied))
                .count();
        }
    }
    assert!(renames > 0, "the histories had no renames to follow");
}

#[test]
fn stopping_or_a_gone_receiver_ends_the_walk() {
    let repo = TestRepo::new();
    let fx = repo.blame_fixture();
    let r = crate::blame::tests::open(repo.root());
    let head = crate::blame::start_commit(&r, None).unwrap().unwrap();
    let err = run(
        &r,
        false,
        head,
        fx.path.into(),
        &AtomicBool::new(true),
        &mut |_| true,
    );
    // 커밋이 256개보다 적으면 멈춤을 보기 전에 끝난다 — 큰 이력에서 본다.
    assert!(err.is_ok());
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("repo");
    load(&dir, tmp.path(), 50, 600, 7).unwrap();
    let r = crate::blame::tests::open(&dir);
    let head = crate::blame::start_commit(&r, None).unwrap().unwrap();
    let err = run(
        &r,
        false,
        head,
        "lib/big.txt".into(),
        &AtomicBool::new(true),
        &mut |_| true,
    )
    .unwrap_err();
    assert_eq!(err.code, pw_rpc::code::CANCELLED);
    let mut sent = 0;
    let done = run(
        &r,
        false,
        head,
        "lib/big.txt".into(),
        &AtomicBool::new(false),
        &mut |_| {
            sent += 1;
            false
        },
    )
    .unwrap();
    assert!(!done);
    assert_eq!(sent, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_stream_sends_a_first_screen_then_the_rest() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("repo");
    load(&dir, tmp.path(), 50, 1200, 3).unwrap();
    let fx = Fx::new("");
    let id = fx.project(&dir);
    let params = GitFileHistoryParams {
        project_id: id.clone(),
        worktree: None,
        path: "lib/big.txt".into(),
        rev: None,
    };
    let (chunks, outcome) = fx
        .stream::<_, FileHistoryChunk>("git.fileHistory", &params)
        .await;
    assert_eq!(outcome, StreamOutcome::Completed);
    assert!(chunks[0].rows.len() <= FIRST_CHUNK);
    assert!(
        chunks
            .iter()
            .all(|c| !c.rows.is_empty() && c.rows.len() <= CHUNK)
    );
    let rows: Vec<FileHistoryRow> = chunks.into_iter().flat_map(|c| c.rows).collect();
    assert_eq!(
        rows,
        assert_matches(&dir, tmp.path(), "HEAD", "lib/big.txt")
    );
    // 흔한 줄(`}`)을 같은 줄로 바꾼 커밋은 파일을 바꾸지 않아 빠진다 — git 도 같다(위).
    assert!(rows.len() > 1100, "{}", rows.len());

    // 저장소가 없거나 HEAD 가 태어나지 않았으면 청크 없이, 폴더 · 나쁜 리비전은 invalid params.
    let unborn = TestRepo::new();
    let unborn_id = fx.project(unborn.root());
    let (chunks, outcome) = fx
        .stream::<_, FileHistoryChunk>(
            "git.fileHistory",
            &GitFileHistoryParams {
                project_id: unborn_id,
                ..params.clone()
            },
        )
        .await;
    assert_eq!(outcome, StreamOutcome::Completed);
    assert!(chunks.is_empty());
    for bad in [
        GitFileHistoryParams {
            path: "lib".into(),
            ..params.clone()
        },
        GitFileHistoryParams {
            rev: Some("nope".into()),
            ..params.clone()
        },
    ] {
        let (_, outcome) = fx
            .stream::<_, FileHistoryChunk>("git.fileHistory", &bad)
            .await;
        assert!(
            matches!(&outcome, StreamOutcome::Failed { error } if error.code == pw_rpc::code::INVALID_PARAMS),
            "{bad:?}: {outcome:?}"
        );
    }
}

/// 얕은 클론: 경계 커밋은 부모가 없는 것으로 본다(그 커밋에서 파일이 생긴 것으로 — git 과 같다).
#[test]
fn shallow_clones_stop_at_the_boundary_like_git() {
    let repo = TestRepo::new();
    let fx = repo.blame_fixture();
    let shallow = repo.base().join("shallow");
    let url = format!("file://{}", repo.root().display());
    repo.git(&[
        "clone",
        "--quiet",
        "--depth",
        "3",
        &url,
        shallow.to_str().unwrap(),
    ]);
    let rows = assert_matches(&shallow, repo.base(), "HEAD", fx.path);
    let last = rows.last().unwrap();
    assert_eq!(last.change, FileChangeKind::Added);
    assert!(last.parents.is_empty());
}
