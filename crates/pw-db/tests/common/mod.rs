//! M3-02 시험 도우미: 임시 앱 폴더 · 레지스트리 · 프로젝트 둘, 가짜 키체인([`MemorySecretStore`]) · 가짜 런타임 환경([`FakeEnv`]),
//! 연결 범위를 고른 RPC 연결([`Conn`]). 사람의 키체인 · 환경 · 설정을 건드리지 않는다.

#![allow(dead_code, reason = "each test binary uses a part of the helpers")]

use std::collections::{BTreeMap, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use pw_core::bus::{DEFAULT_CAPACITY, EventBus};
use pw_core::config::AppPaths;
use pw_core::registry::path::simplify;
use pw_core::registry::{NewProject, Registry};
use pw_db::config::{Connections, HostParts};
use pw_db::secret::{EnvLookup, MemorySecretStore};
use pw_db_types::Secret;
use pw_rpc::protocol::{
    STREAM_CHUNK, STREAM_END, StreamChunk, StreamEnd, StreamOpened, StreamOutcome,
};
use pw_rpc::{Caller, ConnectionScope, EventHub, Outbound, Role, Router, RpcError, Session};
use serde_json::{Value, json};
use tokio::sync::mpsc::UnboundedReceiver;

/// 가짜 런타임 환경: 변수 표와 읽은 이름의 기록.
#[derive(Default)]
pub struct FakeEnv {
    pub vars: Mutex<BTreeMap<String, String>>,
    pub reads: Mutex<Vec<String>>,
}

impl FakeEnv {
    pub fn set(&self, name: &str, value: &str) {
        self.vars
            .lock()
            .unwrap()
            .insert(name.to_owned(), value.to_owned());
    }

    pub fn reads(&self) -> Vec<String> {
        self.reads.lock().unwrap().clone()
    }
}

impl EnvLookup for FakeEnv {
    fn var(&self, _project_id: &str, _root: &Path, name: &str) -> Option<Secret> {
        self.reads.lock().unwrap().push(name.to_owned());
        self.vars
            .lock()
            .unwrap()
            .get(name)
            .cloned()
            .map(Secret::new)
    }
}

pub struct Fixture {
    _tmp: tempfile::TempDir,
    pub base: PathBuf,
    pub app: AppPaths,
    pub projects: Registry,
    pub bus: EventBus,
    pub store: Arc<MemorySecretStore>,
    pub env: Arc<FakeEnv>,
    pub connections: Arc<Connections>,
    pub router: Arc<Router>,
    /// 시험 대상 프로젝트와 그 루트(실제 경로).
    pub id: String,
    pub root: PathBuf,
    /// 같은 Host 의 다른 프로젝트.
    pub other: String,
}

impl Fixture {
    /// 프로젝트 둘. `trusted` 면 시험 대상 프로젝트를 신뢰한다.
    pub fn new(trusted: bool) -> Self {
        Self::build(trusted, |_, _| {})
    }

    /// [`Fixture::new`] 에 메서드를 더 붙인다(실행 파이프라인 등). `extra` 는 연결 설정 메서드를 붙인 뒤에 불린다.
    pub fn build(trusted: bool, extra: impl FnOnce(&mut Router, &Arc<Connections>)) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let base = simplify(fs::canonicalize(tmp.path()).unwrap());
        let app = AppPaths::under(&base.join("app"));
        let bus = EventBus::new(DEFAULT_CAPACITY);
        let projects = Registry::new(app.clone(), None, bus.clone());
        let store = Arc::new(MemorySecretStore::new());
        let env = Arc::new(FakeEnv::default());
        let connections = Arc::new(Connections::new(store.clone(), env.clone()));
        connections.attach(HostParts {
            paths: app.clone(),
            projects: projects.clone(),
            bus: bus.clone(),
        });
        let mut router = Router::new();
        pw_db::config::register(&mut router, Arc::clone(&connections));
        extra(&mut router, &connections);
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
        if trusted {
            projects.set_trust(&id, true).unwrap();
        }
        Self {
            _tmp: tmp,
            base,
            app,
            projects,
            bus,
            store,
            env,
            connections,
            router: Arc::new(router),
            id,
            root,
            other,
        }
    }

    pub fn connect(&self, scope: ConnectionScope) -> Conn {
        self.connect_in(scope, None)
    }

    /// `window` 창(데스크톱은 웹뷰 label)이 연 연결 — 새로고침한 창은 같은 `window` 로 새 연결을 연다.
    pub fn connect_in(&self, scope: ConnectionScope, window: Option<&str>) -> Conn {
        let mut caller = match scope {
            ConnectionScope::File(file) => Caller::file(Role::Owner, file.path().to_path_buf()),
            scope => Caller {
                role: Role::Owner,
                scope,
                window: None,
            },
        };
        caller.window = window.map(str::to_owned);
        let (session, rx) = Session::new(Arc::clone(&self.router), EventHub::new(), caller);
        Conn {
            session,
            rx,
            next_id: 0,
            pending: VecDeque::new(),
        }
    }

    /// 그 프로젝트 창의 연결.
    pub fn window(&self) -> Conn {
        self.connect(ConnectionScope::Project(self.id.clone()))
    }

    /// `.pitwall/db/<name>` 의 경로.
    pub fn db_file(&self, name: &str) -> PathBuf {
        self.root.join(".pitwall").join("db").join(name)
    }

    /// 연결 설정 파일을 손으로 쓴다(동료가 커밋한 파일, `git pull` 흉내).
    pub fn write_db(&self, name: &str, text: &str) {
        let path = self.db_file(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    /// 설정 파일 한 줄을 바꾼다.
    pub fn edit_db(&self, name: &str, from: &str, to: &str) {
        let path = self.db_file(name);
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains(from), "{from:?} not in {text}");
        fs::write(path, text.replace(from, to)).unwrap();
    }
}

pub struct Conn {
    pub session: Session,
    rx: UnboundedReceiver<Outbound>,
    next_id: i64,
    /// 응답을 기다리며 받은 알림(스트림 청크 등) — 버리지 않고 스트림 읽기가 먼저 본다.
    pending: VecDeque<Outbound>,
}

/// 스트림에서 받은 것.
#[derive(Debug)]
pub enum StreamMsg {
    Chunk(Value),
    End(StreamOutcome),
}

impl Conn {
    pub async fn call(&mut self, method: &str, params: Value) -> Result<Value, RpcError> {
        self.next_id += 1;
        let id = self.next_id;
        self.session
            .handle_value(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        loop {
            let msg = tokio::time::timeout(Duration::from_secs(5), self.rx.recv())
                .await
                .expect("no response")
                .expect("closed");
            match msg {
                Outbound::Response(response) if response.id == pw_rpc::Id::Number(id) => {
                    return response.outcome;
                }
                other => self.pending.push_back(other),
            }
        }
    }

    /// 스트림 메서드를 연다. 열리면 스트림 id.
    pub async fn start(&mut self, method: &str, params: Value) -> Result<u64, RpcError> {
        let opened = self.call(method, params).await?;
        Ok(serde_json::from_value::<StreamOpened>(opened)
            .unwrap()
            .stream_id)
    }

    pub fn credit(&self, stream: u64, credit: u32) {
        self.session.handle_value(json!({
            "jsonrpc": "2.0", "method": "stream.credit",
            "params": { "streamId": stream, "credit": credit },
        }));
    }

    pub fn cancel_stream(&self, stream: u64) {
        self.session.handle_value(json!({
            "jsonrpc": "2.0", "method": "stream.cancel", "params": { "streamId": stream },
        }));
    }

    /// 그 스트림의 다음 청크나 끝(`wait` 안에 오지 않으면 `None`).
    pub async fn next_in(&mut self, stream: u64, wait: Duration) -> Option<StreamMsg> {
        let deadline = tokio::time::Instant::now() + wait;
        loop {
            let msg = match self.pending.pop_front() {
                Some(msg) => msg,
                None => tokio::time::timeout_at(deadline, self.rx.recv())
                    .await
                    .ok()??,
            };
            let Some(note) = msg.into_notification() else {
                continue;
            };
            if note.method == STREAM_CHUNK {
                let chunk: StreamChunk = note.params_as().unwrap();
                if chunk.stream_id == stream {
                    return Some(StreamMsg::Chunk(chunk.data));
                }
            } else if note.method == STREAM_END {
                let end: StreamEnd = note.params_as().unwrap();
                if end.stream_id == stream {
                    return Some(StreamMsg::End(end.outcome));
                }
            }
        }
    }

    pub async fn next(&mut self, stream: u64) -> StreamMsg {
        self.next_in(stream, Duration::from_secs(5))
            .await
            .expect("no stream message")
    }

    /// 넉넉한 credit 을 주고 끝까지 모은다.
    pub async fn collect(&mut self, stream: u64) -> (Vec<Value>, StreamOutcome) {
        self.credit(stream, 1_000_000);
        let mut items = Vec::new();
        loop {
            match self.next(stream).await {
                StreamMsg::Chunk(item) => items.push(item),
                StreamMsg::End(outcome) => return (items, outcome),
            }
        }
    }
}

/// 원격 PG 연결 설정(키체인 비밀번호, 터널).
pub const REMOTE_PG: &str = r#"schema = 1
name = "prod pg"
driver = "postgres"
host = "db.internal"
port = 5432
database = "billing"
user = "app"
password = { secret_ref = "keychain" }
read_only = false
sslmode = "verify-full"

[ssh_tunnel]
host_ref = "bastion"
"#;

/// `git worktree add` 가 만드는 모양을 손으로 만든다: `main`(메인 체크아웃, `.git` 폴더를 만든다)의 연결 워크트리 `wt`. 돌려주는 것은
/// 워크트리 꼭대기(실제 경로).
pub fn fake_worktree(main: &Path, wt: &Path) -> PathBuf {
    let admin = main.join(".git/worktrees/wt");
    fs::create_dir_all(&admin).unwrap();
    fs::write(admin.join("commondir"), "../..\n").unwrap();
    fs::create_dir_all(wt).unwrap();
    fs::write(wt.join(".git"), format!("gitdir: {}\n", admin.display())).unwrap();
    fs::write(
        admin.join("gitdir"),
        format!("{}\n", wt.join(".git").display()),
    )
    .unwrap();
    simplify(fs::canonicalize(wt).unwrap())
}

/// 연결 설정(JSON)에서 창이 사람에게 보일 대상 — `db.secret.set` 의 `target`.
pub fn target_of(config: &Value) -> Value {
    let config: pw_db::config::DbConnectionConfig = serde_json::from_value(config.clone()).unwrap();
    serde_json::to_value(pw_db::secret::DbSecretTarget::of(&config)).unwrap()
}

impl Conn {
    /// 지금 설정의 터널 목적지를 확인한다(창이 확인 요청을 보여 주고 [확인] 을 누른 흐름 — M3-11 의 첫 사용 확인).
    pub async fn approve_tunnel(
        &mut self,
        project: &str,
        connection: &str,
    ) -> Result<Value, RpcError> {
        let entry = self
            .call(
                "db.connection.get",
                json!({ "projectId": project, "connectionId": connection }),
            )
            .await?;
        let config = &entry["config"];
        self.call(
            "db.connection.approve",
            json!({ "projectId": project, "connectionId": connection, "approval": {
                "kind": "tunnel", "hostRef": config["sshTunnel"]["hostRef"], "target": target_of(config),
            } }),
        )
        .await
    }

    /// 지금 설정의 대상으로 비밀번호를 넣는다(창이 대상을 보여 주고 넣는 흐름).
    pub async fn set_password(
        &mut self,
        project: &str,
        connection: &str,
        value: &str,
    ) -> Result<Value, RpcError> {
        let entry = self
            .call(
                "db.connection.get",
                json!({ "projectId": project, "connectionId": connection }),
            )
            .await?;
        let target = target_of(&entry["config"]);
        self.call(
            "db.secret.set",
            json!({ "projectId": project, "connectionId": connection, "key": "password",
                    "target": target, "value": value }),
        )
        .await
    }
}
