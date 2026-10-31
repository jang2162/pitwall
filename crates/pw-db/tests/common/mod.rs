//! M3-02 시험 도우미: 임시 앱 폴더 · 레지스트리 · 프로젝트 둘, 가짜 키체인([`MemorySecretStore`]) · 가짜 런타임 환경([`FakeEnv`]),
//! 연결 범위를 고른 RPC 연결([`Conn`]). 사람의 키체인 · 환경 · 설정을 건드리지 않는다.

#![allow(dead_code, reason = "each test binary uses a part of the helpers")]

use std::collections::BTreeMap;
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
        let caller = match scope {
            ConnectionScope::File(file) => Caller::file(Role::Owner, file.path().to_path_buf()),
            scope => Caller {
                role: Role::Owner,
                scope,
            },
        };
        let (session, rx) = Session::new(Arc::clone(&self.router), EventHub::new(), caller);
        Conn {
            session,
            rx,
            next_id: 0,
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
    session: Session,
    rx: UnboundedReceiver<Outbound>,
    next_id: i64,
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
            if let Some(response) = msg.into_response()
                && response.id == pw_rpc::Id::Number(id)
            {
                return response.outcome;
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
