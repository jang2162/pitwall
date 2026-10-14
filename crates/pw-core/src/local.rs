//! `LocalHost`: 데스크톱 프로세스 안의 Host. pw-rpc [`Router`] 를 in-process 로 품는다.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use pw_rpc::{Caller, EventHub, Outbound, Router, RpcError, Session};
use serde::Serialize;
use serde_json::Value;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio_util::sync::CancellationToken;

use crate::ProjectId;
use crate::bus::{DEFAULT_CAPACITY, EventBus, Filter, Received, Scope};
use crate::host::{BoxFuture, Connection, FrameSink, FrameSource, Host, HostError, HostId};
use crate::methods::{
    DemoTaskParams, DemoTaskUpdate, HostDemoTask, HostInfo, HostInfoMethod, HostPing, Pong,
};
use crate::task::{TOPIC_PROGRESS, TaskCtx, TaskProgress, TaskProgressEvent, TaskSpawner};

/// 프로젝트 하나를 다루는 모듈들이 받는 문맥(`docs/design/03-architecture.md` 「코어 모듈」 절).
/// 설정은 M0-08(설정 계층)이 붙인다.
#[derive(Clone)]
pub struct ProjectContext {
    id: ProjectId,
    root: Arc<Path>,
    bus: EventBus,
    tasks: TaskSpawner,
    cancel: CancellationToken,
}

impl ProjectContext {
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn bus(&self) -> &EventBus {
        &self.bus
    }

    /// 이 프로젝트 범위로 이벤트를 낸다. RPC 의 `events.subscribe({ projectId })` 구독자에게도 간다.
    pub fn publish<T: Serialize>(&self, topic: &str, payload: &T) -> serde_json::Result<usize> {
        self.bus
            .publish(Scope::Project(self.id.clone()), topic, payload)
    }

    /// 이 프로젝트 범위의 작업. 프로젝트가 닫히면 함께 취소된다.
    pub fn tasks(&self) -> &TaskSpawner {
        &self.tasks
    }

    /// 프로젝트가 닫히면 취소된다. 모듈은 여기에 자기 수명주기를 건다.
    pub fn cancellation(&self) -> &CancellationToken {
        &self.cancel
    }
}

pub struct LocalHost {
    id: HostId,
    router: Arc<Router>,
    hub: EventHub,
    bus: EventBus,
    tasks: TaskSpawner,
    cancel: CancellationToken,
    projects: Mutex<HashMap<ProjectId, ProjectContext>>,
}

impl LocalHost {
    pub fn new(id: impl Into<HostId>) -> Self {
        let id = id.into();
        let hub = EventHub::new();
        let bus = EventBus::with_rpc_hub(DEFAULT_CAPACITY, hub.clone());
        let cancel = CancellationToken::new();
        let tasks = TaskSpawner::new(bus.clone(), Scope::Host, cancel.child_token());
        let router = Arc::new(build_router(&id, &bus, &tasks));
        Self {
            id,
            router,
            hub,
            bus,
            tasks,
            cancel,
            projects: Mutex::default(),
        }
    }

    pub fn bus(&self) -> &EventBus {
        &self.bus
    }

    /// Host 범위(프로젝트 밖)의 작업.
    pub fn tasks(&self) -> &TaskSpawner {
        &self.tasks
    }

    pub fn router(&self) -> &Router {
        &self.router
    }

    pub fn open_project(&self, id: ProjectId, root: PathBuf) -> Result<ProjectContext, HostError> {
        let mut projects = self.projects.lock().unwrap_or_else(PoisonError::into_inner);
        if projects.contains_key(&id) {
            return Err(HostError::ProjectAlreadyOpen(id));
        }
        let cancel = self.cancel.child_token();
        let ctx = ProjectContext {
            id: id.clone(),
            root: root.into(),
            bus: self.bus.clone(),
            tasks: self
                .tasks
                .scoped(Scope::Project(id.clone()), cancel.clone()),
            cancel,
        };
        projects.insert(id, ctx.clone());
        Ok(ctx)
    }

    pub fn project(&self, id: &str) -> Option<ProjectContext> {
        let projects = self.projects.lock().unwrap_or_else(PoisonError::into_inner);
        projects.get(id).cloned()
    }

    /// 프로젝트를 닫는다. 그 프로젝트의 취소 토큰을 취소하므로 프로젝트 범위 작업이 모두 멈춘다.
    pub fn close_project(&self, id: &str) -> bool {
        let removed = self
            .projects
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(id);
        match removed {
            Some(ctx) => {
                ctx.cancel.cancel();
                true
            }
            None => false,
        }
    }
}

impl Drop for LocalHost {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

impl Host for LocalHost {
    fn id(&self) -> &str {
        &self.id
    }

    fn connect(&self, caller: Caller) -> BoxFuture<'_, Result<Connection, HostError>> {
        let (session, rx) = Session::new(Arc::clone(&self.router), self.hub.clone(), caller);
        Box::pin(async move {
            Ok(Connection {
                sink: Box::new(LocalSink(session)),
                source: Box::new(LocalSource(rx)),
            })
        })
    }
}

/// pw-rpc 세션을 품는다. 버리면 `Session` 의 `Drop` 이 진행 중 요청 · 스트림을 취소하고 구독을 걷는다.
struct LocalSink(Session);

impl FrameSink for LocalSink {
    fn send(&self, frame: Value) {
        self.0.handle_value(frame);
    }
}

struct LocalSource(UnboundedReceiver<Outbound>);

impl FrameSource for LocalSource {
    fn recv(&mut self) -> BoxFuture<'_, Option<Value>> {
        Box::pin(async move {
            let message = self.0.recv().await?;
            // Outbound 는 세션이 만든 값이라 직렬화가 실패하지 않는다.
            Some(serde_json::to_value(message).unwrap_or(Value::Null))
        })
    }
}

fn build_router(host_id: &str, bus: &EventBus, tasks: &TaskSpawner) -> Router {
    let mut router = Router::new();
    let ping_host = host_id.to_owned();
    let info = HostInfo {
        host_id: host_id.to_owned(),
        kind: "local".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        os: std::env::consts::OS.into(),
        arch: std::env::consts::ARCH.into(),
    };
    let bus = bus.clone();
    let tasks = tasks.clone();
    router
        .unary(HostPing, move |_ctx, params| {
            let host_id = ping_host.clone();
            async move {
                Ok(Pong {
                    nonce: params.nonce,
                    host_id,
                })
            }
        })
        .unary(HostInfoMethod, move |_ctx, ()| {
            let info = info.clone();
            async move { Ok(info) }
        })
        .stream(HostDemoTask, move |_ctx, params, sink| {
            let bus = bus.clone();
            let tasks = tasks.clone();
            async move {
                if params.steps == 0 || params.steps > DemoTaskParams::MAX_STEPS {
                    return Err(RpcError::invalid_params(format!(
                        "steps must be 1..={}",
                        DemoTaskParams::MAX_STEPS
                    )));
                }
                if !DemoTaskParams::INTERVAL_MS.contains(&params.interval_ms) {
                    return Err(RpcError::invalid_params(format!(
                        "intervalMs must be {:?}",
                        DemoTaskParams::INTERVAL_MS
                    )));
                }
                // 작업을 띄우기 전에 구독한다. 먼저 띄우면 첫 진행 알림을 놓칠 수 있다.
                let mut events = bus.subscribe(
                    Filter::all()
                        .scope(tasks.scope().clone())
                        .topics([TOPIC_PROGRESS]),
                );
                let mut handle = tasks.spawn("demo", move |ctx| demo_work(ctx, params));
                // 스트림이 취소되면(stream.cancel · 연결 닫힘) pw-rpc 가 이 future 를 버린다. 그때 작업도 멈춘다.
                let _cancel_task = handle.cancel_on_drop();
                let task_id = handle.id();
                let mut bus_open = true;
                loop {
                    // 종료는 버스가 아니라 작업 상태로 본다. 구독은 Host 범위 전체를 받으므로, 이 핸들러가 credit 을
                    // 기다리는 동안 다른 작업의 알림이 상한을 넘게 쌓이면 이 작업의 종료 알림이 버려질 수 있다.
                    // biased 로 남은 진행 알림을 먼저 비워 마지막 진행이 종료보다 앞서 나가게 한다.
                    let received = tokio::select! {
                        biased;
                        received = events.recv(), if bus_open => received,
                        status = handle.wait() => {
                            sink.send(DemoTaskUpdate::Finished {
                                task_id,
                                outcome: status,
                            })
                            .await?;
                            return Ok(());
                        }
                    };
                    // 밀린 진행 알림(Lagged)은 버려져도 된다 — 다음 알림이 더 새 값이다.
                    let event = match received {
                        Some(Received::Event(event)) => event,
                        Some(Received::Lagged(_)) => continue,
                        None => {
                            bus_open = false;
                            continue;
                        }
                    };
                    let e: TaskProgressEvent = decode(&event.payload)?;
                    if e.task_id == task_id {
                        sink.send(DemoTaskUpdate::Progress {
                            task_id,
                            progress: e.progress,
                        })
                        .await?;
                    }
                }
            }
        });
    router
}

fn decode<T: serde::de::DeserializeOwned>(payload: &Value) -> Result<T, RpcError> {
    serde_json::from_value(payload.clone()).map_err(|e| RpcError::internal(e.to_string()))
}

async fn demo_work(ctx: TaskCtx, params: DemoTaskParams) -> Result<(), String> {
    let total = u64::from(params.steps);
    for done in 1..=total {
        tokio::time::sleep(std::time::Duration::from_millis(params.interval_ms)).await;
        ctx.report(TaskProgress {
            done,
            total: Some(total),
            message: Some(format!("step {done}/{total}")),
        });
    }
    Ok(())
}
