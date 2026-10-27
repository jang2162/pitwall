//! RPC: `git.log` 스트림(청크 크기 · `total` · 끝)과 `git.logPage`(커서로 이어 받은 행 == 스트림의 행), 거절 · 저장소 없음 ·
//! 멈춤.

use std::sync::Arc;
use std::time::Duration;

use pw_core::bus::{DEFAULT_CAPACITY, EventBus, Scope};
use pw_core::config::{AppPaths, ConfigSource, ConfigStore};
use pw_core::registry::{NewProject, Registry as Projects};
use pw_rpc::protocol::{
    STREAM_CHUNK, STREAM_END, StreamChunk, StreamEnd, StreamOpened, StreamOutcome,
};
use pw_rpc::{Caller, EventHub, Outbound, Role, Router, RpcError, Session};
use pw_runtime::{LoginShell, Runtime};
use serde_json::json;

use super::*;
use crate::log::methods::{GitLogPage, GitLogPageParams, GitLogParams, register};
use crate::{Git, HostParts};

/// 레지스트리에 붙은 `Git`(로그인 셸 없이 — 로그는 git CLI 를 부르지 않는다).
pub(super) struct Fx {
    _tmp: tempfile::TempDir,
    pub git: Arc<Git>,
    pub projects: Projects,
}

impl Fx {
    pub fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::under(&tmp.path().join("app"));
        std::fs::create_dir_all(&paths.config).unwrap();
        std::fs::write(paths.config.join("settings.toml"), "").unwrap();
        let bus = EventBus::new(DEFAULT_CAPACITY);
        let global = ConfigStore::open(ConfigSource::global(&paths), bus.clone(), Scope::Host);
        let projects = Projects::new(paths, None, bus.clone());
        let gate = projects.clone();
        let runtime = Runtime::new(LoginShell::new(None, Duration::from_secs(5)))
            .with_trust(move |id| gate.is_trusted(id));
        let git = Arc::new(Git::new(Arc::new(runtime)));
        git.attach(HostParts {
            global,
            projects: projects.clone(),
            bus,
        });
        Self {
            _tmp: tmp,
            git,
            projects,
        }
    }

    pub fn project(&self, root: &Path) -> String {
        self.projects
            .create(NewProject {
                root: root.to_path_buf(),
                ..NewProject::default()
            })
            .unwrap()
            .id
    }

    fn session(&self) -> (Session, tokio::sync::mpsc::UnboundedReceiver<Outbound>) {
        let mut router = Router::new();
        register(&mut router, Arc::clone(&self.git));
        Session::new(Arc::new(router), EventHub::new(), Caller::new(Role::Owner))
    }

    /// `git.log` 를 열고 끝까지 받는다: 청크들과 끝의 결과. credit 은 넉넉히 준다.
    pub async fn stream(&self, params: &GitLogParams) -> (Vec<LogChunk>, StreamOutcome) {
        let (session, mut rx) = self.session();
        session.handle_value(
            json!({"jsonrpc": "2.0", "id": 1, "method": "git.log", "params": params}),
        );
        let opened: StreamOpened = next(&mut rx)
            .await
            .into_response()
            .unwrap()
            .result_as()
            .unwrap();
        session.handle_value(json!({"jsonrpc": "2.0", "method": "stream.credit", "params": {"streamId": opened.stream_id, "credit": 100_000}}));
        let mut chunks = Vec::new();
        loop {
            let note = next(&mut rx).await.into_notification().unwrap();
            if note.method == STREAM_END {
                return (chunks, note.params_as::<StreamEnd>().unwrap().outcome);
            }
            assert_eq!(note.method, STREAM_CHUNK);
            chunks.push(
                serde_json::from_value(note.params_as::<StreamChunk>().unwrap().data).unwrap(),
            );
        }
    }

    pub async fn page(&self, params: &GitLogPageParams) -> Result<GitLogPage, RpcError> {
        let (session, mut rx) = self.session();
        session.handle_value(
            json!({"jsonrpc": "2.0", "id": 1, "method": "git.logPage", "params": params}),
        );
        next(&mut rx).await.into_response().unwrap().result_as()
    }
}

async fn next(rx: &mut tokio::sync::mpsc::UnboundedReceiver<Outbound>) -> Outbound {
    tokio::time::timeout(Duration::from_secs(30), rx.recv())
        .await
        .expect("an outbound message")
        .expect("open channel")
}

fn params(project: &str) -> GitLogParams {
    GitLogParams {
        project_id: project.to_owned(),
        ..GitLogParams::default()
    }
}

fn page_params(p: &GitLogParams, cursor: Option<String>, limit: u32) -> GitLogPageParams {
    GitLogPageParams {
        project_id: p.project_id.clone(),
        worktree: p.worktree.clone(),
        refs: p.refs.clone(),
        filter: p.filter.clone(),
        order: p.order,
        cursor,
        limit,
    }
}

/// 쪽을 끝까지 이어 받는다.
async fn all_pages(fx: &Fx, p: &GitLogParams, limit: u32) -> (Vec<LogRow>, usize, Option<u32>) {
    let (mut rows, mut pages, mut cursor) = (Vec::new(), 0, None::<String>);
    let mut total = None;
    loop {
        let page = fx
            .page(&page_params(p, cursor.clone(), limit))
            .await
            .unwrap();
        pages += 1;
        assert!(page.rows.len() <= limit as usize);
        total = total.or(page.total);
        rows.extend(page.rows);
        match page.next_cursor {
            // 커서가 제자리면 끝없이 같은 쪽을 받는다 — 멈추는 대신 실패한다.
            Some(c) => {
                assert_ne!(cursor.as_ref(), Some(&c), "the cursor must move");
                cursor = Some(c);
            }
            None => return (rows, pages, total),
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_stream_sends_a_first_screen_then_the_rest() {
    let s = Synth::new(1200);
    let fx = Fx::new();
    let id = fx.project(&s.dir);
    let (chunks, outcome) = fx.stream(&params(&id)).await;
    assert_eq!(outcome, StreamOutcome::Completed);
    let sizes: Vec<(usize, Option<u32>)> = chunks.iter().map(|c| (c.rows.len(), c.total)).collect();
    let total = chunks.iter().map(|c| c.rows.len()).sum::<usize>();
    assert_eq!(sizes[0], (FIRST_CHUNK, Some(total as u32)));
    assert!(
        sizes[1..].iter().all(|&(n, t)| t.is_none() && n <= CHUNK),
        "{sizes:?}"
    );
    assert_eq!(sizes[1].0, CHUNK);
    let rows: Vec<LogRow> = chunks.into_iter().flat_map(|c| c.rows).collect();
    assert_eq!(
        hashes(&rows),
        s.git_log(&["--topo-order", "--branches", "--remotes", "--tags", "HEAD"])
    );
    check_lanes(&rows);
    // 쪽 단위로 이어 받아도 같은 행이다(레인까지).
    let (paged, pages, page_total) = all_pages(&fx, &params(&id), 500).await;
    assert_eq!(without_tops(&paged), without_tops(&rows));
    // 쪽의 첫 행에는 열린 레인 전체가 있어 쪽부터 이어 계산할 수 있다.
    for page in paged.chunks(500) {
        assert!(page[0].graph.as_ref().unwrap().top.is_some());
    }
    assert_eq!(page_total, Some(total as u32));
    assert_eq!(pages, total.div_ceil(500));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn filtered_streams_and_pages_have_no_graph() {
    let s = Synth::new(1200);
    let fx = Fx::new();
    let id = fx.project(&s.dir);
    let p = GitLogParams {
        filter: LogFilter {
            author: Some("Grace".into()),
            ..LogFilter::default()
        },
        refs: Some(vec!["main".into()]),
        order: LogOrder::Date,
        ..params(&id)
    };
    let (chunks, outcome) = fx.stream(&p).await;
    assert_eq!(outcome, StreamOutcome::Completed);
    assert!(chunks.iter().all(|c| c.total.is_none()));
    let rows: Vec<LogRow> = chunks.into_iter().flat_map(|c| c.rows).collect();
    assert!(rows.len() > 7 && rows.iter().all(|r| r.graph.is_none()));
    assert_eq!(
        hashes(&rows),
        s.git_log(&["--date-order", "-F", "--author=Grace", "main"])
    );
    let (paged, pages, total) = all_pages(&fx, &p, 7).await;
    assert_eq!(paged, rows);
    assert_eq!(total, None);
    assert_eq!(pages, rows.len().div_ceil(7));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bad_requests_fail_and_a_folder_without_a_repository_is_empty() {
    let s = Synth::new(50);
    let fx = Fx::new();
    let id = fx.project(&s.dir);
    let failed = |outcome: StreamOutcome| match outcome {
        StreamOutcome::Failed { error } => error.code,
        other => panic!("expected failed, got {other:?}"),
    };
    let unknown = GitLogParams {
        refs: Some(vec!["no-such-branch".into()]),
        ..params(&id)
    };
    let (chunks, outcome) = fx.stream(&unknown).await;
    assert!(chunks.is_empty());
    assert_eq!(failed(outcome), pw_rpc::code::INVALID_PARAMS);
    let regex = GitLogParams {
        filter: LogFilter {
            message: Some("[".into()),
            regex: true,
            ..LogFilter::default()
        },
        ..params(&id)
    };
    assert_eq!(
        failed(fx.stream(&regex).await.1),
        pw_rpc::code::INVALID_PARAMS
    );
    for (cursor, limit) in [(None, 0), (None, MAX_PAGE + 1), (Some("x".to_owned()), 10)] {
        let e = fx
            .page(&page_params(&params(&id), cursor, limit))
            .await
            .unwrap_err();
        assert_eq!(e.code, pw_rpc::code::INVALID_PARAMS);
    }
    let e = fx.page(&page_params(&unknown, None, 10)).await.unwrap_err();
    assert_eq!(e.code, pw_rpc::code::INVALID_PARAMS);
    // 로그에 없는 커밋의 커서는 낡은 커서다.
    let stale = fx
        .page(&page_params(&params(&id), Some("ab".repeat(20)), 10))
        .await
        .unwrap_err();
    assert_eq!(stale.code, pw_rpc::code::INVALID_PARAMS);
    assert!(
        format!("{:?}", stale.data).contains("stale cursor"),
        "{stale:?}"
    );
    // 마지막 행의 커서는 빈 쪽.
    let all = fx
        .page(&page_params(&params(&id), None, 500))
        .await
        .unwrap();
    let last = all.rows.last().unwrap().hash.clone();
    let past = fx
        .page(&page_params(&params(&id), Some(last), 10))
        .await
        .unwrap();
    assert!(past.rows.is_empty() && past.next_cursor.is_none());

    let plain = s.ceiling.join("plain");
    std::fs::create_dir_all(&plain).unwrap();
    let none = fx.project(&plain);
    let (chunks, outcome) = fx.stream(&params(&none)).await;
    assert!(chunks.is_empty());
    assert_eq!(outcome, StreamOutcome::Completed);
    let page = fx
        .page(&page_params(&params(&none), None, 10))
        .await
        .unwrap();
    assert!(page.rows.is_empty() && page.next_cursor.is_none() && page.total.is_none());
}

/// 멈춤: 걷기 · 순서 · 행 만들기 어디서든 멈춤 신호를 보고 `CANCELLED` 로 끝난다(commit-graph 있음 · 없음 — 걷는 길이 다르다).
/// 받는 쪽이 사라지면(`emit` 이 거절) 더 보내지 않는다.
#[test]
fn a_stop_signal_or_a_gone_receiver_ends_the_log() {
    let s = Synth::new(1200);
    for graph in [false, true] {
        if graph {
            s.write_commit_graph();
        }
        let query = Query::new(None, &LogFilter::default(), LogOrder::Topo).unwrap();
        let e = run(
            &open(&s.dir),
            false,
            &query,
            Window::default(),
            &AtomicBool::new(true),
            &mut |_| true,
        )
        .unwrap_err();
        assert_eq!(e.code, pw_rpc::code::CANCELLED, "commit-graph {graph}");
        let mut chunks = 0;
        let outcome = run(
            &open(&s.dir),
            false,
            &query,
            Window::default(),
            &AtomicBool::new(false),
            &mut |_| {
                chunks += 1;
                false
            },
        )
        .unwrap();
        assert!(outcome.abandoned);
        assert_eq!(chunks, 1);
    }
}

/// 멈춤 신호는 걷기 자체가 본다(첫 청크 전 시간의 대부분이 걷기다): 켜 두고 걸으면 commit-graph 없음(보조 스레드 미리 읽기) ·
/// 있음 · 경로 필터의 걷기 모두 `CANCELLED` 로 끝난다.
#[test]
fn the_walk_itself_stops_on_the_stop_signal() {
    let s = Synth::new(1200);
    for graph in [false, true] {
        if graph {
            s.write_commit_graph();
        }
        let repo = open(&s.dir);
        let tips = tips(
            &repo,
            None,
            &branch_refs(&repo).unwrap(),
            head(&repo).unwrap().0,
        )
        .unwrap();
        for paths in [vec![], vec![gix::bstr::BString::from("d0")]] {
            let e = walk::collect(&repo, &tips, &paths, &AtomicBool::new(true))
                .err()
                .expect("the walk stops");
            assert_eq!(
                e.code,
                pw_rpc::code::CANCELLED,
                "commit-graph {graph}, paths {paths:?}"
            );
        }
    }
}

/// `git.logPage` 의 커서는 쪽의 마지막 커밋이다: 쪽 사이에 새 커밋이 생겨도 다음 쪽이 앞 쪽과 겹치지 않고, 그 커밋이 로그에서
/// 사라지면(force push) 낡은 커서로 invalid params.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_page_cursor_survives_new_commits_and_rejects_a_rewritten_history() {
    let r = crate::testing::TestRepo::with_commit();
    for n in 0..30 {
        r.commit_file("f.txt", &format!("{n}\n"), &format!("c{n}"));
    }
    let fx = Fx::new();
    let id = fx.project(r.root());
    let p = GitLogParams {
        refs: Some(vec!["main".into()]),
        ..params(&id)
    };
    let before: Vec<String> = fx
        .page(&page_params(&p, None, 100))
        .await
        .unwrap()
        .rows
        .into_iter()
        .map(|r| r.hash)
        .collect();
    let first = fx.page(&page_params(&p, None, 10)).await.unwrap();
    assert_eq!(hashes(&first.rows), before[..10]);
    r.commit_file("f.txt", "new\n", "new on top");
    let second = fx
        .page(&page_params(&p, first.next_cursor.clone(), 10))
        .await
        .unwrap();
    assert_eq!(
        hashes(&second.rows),
        before[10..20],
        "continues after the last row"
    );

    r.git(&["reset", "--quiet", "--hard", &before[15]]);
    let e = fx
        .page(&page_params(&p, first.next_cursor, 10))
        .await
        .unwrap_err();
    assert_eq!(e.code, pw_rpc::code::INVALID_PARAMS);
    assert!(format!("{:?}", e.data).contains("stale cursor"), "{e:?}");
}

/// 연결 워크트리를 `worktree` 로 주면 그 워크트리의 HEAD 가 장식된다(저장소는 같다).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_linked_worktree_logs_its_own_head() {
    let r = crate::testing::TestRepo::with_commit();
    let wt = r.add_worktree("wt", "feature");
    r.commit_all_in(&wt, "on feature");
    let fx = Fx::new();
    let id = fx.project(r.root());
    let p = GitLogParams {
        worktree: Some(wt.display().to_string()),
        ..params(&id)
    };
    let (chunks, _) = fx.stream(&p).await;
    let rows: Vec<LogRow> = chunks.into_iter().flat_map(|c| c.rows).collect();
    assert_eq!(rows[0].subject, "on feature");
    let current: Vec<&str> = rows
        .iter()
        .flat_map(|r| r.refs.iter().filter(|d| d.current).map(|d| d.name.as_str()))
        .collect();
    assert_eq!(current, ["feature"]);
}
