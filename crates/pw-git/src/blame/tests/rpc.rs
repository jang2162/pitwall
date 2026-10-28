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
/// 안에 끝난다 — 가짜 git 이 범위 하나를 내고, 30 ms 뒤 하나를 더 내고 30초 잔다.
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
            "#!/bin/sh\nprintf '{head} 1 1 1\\nauthor A\\nauthor-mail <a@x>\\nauthor-time 1\\nsummary s\\nfilename a.txt\\n'\nsleep 0.03\nprintf '{head} 2 2 1\\nfilename a.txt\\n'\nsleep 30 &\necho \"$$ $!\" > '{pids}.tmp'\nmv '{pids}.tmp' '{pids}'\nwait\n",
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
    // 첫 범위는 곧바로, 30 ms 뒤에 온 둘째 범위는 git 이 더 내지 않아도(30초 잔다) 100 ms 안팎에 온다 — 둘째는 데이터가 더 오지
    // 않는 동안의 시간 기준 보내기만 보낼 수 있다(git 이 둘을 한 번에 냈으면 첫 청크에 함께 온다).
    let mut lines = Vec::new();
    while lines.len() < 2 {
        let note = next(&mut rx).await.into_notification().unwrap();
        assert_eq!(
            note.method, STREAM_CHUNK,
            "ranges arrive while git still runs"
        );
        let chunk: BlameChunk =
            serde_json::from_value(note.params_as::<StreamChunk>().unwrap().data).unwrap();
        if lines.is_empty() {
            assert_eq!(chunk.file.unwrap().engine, BlameEngine::Git);
        }
        lines.extend(chunk.ranges.iter().map(|r| r.line));
    }
    assert_eq!(lines, [1, 2]);
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

/// UTF-8 이 아닌 경로(`caf\xe9.txt` → `cafe.txt` 이름 바뀜)는 U+FFFD 로 바꿔 싣지 않고 invalid params 로 끝난다 — 바꾼 이름으로는
/// 다시 지목할 수 없어 「이 커밋 이전으로」 가 조용히 빈 결과가 된다(`super::super::path_text`). 두 엔진 · 파일 히스토리 모두.
/// macOS 는 그런 파일 이름을 만들 수 없어 index 로 커밋한다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn paths_that_are_not_utf8_fail_instead_of_turning_into_replacement_characters() {
    use crate::file_history::FileHistoryChunk;
    use crate::file_history::methods::GitFileHistoryParams;

    let repo = TestRepo::with_commit();
    let index_info = |line: Vec<u8>| {
        let list = repo.base().join("index-info");
        std::fs::write(&list, line).unwrap();
        let out = repo
            .command_in(repo.root())
            .args(["update-index", "--index-info"])
            .stdin(std::fs::File::open(&list).unwrap())
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    let blob = |text: &str| {
        let file = repo.base().join("blob");
        std::fs::write(&file, text).unwrap();
        repo.git(&["hash-object", "-w", "--", file.to_str().unwrap()])
            .trim()
            .to_owned()
    };
    let mut add = format!("100644 {} 0\t", blob("a\nb\nc\nd\ne\nf\ng\nh\n")).into_bytes();
    add.extend_from_slice(b"caf\xe9.txt\n");
    index_info(add);
    repo.git(&["commit", "-q", "-m", "add"]);
    let mut rename = format!("0 {} 0\t", "0".repeat(40)).into_bytes();
    rename.extend_from_slice(b"caf\xe9.txt\n");
    rename.extend_from_slice(
        format!("100644 {} 0\tcafe.txt\n", blob("a\nb\nc\nd\ne\nf\ng\nH\n")).as_bytes(),
    );
    index_info(rename);
    repo.git(&["commit", "-q", "-m", "rename"]);
    // git 도 이름 바뀜으로 본다(대조의 전제).
    assert!(
        repo.git(&[
            "log",
            "--follow",
            "--name-status",
            "--format=",
            "--",
            "cafe.txt"
        ])
        .starts_with('R'),
    );

    let fx = Fx::new("");
    let id = fx.project(repo.root());
    let invalid = |outcome: &StreamOutcome| {
        matches!(outcome, StreamOutcome::Failed { error } if error.code == pw_rpc::code::INVALID_PARAMS
            && error.data.as_ref().and_then(|d| d.as_str()).is_some_and(|d| d.contains("not UTF-8")))
    };
    for (trusted, engine) in [(false, BlameEngine::Gix), (true, BlameEngine::Git)] {
        fx.trust(&id, trusted);
        let (chunks, outcome) = fx
            .stream::<_, BlameChunk>("git.blame", &params(&id, "cafe.txt", None))
            .await;
        assert!(invalid(&outcome), "{engine:?}: {outcome:?}");
        // 실린 경로에 U+FFFD 가 없다.
        for chunk in &chunks {
            assert_eq!(chunk.file.as_ref().map(|f| f.engine), Some(engine));
            assert!(
                chunk
                    .ranges
                    .iter()
                    .all(|r| !r.source_path.contains('\u{fffd}'))
            );
        }
        let (_, outcome) = fx
            .stream::<_, FileHistoryChunk>(
                "git.fileHistory",
                &GitFileHistoryParams {
                    project_id: id.clone(),
                    worktree: None,
                    path: "cafe.txt".into(),
                    rev: None,
                },
            )
            .await;
        assert!(invalid(&outcome), "fileHistory: {outcome:?}");
    }
}

/// 크기 상한(`READ_ONLY_MAX`, 50 MiB — 텍스트 · 바이너리 같다)과 서브모듈 경로.
/// - 그 리비전의 파일이 상한을 넘으면 두 엔진 모두 invalid params(git blame 은 읽는다 — 이 상한은 우리 것이다).
/// - 지금은 작고 이력에만 상한을 넘는 버전이 있으면: 엔진 `git` 은 `git blame -M` 과 줄마다 같고, 엔진 `gix` 는 그 버전을 읽지
///   않고 invalid params 다 — 상한을 넘는 버전을 빈 데이터로 견주어 조용히 다른 결과를 내지 않는다(`native::text_cache`).
/// - 서브모듈 경로는 두 엔진 모두 invalid params(git blame 도 파일이 아니라 실패한다).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn files_over_the_limit_and_submodule_paths_are_refused_by_both_engines() {
    use pw_fs::limits::READ_ONLY_MAX;

    let repo = TestRepo::new();
    let (dir, ceiling) = (repo.root(), repo.base());
    let line = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcde\n";
    let big: String = line.repeat(READ_ONLY_MAX as usize / line.len() + 1);
    assert!(big.len() as u64 > READ_ONLY_MAX);
    repo.write("past.txt", &format!("first\n{big}"));
    repo.write("now.txt", &big);
    repo.commit_all("big");
    repo.write("past.txt", "first\nsecond\n");
    repo.commit_all("small");
    let gitlink = repo.head();
    repo.git(&[
        "update-index",
        "--add",
        "--cacheinfo",
        &format!("160000,{gitlink},sub"),
    ]);
    repo.git(&["commit", "-q", "-m", "submodule"]);
    let out = git_command(dir, ceiling)
        .args(["blame", "HEAD", "--", "sub"])
        .output()
        .unwrap();
    assert!(!out.status.success(), "git blame of a submodule path fails");

    let fx = Fx::new("");
    let id = fx.project(dir);
    let failed = |outcome: &StreamOutcome, needle: &str| {
        matches!(outcome, StreamOutcome::Failed { error } if error.code == pw_rpc::code::INVALID_PARAMS
            && error.data.as_ref().and_then(|d| d.as_str()).is_some_and(|d| d.contains(needle)))
    };
    for trusted in [false, true] {
        fx.trust(&id, trusted);
        for (path, needle) in [("now.txt", "blame reads at most"), ("sub", "is not a file")] {
            let (chunks, outcome) = fx
                .stream::<_, BlameChunk>("git.blame", &params(&id, path, None))
                .await;
            assert!(chunks.is_empty(), "{path}");
            assert!(
                failed(&outcome, needle),
                "trusted {trusted}, {path}: {outcome:?}"
            );
        }
        let (chunks, outcome) = fx
            .stream::<_, BlameChunk>("git.blame", &params(&id, "past.txt", None))
            .await;
        if trusted {
            assert_eq!(outcome, StreamOutcome::Completed);
            let (file, lines, _) = expand(&chunks);
            assert_eq!(file.engine, BlameEngine::Git);
            assert_eq!(lines, git_blame(dir, ceiling, "HEAD", "past.txt", true).0);
        } else {
            assert!(chunks.is_empty());
            assert!(failed(&outcome, "is larger than"), "{outcome:?}");
        }
    }
}
