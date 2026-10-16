//! 현재 설정을 들고, 다시 읽고(파일 감시 포함), 결과를 이벤트 버스로 알린다.
//!
//! - 다시 읽기가 실패하면(파싱 오류 등) **이전 스냅숏을 그대로 쓰고** [`TOPIC_ERROR`] 를 낸다.
//! - 성공하면 값이 바뀐 파일 id 나 알림(마이그레이션 · 새 스키마)이 있을 때만 [`TOPIC_CHANGED`] 를 낸다.
//!   저장만 하고 내용이 같은 변경(에디터의 touch, 마이그레이션 뒤 재기록)은 조용하다.
//!
//! ⚠ 전역 설정은 [`Scope::Host`] 로 낸다. Host 범위 이벤트는 RPC `events.subscribe` 로 나가지 않는다(`crate::bus` 머리 주석) —
//! UI 는 지금 전역 설정 변경을 구독으로 받지 못하고 `config.get` 으로 다시 읽어야 한다.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use notify::event::{AccessKind, AccessMode};
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use super::source::{ConfigError, ConfigNotice, ConfigSnapshot, ConfigSource, PROJECT_BACKUP_DIR};
use crate::bus::{EventBus, Scope};

/// 다시 읽은 결과 값이나 알림이 달라졌다. 페이로드는 [`ConfigChanged`].
pub const TOPIC_CHANGED: &str = "config.changed";
/// 다시 읽기가 실패해 이전 설정을 유지한다. 페이로드는 [`ConfigFault`].
pub const TOPIC_ERROR: &str = "config.error";

/// 파일 감시의 기본 디바운스. 에디터의 「임시 파일 쓰기 → rename」 같은 연속 이벤트를 한 번의 다시 읽기로 묶는다.
pub const DEFAULT_DEBOUNCE: Duration = Duration::from_millis(200);

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct ConfigChanged {
    /// 병합 값이 달라졌거나 새로 생기거나 사라진 파일 id.
    pub changed: Vec<String>,
    pub notices: Vec<ConfigNotice>,
}

/// 마지막 읽기의 실패. 성공하면 사라진다.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct ConfigFault {
    pub message: String,
    /// 문제가 난 파일. 파일과 무관하면 `null`.
    pub path: Option<String>,
}

impl From<&ConfigError> for ConfigFault {
    fn from(e: &ConfigError) -> Self {
        Self {
            message: e.to_string(),
            path: e.path().map(|p| p.display().to_string()),
        }
    }
}

/// 설정 하나(전역 또는 프로젝트 하나)의 현재 상태. 복제해도 같은 상태를 가리킨다.
#[derive(Clone)]
pub struct ConfigStore {
    inner: Arc<Inner>,
}

struct Inner {
    source: ConfigSource,
    bus: EventBus,
    scope: Scope,
    state: Mutex<State>,
    /// 다시 읽기를 한 번에 하나로 묶는다. 겹치면 먼저 읽고 늦게 끝난 쪽이 새 스냅숏을 옛 것으로 덮을 수 있다.
    reloading: Mutex<()>,
}

struct State {
    snapshot: Arc<ConfigSnapshot>,
    fault: Option<ConfigFault>,
}

impl ConfigStore {
    /// 처음 읽는다. 실패해도 앱이 뜨도록 **내장 기본값만으로 시작**하고 그 오류를 [`Self::fault`] 와 [`TOPIC_ERROR`] 로 남긴다.
    /// 파일을 고치면 다음 다시 읽기에서 회복한다.
    pub fn open(source: ConfigSource, bus: EventBus, scope: Scope) -> Self {
        let (snapshot, fault) = match source.load() {
            Ok(snapshot) => (snapshot, None),
            Err(e) => (source.builtin_snapshot(), Some(ConfigFault::from(&e))),
        };
        let store = Self {
            inner: Arc::new(Inner {
                source,
                bus,
                scope,
                state: Mutex::new(State {
                    snapshot: Arc::new(snapshot),
                    fault: fault.clone(),
                }),
                reloading: Mutex::new(()),
            }),
        };
        if let Some(fault) = fault {
            store.publish(TOPIC_ERROR, &fault);
        }
        store
    }

    pub fn source(&self) -> &ConfigSource {
        &self.inner.source
    }

    pub fn snapshot(&self) -> Arc<ConfigSnapshot> {
        Arc::clone(&self.state().snapshot)
    }

    pub fn fault(&self) -> Option<ConfigFault> {
        self.state().fault.clone()
    }

    /// 디스크에서 다시 읽는다. 실패하면 이전 스냅숏을 유지하고 오류를 돌려준다(이벤트도 낸다).
    /// 디스크 IO 를 하므로 비동기 문맥에서는 `spawn_blocking` 으로 부른다.
    pub fn reload(&self) -> Result<Arc<ConfigSnapshot>, ConfigError> {
        let _serial = self
            .inner
            .reloading
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        match self.inner.source.load() {
            Ok(next) => {
                let next = Arc::new(next);
                let previous = {
                    let mut state = self.state();
                    state.fault = None;
                    std::mem::replace(&mut state.snapshot, Arc::clone(&next))
                };
                let changed = changed_ids(&previous, &next);
                if !changed.is_empty() || !next.notices.is_empty() {
                    self.publish(
                        TOPIC_CHANGED,
                        &ConfigChanged {
                            changed,
                            notices: next.notices.clone(),
                        },
                    );
                }
                Ok(next)
            }
            Err(e) => {
                let fault = ConfigFault::from(&e);
                self.state().fault = Some(fault.clone());
                self.publish(TOPIC_ERROR, &fault);
                Err(e)
            }
        }
    }

    /// 설정 폴더를 감시해 바뀌면 [`DEFAULT_DEBOUNCE`] 같은 간격으로 묶어 [`Self::reload`] 한다. `cancel` 이 취소되면 멈춘다.
    /// 감시 대상(없는 폴더면 그 조상)은 다시 읽을 때마다 다시 맞춘다 — `.pitwall/` 이 나중에 생겨도 따라간다.
    /// 감시 작업은 `runtime` 에 띄운다(Tauri 의 setup 처럼 tokio 문맥 밖에서도 부를 수 있게).
    pub fn watch(
        &self,
        runtime: &tokio::runtime::Handle,
        debounce: Duration,
        cancel: CancellationToken,
    ) -> notify::Result<()> {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut watcher =
            notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
                // 감시 자체의 오류도 다시 읽기로 이어 둔다 — 놓친 변경이 있을 수 있다.
                if res.as_ref().map_or(true, is_relevant) {
                    let _ = tx.send(());
                }
            })?;
        let mut watched = Vec::new();
        sync_watches(&mut watcher, &mut watched, self.source().watch_targets())?;
        let store = self.clone();
        runtime.spawn(async move {
            loop {
                tokio::select! {
                    () = cancel.cancelled() => return,
                    signal = rx.recv() => if signal.is_none() { return },
                }
                // 마지막 이벤트 뒤 `debounce` 동안 조용해지면 한 번 읽는다.
                loop {
                    tokio::select! {
                        () = cancel.cancelled() => return,
                        () = tokio::time::sleep(debounce) => break,
                        signal = rx.recv() => if signal.is_none() { return },
                    }
                }
                // 감시 대상을 먼저 맞추고 읽는다. 거꾸로 하면 읽은 뒤 새 폴더를 감시하기 전까지의 쓰기를 놓친다.
                let _ = sync_watches(&mut watcher, &mut watched, store.source().watch_targets());
                // 실패는 reload 가 이벤트로 알린다. 마이그레이션 쓰기까지 하는 블로킹 IO 라 워커 스레드 밖에서 돌린다.
                let reloading = store.clone();
                let _ = tokio::task::spawn_blocking(move || reloading.reload()).await;
            }
        });
        Ok(())
    }

    fn state(&self) -> MutexGuard<'_, State> {
        // 잠금 안에서는 값 교체만 하므로 패닉으로 오염돼도 상태가 어긋나지 않는다.
        self.inner
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn publish<T: Serialize>(&self, topic: &str, payload: &T) {
        // 페이로드는 이 모듈의 직렬화 가능한 구조체라 실패하지 않는다.
        let _ = self
            .inner
            .bus
            .publish(self.inner.scope.clone(), topic, payload);
    }
}

fn changed_ids(previous: &ConfigSnapshot, next: &ConfigSnapshot) -> Vec<String> {
    let before: BTreeMap<_, _> = previous.files.iter().map(|f| (&f.id, &f.value)).collect();
    let after: BTreeMap<_, _> = next.files.iter().map(|f| (&f.id, &f.value)).collect();
    let mut ids: Vec<String> = after
        .iter()
        .filter(|(id, value)| before.get(*id) != Some(*value))
        .map(|(id, _)| (*id).clone())
        .chain(
            before
                .keys()
                .filter(|id| !after.contains_key(*id))
                .map(|id| (*id).clone()),
        )
        .collect();
    ids.sort();
    ids
}

/// 설정에 영향이 있을 수 있는 이벤트인가. 읽기(Access)는 우리 자신의 다시 읽기도 만들므로 빼고(쓰기 닫힘만 남긴다),
/// 원본 보관 폴더와 `.toml` 이 아닌 파일(마이그레이션 임시 파일 포함)은 뺀다. 확장자 없는 경로는 폴더일 수 있어 남긴다.
fn is_relevant(event: &notify::Event) -> bool {
    if let EventKind::Access(kind) = event.kind
        && kind != AccessKind::Close(AccessMode::Write)
    {
        return false;
    }
    event.paths.iter().any(|path| {
        !path
            .components()
            .any(|c| c.as_os_str() == PROJECT_BACKUP_DIR)
            && path
                .extension()
                .is_none_or(|ext| ext.eq_ignore_ascii_case("toml"))
    })
}

fn sync_watches(
    watcher: &mut RecommendedWatcher,
    watched: &mut Vec<(PathBuf, bool)>,
    desired: Vec<(PathBuf, bool)>,
) -> notify::Result<()> {
    for target in watched.iter().filter(|t| !desired.contains(t)) {
        // 폴더가 지워졌으면 감시도 이미 풀려 있다.
        let _ = watcher.unwatch(&target.0);
    }
    watched.retain(|t| desired.contains(t));
    for target in desired {
        if watched.contains(&target) {
            continue;
        }
        let mode = if target.1 {
            RecursiveMode::Recursive
        } else {
            RecursiveMode::NonRecursive
        };
        watcher.watch(&target.0, mode)?;
        watched.push(target);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use serde_json::json;

    use super::*;
    use crate::bus::{Filter, Received, Subscription};

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn store(root: &Path) -> (ConfigStore, Subscription) {
        let bus = EventBus::new(64);
        let events = bus.subscribe(Filter::all().topics([TOPIC_CHANGED, TOPIC_ERROR]));
        let store = ConfigStore::open(ConfigSource::project(root), bus, Scope::Project("p".into()));
        (store, events)
    }

    fn next(events: &mut Subscription) -> Option<(String, serde_json::Value)> {
        match events.try_recv()? {
            Received::Event(e) => Some((e.topic.clone(), e.payload.clone())),
            Received::Lagged(n) => panic!("lagged {n}"),
        }
    }

    #[test]
    fn reload_publishes_changed_ids_only_when_values_change() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join(".pitwall/project.toml");
        write(&file, "schema = 1\nname = \"a\"\n");
        let (store, mut events) = store(tmp.path());
        assert_eq!(store.snapshot().get("project", "name"), Some(&json!("a")));

        // 내용이 같으면 조용하다.
        store.reload().unwrap();
        assert_eq!(next(&mut events), None);

        write(&file, "schema = 1\nname = \"b\"\n");
        write(&tmp.path().join(".pitwall/run/dev.toml"), "schema = 1\n");
        store.reload().unwrap();
        assert_eq!(
            next(&mut events),
            Some((
                TOPIC_CHANGED.into(),
                json!({"changed": ["project", "run/dev"], "notices": []})
            ))
        );
    }

    #[test]
    fn failed_reload_keeps_previous_snapshot_and_reports() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join(".pitwall/project.toml");
        write(&file, "schema = 1\nname = \"good\"\n");
        let (store, mut events) = store(tmp.path());
        let before = store.snapshot();

        write(&file, "schema = 1\nname = \n");
        assert!(store.reload().is_err());
        assert_eq!(store.snapshot(), before);
        assert_eq!(
            store.snapshot().get("project", "name"),
            Some(&json!("good"))
        );
        let fault = store.fault().expect("fault recorded");
        assert_eq!(fault.path, Some(file.display().to_string()));
        let (topic, payload) = next(&mut events).unwrap();
        assert_eq!(topic, TOPIC_ERROR);
        assert_eq!(payload["path"], json!(file.display().to_string()));

        // 고치면 회복하고 오류가 지워진다.
        write(&file, "schema = 1\nname = \"fixed\"\n");
        store.reload().unwrap();
        assert_eq!(store.fault(), None);
        assert_eq!(
            store.snapshot().get("project", "name"),
            Some(&json!("fixed"))
        );
    }

    #[test]
    fn broken_file_at_open_starts_from_builtin_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        write(&tmp.path().join(".pitwall/project.toml"), "= broken\n");
        let (store, mut events) = store(tmp.path());
        assert!(store.fault().is_some());
        assert_eq!(
            store.snapshot().get("project", "sample.greeting"),
            Some(&json!("hello"))
        );
        assert_eq!(next(&mut events).unwrap().0, TOPIC_ERROR);
    }

    #[test]
    fn relevance_filter() {
        use notify::event::{CreateKind, ModifyKind};
        let ev = |kind, path: &str| notify::Event::new(kind).add_path(PathBuf::from(path));
        let modify = EventKind::Modify(ModifyKind::Any);
        assert!(is_relevant(&ev(modify, "/r/.pitwall/project.toml")));
        assert!(is_relevant(&ev(
            EventKind::Create(CreateKind::Folder),
            "/r/.pitwall"
        )));
        assert!(!is_relevant(&ev(
            modify,
            "/r/.pitwall/.backup/project.toml.v0"
        )));
        assert!(!is_relevant(&ev(
            modify,
            "/r/.pitwall/.project.toml.pitwall-tmp"
        )));
        assert!(!is_relevant(&ev(
            EventKind::Access(AccessKind::Open(AccessMode::Read)),
            "/r/.pitwall/project.toml"
        )));
    }

    async fn wait_for(events: &mut Subscription, topic: &str) -> serde_json::Value {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            let received = tokio::time::timeout_at(deadline, events.recv())
                .await
                .unwrap_or_else(|_| panic!("no {topic} within 10s"));
            if let Some(Received::Event(e)) = received
                && e.topic == topic
            {
                return e.payload.clone();
            }
        }
    }

    /// 실제 파일 시스템 감시: 고치면 다시 읽고, 깨뜨리면 이전 설정을 유지하며, `.pitwall/` 이 나중에 생겨도 따라간다.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn watcher_reloads_on_change_and_keeps_previous_on_parse_error() {
        let tmp = tempfile::tempdir().unwrap();
        let (store, mut events) = store(tmp.path());
        let cancel = CancellationToken::new();
        store
            .watch(
                &tokio::runtime::Handle::current(),
                Duration::from_millis(50),
                cancel.clone(),
            )
            .unwrap();

        let file = tmp.path().join(".pitwall/project.toml");
        write(&file, "schema = 1\nname = \"one\"\n");
        let changed = wait_for(&mut events, TOPIC_CHANGED).await;
        assert_eq!(changed["changed"], json!(["project"]));
        assert_eq!(store.snapshot().get("project", "name"), Some(&json!("one")));

        write(&file, "schema = 1\nname = \n");
        wait_for(&mut events, TOPIC_ERROR).await;
        assert_eq!(store.snapshot().get("project", "name"), Some(&json!("one")));

        write(&file, "schema = 1\nname = \"two\"\n");
        wait_for(&mut events, TOPIC_CHANGED).await;
        assert_eq!(store.snapshot().get("project", "name"), Some(&json!("two")));
        cancel.cancel();
    }
}
