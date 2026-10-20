//! 언어 확장의 문법 WASM · 강조 쿼리를 읽어 tree-sitter 언어로 올린다.
//!
//! 문법마다 wasmtime 저장소(`WasmStore`)와 그것을 가진 파서 하나를 둔다([`Lane`]). 한 문법의 긴 파싱은 그 문법의 문서만 기다리게
//! 하고, 마지막 문서가 닫힌 문법은 저장소째 내려 그 메모리(인스턴스 · 선형 메모리)를 돌려준다(`service.rs` 머리 주석). 엔진은
//! 문법들이 함께 쓰고 처음 문법이 필요할 때 만든다 — 강조할 문서를 열지 않은 앱의 시작 시간 · 유휴 메모리에 들지 않는다(결정
//! M1-D2 의 되돌림 조건). 쿼리 실행은 파서와 저장소 없이 된다(쿼리 · 트리는 호스트 쪽 자료다).
//!
//! 문법 WASM 의 실행 시간에는 상한이 없다: 파싱의 진행 콜백은 WASM 호출(렉서 · 외부 스캐너) 안쪽을 끊지 못하고, wasmtime 의
//! epoch · fuel 은 tree-sitter 가 저장소(`Store`)를 만들자마자 WASM 을 부르면서 시한 · 연료를 받을 길을 주지 않아 걸 수 없다.
//! 그래서 문법은 내장 확장의 것만 받는다(`pw-ext` 레지스트리 — `docs/design/07-extensibility.md` 「문법 WASM 의 신뢰 경계」).

use std::ops::ControlFlow;
use std::sync::Arc;

use pw_ext::{ResolvedFile, ResolvedLanguage};
use tree_sitter::{Language, ParseOptions, ParseState, Parser, Query, Tree, WasmStore, wasmtime};

use crate::token::{CaptureKind, classify};

/// 문법 WASM 하나의 크기 상한. 내장 문법 중 가장 큰 것(Kotlin)이 약 4 MB 다.
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

/// 문법 하나와 그것만 올린 저장소 · 파서. 저장소는 엔진을 복제해 쥐므로 엔진보다 오래 살아도 된다.
pub struct Lane {
    parser: Parser,
    grammar: Arc<Grammar>,
}

impl Lane {
    pub fn load(engine: &wasmtime::Engine, language: &ResolvedLanguage) -> Result<Self, String> {
        let (Some(wasm), Some(queries)) = (&language.grammar, &language.queries) else {
            return Err(format!("language `{}` has no grammar", language.id));
        };
        let name = grammar_name(wasm)?;
        let bytes = read_limited(wasm, MAX_GRAMMAR_BYTES)?;
        let highlights = queries.join(HIGHLIGHTS)?;
        let source = String::from_utf8(read_limited(&highlights, MAX_QUERY_BYTES)?)
            .map_err(|_| format!("{} is not UTF-8", highlights.display()))?;

        let mut store =
            WasmStore::new(engine).map_err(|e| format!("cannot create the wasm store: {e}"))?;
        let language = store
            .load_language(&name, &bytes)
            .map_err(|e| format!("{}: {e}", wasm.display()))?;
        let mut parser = Parser::new();
        parser
            .set_wasm_store(store)
            .map_err(|e| format!("cannot attach the wasm store: {e}"))?;
        parser
            .set_language(&language)
            .map_err(|e| format!("cannot use the grammar: {e}"))?;

        let highlights = Query::new(&language, &source).map_err(|e| {
            format!(
                "{}:{}:{}: {}",
                highlights.display(),
                e.row + 1,
                e.column + 1,
                e.message
            )
        })?;
        let kinds = highlights
            .capture_names()
            .iter()
            .map(|n| classify(n))
            .collect();
        Ok(Self {
            parser,
            grammar: Arc::new(Grammar {
                language,
                highlights,
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
