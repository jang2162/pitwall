//! `git.refs` 의 ahead/behind 계산 시간: 긴 일직선 이력(커밋 `<수>` 개) 끝에서 갈라진 두 브랜치(`main` 이 `up` 을 upstream 으로
//! 둔다)를 commit-graph 없이, 그리고 `git commit-graph write` 뒤에 잰다. commit-graph 가 없으면 세대 번호를 직접 세느라 이력 전체를
//! 한 번 읽는다(`pw_git::status::refs::Counter`). 앱은 센 세대 번호를 다음 호출에 넘긴다(`Generations`) — 그것을 쓴 것과 안 쓴
//! 것을 함께 잰다.
//!
//! ```bash
//! cargo run --release -p pw-git --example refs_bench -- <빈 폴더> [커밋 수, 기본 100000] [횟수, 기본 5]
//! ```
//!
//! 이력은 `git fast-import` 로 만든다(사람의 설정을 읽지 않는다). 비교로 `git rev-list --left-right --count main...up` 도 잰다.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn main() {
    let mut args = std::env::args().skip(1);
    let root = std::path::PathBuf::from(args.next().expect("an empty folder"));
    let commits: usize = args.next().map_or(100_000, |n| n.parse().expect("commits"));
    let runs: usize = args.next().map_or(5, |n| n.parse().expect("runs"));
    make(&root, commits);
    let open = || {
        pw_git::repo::discover(&root, false)
            .expect("open")
            .expect("a repository")
    };
    let want = String::from_utf8(
        git(&root)
            .args(["rev-list", "--left-right", "--count", "main...up"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    println!(
        "{}: {commits} commits, git rev-list: {}",
        root.display(),
        want.trim()
    );
    for label in ["without a commit-graph", "with a commit-graph"] {
        if label.starts_with("with ") {
            assert!(
                git(&root)
                    .args(["commit-graph", "write", "--reachable"])
                    .status()
                    .unwrap()
                    .success()
            );
        }
        let check = |generations: Option<&pw_git::status::refs::Generations>| {
            let refs = pw_git::status::refs::refs(&open(), generations).unwrap();
            let main = refs.branches.iter().find(|b| b.name == "main").unwrap();
            let track = main.upstream.as_ref().unwrap().track.unwrap();
            assert_eq!(format!("{}\t{}", track.ahead, track.behind), want.trim());
        };
        report(
            &format!("git.refs {label}, nothing kept"),
            &measure(runs, || check(None)),
        );
        // 앱처럼 센 세대 번호를 다음 호출에 넘긴다 — 첫 회가 채운다.
        let generations = pw_git::status::refs::Generations::default();
        report(
            &format!("git.refs {label}, generations kept"),
            &measure(runs, || check(Some(&generations))),
        );
        let cli = measure(runs, || {
            assert!(
                git(&root)
                    .args(["rev-list", "--left-right", "--count", "main...up"])
                    .output()
                    .unwrap()
                    .status
                    .success()
            );
        });
        report(&format!("git rev-list {label}"), &cli);
    }
}

/// `root` 에 커밋 `commits` 개의 일직선 이력, 그 끝에서 `main` 2개 · `up` 3개를 더 만든다.
fn make(root: &Path, commits: usize) {
    std::fs::create_dir_all(root).unwrap();
    assert!(
        git(root)
            .args(["init", "--quiet", "-b", "main"])
            .status()
            .unwrap()
            .success()
    );
    let mut child = git(root)
        .args(["fast-import", "--quiet"])
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = std::io::BufWriter::new(child.stdin.take().unwrap());
    let mut mark = 0;
    let mut commit = |input: &mut dyn Write, branch: &str, from: Option<usize>| -> usize {
        mark += 1;
        let at = 1_700_000_000 + mark;
        write!(
            input,
            "commit refs/heads/{branch}\nmark :{mark}\ncommitter Bench <bench@pitwall.invalid> {at} +0000\ndata 0\n"
        )
        .unwrap();
        if let Some(from) = from {
            writeln!(input, "from :{from}").unwrap();
        }
        writeln!(input).unwrap();
        mark
    };
    let mut tip = commit(&mut input, "main", None);
    for _ in 1..commits {
        tip = commit(&mut input, "main", Some(tip));
    }
    let fork = tip;
    let mut up = fork;
    for _ in 0..3 {
        up = commit(&mut input, "up", Some(up));
    }
    let mut main = fork;
    for _ in 0..2 {
        main = commit(&mut input, "main", Some(main));
    }
    drop(input);
    assert!(child.wait().unwrap().success());
    for (key, value) in [
        ("branch.main.remote", "."),
        ("branch.main.merge", "refs/heads/up"),
    ] {
        assert!(
            git(root)
                .args(["config", key, value])
                .status()
                .unwrap()
                .success()
        );
    }
}

/// 사람의 설정을 읽지 않는 git.
fn git(root: &Path) -> Command {
    let mut c = Command::new("git");
    c.current_dir(root)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env(
            "GIT_CONFIG_GLOBAL",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
        )
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_CEILING_DIRECTORIES", root.parent().unwrap_or(root));
    c
}

fn measure(runs: usize, mut f: impl FnMut()) -> Vec<Duration> {
    (0..runs)
        .map(|_| {
            let started = Instant::now();
            f();
            started.elapsed()
        })
        .collect()
}

fn report(what: &str, times: &[Duration]) {
    let mut sorted = times.to_vec();
    sorted.sort();
    println!(
        "{what}: first {:?}, median {:?}, min {:?}",
        times[0],
        sorted[sorted.len() / 2],
        sorted[0]
    );
}
