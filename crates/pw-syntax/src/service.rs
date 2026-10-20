//! 문서 세션: 열린 문서마다 텍스트 · 버전 · 구문 트리를 두고, 편집은 바로 적용하고 다시 파싱은 강조 요청 때 바뀐 곳만 한다.

use std::collections::{BTreeMap, HashMap};
use std::mem;
use std::ops::Range;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use pw_ext::{Registry, ResolvedLanguage};
use pw_rpc::{RpcError, code};
use tree_sitter::{InputEdit, Tree};

use crate::grammar::{Grammar, Runtime};
use crate::highlight::spans;
use crate::methods::{
    HighlightSpan, SyntaxEditParams, SyntaxHighlightsParams, SyntaxHighlightsResult,
    SyntaxOpenParams, SyntaxOpenResult, TextChange,
};
use crate::text::{LineIndex, apply};

/// 한 Host 가 열어 두는 문서 수의 상한. 넘으면 가장 오래 쓰지 않은 문서를 닫는다 — 웹뷰가 다시 로드되며 `syntax.close` 없이
/// 사라진 문서가 쌓이지 않게. 닫힌 문서에 온 요청은 not found 로 끝나고 웹뷰는 다시 연다.
pub const MAX_DOCUMENTS: usize = 256;

/// 앞당겨 도착해 차례를 기다리는 편집 수의 상한(문서마다). 넘으면 conflict — 빠진 편집이 오지 않는 것으로 본다.
pub const MAX_EARLY_EDITS: usize = 64;

/// 구문 분석 서비스. Host 하나에 하나다.
pub struct Syntax {
    extensions: Arc<Registry>,
    /// 처음 문법이 필요할 때 만든다(`grammar.rs` 머리 주석).
    runtime: Mutex<Option<Runtime>>,
    documents: Mutex<HashMap<String, Arc<Document>>>,
    clock: AtomicU64,
}

/// `syntax.open` 이 넣은 문서. 문법을 올리는 일(`Syntax::prepare`)로 넘긴다.
pub struct Opened {
    doc_id: String,
    document: Arc<Document>,
}

struct Document {
    language: ResolvedLanguage,
    text: Mutex<TextState>,
    /// 마지막으로 파싱한 결과. 강조 요청이 이 잠금 아래에서 다시 파싱한다.
    parsed: Mutex<Option<Parsed>>,
    used: AtomicU64,
}

struct TextState {
    text: Arc<String>,
    index: Arc<LineIndex>,
    version: u32,
    /// `parsed` 의 트리 뒤로 적용한 편집. 다음 파싱 전에 트리에 알린다.
    pending: Vec<InputEdit>,
    /// 앞당겨 도착한 편집(버전 → 바꾸기).
    early: BTreeMap<u32, Vec<TextChange>>,
}

struct Parsed {
    tree: Tree,
    version: u32,
    text: Arc<String>,
    index: Arc<LineIndex>,
}

impl Syntax {
    pub fn new(extensions: Arc<Registry>) -> Self {
        Self {
            extensions,
            runtime: Mutex::new(None),
            documents: Mutex::new(HashMap::new()),
            clock: AtomicU64::new(0),
        }
    }

    /// 문서를 넣는다(같은 id 는 바꾼다). 언어의 문법이 없으면 넣지 않고(같은 id 의 앞 문서도 닫는다) `None`.
    pub fn open(&self, params: SyntaxOpenParams) -> Option<Opened> {
        let language = self
            .extensions
            .language_for_linguist(&params.language)
            .filter(|l| l.grammar.is_some() && l.queries.is_some());
        let mut documents = lock(&self.documents);
        let Some(language) = language else {
            documents.remove(&params.doc_id);
            return None;
        };
        let index = LineIndex::new(&params.text);
        let document = Arc::new(Document {
            language,
            text: Mutex::new(TextState {
                text: Arc::new(params.text),
                index: Arc::new(index),
                version: 0,
                pending: Vec::new(),
                early: BTreeMap::new(),
            }),
            parsed: Mutex::new(None),
            used: AtomicU64::new(self.tick()),
        });
        documents.insert(params.doc_id.clone(), Arc::clone(&document));
        if documents.len() > MAX_DOCUMENTS
            && let Some(oldest) = documents
                .iter()
                .min_by_key(|(_, d)| d.used.load(Ordering::Relaxed))
                .map(|(id, _)| id.clone())
        {
            tracing::info!(target: "pw_syntax", doc = %oldest, "closing the least recently used document");
            documents.remove(&oldest);
        }
        Some(Opened {
            doc_id: params.doc_id,
            document,
        })
    }

    /// 연 문서의 문법을 (처음이면) 올린다. 블로킹이다. 올리지 못하면 그 문서를 닫는다(그사이 다시 열린 것은 두고).
    pub fn prepare(&self, opened: Opened) -> Result<SyntaxOpenResult, RpcError> {
        match self.grammar(&opened.document.language) {
            Ok(_) => Ok(SyntaxOpenResult {
                grammar: Some(opened.document.language.id.clone()),
            }),
            Err(e) => {
                let mut documents = lock(&self.documents);
                if documents
                    .get(&opened.doc_id)
                    .is_some_and(|d| Arc::ptr_eq(d, &opened.document))
                {
                    documents.remove(&opened.doc_id);
                }
                Err(RpcError::internal(e))
            }
        }
    }

    pub fn edit(&self, params: SyntaxEditParams) -> Result<(), RpcError> {
        let document = self.document(&params.doc_id)?;
        let mut state = lock(&document.text);
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
            return Ok(());
        }
        state
            .apply(&params.changes)
            .map_err(RpcError::invalid_params)?;
        // 기다리던 편집 중 차례가 온 것.
        loop {
            let next = state.version + 1;
            let Some(changes) = state.early.remove(&next) else {
                break;
            };
            if let Err(e) = state.apply(&changes) {
                tracing::warn!(target: "pw_syntax", doc = %params.doc_id, version = next, "dropping an early edit: {e}");
                break;
            }
        }
        Ok(())
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
        let grammar = self
            .grammar(&document.language)
            .map_err(RpcError::internal)?;
        let mut parsed = lock(&document.parsed);
        self.reparse(&document, &grammar, &mut parsed)
            .map_err(RpcError::internal)?;
        let Some(parsed) = parsed.as_ref() else {
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
        let spans = spans(
            &grammar.highlights,
            &grammar.kinds,
            &parsed.tree,
            text,
            range,
        )
        .into_iter()
        .map(|(r, token)| HighlightSpan {
            from: utf16_u32(index.utf16_of(text, r.start)),
            to: utf16_u32(index.utf16_of(text, r.end)),
            token,
        })
        .collect();
        Ok(SyntaxHighlightsResult {
            version: parsed.version,
            spans,
        })
    }

    pub fn close(&self, doc_id: &str) {
        lock(&self.documents).remove(doc_id);
    }

    /// 열린 문서 수(시험용).
    pub fn document_count(&self) -> usize {
        lock(&self.documents).len()
    }

    fn document(&self, doc_id: &str) -> Result<Arc<Document>, RpcError> {
        let document = lock(&self.documents).get(doc_id).cloned().ok_or_else(|| {
            RpcError::new(code::NOT_FOUND, format!("document `{doc_id}` is not open"))
        })?;
        document.used.store(self.tick(), Ordering::Relaxed);
        Ok(document)
    }

    fn grammar(&self, language: &ResolvedLanguage) -> Result<Arc<Grammar>, String> {
        let mut runtime = lock(&self.runtime);
        if runtime.is_none() {
            *runtime = Some(Runtime::new()?);
        }
        runtime.as_mut().expect("created above").grammar(language)
    }

    /// `parsed` 를 지금 텍스트에 맞춘다. 앞 트리가 있으면 그 뒤의 편집을 알리고 바뀐 곳만 다시 파싱한다.
    fn reparse(
        &self,
        document: &Document,
        grammar: &Grammar,
        parsed: &mut Option<Parsed>,
    ) -> Result<(), String> {
        let (text, index, version, edits) = {
            let mut state = lock(&document.text);
            if parsed.as_ref().is_some_and(|p| p.version == state.version) {
                return Ok(());
            }
            (
                Arc::clone(&state.text),
                Arc::clone(&state.index),
                state.version,
                mem::take(&mut state.pending),
            )
        };
        let old = parsed.take().map(|mut p| {
            for edit in &edits {
                p.tree.edit(edit);
            }
            p.tree
        });
        let tree = lock(&self.runtime)
            .as_mut()
            .ok_or("the grammar runtime is not ready")?
            .parse(grammar, &text, old.as_ref())?;
        *parsed = Some(Parsed {
            tree,
            version,
            text,
            index,
        });
        Ok(())
    }

    fn tick(&self) -> u64 {
        self.clock.fetch_add(1, Ordering::Relaxed)
    }
}

impl TextState {
    /// 바꾸기들을 차례로 적용하고 버전을 올린다. 하나라도 실패하면 아무것도 바꾸지 않는다.
    fn apply(&mut self, changes: &[TextChange]) -> Result<(), String> {
        let mut text = String::clone(&self.text);
        let mut index = LineIndex::clone(&self.index);
        let mut edits = Vec::with_capacity(changes.len());
        for c in changes {
            edits.push(apply(
                &mut text,
                &mut index,
                c.from as usize,
                c.to as usize,
                &c.insert,
            )?);
        }
        self.text = Arc::new(text);
        self.index = Arc::new(index);
        self.pending.extend(edits);
        self.version += 1;
        Ok(())
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
