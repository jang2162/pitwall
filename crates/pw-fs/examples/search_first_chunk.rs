//! `search.text` 의 첫 청크까지 걸린 시간을 잰다(M1-07 완료 기준: 이 저장소에서 `fn ` 첫 청크가 release 에서 200 ms 안).
//!
//! ```bash
//! cargo run --release -p pw-fs --example search_first_chunk -- <폴더> 'fn ' [회수] ['{"excludeVendored": true}']
//! ```
//!
//! 네 번째 인자는 요청에 더할 옵션(JSON 객체)이다.
//!
//! 임시 `$DATA` 에 `<폴더>` 를 프로젝트로 등록하고 RPC 세션으로 스트림을 연다(credit 을 먼저 준다). 요청을 넣은 때부터 첫
//! `stream.chunk` · `stream.end` 까지를 회마다 적고 중앙값을 낸다. ignore 존중 · 대소문자 무시(기본값)다.

use std::sync::Arc;
use std::time::{Duration, Instant};

use pw_core::bus::{DEFAULT_CAPACITY, EventBus};
use pw_core::config::AppPaths;
use pw_core::registry::{NewProject, Registry};
use pw_fs::Fs;
use pw_rpc::protocol::{STREAM_CHUNK, STREAM_END, StreamOpened};
use pw_rpc::{Caller, EventHub, Role, Router, Session};
use serde_json::json;

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let mut args = std::env::args().skip(1);
    let dir = std::fs::canonicalize(args.next().expect("folder")).expect("folder exists");
    let query = args.next().unwrap_or_else(|| "fn ".to_owned());
    let runs: usize = args.next().map_or(5, |n| n.parse().expect("runs"));
    let extra: serde_json::Value = args
        .next()
        .map_or_else(|| json!({}), |o| serde_json::from_str(&o).expect("options"));

    let tmp = tempfile::tempdir().unwrap();
    let projects = Registry::new(
        AppPaths::under(tmp.path()),
        None,
        EventBus::new(DEFAULT_CAPACITY),
    );
    let project = projects
        .create(NewProject {
            root: dir.clone(),
            ..NewProject::default()
        })
        .unwrap();
    let fs = Arc::new(Fs::new());
    fs.attach(projects);
    let mut router = Router::new();
    pw_fs::methods::register(&mut router, fs);
    let router = Arc::new(router);

    let mut firsts = Vec::new();
    for run in 0..runs {
        let (session, mut rx) = Session::new(
            Arc::clone(&router),
            EventHub::new(),
            Caller::new(Role::Owner),
        );
        let started = Instant::now();
        let mut params = json!({
            "projectId": project.id, "paths": [project.root().display().to_string()], "query": query,
        });
        params
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().expect("an object").clone());
        session.handle_value(
            json!({"jsonrpc": "2.0", "id": 1, "method": "search.text", "params": params}),
        );
        let opened: StreamOpened = rx
            .recv()
            .await
            .unwrap()
            .into_response()
            .unwrap()
            .result_as()
            .unwrap();
        session.handle_value(json!({"jsonrpc": "2.0", "method": "stream.credit",
            "params": {"streamId": opened.stream_id, "credit": 1_000_000}}));
        let mut first: Option<Duration> = None;
        let mut last = serde_json::Value::Null;
        loop {
            let note = rx.recv().await.unwrap().into_notification().unwrap();
            if note.method == STREAM_CHUNK {
                first.get_or_insert_with(|| started.elapsed());
                last = note.params;
            } else if note.method == STREAM_END {
                break;
            }
        }
        let first = first.expect("a chunk");
        println!(
            "run {run}: first chunk {:.1} ms, end {:.1} ms, done {}",
            first.as_secs_f64() * 1e3,
            started.elapsed().as_secs_f64() * 1e3,
            last["data"]
        );
        firsts.push(first);
    }
    firsts.sort();
    println!(
        "median first chunk: {:.1} ms ({} runs)",
        firsts[firsts.len() / 2].as_secs_f64() * 1e3,
        runs
    );
}
