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
use crate::methods::{RuntimeGetParams, RuntimeProblem, RuntimeSource, RuntimeStatus};
use crate::{
    Layer, LoginShell, ProviderError, Resolved, Runtime, RuntimeEnv, RuntimeHost, RuntimeProvider,
    TOPIC_CHANGED,
};

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
    runtime.attach(host(&projects, &bus, f.tmp.path()));
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
    let mut host_sub = bus.subscribe(Filter::all().scope(Scope::Host).topics([TOPIC_CHANGED]));
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
        matches!(&first.info.status, RuntimeStatus::Fallback { reason, problem: RuntimeProblem::LoginShell } if reason.contains("did not print its environment")),
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
    // 프로젝트 없는 창(설정 창의 `cli.status`)을 위해 Host 범위에도 한 번 나간다.
    assert_eq!(
        changed_events(&mut host_sub),
        vec![serde_json::json!({"reason": "loginShell"})]
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
        matches!(&first.info.status, RuntimeStatus::Fallback { reason, problem: RuntimeProblem::LoginShell } if reason.contains("printed no PATH")),
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

/// [`registry`] 와 같은 `$CONFIG` 자리를 보는 Host 부품.
fn host(projects: &Projects, bus: &EventBus, tmp: &Path) -> RuntimeHost {
    RuntimeHost {
        projects: projects.clone(),
        bus: bus.clone(),
        paths: AppPaths::under(&tmp.join("app")),
    }
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
    runtime.attach(host(&projects, &bus, f.tmp.path()));
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
    runtime.attach(host(&projects, &bus, f.tmp.path()));
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
    runtime.attach(host(&projects, &bus, f.tmp.path()));
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
    fn resolve(
        &self,
        _dir: &Path,
        base: &RuntimeEnv,
        _cancel: &crate::Cancel,
    ) -> Result<Layer, ProviderError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.fail {
            return Err("overlay failed".to_owned().into());
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
            reason: "overlay failed".to_owned(),
            problem: RuntimeProblem::Failed,
        }
    );
    assert!(matches!(
        first.info.source,
        RuntimeSource::LoginShell { .. }
    ));
    assert_eq!(first.env.get("JAVA_HOME"), Some(OsStr::new("/opt/jdk-21")));
    // 실패 직후에는 제공자를 다시 부르지 않는다(냉각). 「환경 다시 읽기」가 냉각을 풀면 다시 부른다.
    runtime.env("p", &f.dir());
    assert_eq!(overlay.calls.load(Ordering::SeqCst), 1);
    runtime.reload("p");
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
    runtime.attach(host(&projects, &bus, f.tmp.path()));
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

/// mise 제공자를 붙인 런타임. 가짜 로그인 셸은 `HOME` 과 PATH(가짜 `mise` 가 있는 `bin` 앞)를 정하고, 가짜 `mise` 는 부를 때마다
/// `calls.log` 에 `cwd` 를 한 줄 적고 PATH 앞에 `/mise/node/bin` 을 붙인 환경을 JSON 으로 찍는다.
struct MiseFixture {
    f: Fixture,
    bus: EventBus,
    projects: Projects,
    trusted: Arc<std::sync::atomic::AtomicBool>,
    runtime: Arc<Runtime>,
}

impl MiseFixture {
    fn new(with_mise: bool) -> Self {
        Self::with_script(
            with_mise.then_some(|tmp: &Path| {
                format!(
                    "printf '%s\\n' \"$PWD\" >> '{}'\nprintf '{{\"PATH\": \"/mise/node/bin:%s\", \"FROM_MISE\": \"yes\"}}' \"$PATH\"",
                    tmp.join("calls.log").display()
                )
            }),
            crate::Mise::default(),
            crate::runtime::FAILURE_COOLDOWN,
        )
    }

    /// 가짜 mise 의 본문을 시험이 정한다(`script` 는 임시 폴더를 받는다). 없으면 mise 가 PATH 에 없다.
    fn with_script(
        script: Option<impl FnOnce(&Path) -> String>,
        mise: crate::Mise,
        cooldown: Duration,
    ) -> Self {
        let f = Fixture::new("");
        let tmp = f.tmp.path().canonicalize().unwrap();
        let bin = tmp.join("bin");
        let home = tmp.join("home");
        std::fs::create_dir_all(&home).unwrap();
        f.rewrite(&format!(
            "HOME='{}'; export HOME\nPATH='{}:/usr/bin:/bin'; export PATH\neval \"$4\"",
            home.display(),
            bin.display()
        ));
        if let Some(script) = script {
            crate::mise::tests::write_mise(&bin, &script(&tmp));
        } else {
            std::fs::create_dir_all(&bin).unwrap();
        }
        let bus = EventBus::new(DEFAULT_CAPACITY);
        let projects = registry(f.tmp.path(), &bus);
        let trusted = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let gate = Arc::clone(&trusted);
        let runtime = Arc::new(
            f.runtime(Duration::from_secs(5))
                .with_provider(Arc::new(mise))
                .with_failure_cooldown(cooldown)
                .with_trust(move |_| gate.load(Ordering::SeqCst)),
        );
        runtime.attach(host(&projects, &bus, f.tmp.path()));
        Self {
            f,
            bus,
            projects,
            trusted,
            runtime,
        }
    }

    /// `home/<name>` 에 `files` 를 둔 프로젝트. (id, 실제 루트).
    fn project(&self, name: &str, files: &[(&str, &str)]) -> (String, PathBuf) {
        let root = self
            .f
            .tmp
            .path()
            .canonicalize()
            .unwrap()
            .join("home")
            .join(name);
        std::fs::create_dir_all(&root).unwrap();
        for (file, body) in files {
            let path = root.join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, body).unwrap();
        }
        (project(&self.projects, &root), root)
    }

    fn calls(&self) -> usize {
        std::fs::read_to_string(self.f.tmp.path().join("calls.log"))
            .map(|s| s.lines().count())
            .unwrap_or(0)
    }

    fn changes(&self, project: &str) -> pw_core::Subscription {
        self.bus.subscribe(
            Filter::all()
                .scope(Scope::Project(project.to_owned()))
                .topics([TOPIC_CHANGED]),
        )
    }
}

/// 미신뢰 프로젝트에서는 mise 를 실행하지 않는다(실행 기록 시험) — `mise.toml` 이 있어도 감지조차 하지 않고 로그인 셸 환경이다.
/// 신뢰하면 그 프로젝트의 디렉터리에서 mise 가 돌고 그 환경이 로그인 셸 환경 위에 덮인다.
#[test]
fn an_untrusted_project_never_runs_mise() {
    let m = MiseFixture::new(true);
    m.trusted.store(false, Ordering::SeqCst);
    let (id, root) = m.project("app", &[("mise.toml", "[tools]\nnode = \"22\"\n")]);
    let untrusted = m.runtime.env(&id, &root);
    assert_eq!(untrusted.info.provider, "shell");
    assert_eq!(untrusted.info.status, RuntimeStatus::Ok);
    assert_eq!(untrusted.env.get("FROM_MISE"), None);
    let info = m
        .runtime
        .get(&RuntimeGetParams {
            project_id: id.clone(),
            dir: None,
        })
        .unwrap();
    assert_eq!(info.provider, "shell");
    assert_eq!(m.calls(), 0, "mise ran for an untrusted project");

    m.trusted.store(true, Ordering::SeqCst);
    m.runtime.trust_changed(std::slice::from_ref(&id));
    let trusted = m.runtime.env(&id, &root);
    assert_eq!(trusted.info.provider, "mise");
    assert_eq!(trusted.info.status, RuntimeStatus::Ok);
    assert_eq!(
        trusted.info.source,
        RuntimeSource::ConfigFiles {
            files: vec![root.join("mise.toml").display().to_string()]
        }
    );
    assert_eq!(trusted.env.get("FROM_MISE"), Some(OsStr::new("yes")));
    let path = trusted.env.path.to_str().unwrap();
    assert!(path.starts_with("/mise/node/bin:"), "{path}");
    assert_eq!(m.calls(), 1);
    // 남긴다 — 같은 디렉터리는 다시 돌리지 않는다.
    m.runtime.env(&id, &root);
    assert_eq!(m.calls(), 1);
}

/// 버전 파일 변경: 그 프로젝트의 캐시만 잊고 `runtime.changed`(`files`)를 한 번 낸다. 다른 파일은 무시한다.
#[test]
fn a_version_file_change_forgets_only_that_project() {
    let m = MiseFixture::new(true);
    let (a, a_root) = m.project("a", &[("mise.toml", "")]);
    let (b, b_root) = m.project("b", &[("mise.toml", "")]);
    let mut a_events = m.changes(&a);
    let mut b_events = m.changes(&b);
    m.runtime.env(&a, &a_root);
    m.runtime.env(&b, &b_root);
    assert_eq!(m.calls(), 2);

    assert!(!m.runtime.files_changed(
        &a,
        &[a_root.join("src/main.rs"), a_root.join("Cargo.toml")],
        false
    ));
    m.runtime.env(&a, &a_root);
    assert_eq!(m.calls(), 2, "an unrelated file forgot the cache");
    assert_eq!(
        changed_events(&mut a_events),
        Vec::<serde_json::Value>::new()
    );

    assert!(
        m.runtime
            .files_changed(&a, &[a_root.join("mise.toml")], false)
    );
    assert_eq!(
        changed_events(&mut a_events),
        vec![serde_json::json!({"reason": "files"})]
    );
    m.runtime.env(&a, &a_root);
    m.runtime.env(&b, &b_root);
    assert_eq!(m.calls(), 3, "only project a runs mise again");
    assert_eq!(
        changed_events(&mut b_events),
        Vec::<serde_json::Value>::new()
    );

    // 놓친 변경(`rescan`)과 Pitwall 프로젝트 설정도 다시 계산한다.
    assert!(m.runtime.files_changed(&a, &[], true));
    assert!(
        m.runtime
            .files_changed(&a, &[a_root.join(".pitwall/project.toml")], false)
    );
    assert_eq!(changed_events(&mut a_events).len(), 2);
}

/// 신뢰한 프로젝트에 mise 설정이 있는데 로그인 셸 PATH 에 mise 가 없으면: 로그인 셸 환경 + 「설치되지 않음」 경고. 남기지 않는다.
#[test]
fn mise_missing_falls_back_with_a_not_installed_problem() {
    let m = MiseFixture::new(false);
    let (id, root) = m.project("app", &[("mise.toml", "")]);
    let resolved = m.runtime.env(&id, &root);
    assert_eq!(resolved.info.provider, "mise");
    assert!(matches!(
        resolved.info.source,
        RuntimeSource::LoginShell { .. }
    ));
    assert!(
        matches!(
            &resolved.info.status,
            RuntimeStatus::Fallback { problem: RuntimeProblem::NotInstalled, reason } if reason.contains("not on the login shell PATH")
        ),
        "{:?}",
        resolved.info.status
    );
    assert!(resolved.login_shell);
    assert_eq!(resolved.env.get("FROM_MISE"), None);
}

/// 프로젝트 설정 `[runtime] provider`(결정 M1-D15): `shell` 은 mise 를 돌리지 않고, `none` 은 앱 프로세스의 환경, `mise` 는
/// 설정 파일이 없어도 mise 를 부른다. 모르는 값은 `auto` 다.
#[test]
fn the_project_setting_picks_the_provider() {
    let m = MiseFixture::new(true);
    let setting = |value: &str| format!("[runtime]\nprovider = \"{value}\"\n");

    let (shell, root) = m.project(
        "shell",
        &[
            ("mise.toml", ""),
            (".pitwall/project.toml", &setting("shell")),
        ],
    );
    assert_eq!(m.runtime.env(&shell, &root).info.provider, "shell");
    assert_eq!(m.calls(), 0);

    let (none, root) = m.project(
        "none",
        &[
            ("mise.toml", ""),
            (".pitwall/project.toml", &setting("none")),
        ],
    );
    let resolved = m.runtime.env(&none, &root);
    assert_eq!(resolved.info.provider, "none");
    assert_eq!(resolved.info.source, RuntimeSource::Process);
    assert_eq!(resolved.env, RuntimeEnv::process());
    assert_eq!(m.calls(), 0);

    let (forced, root) = m.project("forced", &[(".pitwall/project.toml", &setting("mise"))]);
    assert_eq!(m.runtime.env(&forced, &root).info.provider, "mise");
    assert_eq!(m.calls(), 1);

    let (unknown, root) = m.project(
        "unknown",
        &[
            ("mise.toml", ""),
            (".pitwall/project.toml", &setting("direnv")),
        ],
    );
    assert_eq!(m.runtime.env(&unknown, &root).info.provider, "mise");
    assert_eq!(m.calls(), 2);

    // 설정을 바꾸면 파일 감시가 다시 계산하게 한다.
    std::fs::write(
        m.f.tmp
            .path()
            .canonicalize()
            .unwrap()
            .join("home/shell/.pitwall/project.toml"),
        setting("auto"),
    )
    .unwrap();
    let shell_root = m.f.tmp.path().canonicalize().unwrap().join("home/shell");
    assert!(
        m.runtime
            .files_changed(&shell, &[shell_root.join(".pitwall/project.toml")], false)
    );
    assert_eq!(m.runtime.env(&shell, &shell_root).info.provider, "mise");
}

/// 파일 감시 이벤트(`fs.changed`)를 따라가 `runtime.changed` 를 낸다. 이벤트를 놓치면 캐시가 있던 프로젝트마다 낸다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn follow_files_turns_fs_changed_into_runtime_changed() {
    let m = MiseFixture::new(true);
    let (id, root) = m.project("app", &[("mise.toml", "")]);
    let mut events = m.changes(&id);
    let follower = tokio::spawn(crate::follow_files(
        crate::subscribe_files(&m.bus),
        Arc::clone(&m.runtime),
    ));
    let fs_changed = |paths: Vec<String>, rescan: bool| {
        serde_json::json!({
            "root": root.display().to_string(),
            "worktree": null,
            "kinds": paths.iter().map(|_| "modified").collect::<Vec<_>>(),
            "paths": paths,
            "rescan": rescan,
        })
    };
    m.bus
        .publish(
            Scope::Project(id.clone()),
            crate::runtime::FS_CHANGED,
            &fs_changed(vec![root.join("README.md").display().to_string()], false),
        )
        .unwrap();
    m.bus
        .publish(
            Scope::Project(id.clone()),
            crate::runtime::FS_CHANGED,
            &fs_changed(vec![root.join("mise.toml").display().to_string()], false),
        )
        .unwrap();
    let event = tokio::time::timeout(Duration::from_secs(2), events.recv())
        .await
        .expect("runtime.changed within 2 s")
        .unwrap();
    let Received::Event(event) = event else {
        panic!("{event:?}")
    };
    assert_eq!(event.payload, serde_json::json!({"reason": "files"}));
    follower.abort();

    // 놓침: 용량 1 의 버스에서 따라가기 전에 이벤트 둘을 내면 구독자는 `Lagged` 를 먼저 받는다.
    let small = EventBus::new(1);
    let runtime = Arc::new(m.f.runtime(Duration::from_secs(5)));
    runtime.attach(host(&m.projects, &small, m.f.tmp.path()));
    runtime.env(&id, &root);
    let mut events = small.subscribe(
        Filter::all()
            .scope(Scope::Project(id.clone()))
            .topics([TOPIC_CHANGED]),
    );
    let files = crate::subscribe_files(&small);
    for _ in 0..2 {
        small
            .publish(
                Scope::Project("other".to_owned()),
                crate::runtime::FS_CHANGED,
                &fs_changed(vec![], false),
            )
            .unwrap();
    }
    let follower = tokio::spawn(crate::follow_files(files, Arc::clone(&runtime)));
    let event = tokio::time::timeout(Duration::from_secs(2), events.recv())
        .await
        .expect("runtime.changed after missed events")
        .unwrap();
    let Received::Event(event) = event else {
        panic!("{event:?}")
    };
    assert_eq!(event.payload, serde_json::json!({"reason": "files"}));
    follower.abort();
}

/// 멈추는 mise(부를 때마다 `calls.log` 에 한 줄 적고 오래 잔다).
fn hanging_mise(tmp: &Path) -> String {
    format!(
        "printf '%s\\n' \"$PWD\" >> '{}'\nexec sleep 30",
        tmp.join("calls.log").display()
    )
}

fn is_failed(resolved: &Resolved) -> bool {
    matches!(
        resolved.info.status,
        RuntimeStatus::Fallback {
            problem: RuntimeProblem::Failed,
            ..
        }
    )
}

/// 실패 냉각: mise 가 멈춰 시간 초과가 나면 한 번은 기다리지만, 그 뒤 30 s 동안은 mise 를 다시 부르지 않고 곧바로 물러난 환경
/// (로그인 셸)을 준다. 「환경 다시 읽기」 · 버전 파일 변경 · 신뢰 변경은 냉각을 풀어 바로 다시 시도한다. 실패 갈래는 그대로다.
#[test]
fn a_hanging_mise_is_not_waited_for_again_during_the_cooldown() {
    let m = MiseFixture::with_script(
        Some(hanging_mise),
        crate::Mise::new(Duration::from_millis(300)),
        crate::runtime::FAILURE_COOLDOWN,
    );
    let (id, root) = m.project("app", &[("mise.toml", "[tools]\nnode = \"22\"\n")]);
    let started = Instant::now();
    let first = m.runtime.env(&id, &root);
    assert!(started.elapsed() >= Duration::from_millis(300));
    assert!(is_failed(&first), "{:?}", first.info.status);
    assert_eq!(first.env.get("FROM_MISE"), None);
    assert_eq!(m.calls(), 1);

    for _ in 0..3 {
        let started = Instant::now();
        let again = m.runtime.env(&id, &root);
        assert!(
            started.elapsed() < Duration::from_millis(150),
            "waited {:?}",
            started.elapsed()
        );
        assert_eq!(again.info.status, first.info.status);
        assert_eq!(again.info.provider, "mise");
    }
    // 상태바의 `runtime.get` 도 같다.
    let info = m
        .runtime
        .get(&RuntimeGetParams {
            project_id: id.clone(),
            dir: None,
        })
        .unwrap();
    assert!(matches!(info.status, RuntimeStatus::Fallback { .. }));
    assert_eq!(m.calls(), 1, "mise ran again during the cooldown");

    // 「환경 다시 읽기」는 냉각을 푼다.
    m.runtime.reload(&id);
    m.runtime.env(&id, &root);
    assert_eq!(m.calls(), 2);
    m.runtime.env(&id, &root);
    assert_eq!(m.calls(), 2);

    // 버전 파일 변경.
    assert!(
        m.runtime
            .files_changed(&id, &[root.join("mise.toml")], false)
    );
    m.runtime.env(&id, &root);
    assert_eq!(m.calls(), 3);

    // 신뢰 변경.
    m.runtime.trust_changed(std::slice::from_ref(&id));
    m.runtime.env(&id, &root);
    assert_eq!(m.calls(), 4);

    // 파일 감시를 놓쳤을 때.
    m.runtime.files_missed();
    m.runtime.env(&id, &root);
    assert_eq!(m.calls(), 5);
}

/// 냉각은 프로젝트별이다: 한 프로젝트의 버전 파일 변경이 다른 프로젝트의 냉각을 풀지 않는다. 냉각이 지나면 다시 부른다.
#[test]
fn the_cooldown_is_per_project_and_expires() {
    let m = MiseFixture::with_script(
        Some(|tmp: &Path| {
            format!(
                "printf '%s\\n' \"$PWD\" >> '{}'\necho broken >&2\nexit 1",
                tmp.join("calls.log").display()
            )
        }),
        crate::Mise::default(),
        Duration::from_millis(400),
    );
    let (a, a_root) = m.project("a", &[("mise.toml", "")]);
    let (b, b_root) = m.project("b", &[("mise.toml", "")]);
    assert!(is_failed(&m.runtime.env(&a, &a_root)));
    assert!(is_failed(&m.runtime.env(&b, &b_root)));
    assert_eq!(m.calls(), 2);
    m.runtime
        .files_changed(&a, &[a_root.join("mise.toml")], false);
    m.runtime.env(&a, &a_root);
    m.runtime.env(&b, &b_root);
    assert_eq!(m.calls(), 3, "only a's cooldown ends");
    std::thread::sleep(Duration::from_millis(450));
    m.runtime.env(&b, &b_root);
    assert_eq!(m.calls(), 4, "the cooldown expired");
}

/// 미신뢰 · 미설치는 빠른 실패라 냉각하지 않는다(`mise trust` · 설치를 마치고 곧바로 다시 읽을 수 있어야 한다).
#[test]
fn quick_known_failures_are_not_cooled() {
    let m = MiseFixture::with_script(
        Some(|tmp: &Path| {
            format!(
                "printf '%s\\n' \"$PWD\" >> '{}'\necho 'Config files in x are not trusted' >&2\nexit 1",
                tmp.join("calls.log").display()
            )
        }),
        crate::Mise::default(),
        crate::runtime::FAILURE_COOLDOWN,
    );
    let (id, root) = m.project("app", &[("mise.toml", "")]);
    for expected in 1..=2 {
        let resolved = m.runtime.env(&id, &root);
        assert!(matches!(
            resolved.info.status,
            RuntimeStatus::Fallback {
                problem: RuntimeProblem::NotTrusted,
                ..
            }
        ));
        assert_eq!(m.calls(), expected);
    }
}

/// 연속 편집: 앞선 `mise env` 가 아직 도는 중에 같은 디렉터리의 새 시도가 시작되면 앞선 것을 세션째 끝낸다. 앞선 시도의 호출자는
/// 물러난 환경을 곧바로 받고, 마지막 시도의 결과만 남는다.
#[test]
fn a_new_attempt_ends_the_mise_still_running_for_the_same_directory() {
    let m = MiseFixture::with_script(
        Some(|tmp: &Path| {
            // 첫 호출은 오래 자고(pid 를 남긴다), 그다음부터는 바로 끝난다.
            format!(
                "n=$(cat '{pids}' 2>/dev/null | wc -l)\necho $$ >> '{pids}'\nprintf '%s\\n' \"$PWD\" >> '{calls}'\nif [ $n -ge 1 ]; then printf '{{\"PATH\": \"/mise/node/bin:%s\", \"FROM_MISE\": \"yes\"}}' \"$PATH\"; else exec sleep 30; fi",
                pids = tmp.join("pids").display(),
                calls = tmp.join("calls.log").display()
            )
        }),
        crate::Mise::new(Duration::from_secs(20)),
        crate::runtime::FAILURE_COOLDOWN,
    );
    let (id, root) = m.project("app", &[("mise.toml", "")]);
    let pids = m.f.tmp.path().canonicalize().unwrap().join("pids");
    let read_pids = || -> Vec<i32> {
        std::fs::read_to_string(&pids)
            .unwrap_or_default()
            .lines()
            .filter_map(|l| l.trim().parse().ok())
            .collect()
    };

    let old = {
        let (runtime, id, root) = (Arc::clone(&m.runtime), id.clone(), root.clone());
        std::thread::spawn(move || {
            let started = Instant::now();
            (runtime.env(&id, &root), started.elapsed())
        })
    };
    let deadline = Instant::now() + Duration::from_secs(5);
    while read_pids().is_empty() {
        assert!(Instant::now() < deadline, "the first mise never started");
        std::thread::sleep(Duration::from_millis(10));
    }
    let first_pid = read_pids()[0];

    // mise.toml 을 또 바꿨다: 캐시를 잊고 새 시도를 시작한다.
    m.runtime
        .files_changed(&id, &[root.join("mise.toml")], false);
    let latest = m.runtime.env(&id, &root);
    assert_eq!(latest.info.status, RuntimeStatus::Ok);
    assert_eq!(latest.env.get("FROM_MISE"), Some(OsStr::new("yes")));

    let (old_result, waited) = old.join().unwrap();
    assert!(waited < Duration::from_secs(10), "waited {waited:?}");
    assert!(is_failed(&old_result), "{:?}", old_result.info.status);
    let deadline = Instant::now() + Duration::from_secs(2);
    // SAFETY: 신호 0 은 프로세스가 있는지만 본다.
    while unsafe { libc::kill(first_pid, 0) } == 0 {
        assert!(
            Instant::now() < deadline,
            "the first mise {first_pid} survived"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(m.calls(), 2);
    // 마지막 결과만 남았다.
    assert_eq!(
        m.runtime.env(&id, &root).env.get("FROM_MISE"),
        Some(OsStr::new("yes"))
    );
    assert_eq!(m.calls(), 2);
}
