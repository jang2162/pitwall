//! 심볼 인덱스(`docs/design/15-m4-plan.md` M4-15, 07-extensibility.md §4.2 「LSP 서버 정의 없을 때 → tree-sitter 심볼 인덱스로
//! 폴백」): 프로젝트의 파일을 tree-sitter 로 파싱해 정의 쿼리(`tags.scm`)로 심볼(이름 · 종류 · 컨테이너 · 범위)을 뽑아 메모리에
//! 두고, 이름으로 퍼지 매칭한다(`index.symbols.search` — 심볼로 이동 · 클래스로 이동 M4-16)과 파일 하나의 심볼을 준다
//! (`index.file.symbols` — 아웃라인 대체 M4-10).
//!
//! - **파일 목록**: 파일 목록 인덱스(`pw_fs::index`, M1-12)의 목록을 따라간다([`pw_fs::index::FileListObserver`]) — ignore 된
//!   것 · `.git` 안은 이미 빠져 있고, 벤더 · 생성 판정이 붙은 것은 여기서 뺀다. 1 MiB([`MAX_FILE_BYTES`])를 넘는 파일, 정의 쿼리가
//!   없는 언어의 파일은 심볼이 없다(언어 판별은 [`lang`]).
//! - **수명**: 파일 목록 인덱스와 같다 — 창이 루트를 감시(`fs.watch`)하는 동안만 있고 마지막 감시가 풀리면 버린다. 디스크 캐시는
//!   두지 않는다(첫 판 — 성능은 `tests/perf.rs` 로 재고, 목표를 넘으면 M4-19 가 대응 행을 제안한다). 감시하지 않는 루트는 찾지
//!   않는다(결과의 `unindexed`).
//! - **만들기**: 목록이 처음 오면 모든 파일을 병렬로(코어 수 − 1 스레드) 파싱한다. 그동안은 프로젝트 범위 작업(`Task`, 제목
//!   [`TASK_TITLE`])이고 요청은 다 만들 때까지 기다린다. 작업을 취소하면 심볼을 모두 버리고 기다리던 요청은 `CANCELLED` 로
//!   끝난다 — 다음 요청이 다시 만든다(그사이의 목록 변경은 받아 둔다).
//! - **갱신**: 목록의 변경(`fs.changed` 를 적용한 것)마다 바뀐 파일만 다시 파싱한다. 그동안은 옛 심볼로 답한다. 목록을 처음부터
//!   다시 만들면(`rescan` 등) 모든 파일의 크기 · 수정 시각을 다시 보고 달라진 것만 다시 파싱한다.
//! - **신뢰**: 파싱만 하고 프로젝트 코드를 돌리지 않으므로 미신뢰 프로젝트에서도 돈다(결정 M4-D8).
//! - 결과의 위치는 줄(0 부터) · UTF-16 글자다([`extract`]). 매칭은 [`search`].

pub mod extract;
pub mod kind;
pub mod lang;
pub mod methods;
mod search;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, SystemTime};

use pw_core::bus::Scope;
use pw_core::{TaskProgress, TaskSpawner};
use pw_ext::Registry;
use pw_fs::Fs;
use pw_fs::index::{FileFlags, FileListObserver};
use pw_fs::watch::{FsWatchParams, watch_roots};
use pw_rpc::{RpcError, code};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use extract::{Extracted, Extractor, NO_PARENT};
pub use kind::IndexSymbolKind;
use lang::Languages;
pub use methods::register;
use methods::{
    DEFAULT_LIMIT, IndexFileSymbols, IndexFileSymbolsParams, IndexRange, IndexRootState,
    IndexRootStatus, IndexStatus, IndexStatusParams, IndexSymbol, IndexSymbolSearch,
    IndexSymbolSearchParams, MAX_LIMIT, MAX_OFFSET, MAX_QUERY_CHARS,
};

/// 색인을 만드는 작업의 제목(`task.progress` · `task.finished` 의 `title`). Fluent 메시지 id 다 — 창이 자기 언어로 만든다
/// (`extensions/i18n-default/i18n/*/project.ftl`).
pub const TASK_TITLE: &str = "task-title-indexing-symbols";
/// 이보다 큰 파일은 파싱하지 않는다(심볼이 없다).
pub const MAX_FILE_BYTES: u64 = 1024 * 1024;
/// 만드는 작업의 진행을 알리는 간격.
const PROGRESS_EVERY: Duration = Duration::from_millis(250);

/// 파일 내용이 그대로인지 보는 표지(크기 · 수정 시각).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Stamp {
    len: u64,
    modified: Option<SystemTime>,
}

impl Stamp {
    fn of(meta: &std::fs::Metadata) -> Self {
        Self {
            len: meta.len(),
            modified: meta.modified().ok(),
        }
    }
}

/// 파일 하나를 파싱한 결과.
pub(crate) struct Parsed {
    stamp: Stamp,
    /// 언어 확장의 언어 id.
    language: Arc<str>,
    symbols: Extracted,
}

/// 루트 하나의 단계.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// 처음 만드는 중(처음 · 취소 뒤). 요청은 다 만들 때까지 기다린다.
    Building,
    Ready,
    /// 작업이 취소돼 심볼을 버렸다. 다음 요청이 다시 만든다.
    Cancelled,
    /// 버렸다(감시를 놓음). 기다리던 요청은 색인이 없는 루트처럼 답한다.
    Closed,
}

/// 멈춤 신호. 일꾼 하나에 하나.
type Stop = Arc<AtomicBool>;

#[derive(Default)]
struct RootState {
    /// 목록에 든 파일(벤더 · 생성 제외) → 심볼. `None` 은 아직 읽지 않았거나 심볼이 없는 파일(코드가 아님 · 너무 큼 · 파싱 실패).
    files: BTreeMap<Arc<str>, Option<Arc<Parsed>>>,
    /// 다시 볼 파일.
    dirty: BTreeSet<Arc<str>>,
    /// 도는 일꾼.
    worker: Option<Stop>,
    /// 처음 만드는 중인 일꾼(작업이 이것을 취소한다).
    build: Option<Stop>,
    /// 취소로 심볼을 버렸다. 요청이 다시 만들 때까지 일꾼을 띄우지 않는다.
    cancelled: bool,
    closed: bool,
}

impl RootState {
    fn is(&self, slot: &Option<Stop>, stop: &Stop) -> bool {
        slot.as_ref().is_some_and(|s| Arc::ptr_eq(s, stop))
    }
}

/// 루트 하나의 색인.
struct Root {
    project_id: String,
    root: PathBuf,
    state: Mutex<RootState>,
    phase: watch::Sender<Phase>,
    /// 처음 만드는 작업의 진행(파일 수).
    done: AtomicU64,
    total: AtomicU64,
}

impl Root {
    fn state(&self) -> MutexGuard<'_, RootState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

type Key = (String, PathBuf);

struct Inner {
    fs: Arc<Fs>,
    languages: Languages,
    roots: Mutex<HashMap<Key, Arc<Root>>>,
    tasks: OnceLock<(TaskSpawner, tokio::runtime::Handle)>,
    threads: usize,
}

impl Inner {
    fn roots(&self) -> MutexGuard<'_, HashMap<Key, Arc<Root>>> {
        self.roots.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn get(&self, project_id: &str, root: &Path) -> Option<Arc<Root>> {
        self.roots()
            .get(&(project_id.to_owned(), root.to_path_buf()))
            .cloned()
    }
}

/// 심볼 인덱스. 복제해도 같은 표를 가리킨다. 파일 목록 인덱스에 붙이고([`SymbolIndex::observer`] → `FileIndex::observe`)
/// Host 를 만든 뒤 작업 자리를 붙인다([`SymbolIndex::attach`]).
#[derive(Clone)]
pub struct SymbolIndex {
    inner: Arc<Inner>,
}

impl SymbolIndex {
    /// 프로젝트 루트는 `fs` 의 레지스트리에서, 언어 · 쿼리는 `extensions` 에서 찾는다.
    pub fn new(fs: Arc<Fs>, extensions: Arc<Registry>) -> Self {
        let threads =
            std::thread::available_parallelism().map_or(1, |n| n.get().saturating_sub(1).max(1));
        Self {
            inner: Arc::new(Inner {
                fs,
                languages: Languages::new(extensions),
                roots: Mutex::default(),
                tasks: OnceLock::new(),
                threads,
            }),
        }
    }

    /// 파일 목록 인덱스에 붙일 관찰자(`pw_fs::index::FileIndex::observe`). 목록 · 변경은 채널에 넣기만 하고 이 호출이 띄운 스레드가
    /// 차례로 색인에 적용한다 — 파일 목록 인덱스의 잠금 안에서 일하지 않는다(파일 검색 · 변경 적용을 막지 않는다). 감시를 걸 때 ·
    /// 풀 때는 표에 자리를 두거나 빼기만 곧바로 한다(그 직후의 요청이 기다리거나 「색인 없음」 을 받게 — 버린 색인의 메모리는 그
    /// 스레드에서 놓는다). 관찰자가 버려지면 그 스레드도 끝난다.
    pub fn observer(&self) -> Arc<dyn FileListObserver> {
        let (tx, rx) = std::sync::mpsc::channel::<ListEvent>();
        let inner = Arc::clone(&self.inner);
        let spawned = std::thread::Builder::new()
            .name("pw-index-events".to_owned())
            .spawn(move || {
                for event in rx {
                    apply(&inner, event);
                }
            });
        if let Err(e) = spawned {
            tracing::warn!(error = %e, "symbol index event thread not started");
        }
        Arc::new(Observer {
            inner: Arc::clone(&self.inner),
            tx: Mutex::new(tx),
        })
    }

    /// 만드는 동안을 작업으로 보일 곳. tokio 런타임 안에서 부른다(목록은 런타임 밖 스레드에서도 오므로 지금 런타임을 붙잡아 둔다).
    /// 두 번째부터는 무시한다.
    pub fn attach(&self, tasks: TaskSpawner) {
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let _ = self.inner.tasks.set((tasks, handle));
        }
    }

    /// 요청 하나: 감시하는 루트마다 색인이 준비되면(처음 만드는 중이면 기다린다) 매칭한다. 매칭은 블로킹 스레드에서 한다.
    pub async fn search(
        &self,
        params: IndexSymbolSearchParams,
    ) -> Result<IndexSymbolSearch, RpcError> {
        let limit = params.limit.unwrap_or(DEFAULT_LIMIT);
        if !(1..=MAX_LIMIT).contains(&limit) {
            return Err(RpcError::invalid_params(format!(
                "`limit` must be 1 to {MAX_LIMIT}"
            )));
        }
        let offset = params.offset.unwrap_or(0);
        if offset > MAX_OFFSET {
            return Err(RpcError::invalid_params(format!(
                "`offset` must be at most {MAX_OFFSET}"
            )));
        }
        if params.query.chars().count() > MAX_QUERY_CHARS {
            return Err(RpcError::invalid_params(format!(
                "`query` is longer than {MAX_QUERY_CHARS} characters"
            )));
        }
        let roots = self
            .watched(&params.project_id, params.worktree.clone())
            .await?;
        let mut views = Vec::new();
        let mut unindexed = Vec::new();
        for root in roots {
            let ready = match self.inner.get(&params.project_id, &root) {
                Some(entry) => self.settled(entry).await?,
                None => None,
            };
            match ready {
                Some(entry) => views.push(view(&entry)),
                None => unindexed.push(root.display().to_string()),
            }
        }
        blocking(move || {
            Ok(search::search(
                &views,
                &params.query,
                params.kinds.as_deref(),
                offset as usize,
                limit as usize,
                unindexed,
            ))
        })
        .await
    }

    /// 파일 하나의 심볼. `text` 가 있으면 그것을, 색인에 다 읽은 것이 있으면 그것을, 아니면 디스크에서 읽어 파싱한다.
    pub async fn file_symbols(
        &self,
        params: IndexFileSymbolsParams,
    ) -> Result<IndexFileSymbols, RpcError> {
        let fs = Arc::clone(&self.inner.fs);
        let project_id = params.project_id.clone();
        let worktree = params.worktree.clone();
        let path = PathBuf::from(&params.path);
        let (readable, watched) = blocking(move || {
            let roots = fs.roots(&project_id, worktree.as_deref())?;
            let readable = roots
                .readable(&path)
                .map_err(|e| RpcError::invalid_params(e.to_string()))?;
            let watched = watch_roots(
                &fs,
                &FsWatchParams {
                    project_id,
                    worktree,
                },
            )?;
            Ok((readable, watched))
        })
        .await?;
        // 색인에 있으면 그것(텍스트를 주지 않았고, 다시 읽을 차례가 아닌 것).
        if params.text.is_none() {
            for root in &watched {
                let Ok(rel) = readable.real.strip_prefix(root) else {
                    continue;
                };
                let Some(entry) = self.inner.get(&params.project_id, root) else {
                    continue;
                };
                if *entry.phase.borrow() != Phase::Ready {
                    continue;
                }
                let rel = slash(rel);
                let state = entry.state();
                if state.dirty.contains(rel.as_str()) {
                    continue;
                }
                match state.files.get(rel.as_str()) {
                    Some(Some(parsed)) => {
                        return Ok(file_result(Some(&parsed.language), &parsed.symbols));
                    }
                    Some(None) => {}
                    None => {}
                }
            }
        }
        let inner = Arc::clone(&self.inner);
        let text = params.text;
        blocking(move || {
            let rel = readable
                .real
                .strip_prefix(&readable.root)
                .map_or_else(|_| readable.real.display().to_string(), slash);
            let text = match text {
                Some(text) => text,
                None => match read_limited(&readable.real) {
                    Some(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
                    None => return Ok(file_result(None, &Extracted::default())),
                },
            };
            Ok(parse_text(&inner, &rel, &text))
        })
        .await
    }

    /// `text` 를 `rel`(루트 기준 경로 — 언어 판별에 쓴다)의 내용으로 보고 심볼을 뽑는다. 블로킹. [`MAX_FILE_BYTES`] 를 넘거나 정의
    /// 쿼리가 있는 언어가 아니면 심볼이 없다.
    pub fn parse_text(&self, rel: &str, text: &str) -> IndexFileSymbols {
        parse_text(&self.inner, rel, text)
    }

    /// 감시하는 루트마다 색인 상태. 기다리지 않는다.
    pub async fn status(&self, params: IndexStatusParams) -> Result<IndexStatus, RpcError> {
        let roots = self.watched(&params.project_id, params.worktree).await?;
        let roots = roots
            .into_iter()
            .map(|root| {
                let Some(entry) = self.inner.get(&params.project_id, &root) else {
                    return IndexRootStatus {
                        root: root.display().to_string(),
                        state: IndexRootState::None,
                        files: 0,
                        symbols: 0,
                        pending: 0,
                    };
                };
                let state = match *entry.phase.borrow() {
                    Phase::Building => IndexRootState::Building,
                    Phase::Ready => IndexRootState::Ready,
                    Phase::Cancelled => IndexRootState::Cancelled,
                    Phase::Closed => IndexRootState::None,
                };
                let st = entry.state();
                let parsed = st.files.values().flatten();
                IndexRootStatus {
                    root: root.display().to_string(),
                    state,
                    files: parsed.clone().count() as u64,
                    symbols: parsed.map(|p| p.symbols.symbols.len() as u64).sum(),
                    pending: st.dirty.len() as u64,
                }
            })
            .collect();
        Ok(IndexStatus { roots })
    }

    /// 루트의 색인이 다 만들어질 때까지 기다린다(시험 · 측정용). 색인이 없으면(감시하지 않음 · 취소됨) `false`.
    pub async fn ready(&self, project_id: &str, root: &Path) -> bool {
        let Some(entry) = self.inner.get(project_id, root) else {
            return false;
        };
        let mut phase = entry.phase.subscribe();
        let _ = phase.wait_for(|p| *p != Phase::Building).await;
        *phase.borrow() == Phase::Ready
    }

    /// 루트의 색인이 다시 읽을 파일 없이 쉬고 있을 때까지 기다린다(시험 · 측정용). 색인이 없으면 `false`.
    pub async fn idle(&self, project_id: &str, root: &Path) -> bool {
        loop {
            let Some(entry) = self.inner.get(project_id, root) else {
                return false;
            };
            {
                let st = entry.state();
                if st.dirty.is_empty() && st.worker.is_none() {
                    return *entry.phase.borrow() == Phase::Ready;
                }
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    /// 루트들에 든 심볼 수와 그 힙 크기의 근사(바이트, 측정용).
    pub fn footprint(&self) -> (u64, u64) {
        let roots: Vec<Arc<Root>> = self.inner.roots().values().cloned().collect();
        let mut symbols = 0u64;
        let mut bytes = 0u64;
        for root in roots {
            let st = root.state();
            for (rel, parsed) in &st.files {
                bytes += rel.len() as u64 + 32;
                if let Some(p) = parsed {
                    symbols += p.symbols.symbols.len() as u64;
                    bytes += p.symbols.heap_bytes() as u64 + std::mem::size_of::<Parsed>() as u64;
                }
            }
        }
        (symbols, bytes)
    }

    async fn watched(
        &self,
        project_id: &str,
        worktree: Option<String>,
    ) -> Result<Vec<PathBuf>, RpcError> {
        let fs = Arc::clone(&self.inner.fs);
        let params = FsWatchParams {
            project_id: project_id.to_owned(),
            worktree,
        };
        blocking(move || watch_roots(&fs, &params)).await
    }

    /// 답할 색인이 생길 때까지 기다린다. 취소로 버린 색인이면 다시 만들게 한다. 기다리던 만들기가 취소되면 `CANCELLED`, 감시를
    /// 놓았으면 `None`.
    async fn settled(&self, entry: Arc<Root>) -> Result<Option<Arc<Root>>, RpcError> {
        let mut phase = entry.phase.subscribe();
        loop {
            let now = *phase.borrow_and_update();
            match now {
                Phase::Ready => return Ok(Some(entry)),
                Phase::Closed => return Ok(None),
                Phase::Cancelled => restart(&self.inner, &entry),
                Phase::Building => {
                    if phase.changed().await.is_err() {
                        return Ok(None);
                    }
                    if *phase.borrow() == Phase::Cancelled {
                        return Err(RpcError::new(
                            code::CANCELLED,
                            "building the symbol index was cancelled",
                        ));
                    }
                }
            }
        }
    }
}

fn parse_text(inner: &Inner, rel: &str, text: &str) -> IndexFileSymbols {
    if text.len() as u64 > MAX_FILE_BYTES {
        return file_result(None, &Extracted::default());
    }
    let Some(language) = inner.languages.for_path(rel, Some(head(text))) else {
        return file_result(None, &Extracted::default());
    };
    let stop = AtomicBool::new(false);
    let symbols = Extractor::default()
        .extract(&language, text, &stop)
        .unwrap_or_default();
    file_result(Some(&language.id), &symbols)
}

/// 질의 하나 동안 붙잡을 루트의 심볼(경로순, 심볼이 있는 파일만).
fn view(entry: &Root) -> search::RootView {
    let st = entry.state();
    let files = st
        .files
        .iter()
        .filter_map(|(rel, parsed)| {
            let parsed = parsed.as_ref()?;
            (!parsed.symbols.symbols.is_empty()).then(|| (Arc::clone(rel), Arc::clone(parsed)))
        })
        .collect();
    search::RootView {
        root: entry.root.clone(),
        files,
    }
}

fn file_result(language: Option<&str>, symbols: &Extracted) -> IndexFileSymbols {
    IndexFileSymbols {
        language: language.map(str::to_owned),
        symbols: symbols
            .symbols
            .iter()
            .map(|s| IndexSymbol {
                name: symbols.name(s).to_owned(),
                kind: s.kind,
                parent: (s.parent != NO_PARENT).then_some(s.parent),
                range: IndexRange::from_span(s.range),
                selection_range: IndexRange::from_span(s.selection),
            })
            .collect(),
    }
}

/// 파일 목록 인덱스의 목록 사건. 받은 순서대로 적용한다.
enum ListEvent {
    Rebuilt {
        project_id: String,
        root: PathBuf,
        files: Vec<(Arc<str>, FileFlags)>,
    },
    Changed {
        project_id: String,
        root: PathBuf,
        removed: Vec<String>,
        inserted: Vec<(Arc<str>, FileFlags)>,
    },
    /// 감시가 풀려 표에서 뺀 색인 — 이 스레드에서 놓는다(심볼 수십 MB 를 해제하는 일을 감시 표의 잠금 안에서 하지 않게).
    Release(Arc<Root>),
}

/// 파일 목록 인덱스의 목록을 받는 쪽. 콜백은 잠금 안이므로 채널에 넣거나 표의 자리 하나만 바꾸고 곧바로 돌아온다.
struct Observer {
    inner: Arc<Inner>,
    tx: Mutex<std::sync::mpsc::Sender<ListEvent>>,
}

impl Observer {
    fn send(&self, event: ListEvent) {
        // 받는 스레드가 없으면(띄우지 못함) 색인이 없을 뿐이다.
        let _ = self
            .tx
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .send(event);
    }
}

impl FileListObserver for Observer {
    /// 「만드는 중」 자리를 둔다 — 목록이 오기 전에 온 요청도 다 만들 때까지 기다린다(파일 목록 인덱스와 같다).
    fn opened(&self, project_id: &str, root: &Path) {
        self.inner
            .roots()
            .entry((project_id.to_owned(), root.to_path_buf()))
            .or_insert_with(|| {
                Arc::new(Root {
                    project_id: project_id.to_owned(),
                    root: root.to_path_buf(),
                    state: Mutex::default(),
                    phase: watch::Sender::new(Phase::Building),
                    done: AtomicU64::new(0),
                    total: AtomicU64::new(0),
                })
            });
    }

    fn rebuilt(&self, project_id: &str, root: &Path, files: Vec<(Arc<str>, FileFlags)>) {
        self.send(ListEvent::Rebuilt {
            project_id: project_id.to_owned(),
            root: root.to_path_buf(),
            files,
        });
    }

    fn changed(
        &self,
        project_id: &str,
        root: &Path,
        removed: Vec<String>,
        inserted: Vec<(Arc<str>, FileFlags)>,
    ) {
        self.send(ListEvent::Changed {
            project_id: project_id.to_owned(),
            root: root.to_path_buf(),
            removed,
            inserted,
        });
    }

    /// 표에서 빼고 일꾼을 멈춘다. 기다리던 요청은 「색인 없음」 으로 답한다. 메모리는 사건 스레드가 놓는다.
    fn dropped(&self, project_id: &str, root: &Path) {
        let removed = self
            .inner
            .roots()
            .remove(&(project_id.to_owned(), root.to_path_buf()));
        if let Some(entry) = removed {
            {
                let mut st = entry.state();
                st.closed = true;
                if let Some(stop) = st.worker.take() {
                    stop.store(true, Ordering::Relaxed);
                }
                st.build = None;
            }
            entry.phase.send_replace(Phase::Closed);
            self.send(ListEvent::Release(entry));
        }
    }
}

/// 목록 사건 하나를 색인에 적용한다(사건 스레드).
fn apply(inner: &Arc<Inner>, event: ListEvent) {
    match event {
        ListEvent::Rebuilt {
            project_id,
            root,
            files,
        } => {
            #[cfg(test)]
            tests::before_event(&root);
            rebuilt(inner, &project_id, &root, &files);
        }
        ListEvent::Changed {
            project_id,
            root,
            removed,
            inserted,
        } => {
            #[cfg(test)]
            tests::before_event(&root);
            changed(inner, &project_id, &root, &removed, &inserted);
        }
        ListEvent::Release(entry) => {
            let mut st = entry.state();
            st.files.clear();
            st.dirty.clear();
        }
    }
}

/// 목록 전부. 감시가 건 루트(자리가 있는 것)에만 적용한다 — 감시가 풀린 뒤 늦게 닿은 목록은 버린다.
fn rebuilt(inner: &Arc<Inner>, project_id: &str, root: &Path, files: &[(Arc<str>, FileFlags)]) {
    let Some(entry) = inner.get(project_id, root) else {
        return;
    };
    let mut st = entry.state();
    let mut old = std::mem::take(&mut st.files);
    st.files = files
        .iter()
        .filter(|(_, flags)| !excluded(*flags))
        .map(|(rel, _)| (Arc::clone(rel), old.remove(rel).flatten()))
        .collect();
    // 크기 · 수정 시각이 같으면 다시 파싱하지 않는다.
    st.dirty = st.files.keys().cloned().collect();
    kick(inner, &entry, &mut st);
}

fn changed(
    inner: &Arc<Inner>,
    project_id: &str,
    root: &Path,
    removed: &[String],
    inserted: &[(Arc<str>, FileFlags)],
) {
    let Some(entry) = inner.get(project_id, root) else {
        return;
    };
    let mut st = entry.state();
    let mut gone: HashMap<Arc<str>, Option<Arc<Parsed>>> = HashMap::new();
    for rel in removed {
        if rel.is_empty() {
            gone.extend(std::mem::take(&mut st.files));
            st.dirty.clear();
            continue;
        }
        // 그 경로와 `rel/` 로 시작하는 것: `'0'` 은 `'/'` 바로 다음 바이트다(`a-b.rs` 처럼 `a` 와 `a/` 사이에 오는 것은 건드리지
        // 않는다).
        let (lo, hi) = (format!("{rel}/"), format!("{rel}0"));
        let mut keys: Vec<Arc<str>> = st
            .files
            .range::<str, _>((
                std::ops::Bound::Included(lo.as_str()),
                std::ops::Bound::Excluded(hi.as_str()),
            ))
            .map(|(k, _)| Arc::clone(k))
            .collect();
        if let Some((k, _)) = st.files.get_key_value(rel.as_str()) {
            keys.push(Arc::clone(k));
        }
        for k in keys {
            st.dirty.remove(&k);
            if let Some(v) = st.files.remove(&k) {
                gone.insert(k, v);
            }
        }
    }
    for (rel, flags) in inserted {
        if excluded(*flags) {
            continue;
        }
        // 내용이 바뀐 파일은 지움 · 넣음 둘 다에 있다 — 다시 읽을 때까지 옛 심볼로 답한다.
        let old = gone.remove(rel).flatten();
        st.files.insert(Arc::clone(rel), old);
        st.dirty.insert(Arc::clone(rel));
    }
    kick(inner, &entry, &mut st);
}

fn excluded(flags: FileFlags) -> bool {
    flags.vendored || flags.generated
}

/// 다시 볼 파일이 있고 일꾼이 없으면 띄운다. 처음 만드는 중이면 작업으로 보인다.
fn kick(inner: &Arc<Inner>, entry: &Arc<Root>, st: &mut RootState) {
    if st.closed || st.cancelled || st.worker.is_some() {
        return;
    }
    if st.dirty.is_empty() {
        // 읽을 파일이 없는 루트(빈 폴더)도 다 만든 것이다 — 아니면 요청이 「만드는 중」 을 영영 기다린다.
        if *entry.phase.borrow() == Phase::Building {
            entry.phase.send_replace(Phase::Ready);
        }
        return;
    }
    let stop: Stop = Arc::default();
    st.worker = Some(Arc::clone(&stop));
    let building = *entry.phase.borrow() == Phase::Building;
    if building {
        st.build = Some(Arc::clone(&stop));
        entry.done.store(0, Ordering::Relaxed);
        entry.total.store(st.dirty.len() as u64, Ordering::Relaxed);
    }
    let worker_inner = Arc::clone(inner);
    let worker_entry = Arc::clone(entry);
    let worker_stop = Arc::clone(&stop);
    let spawned = std::thread::Builder::new()
        .name("pw-index".to_owned())
        .spawn(move || run(&worker_inner, &worker_entry, &worker_stop));
    if let Err(e) = spawned {
        tracing::warn!(root = %entry.root.display(), error = %e, "symbol index thread not started");
        st.worker = None;
        return;
    }
    if building {
        spawn_task(inner, entry, stop);
    }
}

/// 취소로 버린 색인을 다시 만든다.
fn restart(inner: &Arc<Inner>, entry: &Arc<Root>) {
    let mut st = entry.state();
    if !st.cancelled || st.closed {
        return;
    }
    st.cancelled = false;
    st.dirty = st.files.keys().cloned().collect();
    entry.phase.send_replace(Phase::Building);
    kick(inner, entry, &mut st);
}

/// 처음 만드는 동안을 프로젝트 범위 작업으로 보인다. 작업이 취소되면(사람이 `task.cancel`) 그 만들기를 멈추고 심볼을 버린다.
fn spawn_task(inner: &Arc<Inner>, entry: &Arc<Root>, stop: Stop) {
    let Some((tasks, handle)) = inner.tasks.get() else {
        return;
    };
    let _enter = handle.enter();
    let tasks = tasks.scoped(
        Scope::Project(entry.project_id.clone()),
        CancellationToken::new(),
    );
    let entry = Arc::clone(entry);
    tasks.spawn(TASK_TITLE, move |ctx| {
        let report = {
            let entry = Arc::clone(&entry);
            move || TaskProgress {
                done: entry.done.load(Ordering::Relaxed),
                total: Some(entry.total.load(Ordering::Relaxed)),
                message: Some(entry.root.display().to_string()),
            }
        };
        ctx.report(report());
        let mut phase = entry.phase.subscribe();
        // 작업이 취소되면 이 future 가 버려진다 — 한 번도 poll 되기 전이어도 버려지도록 guard 를 밖에서 만들어 넘긴다.
        let guard = CancelOnDrop {
            entry: Some((entry, stop)),
        };
        async move {
            loop {
                tokio::select! {
                    changed = phase.wait_for(|p| *p != Phase::Building) => {
                        let _ = changed;
                        break;
                    }
                    () = tokio::time::sleep(PROGRESS_EVERY) => ctx.report(report()),
                }
            }
            ctx.report(report());
            guard.disarm();
            Ok(())
        }
    });
}

/// 작업이 취소돼 버려지면 그 만들기를 멈추고 심볼을 버린다.
struct CancelOnDrop {
    entry: Option<(Arc<Root>, Stop)>,
}

impl CancelOnDrop {
    fn disarm(mut self) {
        self.entry = None;
    }
}

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        if let Some((entry, stop)) = self.entry.take() {
            cancel(&entry, &stop);
        }
    }
}

/// 만들기 `stop` 을 멈추고 심볼을 버린다. 이미 끝났거나 다른 만들기면 아무것도 하지 않는다.
fn cancel(entry: &Root, stop: &Stop) {
    let mut st = entry.state();
    if !st.is(&st.build, stop) {
        return;
    }
    stop.store(true, Ordering::Relaxed);
    st.build = None;
    if st.is(&st.worker, stop) {
        st.worker = None;
    }
    for parsed in st.files.values_mut() {
        *parsed = None;
    }
    st.dirty.clear();
    st.cancelled = true;
    drop(st);
    entry.phase.send_replace(Phase::Cancelled);
}

/// 다시 볼 파일 하나.
struct Job {
    rel: Arc<str>,
    old: Option<Arc<Parsed>>,
}

enum Outcome {
    /// 크기 · 수정 시각이 같다.
    Keep,
    Set(Option<Arc<Parsed>>),
}

/// 일꾼: 다시 볼 파일이 없을 때까지 묶음으로 가져가 파싱한다. 블로킹.
fn run(inner: &Inner, entry: &Root, stop: &Stop) {
    let worked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        loop {
            let jobs: Vec<Job> = {
                let mut st = entry.state();
                if stop.load(Ordering::Relaxed) || !st.is(&st.worker, stop) {
                    return;
                }
                if st.dirty.is_empty() {
                    st.worker = None;
                    if st.is(&st.build, stop) {
                        st.build = None;
                        drop(st);
                        entry.phase.send_replace(Phase::Ready);
                    }
                    return;
                }
                let dirty = std::mem::take(&mut st.dirty);
                dirty
                    .into_iter()
                    .map(|rel| Job {
                        old: st.files.get(&rel).cloned().flatten(),
                        rel,
                    })
                    .collect()
            };
            let results = parse_all(inner, entry, &jobs, stop);
            let mut st = entry.state();
            if stop.load(Ordering::Relaxed) || !st.is(&st.worker, stop) {
                return;
            }
            for (rel, outcome) in results {
                if let (Some(slot), Outcome::Set(parsed)) = (st.files.get_mut(&rel), outcome) {
                    *slot = parsed;
                }
            }
        }
    }));
    if worked.is_err() {
        tracing::warn!(root = %entry.root.display(), "symbol index worker panicked");
        let mut st = entry.state();
        if st.is(&st.worker, stop) {
            st.worker = None;
        }
        let build = st.is(&st.build, stop);
        drop(st);
        if build {
            cancel(entry, stop);
        }
    }
}

/// 파일들을 [`Inner::threads`] 스레드로 나눠 파싱한다. `stop` 이 켜지면 남은 것은 건너뛴다.
fn parse_all(
    inner: &Inner,
    entry: &Root,
    jobs: &[Job],
    stop: &AtomicBool,
) -> Vec<(Arc<str>, Outcome)> {
    let threads = inner.threads.min(jobs.len()).max(1);
    let next = AtomicUsize::new(0);
    let work = || {
        let mut extractor = Extractor::default();
        let mut out = Vec::new();
        loop {
            let i = next.fetch_add(1, Ordering::Relaxed);
            if i >= jobs.len() || stop.load(Ordering::Relaxed) {
                break;
            }
            let job = &jobs[i];
            #[cfg(test)]
            tests::before_parse(&entry.root, &job.rel);
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                process(inner, &entry.root, job, &mut extractor, stop)
            }))
            .unwrap_or_else(|_| {
                extractor = Extractor::default();
                Outcome::Set(None)
            });
            out.push((Arc::clone(&job.rel), outcome));
            entry.done.fetch_add(1, Ordering::Relaxed);
        }
        out
    };
    if threads == 1 {
        return work();
    }
    std::thread::scope(|s| {
        let parts: Vec<_> = (0..threads)
            .filter_map(|_| {
                std::thread::Builder::new()
                    .name("pw-index-parse".to_owned())
                    .spawn_scoped(s, work)
                    .ok()
            })
            .collect();
        parts
            .into_iter()
            .flat_map(|p| p.join().unwrap_or_default())
            .collect()
    })
}

/// 파일 하나: 경로로 언어를 고르고, 크기 · 수정 시각이 같으면 그대로, 아니면 읽어 파싱한다.
fn process(
    inner: &Inner,
    root: &Path,
    job: &Job,
    extractor: &mut Extractor,
    stop: &AtomicBool,
) -> Outcome {
    // 경로만으로 정의 쿼리가 있는 언어가 아니면 읽지 않는다.
    if inner.languages.for_path(&job.rel, None).is_none() {
        return Outcome::Set(None);
    }
    let mut path = root.to_path_buf();
    path.extend(job.rel.split('/'));
    let Ok(meta) = std::fs::metadata(&path) else {
        return Outcome::Set(None);
    };
    if !meta.is_file() || meta.len() > MAX_FILE_BYTES {
        return Outcome::Set(None);
    }
    let stamp = Stamp::of(&meta);
    if job.old.as_ref().is_some_and(|p| p.stamp == stamp) {
        return Outcome::Keep;
    }
    #[cfg(test)]
    tests::parsed(root, &job.rel);
    let Some(bytes) = read_limited(&path) else {
        return Outcome::Set(None);
    };
    let text = String::from_utf8_lossy(&bytes);
    let Some(language) = inner.languages.for_path(&job.rel, Some(head(&text))) else {
        return Outcome::Set(None);
    };
    let parsed = extractor.extract(&language, &text, stop).map(|symbols| {
        Arc::new(Parsed {
            stamp,
            language: Arc::clone(&language.id),
            symbols,
        })
    });
    Outcome::Set(parsed)
}

/// [`MAX_FILE_BYTES`] 까지 읽는다. 넘거나 읽지 못하면 `None`.
fn read_limited(path: &Path) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut file = std::fs::File::open(path).ok()?;
    let mut bytes = Vec::new();
    file.by_ref()
        .take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    (bytes.len() as u64 <= MAX_FILE_BYTES).then_some(bytes)
}

/// 언어 판별에 넘기는 앞부분(`pw_lang::HEAD_BYTES` 까지, 글자 경계).
fn head(text: &str) -> &str {
    let mut end = text.len().min(pw_lang::HEAD_BYTES);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// 상대 경로를 `/` 로 이은 문자열.
fn slash(rel: &Path) -> String {
    rel.components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, RpcError> + Send + 'static,
) -> Result<T, RpcError> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| RpcError::internal(e.to_string()))?
}

#[cfg(test)]
mod tests;
