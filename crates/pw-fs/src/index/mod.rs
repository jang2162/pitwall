//! 파일 목록 인덱스(`docs/design/12-m1-plan.md` M1-12, 결정 M1-D5 · M1-D4): 감시하는 루트마다 파일 목록을 메모리에 두고
//! [`SearchFilesMethod`](`search.files`)로 퍼지 매칭한다.
//!
//! - **수명**: 감시([`crate::watch::Watch`])가 루트를 처음 걸 때(창이 `fs.watch` 를 열 때) 만들기 시작하고, 그 루트의 마지막 임대가
//!   풀릴 때 버린다. 만드는 일은 `ignore` 병렬 걷기이고 만들 때마다 프로젝트 범위 작업(`Task`, 제목 [`TASK_TITLE`])으로 보인다.
//!   디스크 캐시는 두지 않는다.
//! - **작업을 취소하면**(사람이 `task.cancel`) 그 루트의 목록을 버리고, 그 만들기를 기다리던 요청은 「취소됨」 오류로 끝난다. 감시가
//!   살아 있는 동안 다음 요청 · `rescan` · 버스 큐 넘침이 다시 만든다(그 사이의 `fs.changed` 는 버린다 — 다시 만들기가 디스크를
//!   처음부터 본다).
//! - **갱신**: 버스의 `fs.changed` 를 받아 그 경로의 지금 디스크 상태를 다시 본다(종류는 보지 않는다 — `removed` 말고는 힌트다).
//!   폴더면 그 아래를 다시 걷고, 없으면 그 경로와 그 아래를 지운다. ignore 파일(`.gitignore` · `.ignore`) · `.gitattributes` 가
//!   바뀌면 그 폴더 아래를 다시 걷는다. 넣기 전의 판정은 [`walk::Walk::ops`]. `rescan` 이나 버스 큐 넘침(`Lagged`)이면 처음부터 다시
//!   만든다 — 다시 만드는 동안은 옛 목록으로 답하고, 그사이 온 변경은 다 만든 뒤 적용한다(처음 만드는 동안 온 요청은 다 만들
//!   때까지 기다린다). 그사이 온 경로가 [`MAX_PENDING`] 을 넘으면 모으지 않고 한 번 더 처음부터 걷는다.
//! - **목록에 드는 것**: 파일. ignore 된 것(규칙은 [`crate::ignore_rules`])과 `.git` 안은 빠지고, 숨김 파일은 든다(`.github/…` ·
//!   `.env.example` 을 찾는다). 심링크는 `search.text` 와 같다 — 실제 대상이 루트(활성 워크트리 포함) 안인 링크만 따라가고(결정
//!   M1-D8), 링크를 지난 파일은 링크 쪽 경로로 든다. 루트 자체가 연 때와 다른 실제 경로로 풀리면(같은 이름의 링크로 바뀜) 비어
//!   있다. 벤더 · 생성 판정(M1-02)은 경로 규칙과 `.gitattributes` 로 한다(`search.text` 의 제외와 같다).
//! - **놓치는 것**: 루트 안 폴더 링크를 지난 경로(`node_modules/x -> ../packages/x`)는 감시가 실제 경로로 알리므로 링크 쪽 항목이
//!   갱신되지 않는다 — 다음 다시 만들기까지 남는다. 감시가 ignore 된 폴더 안의 변경을 그 폴더로 접는 것은 문제가 없다(그 안은
//!   목록에 없다).
//! - **감시하지 않는 루트**(창 없이 부른 요청 · 창과 다른 `worktree`)는 걸어서 만든 목록을 [`ADHOC_KEEP`] 동안 같은 루트의 요청끼리
//!   나눠 쓴다(동시에 온 요청도 걷기는 한 번). 그동안은 디스크 변경을 따라가지 않는다. 한 번에 [`ADHOC_MAX`] 루트까지 둔다.
//! - **자원 상한**: 걷기(감시하지 않는 루트) · 매칭은 Host 전체에서 [`MAX_SEARCHES`] 개까지 함께 돈다 — 넘으면 [`SEARCH_WAIT`] 까지
//!   기다리고 「바쁨」 오류. Readonly · Mobile 기기도 부르는 Read 메서드라 이 상한이 방어선이다(100만 파일 목록의 매칭 하나는 수십
//!   MB 를 잠깐 쓴다). 감시하는 루트를 만드는 걷기는 루트마다 하나라 이 상한에 넣지 않는다.
//! - 점수와 일치 위치는 [`matching`].

mod matching;
mod walk;

use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use pw_core::bus::{EventBus, Filter, Received, Scope};
use pw_core::{TaskProgress, TaskSpawner};
use pw_rpc::{Router, RpcError, code, rpc_method};
use serde::{Deserialize, Serialize};
use tokio::sync::{Semaphore, SemaphorePermit, watch};
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
/// 목록을 만드는 작업의 제목(`task.progress` · `task.finished` 의 `title`). Fluent 메시지 id 다 — 창이 자기 언어로 만든다
/// (`extensions/i18n-default/i18n/*/project.ftl`).
pub const TASK_TITLE: &str = "task-title-indexing-files";
/// 걷기 · 매칭이 Host 전체에서 함께 도는 수의 상한.
pub const MAX_SEARCHES: usize = 4;
/// 차례를 기다리는 최대 시간. 넘으면 「바쁨」 오류로 끝난다.
#[cfg(not(test))]
pub const SEARCH_WAIT: Duration = Duration::from_secs(5);
#[cfg(test)]
pub const SEARCH_WAIT: Duration = Duration::from_secs(1);
/// 감시하지 않는 루트의 목록을 나눠 쓰는 시간(다 걸은 때부터).
#[cfg(not(test))]
pub const ADHOC_KEEP: Duration = Duration::from_secs(3);
#[cfg(test)]
pub const ADHOC_KEEP: Duration = Duration::from_millis(500);
/// 감시하지 않는 루트의 목록을 함께 두는 수의 상한. 넘으면 가장 오래된 것을 버린다.
pub const ADHOC_MAX: usize = 4;
/// 만드는 동안 모아 두는 바뀐 경로 수의 상한. 넘으면 버리고 다 걸은 뒤 처음부터 한 번 더 걷는다.
pub const MAX_PENDING: usize = 65_536;

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
    /// 목록 없이 만드는 중(처음 · 취소 뒤). 요청은 다 만들 때까지 기다린다.
    Building,
    Ready,
    /// 작업이 취소돼 목록을 버렸다. 다음 요청 · `rescan` 이 다시 만든다.
    Cancelled,
    /// 버렸다(감시를 놓음). 기다리던 요청은 감시하지 않는 루트처럼 답한다.
    Closed,
}

/// 만들기 한 번의 멈춤 신호. 취소 · 닫기가 켠다.
type Stop = Arc<AtomicBool>;

/// 루트 하나의 목록.
struct RootIndex {
    project_id: String,
    root: PathBuf,
    /// 경로 판정 범위(심링크 규칙 · `.ignore` 를 읽는 범위). 프로젝트의 등록 루트와 활성 워크트리 안의 루트다.
    scope: Roots,
    state: Mutex<State>,
    phase: watch::Sender<Phase>,
    /// 버렸다(감시를 놓음). 다시 만들지 않는다.
    closed: AtomicBool,
}

#[derive(Default)]
struct State {
    /// 루트 기준 상대 경로 → 판정.
    files: BTreeMap<Arc<str>, Flags>,
    /// `files` 를 매칭용으로 펼친 것. 바뀌면 비우고 다음 요청이 다시 만든다.
    snapshot: Option<Arc<Snapshot>>,
    /// 지금 도는 만들기(다시 만들기 포함). 그사이 온 변경은 `pending` 에 모은다.
    build: Option<Stop>,
    pending: Vec<PathBuf>,
    /// 만드는 중에 다시 만들라는 요청이 왔다(또는 `pending` 이 넘쳤다).
    rescan: bool,
    /// 취소로 목록을 버렸다. 다시 만들 때까지 변경을 적용하지 않는다.
    dropped: bool,
}

impl State {
    fn current(&self, stop: &Stop) -> bool {
        self.build.as_ref().is_some_and(|b| Arc::ptr_eq(b, stop))
    }
}

impl RootIndex {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn walk<'a>(&'a self, stop: &'a AtomicBool) -> Walk<'a> {
        Walk::new(&self.root, &self.scope, stop)
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

    /// 만들기를 시작할 자리를 잡는다. 이미 도는 만들기가 있으면 그것이 끝난 뒤 한 번 더 걷게 하고 `None`.
    fn begin(&self) -> Option<Stop> {
        let mut state = self.state();
        if self.closed.load(Ordering::Relaxed) {
            return None;
        }
        if state.build.is_some() {
            state.rescan = true;
            return None;
        }
        let stop: Stop = Arc::default();
        state.build = Some(Arc::clone(&stop));
        if state.dropped {
            self.phase.send_replace(Phase::Building);
        }
        Some(stop)
    }

    /// 처음부터 걷고, 그사이 모인 변경을 적용한 뒤 바꿔 끼운다. 블로킹.
    fn build(&self, stop: &Stop) {
        'walk: loop {
            let mut files: BTreeMap<Arc<str>, Flags> = self.walk(stop).all().into_iter().collect();
            #[cfg(test)]
            tests::after_walk(&self.root);
            loop {
                let pending = {
                    let mut state = self.state();
                    // 취소 · 닫기가 이 만들기를 놓았다.
                    if stop.load(Ordering::Relaxed) || !state.current(stop) {
                        return;
                    }
                    if std::mem::take(&mut state.rescan) {
                        state.pending.clear();
                        continue 'walk;
                    }
                    if state.pending.is_empty() {
                        state.files = files;
                        state.snapshot = None;
                        state.build = None;
                        state.dropped = false;
                        self.phase.send_replace(Phase::Ready);
                        return;
                    }
                    std::mem::take(&mut state.pending)
                };
                self.walk(stop).refresh(&mut files, &pending);
            }
        }
    }

    /// 그 만들기를 멈추고 목록을 버린다(작업 취소). 이미 끝났거나 다른 만들기로 바뀌었으면 아무것도 하지 않는다.
    fn cancel(&self, stop: &Stop) {
        stop.store(true, Ordering::Relaxed);
        let mut state = self.state();
        if !state.current(stop) {
            return;
        }
        *state = State {
            dropped: true,
            ..State::default()
        };
        if !self.closed.load(Ordering::Relaxed) {
            self.phase.send_replace(Phase::Cancelled);
        }
    }

    /// 바뀐 경로를 적용한다. 만드는 중이면 모아 두고, 취소로 버린 목록이면 버린다. 블로킹.
    fn changed(&self, paths: Vec<PathBuf>) {
        if self.defer(&paths) {
            return;
        }
        // 디스크는 잠금 밖에서 본다. 변경은 한 소비자가 차례로 적용하지만 그사이 요청이 다시 만들기를 시작할 수 있다(취소 뒤) —
        // 적용할 때 다시 본다.
        let stop = AtomicBool::new(false);
        let ops = self.walk(&stop).ops(&paths);
        let mut state = self.state();
        if state.build.is_some() {
            drop(state);
            self.defer(&paths);
            return;
        }
        if state.dropped {
            return;
        }
        walk::apply(&mut state.files, ops);
        state.snapshot = None;
    }

    /// 만드는 중이면 모아 두고(넘치면 다시 걷게 한다) `true`. 버린 목록이면 버리고 `true`.
    fn defer(&self, paths: &[PathBuf]) -> bool {
        let mut state = self.state();
        if state.build.is_some() {
            if !state.rescan {
                if state.pending.len() + paths.len() > MAX_PENDING {
                    state.pending = Vec::new();
                    state.rescan = true;
                } else {
                    state.pending.extend_from_slice(paths);
                }
            }
            return true;
        }
        state.dropped
    }
}

type Key = (String, PathBuf);

struct Inner {
    fs: Arc<Fs>,
    tasks: OnceLock<TaskSpawner>,
    roots: Mutex<HashMap<Key, Arc<RootIndex>>>,
    /// 걷기 · 매칭의 차례([`MAX_SEARCHES`]).
    searches: Semaphore,
    /// 감시하지 않는 루트의 목록(루트 · `worktree` 별).
    adhoc: Mutex<HashMap<AdhocKey, Arc<Adhoc>>>,
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

    fn adhoc(&self) -> MutexGuard<'_, HashMap<AdhocKey, Arc<Adhoc>>> {
        self.adhoc.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// 차례를 기다린다(많아야 [`SEARCH_WAIT`]).
    async fn turn(&self) -> Result<SemaphorePermit<'_>, RpcError> {
        tokio::time::timeout(SEARCH_WAIT, self.searches.acquire())
            .await
            .map_err(|_| {
                RpcError::internal(format!(
                    "file search is busy: {MAX_SEARCHES} searches are already running"
                ))
            })?
            .map_err(|e| RpcError::internal(e.to_string()))
    }
}

type AdhocKey = (String, PathBuf, Option<String>);

/// 감시하지 않는 루트 하나의 목록. 처음 부른 요청이 걷고 그동안 온 요청은 그것을 기다린다.
#[derive(Default)]
struct Adhoc {
    list: tokio::sync::OnceCell<Arc<Snapshot>>,
    /// 다 걸은 때.
    made: OnceLock<Instant>,
}

impl Adhoc {
    fn fresh(&self) -> bool {
        self.made.get().is_none_or(|t| t.elapsed() < ADHOC_KEEP)
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
                searches: Semaphore::new(MAX_SEARCHES),
                adhoc: Mutex::default(),
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
        let stop: Stop = Arc::default();
        let entry = Arc::new(RootIndex {
            project_id: project_id.to_owned(),
            root: root.to_path_buf(),
            scope,
            state: Mutex::new(State {
                build: Some(Arc::clone(&stop)),
                ..State::default()
            }),
            phase: watch::Sender::new(Phase::Building),
            closed: AtomicBool::new(false),
        });
        self.inner.roots().insert(key, Arc::clone(&entry));
        self.spawn_build(entry, stop);
    }

    /// 루트의 목록을 버린다. 그 루트의 마지막 감시 임대가 풀릴 때 부른다.
    pub(crate) fn close(&self, project_id: &str, root: &Path) {
        let removed = self
            .inner
            .roots()
            .remove(&(project_id.to_owned(), root.to_path_buf()));
        if let Some(entry) = removed {
            entry.closed.store(true, Ordering::Relaxed);
            if let Some(stop) = entry.state().build.take() {
                stop.store(true, Ordering::Relaxed);
            }
            entry.phase.send_replace(Phase::Closed);
        }
    }

    /// 처음부터 다시 만든다. 이미 만드는 중이면 그 걷기가 끝난 뒤 한 번 더 걷는다.
    fn rebuild(&self, entry: &Arc<RootIndex>) {
        if let Some(stop) = entry.begin() {
            self.spawn_build(Arc::clone(entry), stop);
        }
    }

    /// 걷기는 이름 붙인 스레드에서 한다. 작업 자리가 붙어 있고 런타임 안이면 그 동안을 프로젝트 범위 작업으로 보인다 — 작업이
    /// 취소되면(사람이 `task.cancel`) 그 목록을 버린다([`RootIndex::cancel`]).
    fn spawn_build(&self, entry: Arc<RootIndex>, stop: Stop) {
        let (done_tx, done_rx) = tokio::sync::oneshot::channel::<()>();
        let worker = Arc::clone(&entry);
        let worker_stop = Arc::clone(&stop);
        let spawned = std::thread::Builder::new()
            .name("pw-fs-index".to_owned())
            .spawn(move || {
                // 걷기가 패닉하면 취소처럼 목록을 버린다 — 「만드는 중」 에 남으면 요청이 영영 기다린다.
                let built = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    worker.build(&worker_stop);
                }));
                if built.is_err() {
                    worker.cancel(&worker_stop);
                }
                let _ = done_tx.send(());
            });
        if let Err(e) = spawned {
            tracing::warn!(root = %entry.root.display(), error = %e, "file index thread not started");
            entry.cancel(&stop);
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
        tasks.spawn(TASK_TITLE, move |ctx| {
            ctx.report(TaskProgress {
                done: 0,
                total: None,
                message: Some(entry.root.display().to_string()),
            });
            // 작업이 취소되면 이 future 가 버려진다 — 한 번도 poll 되기 전이어도 버려지도록 guard 를 밖에서 만들어 넘긴다.
            let cancelled = CancelOnDrop {
                entry: Some((entry, stop)),
            };
            async move {
                let _ = done_rx.await;
                cancelled.disarm();
                Ok(())
            }
        });
    }

    /// 요청 하나. 목록이 준비되면(감시하지 않는 루트는 걸어서) 차례를 받아 매칭한다. 목록 · 매칭은 블로킹 스레드에서 한다.
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
        let mut lists = Vec::with_capacity(roots.len());
        for root in roots {
            let list = match self.inner.get(&params.project_id, &root) {
                Some(entry) => self.settled(entry).await?,
                None => None,
            };
            let list = match list {
                Some(entry) => List::Indexed(entry),
                None => {
                    let key = (params.project_id.clone(), root, params.worktree.clone());
                    List::Walked(self.walked(key, scope.clone()).await?)
                }
            };
            lists.push(list);
        }
        let _turn = self.inner.turn().await?;
        let query = params.query;
        blocking(move || {
            let snapshots: Vec<Arc<Snapshot>> = lists
                .into_iter()
                .map(|list| match list {
                    List::Indexed(entry) => entry.snapshot(),
                    List::Walked(snapshot) => snapshot,
                })
                .collect();
            Ok(matching::search(&snapshots, &query, limit as usize))
        })
        .await
    }

    /// 답할 목록이 생길 때까지 기다린다. 취소로 버린 목록이면 다시 만들게 한다. 기다리던 만들기가 취소되면 「취소됨」 오류,
    /// 감시를 놓았으면 `None`(감시하지 않는 루트처럼 답한다).
    async fn settled(&self, entry: Arc<RootIndex>) -> Result<Option<Arc<RootIndex>>, RpcError> {
        let mut phase = entry.phase.subscribe();
        loop {
            let now = *phase.borrow_and_update();
            match now {
                Phase::Ready => return Ok(Some(entry)),
                Phase::Closed => return Ok(None),
                Phase::Cancelled => self.rebuild(&entry),
                Phase::Building => {
                    let _ = phase.changed().await;
                    if *phase.borrow() == Phase::Cancelled {
                        return Err(RpcError::new(
                            code::CANCELLED,
                            "building the file list was cancelled",
                        ));
                    }
                }
            }
        }
    }

    /// 감시하지 않는 루트의 목록: [`ADHOC_KEEP`] 안에 걸은 것이 있으면 그것, 같은 루트를 걷는 중이면 그것을 기다린다.
    async fn walked(&self, key: AdhocKey, scope: Roots) -> Result<Arc<Snapshot>, RpcError> {
        let slot = {
            let mut table = self.inner.adhoc();
            table.retain(|_, slot| slot.fresh());
            match table.get(&key) {
                Some(slot) => Arc::clone(slot),
                None => {
                    if table.len() >= ADHOC_MAX
                        && let Some(oldest) = table
                            .iter()
                            .filter_map(|(k, slot)| slot.made.get().map(|t| (k.clone(), *t)))
                            .min_by_key(|(_, t)| *t)
                            .map(|(k, _)| k)
                    {
                        table.remove(&oldest);
                    }
                    let slot = Arc::new(Adhoc::default());
                    table.insert(key.clone(), Arc::clone(&slot));
                    slot
                }
            }
        };
        let list = slot
            .list
            .get_or_try_init(|| async {
                let _turn = self.inner.turn().await?;
                let root = key.1.clone();
                let snapshot = blocking(move || Ok(Arc::new(adhoc(&root, &scope)))).await?;
                let _ = slot.made.set(Instant::now());
                self.expire(key.clone(), Arc::clone(&slot));
                Ok::<_, RpcError>(snapshot)
            })
            .await;
        match list {
            Ok(list) => Ok(Arc::clone(list)),
            Err(e) => {
                // 걷지 못한 자리(바쁨)는 남기지 않는다. 함께 기다리던 요청은 스스로 다시 걷는다.
                let mut table = self.inner.adhoc();
                if slot.list.get().is_none()
                    && table.get(&key).is_some_and(|s| Arc::ptr_eq(s, &slot))
                {
                    table.remove(&key);
                }
                Err(e)
            }
        }
    }

    /// [`ADHOC_KEEP`] 뒤에 그 목록을 표에서 뺀다(다음 요청이 없어도 메모리를 놓게).
    fn expire(&self, key: AdhocKey, slot: Arc<Adhoc>) {
        let inner = Arc::downgrade(&self.inner);
        tokio::spawn(async move {
            tokio::time::sleep(ADHOC_KEEP).await;
            if let Some(inner) = inner.upgrade() {
                let mut table = inner.adhoc();
                if table.get(&key).is_some_and(|s| Arc::ptr_eq(s, &slot)) {
                    table.remove(&key);
                }
            }
        });
    }

    /// 루트의 목록이 다 만들어질 때까지 기다린다(시험 · 측정용). 목록이 없으면(감시하지 않음 · 취소됨) `false`.
    pub async fn ready(&self, project_id: &str, root: &Path) -> bool {
        let Some(entry) = self.inner.get(project_id, root) else {
            return false;
        };
        let mut phase = entry.phase.subscribe();
        let _ = phase.wait_for(|p| *p != Phase::Building).await;
        *phase.borrow() == Phase::Ready
    }
}

/// 요청 하나가 매칭할 목록.
enum List {
    Indexed(Arc<RootIndex>),
    Walked(Arc<Snapshot>),
}

/// 감시하지 않는 루트: 걸어서 목록을 만든다.
fn adhoc(root: &Path, scope: &Roots) -> Snapshot {
    #[cfg(test)]
    tests::adhoc_walked(root);
    let stop = AtomicBool::new(false);
    let files: BTreeMap<Arc<str>, Flags> =
        Walk::new(root, scope, &stop).all().into_iter().collect();
    Snapshot::new(root, &files)
}

/// 작업이 취소돼 버려지면 그 만들기를 멈추고 목록을 버린다.
struct CancelOnDrop {
    entry: Option<(Arc<RootIndex>, Stop)>,
}

impl CancelOnDrop {
    fn disarm(mut self) {
        self.entry = None;
    }
}

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        if let Some((entry, stop)) = self.entry.take() {
            entry.cancel(&stop);
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
