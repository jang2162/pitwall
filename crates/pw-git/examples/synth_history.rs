//! 합성 이력 저장소를 만든다(`docs/design/13-m2-plan.md` M2-03 — 10만 커밋 저장소의 판정 기준). 시드가 같으면 같은 저장소다.
//!
//! ```text
//! cargo run --release -p pw-git --features test-support --example synth_history -- <폴더> [커밋 수=100000] [시드] [--commit-graph]
//! cargo run --release -p pw-git --features test-support --example synth_history -- <폴더> 20000 --fanout 3000
//! cargo run --release -p pw-git --features test-support --example synth_history -- <폴더> 100000 --perf-fixture [--commit-graph]
//! ```
//!
//! `<폴더>` 는 비어 있거나 없어야 한다. 위로 찾기는 `<폴더>` 의 부모에서 멈춘다. `--commit-graph` 면 `git commit-graph write
//! --reachable` 까지 한다(없음 · 있음 둘 다 재려면 폴더 두 개를 만든다). `--fanout <n>` 이면 브랜치 · 머지가 섞인 이력 대신 곧은
//! `main` 곳곳에서 갈라진 원격 브랜치 `n` 개를 만든다(`testing::synth::fanout` — 레인 상한). `--perf-fixture` 면 실기 측정이 쓰는
//! 1만 줄 diff 커밋 · 고칠 파일 1만 개 · 타이핑 대상 파일과 체크아웃용 브랜치를 commit-graph 보다 먼저 더한다
//! (`testing::synth::perf_fixture`, `mise run perf-git`).

use std::path::PathBuf;
use std::time::Instant;

use pw_git::testing::synth;

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let graph = args.iter().any(|a| a == "--commit-graph");
    let perf = args.iter().any(|a| a == "--perf-fixture");
    args.retain(|a| a != "--commit-graph" && a != "--perf-fixture");
    let fanout = args.iter().position(|a| a == "--fanout").map(|i| {
        let n: usize = args
            .get(i + 1)
            .and_then(|n| n.parse().ok())
            .expect("--fanout <branches>");
        args.drain(i..=i + 1);
        n
    });
    let Some(dir) = args.first().map(PathBuf::from) else {
        eprintln!(
            "usage: synth_history <dir> [commits=100000] [seed] [--commit-graph] [--perf-fixture]"
        );
        std::process::exit(2);
    };
    let commits = args
        .get(1)
        .map_or(100_000, |n| n.parse().expect("commit count"));
    let seed = args.get(2).map_or(synth::SEED, |n| {
        let n = n.trim_start_matches("0x");
        u64::from_str_radix(n, 16).expect("seed in hex")
    });
    if dir.exists()
        && std::fs::read_dir(&dir)
            .expect("read the folder")
            .next()
            .is_some()
    {
        eprintln!("{} is not empty", dir.display());
        std::process::exit(2);
    }
    std::fs::create_dir_all(&dir).expect("create the folder");
    let dir = std::fs::canonicalize(&dir).expect("resolve the folder");
    let ceiling = dir.parent().expect("a parent folder").to_path_buf();
    let started = Instant::now();
    let summary = match fanout {
        Some(branches) => {
            synth::fanout(&dir, &ceiling, commits, branches, seed).expect("create the repository");
            format!("fanout {commits} commits, {branches} remote branches")
        }
        None => format!(
            "{:?}",
            synth::create(&dir, &ceiling, commits, seed).expect("create the repository")
        ),
    };
    let summary = if perf {
        let fixture = synth::perf_fixture(&dir, &ceiling).expect("add the perf fixture");
        format!("{summary}, perf fixture {}", fixture.commit)
    } else {
        summary
    };
    if graph {
        synth::write_commit_graph(&dir, &ceiling).expect("write the commit-graph");
    }
    println!(
        "{}: {summary}, seed {seed:#x}, commit-graph {graph}, {:.1}s",
        dir.display(),
        started.elapsed().as_secs_f64()
    );
}
