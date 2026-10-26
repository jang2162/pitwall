//! `git.status` 의 계산 시간(M2-02 완료 기준 ③): 합성 저장소에서 전체 status 와, 파일 하나를 바꾼 뒤 그 경로만 본 status 를 잰다.
//!
//! ```bash
//! cargo run --release -p pw-fs --example synth_repo -- <빈 폴더> 100000 1 --git
//! cargo run --release -p pw-git --example status_bench -- <그 폴더> [횟수, 기본 5]
//! ```
//!
//! 한 번마다 저장소를 새로 연다(`git.status` 가 부를 때마다 여는 것과 같다). 미신뢰로 연다 — 저장소 설정의 필터를 고르지 않는다.
//! 비교로 같은 작업 트리에서 `git status --porcelain=v2 -z --untracked-files=all` 도 잰다(사람의 설정을 읽지 않고 index 를 다시
//! 쓰지 않는다). 측정이 끝나면 바꾼 파일을 되돌린다.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use pw_git::status::compute;

fn main() {
    let mut args = std::env::args().skip(1);
    let root = PathBuf::from(args.next().expect("a repository made by synth_repo --git"));
    let runs: usize = args.next().map_or(5, |n| n.parse().expect("runs"));
    let open = || {
        pw_git::repo::discover(&root, false)
            .expect("open")
            .expect("a repository")
    };
    let entries = compute::index_entries(&open()).unwrap();
    println!("{}: {entries} index entries", root.display());

    let full = measure(runs, || {
        let repo = open();
        let got = compute::status(&repo, None, &Arc::new(AtomicBool::new(false)), None).unwrap();
        assert!(
            got.is_empty(),
            "the synthetic repository should be clean: {got:?}"
        );
    });
    report("full status (clean)", &full);
    let cli_full = measure(runs, || cli_status(&root, &[]));
    report("git status --porcelain=v2 -uall (clean)", &cli_full);

    // 파일 하나를 바꾼다(크기도 바뀌게 — 내용 비교 없이 stat 만으로 갈린다).
    let changed = first_file(&root);
    let rel = pw_git::status::paths::relative(&root, &changed).unwrap();
    let before = std::fs::read(&changed).unwrap();
    std::fs::write(&changed, b"changed for the bench\n").unwrap();
    let limited = |cache: Option<&compute::StagedCache>| {
        let repo = open();
        let got = compute::status(
            &repo,
            Some(std::slice::from_ref(&rel)),
            &Arc::new(AtomicBool::new(false)),
            cache.map(|c| (c, false)),
        )
        .unwrap();
        assert_eq!(got.len(), 1, "{got:?}");
    };
    report(
        &format!("path-limited status, no cache ({rel})"),
        &measure(runs, || limited(None)),
    );
    // `git.status` 는 HEAD↔index 결과를 담아 둔다 — 편집 뒤의 경로 한정 status 는 보통 이쪽이다(첫 번은 채우는 시간).
    let cache = compute::StagedCache::default();
    report(
        &format!("path-limited status, as git.status ({rel})"),
        &measure(runs, || limited(Some(&cache))),
    );
    let full_dirty = measure(runs, || {
        let repo = open();
        let got = compute::status(&repo, None, &Arc::new(AtomicBool::new(false)), None).unwrap();
        assert_eq!(got.len(), 1);
    });
    report("full status (one file changed)", &full_dirty);
    let cli_limited = measure(runs, || cli_status(&root, &[&rel]));
    report(&format!("git status -- {rel}"), &cli_limited);
    std::fs::write(&changed, before).unwrap();
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
    let ms = |d: &Duration| d.as_secs_f64() * 1000.0;
    println!(
        "{what}: first {:.1} ms, median {:.1} ms, min {:.1} ms, max {:.1} ms ({} runs)",
        ms(&times[0]),
        ms(&sorted[sorted.len() / 2]),
        ms(&sorted[0]),
        ms(sorted.last().unwrap()),
        times.len()
    );
}

fn cli_status(root: &Path, paths: &[&str]) {
    let null = if cfg!(windows) { "NUL" } else { "/dev/null" };
    let mut c = std::process::Command::new("git");
    c.current_dir(root)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", null)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .args(["status", "--porcelain=v2", "-z", "--untracked-files=all"]);
    if !paths.is_empty() {
        c.arg("--");
        c.args(paths.iter().map(|p| format!(":(top,literal){p}")));
    }
    let out = c.output().expect("run git");
    assert!(out.status.success());
}

/// 이름순으로 첫 추적 파일(`.gitignore` 말고).
fn first_file(root: &Path) -> PathBuf {
    let mut dir = root.join("apps");
    for top in ["apps", "crates", "docs", "packages", "vendor"] {
        if root.join(top).is_dir() {
            dir = root.join(top);
            break;
        }
    }
    loop {
        let mut entries: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        entries.sort();
        if let Some(file) = entries.iter().find(|p| p.is_file()) {
            return file.clone();
        }
        dir = entries.into_iter().find(|p| p.is_dir()).expect("a folder");
    }
}
