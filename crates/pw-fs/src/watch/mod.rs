//! 파일 감시(`docs/design/12-m1-plan.md` M1-06, 결정 M1-D7): 창의 루트(활성 워크트리)를 감시해 바뀐 경로를 프로젝트 범위 이벤트
//! [`TOPIC_CHANGED`](`fs.changed`)로 낸다.
//!
//! - 감시는 창이 [`FsWatchMethod`](`fs.watch`) 스트림을 여는 동안 산다. 스트림이 닫히면(창이 닫히거나 새로 고침 — 연결이 닫히면
//!   pw-rpc 가 스트림을 취소한다) 놓고, 창의 활성 워크트리가 바뀌면 창이 스트림을 닫고 새 `worktree` 로 다시 연다. 같은 루트를 보는
//!   창이 여럿이면 OS 감시는 하나다([`Watch`] 가 루트마다 센다).
//! - 이벤트는 버스로 나간다 — 창은 `events.subscribe({ projectId, topics: ["fs.changed"] })` 로, Host 안의 소비자(파일 목록 인덱스
//!   M1-12 · 런타임 제공자 M1-16)는 [`pw_core::EventBus`] 로 받는다. 스트림의 항목은 변경이 아니라 감시 방식([`FsWatching`])이다.
//! - 방식 · 디바운스 · 폴백은 [`engine`], 창 하나 동안의 경로 모으기와 종류 판정은 [`batch`].
//! - OS 감시 한도를 넘으면 경고 알림(`notification.show`, [`pw_core::notification`])을 낸다.

pub mod batch;
pub mod engine;

use std::collections::{HashMap, HashSet};
use std::future::Future as _;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::task::Poll;

use pw_core::EventBus;
use pw_core::registry::methods::rpc_error;
use pw_rpc::{Router, RpcError, code, rpc_method};
use serde::{Deserialize, Serialize};

use crate::Fs;
use crate::scope::Roots;
use engine::{Engine, Options, Sink};

/// 바뀐 경로를 알리는 프로젝트 범위 토픽.
pub const TOPIC_CHANGED: &str = "fs.changed";

/// 경로 하나의 변경. 창이 닫힐 때 디스크를 보고 정한다 — `removed` 는 확실하고 `created` · `modified` 는 힌트다([`batch`]).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum FsChangeKind {
    Created,
    Modified,
    Removed,
}

/// [`TOPIC_CHANGED`] 의 내용. 디바운스 창 하나(75 ms)에 루트마다 하나다.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct FsChanged {
    /// 감시한 루트(정규화한 실제 경로).
    pub root: String,
    /// 그 루트를 연 창의 활성 워크트리(`fs.watch` 의 `worktree`).
    pub worktree: Option<String>,
    /// 바뀐 절대 경로. 이름순, 중복 없음. `.git/` 안은 없다. ignore 된 폴더 안의 변경은 재귀 감시(macOS · Windows)에서는 그 폴더
    /// (가장 바깥의 ignore 된 폴더) 하나의 `modified` 로 접히고 — 받는 쪽은 그 아래 펼친 것을 다시 읽는다 — 폴더별 감시(Linux)에서는
    /// 오지 않는다.
    pub paths: Vec<String>,
    /// `paths[i]` 의 변경.
    pub kinds: Vec<FsChangeKind>,
    /// 놓친 변경이 있을 수 있다(OS 큐 넘침 · 감시 오류 · 창 하나에 경로가 너무 많음). `paths` 는 비고, 루트 전체를 다시 읽는다.
    pub rescan: bool,
}

/// 루트 하나를 보는 방식.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum WatchMode {
    /// OS 의 재귀 감시(macOS FSEvents · Windows ReadDirectoryChangesW).
    Recursive,
    /// ignore 되지 않은 폴더마다 OS 감시(Linux inotify). ignore 된 폴더 안의 변경은 오지 않는다.
    Folders,
    /// OS 감시 한도를 넘어, 그 뒤의 폴더는 5초마다 폴링으로 본다(그 폴더의 변경은 그만큼 늦다).
    Polling,
}

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct FsWatchParams {
    pub project_id: String,
    /// 창의 활성 워크트리 꼭대기(`ActiveWorktree.top`). 있으면 그 워크트리 안의 프로젝트 루트를, 없으면 프로젝트의 등록 루트를 본다.
    #[serde(default)]
    pub worktree: Option<String>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct FsWatchedRoot {
    pub path: String,
    pub mode: WatchMode,
}

/// `fs.watch` 의 항목: 지금 보는 루트와 방식. 감시를 다 건 뒤 한 번, 방식이 바뀔 때마다(한도 초과 → 폴링) 한 번.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct FsWatching {
    pub roots: Vec<FsWatchedRoot>,
}

rpc_method! {
    /// 창의 루트(활성 워크트리)를 감시한다. 스트림이 열려 있는 동안 바뀐 경로가 프로젝트 범위 이벤트 `fs.changed` 로 나가고, 닫으면
    /// 감시를 놓는다. 항목은 감시 방식이다 — 첫 항목이 오면 그 뒤의 변경은 놓치지 않는다.
    pub FsWatchMethod: "fs.watch" {
        params: FsWatchParams,
        stream: FsWatching,
        tier: Read,
    }
}

type Key = (String, PathBuf);

/// 루트 하나의 엔진과 그것을 쓰는 임대 수.
struct Slot {
    refs: usize,
    engine: Engine,
}

struct Inner {
    fs: Arc<Fs>,
    bus: OnceLock<EventBus>,
    options: Options,
    slots: Mutex<HashMap<Key, Slot>>,
    warned: Arc<Mutex<HashSet<PathBuf>>>,
}

impl Inner {
    fn slots(&self) -> MutexGuard<'_, HashMap<Key, Slot>> {
        // 잠금 안에서는 표의 삽입 · 삭제만 한다.
        self.slots.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// 감시 서비스. 복제해도 같은 표를 가리킨다. Host 를 만든 뒤 버스를 붙인다([`Watch::attach`]).
#[derive(Clone)]
pub struct Watch {
    inner: Arc<Inner>,
}

impl Watch {
    /// 프로젝트 루트는 `fs` 의 레지스트리에서 찾는다.
    pub fn new(fs: Arc<Fs>) -> Self {
        Self::with_options(fs, Options::default())
    }

    pub fn with_options(fs: Arc<Fs>, options: Options) -> Self {
        Self {
            inner: Arc::new(Inner {
                fs,
                bus: OnceLock::new(),
                options,
                slots: Mutex::default(),
                warned: Arc::default(),
            }),
        }
    }

    /// 두 번째부터는 무시한다.
    pub fn attach(&self, bus: EventBus) {
        let _ = self.inner.bus.set(bus);
    }

    /// 지금 OS 감시를 건 루트 수.
    pub fn watched_roots(&self) -> usize {
        self.inner.slots().len()
    }

    /// 루트를 감시한다. 돌려준 임대를 버리면 놓는다. OS 감시를 다 건 뒤에 돌아온다(폴더별 감시는 루트를 걷는다 — 블로킹).
    pub fn acquire(&self, params: &FsWatchParams) -> Result<Lease, RpcError> {
        let bus = self.inner.bus.get().ok_or_else(|| {
            RpcError::internal("the file watch service is not attached to an event bus")
        })?;
        let roots = watch_roots(&self.inner.fs, params)?;
        let mut lease = Lease {
            inner: Arc::clone(&self.inner),
            held: Vec::new(),
        };
        for root in roots {
            let key = (params.project_id.clone(), root.clone());
            // 이미 보는 루트면 센다. 아니면 잠금 밖에서 엔진을 만든다 — 폴더별 감시는 루트를 걸어 오래 걸릴 수 있다.
            if let Some(status) = self.retain(&key) {
                lease.held.push((key, status));
                continue;
            }
            let sink = Sink {
                bus: bus.clone(),
                project_id: params.project_id.clone(),
                worktree: params.worktree.clone(),
                warned: Arc::clone(&self.inner.warned),
            };
            let engine = Engine::start(root.clone(), sink, self.inner.options.clone())
                .map_err(|e| watch_error(&root, &e))?;
            let status = {
                let mut slots = self.inner.slots();
                // 그 사이 다른 창이 같은 루트를 걸었으면 그것을 쓰고 만든 것은 버린다.
                let slot = slots.entry(key.clone()).or_insert(Slot { refs: 0, engine });
                slot.refs += 1;
                slot.engine.status()
            };
            lease.held.push((key, status));
        }
        Ok(lease)
    }

    fn retain(&self, key: &Key) -> Option<tokio::sync::watch::Receiver<WatchMode>> {
        let mut slots = self.inner.slots();
        let slot = slots.get_mut(key)?;
        slot.refs += 1;
        Some(slot.engine.status())
    }
}

/// 감시 하나의 임대. 버리면 그 루트를 쓰는 마지막 임대일 때 OS 감시를 푼다.
pub struct Lease {
    inner: Arc<Inner>,
    held: Vec<(Key, tokio::sync::watch::Receiver<WatchMode>)>,
}

impl Lease {
    /// 지금 방식. 읽은 값은 [`Self::changed`] 가 기다리지 않는다.
    pub fn snapshot(&mut self) -> FsWatching {
        FsWatching {
            roots: self
                .held
                .iter_mut()
                .map(|((_, root), status)| FsWatchedRoot {
                    path: root.display().to_string(),
                    mode: *status.borrow_and_update(),
                })
                .collect(),
        }
    }

    /// 어느 루트든 방식이 바뀔 때까지 기다린다. 엔진이 멈췄으면(감시 스레드가 끝남) 그 루트는 더 기다리지 않는다.
    pub async fn changed(&mut self) {
        let mut waits: Vec<_> = self
            .held
            .iter_mut()
            .map(|(_, status)| Some(Box::pin(status.changed())))
            .collect();
        std::future::poll_fn(|cx| {
            for slot in &mut waits {
                let Some(wait) = slot else { continue };
                match Pin::new(wait).poll(cx) {
                    Poll::Ready(Ok(())) => return Poll::Ready(()),
                    Poll::Ready(Err(_)) => *slot = None,
                    Poll::Pending => {}
                }
            }
            Poll::Pending
        })
        .await;
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        // 엔진은 잠금 밖에서 버린다(멈춤 신호만 보내 곧 돌아오지만, 표를 잡은 채 하지 않는다).
        let mut stopped = Vec::new();
        {
            let mut slots = self.inner.slots();
            for (key, _) in &self.held {
                if let Some(slot) = slots.get_mut(key) {
                    slot.refs -= 1;
                    if slot.refs == 0 {
                        stopped.extend(slots.remove(key));
                    }
                }
            }
        }
        drop(stopped);
    }
}

/// 감시할 루트: 활성 워크트리가 있으면 그 안의 프로젝트 루트, 없으면 프로젝트의 등록 루트.
fn watch_roots(fs: &Fs, params: &FsWatchParams) -> Result<Vec<PathBuf>, RpcError> {
    let projects = fs.projects.get().ok_or_else(|| {
        RpcError::internal("the file system service is not attached to a project registry")
    })?;
    let project = projects.get(&params.project_id).map_err(rpc_error)?;
    Ok(match &params.worktree {
        Some(top) => vec![crate::worktree_root(&project, top)?],
        None => Roots::new(project.roots.iter().map(PathBuf::from))
            .as_slice()
            .to_vec(),
    })
}

fn watch_error(root: &std::path::Path, e: &notify::Error) -> RpcError {
    let shown = root.display().to_string();
    match &e.kind {
        notify::ErrorKind::PathNotFound => {
            RpcError::new(code::NOT_FOUND, "not found").with_data(serde_json::Value::String(shown))
        }
        notify::ErrorKind::Io(io) if io.kind() == std::io::ErrorKind::NotFound => {
            RpcError::new(code::NOT_FOUND, "not found").with_data(serde_json::Value::String(shown))
        }
        _ => RpcError::internal(format!("cannot watch {shown}: {e}")),
    }
}

/// `fs.watch` 를 붙인다. 감시를 거는 일(폴더별 감시의 걷기)은 블로킹 스레드에서 한다. 스트림이 취소되거나 세션이 닫히면 pw-rpc 가
/// 이 future 를 버리고, 그와 함께 임대가 풀린다(걸고 있는 중이었으면 다 건 임대가 버려진다).
pub fn register(router: &mut Router, watch: Watch) {
    router.stream(FsWatchMethod, move |_ctx, params: FsWatchParams, sink| {
        let watch = watch.clone();
        async move {
            let mut lease = tokio::task::spawn_blocking(move || watch.acquire(&params))
                .await
                .map_err(|e| RpcError::internal(e.to_string()))??;
            loop {
                sink.send(lease.snapshot()).await?;
                lease.changed().await;
            }
        }
    });
}

#[cfg(test)]
mod tests;
