//! Desktop 쪽의 비밀 답(18-m7-plan M7-12 ② · 결정 D47 · 메인 판정 M7-P19 「비밀은 증명된 머신 id 에만」).
//!
//! 붙은 원격 머신의 Relay 는 DB 비밀을 디스크에 두지 않고 이 Desktop 에 청한다(`pw_db::desktop_secret` — 그 Relay 의 Host 가 내는
//! `secret.requests` 스트림 · `secret.answer`). 이 모듈이 그 청에 답한다:
//!
//! - **열쇠의 머신 id 는 그 링크가 증명한 것이다**(핸드셰이크의 도전 서명 — `pw_link::dial`). 청이 대는 것은 그 머신 레지스트리의
//!   프로젝트 id · 연결 id · 키뿐이고, 키체인 계정은 `pw_db::secret::machine_account(<증명된 머신 id>, …)` 다 — 다른 머신 id 의
//!   링크는 같은 프로젝트 · 연결 id 를 대도 남의 비밀을 받지 못한다(계획 리뷰 F1).
//! - **그 프로젝트 창이 그 연결로 요청을 보내고 답을 기다리는 동안만 답한다.** [`SecretDesk::gate`] 가 원격 Host 를 감싸, 그
//!   머신의 프로젝트로 연 창 연결(`Project` 범위 · 창 label 이 있는 것)이 보내는 `db.*` 요청 가운데 `projectId` · `connectionId` 를
//!   싣고 아직 답이 오지 않은 것을 센다([`Flights`]). 읽기 · 지우기는 그런 요청이 하나라도 있을 때, 두기는 `db.secret.set` 이
//!   기다릴 때만 한다. 에이전트 범위 · Host 범위 연결의 요청은 세지 않는다 — 에이전트의 DB 툴이 일으킨 청에는 답하지 않는다(D48 ·
//!   D60). 그 밖의 청은 [`SecretAnswer::Declined`] 다.
//! - 값은 로그에 쓰지 않는다(머신 · 프로젝트 · 연결 · 청의 종류만).
//! - 키체인은 계정을 나열하지 못해 머신마다 계정 목록(`pw_db::secret::machine_index_account` — 계정 이름만)을 함께 둔다. 「이
//!   머신을 새로 믿기」(메인 판정 M7-P6)가 옛 id 의 비밀을 그것으로 지운다([`SecretDesk::forget_machine`]).

use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use pw_core::{BoxFuture, Connection, FrameSink, FrameSource, Host, HostError};
use pw_db::desktop_secret::{SecretAnswer, SecretRequest, SecretRequestOp};
use pw_db::secret::{SecretStore, machine_account, machine_index_account};
use pw_db_types::Secret;
use pw_rpc::{Caller, ConnectionScope, Role};
use serde_json::{Value, json};

use crate::lock;

/// 창 연결이 보낸, 답을 기다리는 `db.*` 요청 하나.
#[derive(Debug, Clone)]
struct Flight {
    project: String,
    connection: String,
    method: String,
}

/// 한 머신의 창 연결들이 기다리는 DB 요청. 감싼 Host([`SecretDesk::gate`])가 채우고 [`SecretDesk::answer`] 가 본다.
#[derive(Default)]
pub struct Flights {
    next: AtomicU64,
    entries: Mutex<HashMap<(u64, String), Flight>>,
}

impl Flights {
    fn connection(&self) -> u64 {
        self.next.fetch_add(1, Ordering::Relaxed)
    }

    /// 그 프로젝트 · 연결로 기다리는 요청 가운데 `method` 를 만족하는 것이 있는가.
    fn any(&self, project: &str, connection: &str, method: impl Fn(&str) -> bool) -> bool {
        lock(&self.entries)
            .values()
            .any(|f| f.project == project && f.connection == connection && method(&f.method))
    }

    /// 지금 기다리는 요청 수(시험).
    pub fn len(&self) -> usize {
        lock(&self.entries).len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn sent(&self, conn: u64, project: &str, frame: &Value) {
        let (Some(id), Some(method)) =
            (frame.get("id"), frame.get("method").and_then(Value::as_str))
        else {
            return;
        };
        if !method.starts_with("db.") {
            return;
        }
        let params = frame.get("params");
        let field = |name: &str| params.and_then(|p| p.get(name)).and_then(Value::as_str);
        let (Some(p), Some(c)) = (field("projectId"), field("connectionId")) else {
            return;
        };
        // 그 연결의 프로젝트가 아닌 요청은 Relay 가 거절한다 — 세지 않는다.
        if p != project {
            return;
        }
        lock(&self.entries).insert(
            (conn, id.to_string()),
            Flight {
                project: p.to_owned(),
                connection: c.to_owned(),
                method: method.to_owned(),
            },
        );
    }

    fn received(&self, conn: u64, frame: &Value) {
        if frame.get("method").is_some() {
            return;
        }
        if let Some(id) = frame.get("id") {
            lock(&self.entries).remove(&(conn, id.to_string()));
        }
    }

    fn closed(&self, conn: u64) {
        lock(&self.entries).retain(|(c, _), _| *c != conn);
    }
}

/// 원격 Host 를 감싼 것: 그 머신의 프로젝트 창 연결이 보내고 받는 것을 지켜본다. 나머지는 그대로 넘긴다.
struct GatedHost {
    inner: Arc<dyn Host>,
    flights: Arc<Flights>,
}

impl Host for GatedHost {
    fn id(&self) -> &str {
        self.inner.id()
    }

    fn connect(&self, caller: Caller) -> BoxFuture<'_, Result<Connection, HostError>> {
        Box::pin(async move {
            // 사람의 프로젝트 창(창 label 이 있는 `Project` 범위)만 센다. 에이전트 · Host 범위 연결은 지켜보지 않는다.
            let watched = match (&caller.scope, &caller.window) {
                (ConnectionScope::Project(project), Some(_)) => Some(project.clone()),
                _ => None,
            };
            let connection = self.inner.connect(caller).await?;
            let Some(project) = watched else {
                return Ok(connection);
            };
            let conn = self.flights.connection();
            Ok(Connection {
                sink: Box::new(TapSink {
                    inner: connection.sink,
                    flights: Arc::clone(&self.flights),
                    conn,
                    project,
                }),
                source: Box::new(TapSource {
                    inner: connection.source,
                    flights: Arc::clone(&self.flights),
                    conn,
                }),
            })
        })
    }

    fn is_remote(&self) -> bool {
        self.inner.is_remote()
    }
}

struct TapSink {
    inner: Box<dyn FrameSink>,
    flights: Arc<Flights>,
    conn: u64,
    project: String,
}

impl FrameSink for TapSink {
    fn send(&self, frame: Value) {
        self.flights.sent(self.conn, &self.project, &frame);
        self.inner.send(frame);
    }
}

impl Drop for TapSink {
    fn drop(&mut self) {
        self.flights.closed(self.conn);
    }
}

struct TapSource {
    inner: Box<dyn FrameSource>,
    flights: Arc<Flights>,
    conn: u64,
}

impl FrameSource for TapSource {
    fn recv(&mut self) -> BoxFuture<'_, Option<Value>> {
        Box::pin(async move {
            let frame = self.inner.recv().await;
            match &frame {
                Some(frame) => self.flights.received(self.conn, frame),
                None => self.flights.closed(self.conn),
            }
            frame
        })
    }
}

/// 원격 Relay 의 비밀 청에 답하는 것. 저장소는 이 Desktop 의 키체인(앱은 `pw_db::secret::KeyringStore`).
pub struct SecretDesk {
    store: Arc<dyn SecretStore>,
    /// 계정 목록 항목의 읽고 고쳐 쓰기를 한 줄로.
    index: Mutex<()>,
}

impl SecretDesk {
    pub fn new(store: Arc<dyn SecretStore>) -> Self {
        Self {
            store,
            index: Mutex::new(()),
        }
    }

    /// 원격 Host 를 감싼다. 감싼 것을 라우터에 더하고, 돌려준 [`Flights`] 를 [`Self::serve`] · [`Self::answer`] 에 준다.
    pub fn gate(host: Arc<dyn Host>) -> (Arc<dyn Host>, Arc<Flights>) {
        let flights = Arc::new(Flights::default());
        let gated = GatedHost {
            inner: host,
            flights: Arc::clone(&flights),
        };
        (Arc::new(gated), flights)
    }

    /// 청 하나의 답(블로킹 — 키체인). `machine` 은 그 링크가 증명한 머신 id 다.
    pub fn answer(
        &self,
        machine: &str,
        flights: &Flights,
        request: &SecretRequest,
    ) -> SecretAnswer {
        let (project, connection) = (&request.project_id, &request.connection_id);
        let any_db = |m: &str| m.starts_with("db.");
        let (allowed, kind) = match &request.op {
            SecretRequestOp::Read => (flights.any(project, connection, any_db), "read"),
            SecretRequestOp::Store { .. } => (
                flights.any(project, connection, |m| m == "db.secret.set"),
                "store",
            ),
            SecretRequestOp::Remove => (flights.any(project, connection, any_db), "remove"),
        };
        if !allowed {
            tracing::info!(target: "pw_remote", machine, project, connection, kind,
                "declined a secret request: no window of that project is waiting on that connection");
            return SecretAnswer::Declined;
        }
        let account = machine_account(machine, project, connection, request.key);
        let failed = |e: pw_db::secret::StoreError| {
            tracing::warn!(target: "pw_remote", machine, %account, %e, "cannot use the keychain for a remote secret");
            SecretAnswer::Failed {
                message: format!("cannot use this desktop's keychain: {e}"),
            }
        };
        let answer = match &request.op {
            SecretRequestOp::Read => match self.store.get(&account) {
                Ok(Some(item)) => SecretAnswer::Value { item },
                Ok(None) => SecretAnswer::Missing,
                Err(e) => failed(e),
            },
            SecretRequestOp::Store { item } => match self.store.set(&account, item) {
                Ok(()) => {
                    self.index_update(machine, |accounts| {
                        accounts.insert(account.clone());
                    });
                    SecretAnswer::Done
                }
                Err(e) => failed(e),
            },
            SecretRequestOp::Remove => match self.store.delete(&account) {
                Ok(()) => {
                    self.index_update(machine, |accounts| {
                        accounts.remove(&account);
                    });
                    SecretAnswer::Done
                }
                Err(e) => failed(e),
            },
        };
        tracing::info!(target: "pw_remote", machine, %account, kind, "answered a remote secret request");
        answer
    }

    fn index(&self, machine: &str) -> BTreeSet<String> {
        match self.store.get(&machine_index_account(machine)) {
            Ok(Some(raw)) => serde_json::from_str(raw.expose()).unwrap_or_default(),
            _ => BTreeSet::new(),
        }
    }

    fn index_update(&self, machine: &str, change: impl FnOnce(&mut BTreeSet<String>)) {
        let _one = lock(&self.index);
        let mut accounts = self.index(machine);
        change(&mut accounts);
        let account = machine_index_account(machine);
        let result = if accounts.is_empty() {
            self.store.delete(&account)
        } else {
            let raw = serde_json::to_string(&accounts).unwrap_or_else(|_| "[]".to_owned());
            self.store.set(&account, &Secret::new(raw))
        };
        if let Err(e) = result {
            tracing::warn!(target: "pw_remote", machine, %e, "cannot update the remote secret index");
        }
    }

    /// 그 머신 id 의 비밀을 모두 지운다(「이 머신을 새로 믿기」 — 옛 id, 메인 판정 M7-P6). 블로킹.
    pub fn forget_machine(&self, machine: &str) {
        let _one = lock(&self.index);
        let accounts = self.index(machine);
        for account in &accounts {
            if let Err(e) = self.store.delete(account) {
                tracing::warn!(target: "pw_remote", machine, %account, %e, "cannot remove a secret of the old machine id");
            }
        }
        if let Err(e) = self.store.delete(&machine_index_account(machine)) {
            tracing::warn!(target: "pw_remote", machine, %e, "cannot remove the old machine's secret index");
        }
        tracing::info!(target: "pw_remote", machine, removed = accounts.len(), "removed the secrets of an old machine id");
    }

    /// 그 머신 링크의 청을 받아 답한다 — 링크가 닫히거나 Relay 가 스트림을 닫을 때까지. `host` 는 감싸기 전의 원격 Host,
    /// `machine` 은 그 링크가 증명한 머신 id 다.
    pub async fn serve(
        self: Arc<Self>,
        machine: String,
        host: Arc<dyn Host>,
        flights: Arc<Flights>,
    ) {
        let mut connection = match host.connect(Caller::new(Role::Owner)).await {
            Ok(connection) => connection,
            Err(e) => {
                tracing::info!(target: "pw_remote", %machine, ?e, "cannot listen for remote secret requests");
                return;
            }
        };
        const OPEN: u64 = 0;
        connection.sink.send(json!({
            "jsonrpc": "2.0", "id": OPEN, "method": "secret.requests", "params": null,
        }));
        let mut stream = None;
        let mut next = OPEN;
        while let Some(frame) = connection.source.recv().await {
            if frame.get("method").is_none() && frame.get("id") == Some(&json!(OPEN)) {
                let Some(id) = frame.pointer("/result/streamId").and_then(Value::as_u64) else {
                    tracing::info!(target: "pw_remote", %machine, "the relay does not take secret requests");
                    return;
                };
                stream = Some(id);
                connection.sink.send(json!({
                    "jsonrpc": "2.0", "method": "stream.credit",
                    "params": { "streamId": id, "credit": 16 },
                }));
                continue;
            }
            let Some(stream) = stream else { continue };
            let method = frame.get("method").and_then(Value::as_str);
            if frame.pointer("/params/streamId").and_then(Value::as_u64) != Some(stream) {
                continue;
            }
            match method {
                Some("stream.chunk") => {
                    let request = frame
                        .pointer("/params/data")
                        .cloned()
                        .and_then(|data| serde_json::from_value::<SecretRequest>(data).ok());
                    connection.sink.send(json!({
                        "jsonrpc": "2.0", "method": "stream.credit",
                        "params": { "streamId": stream, "credit": 1 },
                    }));
                    let Some(request) = request else {
                        tracing::info!(target: "pw_remote", %machine, "skipped an unreadable secret request");
                        continue;
                    };
                    let (desk, m, f) = (Arc::clone(&self), machine.clone(), Arc::clone(&flights));
                    let id = request.id;
                    let answer = tokio::task::spawn_blocking(move || desk.answer(&m, &f, &request))
                        .await
                        .unwrap_or(SecretAnswer::Failed {
                            message: "the desktop could not answer".to_owned(),
                        });
                    next += 1;
                    connection.sink.send(json!({
                        "jsonrpc": "2.0", "id": next, "method": "secret.answer",
                        "params": { "id": id, "answer": answer },
                    }));
                }
                Some("stream.end") => return,
                _ => {}
            }
        }
    }
}

#[cfg(test)]
#[path = "secrets_tests.rs"]
mod tests;
