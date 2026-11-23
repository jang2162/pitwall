//! Desktop 쪽의 비밀 답(18-m7-plan M7-12 ② · 결정 D47 · 메인 판정 M7-P19 「비밀은 증명된 머신 id 에만」).
//!
//! 붙은 원격 머신의 Relay 는 DB 비밀을 디스크에 두지 않고 이 Desktop 에 청한다(`pw_db::desktop_secret` — 그 Relay 의 Host 가 내는
//! `secret.requests` 스트림 · `secret.answer`). 이 모듈이 그 청에 답한다:
//!
//! - **열쇠의 머신 id 는 그 링크가 증명한 것이다**(핸드셰이크의 도전 서명 — `pw_link::dial`). 청이 대는 것은 그 머신 레지스트리의
//!   프로젝트 id · 연결 id · 키뿐이고, 키체인 계정은 `pw_db::secret::machine_account(<증명된 머신 id>, …)` 다 — 다른 머신 id 의
//!   링크는 같은 프로젝트 · 연결 id 를 대도 남의 비밀을 받지 못한다(계획 리뷰 F1).
//! - **청이 댄 그 창 요청 하나에만 답한다**(18-m7-plan M7-12 리뷰 F1). Relay 는 청에 그 비밀을 부른 창의 요청(창 label · 요청
//!   id — `pw_rpc::current_call`)을 싣는다. [`SecretDesk::gate`] 가 원격 Host 를 감싸, 그 머신의 프로젝트로 연 창 연결(`Project`
//!   범위 · 창 label 이 있는 것)이 보내는 요청 가운데 **비밀이 필요한 메서드**([`SECRET_FLOWS`] — 연결 열기 · 연결 시험 · 탐색 ·
//!   쪽 읽기 · 비밀 넣기 · 지우기 · 연결 지우기)만, 아직 답이 오지 않은 동안 센다([`Flights`]). 답하는 것은 ① 그 요청이 지금
//!   기다리는 중이고 ② 표에 있는 메서드이고 ③ 청의 연결이 그 요청의 연결이고 ④ 청의 종류(읽기 · 두기 · 지우기)가 그 메서드의
//!   것이고 ⑤ 그 요청이 아직 청을 쓰지 않았고(한 번) ⑥ 보낸 지 [`FLIGHT_LIMIT`] 안일 때뿐이다. 창이 저절로 보내는 읽기
//!   (`db.connection.get` · `db.connection.marks` 등)는 표에 없어 Relay 가 그 답을 붙잡아도 아무 청도 열지 못한다. 에이전트 범위 ·
//!   Host 범위 연결의 요청은 세지 않는다(D48 · D60). 그 밖의 청은 [`SecretAnswer::Declined`] 다.
//! - 값은 로그에 쓰지 않는다(머신 · 프로젝트 · 연결 · 청의 종류만).
//! - 키체인은 계정을 나열하지 못해 머신마다 계정 목록(`pw_db::secret::machine_index_account` — 계정 이름만)을 함께 둔다. 「이
//!   머신을 새로 믿기」(메인 판정 M7-P6)가 옛 id 의 비밀을 그것으로 지운다([`SecretDesk::forget_machine`]).

use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use pw_core::{BoxFuture, Connection, FrameSink, FrameSource, Host, HostError};
use pw_db::desktop_secret::{SecretAnswer, SecretRequest, SecretRequestOp};
use pw_db::secret::{SecretStore, machine_account, machine_index_account};
use pw_db_types::Secret;
use pw_rpc::{Caller, ConnectionScope, Role};
use serde_json::{Value, json};

use crate::lock;

/// 청의 종류(Read · Store · Remove — [`SecretRequestOp`] 의 갈래).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretUse {
    Read,
    Store,
    Remove,
}

impl SecretUse {
    fn of(op: &SecretRequestOp) -> Self {
        match op {
            SecretRequestOp::Read => Self::Read,
            SecretRequestOp::Store { .. } => Self::Store,
            SecretRequestOp::Remove => Self::Remove,
        }
    }
}

/// 비밀이 필요한 창 요청과 그 요청 하나가 열 수 있는 청(종류 · 한 번 — 18-m7-plan M7-12 리뷰 F1). **표에 없는 메서드는 기다려도
/// 아무 청도 열지 않는다** — 창이 저절로 보내는 읽기(`db.connection.get` · `db.connection.marks` · `db.connection.list` 등)나
/// 연결을 열지 않는 쓰기(`upsert` · `approve` · `forgetMarks`)가 그렇다. 지우기는 사람이 비밀 · 연결을 지우는 요청에만.
pub const SECRET_FLOWS: &[(&str, SecretUse)] = &[
    ("db.session.open", SecretUse::Read),
    ("db.connection.test", SecretUse::Read),
    ("db.query.page", SecretUse::Read),
    ("db.schema.databases", SecretUse::Read),
    ("db.schema.schemas", SecretUse::Read),
    ("db.schema.objects", SecretUse::Read),
    ("db.schema.columns", SecretUse::Read),
    ("db.schema.indexes", SecretUse::Read),
    ("db.schema.foreignKeys", SecretUse::Read),
    ("db.schema.ddl", SecretUse::Read),
    ("db.schema.refresh", SecretUse::Read),
    ("db.secret.set", SecretUse::Store),
    ("db.secret.clear", SecretUse::Remove),
    ("db.connection.delete", SecretUse::Remove),
];

/// 요청 하나가 청을 열 수 있는 시간(요청을 보낸 뒤). Relay 가 답을 붙잡아 기다림을 늘려도 이 뒤에는 열지 않는다.
pub const FLIGHT_LIMIT: Duration = pw_db::desktop_secret::WAIT;

/// 창 연결이 보낸, 답을 기다리는 비밀이 필요한 요청 하나([`SECRET_FLOWS`]).
#[derive(Debug, Clone)]
struct Flight {
    window: String,
    project: String,
    connection: String,
    method: &'static str,
    allows: SecretUse,
    /// 남은 청 수(한 요청에 한 번).
    left: u32,
    sent: Instant,
}

/// 한 머신의 창 연결들이 기다리는 비밀이 필요한 요청. 감싼 Host([`SecretDesk::gate`])가 채우고 [`SecretDesk::answer`] 가 쓴다.
#[derive(Default)]
pub struct Flights {
    next: AtomicU64,
    /// (연결, 요청 id 의 JSON 글) → 요청.
    entries: Mutex<HashMap<(u64, String), Flight>>,
}

/// 청이 댄 창 label(`<기기 id>/<label>` — 기기 id 에 `/` 가 있을 수 있고 label 에는 없다)의 label.
fn label_of(window: &str) -> &str {
    window.rsplit_once('/').map_or(window, |(_, label)| label)
}

impl Flights {
    fn connection(&self) -> u64 {
        self.next.fetch_add(1, Ordering::Relaxed)
    }

    /// 청이 댄 그 요청이 지금 기다리는 중이고, 그 프로젝트 · 연결의 것이고, 그 메서드가 이 종류의 청을 열 수 있고, 남은 청이
    /// 있고, 보낸 지 [`FLIGHT_LIMIT`] 안일 때만 하나를 쓰고 그 메서드를 돌려준다.
    fn take(
        &self,
        window: &str,
        request: &str,
        project: &str,
        connection: &str,
        want: SecretUse,
    ) -> Option<&'static str> {
        let label = label_of(window);
        let mut entries = lock(&self.entries);
        let flight = entries.iter_mut().find_map(|((_, id), f)| {
            (id == request
                && f.window == label
                && f.project == project
                && f.connection == connection
                && f.allows == want
                && f.left > 0
                && f.sent.elapsed() < FLIGHT_LIMIT)
                .then_some(f)
        })?;
        flight.left -= 1;
        Some(flight.method)
    }

    /// 지금 기다리는 요청 수(시험).
    pub fn len(&self) -> usize {
        lock(&self.entries).len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn sent(&self, conn: u64, window: &str, project: &str, frame: &Value) {
        let (Some(id), Some(method)) =
            (frame.get("id"), frame.get("method").and_then(Value::as_str))
        else {
            return;
        };
        let Some((method, allows)) = SECRET_FLOWS.iter().find(|(m, _)| *m == method).copied()
        else {
            return;
        };
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
                window: window.to_owned(),
                project: p.to_owned(),
                connection: c.to_owned(),
                method,
                allows,
                left: 1,
                sent: Instant::now(),
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
                (ConnectionScope::Project(project), Some(window)) => {
                    Some((project.clone(), window.clone()))
                }
                _ => None,
            };
            let connection = self.inner.connect(caller).await?;
            let Some((project, window)) = watched else {
                return Ok(connection);
            };
            let conn = self.flights.connection();
            Ok(Connection {
                sink: Box::new(TapSink {
                    inner: connection.sink,
                    flights: Arc::clone(&self.flights),
                    conn,
                    project,
                    window,
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
    window: String,
}

impl FrameSink for TapSink {
    fn send(&self, frame: Value) {
        self.flights
            .sent(self.conn, &self.window, &self.project, &frame);
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
        let want = SecretUse::of(&request.op);
        let kind = match want {
            SecretUse::Read => "read",
            SecretUse::Store => "store",
            SecretUse::Remove => "remove",
        };
        // 계정의 마디가 될 값에 `/` · 빈 값이 오면 답하지 않는다(리뷰 R3).
        if [project, connection]
            .iter()
            .any(|v| v.is_empty() || v.contains('/'))
        {
            tracing::info!(target: "pw_remote", machine, kind, "declined a malformed secret request");
            return SecretAnswer::Declined;
        }
        let Some(method) = flights.take(
            &request.call.window,
            &request.call.request,
            project,
            connection,
            want,
        ) else {
            tracing::info!(target: "pw_remote", machine, project, connection, kind,
                "declined a secret request: the window's request it names is not waiting for this");
            return SecretAnswer::Declined;
        };
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
        tracing::info!(target: "pw_remote", machine, %account, kind, method, "answered a remote secret request");
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
pub(crate) mod tests;
