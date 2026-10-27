//! 픽스처 · 무작위 이력에서 두 엔진을 `git blame` 과 줄마다 맞춘다. 결정 M2-D1 의 되돌림 판정도 여기서 본다.

use std::collections::{BTreeSet, HashSet};

use super::*;
use crate::testing::TestRepo;
use crate::testing::blame::random;

/// 엔진 `git` 의 결과가 `git blame -M` 과 줄마다(커밋 · 원래 줄 · 경로 · 「이 커밋 이전으로」 · 커밋 정보) 같다.
fn assert_cli_matches(dir: &Path, ceiling: &Path, rev: &str, path: &str) -> Vec<Line> {
    let (file, ours, metas) = expand(&blame_cli(dir, ceiling, rev, path));
    let (theirs, their_metas) = git_blame(dir, ceiling, rev, path, true);
    assert_eq!(file.engine, BlameEngine::Git);
    assert_eq!(ours, theirs, "git blame -M {rev} -- {path}");
    for (commit, meta) in &metas {
        assert_eq!(Some(meta), their_metas.get(commit), "commit {commit}");
    }
    ours
}

/// 엔진 `gix` 의 결과가 `git blame`(`-M` 없이)과 줄마다 같다.
fn assert_gix_matches(dir: &Path, ceiling: &Path, rev: &str, path: &str) -> Vec<Line> {
    let (file, ours, metas) = expand(&blame_gix(dir, rev, path));
    let (theirs, their_metas) = git_blame(dir, ceiling, rev, path, false);
    assert_eq!(file.engine, BlameEngine::Gix);
    assert_eq!(ours, theirs, "git blame {rev} -- {path}");
    for (commit, meta) in &metas {
        assert_eq!(Some(meta), their_metas.get(commit), "commit {commit}");
    }
    ours
}

/// 「이 커밋 이전으로」 를 끝까지 따라가며(범위의 `previous` 마다) 그 리비전 · 경로의 blame 도 같은지 본다. 따라간 (리비전,
/// 경로) 수를 돌려준다.
fn follow_previous(
    dir: &Path,
    ceiling: &Path,
    first: Vec<Line>,
    check: fn(&Path, &Path, &str, &str) -> Vec<Line>,
) -> usize {
    let mut seen = HashSet::new();
    let mut todo: Vec<(String, String)> = first.into_iter().filter_map(|l| l.previous).collect();
    while let Some((rev, path)) = todo.pop() {
        if !seen.insert((rev.clone(), path.clone())) {
            continue;
        }
        todo.extend(
            check(dir, ceiling, &rev, &path)
                .into_iter()
                .filter_map(|l| l.previous),
        );
    }
    seen.len()
}

#[test]
fn the_fixture_blames_like_git_blame_m() {
    let repo = TestRepo::new();
    let fx = repo.blame_fixture();
    let (dir, ceiling) = (repo.root(), repo.base());
    let lines = assert_cli_matches(dir, ceiling, "HEAD", fx.path);
    // 옮긴 블록(원래 10~14행)은 끝(26~30행)에서도 첫 커밋의 것이다.
    let first = repo.rev_parse("HEAD~7");
    assert!(lines[25..30].iter().all(|l| l.commit == first), "{lines:?}");
    assert_eq!(
        lines[25..30]
            .iter()
            .map(|l| l.source_line)
            .collect::<Vec<_>>(),
        [10, 11, 12, 13, 14]
    );
    // 이름 바뀜을 따라 처음 이름까지 간다.
    assert!(lines.iter().any(|l| l.path == fx.paths[0]));
    assert!(lines.iter().any(|l| l.path == fx.paths[1]));
    // 머지 · 옮긴 커밋 · 그 이전에서도 같다.
    assert_cli_matches(dir, ceiling, &fx.merge, fx.paths[1]);
    assert_cli_matches(dir, ceiling, &fx.moved, fx.paths[1]);
    assert_cli_matches(dir, ceiling, &format!("{}^", fx.moved), fx.paths[1]);
    let followed = follow_previous(dir, ceiling, lines, assert_cli_matches);
    assert!(followed >= 3, "followed {followed} previous revisions");
}

/// 결정 M2-D1 의 되돌림 판정: gix blame 은 `git blame`(-M 없이)과 같지만 `git blame -M` 과는 옮긴 줄에서 다르다 — 그래서 신뢰한
/// 프로젝트의 blame 은 git CLI 로 간다.
#[test]
fn gix_blame_misses_moved_lines_so_trusted_projects_use_the_cli() {
    let repo = TestRepo::new();
    let fx = repo.blame_fixture();
    let (dir, ceiling) = (repo.root(), repo.base());
    let gix = assert_gix_matches(dir, ceiling, "HEAD", fx.path);
    let (with_moves, _) = git_blame(dir, ceiling, "HEAD", fx.path, true);
    let differ: Vec<usize> = (0..gix.len())
        .filter(|&i| gix[i] != with_moves[i])
        .map(|i| i + 1)
        .collect();
    assert_eq!(differ, [26, 27, 28, 29, 30]);
    assert!(differ.iter().all(|&n| gix[n - 1].commit == fx.moved));
    for rev in [fx.merge.as_str(), fx.moved.as_str()] {
        assert_gix_matches(dir, ceiling, rev, fx.paths[1]);
    }
    let followed = follow_previous(dir, ceiling, gix, assert_gix_matches);
    assert!(followed >= 3, "followed {followed} previous revisions");
}

/// 무작위 이력(이름 바뀜 · 지우고 다시 만들기 · 복사 · 줄 이동 · 갈래와 머지 · 흔한 줄)에서 두 엔진이 git 과 같다. 대상 파일의
/// 끝 이름과, 그 파일이 거친 모든 이름의 마지막 커밋에서 본다.
#[test]
fn random_histories_blame_like_git() {
    let seeds: u64 = std::env::var("PW_BLAME_SEEDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(8);
    for seed in 1..=seeds {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("repo");
        let summary = random(&dir, tmp.path(), 150, seed).unwrap();
        let mut targets: BTreeSet<(String, String)> = BTreeSet::new();
        for name in &summary.names {
            let out = git_command(&dir, tmp.path())
                .args(["log", "-1", "--format=%H", "main", "--", name])
                .output()
                .unwrap();
            let rev = String::from_utf8(out.stdout).unwrap().trim().to_owned();
            if rev.is_empty() {
                continue;
            }
            // 그 커밋에서 그 파일이 지워졌으면 그 부모에서 본다.
            for rev in [rev.clone(), format!("{rev}^")] {
                let exists = git_command(&dir, tmp.path())
                    .args(["cat-file", "-e", &format!("{rev}:{name}")])
                    .status()
                    .unwrap()
                    .success();
                if exists {
                    targets.insert((rev, name.clone()));
                    break;
                }
            }
        }
        assert!(!targets.is_empty(), "seed {seed}: {summary:?}");
        for (rev, path) in &targets {
            let cli = std::panic::catch_unwind(|| assert_cli_matches(&dir, tmp.path(), rev, path));
            assert!(
                cli.is_ok(),
                "seed {seed}: git engine differs at {rev}:{path}"
            );
            let gix = std::panic::catch_unwind(|| assert_gix_matches(&dir, tmp.path(), rev, path));
            assert!(
                gix.is_ok(),
                "seed {seed}: gix engine differs at {rev}:{path}"
            );
        }
    }
}

#[test]
fn a_file_missing_at_the_revision_or_an_unborn_head_has_nothing_to_blame() {
    let repo = TestRepo::new();
    assert!(
        subject(&open(repo.root()), None, "a.txt")
            .unwrap()
            .is_none()
    );
    repo.commit_file("a.txt", "one\n", "add");
    let r = open(repo.root());
    assert!(subject(&r, None, "missing.txt").unwrap().is_none());
    assert!(subject(&r, Some("HEAD"), "a.txt").unwrap().is_some());
}

#[test]
fn bad_revisions_paths_and_directories_are_invalid_params() {
    let repo = TestRepo::new();
    repo.commit_file("dir/a.txt", "one\n", "add");
    let r = open(repo.root());
    let code = |e: RpcError| e.code;
    let invalid = pw_rpc::code::INVALID_PARAMS;
    assert_eq!(
        subject(&r, Some("nope"), "dir/a.txt").map_err(code).err(),
        Some(invalid)
    );
    assert_eq!(
        subject(&r, Some("HEAD^{tree}"), "dir/a.txt")
            .map_err(code)
            .err(),
        Some(invalid)
    );
    assert_eq!(subject(&r, None, "dir").map_err(code).err(), Some(invalid));
    assert_eq!(subject(&r, None, "../x").map_err(code).err(), Some(invalid));
    assert_eq!(subject(&r, None, "/abs").map_err(code).err(), Some(invalid));
}

#[test]
fn lines_are_counted_like_git() {
    assert_eq!(count_lines(b""), 0);
    assert_eq!(count_lines(b"a"), 1);
    assert_eq!(count_lines(b"a\n"), 1);
    assert_eq!(count_lines(b"a\nb"), 2);
    assert_eq!(count_lines(b"\n\n"), 2);
}

#[test]
fn an_empty_file_sends_one_chunk_without_ranges() {
    let repo = TestRepo::new();
    repo.commit_file("empty.txt", "", "add");
    let (dir, ceiling) = (repo.root(), repo.base());
    for chunks in [
        blame_gix(dir, "HEAD", "empty.txt"),
        blame_cli(dir, ceiling, "HEAD", "empty.txt"),
    ] {
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].file.as_ref().unwrap().lines, 0);
        assert!(chunks[0].ranges.is_empty() && chunks[0].commits.is_empty());
    }
}

#[test]
fn the_gix_engine_stops_when_asked() {
    let repo = TestRepo::new();
    let fx = repo.blame_fixture();
    let r = open(repo.root());
    let subject = subject(&r, None, fx.path).unwrap().unwrap();
    let err = native::run(&r, false, &subject, &AtomicBool::new(true), &mut |_| true).unwrap_err();
    assert_eq!(err.code, pw_rpc::code::CANCELLED);
    // 받는 쪽이 사라지면 더 보내지 않는다.
    let mut sent = 0;
    let done = native::run(&r, false, &subject, &AtomicBool::new(false), &mut |_| {
        sent += 1;
        false
    })
    .unwrap();
    assert!(!done);
    assert_eq!(sent, 1);
}

#[test]
fn chunks_carry_at_most_chunk_ranges_and_the_first_one_goes_out_at_once() {
    let mut c = Chunker::new(BlameFile {
        commit: "c".into(),
        lines: 2000,
        engine: BlameEngine::Gix,
    });
    let range = |line| BlameRange {
        line,
        count: 1,
        commit: "c".into(),
        source_path: "p".into(),
        source_line: line,
        previous: None,
    };
    assert!(!c.due());
    c.range(range(1));
    assert!(c.due(), "the first range goes out at once");
    assert!(c.take().file.is_some());
    for k in 2..CHUNK as u32 + 1 {
        c.range(range(k));
    }
    assert!(!c.due() || c.wait() == Some(std::time::Duration::ZERO));
    c.range(range(CHUNK as u32 + 1));
    assert!(c.due());
    let chunk = c.take();
    assert_eq!(chunk.ranges.len(), CHUNK);
    assert!(chunk.file.is_none());
    assert!(c.finish().is_none());
}

/// 얕은 클론: 경계 커밋은 부모가 없는 것으로 보고 그 줄을 경계 커밋의 것으로 돌린다(git 과 같다).
#[test]
fn shallow_clones_blame_up_to_the_boundary_like_git() {
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
    let lines = assert_gix_matches(&shallow, repo.base(), "HEAD", fx.path);
    assert!(lines.iter().any(|l| l.previous.is_none()));
    let (_, _, metas) = expand(&blame_gix(&shallow, "HEAD", fx.path));
    assert!(metas.values().any(|m| m.boundary), "{metas:?}");
    assert_cli_matches(&shallow, repo.base(), "HEAD", fx.path);
}

/// 곧은 긴 이력(줄 덩어리 이동 · 이름 바뀜 둘)에서도 두 엔진이 git 과 같다 — 측정용 픽스처(`examples/blame_timing.rs`)의 작은 판.
#[test]
fn a_long_linear_history_blames_like_git() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("repo");
    crate::testing::blame::load(&dir, tmp.path(), 600, 240, 11).unwrap();
    assert_cli_matches(&dir, tmp.path(), "HEAD", "lib/big.txt");
    assert_gix_matches(&dir, tmp.path(), "HEAD", "lib/big.txt");
}

/// 멈춤 신호가 켜지면 gix-blame 에 주는 객체 읽기가 실패한다 — 큰 파일의 blame 도 다음 객체 읽기에서 끝난다.
#[test]
fn the_object_reader_fails_once_stopped() {
    use gix::objs::Find;

    let repo = TestRepo::new();
    let head = repo.commit_file("a.txt", "one\n", "add");
    let r = open(repo.root());
    let id = gix::ObjectId::from_hex(head.as_bytes()).unwrap();
    let (stop, shallow) = (AtomicBool::new(false), std::collections::HashSet::new());
    let objects = native::Objects {
        odb: &r.objects,
        stop: &stop,
        shallow: &shallow,
    };
    let mut buf = Vec::new();
    assert!(objects.try_find(&id, &mut buf).unwrap().is_some());
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    assert!(objects.try_find(&id, &mut buf).is_err());
}
