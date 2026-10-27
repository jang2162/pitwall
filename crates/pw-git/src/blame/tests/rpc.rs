//! RPC: `git.blame` 의 엔진 고르기(신뢰 → git, 미신뢰 → gix), 청크 없는 끝, 거절, 멈춤(git 의 프로세스 그룹이 끝난다).

use std::sync::Arc;
use std::time::Duration;

use pw_core::bus::{DEFAULT_CAPACITY, EventBus, Scope};
use pw_core::config::{AppPaths, ConfigSource, ConfigStore};
use pw_core::registry::{NewProject, Registry as Projects};
use pw_rpc::protocol::{
    STREAM_CHUNK, STREAM_END, StreamChunk, StreamEnd, StreamOpened, StreamOutcome,
};
use pw_rpc::{Caller, EventHub, Outbound, Role, Router, Session};
use pw_runtime::{LoginShell, Runtime};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::json;

use super::*;
use crate::blame::methods::GitBlameParams;
use crate::testing::TestRepo;
use crate::{Git, HostParts};

/// 레지스트리에 붙은 `Git`. 로그인 셸은 없다 — 앱 프로세스(시험)의 PATH 에서 git 을 찾는다.
pub(crate) struct Fx {
    _tmp: tempfile::TempDir,
    pub git: Arc<Git>,
    pub projects: Projects,
}

impl Fx {
    /// `settings` 는 전역 `settings.toml` 내용.
    pub fn new(settings: &str) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::under(&tmp.path().join("app"));
        std::fs::create_dir_all(&paths.config).unwrap();
        std::fs::write(paths.config.join("settings.toml"), settings).unwrap();
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

    pub fn trust(&self, id: &str, trusted: bool) {
        self.projects.set_trust(id, trusted).unwrap();
    }

    pub fn session(&self) -> (Session, tokio::sync::mpsc::UnboundedReceiver<Outbound>) {
        let mut router = Router::new();
        crate::methods::register(&mut router, Arc::clone(&self.git));
        Session::new(Arc::new(router), EventHub::new(), Caller::new(Role::Owner))
    }

    /// 스트림 `method` 를 열고 끝까지 받는다: 청크들과 끝의 결과. credit 은 넉넉히 준다.
    pub async fn stream<P: Serialize, C: DeserializeOwned>(
        &self,
        method: &str,
        params: &P,
    ) -> (Vec<C>, StreamOutcome) {
        let (session, mut rx) = self.session();
        let opened = open_stream(&session, &mut rx, method, params).await;
        let _ = opened;
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
}

/// 스트림을 열고 credit 을 준다.
pub(crate) async fn open_stream<P: Serialize>(
    session: &Session,
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<Outbound>,
    method: &str,
    params: &P,
) -> StreamOpened {
    session.handle_value(json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}));
    let opened: StreamOpened = next(rx).await.into_response().unwrap().result_as().unwrap();
    session.handle_value(json!({"jsonrpc": "2.0", "method": "stream.credit", "params": {"streamId": opened.stream_id, "credit": 100_000}}));
    opened
}

pub(crate) async fn next(rx: &mut tokio::sync::mpsc::UnboundedReceiver<Outbound>) -> Outbound {
    tokio::time::timeout(Duration::from_secs(30), rx.recv())
        .await
        .expect("an outbound message")
        .expect("open channel")
}

fn params(project: &str, path: &str, rev: Option<&str>) -> GitBlameParams {
    GitBlameParams {
        project_id: project.to_owned(),
        worktree: None,
        path: path.to_owned(),
        rev: rev.map(str::to_owned),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn trusted_projects_blame_with_git_and_untrusted_ones_with_gix() {
    let repo = TestRepo::new();
    let fx_repo = repo.blame_fixture();
    let fx = Fx::new("");
    let id = fx.project(repo.root());
    let (dir, ceiling) = (repo.root(), repo.base());

    let (chunks, outcome) = fx
        .stream::<_, BlameChunk>("git.blame", &params(&id, fx_repo.path, None))
        .await;
    assert_eq!(outcome, StreamOutcome::Completed);
    let (file, lines, _) = expand(&chunks);
    assert_eq!(file.engine, BlameEngine::Gix);
    assert_eq!(file.commit, repo.head());
    assert_eq!(
        lines,
        git_blame(dir, ceiling, "HEAD", fx_repo.path, false).0
    );

    fx.trust(&id, true);
    let (chunks, outcome) = fx
        .stream::<_, BlameChunk>("git.blame", &params(&id, fx_repo.path, None))
        .await;
    assert_eq!(outcome, StreamOutcome::Completed);
    let (file, lines, _) = expand(&chunks);
    assert_eq!(file.engine, BlameEngine::Git);
    assert_eq!(lines, git_blame(dir, ceiling, "HEAD", fx_repo.path, true).0);

    // 「이 커밋 이전으로」: 범위의 previous 로 다시 부른다.
    let previous = lines.iter().find_map(|l| l.previous.clone()).unwrap();
    let (chunks, _) = fx
        .stream::<_, BlameChunk>("git.blame", &params(&id, &previous.1, Some(&previous.0)))
        .await;
    let (file, lines, _) = expand(&chunks);
    assert_eq!(file.commit, previous.0);
    assert_eq!(
        lines,
        git_blame(dir, ceiling, &previous.0, &previous.1, true).0
    );
}

/// git 을 찾지 못하면 신뢰한 프로젝트도 gix 로 blame 한다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_missing_git_falls_back_to_gix() {
    let repo = TestRepo::new();
    let fx_repo = repo.blame_fixture();
    let fx = Fx::new("[git]\npath = \"/nonexistent/pitwall-no-git\"\n");
    let id = fx.project(repo.root());
    fx.trust(&id, true);
    let (chunks, outcome) = fx
        .stream::<_, BlameChunk>("git.blame", &params(&id, fx_repo.path, None))
        .await;
    assert_eq!(outcome, StreamOutcome::Completed);
    assert_eq!(expand(&chunks).0.engine, BlameEngine::Gix);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nothing_to_blame_ends_without_chunks_and_bad_params_fail() {
    let fx = Fx::new("");
    let outside = tempfile::tempdir().unwrap();
    let none = fx.project(outside.path());
    let unborn_repo = TestRepo::new();
    let unborn = fx.project(unborn_repo.root());
    let repo = TestRepo::new();
    repo.commit_file("dir/a.txt", "one\n", "add");
    let id = fx.project(repo.root());
    for p in [
        params(&none, "a.txt", None),
        params(&unborn, "a.txt", None),
        params(&id, "missing.txt", None),
    ] {
        let (chunks, outcome) = fx.stream::<_, BlameChunk>("git.blame", &p).await;
        assert_eq!(outcome, StreamOutcome::Completed, "{p:?}");
        assert!(chunks.is_empty(), "{p:?}");
    }
    for p in [
        params(&id, "dir", None),
        params(&id, "../a.txt", None),
        params(&id, "dir/a.txt", Some("no-such-rev")),
        params("no-such-project", "a.txt", None),
    ] {
        let (_, outcome) = fx.stream::<_, BlameChunk>("git.blame", &p).await;
        match outcome {
            StreamOutcome::Failed { error } => {
                assert_eq!(error.code, pw_rpc::code::INVALID_PARAMS, "{p:?}");
            }
            other => panic!("{p:?}: {other:?}"),
        }
    }
}

/// git 이 출력을 멈춘 동안 모인 범위는 기다리지 않고 보낸다(100 ms), 멈추면(`stream.cancel`) `git blame` 의 프로세스 그룹이 1초
/// 안에 끝난다 — 가짜 git 이 범위 하나를 내고, 잠시 뒤 하나를 더 내고 30초 잔다.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelling_ends_git_blame_within_a_second() {
    use std::os::unix::fs::PermissionsExt;
    use std::time::Instant;

    use pw_rpc::protocol::STREAM_CANCEL;

    let repo = TestRepo::new();
    let head = repo.commit_file("a.txt", "one\ntwo\n", "add");
    let bin = repo.base().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let pids = repo.base().join("pids");
    let fake = bin.join("git");
    std::fs::write(
        &fake,
        format!(
            "#!/bin/sh\nprintf '{head} 1 1 1\\nauthor A\\nauthor-mail <a@x>\\nauthor-time 1\\nsummary s\\nfilename a.txt\\n'\nsleep 0.3\nprintf '{head} 2 2 1\\nfilename a.txt\\n'\nsleep 30 &\necho \"$$ $!\" > '{pids}.tmp'\nmv '{pids}.tmp' '{pids}'\nwait\n",
            pids = pids.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    let fx = Fx::new(&format!("[git]\npath = \"{}\"\n", fake.display()));
    let id = fx.project(repo.root());
    fx.trust(&id, true);

    let (session, mut rx) = fx.session();
    let opened = open_stream(&session, &mut rx, "git.blame", &params(&id, "a.txt", None)).await;
    let note = next(&mut rx).await.into_notification().unwrap();
    assert_eq!(
        note.method, STREAM_CHUNK,
        "the first range arrives before git ends"
    );
    let chunk: BlameChunk =
        serde_json::from_value(note.params_as::<StreamChunk>().unwrap().data).unwrap();
    assert_eq!(chunk.file.unwrap().engine, BlameEngine::Git);
    // 둘째 범위: git 이 더 내지 않아도(30초 잔다) 곧 온다.
    let note = next(&mut rx).await.into_notification().unwrap();
    assert_eq!(
        note.method, STREAM_CHUNK,
        "the pending range is sent while git waits"
    );
    let chunk: BlameChunk =
        serde_json::from_value(note.params_as::<StreamChunk>().unwrap().data).unwrap();
    assert_eq!(chunk.ranges.len(), 1);
    assert_eq!(chunk.ranges[0].line, 2);
    let deadline = Instant::now() + Duration::from_secs(10);
    let (git, child) = loop {
        if let Ok(text) = std::fs::read_to_string(&pids) {
            let mut it = text
                .split_whitespace()
                .map(|p| p.parse::<libc::pid_t>().unwrap());
            break (it.next().unwrap(), it.next().unwrap());
        }
        assert!(Instant::now() < deadline, "the fake git did not start");
        tokio::time::sleep(Duration::from_millis(10)).await;
    };
    let alive = |pid: libc::pid_t| {
        // SAFETY: 신호 0 은 프로세스가 있는지만 본다.
        unsafe { libc::kill(pid, 0) == 0 }
    };
    assert!(alive(git) && alive(child));
    let cancelled = Instant::now();
    session.handle_value(
        json!({"jsonrpc": "2.0", "method": STREAM_CANCEL, "params": {"streamId": opened.stream_id}}),
    );
    while alive(git) || alive(child) {
        assert!(
            cancelled.elapsed() < Duration::from_secs(1),
            "git {git} alive: {}, child {child} alive: {}",
            alive(git),
            alive(child)
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    loop {
        let note = next(&mut rx).await.into_notification().unwrap();
        if note.method == STREAM_END {
            assert_eq!(
                note.params_as::<StreamEnd>().unwrap().outcome,
                StreamOutcome::Cancelled
            );
            break;
        }
    }
}

/// 신뢰한 프로젝트의 blame 은 저장소의 textconv 를 돌리지 않고 커밋의 내용을 blame 한다(줄 수가 blob 과 같다).
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn trusted_blame_reads_the_stored_content_not_the_textconv_output() {
    use std::os::unix::fs::PermissionsExt;

    let repo = TestRepo::new();
    repo.commit_file("f.txt", "a\nb\n", "add");
    let marks = repo.base().join("textconv-ran");
    let textconv = repo.base().join("textconv");
    std::fs::write(
        &textconv,
        format!(
            "#!/bin/sh\ntouch '{}'\ntr a-z A-Z < \"$1\"\necho extra\n",
            marks.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&textconv, std::fs::Permissions::from_mode(0o755)).unwrap();
    repo.git(&["config", "diff.up.textconv", textconv.to_str().unwrap()]);
    std::fs::write(repo.root().join(".git/info/attributes"), "*.txt diff=up\n").unwrap();
    // 대조: git blame 의 기본은 textconv 를 돌린다.
    assert_eq!(
        repo.git(&["blame", "--porcelain", "HEAD", "--", "f.txt"])
            .matches("\n\t")
            .count(),
        3
    );
    assert!(marks.exists());
    std::fs::remove_file(&marks).unwrap();

    let fx = Fx::new("");
    let id = fx.project(repo.root());
    fx.trust(&id, true);
    let (chunks, outcome) = fx
        .stream::<_, BlameChunk>("git.blame", &params(&id, "f.txt", None))
        .await;
    assert_eq!(outcome, StreamOutcome::Completed);
    let (file, lines, _) = expand(&chunks);
    assert_eq!(
        (file.engine, file.lines, lines.len()),
        (BlameEngine::Git, 2, 2)
    );
    assert!(!marks.exists(), "textconv ran");
}
