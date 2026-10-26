//! release 에서만 재는 수치. 기본 시험에서는 빠진다(`#[ignore]`):
//!
//! ```text
//! cargo nextest run --release -p pw-git --run-ignored only --no-capture -j 1 -E 'test(/in_release/)'
//! ```
//!
//! 1만 줄 바뀐 파일의 diff 는 M2-15 의 「1만 줄 바뀐 파일의 diff 첫 그림이 release 에서 200 ms 안」 중 Rust 몫이라 그 상한으로
//! 판정한다. 나머지는 상한 없이 수치만 찍는다.

use std::io::Write as _;
use std::process::Stdio;
use std::time::{Duration, Instant};

use super::*;

/// `runs` 번 재서 (가장 빠른 것, 중앙값).
fn measure(runs: usize, mut f: impl FnMut()) -> (Duration, Duration) {
    let mut times: Vec<Duration> = (0..runs)
        .map(|_| {
            let start = Instant::now();
            f();
            start.elapsed()
        })
        .collect();
    times.sort();
    (times[0], times[runs / 2])
}

#[test]
#[ignore = "release 수치(머리 주석의 명령)"]
fn diff_of_ten_thousand_changed_lines_in_release() {
    let r = TestRepo::with_commit();
    let before: String = (0..20_000)
        .map(|i| format!("    let value_{i} = {i};\n"))
        .collect();
    let after: String = (0..20_000)
        .map(|i| {
            if i % 2 == 0 {
                format!("    let value_{i} = {i} + 1;\n")
            } else {
                format!("    let value_{i} = {i};\n")
            }
        })
        .collect();
    r.commit_file("big.rs", &before, "before");
    r.commit_file("big.rs", &after, "after");
    let repo = Repo::new(r);
    let mut removed = 0;
    let (best, median) = measure(7, || {
        let diff = repo.diff(None, commit("HEAD"));
        removed = diff.files[0].removed;
    });
    assert_eq!(removed, 10_000);
    println!("git.diff, 10,000 changed lines in one file: best {best:?}, median {median:?}");
    assert!(median < Duration::from_millis(200), "median {median:?}");
}

#[test]
#[ignore = "release 수치(머리 주석의 명령)"]
fn diff_of_a_thousand_files_in_release() {
    let r = TestRepo::with_commit();
    for i in 0..1000 {
        r.write(
            &format!("dir{}/f{i}.txt", i % 20),
            &format!("a\nb\nc {i}\nd\ne\n"),
        );
    }
    r.commit_all("before");
    for i in 0..1000 {
        r.write(
            &format!("dir{}/f{i}.txt", i % 20),
            &format!("a\nb\nC {i}\nd\ne\n"),
        );
    }
    r.commit_all("after");
    let repo = Repo::new(r);
    let (best, median) = measure(7, || {
        assert_eq!(repo.diff(None, commit("HEAD")).files.len(), 1000);
    });
    println!("git.diff, 1,000 changed files: best {best:?}, median {median:?}");
    // 작업 트리 diff: 1천 파일 중 하나만 바뀌었다(나머지는 stat 으로 거른다).
    repo.r.write("dir0/f0.txt", "changed\n");
    let (best, median) = measure(7, || {
        assert_eq!(repo.diff(None, DiffSide::WorkTree).files.len(), 1);
    });
    println!(
        "git.diff index↔workTree, 1,001 tracked files, 1 changed: best {best:?}, median {median:?}"
    );
}

#[test]
#[ignore = "release 수치(머리 주석의 명령)"]
fn contained_in_over_ten_thousand_commits_in_release() {
    const COMMITS: usize = 10_000;
    let r = TestRepo::new();
    // `git fast-import` 로 한 줄 이력 1만 커밋과, 그 위 여러 자리의 브랜치 20개 · 태그 20개를 만든다.
    let mut stream = String::new();
    for i in 1..=COMMITS {
        let msg = format!("c{i}\n");
        let body = format!("{i}\n");
        stream.push_str(&format!(
            "commit refs/heads/main\nmark :{i}\ncommitter T <t@t> {} +0000\ndata {}\n{msg}",
            1_700_000_000 + i,
            msg.len()
        ));
        if i > 1 {
            stream.push_str(&format!("from :{}\n", i - 1));
        }
        stream.push_str(&format!(
            "M 100644 inline f.txt\ndata {}\n{body}\n",
            body.len()
        ));
    }
    for k in 0..20 {
        let at = COMMITS - k * 400;
        stream.push_str(&format!("reset refs/heads/b{k}\nfrom :{at}\n\n"));
        stream.push_str(&format!("reset refs/tags/t{k}\nfrom :{}\n\n", at - 200));
    }
    let mut child = r
        .command_in(r.root())
        .args(["fast-import", "--quiet"])
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stream.as_bytes())
        .unwrap();
    assert!(child.wait().unwrap().success());
    r.git(&["checkout", "-q", "main"]);
    let repo = Repo::new(r);
    let first = repo.r.git(&["rev-list", "--max-parents=0", "HEAD"]);
    let first = first.trim().to_owned();
    let (best, median) = measure(5, || {
        let got = repo.f.git.contained_in(&repo.rev(&first)).unwrap();
        assert_eq!(got.refs.len(), 41);
    });
    println!("git.containedIn, root of 10,000 commits, 41 refs: best {best:?}, median {median:?}");
}

/// 이 프로세스의 최대 RSS(MiB).
#[cfg(unix)]
fn max_rss_mib() -> f64 {
    // SAFETY: getrusage 는 넘긴 구조체만 채운다.
    let usage = unsafe {
        let mut usage: libc::rusage = std::mem::zeroed();
        libc::getrusage(libc::RUSAGE_SELF, &mut usage);
        usage
    };
    // macOS 는 바이트, Linux 는 KiB 다.
    let bytes = if cfg!(target_os = "macos") {
        usage.ru_maxrss as f64
    } else {
        usage.ru_maxrss as f64 * 1024.0
    };
    bytes / 1024.0 / 1024.0
}

/// 38 MiB 파일의 줄 200만 개가 모두 바뀐다(M2-04 리뷰의 재현): hunk 가 파일 하나의 상한을 넘어 `tooLarge` 이고, 줄을 만들지 않으므로
/// 응답 · 메모리가 작다.
#[cfg(unix)]
#[test]
#[ignore = "release 수치(머리 주석의 명령)"]
fn diff_of_a_38_mib_file_with_every_line_changed_in_release() {
    let r = TestRepo::with_commit();
    let n = 2_000_000;
    r.commit_file(
        "big.txt",
        &(0..n).map(|i| format!("a{i:018}\n")).collect::<String>(),
        "a",
    );
    r.commit_file(
        "big.txt",
        &(0..n).map(|i| format!("b{i:018}\n")).collect::<String>(),
        "b",
    );
    let repo = Repo::new(r);
    let before = max_rss_mib();
    let start = Instant::now();
    let diff = repo.diff(None, commit("HEAD"));
    let took = start.elapsed();
    let json = serde_json::to_vec(&diff).unwrap().len();
    let after = max_rss_mib();
    let f = &diff.files[0];
    assert!(f.too_large && f.hunks.is_empty() && f.added == n && f.removed == n);
    println!(
        "git.diff, 38 MiB file with 2,000,000 lines changed: {took:?}, JSON {json} bytes, max RSS {before:.0} -> {after:.0} MiB"
    );
    assert!(json < 4096, "{json}");
}

/// 40 MiB 한 줄 파일 두 쪽의 diff 를 8개 동시에(M2-04 리뷰의 재현): `git.blob` 과 같은 동시 상한(2개)을 거친다.
#[cfg(unix)]
#[test]
#[ignore = "release 수치(머리 주석의 명령)"]
fn eight_concurrent_diffs_of_forty_mib_files_in_release() {
    let r = TestRepo::with_commit();
    r.commit_file("big.txt", &("x".repeat(40 * 1024 * 1024) + "\n"), "a");
    r.commit_file("big.txt", &("y".repeat(40 * 1024 * 1024) + "\n"), "b");
    let repo = std::sync::Arc::new(Repo::new(r));
    let before = max_rss_mib();
    let start = Instant::now();
    let threads: Vec<_> = (0..8)
        .map(|_| {
            let repo = std::sync::Arc::clone(&repo);
            std::thread::spawn(move || repo.diff(None, commit("HEAD")).files.remove(0))
        })
        .collect();
    let files: Vec<DiffFile> = threads.into_iter().map(|t| t.join().unwrap()).collect();
    let took = start.elapsed();
    let after = max_rss_mib();
    let busy = files.iter().filter(|f| f.error.is_some()).count();
    println!(
        "git.diff x8 at once, 2 x 40 MiB each: {took:?}, {busy} busy, max RSS {before:.0} -> {after:.0} MiB"
    );
    assert!(files.iter().all(|f| f.too_large || f.error.is_some()));
}
