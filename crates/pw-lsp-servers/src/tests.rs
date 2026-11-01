//! 해석 순서 · 신뢰 · 설정 합치기를 가짜 폴더(가짜 실행 파일 · 가짜 `rustup`) · 가짜 런타임 환경으로 본다. 사람의 실제 서버 ·
//! rustup 은 부르지 않는다.

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

/// 설치본 자리를 흉내 낸다.
struct FakeTools(Located);

impl ToolLocator for FakeTools {
    fn locate(&self, package: &str, bin: &str) -> Located {
        assert_eq!(
            (package, bin),
            ("typescript-language-server", "typescript-language-server")
        );
        self.0.clone()
    }
}

fn settings_path(value: &str, origin: LspSettingsOrigin) -> ServerConfig {
    let mut config = settings::server_config(&def("typescript-language-server"), None, None);
    config.path = Some(ConfiguredPath {
        value: value.to_owned(),
        origin,
    });
    config
}

/// 해석의 다섯 단계(설정 경로 → 프로젝트 로컬 → PATH → 설치본 → 설치 필요)를 한 단계씩 지워 가며 본다.
#[test]
fn resolution_follows_settings_project_local_path_installed_then_not_installed() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("project");
    let sub = root.join("packages/web");
    fs::create_dir_all(&sub).unwrap();
    let name = "typescript-language-server";
    let configured = executable(&tmp.path().join("opt/ts-ls"), "true");
    let local = executable(&root.join("node_modules/.bin").join(name), "true");
    let nearest = executable(&sub.join("node_modules/.bin").join(name), "true");
    // 프로젝트 루트 밖(위)의 것은 프로젝트 로컬이 아니다.
    executable(&tmp.path().join("node_modules/.bin").join(name), "true");
    let on_path = executable(&tmp.path().join("path bin").join(name), "true");
    let installed = tmp.path().join("tools/bin").join(name);
    let ts = def(name);
    let defaults = settings::server_config(&ts, None, None);
    let env = env_with_path(&[&tmp.path().join("path bin")]);
    let tools = FakeTools(Located::Installed {
        path: installed.clone(),
        version: "5.1.3".into(),
    });
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
        other => panic!("{other:?}"),
    };

    // 1. 설정 경로가 무엇보다 먼저다.
    let config = settings_path(configured.to_str().unwrap(), LspSettingsOrigin::Global);
    assert_eq!(
        found(resolve::resolve(&ts, &config, &ctx(true))),
        (configured.clone(), LspResolveStep::Settings, None)
    );
    // 2. 프로젝트 로컬 — 해석하는 디렉터리(packages/web)에서 프로젝트 루트까지 올라가며 가장 가까운 것.
    assert_eq!(
        found(resolve::resolve(&ts, &defaults, &ctx(true))),
        (nearest.clone(), LspResolveStep::ProjectLocal, None)
    );
    fs::remove_file(&nearest).unwrap();
    assert_eq!(
        found(resolve::resolve(&ts, &defaults, &ctx(true))),
        (local.clone(), LspResolveStep::ProjectLocal, None)
    );
    // 미신뢰 프로젝트는 node_modules/.bin 이 있어도 고르지 않는다.
    assert_eq!(
        found(resolve::resolve(&ts, &defaults, &ctx(false))),
        (on_path.clone(), LspResolveStep::Path, None)
    );
    // 3. PATH.
    fs::remove_file(&local).unwrap();
    assert_eq!(
        found(resolve::resolve(&ts, &defaults, &ctx(true))),
        (on_path.clone(), LspResolveStep::Path, None)
    );
    // 4. 설치본(판은 설치 기록의 것).
    fs::remove_file(&on_path).unwrap();
    assert_eq!(
        found(resolve::resolve(&ts, &defaults, &ctx(true))),
        (installed, LspResolveStep::Installed, Some("5.1.3".into()))
    );
    // 5. 설치 필요. 설치본이 깨졌으면 그 까닭을 싣는다.
    let none = ResolveContext {
        tools: &NoTools,
        ..ctx(true)
    };
    assert_eq!(
        resolve::resolve(&ts, &defaults, &none),
        LspResolution::NotInstalled {
            mason: Some(name.into()),
            reinstall: None,
            skipped: vec![],
        }
    );
    let broken = FakeTools(Located::Broken {
        reason: "python is gone".into(),
    });
    assert_eq!(
        resolve::resolve(
            &ts,
            &defaults,
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

/// 설정 경로는 절대 경로 · PATH 의 이름 · (프로젝트 설정만) 프로젝트 루트 기준 상대 경로다. 쓸 수 없으면 다음 단계로 가지 않는다.
#[test]
fn a_configured_path_is_used_or_reported_but_never_skipped() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("project");
    let in_project = executable(&root.join("tools/ts-ls"), "true");
    let named = executable(&tmp.path().join("bin/my-ts-ls"), "true");
    // PATH 에 정의의 이름이 있어도 틀린 설정 경로를 가리지 않는다.
    executable(&tmp.path().join("bin/typescript-language-server"), "true");
    let ts = def("typescript-language-server");
    let env = env_with_path(&[&tmp.path().join("bin")]);
    let ctx = ResolveContext {
        trusted: true,
        project_root: &root,
        dir: &root,
        env: &env,
        tools: &NoTools,
    };
    let resolve = |value: &str, origin| resolve::resolve(&ts, &settings_path(value, origin), &ctx);
    assert_eq!(
        resolve("my-ts-ls", LspSettingsOrigin::Global).path(),
        Some(named.as_path())
    );
    assert_eq!(
        resolve("tools/ts-ls", LspSettingsOrigin::Project).path(),
        Some(in_project.as_path())
    );
    let invalid = |value: &str, origin, reason: &str| LspResolution::InvalidPath {
        path: value.into(),
        origin,
        reason: reason.into(),
    };
    assert_eq!(
        resolve("tools/ts-ls", LspSettingsOrigin::Global),
        invalid(
            "tools/ts-ls",
            LspSettingsOrigin::Global,
            "a relative path in the global settings has nothing to be relative to; use an absolute path or a name on PATH"
        )
    );
    assert_eq!(
        resolve("/nowhere/ts-ls", LspSettingsOrigin::Global),
        invalid(
            "/nowhere/ts-ls",
            LspSettingsOrigin::Global,
            "not an executable file"
        )
    );
    assert_eq!(
        resolve("no-such-ls", LspSettingsOrigin::Project),
        invalid(
            "no-such-ls",
            LspSettingsOrigin::Project,
            "not found on PATH"
        )
    );
    let mut disabled = settings_path("my-ts-ls", LspSettingsOrigin::Global);
    disabled.enabled = false;
    assert_eq!(
        resolve::resolve(&ts, &disabled, &ctx),
        LspResolution::Disabled
    );
}

/// 가짜 `rustup`: `which <이름>` 에 cwd 의 `has-component` 파일이 있으면 그 툴체인 실행 파일 경로를, 없으면 rustup 과 같은 오류를
/// 낸다. 받은 `RUSTUP_AUTO_INSTALL` 을 `auto-install` 파일에 적는다.
fn fake_rustup(bin: &Path, toolchain_ra: &Path, log: &Path) -> PathBuf {
    executable(
        &bin.join("rustup"),
        &format!(
            "printf '%s' \"${{RUSTUP_AUTO_INSTALL-unset}}\" > '{log}'\n\
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
        let rustup = fake_rustup(&bin, &toolchain_ra, &log);
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
    fake_rustup(&bin, Path::new("/x"), &log);
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
        let shell = executable(
            &tmp.path().join("fake-shell"),
            &format!(
                "PATH='{}:/usr/bin:/bin'; export PATH\neval \"$4\"",
                bin.display()
            ),
        );
        let runtime = Runtime::new(LoginShell::new(Some(shell), Duration::from_secs(5)));
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
    let shared = format!(
        "schema = 1\n[lsp.rust-analyzer]\npath = \"{}\"\nargs = [\"--evil\"]\n",
        evil.display()
    );
    let local = format!("schema = 1\n[lsp.ruff]\npath = \"{}\"\n", mine.display());
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

    f.projects.set_trust(&id, true).unwrap();
    let list = f.list(&id);
    let ra = server(&list, "rust-analyzer");
    assert_eq!(ra.resolution.path(), Some(evil.as_path()));
    assert_eq!(ra.args, ["--evil"]);
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
