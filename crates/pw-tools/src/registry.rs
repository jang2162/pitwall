//! Host 쪽 registry 상태: 지금 쓰는 판(스냅숏 · 캐시), 가져오기 시점, 검색 · 조회.
//!
//! 처음 불릴 때 캐시를 읽고(디스크만), 가져오기(네트워크)는 뒤에서 돈다 — 호출을 기다리게 하지 않는다. 다시 가져오는 때: 아직
//! 한 번도 안 했을 때, 설정의 `[tools] registry` 가 바뀌었을 때, 최신 모드에서 하루가 지났을 때, 실패한 뒤 한 시간이 지났을 때.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use pw_core::config::ConfigStore;

use crate::fetch::Fetcher;
use crate::index::{Index, RegistryOrigin};
use crate::overrides::Overrides;
use crate::pinned::Pinned;
use crate::source::{self, Cache, Endpoints, LATEST_CHECK_INTERVAL_MS, RegistryMode};

/// 실패한 가져오기를 다시 하는 간격.
pub const RETRY_AFTER: Duration = Duration::from_secs(60 * 60);
/// 설정 열쇠(`settings.toml` 의 `[tools] registry`).
pub const SETTINGS_KEY: &str = "tools.registry";

type Clock = Arc<dyn Fn() -> SystemTime + Send + Sync>;

fn ms(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 설정을 어디서 읽나.
#[derive(Clone)]
enum ModeSource {
    Config(ConfigStore),
    #[cfg_attr(not(test), allow(dead_code))]
    Fixed(RegistryMode),
}

#[derive(Clone)]
struct Attached {
    cache: Cache,
    mode: ModeSource,
}

#[derive(Default)]
struct RefreshState {
    local_loaded: bool,
    running: bool,
    /// 마지막으로 시도한 (모드, 때, 성공).
    last: Option<(RegistryMode, SystemTime, bool)>,
    last_error: Option<String>,
}

pub struct ToolsRegistry {
    pinned: Pinned,
    overrides: Overrides,
    endpoints: Endpoints,
    fetcher: OnceLock<Result<Fetcher, String>>,
    #[cfg(test)]
    test_fetcher: Option<Fetcher>,
    host: OnceLock<Attached>,
    index: RwLock<Arc<Index>>,
    state: Mutex<RefreshState>,
    clock: Clock,
}

impl Default for ToolsRegistry {
    fn default() -> Self {
        ToolsRegistry::new()
    }
}

impl ToolsRegistry {
    /// 내장 고정 표 · 덮어쓰기 · 스냅숏으로 시작한다.
    pub fn new() -> ToolsRegistry {
        ToolsRegistry::with_parts(Pinned::builtin(), Overrides::builtin(), Index::snapshot())
    }

    pub fn with_parts(pinned: Pinned, overrides: Overrides, snapshot: Index) -> ToolsRegistry {
        ToolsRegistry {
            pinned,
            overrides,
            endpoints: Endpoints::default(),
            fetcher: OnceLock::new(),
            #[cfg(test)]
            test_fetcher: None,
            host: OnceLock::new(),
            index: RwLock::new(Arc::new(snapshot)),
            state: Mutex::new(RefreshState::default()),
            clock: Arc::new(SystemTime::now),
        }
    }

    /// 캐시 폴더(`$CACHE`)와 전역 설정을 붙인다. 두 번째부터는 무시한다.
    pub fn attach(&self, cache_dir: PathBuf, global: ConfigStore) {
        let _ = self.host.set(Attached {
            cache: Cache::new(cache_dir.join("registry")),
            mode: ModeSource::Config(global),
        });
    }

    #[cfg(test)]
    pub(crate) fn attach_fixed(&self, cache_dir: PathBuf, mode: RegistryMode) {
        let _ = self.host.set(Attached {
            cache: Cache::new(cache_dir.join("registry")),
            mode: ModeSource::Fixed(mode),
        });
    }

    #[cfg(test)]
    pub(crate) fn with_test_network(mut self, endpoints: Endpoints, clock: Clock) -> Self {
        self.endpoints = endpoints;
        self.test_fetcher = Some(Fetcher::for_loopback_tests());
        self.clock = clock;
        self
    }

    pub fn pinned(&self) -> &Pinned {
        &self.pinned
    }

    pub fn overrides(&self) -> &Overrides {
        &self.overrides
    }

    /// 지금 쓰는 판.
    pub fn index(&self) -> Arc<Index> {
        Arc::clone(&self.index.read().unwrap())
    }

    pub fn mode(&self) -> RegistryMode {
        match self.host.get().map(|h| &h.mode) {
            Some(ModeSource::Config(store)) => {
                let snapshot = store.snapshot();
                match snapshot
                    .get("settings", SETTINGS_KEY)
                    .and_then(|v| v.as_str())
                {
                    None | Some("pinned") => RegistryMode::Pinned,
                    Some("latest") => RegistryMode::Latest,
                    Some(other) => {
                        tracing::warn!(target: "pw_tools", "settings: unknown [tools] registry = {other:?}, using pinned");
                        RegistryMode::Pinned
                    }
                }
            }
            Some(ModeSource::Fixed(m)) => *m,
            None => RegistryMode::Pinned,
        }
    }

    pub fn fetching(&self) -> bool {
        self.state.lock().unwrap().running
    }

    pub fn last_error(&self) -> Option<String> {
        self.state.lock().unwrap().last_error.clone()
    }

    fn fetcher(&self) -> Result<Fetcher, String> {
        #[cfg(test)]
        if let Some(f) = &self.test_fetcher {
            return Ok(f.clone());
        }
        self.fetcher
            .get_or_init(|| Fetcher::new().map_err(|e| e.to_string()))
            .clone()
    }

    fn set_index(&self, index: Index) {
        *self.index.write().unwrap() = Arc::new(index);
    }

    /// 처음 한 번 캐시를 읽는다(디스크만). 고정 모드는 고정 판의 캐시만, 최신 모드는 마지막으로 본 판, 없으면 마지막 캐시.
    pub async fn load_local(&self) {
        let Some(host) = self.host.get().cloned() else {
            return;
        };
        {
            let mut state = self.state.lock().unwrap();
            if state.local_loaded {
                return;
            }
            state.local_loaded = true;
        }
        let mode = self.mode();
        let pinned = self.pinned.registry.clone();
        let loaded = tokio::task::spawn_blocking(move || match mode {
            RegistryMode::Pinned => host
                .cache
                .load(&pinned.version)
                .ok()
                .filter(|i| i.version == pinned.version),
            RegistryMode::Latest => host
                .cache
                .latest_state()
                .and_then(|s| host.cache.load(&s.version).ok())
                .or_else(|| host.cache.load_last()),
        })
        .await
        .ok()
        .flatten();
        if let Some(index) = loaded {
            self.set_index(index);
        }
    }

    /// 지금 가져와야 하는가.
    fn due(&self, mode: RegistryMode, now: SystemTime) -> bool {
        let state = self.state.lock().unwrap();
        if state.running || self.host.get().is_none() {
            return false;
        }
        let index = self.index();
        match state.last {
            None => match mode {
                // 고정 판이 이미 캐시에 있으면 받을 것이 없다.
                RegistryMode::Pinned => {
                    !(index.origin == RegistryOrigin::Cache
                        && index.version == self.pinned.registry.version)
                }
                // 하루 안에 본 판이 있으면(앱을 다시 띄운 경우) 그 판을 쓴다.
                RegistryMode::Latest => self
                    .host
                    .get()
                    .and_then(|h| h.cache.latest_state())
                    .is_none_or(|s| {
                        ms(now).saturating_sub(s.checked_at_ms) >= LATEST_CHECK_INTERVAL_MS
                            || index.version != s.version
                    }),
            },
            Some((last_mode, at, ok)) => {
                let age = now.duration_since(at).unwrap_or_default();
                last_mode != mode
                    || (!ok && age >= RETRY_AFTER)
                    || (mode == RegistryMode::Latest
                        && age.as_millis() as u64 >= LATEST_CHECK_INTERVAL_MS)
            }
        }
    }

    /// 캐시를 읽고, 가져와야 하면 뒤에서 가져온다. RPC 가 처음 불릴 때마다 부른다.
    pub async fn ensure(self: &Arc<Self>) {
        self.load_local().await;
        let mode = self.mode();
        if self.due(mode, (self.clock)()) {
            let this = Arc::clone(self);
            tokio::spawn(async move { this.refresh().await });
        }
    }

    /// 지금 모드로 가져온다(기다린다). 실패하면 마지막 캐시를 쓴다. 이미 도는 중이면 아무것도 하지 않는다.
    pub async fn refresh(&self) {
        let Some(host) = self.host.get().cloned() else {
            return;
        };
        {
            let mut state = self.state.lock().unwrap();
            if state.running {
                return;
            }
            state.running = true;
        }
        let mode = self.mode();
        let now = (self.clock)();
        let result = self.fetch(mode, &host.cache, ms(now)).await;
        let ok = result.is_ok();
        match result {
            Ok(index) => self.set_index(index),
            Err(e) => {
                tracing::warn!(target: "pw_tools", "registry: {e}");
                let fallback = tokio::task::spawn_blocking({
                    let cache = host.cache.clone();
                    move || cache.load_last()
                })
                .await
                .ok()
                .flatten();
                if let Some(index) = fallback {
                    let current = self.index();
                    // 이미 캐시를 쓰고 있으면(같은 판 · 더 늦게 받은 판) 바꾸지 않는다.
                    if current.origin == RegistryOrigin::Snapshot
                        || index.fetched_at_ms > current.fetched_at_ms
                    {
                        self.set_index(index);
                    }
                }
                self.state.lock().unwrap().last_error = Some(e);
            }
        }
        let mut state = self.state.lock().unwrap();
        if ok {
            state.last_error = None;
        }
        state.running = false;
        state.last = Some((mode, now, ok));
    }

    async fn fetch(&self, mode: RegistryMode, cache: &Cache, now_ms: u64) -> Result<Index, String> {
        let fetcher = self.fetcher()?;
        match mode {
            RegistryMode::Pinned => {
                let (c, v) = (cache.clone(), self.pinned.registry.version.clone());
                if let Ok(Ok(index)) = tokio::task::spawn_blocking(move || c.load(&v)).await {
                    return Ok(index);
                }
                source::fetch_pinned(&fetcher, &self.pinned.registry, cache, now_ms).await
            }
            RegistryMode::Latest => {
                source::fetch_latest(&fetcher, &self.endpoints, cache, now_ms).await
            }
        }
    }
}

#[cfg(test)]
mod tests;
