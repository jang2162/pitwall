//! 서버 관리자 시험의 바탕(tests/manager.rs · tests/live_lsp.rs): 임시 앱 폴더 · 레지스트리 · 프로젝트 둘, 가짜 로그인 셸, 내장
//! 번들 정의 위의 관리자, 실제 라우터 · 세션으로 부르는 연결([`Conn`]), 요청 길을 부르는 시험 전용 기능 RPC(`test.lsp.feature`).

#![allow(dead_code, reason = "each test binary uses a part of the helpers")]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use pw_core::bus::{DEFAULT_CAPACITY, EventBus, Scope};
use pw_core::config::{AppPaths, ConfigSource, ConfigStore};
use pw_core::registry::path::simplify;
use pw_core::registry::{NewProject, Registry};
use pw_core::task::TaskSpawner;
use pw_ext::LspFeature;
use pw_lsp::fake::Scenario;
use pw_lsp::manager::{FeatureRequest, HostParts, LspManager, ManagerOptions};
use pw_lsp::methods::LspInstanceState;
use pw_lsp_servers::LspServers;
use pw_rpc::{
    Caller, ConnectionScope, EventHub, Outbound, Role, Router, RpcError, Session, rpc_method,
};
use pw_runtime::{LoginShell, Runtime};
use semver::Version;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::mpsc::UnboundedReceiver;
use tokio_util::sync::CancellationToken;

use super::eventually;

// ---- 시험 전용 기능 RPC(요청 길을 부르는 기능 행 흉내) --------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TestFeatureParams {
    pub doc_id: String,
    pub feature: LspFeature,
    pub method: String,
    pub offset: u32,
    pub version: Option<u32>,
    /// `textDocument/references` 의 `context.includeDeclaration`.
    #[serde(default)]
    pub include_declaration: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TestFeatureResult {
    pub server: String,
    pub stale_version: bool,
    #[specta(type = pw_rpc::JsonValue)]
    pub result: Value,
}

rpc_method! {
    /// 시험 전용: 요청 길(`LspManager::request`)로 위치 하나를 묻는다.
    pub TestFeature: "test.lsp.feature" {
        params: TestFeatureParams,
        result: TestFeatureResult,
        tier: Read,
        roles: [Owner],
    }
}

// ---- 바탕 ----------------------------------------------------------------------------------------------------------------------

pub fn executable(path: &Path, body: &str) -> PathBuf {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    path.to_path_buf()
}

pub fn fake_program() -> &'static str {
    env!("CARGO_BIN_EXE_pw-lsp-fake")
}

pub struct Env {
    pub _tmp: tempfile::TempDir,
    pub base: PathBuf,
    pub paths: AppPaths,
    pub bus: EventBus,
    pub projects: Registry,
    pub global: ConfigStore,
    pub manager: LspManager,
    /// 정의 · 해석(설치본 찾기를 바꿀 때 — `LspServers::set_tools`).
    pub servers: Arc<LspServers>,
    /// 관리자가 진행을 띄우는 작업 표(같은 표를 본다).
    pub tasks: TaskSpawner,
    pub router: Arc<Router>,
    /// 시험 대상 프로젝트와 그 루트(실제 경로).
    pub id: String,
    pub root: PathBuf,
    /// 같은 Host 의 다른 프로젝트.
    pub other: String,
    /// 가짜 로그인 셸 PATH 의 첫 폴더(가짜 런타임을 둔다).
    pub bin: PathBuf,
}

pub struct EnvOptions {
    pub trusted: bool,
    pub follow: bool,
    pub manager: ManagerOptions,
    /// 가짜 로그인 셸이 PATH · HOME 을 시험 폴더로 바꾸는가. 끄면 시험 프로세스의 환경을 그대로 넘긴다(실 서버 시험 — cargo · node).
    pub isolated_shell: bool,
}

impl Default for EnvOptions {
    fn default() -> Self {
        Self {
            trusted: true,
            follow: true,
            isolated_shell: true,
            manager: ManagerOptions {
                backoff_base: Duration::from_millis(10),
                backoff_max: Duration::from_millis(50),
                idle_shutdown: Some(Duration::from_secs(600)),
                ..ManagerOptions::default()
            },
        }
    }
}

impl Env {
    pub fn new(options: EnvOptions) -> Self {
        Self::with_listener(options, |_| {})
    }

    pub fn with_listener(options: EnvOptions, extra: impl FnOnce(&LspManager)) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let base = simplify(fs::canonicalize(tmp.path()).unwrap());
        let paths = AppPaths::under(&base.join("app"));
        fs::create_dir_all(&paths.config).unwrap();
        fs::write(paths.config.join("settings.toml"), "schema = 1\n").unwrap();
        let bus = EventBus::new(DEFAULT_CAPACITY);
        let global = ConfigStore::open(ConfigSource::global(&paths), bus.clone(), Scope::Host);
        let projects = Registry::new(paths.clone(), None, bus.clone());
        let bin = base.join("shell bin");
        fs::create_dir_all(&bin).unwrap();
        // basedpyright 의 `requires python >= 3.10` 를 맞추는 가짜 python.
        executable(&bin.join("python3"), "echo 'Python 3.12.4'");
        let home = base.join("home");
        fs::create_dir_all(&home).unwrap();
        let body = if options.isolated_shell {
            format!(
                "PATH='{}:/usr/bin:/bin'; export PATH\nHOME='{}'; export HOME\nunset MISE_DATA_DIR XDG_DATA_HOME\neval \"$4\"",
                bin.display(),
                home.display()
            )
        } else {
            "eval \"$4\"".to_owned()
        };
        let shell = executable(&base.join("fake-shell"), &body);
        let runtime = Arc::new(Runtime::new(LoginShell::new(
            Some(shell),
            Duration::from_secs(60),
        )));
        let extensions = Arc::new(pw_ext::Registry::from_roots(
            &Version::new(0, 0, 0),
            pw_ext::builtin::roots(),
        ));
        let servers = Arc::new(LspServers::new(Arc::clone(&extensions), runtime));
        servers.attach(pw_lsp_servers::HostParts {
            paths: paths.clone(),
            global: global.clone(),
            projects: projects.clone(),
        });
        let manager = LspManager::new(Arc::clone(&servers), extensions, options.manager);
        let tasks = TaskSpawner::new(bus.clone(), Scope::Host, CancellationToken::new());
        manager.attach(HostParts {
            projects: projects.clone(),
            bus: bus.clone(),
            tasks: tasks.clone(),
            global: global.clone(),
            state_dir: paths.state.clone(),
        });
        extra(&manager);
        if options.follow {
            tokio::spawn(manager.clone().follow(LspManager::subscribe(&bus)));
        }
        let mut router = Router::new();
        pw_lsp::register(&mut router, &manager);
        let m = manager.clone();
        router.unary(TestFeature, move |ctx, p: TestFeatureParams| {
            let m = m.clone();
            async move {
                let answer = m
                    .request(
                        &ctx,
                        FeatureRequest {
                            doc_id: &p.doc_id,
                            feature: p.feature,
                            method: &p.method,
                            expected_version: p.version,
                            timeout: None,
                        },
                        |view| {
                            let mut params = json!({
                                "textDocument": view.text_document(),
                                "position": view.position(p.offset)?,
                            });
                            if let Some(include) = p.include_declaration {
                                params["context"] = json!({ "includeDeclaration": include });
                            }
                            Ok(params)
                        },
                    )
                    .await?;
                Ok(TestFeatureResult {
                    server: answer.server,
                    stale_version: answer.stale_version,
                    result: answer.result,
                })
            }
        });
        let project = |name: &str| {
            let root = base.join(name);
            fs::create_dir_all(&root).unwrap();
            let id = projects
                .create(NewProject {
                    root: root.clone(),
                    ..NewProject::default()
                })
                .unwrap()
                .id;
            (id, root)
        };
        let (id, root) = project("p1");
        let (other, _) = project("p2");
        if options.trusted {
            projects.set_trust(&id, true).unwrap();
        }
        Self {
            _tmp: tmp,
            base,
            paths,
            bus,
            projects,
            global,
            manager,
            servers,
            tasks,
            router: Arc::new(router),
            id,
            root,
            other,
            bin,
        }
    }

    /// 서버 `server` 의 실행 파일 · 인자를 설정(`[lsp.<id>] path` · `args`)으로 정한다.
    pub fn configure(&self, server: &str, program: &Path, args: &[String]) {
        let settings = self.paths.config.join("settings.toml");
        let mut text = fs::read_to_string(&settings).unwrap();
        text.push_str(&format!(
            "[lsp.{server}]\npath = \"{}\"\nargs = {}\n",
            program.display(),
            serde_json::to_string(args).unwrap()
        ));
        fs::write(&settings, text).unwrap();
        self.global.reload().unwrap();
    }

    /// 서버 `server` 를 가짜 서버(시나리오 `scenario`)로 바꾼다 — 전역 `[lsp.<id>] path` · `args`. 받은 메시지는 돌려준 기록 파일에.
    pub fn fake(&self, server: &str, mut scenario: Scenario) -> PathBuf {
        let dir = self.base.join("fake");
        fs::create_dir_all(&dir).unwrap();
        let record = dir.join(format!("{server}.jsonl"));
        scenario.record = Some(record.clone());
        let path = scenario.write(&dir, server).unwrap();
        let settings = self.paths.config.join("settings.toml");
        let mut text = fs::read_to_string(&settings).unwrap();
        text.push_str(&format!(
            "[lsp.{server}]\npath = \"{}\"\nargs = [\"{}\"]\n",
            fake_program(),
            path.display()
        ));
        fs::write(&settings, text).unwrap();
        self.global.reload().unwrap();
        record
    }

    pub fn file(&self, rel: &str, text: &str) -> PathBuf {
        let path = self.root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, text).unwrap();
        path
    }

    pub fn connect(&self, scope: ConnectionScope) -> Conn {
        let caller = match scope {
            ConnectionScope::File(file) => Caller::file(Role::Owner, file.path().to_path_buf()),
            scope => Caller {
                role: Role::Owner,
                scope,
                window: None,
            },
        };
        let (session, rx) = Session::new(Arc::clone(&self.router), EventHub::new(), caller);
        Conn {
            session,
            rx,
            next_id: 0,
            pending: Vec::new(),
        }
    }

    pub fn window(&self) -> Conn {
        self.connect(ConnectionScope::Project(self.id.clone()))
    }

    pub fn states(&self) -> Vec<(String, LspInstanceState)> {
        self.manager
            .status(&self.id)
            .instances
            .into_iter()
            .map(|i| (i.server, i.state))
            .collect()
    }

    pub fn state_of(&self, server: &str) -> Option<LspInstanceState> {
        self.states()
            .into_iter()
            .find(|(s, _)| s == server)
            .map(|(_, state)| state)
    }

    pub async fn wait_state(&self, server: &str, want: LspInstanceState) {
        let ok = eventually(Duration::from_secs(10), || {
            self.state_of(server).as_ref() == Some(&want)
        })
        .await;
        assert!(ok, "{server} never became {want:?}: {:?}", self.states());
    }
}

pub struct Conn {
    pub session: Session,
    pub rx: UnboundedReceiver<Outbound>,
    pub next_id: i64,
    pub pending: Vec<Outbound>,
}

impl Conn {
    /// 요청을 보내고 id 를 돌려준다(답은 [`Self::response`]).
    pub fn send(&mut self, method: &str, params: Value) -> i64 {
        self.next_id += 1;
        self.session.handle_value(
            json!({"jsonrpc": "2.0", "id": self.next_id, "method": method, "params": params}),
        );
        self.next_id
    }

    pub async fn response(&mut self, id: i64) -> Result<Value, RpcError> {
        let ours =
            |m: &Outbound| matches!(m, Outbound::Response(r) if r.id == pw_rpc::Id::Number(id));
        if let Some(at) = self.pending.iter().position(ours)
            && let Outbound::Response(r) = self.pending.remove(at)
        {
            return r.outcome;
        }
        loop {
            let msg = tokio::time::timeout(Duration::from_secs(15), self.rx.recv())
                .await
                .expect("no response")
                .expect("closed");
            match msg {
                Outbound::Response(r) if r.id == pw_rpc::Id::Number(id) => return r.outcome,
                other => self.pending.push(other),
            }
        }
    }

    pub async fn call(&mut self, method: &str, params: Value) -> Result<Value, RpcError> {
        let id = self.send(method, params);
        self.response(id).await
    }

    pub async fn open(&mut self, env: &Env, path: &Path, language: &str, text: &str) -> Value {
        self.call(
            "lsp.document.open",
            json!({
                "projectId": env.id,
                "path": path,
                "language": language,
                "version": 0,
                "text": text,
            }),
        )
        .await
        .unwrap()
    }
}
