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
