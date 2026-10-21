//! 문서 세션: 열린 문서마다 텍스트 · 버전 · 구문 트리를 두고, 편집은 바로 적용하고 다시 파싱은 강조 요청 때 바뀐 곳만 한다.
//!
//! 한 문서가 서비스 전체를 붙잡지 않게 하는 상한들:
//! - **크기:** 결정 M1-D9 의 「편집 + 강조」 범위(5 MiB 이하, 1만 자 넘는 줄 없음 — 상수는 `pw_fs::judge`)를 넘는 문서는 열지
//!   않고, 편집으로 넘어가면 그 문서의 강조를 끈다([`SyntaxOff`]). 클라이언트(M1-17 · M1-23)도 같은 기준으로 끄지만 믿지 않는다 —
//!   10 MB 한 줄 JSON 하나가 파싱에 1.4 GB 를 썼다.
//! - **파싱 시간:** 파싱 한 번(과 강조 쿼리 한 번)이 [`parse_budget`] 을 넘으면 그 문서의 강조를 끈다. 새 편집이 오거나 문서가
//!   닫히면 하던 파싱을 그만둔다(그 요청은 빈 구간으로 끝난다 — [`SyntaxHighlightsResult`] 의 `version`). 쿼리는 진행 확인이
//!   드물 수 있어 제 스레드에서 돌리고 요청은 시한에 돌려준다([`by_deadline`]).
//! - **잠금:** 파싱은 그 문법의 파서 잠금만 쥔다 — 다른 문법의 문서는 기다리지 않는다. 같은 문법의 문서는 그 파싱이 끝나거나 위
//!   상한에 걸릴 때까지 기다린다(WASM 문법은 파서 · 저장소를 문서마다 두면 문법의 선형 메모리를 문서마다 다시 만든다).
//! - **문법 메모리:** 문법은 그 언어의 문서를 처음 열 때 올리고(파서를 만들고 강조 쿼리를 컴파일한다), 그 문법을 쓰는 마지막
//!   문서가 사라진 뒤 [`UNLOAD_AFTER`] 가 지나면 파서 · 쿼리째 내린다. 네이티브 문법의 파싱 표는 바이너리에 있다. WASM 문법
//!   (`wasm` 기능)은 저장소째 내리고, 올린 문법이 하나도 없으면 엔진도 버린다.

use std::collections::{BTreeMap, HashMap};
use std::mem;
use std::ops::{ControlFlow, Range};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::{Duration, Instant};

use pw_ext::{Registry, ResolvedLanguage};
use pw_fs::judge::{EDITABLE_MAX, LONG_LINE_CHARS};
use pw_rpc::{RpcError, code};
use tree_sitter::{InputEdit, Tree};

use crate::grammar::{Grammar, Lane, has_grammar};
use crate::highlight::spans;
use crate::methods::{
    HighlightSpan, SyntaxEditParams, SyntaxEditResult, SyntaxHighlightsParams,
    SyntaxHighlightsResult, SyntaxOff, SyntaxOpenParams, SyntaxOpenResult, TextChange,
};
use crate::text::{Change, LineIndex, apply};

/// 한 Host 가 열어 두는 문서 수의 상한. 넘으면 가장 오래 쓰지 않은 문서를 닫는다 — 웹뷰가 다시 로드되며 `syntax.close` 없이
/// 사라진 문서가 쌓이지 않게. 닫힌 문서에 온 요청은 not found 로 끝나고 웹뷰는 다시 연다.
pub const MAX_DOCUMENTS: usize = 256;

/// 앞당겨 도착해 차례를 기다리는 편집 수의 상한(문서마다). 넘으면 conflict — 빠진 편집이 오지 않는 것으로 본다.
pub const MAX_EARLY_EDITS: usize = 64;

/// 문법을 쓰는 마지막 문서가 사라진 뒤 그 문법을 내리기까지. 탭을 닫았다 곧 다시 여는 흔한 경우에 문법을 다시 올리지(강조 쿼리
/// 컴파일, release 로 0.1~22 ms — `tests/perf.rs` 의 `measure_grammar_loads`) 않게 둔다.
pub const UNLOAD_AFTER: Duration = Duration::from_secs(5 * 60);

/// 파싱(또는 강조 쿼리) 한 번의 시간 상한: 1초 + 텍스트 1 MiB 마다 1초(5 MiB 에서 6초).
///
/// 정상 문서의 첫 파싱은 대부분 1 MiB 당 0.12초 안이고 느린 문법이 Kotlin 0.31초 · Markdown 0.13초다(내장 픽스처를 5 MiB 까지
/// 이어 붙여 release 로 잰 것 — `tests/perf.rs` 의 `measure_5mib_first_parse`). 상한을 넘는 것은 오류 회복이 폭주하는
/// 입력이다 — 닫히지 않은 괄호 20만 줄(400 KB Rust)이 53초, `[` 50만 줄(1 MB JSON)이 120초 넘게 걸렸다(WASM 문법 때 잰 것).
pub fn parse_budget(text_bytes: usize) -> Duration {
    const MIB: f64 = 1024.0 * 1024.0;
    Duration::from_secs(1) + Duration::from_secs_f64(text_bytes as f64 / MIB)
}

/// 구문 분석 서비스. Host 하나에 하나다.
pub struct Syntax {
    extensions: Arc<Registry>,
    grammars: Arc<Grammars>,
    documents: Mutex<HashMap<String, Arc<Document>>>,
    clock: AtomicU64,
    /// 텍스트 바이트 수 → 파싱 · 쿼리의 시간 상한. 보통 [`parse_budget`] 이다.
    budget: fn(usize) -> Duration,
    /// 시험: 강조 쿼리 스레드가 쿼리 전에 이만큼 쉰다(진행 확인 없이 오래 도는 쿼리를 재현한다).
    #[cfg(test)]
    pub(crate) query_delay: Option<Duration>,
}

/// 문법과 쿼리가 놓인 자리로 가른다 — 같은 문법 · 쿼리를 쓰는 두 기여(JSON · JSON with Comments)는 한 번만 올린다.
type Key = (String, String);

/// 올린 문법들과 그것을 쓰는 문서 수.
struct Grammars {
    table: Mutex<Table>,
    /// WASM 문법의 저장소들이 함께 쓰는 엔진. 올린 문법이 없으면 `None` 이다.
    #[cfg(feature = "wasm")]
    engine: Mutex<Option<tree_sitter::wasmtime::Engine>>,
    unload_after: Duration,
}

#[derive(Default)]
struct Table {
    slots: HashMap<Key, Entry>,
    /// 문법을 내리는 스레드가 돌고 있다.
    unloading: bool,
}

struct Entry {
    slot: Arc<Slot>,
    /// 이 문법을 쓰는 문서 수.
    documents: usize,
    /// `documents` 가 0 이 된 때.
    idle_since: Option<Instant>,
}

/// 문법 하나. 잠금은 그 문법을 올리는 일과 파싱이 쥔다.
struct Slot {
    lane: Mutex<LaneState>,
}

enum LaneState {
    Empty,
    Ready(Lane),
    /// 올리지 못했다. 같은 문법을 매번 다시 읽지 않는다(내려질 때 잊는다).
    Failed(String),
}

/// 문서가 문법 하나를 쓰고 있다는 표. 버려지면 그 수를 줄인다.
struct Usage {
    owner: Weak<Grammars>,
    key: Key,
    slot: Arc<Slot>,
}

struct Document {
    usage: Usage,
    language: ResolvedLanguage,
    grammar: Arc<Grammar>,
    text: Mutex<TextState>,
    /// 마지막으로 파싱한 결과. 강조 요청이 이 잠금 아래에서 다시 파싱한다.
    parsed: Mutex<Option<Parsed>>,
    used: AtomicU64,
    /// 텍스트의 지금 버전. 하던 파싱이 낡았는지 진행 콜백이 잠금 없이 본다.
    version: AtomicU32,
    /// 닫혔거나 같은 id 로 다시 열려 버려졌다. 하던 파싱을 그만둔다.
    dropped: AtomicBool,
}

struct TextState {
    text: Arc<String>,
    index: Arc<LineIndex>,
    version: u32,
    /// `parsed` 의 트리 뒤로 적용한 편집. 다음 파싱 전에 트리에 알린다.
    pending: Vec<InputEdit>,
    /// 앞당겨 도착한 편집(버전 → 바꾸기).
    early: BTreeMap<u32, Vec<TextChange>>,
    /// 앞당겨 온 편집을 버렸다 — 웹뷰와 텍스트가 어긋났으니 다음 편집은 conflict 다(웹뷰가 다시 연다).
    diverged: bool,
    /// 강조를 끈 까닭. 끈 뒤에는 텍스트를 버리고 편집은 받기만 한다.
    off: Option<SyntaxOff>,
}

struct Parsed {
    tree: Tree,
    version: u32,
    text: Arc<String>,
    index: Arc<LineIndex>,
}

/// 다시 파싱한 결과.
enum Reparse {
    /// 트리가 지금 텍스트에 맞다. 이 요청에서 파싱했으면 그 파싱을 시작한 때(강조 쿼리가 같은 시한을 나눠 쓴다).
    Ready(Option<Instant>),
    /// 새 편집이 왔거나 문서가 닫혀 그만뒀다.
    Stopped {
        version: u32,
    },
    Off {
        version: u32,
        off: SyntaxOff,
    },
}

impl Syntax {
    pub fn new(extensions: Arc<Registry>) -> Self {
        Self::with_unload_after(extensions, UNLOAD_AFTER)
    }

    /// 문법을 내리기까지의 시간을 정한다(시험 · 측정용).
    pub fn with_unload_after(extensions: Arc<Registry>, unload_after: Duration) -> Self {
        Self {
            extensions,
            grammars: Arc::new(Grammars {
                table: Mutex::default(),
                #[cfg(feature = "wasm")]
                engine: Mutex::new(None),
                unload_after,
            }),
            documents: Mutex::new(HashMap::new()),
            clock: AtomicU64::new(0),
            budget: parse_budget,
            #[cfg(test)]
            query_delay: None,
        }
    }

    /// 파싱 · 쿼리의 시간 상한을 바꾼다(시험용 — 0 이면 첫 진행 확인에서 멈춰, 기계 부하와 상관없이 상한에 걸린 문서를 만든다).
    pub fn with_budget(mut self, budget: fn(usize) -> Duration) -> Self {
        self.budget = budget;
        self
    }

    /// 문서를 연다(같은 id 는 바꾼다). 블로킹이다(문법을 처음 쓰면 올린다). 언어의 문법이 없거나 크기 상한을 넘으면 열지 않는다
    /// (같은 id 의 앞 문서도 닫는다). 문법을 올리지 못하면 internal 오류다.
    pub fn open(&self, params: SyntaxOpenParams) -> Result<SyntaxOpenResult, RpcError> {
        let SyntaxOpenParams {
            doc_id,
            language,
            text,
        } = params;
        let not_opened = |off| {
            self.close(&doc_id);
            Ok(SyntaxOpenResult { grammar: None, off })
        };
        let Some(language) = self
            .extensions
            .language_for_linguist(&language)
            .filter(has_grammar)
        else {
            return not_opened(None);
        };
        if text.len() as u64 > EDITABLE_MAX {
            return not_opened(Some(SyntaxOff::TooLarge));
        }
        let index = LineIndex::new(&text);
        if index.longest_line() > LONG_LINE_CHARS {
            return not_opened(Some(SyntaxOff::LongLine));
        }

        let usage = self.grammars.acquire(key(&language));
        let grammar = match usage
            .slot
            .with_lane(&self.grammars, &language, |lane| Arc::clone(lane.grammar()))
        {
            Ok(grammar) => grammar,
            Err(e) => {
                self.close(&doc_id);
                return Err(RpcError::internal(e));
            }
        };
        let document = Arc::new(Document {
            usage,
            grammar,
            text: Mutex::new(TextState {
                text: Arc::new(text),
                index: Arc::new(index),
                version: 0,
                pending: Vec::new(),
                early: BTreeMap::new(),
                diverged: false,
                off: None,
            }),
            parsed: Mutex::new(None),
            used: AtomicU64::new(self.tick()),
            version: AtomicU32::new(0),
            dropped: AtomicBool::new(false),
            language,
        });
        let grammar = Some(document.language.id.clone());
        let mut documents = lock(&self.documents);
        if let Some(old) = documents.insert(doc_id, document) {
            old.dropped.store(true, Ordering::Relaxed);
        }
        if documents.len() > MAX_DOCUMENTS
            && let Some(oldest) = documents
                .iter()
                .min_by_key(|(_, d)| d.used.load(Ordering::Relaxed))
                .map(|(id, _)| id.clone())
        {
            tracing::info!(target: "pw_syntax", doc = %oldest, "closing the least recently used document");
            if let Some(old) = documents.remove(&oldest) {
                old.dropped.store(true, Ordering::Relaxed);
            }
        }
        Ok(SyntaxOpenResult { grammar, off: None })
    }

    /// 블로킹이다(텍스트를 다시 만든다).
    pub fn edit(&self, params: SyntaxEditParams) -> Result<SyntaxEditResult, RpcError> {
        let document = self.document(&params.doc_id)?;
        let mut state = lock(&document.text);
        if let Some(off) = state.off {
            return Ok(SyntaxEditResult { off: Some(off) });
        }
        if state.diverged {
            return Err(conflict(
                "an earlier edit was dropped; reopen the document".to_owned(),
                state.version,
            ));
        }
        if params.version <= state.version {
            return Err(conflict(
                format!(
                    "edit version {} is not after the document version {}",
                    params.version, state.version
                ),
                state.version,
            ));
        }
        if params.version > state.version + 1 {
            if state.early.len() >= MAX_EARLY_EDITS {
                return Err(conflict(
                    format!(
                        "edit {} arrived but edit {} never did",
                        params.version,
                        state.version + 1
                    ),
                    state.version,
                ));
            }
            state.early.insert(params.version, params.changes);
            return Ok(SyntaxEditResult { off: None });
        }
        state
            .apply(&params.changes)
            .map_err(RpcError::invalid_params)?;
        // 기다리던 편집 중 차례가 온 것.
        while state.off.is_none() {
            let next = state.version + 1;
            let Some(changes) = state.early.remove(&next) else {
                break;
            };
            if let Err(e) = state.apply(&changes) {
                tracing::warn!(target: "pw_syntax", doc = %params.doc_id, version = next, "dropping an early edit: {e}");
                state.early.clear();
                state.diverged = true;
                break;
            }
        }
        document.version.store(state.version, Ordering::Relaxed);
        let off = state.off;
        drop(state);
        if off.is_some()
            && let Ok(mut parsed) = document.parsed.try_lock()
        {
            *parsed = None;
        }
        Ok(SyntaxEditResult { off })
    }

    /// 블로킹이다(필요하면 다시 파싱한다).
    pub fn highlights(
        &self,
        params: &SyntaxHighlightsParams,
    ) -> Result<SyntaxHighlightsResult, RpcError> {
        let (from, to) = (params.range.from as usize, params.range.to as usize);
        if from > to {
            return Err(RpcError::invalid_params(format!(
                "range {from}..{to} is reversed"
            )));
        }
        let document = self.document(&params.doc_id)?;
        let mut slot = lock(&document.parsed);
        let parse_started = match self.reparse(&document, &mut slot)? {
            Reparse::Ready(started) => started,
            Reparse::Stopped { version } => return Ok(empty(version, None)),
            Reparse::Off { version, off } => return Ok(empty(version, Some(off))),
        };
        let Some(parsed) = slot.as_ref() else {
            unreachable!("reparse leaves a tree");
        };
        let text = parsed.text.as_str();
        let index = &parsed.index;
        let end = index.len_utf16();
        let bytes = |utf16: usize| -> Result<usize, RpcError> {
            index.byte_of(text, utf16.min(end)).ok_or_else(|| {
                RpcError::invalid_params(format!("position {utf16} splits a surrogate pair"))
            })
        };
        let range: Range<usize> = bytes(from)?..bytes(to)?;
        // 파싱과 쿼리가 한 시한을 나눠 쓴다 — 상한 바로 아래에서 끝난 파싱 뒤에 쿼리가 상한을 또 쓰지 않게.
        let budget = (self.budget)(text.len());
        let deadline = parse_started.unwrap_or_else(Instant::now) + budget;
        let query = {
            let grammar = Arc::clone(&document.grammar);
            let tree = parsed.tree.clone();
            let text = Arc::clone(&parsed.text);
            #[cfg(test)]
            let delay = self.query_delay;
            move || {
                #[cfg(test)]
                if let Some(delay) = delay {
                    std::thread::sleep(delay);
                }
                spans(
                    &grammar.highlights,
                    &grammar.kinds,
                    &tree,
                    &text,
                    range,
                    &mut || {
                        if Instant::now() > deadline {
                            ControlFlow::Break(())
                        } else {
                            ControlFlow::Continue(())
                        }
                    },
                )
            }
        };
        let Some(spans) = by_deadline(deadline, query)? else {
            let version = parsed.version;
            tracing::warn!(target: "pw_syntax", doc = %params.doc_id, "highlighting took over {budget:?}; turning it off");
            lock(&document.text).turn_off(SyntaxOff::TimedOut);
            // 쿼리 스레드가 아직 트리를 쥐고 있으면 큰 트리는 그쪽이 끝날 때 푼다 — 이 요청이 풀며 늦어지지 않게.
            *slot = None;
            return Ok(empty(version, Some(SyntaxOff::TimedOut)));
        };
        Ok(SyntaxHighlightsResult {
            version: parsed.version,
            spans: spans
                .into_iter()
                .map(|(r, token)| HighlightSpan {
                    from: utf16_u32(index.utf16_of(text, r.start)),
                    to: utf16_u32(index.utf16_of(text, r.end)),
                    token,
                })
                .collect(),
            off: None,
        })
    }

    pub fn close(&self, doc_id: &str) {
        if let Some(document) = lock(&self.documents).remove(doc_id) {
            document.dropped.store(true, Ordering::Relaxed);
        }
    }

    /// 열린 문서 수(시험용).
    pub fn document_count(&self) -> usize {
        lock(&self.documents).len()
    }

    /// 문서의 지금 텍스트(시험용).
    #[cfg(test)]
    pub(crate) fn text_of(&self, doc_id: &str) -> String {
        let document = self.document(doc_id).unwrap();
        let state = lock(&document.text);
        String::clone(&state.text)
    }

    /// 올라가 있는 문법 수(시험 · 측정용).
    pub fn loaded_grammars(&self) -> usize {
        let table = lock(&self.grammars.table);
        table
            .slots
            .values()
            .filter(|e| matches!(*lock(&e.slot.lane), LaneState::Ready(_)))
            .count()
    }

    fn document(&self, doc_id: &str) -> Result<Arc<Document>, RpcError> {
        let document = lock(&self.documents).get(doc_id).cloned().ok_or_else(|| {
            RpcError::new(code::NOT_FOUND, format!("document `{doc_id}` is not open"))
        })?;
        document.used.store(self.tick(), Ordering::Relaxed);
        Ok(document)
    }

    /// `parsed` 를 지금 텍스트에 맞춘다. 앞 트리가 있으면 그 뒤의 편집을 알리고 바뀐 곳만 다시 파싱한다.
    fn reparse(
        &self,
        document: &Document,
        parsed: &mut Option<Parsed>,
    ) -> Result<Reparse, RpcError> {
        let (text, index, version, edits) = {
            let mut state = lock(&document.text);
            if let Some(off) = state.off {
                *parsed = None;
                return Ok(Reparse::Off {
                    version: state.version,
                    off,
                });
            }
            if parsed.as_ref().is_some_and(|p| p.version == state.version) {
                return Ok(Reparse::Ready(None));
            }
            (
                Arc::clone(&state.text),
                Arc::clone(&state.index),
                state.version,
                mem::take(&mut state.pending),
            )
        };
        // 앞 트리는 그대로 두고 복제본을 편집한다 — 파싱을 그만두면 앞 트리와 편집을 되돌려 놓는다.
        let old = parsed.as_ref().map(|p| {
            let mut tree = p.tree.clone();
            for edit in &edits {
                tree.edit(edit);
            }
            tree
        });
        let mut stop = None;
        let mut started = Instant::now();
        let tree = document
            .usage
            .slot
            .with_lane(&self.grammars, &document.language, |lane| {
                // 문법의 잠금을 기다린 시간은 넣지 않는다 — 다른 문서의 파싱 때문에 이 문서를 끄지 않게.
                started = Instant::now();
                let budget = (self.budget)(text.len());
                lane.parse(&text, old.as_ref(), &mut || {
                    if document.dropped.load(Ordering::Relaxed)
                        || document.version.load(Ordering::Relaxed) != version
                    {
                        stop = Some(Stop::Superseded);
                    } else if started.elapsed() > budget {
                        stop = Some(Stop::TimedOut(budget));
                    } else {
                        return ControlFlow::Continue(());
                    }
                    ControlFlow::Break(())
                })
            })
            .map_err(RpcError::internal)?;
        if let Some(tree) = tree {
            *parsed = Some(Parsed {
                tree,
                version,
                text,
                index,
            });
            return Ok(Reparse::Ready(Some(started)));
        }
        let mut state = lock(&document.text);
        match stop {
            Some(Stop::Superseded) => {
                let mut edits = edits;
                edits.append(&mut state.pending);
                state.pending = edits;
                Ok(Reparse::Stopped { version })
            }
            Some(Stop::TimedOut(budget)) => {
                tracing::warn!(target: "pw_syntax", language = %document.language.id, bytes = text.len(), "parsing took over {budget:?}; turning highlighting off");
                state.turn_off(SyntaxOff::TimedOut);
                *parsed = None;
                Ok(Reparse::Off {
                    version: state.version,
                    off: SyntaxOff::TimedOut,
                })
            }
            None => {
                tracing::error!(target: "pw_syntax", language = %document.language.id, "the parser stopped before the end");
                state.turn_off(SyntaxOff::Failed);
                *parsed = None;
                Ok(Reparse::Off {
                    version: state.version,
                    off: SyntaxOff::Failed,
                })
            }
        }
    }

    fn tick(&self) -> u64 {
        self.clock.fetch_add(1, Ordering::Relaxed)
    }
}

enum Stop {
    Superseded,
    TimedOut(Duration),
}

impl Grammars {
    fn acquire(self: &Arc<Self>, key: Key) -> Usage {
        let mut table = lock(&self.table);
        let entry = table.slots.entry(key.clone()).or_insert_with(|| Entry {
            slot: Arc::new(Slot {
                lane: Mutex::new(LaneState::Empty),
            }),
            documents: 0,
            idle_since: None,
        });
        entry.documents += 1;
        entry.idle_since = None;
        Usage {
            owner: Arc::downgrade(self),
            key,
            slot: Arc::clone(&entry.slot),
        }
    }

    fn release(self: &Arc<Self>, key: &Key) {
        let mut table = lock(&self.table);
        let Some(entry) = table.slots.get_mut(key) else {
            return;
        };
        entry.documents -= 1;
        if entry.documents > 0 {
            return;
        }
        entry.idle_since = Some(Instant::now());
        if table.unloading {
            return;
        }
        let owner = Arc::downgrade(self);
        match std::thread::Builder::new()
            .name("pw-syntax-unload".to_owned())
            .spawn(move || unload_idle(&owner))
        {
            Ok(_) => table.unloading = true,
            Err(e) => {
                tracing::warn!(target: "pw_syntax", "cannot start the grammar unloading thread: {e}");
            }
        }
    }

    /// 언어의 문법을 올린다: 내장 문법이 있으면 그것을, 없으면(`wasm` 기능) 문법 WASM 을.
    fn load(&self, language: &ResolvedLanguage) -> Result<Lane, String> {
        #[cfg(feature = "wasm")]
        if language.builtin_grammar.is_none() {
            let engine = lock(&self.engine)
                .get_or_insert_with(tree_sitter::wasmtime::Engine::default)
                .clone();
            return Lane::wasm(&engine, language);
        }
        Lane::native(language)
    }
}

/// 문서가 없는 채로 `unload_after` 가 지난 문법을 내린다. 기다리는 문법이 없으면 끝난다.
fn unload_idle(owner: &Weak<Grammars>) {
    loop {
        let Some(grammars) = owner.upgrade() else {
            return;
        };
        let now = Instant::now();
        let mut unloaded = Vec::new();
        let mut next: Option<Duration> = None;
        {
            let mut table = lock(&grammars.table);
            table.slots.retain(|_, e| {
                let Some(since) = e.idle_since.filter(|_| e.documents == 0) else {
                    return true;
                };
                let idle = now.saturating_duration_since(since);
                if idle >= grammars.unload_after {
                    unloaded.push(Arc::clone(&e.slot));
                    return false;
                }
                let left = grammars.unload_after - idle;
                next = Some(next.map_or(left, |n| n.min(left)));
                true
            });
            #[cfg(feature = "wasm")]
            if table.slots.is_empty() {
                *lock(&grammars.engine) = None;
            }
            if next.is_none() {
                table.unloading = false;
            }
        }
        // 파서 · 문법은 잠금 밖에서 버린다.
        if !unloaded.is_empty() {
            tracing::info!(target: "pw_syntax", count = unloaded.len(), "unloading idle grammars");
        }
        drop(unloaded);
        drop(grammars);
        match next {
            Some(wait) => std::thread::sleep(wait),
            None => return,
        }
    }
}

impl Slot {
    /// 문법의 파서로 `f` 를 한다. 처음이면 문법을 올린다(그동안 이 문법의 다른 문서는 기다린다).
    fn with_lane<R>(
        &self,
        grammars: &Grammars,
        language: &ResolvedLanguage,
        f: impl FnOnce(&mut Lane) -> R,
    ) -> Result<R, String> {
        let mut state = lock(&self.lane);
        if matches!(*state, LaneState::Empty) {
            *state = match grammars.load(language) {
                Ok(lane) => LaneState::Ready(lane),
                Err(e) => {
                    tracing::error!(target: "pw_syntax", language = %language.id, "cannot load the grammar: {e}");
                    LaneState::Failed(e)
                }
            };
        }
        match &mut *state {
            LaneState::Ready(lane) => Ok(f(lane)),
            LaneState::Failed(e) => Err(e.clone()),
            LaneState::Empty => unreachable!("loaded above"),
        }
    }
}

impl Drop for Usage {
    fn drop(&mut self) {
        if let Some(grammars) = self.owner.upgrade() {
            grammars.release(&self.key);
        }
    }
}

impl TextState {
    /// 바꾸기들을 차례로 적용하고 버전을 올린다. 범위가 틀리면 아무것도 바꾸지 않는다. 결과가 크기 상한을 넘으면 강조를 끈다.
    fn apply(&mut self, changes: &[TextChange]) -> Result<(), String> {
        let changes: Vec<Change<'_>> = changes
            .iter()
            .map(|c| Change {
                from: c.from as usize,
                to: c.to as usize,
                insert: &c.insert,
            })
            .collect();
        if let Some(applied) = apply(&self.text, &self.index, &changes)? {
            if applied.text.len() as u64 > EDITABLE_MAX {
                self.turn_off(SyntaxOff::TooLarge);
            } else if applied.index.longest_line() > LONG_LINE_CHARS {
                self.turn_off(SyntaxOff::LongLine);
            } else {
                self.text = Arc::new(applied.text);
                self.index = Arc::new(applied.index);
                self.pending.extend(applied.edits);
            }
        }
        self.version += 1;
        Ok(())
    }

    fn turn_off(&mut self, off: SyntaxOff) {
        self.off = Some(off);
        self.text = Arc::default();
        self.index = Arc::new(LineIndex::new(""));
        self.pending = Vec::new();
        self.early = BTreeMap::new();
    }
}

fn key(language: &ResolvedLanguage) -> Key {
    let display =
        |f: &Option<pw_ext::ResolvedFile>| f.as_ref().map(|f| f.display()).unwrap_or_default();
    let grammar = match &language.builtin_grammar {
        Some(name) => format!("builtin:{name}"),
        None => display(&language.grammar),
    };
    (grammar, display(&language.queries))
}

/// `work` 를 제 스레드에서 돌리고 `deadline` 까지만 기다린다. 시한까지 끝나지 않았거나 `work` 가 그만뒀으면(`None`) `None` 이다.
/// `work` 는 시한을 스스로 보고 그만둬야 한다 — 기다림만 끊고 스레드는 그대로 둔다.
///
/// 강조 쿼리가 이렇게 돈다. tree-sitter 는 쿼리의 진행 콜백을 노드 1000개쯤마다 부르는데, 노드마다 뒤 형제를 끝까지 훑는 트리
/// (오류 회복이 만든, 익명 토큰 수십만 개를 자식으로 둔 ERROR)에서는 콜백 사이가 몇 초다 — 닫히지 않은 `[` 50만 줄(1 MB JSON)에서
/// release 로 0.9초, debug 로 2.7초를 쟀다. 그동안 요청이 시한을 넘겨 붙잡히지 않게 한다. 남은 쿼리는 다음 콜백에서 그만둔다.
pub(crate) fn by_deadline<T: Send + 'static>(
    deadline: Instant,
    work: impl FnOnce() -> Option<T> + Send + 'static,
) -> Result<Option<T>, RpcError> {
    let (done, result) = mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("pw-syntax-query".to_owned())
        .spawn(move || {
            let _ = done.send(work());
        })
        .map_err(|e| RpcError::internal(format!("cannot start the highlight query: {e}")))?;
    match result.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
        Ok(out) => Ok(out),
        Err(RecvTimeoutError::Timeout) => Ok(None),
        Err(RecvTimeoutError::Disconnected) => {
            Err(RpcError::internal("the highlight query panicked"))
        }
    }
}

fn empty(version: u32, off: Option<SyntaxOff>) -> SyntaxHighlightsResult {
    SyntaxHighlightsResult {
        version,
        spans: Vec::new(),
        off,
    }
}

fn utf16_u32(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

fn conflict(message: String, version: u32) -> RpcError {
    RpcError::new(code::CONFLICT, message).with_data(serde_json::json!({ "version": version }))
}

/// 잠금을 쥔 스레드가 패닉했어도 자료는 쓸 수 있다(바꾸기는 다 만든 뒤에 한 번에 넣는다).
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}
