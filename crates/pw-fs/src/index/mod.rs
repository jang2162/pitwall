//! 파일 목록 인덱스(`docs/design/12-m1-plan.md` M1-12, 결정 M1-D5 · M1-D4): 감시하는 루트마다 파일 목록을 메모리에 두고
//! [`SearchFilesMethod`](`search.files`)로 퍼지 매칭한다.
//!
//! - **수명**: 감시([`crate::watch::Watch`])가 루트를 처음 걸 때(창이 `fs.watch` 를 열 때) 만들기 시작하고, 그 루트의 마지막 임대가
//!   풀릴 때 버린다. 만드는 일은 `ignore` 병렬 걷기이고 프로젝트 범위 작업(`Task`, 제목 [`TASK_TITLE`])으로 보인다 — 작업을 취소하면
//!   그 루트의 목록을 버린다. 디스크 캐시는 두지 않는다.
//! - **갱신**: 버스의 `fs.changed` 를 받아 그 경로의 지금 디스크 상태를 다시 본다(종류는 보지 않는다 — `removed` 말고는 힌트다).
//!   폴더면 그 아래를 다시 걷고, 없으면 그 경로와 그 아래를 지운다. ignore 파일(`.gitignore` · `.ignore`) · `.gitattributes` 가
//!   바뀌면 그 폴더 아래를 다시 걷는다. `rescan` 이나 버스 큐 넘침(`Lagged`)이면 처음부터 다시 만든다 — 다시 만드는 동안은 옛 목록으로
//!   답하고, 그사이 온 변경은 다 만든 뒤 적용한다(처음 만드는 동안 온 요청은 다 만들 때까지 기다린다).
//! - **목록에 드는 것**: 파일. ignore 된 것(규칙은 [`crate::ignore_rules`])과 `.git` 안은 빠지고, 숨김 파일은 든다(`.github/…` ·
//!   `.env.example` 을 찾는다). 심링크는 `search.text` 와 같다 — 실제 대상이 루트(활성 워크트리 포함) 안인 링크만 따라가고(결정
//!   M1-D8), 링크를 지난 파일은 링크 쪽 경로로 든다. 벤더 · 생성 판정(M1-02)은 경로 규칙과 `.gitattributes` 로 한다(`search.text` 의
//!   제외와 같다).
//! - **놓치는 것**: 루트 안 폴더 링크를 지난 경로(`node_modules/x -> ../packages/x`)는 감시가 실제 경로로 알리므로 링크 쪽 항목이
//!   갱신되지 않는다 — 다음 다시 만들기까지 남는다. 감시가 ignore 된 폴더 안의 변경을 그 폴더로 접는 것은 문제가 없다(그 안은
//!   목록에 없다).
//! - **감시하지 않는 루트**(창 없이 부른 요청)는 요청마다 걸어 답하고 기억하지 않는다.
//! - 점수와 일치 위치는 [`matching`].

mod matching;
mod walk;

use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};

use pw_core::bus::{EventBus, Filter, Received, Scope};
use pw_core::{TaskProgress, TaskSpawner};
use pw_rpc::{Router, RpcError, rpc_method};
use serde::{Deserialize, Serialize};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use crate::Fs;
use crate::scope::Roots;
use crate::watch::{FsChanged, FsWatchParams, TOPIC_CHANGED, watch_roots};
pub(crate) use matching::Snapshot;
use walk::{Flags, Walk};

/// 결과 수의 기본값.
pub const DEFAULT_LIMIT: u32 = 100;
/// `limit` 로 줄 수 있는 가장 큰 값.
pub const MAX_LIMIT: u32 = 1_000;
/// 질의의 최대 길이(글자). 매칭 비용이 질의 길이에 비례한다 — Readonly · Mobile 기기도 부르는 Read 메서드다.
pub const MAX_QUERY_CHARS: usize = 256;
/// 목록을 만드는 작업의 제목(`task.progress` · `task.finished` 의 `title`).
pub const TASK_TITLE: &str = "Indexing files";

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct SearchFilesParams {
    pub project_id: String,
    /// 창의 활성 워크트리 꼭대기(`ActiveWorktree.top`). 있으면 그 워크트리 안의 프로젝트 루트를, 없으면 프로젝트의 등록 루트를
    /// 찾는다(`fs.watch` 와 같다).
    #[serde(default)]
    pub worktree: Option<String>,
    /// 찾을 것. 공백으로 가른 낱말이 모두 맞아야 한다(낱말마다 퍼지, 대소문자는 대문자가 있을 때만 가린다). 비어 있으면 경로순
    /// 앞에서부터. 글자 수 상한 [`MAX_QUERY_CHARS`].
    pub query: String,
    /// 결과 수. 기본 [`DEFAULT_LIMIT`], 1 ~ [`MAX_LIMIT`].
    #[serde(default)]
    pub limit: Option<u32>,
}

/// 맞는 파일 하나.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct FileMatch {
    /// 절대 경로. 루트 안 심링크를 지났으면 링크 쪽 경로다 — 실제 경로는 `fs.stat` 이 준다.
    pub path: String,
    /// 그 루트(정규화한 실제 경로).
    pub root: String,
    /// 루트 기준 상대 경로(`/` 로 가른다). 매칭은 이 문자열에 한다.
    pub relative: String,
    /// `relative` 안의 일치 글자 위치(UTF-16 코드 단위, 오름차순, 중복 없음). 파일 이름에서 맞았으면 파일 이름 쪽이다.
    pub positions: Vec<u32>,
    /// 높을수록 위다. 같은 질의 안에서만 비교한다.
    pub score: u32,
    /// 벤더 파일(`vendor.yml` 의 경로 · `linguist-vendored`). 점수를 낮춘 것이다.
    pub vendored: bool,
    /// 생성 파일(generated 의 경로 규칙 · `linguist-generated`). 점수를 낮춘 것이다.
    pub generated: bool,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct SearchFiles {
    /// 점수순(같으면 짧은 경로 · 경로순).
    pub files: Vec<FileMatch>,
    /// 맞은 파일 수(`files` 는 그 앞부분).
    #[specta(type = pw_rpc::JsNumber)]
    pub matched: u64,
    /// 찾아본 파일 수(목록의 크기).
    #[specta(type = pw_rpc::JsNumber)]
    pub indexed: u64,
}

rpc_method! {
    /// 프로젝트의 파일을 이름 · 경로로 퍼지 매칭한다(`⌘⇧O` · Search Everywhere). 목록은 창이 `fs.watch` 로 보는 루트마다 메모리에
    /// 있고 변경을 따라간다(`pw_fs::index` 머리 주석). 처음 만드는 중이면 다 만들 때까지 기다린다. 빈 질의는 경로순 앞에서부터다.
    pub SearchFilesMethod: "search.files" {
        params: SearchFilesParams,
        result: SearchFiles,
        tier: Read,
    }
}

/// 인덱스 하나의 단계.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// 처음 만드는 중(답할 목록이 없다).
    Building,
    Ready,
    /// 버렸다(감시를 놓음 · 작업 취소). 기다리던 요청은 직접 걷는다.
    Closed,
}

/// 루트 하나의 목록.
struct RootIndex {
    project_id: String,
    root: PathBuf,
    /// 경로 판정 범위(심링크 규칙 · `.ignore` 를 읽는 범위). 프로젝트의 등록 루트와 활성 워크트리 안의 루트다.
    scope: Roots,
    state: Mutex<State>,
    phase: watch::Sender<Phase>,
    /// 걷기를 멈춘다(버렸다).
    stop: AtomicBool,
}

#[derive(Default)]
struct State {
    /// 루트 기준 상대 경로 → 판정.
    files: BTreeMap<Arc<str>, Flags>,
    /// `files` 를 매칭용으로 펼친 것. 바뀌면 비우고 다음 요청이 다시 만든다.
    snapshot: Option<Arc<Snapshot>>,
    /// 만드는 중이다(다시 만들기 포함). 그사이 온 변경은 `pending` 에 모은다.
    building: bool,
    pending: Vec<PathBuf>,
    /// 만드는 중에 다시 만들라는 요청이 왔다.
    rescan: bool,
}

impl RootIndex {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn walk(&self) -> Walk<'_> {
        Walk::new(&self.root, &self.scope, &self.stop)
    }

    /// 처음 만들기가 끝날 때까지 기다린다. 답할 목록이 있으면 `true`, 버렸으면 `false`.
    async fn settled(&self) -> bool {
        let mut phase = self.phase.subscribe();
        let _ = phase.wait_for(|p| *p != Phase::Building).await;
        *phase.borrow() == Phase::Ready
    }

    fn snapshot(&self) -> Arc<Snapshot> {
        let mut state = self.state();
        if let Some(snapshot) = &state.snapshot {
            return Arc::clone(snapshot);
        }
        let snapshot = Arc::new(Snapshot::new(&self.root, &state.files));
        state.snapshot = Some(Arc::clone(&snapshot));
        snapshot
    }

    /// 처음부터 걷고, 그사이 모인 변경을 적용한 뒤 바꿔 끼운다. 블로킹.
    fn build(&self) {
        'walk: loop {
            let mut files: BTreeMap<Arc<str>, Flags> = self.walk().all().into_iter().collect();
            #[cfg(test)]
            tests::after_walk(&self.root);
            loop {
                if self.stop.load(Ordering::Relaxed) {
                    return;
                }
                let pending = {
                    let mut state = self.state();
                    if std::mem::take(&mut state.rescan) {
                        state.pending.clear();
                        continue 'walk;
                    }
                    if state.pending.is_empty() {
                        state.files = files;
                        state.snapshot = None;
                        state.building = false;
                        break 'walk;
                    }
                    std::mem::take(&mut state.pending)
                };
                self.walk().refresh(&mut files, &pending);
            }
        }
        self.phase.send_replace(Phase::Ready);
    }

    /// 바뀐 경로를 적용한다. 만드는 중이면 모아 둔다. 블로킹.
    fn changed(&self, paths: Vec<PathBuf>) {
        {
            let mut state = self.state();
            if state.building {
                state.pending.extend(paths);
                return;
            }
        }
        // 디스크는 잠금 밖에서 본다. 변경은 한 소비자가 차례로 적용하므로 그사이 `files` 를 바꾸는 쪽이 없다 — 다시 만들기도 그
        // 소비자가 시작한다(`building` 을 켜고 나서 걷는다).
        let ops = self.walk().ops(&paths);
        let mut state = self.state();
        walk::apply(&mut state.files, ops);
        state.snapshot = None;
    }
}

type Key = (String, PathBuf);

struct Inner {
    fs: Arc<Fs>,
    tasks: OnceLock<TaskSpawner>,
    roots: Mutex<HashMap<Key, Arc<RootIndex>>>,
}

impl Inner {
    fn roots(&self) -> MutexGuard<'_, HashMap<Key, Arc<RootIndex>>> {
        // 잠금 안에서는 표의 삽입 · 삭제 · 조회만 한다.
        self.roots.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn get(&self, project_id: &str, root: &Path) -> Option<Arc<RootIndex>> {
        self.roots()
            .get(&(project_id.to_owned(), root.to_path_buf()))
            .cloned()
    }
}

/// 파일 목록 인덱스. 복제해도 같은 표를 가리킨다. 감시에 붙이고([`crate::watch::Watch::attach_index`]) Host 를 만든 뒤 버스 ·
/// 작업을 붙인다([`FileIndex::attach`]).
#[derive(Clone)]
pub struct FileIndex {
    inner: Arc<Inner>,
}

impl FileIndex {
    /// 프로젝트 루트는 `fs` 의 레지스트리에서 찾는다.
    pub fn new(fs: Arc<Fs>) -> Self {
        Self {
            inner: Arc::new(Inner {
                fs,
                tasks: OnceLock::new(),
                roots: Mutex::default(),
            }),
        }
    }

    /// 버스의 `fs.changed` 를 구독하고 목록을 만드는 작업을 띄울 곳을 정한다. 돌려준 future 가 변경을 적용한다 — Host 의 런타임에
    /// 띄운다(버스가 사라지면 끝난다). 구독은 이 호출 안에서 하므로 그 뒤의 변경은 놓치지 않는다. 두 번째부터 작업 자리는 무시한다.
    pub fn attach(
        &self,
        bus: &EventBus,
        tasks: TaskSpawner,
    ) -> impl Future<Output = ()> + Send + 'static {
        let _ = self.inner.tasks.set(tasks);
        let mut changes = bus.subscribe(Filter::all().topics([TOPIC_CHANGED]));
        let index = self.clone();
        async move {
            while let Some(received) = changes.recv().await {
                match received {
                    Received::Event(event) => {
                        let Scope::Project(project_id) = &event.scope else {
                            continue;
                        };
                        let Ok(changed) =
                            serde_json::from_value::<FsChanged>(event.payload.clone())
                        else {
                            continue;
                        };
                        let Some(entry) = index.inner.get(project_id, Path::new(&changed.root))
                        else {
                            continue;
                        };
                        if changed.rescan {
                            index.rebuild(&entry);
                            continue;
                        }
                        let paths = changed.paths.into_iter().map(PathBuf::from).collect();
                        let _ = tokio::task::spawn_blocking(move || entry.changed(paths)).await;
                    }
                    // 어느 루트의 변경을 놓쳤는지 모른다.
                    Received::Lagged(_) => {
                        let all: Vec<_> = index.inner.roots().values().cloned().collect();
                        for entry in &all {
                            index.rebuild(entry);
                        }
                    }
                }
            }
        }
    }

    /// 루트의 목록을 만들기 시작한다. 이미 있으면 아무것도 하지 않는다. 감시가 그 루트를 처음 걸 때 부른다.
    pub(crate) fn open(&self, project_id: &str, root: &Path, worktree: Option<&str>) {
        let key = (project_id.to_owned(), root.to_path_buf());
        if self.inner.roots().contains_key(&key) {
            return;
        }
        let scope = match self.inner.fs.roots(project_id, worktree) {
            Ok(scope) => scope,
            Err(e) => {
                tracing::warn!(root = %root.display(), error = %e.message, "file index not started");
                return;
            }
        };
        let entry = Arc::new(RootIndex {
            project_id: project_id.to_owned(),
            root: root.to_path_buf(),
            scope,
            state: Mutex::new(State {
                building: true,
                ..State::default()
            }),
            phase: watch::Sender::new(Phase::Building),
            stop: AtomicBool::new(false),
        });
        self.inner.roots().insert(key, Arc::clone(&entry));
        self.spawn_build(entry);
    }

    /// 루트의 목록을 버린다. 그 루트의 마지막 감시 임대가 풀릴 때 부른다.
    pub(crate) fn close(&self, project_id: &str, root: &Path) {
        let removed = self
            .inner
            .roots()
            .remove(&(project_id.to_owned(), root.to_path_buf()));
        if let Some(entry) = removed {
            entry.stop.store(true, Ordering::Relaxed);
            entry.phase.send_replace(Phase::Closed);
        }
    }

    /// 표의 그 루트가 `entry` 그대로면 버린다(그 사이 다시 열린 새 목록은 건드리지 않는다).
    fn close_if_current(&self, entry: &Arc<RootIndex>) {
        let current = self.inner.get(&entry.project_id, &entry.root);
        if current.is_some_and(|c| Arc::ptr_eq(&c, entry)) {
            self.close(&entry.project_id, &entry.root);
        }
        // 표에서 이미 빠졌어도 기다리는 요청을 깨운다.
        entry.phase.send_replace(Phase::Closed);
    }

    /// 처음부터 다시 만든다. 이미 만드는 중이면 그 걷기가 끝난 뒤 한 번 더 걷는다.
    fn rebuild(&self, entry: &Arc<RootIndex>) {
        {
            let mut state = entry.state();
            if state.building {
                state.rescan = true;
                return;
            }
            state.building = true;
        }
        self.spawn_build(Arc::clone(entry));
    }

    /// 걷기는 이름 붙인 스레드에서 한다. 작업 자리가 붙어 있고 런타임 안이면 그 동안을 프로젝트 범위 작업으로 보인다 — 작업이
    /// 취소되면(사람이 `task.cancel`) 그 루트의 목록을 버린다.
    fn spawn_build(&self, entry: Arc<RootIndex>) {
        let (done_tx, done_rx) = tokio::sync::oneshot::channel::<()>();
        let worker = Arc::clone(&entry);
        let index = self.clone();
        let spawned = std::thread::Builder::new()
            .name("pw-fs-index".to_owned())
            .spawn(move || {
                // 걷기가 패닉하면 그 목록을 버린다 — 「만드는 중」 에 남으면 요청이 영영 기다린다.
                let built =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| worker.build()));
                if built.is_err() {
                    index.close_if_current(&worker);
                }
                let _ = done_tx.send(());
            });
        if let Err(e) = spawned {
            tracing::warn!(root = %entry.root.display(), error = %e, "file index thread not started");
            self.close(&entry.project_id, &entry.root);
            return;
        }
        let Some(tasks) = self.inner.tasks.get() else {
            return;
        };
        if tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        let tasks = tasks.scoped(
            Scope::Project(entry.project_id.clone()),
            CancellationToken::new(),
        );
        let index = self.clone();
        tasks.spawn(TASK_TITLE, move |ctx| {
            ctx.report(TaskProgress {
                done: 0,
                total: None,
                message: Some(entry.root.display().to_string()),
            });
            // 작업이 취소되면 이 future 가 버려진다 — 한 번도 poll 되기 전이어도 버려지도록 guard 를 밖에서 만들어 넘긴다.
            let cancelled = CloseOnDrop {
                index,
                entry: Some(entry),
            };
            async move {
                let _ = done_rx.await;
                cancelled.disarm();
                Ok(())
            }
        });
    }

    /// 요청 하나. 목록 · 매칭은 블로킹 스레드에서 한다.
    pub async fn search(&self, params: SearchFilesParams) -> Result<SearchFiles, RpcError> {
        let limit = params.limit.unwrap_or(DEFAULT_LIMIT);
        if !(1..=MAX_LIMIT).contains(&limit) {
            return Err(RpcError::invalid_params(format!(
                "`limit` must be 1 to {MAX_LIMIT}"
            )));
        }
        if params.query.chars().count() > MAX_QUERY_CHARS {
            return Err(RpcError::invalid_params(format!(
                "`query` is longer than {MAX_QUERY_CHARS} characters"
            )));
        }
        let fs = Arc::clone(&self.inner.fs);
        let watch_params = FsWatchParams {
            project_id: params.project_id.clone(),
            worktree: params.worktree.clone(),
        };
        let (roots, scope) = blocking(move || {
            let roots = watch_roots(&fs, &watch_params)?;
            let scope = fs.roots(&watch_params.project_id, watch_params.worktree.as_deref())?;
            Ok((roots, scope))
        })
        .await?;
        let mut snapshots = Vec::with_capacity(roots.len());
        for root in roots {
            let entry = self.inner.get(&params.project_id, &root);
            let ready = match &entry {
                Some(entry) => entry.settled().await,
                None => false,
            };
            let snapshot = match entry {
                Some(entry) if ready => blocking(move || Ok(entry.snapshot())).await?,
                _ => {
                    let scope = scope.clone();
                    blocking(move || Ok(Arc::new(adhoc(&root, &scope)))).await?
                }
            };
            snapshots.push(snapshot);
        }
        let query = params.query;
        blocking(move || Ok(matching::search(&snapshots, &query, limit as usize))).await
    }

    /// 루트의 목록이 다 만들어질 때까지 기다린다(시험 · 측정용). 목록이 없으면(감시하지 않음) 바로 `false`.
    pub async fn ready(&self, project_id: &str, root: &Path) -> bool {
        match self.inner.get(project_id, root) {
            Some(entry) => entry.settled().await,
            None => false,
        }
    }
}

/// 감시하지 않는 루트: 걸어서 한 번 쓰고 버린다.
fn adhoc(root: &Path, scope: &Roots) -> Snapshot {
    let stop = AtomicBool::new(false);
    let files: BTreeMap<Arc<str>, Flags> =
        Walk::new(root, scope, &stop).all().into_iter().collect();
    Snapshot::new(root, &files)
}

/// 작업이 취소돼 버려지면 그 루트의 목록을 버린다.
struct CloseOnDrop {
    index: FileIndex,
    entry: Option<Arc<RootIndex>>,
}

impl CloseOnDrop {
    fn disarm(mut self) {
        self.entry = None;
    }
}

impl Drop for CloseOnDrop {
    fn drop(&mut self) {
        if let Some(entry) = self.entry.take() {
            self.index.close_if_current(&entry);
        }
    }
}

async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, RpcError> + Send + 'static,
) -> Result<T, RpcError> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| RpcError::internal(e.to_string()))?
}

/// `search.files` 를 붙인다.
pub fn register(router: &mut Router, index: FileIndex) {
    router.unary(SearchFilesMethod, move |_ctx, params: SearchFilesParams| {
        let index = index.clone();
        async move { index.search(params).await }
    });
}

#[cfg(test)]
mod tests;
