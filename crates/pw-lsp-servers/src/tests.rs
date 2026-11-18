//! 해석 순서 · 신뢰 · 설정 합치기를 가짜 폴더(가짜 실행 파일 · 가짜 `rustup`) · 가짜 런타임 환경으로 본다. 사람의 실제 서버 ·
//! rustup 은 부르지 않는다.

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use pw_core::bus::{DEFAULT_CAPACITY, EventBus, Scope};
use pw_core::config::{AppPaths, ConfigSource, ConfigStore};
use pw_core::registry::NewProject;
use pw_runtime::{LoginShell, RuntimeEnv};
use semver::Version;
use serde_json::json;

use super::*;
use crate::methods::LspServersListParams;

fn builtin() -> Arc<Registry> {
    Arc::new(Registry::from_roots(
        &Version::new(0, 0, 0),
        pw_ext::builtin::roots(),
    ))
}

fn def(id: &str) -> LanguageServerDecl {
    builtin().language_server(id).unwrap().1.clone()
}

fn executable(path: &Path, body: &str) -> PathBuf {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    path.to_path_buf()
}

fn env_with_path(dirs: &[&Path]) -> RuntimeEnv {
    let mut env = RuntimeEnv::default();
    env.path = std::env::join_paths(
        dirs.iter()
            .map(|d| d.to_path_buf())
            .chain([PathBuf::from("/usr/bin"), PathBuf::from("/bin")]),
    )
    .unwrap();
    env
}

/// 설치본 자리를 흉내 낸다. 패키지 · 실행 파일 이름은 둘 다 `.0` 이다.
struct FakeTools(&'static str, Located);

impl ToolLocator for FakeTools {
    fn locate(&self, package: &str, bin: &str) -> Located {
        assert_eq!((package, bin), (self.0, self.0));
        self.1.clone()
    }
}

/// 해석을 부르는 길 하나: (설정 `path`, `enabled`, 문맥) → 결과. 언어 서버와 디버그 어댑터가 같은 표를 지난다(17-m6-plan.md 메인
/// 판정 M6-P8 · M6-16 완료 기준 ② — 해석 순서를 바꾸면 둘 다 실패한다).
type Resolver<'a> = dyn Fn(Option<ConfiguredPath>, bool, &ResolveContext<'_>) -> LspResolution + 'a;

/// 언어 서버 `typescript-language-server`(내장 정의 그대로)의 해석.
fn lsp_resolver(
    ts: &LanguageServerDecl,
) -> impl Fn(Option<ConfiguredPath>, bool, &ResolveContext<'_>) -> LspResolution + '_ {
    move |path, enabled, ctx| {
        let mut config = settings::server_config(ts, None, None);
        config.path = path;
        config.enabled = enabled;
        resolve::resolve(ts, &config, ctx)
    }
}

/// 실행 파일이 있는 디버그 어댑터 정의(이름 · `mason` · 실행 파일이 `name`, 프로젝트 로컬은 `node_modules/.bin/<name>`).
fn adapter_def(name: &str) -> DebugAdapterDecl {
    let source = format!(
        "id = \"dev.example.dap\"\nname = \"D\"\nversion = \"1.0.0\"\nengines = {{ pitwall = \"*\" }}\n\
         [contributes.debug_adapters.{name}]\nlanguages = [\"javascript\"]\nmason = \"{name}\"\ncommand = \"{name}\"\n\
         project_local = \"node_modules/.bin/{name}\"\nlaunch = {{ program = \"{{program}}\" }}\n"
    );
    let (manifest, diagnostics) = pw_ext::manifest::parse_manifest(&source, "/x/extension.toml");
    assert_eq!(diagnostics, vec![]);
    manifest.unwrap().contributes.debug_adapters.remove(0)
}

/// 디버그 어댑터의 해석(`[dap.<id>]` 를 합친 값으로).
fn dap_resolver(
    adapter: &DebugAdapterDecl,
) -> impl Fn(Option<ConfiguredPath>, bool, &ResolveContext<'_>) -> LspResolution + '_ {
    move |path, enabled, ctx| {
        let mut config = dap::adapter_config(adapter, None, None);
        config.path = path;
        config.enabled = enabled;
        match dap::resolve_adapter(adapter, &config, ctx) {
            AdapterLaunch::Process(r) => r,
            AdapterLaunch::Disabled => LspResolution::Disabled,
            other => panic!("{other:?}"),
        }
    }
}

fn configured(value: &str, origin: LspSettingsOrigin) -> Option<ConfiguredPath> {
    Some(ConfiguredPath {
        value: value.to_owned(),
        origin,
    })
}

/// 해석의 다섯 단계(설정 경로 → 프로젝트 로컬 → PATH → 설치본 → 설치 필요)를 한 단계씩 지워 가며 본다 — 언어 서버와 디버그
/// 어댑터가 같은 표다.
#[test]
fn resolution_follows_settings_project_local_path_installed_then_not_installed() {
    let ts = def("typescript-language-server");
    resolution_order("typescript-language-server", &lsp_resolver(&ts));
}

/// 디버그 어댑터도 같은 표를 같은 함수로 지난다(17-m6-plan.md 메인 판정 M6-P8 · M6-16 완료 기준 ②).
#[test]
fn adapter_resolution_follows_the_same_order() {
    let adapter = adapter_def("js-debug-adapter");
    resolution_order("js-debug-adapter", &dap_resolver(&adapter));
}

fn resolution_order(name: &'static str, resolve: &Resolver<'_>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("project");
    let sub = root.join("packages/web");
    fs::create_dir_all(&sub).unwrap();
    let configured_exe = executable(&tmp.path().join("opt/my-exe"), "true");
    // 프로젝트 로컬은 실제 경로로 찾는다(macOS 의 /var → /private/var).
    let real = |p: PathBuf| fs::canonicalize(p).unwrap();
    let local = real(executable(
        &root.join("node_modules/.bin").join(name),
        "true",
    ));
    let nearest = real(executable(
        &sub.join("node_modules/.bin").join(name),
        "true",
    ));
    // 프로젝트 루트 밖(위)의 것은 프로젝트 로컬이 아니다.
    executable(&tmp.path().join("node_modules/.bin").join(name), "true");
    let on_path = executable(&tmp.path().join("path bin").join(name), "true");
    let installed = tmp.path().join("tools/bin").join(name);
    let env = env_with_path(&[&tmp.path().join("path bin")]);
    let tools = FakeTools(
        name,
        Located::Installed {
            path: installed.clone(),
            version: "5.1.3".into(),
        },
    );
    let ctx = |trusted| ResolveContext {
        trusted,
        project_root: &root,
        dir: &sub,
        env: &env,
        tools: &tools,
    };
    let found = |r: LspResolution| match r {
        LspResolution::Found {
            path,
            step,
            version,
            ..
        } => (path, step, version),
        other => panic!("{name}: {other:?}"),
    };

    // 1. 설정 경로가 무엇보다 먼저다.
    let path = configured(configured_exe.to_str().unwrap(), LspSettingsOrigin::Global);
    assert_eq!(
        found(resolve(path, true, &ctx(true))),
        (configured_exe.clone(), LspResolveStep::Settings, None)
    );
    // 2. 프로젝트 로컬 — 해석하는 디렉터리(packages/web)에서 프로젝트 루트까지 올라가며 가장 가까운 것.
    assert_eq!(
        found(resolve(None, true, &ctx(true))),
        (nearest.clone(), LspResolveStep::ProjectLocal, None)
    );
    fs::remove_file(&nearest).unwrap();
    assert_eq!(
        found(resolve(None, true, &ctx(true))),
        (local.clone(), LspResolveStep::ProjectLocal, None)
    );
    // 미신뢰 프로젝트는 node_modules/.bin 이 있어도 고르지 않는다.
    assert_eq!(
        found(resolve(None, true, &ctx(false))),
        (on_path.clone(), LspResolveStep::Path, None)
    );
    // 3. PATH.
    fs::remove_file(&local).unwrap();
    assert_eq!(
        found(resolve(None, true, &ctx(true))),
        (on_path.clone(), LspResolveStep::Path, None)
    );
    // 4. 설치본(판은 설치 기록의 것).
    fs::remove_file(&on_path).unwrap();
    assert_eq!(
        found(resolve(None, true, &ctx(true))),
        (installed, LspResolveStep::Installed, Some("5.1.3".into()))
    );
    // 5. 설치 필요. 설치본이 깨졌으면 그 까닭을 싣는다.
    let none = ResolveContext {
        tools: &NoTools,
        ..ctx(true)
    };
    assert_eq!(
        resolve(None, true, &none),
        LspResolution::NotInstalled {
            mason: Some(name.into()),
            reinstall: None,
            skipped: vec![],
        }
    );
    let broken = FakeTools(
        name,
        Located::Broken {
            reason: "python is gone".into(),
        },
    );
    assert_eq!(
        resolve(
            None,
            true,
            &ResolveContext {
                tools: &broken,
                ..ctx(true)
            }
        ),
        LspResolution::NotInstalled {
            mason: Some(name.into()),
            reinstall: Some("python is gone".into()),
            skipped: vec![],
        }
    );
}

/// 설정 경로는 절대 경로 · PATH 의 이름 · (프로젝트 설정만) 프로젝트 루트 기준 상대 경로다. 쓸 수 없으면 다음 단계로 가지 않는다
/// — 언어 서버와 디버그 어댑터가 같다.
#[test]
fn a_configured_path_is_used_or_reported_but_never_skipped() {
    let ts = def("typescript-language-server");
    configured_path_rules("typescript-language-server", &lsp_resolver(&ts));
}

#[test]
fn an_adapter_configured_path_is_used_or_reported_but_never_skipped() {
    let adapter = adapter_def("js-debug-adapter");
    configured_path_rules("js-debug-adapter", &dap_resolver(&adapter));
}

fn configured_path_rules(name: &str, resolve: &Resolver<'_>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("project");
    let in_project = executable(&root.join("tools/my-exe"), "true");
    let named = executable(&tmp.path().join("bin/my-named-exe"), "true");
    // PATH 에 정의의 이름이 있어도 틀린 설정 경로를 가리지 않는다.
    executable(&tmp.path().join("bin").join(name), "true");
    let env = env_with_path(&[&tmp.path().join("bin")]);
    let ctx = ResolveContext {
        trusted: true,
        project_root: &root,
        dir: &root,
        env: &env,
        tools: &NoTools,
    };
    let at = |value: &str, origin| resolve(configured(value, origin), true, &ctx);
    assert_eq!(
        at("my-named-exe", LspSettingsOrigin::Global).path(),
        Some(named.as_path())
    );
    assert_eq!(
        at("tools/my-exe", LspSettingsOrigin::Project).path(),
        Some(in_project.as_path())
    );
    let invalid = |value: &str, origin, reason: &str| LspResolution::InvalidPath {
        path: value.into(),
        origin,
        reason: reason.into(),
    };
    assert_eq!(
        at("tools/my-exe", LspSettingsOrigin::Global),
        invalid(
            "tools/my-exe",
            LspSettingsOrigin::Global,
            "a relative path in the global settings has nothing to be relative to; use an absolute path or a name on PATH"
        )
    );
    assert_eq!(
        at("/nowhere/my-exe", LspSettingsOrigin::Global),
        invalid(
            "/nowhere/my-exe",
            LspSettingsOrigin::Global,
            "not an executable file"
        )
    );
    assert_eq!(
        at("no-such-exe", LspSettingsOrigin::Project),
        invalid(
            "no-such-exe",
            LspSettingsOrigin::Project,
            "not found on PATH"
        )
    );
    // 꺼 두면 설정 경로가 있어도 찾지 않는다.
    assert_eq!(
        resolve(
            configured("my-named-exe", LspSettingsOrigin::Global),
            false,
            &ctx
        ),
        LspResolution::Disabled
    );
}

/// 가짜 `rustup`: `--version` 에는 `version` 을, `which <이름>` 에는 cwd 의 `has-component` 파일이 있으면 그 툴체인 실행 파일
/// 경로를, 없으면 rustup 과 같은 오류를 낸다. `which` 를 받으면 받은 `RUSTUP_AUTO_INSTALL` 을 `log` 파일에 적는다.
fn fake_rustup(bin: &Path, toolchain_ra: &Path, log: &Path, version: &str) -> PathBuf {
    executable(
        &bin.join("rustup"),
        &format!(
            "if [ \"$1\" = --version ]; then echo 'rustup {version} (d8c92c676 2026-10-21)'; exit 0; fi\n\
             printf '%s' \"${{RUSTUP_AUTO_INSTALL-unset}}\" > '{log}'\n\
             [ \"$1\" = which ] || exit 2\n\
             if [ -f has-component ]; then echo '{ra}'; exit 0; fi\n\
             echo \"error: '$2' is not installed for the toolchain 'stable-aarch64-apple-darwin'\" >&2\nexit 1",
            log = log.display(),
            ra = toolchain_ra.display()
        ),
    )
}

/// 컴포넌트 없는 rustup 프록시만 PATH 에 있으면 설치본 · 「설치 필요」 로 간다. 컴포넌트가 있으면 프록시를 쓴다. 프록시는 하드 링크 ·
/// 심볼릭 링크 둘 다 알아본다.
#[test]
fn a_rustup_proxy_is_used_only_when_the_component_is_installed() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("crate");
    fs::create_dir_all(&root).unwrap();
    let toolchain_ra = executable(
        &tmp.path().join("toolchains/stable/bin/rust-analyzer"),
        "true",
    );
    let ra = def("rust-analyzer");
    let config = settings::server_config(&ra, None, None);
    for link in ["hard", "symlink"] {
        let bin = tmp.path().join(format!("cargo-{link}/bin"));
        let log = tmp.path().join(format!("auto-install-{link}"));
        let rustup = fake_rustup(&bin, &toolchain_ra, &log, "1.29.2");
        let proxy = bin.join("rust-analyzer");
        if link == "hard" {
            fs::hard_link(&rustup, &proxy).unwrap();
        } else {
            std::os::unix::fs::symlink(&rustup, &proxy).unwrap();
        }
        let env = env_with_path(&[&bin]);
        let ctx = |trusted| ResolveContext {
            trusted,
            project_root: &root,
            dir: &root,
            env: &env,
            tools: &NoTools,
        };
        let without = resolve::resolve(&ra, &config, &ctx(true));
        assert_eq!(
            without,
            LspResolution::NotInstalled {
                mason: Some("rust-analyzer".into()),
                reinstall: None,
                skipped: vec![LspSkipped {
                    step: LspResolveStep::Path,
                    path: proxy.clone(),
                    reason: "a rustup proxy without the component: error: 'rust-analyzer' is not installed for the toolchain 'stable-aarch64-apple-darwin'".into(),
                }],
            },
            "{link}"
        );
        assert_eq!(fs::read_to_string(&log).unwrap(), "0", "{link}");
        // 미신뢰 프로젝트에서는 rustup 을 돌리지 않고 건너뛴다.
        fs::remove_file(&log).unwrap();
        let untrusted = resolve::resolve(&ra, &config, &ctx(false));
        assert!(
            matches!(&untrusted, LspResolution::NotInstalled { skipped, .. }
                if skipped[0].reason == "a rustup proxy; not checked in an untrusted project"),
            "{untrusted:?}"
        );
        assert!(!log.exists(), "{link}: rustup ran in an untrusted project");

        fs::write(root.join("has-component"), "").unwrap();
        assert_eq!(
            resolve::resolve(&ra, &config, &ctx(true)).path(),
            Some(proxy.as_path()),
            "{link}"
        );
        fs::remove_file(root.join("has-component")).unwrap();
    }
}

/// 프록시가 아닌 rust-analyzer(직접 받은 바이너리)는 rustup 없이 그대로 쓴다. 같은 폴더의 rustup 과 다른 파일이다.
#[test]
fn a_plain_rust_analyzer_on_path_is_used_as_is() {
    let tmp = tempfile::tempdir().unwrap();
    let bin = tmp.path().join("bin");
    let log = tmp.path().join("ran");
    fake_rustup(&bin, Path::new("/x"), &log, "1.29.2");
    let plain = executable(&bin.join("rust-analyzer"), "true");
    let ra = def("rust-analyzer");
    let env = env_with_path(&[&bin]);
    let resolution = resolve::resolve(
        &ra,
        &settings::server_config(&ra, None, None),
        &ResolveContext {
            trusted: true,
            project_root: tmp.path(),
            dir: tmp.path(),
            env: &env,
            tools: &NoTools,
        },
    );
    assert_eq!(resolution.path(), Some(plain.as_path()));
    assert!(!log.exists(), "rustup ran for a plain binary");
}

#[test]
fn a_rustup_which_that_hangs_is_cut_off() {
    let tmp = tempfile::tempdir().unwrap();
    let rustup = executable(&tmp.path().join("rustup"), "sleep 30");
    let started = std::time::Instant::now();
    let result = rustup::which(
        &rustup,
        "rust-analyzer",
        tmp.path(),
        &env_with_path(&[]),
        Duration::from_millis(300),
    );
    assert_eq!(
        result,
        Err("`rustup which rust-analyzer` did not finish in 300ms".into())
    );
    assert!(started.elapsed() < Duration::from_secs(5));
}

struct Fixture {
    tmp: tempfile::TempDir,
    servers: LspServers,
    projects: Projects,
    /// 가짜 로그인 셸 PATH 의 첫 폴더.
    bin: PathBuf,
}

impl Fixture {
    /// `settings` 는 전역 `settings.toml` 내용.
    fn new(settings: &str) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::under(&tmp.path().join("app"));
        fs::create_dir_all(&paths.config).unwrap();
        fs::write(paths.config.join("settings.toml"), settings).unwrap();
        let bus = EventBus::new(DEFAULT_CAPACITY);
        let global = ConfigStore::open(ConfigSource::global(&paths), bus.clone(), Scope::Host);
        assert_eq!(global.fault(), None);
        let projects = Projects::new(paths.clone(), None, bus);
        let bin = tmp.path().join("shell bin");
        fs::create_dir_all(&bin).unwrap();
        // 기기의 `JAVA_HOME`(CI 러너는 늘 있다)이 런타임 환경으로 새면 「java 가 아닌 서버는 JAVA_HOME 을 받지 않는다」를 가를 수 없다.
        let shell = executable(
            &tmp.path().join("fake-shell"),
            &format!(
                "PATH='{}:/usr/bin:/bin'; export PATH\nunset JAVA_HOME\nPW_TEST_SECRET='shell-s3cr3t'; export PW_TEST_SECRET\neval \"$4\"",
                bin.display()
            ),
        );
        // 시한이 짧으면 기기가 바쁠 때 앱 프로세스 환경으로 물러나 PATH 가 달라진다 — 넉넉히 둔다(가짜 셸은 곧바로 끝난다).
        let runtime = Runtime::new(LoginShell::new(Some(shell), Duration::from_secs(60)));
        let servers = LspServers::new(builtin(), Arc::new(runtime));
        servers.attach(HostParts {
            paths,
            global,
            projects: projects.clone(),
        });
        Self {
            tmp,
            servers,
            projects,
            bin,
        }
    }

    fn project(&self, files: &[(&str, &str)], trusted: bool) -> (String, PathBuf) {
        let root = self.tmp.path().join("project");
        for (path, content) in files {
            let path = root.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, content).unwrap();
        }
        fs::create_dir_all(&root).unwrap();
        let project = self
            .projects
            .create(NewProject {
                root: root.clone(),
                ..NewProject::default()
            })
            .unwrap();
        self.projects.set_trust(&project.id, trusted).unwrap();
        // 레지스트리는 실제 경로로 적는다(macOS 의 /var → /private/var).
        let root = project.root().to_path_buf();
        (project.id, root)
    }

    fn list(&self, project: &str) -> LspServerList {
        self.servers
            .list(&LspServersListParams {
                project_id: project.to_owned(),
                worktree: None,
            })
            .unwrap()
    }
}

fn server<'a>(list: &'a LspServerList, id: &str) -> &'a LspServerInfo {
    list.servers.iter().find(|s| s.id == id).unwrap()
}

/// 미신뢰 프로젝트에서는 공유 레이어(`.pitwall/project.toml`)의 `path` 를 읽지 않는다 — `.local.toml` 은 읽는다.
#[test]
fn an_untrusted_project_does_not_read_the_shared_layer() {
    let f = Fixture::new("schema = 1\n");
    let evil = executable(&f.tmp.path().join("evil/ra"), "true");
    let mine = executable(&f.tmp.path().join("mine/ruff"), "true");
    let evil_dap = executable(&f.tmp.path().join("evil/js-debug"), "true");
    let mine_dap = executable(&f.tmp.path().join("mine/codelldb"), "true");
    let shared = format!(
        "schema = 1\n[lsp.rust-analyzer]\npath = \"{}\"\nargs = [\"--evil\"]\n[dap.js-debug-adapter]\npath = \"{}\"\nargs = [\"--evil\"]\nenv = {{ EVIL = \"1\" }}\n",
        evil.display(),
        evil_dap.display()
    );
    let local = format!(
        "schema = 1\n[lsp.ruff]\npath = \"{}\"\n[dap.codelldb]\npath = \"{}\"\n",
        mine.display(),
        mine_dap.display()
    );
    let (id, _) = f.project(
        &[
            (".pitwall/project.toml", &shared),
            (".pitwall/project.local.toml", &local),
        ],
        false,
    );
    let list = f.list(&id);
    assert!(!list.trusted);
    let ra = server(&list, "rust-analyzer");
    assert!(
        matches!(ra.resolution, LspResolution::NotInstalled { .. }),
        "{:?}",
        ra.resolution
    );
    assert_eq!(ra.args, Vec::<String>::new());
    assert_eq!(
        server(&list, "ruff").resolution.path(),
        Some(mine.as_path())
    );

    // `[dap.<id>]` 도 같다(17-m6-plan.md M6-16 리뷰 — 공유 파일의 어댑터 경로 · 인자 · 환경은 미신뢰에서 읽지 않고, 개인 파일은 읽는다).
    // 내장 확장에 어댑터가 아직 없어(M6-19) 정의를 직접 준다.
    let js = adapter_def("js-debug-adapter");
    let lldb = adapter_def("codelldb");
    let dap = |trusted: bool| {
        let view = f.servers.view(&id, None).unwrap();
        assert_eq!(view.trusted, trusted);
        let root = view.root.clone();
        (
            f.servers
                .adapter_setup_in(&view, "dev.example.dap", &js, &root),
            f.servers
                .adapter_setup_in(&view, "dev.example.dap", &lldb, &root),
        )
    };
    let (js_setup, lldb_setup) = dap(false);
    assert_eq!(js_setup.config.path, None);
    assert_eq!(js_setup.config.args, Vec::<String>::new());
    assert!(!js_setup.config.env.contains_key("EVIL"));
    assert!(
        matches!(
            js_setup.launch,
            AdapterLaunch::Process(LspResolution::NotInstalled { .. })
        ),
        "{:?}",
        js_setup.launch
    );
    assert_eq!(
        lldb_setup.launch,
        AdapterLaunch::Process(found_at(&mine_dap))
    );

    f.projects.set_trust(&id, true).unwrap();
    let list = f.list(&id);
    let ra = server(&list, "rust-analyzer");
    assert_eq!(ra.resolution.path(), Some(evil.as_path()));
    assert_eq!(ra.args, ["--evil"]);
    let (js_setup, _) = dap(true);
    assert_eq!(js_setup.launch, AdapterLaunch::Process(found_at(&evil_dap)));
    assert_eq!(js_setup.config.args, ["--evil"]);
    assert_eq!(js_setup.config.env["EVIL"], "1");
}

fn git(dir: &Path, args: &[&str]) {
    let mut c = std::process::Command::new("git");
    for (name, _) in std::env::vars_os() {
        if name.to_string_lossy().starts_with("GIT_") {
            c.env_remove(name);
        }
    }
    let out = c
        .current_dir(dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", dir.join(".no-gitconfig"))
        .env("GIT_AUTHOR_NAME", "Pitwall Test")
        .env("GIT_AUTHOR_EMAIL", "test@pitwall.invalid")
        .env("GIT_COMMITTER_NAME", "Pitwall Test")
        .env("GIT_COMMITTER_EMAIL", "test@pitwall.invalid")
        .args(["-c", "commit.gpgsign=false"])
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// 미신뢰 클론에 **커밋된** `.pitwall/project.local.toml` 은 사람의 개인 파일이 아니다 — 언어 서버 · 디버그 어댑터의 실행 파일 ·
/// 인자 · 환경을 정하지 못한다(17-m6-plan m6-localpolicy). 고치기 전에는 미신뢰 읽기가 공유 레이어만 빼고 그 파일을 개인 레이어로
/// 읽어 저장소가 고른 실행 파일을 띄웠다. 추적을 풀면(사람이 그 파일을 자기 것으로 삼으면) 개인 파일이라 읽는다.
#[test]
fn an_untrusted_clone_cannot_pick_a_server_through_a_committed_local_file() {
    let f = Fixture::new("schema = 1\n");
    let evil = executable(&f.tmp.path().join("evil/ra"), "true");
    let evil_dap = executable(&f.tmp.path().join("evil/codelldb"), "true");
    let committed = format!(
        "schema = 1\n[lsp.rust-analyzer]\npath = \"{}\"\nargs = [\"--evil\"]\n[dap.codelldb]\npath = \"{}\"\nargs = [\"--evil\"]\nenv = {{ EVIL = \"1\" }}\n",
        evil.display(),
        evil_dap.display()
    );
    // 클론한 저장소: 개인 파일이 강제로 커밋되어 있다.
    let upstream = f.tmp.path().join("upstream");
    let file = upstream.join(".pitwall/project.local.toml");
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(&file, &committed).unwrap();
    git(&upstream, &["init", "-q"]);
    git(&upstream, &["add", "-f", ".pitwall/project.local.toml"]);
    git(
        &upstream,
        &["commit", "-q", "-m", "pick the language server"],
    );
    let clone = f.tmp.path().join("project");
    git(
        f.tmp.path(),
        &[
            "clone",
            "-q",
            upstream.to_str().unwrap(),
            clone.to_str().unwrap(),
        ],
    );
    let (id, _) = f.project(&[], false);

    let list = f.list(&id);
    assert!(!list.trusted);
    let ra = server(&list, "rust-analyzer");
    assert!(
        matches!(ra.resolution, LspResolution::NotInstalled { .. }),
        "an untrusted clone picked the language server: {:?}",
        ra.resolution
    );
    assert_eq!(ra.args, Vec::<String>::new());
    let view = f.servers.view(&id, None).unwrap();
    let lldb = adapter_def("codelldb");
    let setup = f
        .servers
        .adapter_setup_in(&view, "dev.example.dap", &lldb, &view.root.clone());
    assert_eq!(setup.config.path, None);
    assert_eq!(setup.config.args, Vec::<String>::new());
    assert!(!setup.config.env.contains_key("EVIL"));

    // 사람이 추적을 풀고 고치면 그 파일은 사람의 개인 파일이다.
    git(
        &clone,
        &["rm", "-q", "--cached", ".pitwall/project.local.toml"],
    );
    // 그 내용은 git 이 받아 온 것이라 고쳐야 사람의 파일이다(`pw_core::config::tracked` — 내용 판정).
    fs::write(
        clone.join(".pitwall/project.local.toml"),
        format!("{committed}# mine\n"),
    )
    .unwrap();
    let list = f.list(&id);
    let ra = server(&list, "rust-analyzer");
    assert_eq!(ra.resolution.path(), Some(evil.as_path()));
    assert_eq!(ra.args, ["--evil"]);
}

/// 설정 경로로 찾은 결과.
fn found_at(path: &Path) -> LspResolution {
    LspResolution::Found {
        path: path.to_path_buf(),
        step: LspResolveStep::Settings,
        version: None,
        skipped: vec![],
    }
}

/// `lsp.servers.list` 는 기본 번들 서버 여섯을 등록 순서로 내고, 전역 · 프로젝트 · 정의의 설정을 깊게 합친다. 모르는 서버의 설정은
/// 경고로 남는다.
#[test]
fn lists_the_default_bundle_with_merged_settings() {
    let f = Fixture::new(
        "schema = 1\n[lsp]\nidle_shutdown_minutes = 5\n\
         [lsp.basedpyright]\nenabled = false\nsettings = { basedpyright = { analysis = { typeCheckingMode = \"strict\", diagnosticMode = \"workspace\" } } }\n\
         [lsp.gopls]\nenabled = true\n",
    );
    executable(&f.bin.join("ruff"), "true");
    let (id, root) = f.project(
        &[(
            ".pitwall/project.toml",
            "schema = 1\n[lsp.basedpyright]\nenabled = true\nenv = { PYTHONPATH = \"src\" }\nsettings = { basedpyright = { analysis = { typeCheckingMode = \"basic\" } } }\nfeatures = { disable = [\"hover\"] }\n",
        )],
        true,
    );
    let list = f.list(&id);
    assert_eq!(list.problem, None);
    assert_eq!(
        list.warnings,
        ["`[lsp.gopls]`: no language server `gopls` is registered; ignored"]
    );
    let ids: Vec<&str> = list.servers.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "jdtls",
            "kotlin-lsp",
            "basedpyright",
            "ruff",
            "rust-analyzer",
            "typescript-language-server"
        ]
    );
    let pyright = server(&list, "basedpyright");
    assert_eq!(
        serde_json::to_value(pyright).unwrap(),
        json!({
            "id": "basedpyright",
            "extension": "dev.pitwall.lang-python",
            "languages": ["python"],
            "mason": "basedpyright",
            "command": "basedpyright-langserver",
            "args": ["--stdio"],
            "env": {"PYTHONPATH": "src"},
            "rootMarkers": ["pyproject.toml", "pyrightconfig.json", "setup.py", "setup.cfg", "requirements.txt"],
            "requires": [{"runtime": "python", "version": ">=3.10"}],
            "disabledFeatures": ["hover", "formatting"],
            "enabled": true,
            "settings": {"basedpyright": {"analysis": {"typeCheckingMode": "basic", "diagnosticMode": "workspace"}}},
            "initializationOptions": {},
            "installPrompt": true,
            "resolution": {"kind": "notInstalled", "mason": "basedpyright", "reinstall": null, "skipped": []},
        })
    );
    // 로그인 셸 PATH 의 것을 찾는다.
    assert_eq!(
        server(&list, "ruff").resolution,
        LspResolution::Found {
            path: f.bin.join("ruff"),
            step: LspResolveStep::Path,
            version: None,
            skipped: vec![],
        }
    );
    // 서버 관리자가 쓰는 길도 같은 값을 준다.
    let setup = f.servers.setup(&id, None, "basedpyright", &root).unwrap();
    assert_eq!(setup.config.env["PYTHONPATH"], "src");
    assert!(f.servers.setup(&id, None, "gopls", &root).is_err());
    assert!(
        f.servers.setup(&id, None, "ruff", f.tmp.path()).is_err(),
        "a directory outside the project root"
    );
}

/// TS 의 `tsserver.path` 는 신뢰한 프로젝트에 `node_modules/typescript/lib` 이 있을 때만 들어간다(결정 M4-D7).
#[test]
fn the_project_typescript_is_passed_only_in_a_trusted_project() {
    let f = Fixture::new("schema = 1\n");
    let (id, root) = f.project(&[("node_modules/typescript/lib/tsserver.js", "")], false);
    let options = |f: &Fixture| {
        server(&f.list(&id), "typescript-language-server")
            .initialization_options
            .clone()
    };
    assert_eq!(options(&f), json!({}));
    f.projects.set_trust(&id, true).unwrap();
    assert_eq!(
        options(&f),
        json!({"tsserver": {"path": root.join("node_modules/typescript/lib").display().to_string()}})
    );
}

/// 설정이 바뀐 서버만 「다시 시작 필요」 대상이다.
#[test]
fn changed_servers_are_the_ones_whose_merged_config_differs() {
    let f = Fixture::new("schema = 1\n");
    let (id, root) = f.project(&[], true);
    let before = f.servers.configs(&id, None).unwrap();
    fs::create_dir_all(root.join(".pitwall")).unwrap();
    fs::write(
        root.join(".pitwall/project.toml"),
        "schema = 1\n[lsp.ruff]\nargs = [\"server\", \"--preview\"]\n[lsp.jdtls]\nenabled = true\n",
    )
    .unwrap();
    let after = f.servers.configs(&id, None).unwrap();
    assert_eq!(changed_servers(&before, &after), ["ruff"]);
    assert_eq!(changed_servers(&before, &before), Vec::<String>::new());
}

/// `setup` 의 `dir` 은 프로젝트 루트 안의 있는 폴더여야 한다 — `..` · `.` 마디, 루트 밖을 가리키는 링크, 없는 폴더는 거절한다.
/// 루트의 부모에 있는 `node_modules` 는 프로젝트 로컬로 고르지 않는다.
#[test]
fn setup_rejects_a_directory_outside_the_project_root() {
    let f = Fixture::new("schema = 1\n");
    let (id, root) = f.project(&[("a/b/keep", "")], true);
    let parent = root.parent().unwrap();
    executable(
        &parent.join("node_modules/.bin/typescript-language-server"),
        "true",
    );
    fs::create_dir_all(parent.join("node_modules/typescript/lib")).unwrap();
    let elsewhere = f.tmp.path().join("elsewhere");
    fs::create_dir_all(&elsewhere).unwrap();
    std::os::unix::fs::symlink(&elsewhere, root.join("link")).unwrap();
    let setup = |dir: PathBuf| {
        f.servers
            .setup(&id, None, "typescript-language-server", &dir)
    };
    for (dir, reason) in [
        (root.join(".."), "has a `..` or `.` segment"),
        (root.join("a/../.."), "has a `..` or `.` segment"),
        (root.join("a/./b"), "has a `..` or `.` segment"),
        (root.join("link"), "is not inside the project root"),
        (root.join("missing"), "does not exist"),
        (root.join("a/b/keep"), "is not a folder"),
        (PathBuf::from("a/b"), "is not an absolute path"),
    ] {
        // 성공한 값은 서버 환경 전체를 담아 출력하지 않는다(사람의 환경 변수가 시험 출력에 남는다).
        let Err(error) = setup(dir.clone()) else {
            panic!("{}: accepted", dir.display());
        };
        let detail = error
            .data
            .as_ref()
            .and_then(|d| d.as_str())
            .unwrap_or_default();
        assert!(detail.contains(reason), "{}: {detail}", dir.display());
    }
    let inside = setup(root.join("a/b")).unwrap();
    assert!(
        matches!(inside.resolution, LspResolution::NotInstalled { .. }),
        "{:?}",
        inside.resolution
    );
    assert_eq!(inside.initialization_options, json!({}));
}

/// 해석 함수를 직접 불러도(`ResolveContext` 는 공개다) 올라가기는 실제 경로로 루트 안에서만 한다.
#[test]
fn project_local_lookup_never_climbs_above_the_real_root() {
    let tmp = tempfile::tempdir().unwrap();
    // 실제 경로로 시작한다 — macOS 의 /var → /private/var 가 어휘 비교를 우연히 막아 되돌림을 가리지 않게.
    let base = fs::canonicalize(tmp.path()).unwrap();
    let root = base.join("project");
    fs::create_dir_all(root.join("a")).unwrap();
    let name = "typescript-language-server";
    executable(&base.join("node_modules/.bin").join(name), "true");
    let elsewhere = base.join("elsewhere");
    executable(&elsewhere.join("node_modules/.bin").join(name), "true");
    std::os::unix::fs::symlink(&elsewhere, root.join("link")).unwrap();
    let ts = def(name);
    let config = settings::server_config(&ts, None, None);
    let env = env_with_path(&[]);
    for dir in [root.join(".."), root.join("a/../.."), root.join("link")] {
        let resolution = resolve::resolve(
            &ts,
            &config,
            &ResolveContext {
                trusted: true,
                project_root: &root,
                dir: &dir,
                env: &env,
                tools: &NoTools,
            },
        );
        assert!(
            matches!(resolution, LspResolution::NotInstalled { .. }),
            "{}: {resolution:?}",
            dir.display()
        );
    }
}

/// 서버 환경은 런타임 환경 위에 정의 · 설정의 `env` 를 덮은 것이고, `requires` 로 찾은 런타임은 PATH 맨 앞 · `JAVA_HOME` 으로
/// 들어간다(메인 판정 2026-11-02). 사람이 `[lsp.<id>] env` 에 적은 `JAVA_HOME` 은 지킨다.
#[test]
fn the_server_environment_puts_the_found_runtime_first() {
    let f = Fixture::new("schema = 1\n");
    let (id, root) = f.project(
        &[(
            ".pitwall/project.toml",
            "schema = 1\n[lsp.jdtls]\nenv = { JDTLS_LOG = \"1\" }\n[lsp.kotlin-lsp]\nenv = { JAVA_HOME = \"/mine\" }\n",
        )],
        true,
    );
    let jdk = FoundRuntime {
        runtime: LspServerRuntime::Java,
        home: PathBuf::from("/jdks/21"),
        bin: PathBuf::from("/jdks/21/bin"),
    };
    let mut jdtls = f.servers.setup(&id, None, "jdtls", &root).unwrap();
    assert_eq!(jdtls.env.get("JDTLS_LOG"), Some(std::ffi::OsStr::new("1")));
    let before: Vec<PathBuf> = std::env::split_paths(&jdtls.env.path).collect();
    assert_eq!(before[0], f.bin, "the runtime environment PATH");
    jdtls.with_runtime(&jdk);
    let after: Vec<PathBuf> = std::env::split_paths(&jdtls.env.path).collect();
    assert_eq!(after[0], PathBuf::from("/jdks/21/bin"));
    assert_eq!(&after[1..], &before[..]);
    assert_eq!(
        jdtls.env.get("JAVA_HOME"),
        Some(std::ffi::OsStr::new("/jdks/21"))
    );
    // 두 번 넣어도 PATH 에 한 번이다.
    jdtls.with_runtime(&jdk);
    assert_eq!(std::env::split_paths(&jdtls.env.path).count(), after.len());

    let mut kotlin = f.servers.setup(&id, None, "kotlin-lsp", &root).unwrap();
    kotlin.with_runtime(&jdk);
    assert_eq!(
        kotlin.env.get("JAVA_HOME"),
        Some(std::ffi::OsStr::new("/mine"))
    );
    // 프로젝트 런타임 환경(프로젝트 JDK 를 찾는 곳)에는 서버용으로 적은 env 가 들지 않는다(M4-18).
    assert_ne!(
        kotlin.project_env.get("JAVA_HOME"),
        Some(std::ffi::OsStr::new("/mine"))
    );
    assert_eq!(
        std::env::split_paths(&kotlin.project_env.path).collect::<Vec<_>>(),
        before
    );
    // node · python 은 PATH 만.
    let mut ts = f
        .servers
        .setup(&id, None, "typescript-language-server", &root)
        .unwrap();
    ts.with_runtime(&FoundRuntime {
        runtime: LspServerRuntime::Node,
        home: PathBuf::from("/node/22"),
        bin: PathBuf::from("/node/22/bin"),
    });
    assert_eq!(
        std::env::split_paths(&ts.env.path).next(),
        Some(PathBuf::from("/node/22/bin"))
    );
    assert_eq!(ts.env.get("JAVA_HOME"), None);
}

/// `RUSTUP_AUTO_INSTALL` 을 모르는 옛 rustup(1.28.1 전)이면 `rustup which` 를 돌리지 않고 그 프록시를 건너뛴다.
#[test]
fn an_old_rustup_is_not_asked_and_its_proxy_is_skipped() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("crate");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("has-component"), "").unwrap();
    let bin = tmp.path().join("cargo/bin");
    let log = tmp.path().join("which-ran");
    let toolchain_ra = executable(&tmp.path().join("tc/rust-analyzer"), "true");
    let rustup = fake_rustup(&bin, &toolchain_ra, &log, "1.27.1");
    std::os::unix::fs::symlink(&rustup, bin.join("rust-analyzer")).unwrap();
    let ra = def("rust-analyzer");
    let env = env_with_path(&[&bin]);
    let resolution = resolve::resolve(
        &ra,
        &settings::server_config(&ra, None, None),
        &ResolveContext {
            trusted: true,
            project_root: &root,
            dir: &root,
            env: &env,
            tools: &NoTools,
        },
    );
    assert!(
        matches!(&resolution, LspResolution::NotInstalled { skipped, .. }
            if skipped[0].reason == "a rustup proxy; rustup 1.27.1 is older than 1.28.1 and may install a toolchain while checking, so it is not checked"),
        "{resolution:?}"
    );
    assert!(!log.exists(), "rustup which ran on an old rustup");
}

/// `ServerSetup` · `ServerConfig` 의 `Debug` 에는 비밀 값이 없다 — 런타임 환경의 변수 값, 설정 `env` 의 값, `settings` 의 값 모두.
/// 이름 · 키는 남는다(무엇이 있는지는 보이게).
#[test]
fn debug_output_of_a_setup_has_no_secret_values() {
    let f = Fixture::new("schema = 1\n");
    let (id, root) = f.project(
        &[(
            ".pitwall/project.toml",
            "schema = 1\n[lsp.ruff]\nenv = { RUFF_TOKEN = \"env-s3cr3t\" }\nsettings = { auth = { token = \"settings-s3cr3t\" } }\n",
        )],
        true,
    );
    let setup = f.servers.setup(&id, None, "ruff", &root).unwrap();
    // 비밀이 실제로 담겨 있다(그래야 아래 단언이 뜻이 있다).
    assert_eq!(
        setup.env.get("PW_TEST_SECRET"),
        Some(std::ffi::OsStr::new("shell-s3cr3t"))
    );
    assert_eq!(setup.config.env["RUFF_TOKEN"], "env-s3cr3t");
    for shown in [format!("{setup:?}"), format!("{setup:#?}")] {
        for secret in ["shell-s3cr3t", "env-s3cr3t", "settings-s3cr3t"] {
            assert!(!shown.contains(secret), "`{secret}` leaked into Debug");
        }
        for name in ["PW_TEST_SECRET", "RUFF_TOKEN", "auth", "ruff"] {
            assert!(shown.contains(name), "`{name}` missing from Debug");
        }
    }
    let configs = f.servers.configs(&id, None).unwrap();
    let shown = format!("{configs:?}");
    assert!(!shown.contains("env-s3cr3t") && !shown.contains("settings-s3cr3t"));
}

/// 포맷 답을 믿지 않을 진행(M4-13 재리뷰 N1): 내장 정의 중 rust-analyzer 만 `Roots Scanned` 를 적는다 — 다른 서버는 진행이
/// 있어도 포맷을 막지 않는다(빈 목록이 기본).
#[test]
fn only_rust_analyzer_marks_a_progress_that_makes_its_format_answers_unreliable() {
    let registry = builtin();
    let marked: Vec<(&str, &[String])> = registry
        .language_servers()
        .map(|(_, d)| (d.id.as_str(), d.format_unreliable_while_progress.as_slice()))
        .filter(|(_, titles)| !titles.is_empty())
        .collect();
    assert_eq!(
        marked,
        [("rust-analyzer", &["Roots Scanned".to_owned()][..])]
    );
    assert!(registry.language_servers().count() > 1);
}

/// 프로젝트 JDK 는 정의의 `project_jdk_setting`(jdtls `java.configuration.runtimes`) 경로에 합친 설정 · `initializationOptions.settings`
/// 두 곳에 같은 값으로 들어간다(15-m4-plan.md M4-18, 결정 M4-D6). 못 찾으면 빈 목록, 사람이 적은 값은 지킨다, 키가 없는 정의는
/// 그대로다.
#[test]
fn the_project_jdk_goes_to_the_named_setting_in_both_places() {
    let f = Fixture::new("schema = 1\n");
    let (id, root) = f.project(&[], true);
    let at = |v: &serde_json::Value, path: &str| -> serde_json::Value {
        path.split('.')
            .try_fold(v, |v, k| v.get(k))
            .cloned()
            .unwrap_or(serde_json::Value::Null)
    };
    let jdk17 = ProjectJdk {
        home: PathBuf::from("/jdks/17"),
        major: 17,
    };
    assert_eq!(
        ProjectJdk {
            home: PathBuf::from("/jdks/8"),
            major: 8
        }
        .execution_environment(),
        "JavaSE-1.8"
    );

    let mut jdtls = f.servers.setup(&id, None, "jdtls", &root).unwrap();
    jdtls.with_project_jdk(Some(&jdk17));
    let want = serde_json::json!([{ "name": "JavaSE-17", "path": "/jdks/17", "default": true }]);
    assert_eq!(
        at(&jdtls.config.settings, "java.configuration.runtimes"),
        want
    );
    assert_eq!(
        at(
            &jdtls.initialization_options,
            "settings.java.configuration.runtimes"
        ),
        want
    );

    // 못 찾았으면 빈 목록(서버 JDK 를 내세우지 않는다).
    let mut none = f.servers.setup(&id, None, "jdtls", &root).unwrap();
    none.with_project_jdk(None);
    assert_eq!(
        at(&none.config.settings, "java.configuration.runtimes"),
        serde_json::json!([])
    );
    assert_eq!(
        at(
            &none.initialization_options,
            "settings.java.configuration.runtimes"
        ),
        serde_json::json!([])
    );

    // 키가 없는 정의는 그대로다.
    let mut kotlin = f.servers.setup(&id, None, "kotlin-lsp", &root).unwrap();
    let (settings, options) = (
        kotlin.config.settings.clone(),
        kotlin.initialization_options.clone(),
    );
    kotlin.with_project_jdk(Some(&jdk17));
    assert_eq!(
        (kotlin.config.settings, kotlin.initialization_options),
        (settings, options)
    );
}

/// 사람이 `[lsp.jdtls] settings` 에 `java.configuration.runtimes` 를 적었으면 찾은 JDK 대신 그 값을 두 곳에 쓴다.
#[test]
fn a_configured_runtimes_list_wins_over_the_found_jdk() {
    let f = Fixture::new("schema = 1\n");
    let (id, root) = f.project(
        &[(
            ".pitwall/project.toml",
            "schema = 1\n[lsp.jdtls.settings.java.configuration]\nruntimes = [{ name = \"JavaSE-11\", path = \"/mine/11\" }]\n",
        )],
        true,
    );
    let mut jdtls = f.servers.setup(&id, None, "jdtls", &root).unwrap();
    jdtls.with_project_jdk(Some(&ProjectJdk {
        home: PathBuf::from("/jdks/17"),
        major: 17,
    }));
    let want = serde_json::json!([{ "name": "JavaSE-11", "path": "/mine/11" }]);
    assert_eq!(
        jdtls.config.settings["java"]["configuration"]["runtimes"],
        want
    );
    assert_eq!(
        jdtls.initialization_options["settings"]["java"]["configuration"]["runtimes"],
        want
    );
}

/// java-debug 처럼 언어 서버가 여는 어댑터(`start`, `bundles`).
fn start_adapter(id: &str, server: &str) -> DebugAdapterDecl {
    let source = format!(
        "id = \"dev.example.dap\"\nname = \"D\"\nversion = \"1.0.0\"\nengines = {{ pitwall = \"*\" }}\n\
         [contributes.debug_adapters.{id}]\nlanguages = [\"java\"]\nmason = \"java-debug-adapter\"\n\
         start = {{ lsp = \"{server}\", command = \"vscode.java.startDebugSession\", bundles = [\"java-debug-adapter/a.jar\", \"java-debug-adapter/b.jar\"] }}\n\
         attach = {{ port = \"{{port}}\" }}\n"
    );
    let (manifest, diagnostics) = pw_ext::manifest::parse_manifest(&source, "/x/extension.toml");
    assert_eq!(diagnostics, vec![]);
    manifest.unwrap().contributes.debug_adapters.remove(0)
}

/// `[dap.<id>]`: `enabled` · `path` · `args` · `env` 만 받고(언어 서버의 키는 경고), 프로젝트가 전역을 키마다 이기고, `env` 는
/// 변수마다 덮는다. 등록되지 않은 어댑터 id 는 경고한다. 합친 값으로 정의의 실행 파일을 찾고, 어댑터 환경에 설정 `env` 가 든다.
#[test]
fn adapter_settings_merge_and_resolve_like_language_servers() {
    let f = Fixture::new(
        "schema = 1\n[dap.js-debug-adapter]\npath = \"/nowhere/js-debug\"\nargs = [\"--global\"]\nenv = { A = \"global\", B = \"global\" }\nsettings = { x = 1 }\n",
    );
    let (id, root) = f.project(
        &[(
            ".pitwall/project.toml",
            "schema = 1\n[dap.js-debug-adapter]\nargs = [\"{port}\", \"{host}\"]\nenv = { B = \"project\" }\nfeatures = { disable = [\"hover\"] }\n[dap.ghost]\nenabled = false\n",
        )],
        true,
    );
    let view = f.servers.view(&id, None).unwrap();
    assert_eq!(
        view.warnings,
        [
            "settings: `[dap.js-debug-adapter]`: unknown key `settings` is ignored",
            "project: `[dap.js-debug-adapter]`: unknown key `features` is ignored",
            "`[dap.ghost]`: no debug adapter `ghost` is registered; ignored",
            "`[dap.js-debug-adapter]`: no debug adapter `js-debug-adapter` is registered; ignored",
        ]
    );
    let adapter = adapter_def("js-debug-adapter");
    let setup = f
        .servers
        .adapter_setup_in(&view, "dev.example.dap", &adapter, &root);
    assert_eq!(setup.config.args, ["{port}", "{host}"]);
    assert_eq!(
        setup.config.env,
        BTreeMap::from([
            ("A".to_owned(), "global".to_owned()),
            ("B".to_owned(), "project".to_owned())
        ])
    );
    assert_eq!(setup.env.get("B"), Some(std::ffi::OsStr::new("project")));
    // 전역의 `path` 가 쓸 수 없는 경로라 다음 단계로 가지 않는다(언어 서버와 같은 규칙).
    assert_eq!(
        setup.launch,
        AdapterLaunch::Process(LspResolution::InvalidPath {
            path: "/nowhere/js-debug".into(),
            origin: LspSettingsOrigin::Global,
            reason: "not an executable file".into(),
        })
    );
    // Debug 에 env 값이 나오지 않는다.
    let shown = format!("{setup:?}");
    assert!(
        !shown.contains("global\"") && !shown.contains("project\""),
        "{shown}"
    );
}

/// 언어 서버가 여는 어댑터는 실행 파일을 찾지 않고 그 서버 · 명령을 낸다. 사람이 `[dap.<id>] path` 를 적었으면 그 실행 파일이다.
#[test]
fn a_start_adapter_names_its_language_server_unless_a_path_is_set() {
    let tmp = tempfile::tempdir().unwrap();
    let exe = executable(&tmp.path().join("java-debug"), "true");
    let env = env_with_path(&[]);
    let ctx = ResolveContext {
        trusted: true,
        project_root: tmp.path(),
        dir: tmp.path(),
        env: &env,
        tools: &NoTools,
    };
    let adapter = start_adapter("java-debug", "jdtls");
    let mut config = dap::adapter_config(&adapter, None, None);
    assert_eq!(
        dap::resolve_adapter(&adapter, &config, &ctx),
        AdapterLaunch::LanguageServer {
            server: "jdtls".into(),
            command: "vscode.java.startDebugSession".into()
        }
    );
    config.path = configured(exe.to_str().unwrap(), LspSettingsOrigin::Global);
    assert!(matches!(
        dap::resolve_adapter(&adapter, &config, &ctx),
        AdapterLaunch::Process(LspResolution::Found {
            step: LspResolveStep::Settings,
            ..
        })
    ));
    config.enabled = false;
    assert_eq!(
        dap::resolve_adapter(&adapter, &config, &ctx),
        AdapterLaunch::Disabled
    );
}

/// 가짜 언어 서버: `server` 가 떠 있으면 명령에 `answer` 로 답한다.
struct FakeServers {
    running: &'static str,
    answer: Result<serde_json::Value, String>,
    asked: std::sync::Mutex<Vec<(String, String)>>,
}

impl LspCommands for FakeServers {
    fn execute_command<'a>(
        &'a self,
        _project_id: &'a str,
        _root: &'a std::path::Path,
        server: &'a str,
        command: &'a str,
        _arguments: Vec<serde_json::Value>,
    ) -> dap::CommandAnswer<'a> {
        self.asked
            .lock()
            .unwrap()
            .push((server.to_owned(), command.to_owned()));
        let answer = (server == self.running).then(|| self.answer.clone());
        Box::pin(async move { answer })
    }
}

/// 완료 기준 ⑦: `start.lsp` 어댑터는 떠 있는 언어 서버에 명령을 보내 받은 포트(`127.0.0.1`)로 붙는다. 그 서버가 떠 있지 않으면
/// 「떠야 한다」 로 거절하고, 포트가 아닌 답 · 서버의 거절은 그대로 알린다.
#[tokio::test]
async fn a_start_adapter_connects_to_the_port_its_language_server_answers() {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    let listener = tokio::net::TcpListener::bind((pw_ext::ADAPTER_HOST, 0))
        .await
        .unwrap();
    let port = listener.local_addr().unwrap().port();
    let adapter = start_adapter("java-debug", "jdtls");
    let start = adapter.start.as_ref().unwrap();
    let servers = FakeServers {
        running: "jdtls",
        answer: Ok(json!(port)),
        asked: Default::default(),
    };
    let (got, mut stream) = dap::connect_via_language_server(
        "p",
        std::path::Path::new("/p"),
        start,
        &servers,
        Duration::from_secs(5),
    )
    .await
    .unwrap();
    assert_eq!(got, port);
    assert_eq!(
        *servers.asked.lock().unwrap(),
        [(
            "jdtls".to_owned(),
            "vscode.java.startDebugSession".to_owned()
        )]
    );
    let (mut accepted, _) = listener.accept().await.unwrap();
    stream
        .write_all(b"Content-Length: 2\r\n\r\n{}")
        .await
        .unwrap();
    let mut buf = [0u8; 23];
    accepted.read_exact(&mut buf).await.unwrap();
    assert_eq!(&buf, b"Content-Length: 2\r\n\r\n{}");

    let connect = |running: &'static str, answer| {
        let servers = FakeServers {
            running,
            answer,
            asked: Default::default(),
        };
        async move {
            dap::connect_via_language_server(
                "p",
                std::path::Path::new("/p"),
                start,
                &servers,
                Duration::from_secs(5),
            )
            .await
            .map(|(port, _)| port)
        }
    };
    let not_running = connect("kotlin-lsp", Ok(json!(port))).await.unwrap_err();
    assert_eq!(
        not_running,
        AdapterStartError::NotRunning {
            server: "jdtls".into()
        }
    );
    assert_eq!(
        not_running.to_string(),
        "the language server `jdtls` must be running to start this debug adapter — open a file it handles first"
    );
    for answer in [json!(0), json!(70000), json!("5005"), json!(null)] {
        assert!(
            matches!(
                connect("jdtls", Ok(answer.clone())).await,
                Err(AdapterStartError::BadPort { .. })
            ),
            "{answer}"
        );
    }
    assert_eq!(
        connect("jdtls", Err("no debug session".into())).await,
        Err(AdapterStartError::Command {
            server: "jdtls".into(),
            message: "no debug session".into()
        })
    );
}

/// 설치본의 `share` 를 흉내 낸다: `java-debug-adapter` 의 `a.jar` 만 있다.
struct ShareTools;

impl ToolLocator for ShareTools {
    fn locate(&self, _package: &str, _bin: &str) -> Located {
        Located::Absent
    }

    fn locate_share(&self, package: &str, share: &str) -> Located {
        match (package, share) {
            ("java-debug-adapter", "java-debug-adapter/a.jar") => Located::Installed {
                path: PathBuf::from("/tools/java-debug-adapter/0.59.0/extension/server/a.jar"),
                version: "0.59.0".into(),
            },
            _ => Located::Absent,
        }
    }
}

/// 언어 서버가 여는 어댑터의 `bundles` 는 설치된 것만 그 서버의 `initializationOptions.bundles` 뒤에 붙는다. 다른 서버 · 꺼 둔
/// 어댑터의 것은 넣지 않는다.
#[test]
fn installed_adapter_bundles_go_to_their_language_server() {
    let java = start_adapter("java-debug", "jdtls");
    let other = start_adapter("other-debug", "kotlin-lsp");
    let mut options = json!({"bundles": ["/mine.jar"], "settings": {}});
    dap::add_bundles(
        &mut options,
        "jdtls",
        [(&java, true), (&other, true)],
        &ShareTools,
    );
    assert_eq!(
        options,
        json!({
            "bundles": ["/mine.jar", "/tools/java-debug-adapter/0.59.0/extension/server/a.jar"],
            "settings": {}
        })
    );
    let mut options = json!({});
    dap::add_bundles(&mut options, "jdtls", [(&java, false)], &ShareTools);
    assert_eq!(options, json!({}));
    dap::add_bundles(&mut options, "jdtls", [(&java, true)], &NoTools);
    assert_eq!(options, json!({}));
}
