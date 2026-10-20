//! 장기 작업(`Task`): 진행률 보고, 취소, 완료 · 실패 · 취소 상태. 진행과 종료는 이벤트 버스로도 알린다.
//!
//! UI 상태바와 MCP 가 같은 작업을 보도록 진행 알림은 버스의 [`TOPIC_PROGRESS`] · [`TOPIC_FINISHED`] 로 나간다
//! (`docs/design/03-architecture.md` 「코어 모듈」 절). 알림을 놓친 쪽(나중에 열린 창)은 [`TaskSpawner::running`]
//! (`task.list`)으로 지금 도는 작업을 읽고, 띄운 쪽이 아니어도 [`TaskSpawner::cancel`](`task.cancel`)로 멈출 수 있다.

use std::collections::BTreeMap;
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use serde::{Deserialize, Serialize};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use crate::bus::{EventBus, Scope};

pub type TaskId = u64;

/// 진행 알림의 토픽. 페이로드는 [`TaskProgressEvent`].
pub const TOPIC_PROGRESS: &str = "task.progress";
/// 종료 알림의 토픽. 페이로드는 [`TaskFinishedEvent`]. 작업마다 정확히 한 번 나간다.
pub const TOPIC_FINISHED: &str = "task.finished";

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum TaskStatus {
    Running,
    Completed,
    Failed { error: String },
    Cancelled,
}

impl TaskStatus {
    pub fn is_finished(&self) -> bool {
        !matches!(self, TaskStatus::Running)
    }
}

/// 진행 양은 2^53 안에 머문다고 보고 TS `number` 로 낸다(pw-rpc `JsNumber`).
/// 없는 값은 키를 빼지 않고 `null` 로 보낸다 — specta 가 `skip_serializing_if` 를 TS 로 표현하지 못한다.
#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct TaskProgress {
    #[specta(type = pw_rpc::JsNumber)]
    pub done: u64,
    /// 전체 양을 모르면 `None`(UI 는 불확정 진행 막대).
    #[serde(default)]
    #[specta(type = Option<pw_rpc::JsNumber>)]
    pub total: Option<u64>,
    #[serde(default)]
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskProgressEvent {
    pub task_id: TaskId,
    pub title: String,
    pub progress: TaskProgress,
}

/// 도는 작업 하나(`task.list` 의 항목).
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct TaskInfo {
    #[specta(type = pw_rpc::JsNumber)]
    pub task_id: TaskId,
    pub title: String,
    /// 프로젝트 범위 작업이면 그 프로젝트, Host 범위면 `None`.
    pub project_id: Option<String>,
    pub progress: TaskProgress,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskFinishedEvent {
    pub task_id: TaskId,
    pub title: String,
    #[serde(flatten)]
    pub status: TaskStatus,
}

/// 작업을 띄우는 곳. 범위(Host · 프로젝트)와 부모 취소 토큰을 갖는다 — 부모가 취소되면(프로젝트 닫힘 등)
/// 그 아래 작업이 모두 취소된다. 작업 id 는 같은 Host 의 spawner 끼리 겹치지 않는다.
#[derive(Clone)]
pub struct TaskSpawner {
    bus: EventBus,
    scope: Scope,
    cancel: CancellationToken,
    next_id: Arc<AtomicU64>,
    running: Running,
}

/// 끝나지 않은 작업 표. 같은 Host 의 spawner 끼리 함께 쓴다. 작업은 띄울 때 들어가고 종료 알림과 함께 빠진다.
#[derive(Clone, Default)]
struct Running(Arc<Mutex<BTreeMap<TaskId, RunningTask>>>);

struct RunningTask {
    title: String,
    scope: Scope,
    cancel: CancellationToken,
    progress: watch::Receiver<TaskProgress>,
}

impl Running {
    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<TaskId, RunningTask>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl TaskSpawner {
    pub fn new(bus: EventBus, scope: Scope, cancel: CancellationToken) -> Self {
        Self {
            bus,
            scope,
            cancel,
            next_id: Arc::new(AtomicU64::new(1)),
            running: Running::default(),
        }
    }

    /// 같은 id 공간을 쓰되 범위와 부모 토큰이 다른 spawner.
    #[must_use]
    pub fn scoped(&self, scope: Scope, cancel: CancellationToken) -> Self {
        Self {
            bus: self.bus.clone(),
            scope,
            cancel,
            next_id: Arc::clone(&self.next_id),
            running: self.running.clone(),
        }
    }

    pub fn scope(&self) -> &Scope {
        &self.scope
    }

    /// 끝나지 않은 작업(id 순). Host 범위 작업과, `project` 를 주면 그 프로젝트의 작업. 다른 프로젝트의 작업은 넣지 않는다 —
    /// 이벤트 구독과 같은 범위 규칙이다(`pw_rpc::EventHub`).
    pub fn running(&self, project: Option<&str>) -> Vec<TaskInfo> {
        self.running
            .lock()
            .iter()
            .filter_map(|(&task_id, task)| {
                let project_id = match &task.scope {
                    Scope::Host => None,
                    Scope::Project(id) if Some(id.as_str()) == project => Some(id.clone()),
                    Scope::Project(_) => return None,
                };
                Some(TaskInfo {
                    task_id,
                    title: task.title.clone(),
                    project_id,
                    progress: task.progress.borrow().clone(),
                })
            })
            .collect()
    }

    /// 도는 작업을 id 로 취소한다. 그런 작업이 없으면(이미 끝남 · 모르는 id) `false`. 어느 spawner 가 띄웠든 같은 Host 면 멈춘다.
    pub fn cancel(&self, id: TaskId) -> bool {
        let token = self.running.lock().get(&id).map(|t| t.cancel.clone());
        token.inspect(CancellationToken::cancel).is_some()
    }

    /// 작업을 tokio 태스크로 띄운다. tokio 런타임 안에서 불러야 한다.
    ///
    /// 취소되면 작업의 future 는 다음 `.await` 지점에서 버려진다. `.await` 없이 오래 도는 구간은
    /// [`TaskCtx::cancellation`] 을 직접 확인해야 멈춘다.
    pub fn spawn<F, Fut>(&self, title: impl Into<String>, work: F) -> TaskHandle
    where
        F: FnOnce(TaskCtx) -> Fut,
        Fut: Future<Output = Result<(), String>> + Send + 'static,
    {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let title = title.into();
        let cancel = self.cancel.child_token();
        let (progress_tx, progress_rx) = watch::channel(TaskProgress::default());
        let (status_tx, status_rx) = watch::channel(TaskStatus::Running);
        let ctx = TaskCtx {
            id,
            title: title.clone(),
            scope: self.scope.clone(),
            bus: self.bus.clone(),
            cancel: cancel.clone(),
            progress: progress_tx,
        };
        let fut = work(ctx);
        // `work` 가 패닉하면 표에서 빼 줄 종료 guard 가 아직 없다. 그래서 표에는 `work` 를 부른 뒤에 넣는다.
        self.running.lock().insert(
            id,
            RunningTask {
                title: title.clone(),
                scope: self.scope.clone(),
                cancel: cancel.clone(),
                progress: progress_rx.clone(),
            },
        );
        let mut finish = FinishGuard {
            id,
            title,
            scope: self.scope.clone(),
            bus: self.bus.clone(),
            status: Some(status_tx),
            running: self.running.clone(),
        };
        let token = cancel.clone();
        tokio::spawn(async move {
            // biased: 취소를 먼저 본다. 취소된 뒤에는 작업을 한 번도 더 poll 하지 않는다.
            let status = tokio::select! {
                biased;
                () = token.cancelled() => TaskStatus::Cancelled,
                result = fut => match result {
                    Ok(()) => TaskStatus::Completed,
                    Err(error) => TaskStatus::Failed { error },
                },
            };
            finish.finish(status);
        });
        TaskHandle {
            id,
            cancel,
            progress: progress_rx,
            status: status_rx,
        }
    }
}

/// 작업 본문이 받는 문맥.
pub struct TaskCtx {
    id: TaskId,
    title: String,
    scope: Scope,
    bus: EventBus,
    cancel: CancellationToken,
    progress: watch::Sender<TaskProgress>,
}

impl TaskCtx {
    pub fn id(&self) -> TaskId {
        self.id
    }

    pub fn cancellation(&self) -> &CancellationToken {
        &self.cancel
    }

    /// 진행률을 갱신하고 버스에 알린다.
    pub fn report(&self, progress: TaskProgress) {
        let event = TaskProgressEvent {
            task_id: self.id,
            title: self.title.clone(),
            progress: progress.clone(),
        };
        self.progress.send_replace(progress);
        // 직렬화가 실패할 수 없는 타입이다. 실패해도 작업을 멈출 이유는 아니다.
        let _ = self.bus.publish(self.scope.clone(), TOPIC_PROGRESS, &event);
    }
}

/// 띄운 작업의 손잡이. 버려도 작업은 멈추지 않는다 — 멈추려면 [`TaskHandle::cancel`].
pub struct TaskHandle {
    id: TaskId,
    cancel: CancellationToken,
    progress: watch::Receiver<TaskProgress>,
    status: watch::Receiver<TaskStatus>,
}

impl TaskHandle {
    pub fn id(&self) -> TaskId {
        self.id
    }

    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    /// 버려질 때 작업을 취소하는 guard. 작업의 결과를 기다리던 쪽(RPC 호출 등)이 사라지면 작업도 멈추게 할 때 쓴다.
    pub fn cancel_on_drop(&self) -> tokio_util::sync::DropGuard {
        self.cancel.clone().drop_guard()
    }

    pub fn progress(&self) -> TaskProgress {
        self.progress.borrow().clone()
    }

    pub fn status(&self) -> TaskStatus {
        self.status.borrow().clone()
    }

    /// 작업이 끝날 때까지 기다린다.
    pub async fn wait(&mut self) -> TaskStatus {
        // 송신 쪽(FinishGuard)은 끝난 상태를 보낸 뒤에만 사라지므로 Err 는 이미 끝난 경우뿐이다.
        let _ = self.status.wait_for(TaskStatus::is_finished).await;
        self.status()
    }
}

/// 작업이 끝까지 돌지 못하고 태스크가 버려져도(패닉, 런타임 종료) 종료 상태와 종료 알림이 나가게 한다.
/// 안 나가면 UI 가 영원히 「진행 중」으로 본다. drop 에서는 둘을 구분할 수 없어 같은 문구로 알린다.
struct FinishGuard {
    id: TaskId,
    title: String,
    scope: Scope,
    bus: EventBus,
    status: Option<watch::Sender<TaskStatus>>,
    running: Running,
}

impl FinishGuard {
    fn finish(&mut self, status: TaskStatus) {
        let Some(sender) = self.status.take() else {
            return;
        };
        // 종료 알림보다 먼저 뺀다 — 알림을 받고 `task.list` 를 부른 쪽이 끝난 작업을 다시 보지 않게.
        self.running.lock().remove(&self.id);
        let event = TaskFinishedEvent {
            task_id: self.id,
            title: std::mem::take(&mut self.title),
            status: status.clone(),
        };
        let _ = self.bus.publish(self.scope.clone(), TOPIC_FINISHED, &event);
        sender.send_replace(status);
    }
}

impl Drop for FinishGuard {
    fn drop(&mut self) {
        self.finish(TaskStatus::Failed {
            error: TASK_ABORTED.into(),
        });
    }
}

/// 작업이 패닉했거나 런타임이 닫혀 작업이 버려졌을 때의 실패 문구.
pub const TASK_ABORTED: &str = "task aborted (panicked or runtime shut down)";

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::bus::{Filter, Received};

    fn spawner() -> (EventBus, TaskSpawner) {
        let bus = EventBus::new(64);
        let spawner = TaskSpawner::new(bus.clone(), Scope::Host, CancellationToken::new());
        (bus, spawner)
    }

    /// 1ms 마다 카운터를 올리며 끝없이 도는 작업.
    fn ticking(counter: Arc<AtomicU64>) -> impl FnOnce(TaskCtx) -> BoxedWork {
        move |_ctx| {
            Box::pin(async move {
                loop {
                    counter.fetch_add(1, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(1)).await;
                }
            })
        }
    }

    type BoxedWork = std::pin::Pin<Box<dyn Future<Output = Result<(), String>> + Send>>;

    /// 시간 제한이 있는 `wait`. 취소가 전파되지 않는 회귀가 무한 대기가 아니라 실패로 끝나게 한다.
    /// `start_paused` 테스트에서도 다른 타이머가 없으면 곧바로 만료된다.
    async fn wait(handle: &mut TaskHandle) -> TaskStatus {
        tokio::time::timeout(Duration::from_secs(5), handle.wait())
            .await
            .expect("task did not finish in time")
    }

    #[tokio::test(start_paused = true)]
    async fn progress_is_reported_and_published() {
        let (bus, spawner) = spawner();
        let mut sub = bus.subscribe(Filter::all().topics([TOPIC_PROGRESS, TOPIC_FINISHED]));
        let mut handle = spawner.spawn("count", |ctx| async move {
            for done in 1..=2 {
                ctx.report(TaskProgress {
                    done,
                    total: Some(2),
                    message: None,
                });
            }
            Ok(())
        });
        assert_eq!(wait(&mut handle).await, TaskStatus::Completed);
        assert_eq!(handle.progress().done, 2);

        let mut topics = Vec::new();
        while let Some(Received::Event(e)) = sub.try_recv() {
            topics.push(e.topic.clone());
            if e.topic == TOPIC_FINISHED {
                let finished: TaskFinishedEvent =
                    serde_json::from_value(e.payload.clone()).unwrap();
                assert_eq!(finished.task_id, handle.id());
                assert_eq!(finished.status, TaskStatus::Completed);
            }
        }
        assert_eq!(topics, [TOPIC_PROGRESS, TOPIC_PROGRESS, TOPIC_FINISHED]);
    }

    #[tokio::test(start_paused = true)]
    async fn cancel_stops_the_work() {
        let (_bus, spawner) = spawner();
        let counter = Arc::new(AtomicU64::new(0));
        let mut handle = spawner.spawn("tick", ticking(Arc::clone(&counter)));
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert!(counter.load(Ordering::SeqCst) > 0);

        handle.cancel();
        assert_eq!(wait(&mut handle).await, TaskStatus::Cancelled);
        let stopped_at = counter.load(Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(counter.load(Ordering::SeqCst), stopped_at);
    }

    #[tokio::test(start_paused = true)]
    async fn cancelling_the_parent_cancels_its_tasks() {
        let bus = EventBus::new(8);
        let parent = CancellationToken::new();
        let spawner = TaskSpawner::new(bus, Scope::Host, parent.clone());
        let counter = Arc::new(AtomicU64::new(0));
        let mut handle = spawner.spawn("tick", ticking(Arc::clone(&counter)));
        parent.cancel();
        assert_eq!(wait(&mut handle).await, TaskStatus::Cancelled);
    }

    #[tokio::test(start_paused = true)]
    async fn dropping_the_cancel_guard_cancels() {
        let (_bus, spawner) = spawner();
        let mut handle = spawner.spawn("tick", ticking(Arc::new(AtomicU64::new(0))));
        drop(handle.cancel_on_drop());
        assert_eq!(wait(&mut handle).await, TaskStatus::Cancelled);
    }

    #[tokio::test]
    async fn failure_and_panic_are_reported_as_failed() {
        let (bus, spawner) = spawner();
        let mut sub = bus.subscribe(Filter::all().topics([TOPIC_FINISHED]));
        let mut failed = spawner.spawn("fail", |_ctx| async { Err("boom".to_owned()) });
        assert_eq!(
            wait(&mut failed).await,
            TaskStatus::Failed {
                error: "boom".into()
            }
        );
        let mut panicked = spawner.spawn("panic", |_ctx| async { panic!("task panic for test") });
        assert_eq!(
            wait(&mut panicked).await,
            TaskStatus::Failed {
                error: TASK_ABORTED.into()
            }
        );
        // 패닉해도 종료 알림은 나간다.
        let mut finished = 0;
        while let Some(Received::Event(_)) = sub.try_recv() {
            finished += 1;
        }
        assert_eq!(finished, 2);
    }

    /// 도는 작업은 목록에 있고 끝나면 빠진다. 프로젝트 작업은 그 프로젝트를 물을 때만 나온다.
    #[tokio::test(start_paused = true)]
    async fn running_lists_unfinished_tasks_in_scope() {
        let (_bus, host) = spawner();
        let p1 = host.scoped(Scope::Project("p1".into()), CancellationToken::new());
        let p2 = host.scoped(Scope::Project("p2".into()), CancellationToken::new());
        let mut a = host.spawn("a", |ctx| async move {
            ctx.report(TaskProgress {
                done: 1,
                total: Some(4),
                message: None,
            });
            std::future::pending().await
        });
        let b = p1.spawn("b", ticking(Arc::new(AtomicU64::new(0))));
        let _c = p2.spawn("c", ticking(Arc::new(AtomicU64::new(0))));
        tokio::time::sleep(Duration::from_millis(5)).await;

        let titles = |tasks: Vec<TaskInfo>| -> Vec<(String, Option<String>)> {
            tasks.into_iter().map(|t| (t.title, t.project_id)).collect()
        };
        assert_eq!(titles(host.running(None)), [("a".to_owned(), None)]);
        assert_eq!(
            titles(p2.running(Some("p1"))),
            [
                ("a".to_owned(), None),
                ("b".to_owned(), Some("p1".to_owned()))
            ],
            "다른 spawner 가 띄운 작업도 같은 표에 있다"
        );
        assert_eq!(host.running(None)[0].progress.done, 1);

        a.cancel();
        assert_eq!(wait(&mut a).await, TaskStatus::Cancelled);
        assert_eq!(
            titles(host.running(Some("p1"))),
            [("b".to_owned(), Some("p1".to_owned()))]
        );
        drop(b);
    }

    /// 띄운 spawner · 핸들이 아니어도 id 로 취소된다. 끝난 작업 · 모르는 id 는 `false`.
    #[tokio::test(start_paused = true)]
    async fn cancel_by_id_stops_a_task_started_elsewhere() {
        let (_bus, host) = spawner();
        let project = host.scoped(Scope::Project("p1".into()), CancellationToken::new());
        let counter = Arc::new(AtomicU64::new(0));
        let mut handle = project.spawn("tick", ticking(Arc::clone(&counter)));
        tokio::time::sleep(Duration::from_millis(5)).await;

        assert!(host.cancel(handle.id()));
        assert_eq!(wait(&mut handle).await, TaskStatus::Cancelled);
        assert!(!host.cancel(handle.id()), "끝난 작업");
        assert!(!host.cancel(9999), "모르는 id");
        assert!(host.running(Some("p1")).is_empty());
    }

    /// 패닉한 작업도 표에서 빠진다(종료 guard 의 drop).
    #[tokio::test]
    async fn a_panicked_task_leaves_the_list() {
        let (_bus, spawner) = spawner();
        let mut panicked = spawner.spawn("panic", |_ctx| async { panic!("task panic for test") });
        wait(&mut panicked).await;
        assert!(spawner.running(None).is_empty());
    }

    #[test]
    fn scoped_spawners_share_the_id_space() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let _enter = runtime.enter();
        let (_bus, host) = spawner();
        let project = host.scoped(Scope::Project("p1".into()), CancellationToken::new());
        let a = host.spawn("a", |_ctx| async { Ok(()) });
        let b = project.spawn("b", |_ctx| async { Ok(()) });
        assert_ne!(a.id(), b.id());
    }
}
