//! 10만 커밋 합성 저장소에서 `git.log` 첫 청크(200행 + 레인)가 commit-graph 없음 · 있음 각각 300 ms 안(`docs/design/13-m2-plan.md`
//! M2-03 — 06 「성능 목표」 의 500 ms 중 Rust 몫). 저장소를 만드는 데 20초쯤 걸려 기본 시험에서 빼고 release 에서만 판정한다:
//!
//! ```text
//! cargo nextest run --release -p pw-git --run-ignored only -E 'test(=log::tests::perf::the_first_chunk_of_100k_commits_is_within_300ms)' --no-capture
//! ```
//!
//! 잰 것: 저장소 열기(RPC 와 같은 미신뢰 열기)부터 첫 청크를 내보낼 때까지. 다섯 번 재서 가운데 값으로 판정한다(다른 프로세스의
//! 부하로 튀는 한 번을 거른다). 다섯 값은 모두 출력한다. 전체 배치 시간 · 최고 메모리는 `examples/log_timing.rs` 로 잰다.

use std::time::{Duration, Instant};

use super::*;

const COMMITS: usize = 100_000;
const BUDGET: Duration = Duration::from_millis(300);
const RUNS: usize = 5;

fn first_chunk(dir: &Path) -> Duration {
    let query = Query::new(None, &LogFilter::default(), LogOrder::Topo).unwrap();
    let started = Instant::now();
    let repo = open(dir);
    let mut first = None;
    run(
        &repo,
        &query,
        Window::default(),
        &AtomicBool::new(false),
        &mut |chunk| {
            assert_eq!(chunk.rows.len(), FIRST_CHUNK);
            assert!(chunk.rows.iter().all(|r| r.graph.is_some()));
            first = Some(started.elapsed());
            false
        },
    )
    .unwrap();
    first.unwrap()
}

#[test]
#[ignore = "release timing: builds a 100k-commit repository"]
fn the_first_chunk_of_100k_commits_is_within_300ms() {
    let s = Synth::new(COMMITS);
    for graph in [false, true] {
        if graph {
            s.write_commit_graph();
        }
        let mut times: Vec<Duration> = (0..RUNS).map(|_| first_chunk(&s.dir)).collect();
        eprintln!("commit-graph {graph}: first chunk {times:?}");
        times.sort();
        let median = times[RUNS / 2];
        if cfg!(debug_assertions) {
            eprintln!("debug build: not judged");
            continue;
        }
        assert!(
            median < BUDGET,
            "commit-graph {graph}: median first chunk {median:?} (runs {times:?})"
        );
    }
}
