//! `search.files` 를 잰다(M1-12 완료 기준: 합성 10만 파일 저장소에서 목록 빌드 시간, 키 입력마다의 상위 100 p95 < 30 ms, 만들거나
//! 지운 파일이 500 ms 안에 결과에 반영).
//!
//! ```bash
//! cargo run --release -p pw-fs --example synth_repo -- <폴더>
//! cargo run --release -p pw-fs --example search_files_bench -- <폴더> [회수, 기본 5] [질의 …]
//! ```
//!
//! 임시 `$DATA` 에 `<폴더>` 를 프로젝트로 등록하고, 앱과 같이 감시(`fs.watch` 의 임대)에 인덱스를 붙인다. 임대를 건 때부터 목록이
//! 다 만들어질 때까지를 빌드 시간으로 적는다. 질의마다 앞 1 글자 · 2 글자 · … 를 차례로 RPC 세션으로 보내(키 입력 하나 = 요청 하나,
//! `limit` 100) 요청을 넣은 때부터 응답까지를 모으고, 회수만큼 되풀이해 p50 · p95 · 최대를 낸다. 마지막으로 깊은 폴더에 파일을 만들고
//! 지우며 결과에 반영될 때까지(5 ms 간격으로 다시 찾는다)를 잰다 — 만든 파일은 지운다.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use pw_core::TaskSpawner;
use pw_core::bus::{DEFAULT_CAPACITY, EventBus, Scope};
use pw_core::config::AppPaths;
use pw_core::registry::{NewProject, Registry};
use pw_fs::Fs;
use pw_fs::index::FileIndex;
use pw_fs::watch::{FsWatchParams, Watch};
use pw_rpc::{Caller, EventHub, Role, Router, Session};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

/// 기본 질의: 합성 저장소의 낱말 · 확장자 · 경로 조각.
const QUERIES: [&str; 8] = [
    "handler",
    "config.ts",
    "src/router",
    "parser_lexer",
    "editor buffer",
    "pkgsrvc",
    "tokenrs",
    "watch-3/src/event",
];

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1e3
}

fn percentile(sorted: &[Duration], p: f64) -> Duration {
    let i = ((sorted.len() as f64 * p).ceil() as usize).clamp(1, sorted.len()) - 1;
    sorted[i]
}

struct Rpc {
    session: Session,
    rx: tokio::sync::mpsc::UnboundedReceiver<pw_rpc::Outbound>,
    next: u64,
}

impl Rpc {
    async fn search(&mut self, project: &str, query: &str) -> Value {
        self.next += 1;
        self.session.handle_value(
            json!({"jsonrpc": "2.0", "id": self.next, "method": "search.files",
            "params": {"projectId": project, "query": query, "limit": 100}}),
        );
        let response = self.rx.recv().await.unwrap().into_response().unwrap();
        response.result_as().unwrap()
    }

    async fn has(&mut self, project: &str, query: &str, relative: &str) -> bool {
        let found = self.search(project, query).await;
        found["files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["relative"] == relative)
    }
}

async fn reflected(
    rpc: &mut Rpc,
    project: &str,
    query: &str,
    relative: &str,
    present: bool,
) -> Duration {
    let started = Instant::now();
    while rpc.has(project, query, relative).await != present {
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "{relative} not reflected"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    started.elapsed()
}

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let mut args = std::env::args().skip(1);
    let dir = std::fs::canonicalize(args.next().expect("folder")).expect("folder exists");
    let runs: usize = args.next().map_or(5, |n| n.parse().expect("runs"));
    let queries: Vec<String> = {
        let given: Vec<String> = args.collect();
        if given.is_empty() {
            QUERIES.iter().map(|q| (*q).to_owned()).collect()
        } else {
            given
        }
    };

    let tmp = tempfile::tempdir().unwrap();
    let bus = EventBus::new(DEFAULT_CAPACITY);
    let projects = Registry::new(AppPaths::under(tmp.path()), None, bus.clone());
    let project = projects
        .create(NewProject {
            root: dir.clone(),
            ..NewProject::default()
        })
        .unwrap();
    let fs = Arc::new(Fs::new());
    fs.attach(projects);
    let index = FileIndex::new(Arc::clone(&fs));
    tokio::spawn(index.attach(
        &bus,
        TaskSpawner::new(bus.clone(), Scope::Host, CancellationToken::new()),
    ));
    let watch = Watch::new(Arc::clone(&fs));
    watch.attach(bus.clone());
    watch.attach_index(index.clone());
    let mut router = Router::new();
    pw_fs::index::register(&mut router, index.clone());
    let (session, rx) = Session::new(Arc::new(router), EventHub::new(), Caller::new(Role::Owner));
    let mut rpc = Rpc {
        session,
        rx,
        next: 0,
    };

    let started = Instant::now();
    let params = FsWatchParams {
        project_id: project.id.clone(),
        worktree: None,
    };
    let lease = {
        let watch = watch.clone();
        tokio::task::spawn_blocking(move || watch.acquire(&params))
            .await
            .unwrap()
            .unwrap()
    };
    let watched = started.elapsed();
    assert!(index.ready(&project.id, project.root()).await);
    let built = started.elapsed();
    let all = rpc.search(&project.id, "").await;
    println!(
        "build: {} files, watch {:.0} ms, list ready {:.0} ms after the watch started",
        all["indexed"],
        ms(watched),
        ms(built - watched)
    );

    let mut times = Vec::new();
    let mut per_query: Vec<(String, Vec<Duration>, Value)> = Vec::new();
    for _ in 0..runs {
        for query in &queries {
            let chars: Vec<char> = query.chars().collect();
            let mut mine = Vec::new();
            let mut last = Value::Null;
            for n in 1..=chars.len() {
                let typed: String = chars[..n].iter().collect();
                let at = Instant::now();
                last = rpc.search(&project.id, &typed).await;
                let took = at.elapsed();
                times.push(took);
                mine.push(took);
            }
            match per_query.iter_mut().find(|(q, _, _)| q == query) {
                Some((_, all, _)) => all.extend(mine),
                None => per_query.push((query.clone(), mine, last)),
            }
        }
    }
    for (query, mut took, last) in per_query {
        took.sort();
        println!(
            "{query:>20}: {} keystrokes, p50 {:.2} ms, p95 {:.2} ms, max {:.2} ms, final matched {}, top {}",
            took.len(),
            ms(percentile(&took, 0.5)),
            ms(percentile(&took, 0.95)),
            ms(*took.last().unwrap()),
            last["matched"],
            last["files"][0]["relative"]
        );
    }
    times.sort();
    println!(
        "all: {} keystrokes, p50 {:.2} ms, p95 {:.2} ms, max {:.2} ms",
        times.len(),
        ms(percentile(&times, 0.5)),
        ms(percentile(&times, 0.95)),
        ms(*times.last().unwrap())
    );

    // 반영: 가장 깊은 폴더 하나에 파일을 만들고 지운다.
    let deep = all["files"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|f| f["relative"].as_str())
        .max_by_key(|r| r.matches('/').count())
        .expect("a file");
    let deep_dir = Path::new(deep)
        .parent()
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    let mut created = Vec::new();
    let mut removed = Vec::new();
    for i in 0..5 {
        let relative = format!("{deep_dir}/zz_probe_{i}.rs");
        let path = dir.join(&relative);
        std::fs::write(&path, b"").unwrap();
        created.push(
            reflected(
                &mut rpc,
                &project.id,
                &format!("zz_probe_{i}"),
                &relative,
                true,
            )
            .await,
        );
        std::fs::remove_file(&path).unwrap();
        removed.push(
            reflected(
                &mut rpc,
                &project.id,
                &format!("zz_probe_{i}"),
                &relative,
                false,
            )
            .await,
        );
    }
    println!(
        "reflected: created max {:.0} ms ({:?}), removed max {:.0} ms ({:?})",
        ms(*created.iter().max().unwrap()),
        created.iter().map(|d| ms(*d).round()).collect::<Vec<_>>(),
        ms(*removed.iter().max().unwrap()),
        removed.iter().map(|d| ms(*d).round()).collect::<Vec<_>>(),
    );
    drop(lease);
}
