//! 가짜 셸(받은 스크립트를 `eval` 로 돌리는 sh 스크립트)로 캐시 · 물러나기 · 다시 읽기 · 제공자 고르기를 본다.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use pw_core::bus::{DEFAULT_CAPACITY, EventBus, Filter, Received, Scope};
use pw_core::config::AppPaths;
use pw_core::registry::{NewProject, Registry as Projects};

use crate::login_shell::tests::fake_shell;
use crate::methods::{RuntimeGetParams, RuntimeSource, RuntimeStatus};
use crate::{Layer, LoginShell, Runtime, RuntimeEnv, RuntimeHost, RuntimeProvider, TOPIC_CHANGED};

const WORKING: &str = "JAVA_HOME=/opt/jdk-21; export JAVA_HOME\nPATH=/shell/bin:/usr/bin:/bin; export PATH\neval \"$4\"";
const HANGING: &str = "sleep 30";

struct Fixture {
    tmp: tempfile::TempDir,
    shell: PathBuf,
}

impl Fixture {
    fn new(body: &str) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let shell = fake_shell(tmp.path(), body);
        Self { tmp, shell }
    }

    fn rewrite(&self, body: &str) {
        fake_shell(self.tmp.path(), body);
    }

    fn runtime(&self, timeout: Duration) -> Runtime {
        self.runtime_cooling(timeout, Duration::from_secs(30))
    }

    fn runtime_cooling(&self, timeout: Duration, cooldown: Duration) -> Runtime {
        Runtime::new(LoginShell::new(Some(self.shell.clone()), timeout).with_cooldown(cooldown))
    }

    fn dir(&self) -> PathBuf {
        self.tmp.path().join("project")
    }
}

/// ① 첫 호출이 시간을 넘기면 앱 프로세스의 환경으로 물러난다. 그 뒤로는 셸을 기다리지 않는다 — 냉각 동안은 셸을 다시 돌리지
/// 않고, 냉각이 끝난 뒤의 호출도 물러난 결과를 곧바로 받고 재시도는 백그라운드로 돈다. 셸을 고쳤으면 재시도가 끝난 뒤의 호출이
/// 로그인 셸 환경을 받고(실패를 남기지 않는다), 물러난 환경을 받았던 프로젝트에 `runtime.changed`(`loginShell`)가 한 번 나간다.
#[test]
fn a_timeout_falls_back_and_later_retries_in_the_background_without_waiting() {
    let f = Fixture::new(HANGING);
    let bus = EventBus::new(DEFAULT_CAPACITY);
    let projects = registry(f.tmp.path(), &bus);
    let p1 = project(&projects, &f.tmp.path().join("one"));
    let p2 = project(&projects, &f.tmp.path().join("two"));
    let cooldown = Duration::from_millis(1500);
    let timeout = Duration::from_millis(300);
    let runtime = f.runtime_cooling(timeout, cooldown);
    runtime.attach(RuntimeHost {
        projects: projects.clone(),
        bus: bus.clone(),
    });
    let mut subs: Vec<_> = [&p1, &p2]
        .map(|p| {
            bus.subscribe(
                Filter::all()
                    .scope(Scope::Project(p.clone()))
                    .topics([TOPIC_CHANGED]),
            )
        })
        .into_iter()
        .collect();
    // 냉각은 첫 호출 안에서 시작한다 — 끝나는 때는 `before + cooldown` 과 `after + cooldown` 사이다.
    let before = Instant::now();
    let first = runtime.env(&p1, &f.dir());
    let after = Instant::now();
    assert!(after - before >= timeout, "{:?}", after - before);
    assert!(
        after - before < Duration::from_secs(3),
        "{:?}",
        after - before
    );
    assert_eq!(first.info.source, RuntimeSource::Process);
    assert!(!first.login_shell);
    assert!(
        matches!(&first.info.status, RuntimeStatus::Fallback { reason } if reason.contains("did not print its environment")),
        "{:?}",
        first.info.status
    );
    assert_eq!(first.env, RuntimeEnv::process());

    // 냉각 중: 셸을 기다리지 않는다(시간 제한 300 ms 보다 빠르다).
    let started = Instant::now();
    let cooling = runtime.env(&p1, &f.dir());
    assert!(
        started.elapsed() < Duration::from_millis(200),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(cooling.info.source, RuntimeSource::Process);
    assert!(
        Instant::now() < before + cooldown,
        "the cooldown ended before the check"
    );

    // 냉각이 끝난 뒤에도 셸이 여전히 멈추면: 기다리지 않고, 재시도가 다시 시간을 넘겨 냉각이 다시 걸린다.
    std::thread::sleep(
        (after + cooldown + Duration::from_millis(20)).saturating_duration_since(Instant::now()),
    );
    let started = Instant::now();
    let retrying = runtime.env(&p1, &f.dir());
    assert!(
        started.elapsed() < Duration::from_millis(200),
        "the call after the cooldown waited {:?}",
        started.elapsed()
    );
    assert_eq!(retrying.info.source, RuntimeSource::Process);
    std::thread::sleep(timeout + Duration::from_millis(300));
    let started = Instant::now();
    assert_eq!(
        runtime.env(&p1, &f.dir()).info.source,
        RuntimeSource::Process
    );
    assert!(
        started.elapsed() < Duration::from_millis(200),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(
        changed_events(&mut subs[0]),
        Vec::<serde_json::Value>::new()
    );

    // 셸을 고치고 냉각이 끝나면: 그 호출은 여전히 곧바로 물러나고, 백그라운드 재시도가 끝난 뒤 로그인 셸 환경이 온다.
    f.rewrite(WORKING);
    std::thread::sleep(cooldown + Duration::from_millis(50));
    let started = Instant::now();
    assert_eq!(
        runtime.env(&p1, &f.dir()).info.source,
        RuntimeSource::Process
    );
    assert!(
        started.elapsed() < Duration::from_millis(200),
        "{:?}",
        started.elapsed()
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    let second = loop {
        let resolved = runtime.env(&p1, &f.dir());
        if resolved.info.status == RuntimeStatus::Ok {
            break resolved;
        }
        assert!(
            Instant::now() < deadline,
            "the background retry did not recover"
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(second.info.provider, "shell");
    assert_eq!(
        second.info.source,
        RuntimeSource::LoginShell {
            shell: f.shell.display().to_string()
        }
    );
    assert!(second.login_shell);
    assert_eq!(second.env.path, OsString::from("/shell/bin:/usr/bin:/bin"));
    assert_eq!(second.env.get("JAVA_HOME"), Some(OsStr::new("/opt/jdk-21")));
    assert_eq!(
        changed_events(&mut subs[0]),
        vec![serde_json::json!({"reason": "loginShell"})]
    );
    // p2 는 아무것도 묻지 않았다.
    assert_eq!(
        changed_events(&mut subs[1]),
        Vec::<serde_json::Value>::new()
    );
}

/// 「환경 다시 읽기」 는 시간 초과의 냉각을 풀어, 셸을 고친 뒤 곧바로 로그인 셸 환경을 준다.
#[test]
fn reload_ends_the_timeout_cooldown() {
    let f = Fixture::new(HANGING);
    let runtime = f.runtime_cooling(Duration::from_millis(300), Duration::from_secs(60));
    assert_eq!(
        runtime.env("p1", &f.dir()).info.source,
        RuntimeSource::Process
    );
    f.rewrite(WORKING);
    assert_eq!(
        runtime.env("p1", &f.dir()).info.source,
        RuntimeSource::Process
    );
    runtime.reload("p1");
    let reloaded = runtime.env("p1", &f.dir());
    assert_eq!(reloaded.info.status, RuntimeStatus::Ok);
    assert_eq!(
        reloaded.env.get("JAVA_HOME"),
        Some(OsStr::new("/opt/jdk-21"))
    );
}

/// 셸 실행 자체의 실패(시간 초과가 아닌 것)는 냉각 없이 다음 호출이 다시 시도한다.
#[test]
fn a_shell_error_is_retried_without_a_cooldown() {
    let f = Fixture::new("exit 1");
    let runtime = f.runtime_cooling(Duration::from_secs(5), Duration::from_secs(60));
    let first = runtime.env("p1", &f.dir());
    assert!(
        matches!(&first.info.status, RuntimeStatus::Fallback { reason } if reason.contains("printed no PATH")),
        "{:?}",
        first.info.status
    );
    f.rewrite(WORKING);
    assert_eq!(runtime.env("p1", &f.dir()).info.status, RuntimeStatus::Ok);
}

/// 성공한 환경은 남는다 — 셸을 바꿔도 다시 읽기 전에는 같은 환경이다. 디렉터리가 달라도 셸은 다시 돌지 않는다.
#[test]
fn a_success_is_cached_until_reload() {
    let f = Fixture::new(WORKING);
    let runtime = f.runtime(Duration::from_secs(5));
    let first = runtime.env("p1", &f.dir());
    f.rewrite("JAVA_HOME=/opt/jdk-25; export JAVA_HOME\neval \"$4\"");
    let again = runtime.env("p1", &f.dir());
    assert!(Arc::ptr_eq(&first, &again));
    let other_dir = runtime.env("p1", &f.dir().join("sub"));
    assert_eq!(
        other_dir.env.get("JAVA_HOME"),
        Some(OsStr::new("/opt/jdk-21"))
    );

    runtime.reload("p1");
    let reloaded = runtime.env("p1", &f.dir());
    assert_eq!(
        reloaded.env.get("JAVA_HOME"),
        Some(OsStr::new("/opt/jdk-25"))
    );
}

fn registry(tmp: &Path, bus: &EventBus) -> Projects {
    Projects::new(AppPaths::under(&tmp.join("app")), None, bus.clone())
}

fn project(projects: &Projects, root: &Path) -> String {
    std::fs::create_dir_all(root).unwrap();
    projects
        .create(NewProject {
            root: root.to_path_buf(),
            ..NewProject::default()
        })
        .unwrap()
        .id
}

fn changed_events(sub: &mut pw_core::Subscription) -> Vec<serde_json::Value> {
    let mut events = Vec::new();
    while let Some(received) = sub.try_recv() {
        match received {
            Received::Event(e) => events.push(e.payload.clone()),
            Received::Lagged(n) => panic!("lagged {n}"),
        }
    }
    events
}

/// ② `runtime.reload` 뒤 그 프로젝트에 `runtime.changed` 가 한 번 나간다. 캐시가 있던 다른 프로젝트에도 한 번씩 나간다.
#[test]
fn reload_publishes_runtime_changed_once_per_project() {
    let f = Fixture::new(WORKING);
    let bus = EventBus::new(DEFAULT_CAPACITY);
    let projects = registry(f.tmp.path(), &bus);
    let p1 = project(&projects, &f.tmp.path().join("one"));
    let p2 = project(&projects, &f.tmp.path().join("two"));
    let p3 = project(&projects, &f.tmp.path().join("three"));
    let runtime = f.runtime(Duration::from_secs(5));
    runtime.attach(RuntimeHost {
        projects: projects.clone(),
        bus: bus.clone(),
    });
    let mut subs: Vec<_> = [&p1, &p2, &p3]
        .map(|p| {
            bus.subscribe(
                Filter::all()
                    .scope(Scope::Project(p.clone()))
                    .topics([TOPIC_CHANGED]),
            )
        })
        .into_iter()
        .collect();
    // p2 는 캐시가 있다(두 디렉터리). p3 는 아무것도 묻지 않았다.
    runtime.env(&p2, &f.dir());
    runtime.env(&p2, &f.dir().join("sub"));

    runtime.reload_project(&p1).unwrap();
    let expected = vec![serde_json::json!({"reason": "reload"})];
    assert_eq!(changed_events(&mut subs[0]), expected);
    assert_eq!(changed_events(&mut subs[1]), expected);
    assert_eq!(
        changed_events(&mut subs[2]),
        Vec::<serde_json::Value>::new()
    );

    // 다시 읽은 뒤 아무것도 묻지 않았으면 다음 다시 읽기는 부른 프로젝트에만 나간다.
    runtime.reload_project(&p1).unwrap();
    assert_eq!(changed_events(&mut subs[0]), expected);
    assert_eq!(
        changed_events(&mut subs[1]),
        Vec::<serde_json::Value>::new()
    );

    let err = runtime.reload_project("no-such-project").unwrap_err();
    assert_eq!(err.code, pw_rpc::code::INVALID_PARAMS, "{err:?}");
}

#[test]
fn get_defaults_to_the_project_root_and_rejects_relative_dirs() {
    let f = Fixture::new(WORKING);
    let bus = EventBus::new(DEFAULT_CAPACITY);
    let projects = registry(f.tmp.path(), &bus);
    let root = f.tmp.path().join("one");
    let id = project(&projects, &root);
    let runtime = f.runtime(Duration::from_secs(5));
    runtime.attach(RuntimeHost { projects, bus });
    let info = runtime
        .get(&RuntimeGetParams {
            project_id: id.clone(),
            dir: None,
        })
        .unwrap();
    assert_eq!(info.provider, "shell");
    assert_eq!(info.status, RuntimeStatus::Ok);
    assert!(info.tools.is_empty());
    let relative = runtime
        .get(&RuntimeGetParams {
            project_id: id,
            dir: Some("sub".to_owned()),
        })
        .unwrap_err();
    assert_eq!(relative.code, pw_rpc::code::INVALID_PARAMS);
    let unknown = runtime
        .get(&RuntimeGetParams {
            project_id: "nope".to_owned(),
            dir: None,
        })
        .unwrap_err();
    assert_eq!(unknown.code, pw_rpc::code::INVALID_PARAMS);
}

/// `dir` 은 정규화한 뒤 프로젝트 루트 안이나, 루트를 담은 연결 워크트리 안이어야 한다.
#[test]
fn get_accepts_dirs_inside_the_project_or_its_worktrees_only() {
    let f = Fixture::new(WORKING);
    let bus = EventBus::new(DEFAULT_CAPACITY);
    let projects = registry(f.tmp.path(), &bus);
    // 메인 체크아웃 `repo` 안의 하위 폴더 `app` 이 프로젝트 루트다. `wt` 는 `repo` 의 연결 워크트리.
    let repo = f.tmp.path().join("repo");
    std::fs::create_dir_all(repo.join(".git/worktrees/wt")).unwrap();
    std::fs::write(repo.join(".git/worktrees/wt/commondir"), "../..\n").unwrap();
    let wt = f.tmp.path().join("wt");
    std::fs::create_dir_all(wt.join("app/src")).unwrap();
    std::fs::create_dir_all(wt.join("other")).unwrap();
    std::fs::write(
        wt.join(".git"),
        format!("gitdir: {}\n", repo.join(".git/worktrees/wt").display()),
    )
    .unwrap();
    // git 이 워크트리마다 적는 되돌림 포인터(`pw_core::config::gitdir` 가 확인한다).
    std::fs::write(
        repo.join(".git/worktrees/wt/gitdir"),
        format!("{}\n", wt.join(".git").display()),
    )
    .unwrap();
    let root = repo.join("app");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::create_dir_all(f.tmp.path().join("repo/app-other")).unwrap();
    let id = project(&projects, &root);
    let runtime = f.runtime(Duration::from_secs(5));
    runtime.attach(RuntimeHost { projects, bus });
    let get = |dir: PathBuf| {
        runtime.get(&RuntimeGetParams {
            project_id: id.clone(),
            dir: Some(dir.display().to_string()),
        })
    };
    assert!(get(root.join("src")).is_ok());
    assert!(get(root.join("not-yet-created")).is_ok());
    assert!(get(wt.join("app/src")).is_ok());
    // 글자로는 루트로 시작하지만 정규화하면 루트 밖이다.
    for outside in [
        root.join("../app-other"),
        f.tmp.path().to_path_buf(),
        wt.join("other"),
        PathBuf::from("/"),
    ] {
        let err = get(outside.clone()).unwrap_err();
        assert_eq!(
            err.code,
            pw_rpc::code::INVALID_PARAMS,
            "{}",
            outside.display()
        );
        assert!(
            format!("{err:?}").contains("is not inside project"),
            "{err:?}"
        );
    }
}

/// 로그인 셸 위에 덮는 제공자 흉내(mise 자리). `fail` 이면 실패한다.
struct Overlay {
    calls: AtomicUsize,
    fail: bool,
}

impl RuntimeProvider for Overlay {
    fn id(&self) -> &'static str {
        "overlay"
    }
    fn runs_project_code(&self) -> bool {
        true
    }
    fn applies_to(&self, dir: &Path, base: &RuntimeEnv) -> bool {
        // 감지는 바탕(로그인 셸) 환경을 본다.
        dir.ends_with("project") && base.get("JAVA_HOME").is_some()
    }
    fn resolve(&self, _dir: &Path, base: &RuntimeEnv) -> Result<Layer, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.fail {
            return Err("overlay failed".to_owned());
        }
        let mut env = base.clone();
        env.vars.insert("NODE_VERSION".into(), "22".into());
        Ok(Layer {
            env,
            source: RuntimeSource::ConfigFiles {
                files: vec!["mise.toml".to_owned()],
            },
        })
    }
}

/// 프로젝트 코드를 돌리는 제공자는 신뢰한 프로젝트에서만 고른다. 고르면 로그인 셸 환경 위에 덮는다.
#[test]
fn providers_that_run_project_code_wait_for_trust() {
    let f = Fixture::new(WORKING);
    let overlay = Arc::new(Overlay {
        calls: AtomicUsize::new(0),
        fail: false,
    });
    let runtime = f
        .runtime(Duration::from_secs(5))
        .with_provider(overlay.clone())
        .with_trust(|project| project == "trusted");
    let untrusted = runtime.env("untrusted", &f.dir());
    assert_eq!(untrusted.info.provider, "shell");
    assert_eq!(untrusted.env.get("NODE_VERSION"), None);
    assert_eq!(overlay.calls.load(Ordering::SeqCst), 0);

    let trusted = runtime.env("trusted", &f.dir());
    assert_eq!(trusted.info.provider, "overlay");
    assert_eq!(trusted.info.status, RuntimeStatus::Ok);
    assert_eq!(trusted.env.get("NODE_VERSION"), Some(OsStr::new("22")));
    assert_eq!(
        trusted.env.get("JAVA_HOME"),
        Some(OsStr::new("/opt/jdk-21"))
    );
    assert!(trusted.login_shell);
    // 감지가 맞지 않는 디렉터리는 로그인 셸 그대로다.
    let elsewhere = runtime.env("trusted", &f.dir().join("docs"));
    assert_eq!(elsewhere.info.provider, "shell");
}

/// 제공자가 실패하면 로그인 셸 환경으로 물러나고 남기지 않는다(다음 요청이 제공자를 다시 부른다).
#[test]
fn a_failing_provider_falls_back_to_the_login_shell_without_caching() {
    let f = Fixture::new(WORKING);
    let overlay = Arc::new(Overlay {
        calls: AtomicUsize::new(0),
        fail: true,
    });
    let runtime = f
        .runtime(Duration::from_secs(5))
        .with_provider(overlay.clone())
        .with_trust(|_| true);
    let first = runtime.env("p", &f.dir());
    assert_eq!(first.info.provider, "overlay");
    assert_eq!(
        first.info.status,
        RuntimeStatus::Fallback {
            reason: "overlay failed".to_owned()
        }
    );
    assert!(matches!(
        first.info.source,
        RuntimeSource::LoginShell { .. }
    ));
    assert_eq!(first.env.get("JAVA_HOME"), Some(OsStr::new("/opt/jdk-21")));
    runtime.env("p", &f.dir());
    assert_eq!(overlay.calls.load(Ordering::SeqCst), 2);
}

/// 위조 `.git` 파일(관리 폴더 · `commondir` · 되돌림 포인터를 모두 제 폴더 안에 둔 것)로 신뢰된 프로젝트의 워크트리인 척하는
/// 폴더에서는 `runtime.get` 이 거절되고 제공자가 돌지 않는다(`pw_core::config::gitdir`).
#[test]
fn a_forged_worktree_of_a_trusted_project_does_not_run_its_provider() {
    let f = Fixture::new(WORKING);
    let bus = EventBus::new(DEFAULT_CAPACITY);
    let projects = registry(f.tmp.path(), &bus);
    let trusted = f.tmp.path().join("trusted");
    std::fs::create_dir_all(trusted.join(".git")).unwrap();
    let id = project(&projects, &trusted);
    let evil = f.tmp.path().join("downloads/evil");
    std::fs::create_dir_all(evil.join(".admin")).unwrap();
    std::fs::create_dir_all(evil.join("project")).unwrap();
    std::fs::write(evil.join(".git"), "gitdir: .admin\n").unwrap();
    std::fs::write(evil.join(".admin/gitdir"), "../.git\n").unwrap();
    let overlay = Arc::new(Overlay {
        calls: AtomicUsize::new(0),
        fail: false,
    });
    let runtime = f
        .runtime(Duration::from_secs(5))
        .with_provider(overlay.clone())
        .with_trust(|_| true);
    runtime.attach(RuntimeHost { projects, bus });
    for common in [
        trusted.join(".git").display().to_string(),
        "../../../trusted/.git".to_owned(),
    ] {
        std::fs::write(evil.join(".admin/commondir"), format!("{common}\n")).unwrap();
        let err = runtime
            .get(&RuntimeGetParams {
                project_id: id.clone(),
                dir: Some(evil.join("project").display().to_string()),
            })
            .unwrap_err();
        assert_eq!(err.code, pw_rpc::code::INVALID_PARAMS, "{common}");
    }
    assert_eq!(overlay.calls.load(Ordering::SeqCst), 0);
}
