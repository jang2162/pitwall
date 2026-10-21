//! 언어 확장의 문법 · 강조 쿼리를 읽어 tree-sitter 언어로 올린다.
//!
//! 내장 언어는 앱에 링크한 네이티브 문법이다(매니페스트의 `builtin_grammar` → [`crate::native`]) — 결정 M1-D2 의 되돌림
//! (`docs/design/12-m1-plan.md` §4: WASM 문법은 5개 언어를 연 유휴 메모리가 +50 MB 를 넘었다). 문법마다 파서 하나를 둔다
//! ([`Lane`]). 한 문법의 긴 파싱은 그 문법의 문서만 기다리게 하고, 마지막 문서가 닫힌 문법은 파서째 내린다(`service.rs` 머리
//! 주석). 쿼리 실행은 파서 없이 된다(쿼리 · 트리는 호스트 쪽 자료다).
//!
//! 문법 WASM(외부 확장의 문법, M8)은 `wasm` 기능에서만 올린다 — 앱은 그 기능을 켜지 않고, 레지스트리도 내장이 아닌 확장의
//! 문법을 받지 않는다. 문법 WASM 의 실행 시간에는 상한이 없다: 파싱의 진행 콜백은 WASM 호출(렉서 · 외부 스캐너) 안쪽을 끊지
//! 못하고, wasmtime 의 epoch · fuel 은 tree-sitter 가 저장소(`Store`)를 만들자마자 WASM 을 부르면서 시한 · 연료를 받을 길을 주지
//! 않아 걸 수 없다(`docs/design/07-extensibility.md` 「문법 WASM 의 신뢰 경계」). 문법마다 wasmtime 저장소(`WasmStore`)를 따로
//! 둔다.

use std::ops::ControlFlow;
use std::sync::Arc;

use pw_ext::{ResolvedFile, ResolvedLanguage};
use tree_sitter::{Language, ParseOptions, ParseState, Parser, Query, Tree};

use crate::token::{CaptureKind, classify};

/// 문법 WASM 하나의 크기 상한. 빌드해 본 내장 문법 중 가장 큰 것(Kotlin)이 약 4 MB 였다.
#[cfg(feature = "wasm")]
pub const MAX_GRAMMAR_BYTES: u64 = 32 * 1024 * 1024;

/// 쿼리 파일의 크기 상한(레지스트리가 통째로 읽는 파일과 같다).
pub const MAX_QUERY_BYTES: u64 = pw_ext::registry::MAX_CONTENT_BYTES;

/// 강조 쿼리 파일 이름. 쿼리 폴더 바로 아래에 있다(`docs/design/07-extensibility.md` §2 의 `queries`).
pub const HIGHLIGHTS: &str = "highlights.scm";

/// 올린 문법.
pub struct Grammar {
    pub language: Language,
    pub highlights: Query,
    /// 캡처 색인 → 하는 일.
    pub kinds: Vec<CaptureKind>,
}

/// 문법 하나와 그것만 쓰는 파서. WASM 문법이면 파서가 그 문법만 올린 저장소를 쥔다(저장소는 엔진을 복제해 쥐므로 엔진보다
/// 오래 살아도 된다).
pub struct Lane {
    parser: Parser,
    grammar: Arc<Grammar>,
}

/// 언어에 문법이 있는가 — 내장 문법 이름이나(`wasm` 기능에서는) 문법 WASM, 그리고 쿼리 폴더.
pub fn has_grammar(language: &ResolvedLanguage) -> bool {
    let wasm = cfg!(feature = "wasm") && language.grammar.is_some();
    (language.builtin_grammar.is_some() || wasm) && language.queries.is_some()
}

impl Lane {
    /// 내장 문법(`builtin_grammar`)을 올린다.
    pub fn native(language: &ResolvedLanguage) -> Result<Self, String> {
        let (Some(name), Some(queries)) = (&language.builtin_grammar, &language.queries) else {
            return Err(format!(
                "language `{}` has no built-in grammar",
                language.id
            ));
        };
        let ts = crate::native::language(name).ok_or_else(|| {
            format!(
                "language `{}`: no built-in grammar named `{name}`",
                language.id
            )
        })?;
        let mut parser = Parser::new();
        parser
            .set_language(&ts)
            .map_err(|e| format!("cannot use the grammar: {e}"))?;
        Self::with_queries(parser, ts, queries)
    }

    /// 문법 WASM(`grammar`)을 `engine` 위의 새 저장소에 올린다.
    #[cfg(feature = "wasm")]
    pub fn wasm(
        engine: &tree_sitter::wasmtime::Engine,
        language: &ResolvedLanguage,
    ) -> Result<Self, String> {
        let (Some(wasm), Some(queries)) = (&language.grammar, &language.queries) else {
            return Err(format!("language `{}` has no grammar", language.id));
        };
        let name = grammar_name(wasm)?;
        let bytes = read_limited(wasm, MAX_GRAMMAR_BYTES)?;
        let mut store = tree_sitter::WasmStore::new(engine)
            .map_err(|e| format!("cannot create the wasm store: {e}"))?;
        let ts = store
            .load_language(&name, &bytes)
            .map_err(|e| format!("{}: {e}", wasm.display()))?;
        let mut parser = Parser::new();
        parser
            .set_wasm_store(store)
            .map_err(|e| format!("cannot attach the wasm store: {e}"))?;
        parser
            .set_language(&ts)
            .map_err(|e| format!("cannot use the grammar: {e}"))?;
        Self::with_queries(parser, ts, queries)
    }

    fn with_queries(
        parser: Parser,
        language: Language,
        queries: &ResolvedFile,
    ) -> Result<Self, String> {
        let highlights = queries.join(HIGHLIGHTS)?;
        let source = String::from_utf8(read_limited(&highlights, MAX_QUERY_BYTES)?)
            .map_err(|_| format!("{} is not UTF-8", highlights.display()))?;
        let query = Query::new(&language, &source).map_err(|e| {
            format!(
                "{}:{}:{}: {}",
                highlights.display(),
                e.row + 1,
                e.column + 1,
                e.message
            )
        })?;
        let kinds = query.capture_names().iter().map(|n| classify(n)).collect();
        Ok(Self {
            parser,
            grammar: Arc::new(Grammar {
                language,
                highlights: query,
                kinds,
            }),
        })
    }

    pub fn grammar(&self) -> &Arc<Grammar> {
        &self.grammar
    }

    /// `old` 는 이 텍스트에 맞춰 편집(`Tree::edit`)해 둔 앞 트리 — 바뀐 곳만 다시 파싱한다. `progress` 는 파싱 중에 자주 불리고,
    /// `Break` 를 돌려주면 파싱을 그만두고 `None` 이다. 끝까지 가지 못한 파싱의 상태는 버린다(다음 파싱은 처음부터).
    pub fn parse(
        &mut self,
        text: &str,
        old: Option<&Tree>,
        progress: &mut dyn FnMut() -> ControlFlow<()>,
    ) -> Option<Tree> {
        let bytes = text.as_bytes();
        let mut callback = |_: &ParseState| progress();
        let tree = self.parser.parse_with_options(
            &mut |offset, _| bytes.get(offset..).unwrap_or_default(),
            old,
            Some(ParseOptions::new().progress_callback(&mut callback)),
        );
        if tree.is_none() {
            self.parser.reset();
        }
        tree
    }
}

/// WASM 파일 이름(확장자 뺀 것)이 tree-sitter 문법 이름이다 — 모듈이 `tree_sitter_<이름>` 을 내보낸다.
#[cfg(feature = "wasm")]
fn grammar_name(wasm: &ResolvedFile) -> Result<String, String> {
    let file = wasm.path.as_str().rsplit('/').next().unwrap_or_default();
    match file.strip_suffix(".wasm") {
        Some(name) if !name.is_empty() => Ok(name.to_owned()),
        _ => Err(format!(
            "{}: a grammar file must be named <grammar name>.wasm",
            wasm.display()
        )),
    }
}

fn read_limited(file: &ResolvedFile, max: u64) -> Result<Vec<u8>, String> {
    let size = file
        .root
        .size(&file.path)
        .map_err(|e| format!("{}: {e}", file.display()))?;
    if size > max {
        return Err(format!(
            "{} is {size} bytes, over the {max}-byte limit",
            file.display()
        ));
    }
    file.read()
        .map(std::borrow::Cow::into_owned)
        .map_err(|e| format!("{}: {e}", file.display()))
}
