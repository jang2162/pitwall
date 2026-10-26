//! 합성 저장소(`examples/synth_history.rs`)에서 `git.log` 의 Rust 몫을 잰다(`docs/design/13-m2-plan.md` M2-03 — 첫 청크 300 ms).
//!
//! ```text
//! cargo run --release -p pw-git --example log_timing -- <저장소> [반복=5] [topo|date]
//! /usr/bin/time -l target/release/examples/log_timing <저장소> 1      # 최고 메모리(maximum resident set size)
//! ```
//!
//! 저장소는 RPC 와 같은 경로로 연다(미신뢰 — `pw_git::repo::discover`). 잰 것: 저장소 열기부터 첫 청크(200행 + 레인)를 내보낼
//! 때까지, 마지막 청크까지(전체 배치 · 메시지 읽기). 반복마다 저장소를 새로 연다(객체 캐시가 이어지지 않는다). OS 의 파일 캐시는
//! 데워진 채다 — 첫 반복이 가장 차갑다.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use pw_git::log::{LogFilter, LogOrder, Query, Window, run};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(dir) = args.first().map(PathBuf::from) else {
        eprintln!("usage: log_timing <repo> [runs=5] [topo|date]");
        std::process::exit(2);
    };
    let runs: usize = args.get(1).map_or(5, |n| n.parse().expect("run count"));
    let order = match args.get(2).map(String::as_str) {
        None | Some("topo") => LogOrder::Topo,
        Some("date") => LogOrder::Date,
        Some(other) => panic!("unknown order `{other}`"),
    };
    let query = Query::new(None, &LogFilter::default(), order).expect("a query");
    let mut firsts = Vec::new();
    for n in 1..=runs {
        let started = Instant::now();
        let repo = pw_git::repo::discover(&dir, false)
            .expect("open the repository")
            .expect("a repository");
        let graph = repo.commit_graph_if_enabled().ok().flatten().is_some();
        let mut first = None;
        let mut rows = 0usize;
        let mut lanes = 0u32;
        let mut total = 0u32;
        run(
            &repo,
            &query,
            Window::default(),
            &AtomicBool::new(false),
            &mut |chunk| {
                if first.is_none() {
                    first = Some(started.elapsed());
                    total = chunk.total.unwrap_or(0);
                }
                for row in &chunk.rows {
                    if let Some(g) = &row.graph {
                        let widest = g
                            .pass
                            .iter()
                            .chain(&g.up)
                            .chain(&g.down)
                            .map(|e| e.0)
                            .max()
                            .unwrap_or(0);
                        lanes = lanes.max(widest.max(g.lane) + 1);
                    }
                }
                rows += chunk.rows.len();
                true
            },
        )
        .expect("the log");
        let all = started.elapsed();
        let first = first.expect("a first chunk");
        firsts.push(first);
        println!(
            "run {n}: commit-graph {graph}, first chunk {:.1} ms, all {rows}/{total} rows {:.1} ms, widest {lanes} lanes",
            first.as_secs_f64() * 1000.0,
            all.as_secs_f64() * 1000.0
        );
    }
    firsts.sort();
    println!(
        "first chunk: median {:.1} ms, max {:.1} ms",
        firsts[firsts.len() / 2].as_secs_f64() * 1000.0,
        firsts[firsts.len() - 1].as_secs_f64() * 1000.0
    );
}
