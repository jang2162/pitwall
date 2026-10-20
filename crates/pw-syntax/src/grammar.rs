//! 언어 확장의 문법 WASM · 강조 쿼리를 읽어 tree-sitter 언어로 올린다.
//!
//! WASM 문법은 wasmtime 엔진 위의 저장소(`WasmStore`)에 올리고, 그 저장소를 가진 파서 하나로 모든 문서를 파싱한다. 엔진 · 파서는
//! 처음 문법이 필요할 때 만든다 — 강조할 문서를 열지 않은 앱의 시작 시간 · 유휴 메모리에 들지 않는다(결정 M1-D2 의 되돌림 조건).
//! 쿼리 실행은 파서와 저장소 없이 된다(쿼리 · 트리는 호스트 쪽 자료다).

use std::collections::HashMap;
use std::sync::Arc;

use pw_ext::{ResolvedFile, ResolvedLanguage};
use tree_sitter::{Language, Parser, Query, Tree, WasmStore, wasmtime};

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

/// 문법이 놓인 자리로 가른다 — 같은 WASM · 쿼리를 쓰는 두 기여(JSON · JSON with Comments)는 한 번만 올린다.
type Key = (String, String);

/// 엔진 · 파서와 올린 문법. 하나의 잠금 아래에 둔다 — 문법을 올리는 일과 파싱이 같은 저장소를 쓴다.
pub struct Runtime {
    /// 저장소를 가진다. `engine` 보다 먼저 버려져야 한다(필드는 적은 순서로 버려진다) — 저장소가 엔진을 가리킨다.
    parser: Parser,
    #[allow(
        dead_code,
        reason = "the wasm store points at it; it must outlive `parser`"
    )]
    engine: wasmtime::Engine,
    loaded: HashMap<Key, Result<Arc<Grammar>, String>>,
}

impl Runtime {
    pub fn new() -> Result<Self, String> {
        let engine = wasmtime::Engine::default();
        let store =
            WasmStore::new(&engine).map_err(|e| format!("cannot create the wasm store: {e}"))?;
        let mut parser = Parser::new();
        parser
            .set_wasm_store(store)
            .map_err(|e| format!("cannot attach the wasm store: {e}"))?;
        Ok(Self {
            parser,
            engine,
            loaded: HashMap::new(),
        })
    }

    /// 언어의 문법. 처음이면 올린다. 실패도 기억한다 — 같은 문법을 매번 다시 읽지 않는다.
    pub fn grammar(&mut self, language: &ResolvedLanguage) -> Result<Arc<Grammar>, String> {
        let (Some(wasm), Some(queries)) = (&language.grammar, &language.queries) else {
            return Err(format!("language `{}` has no grammar", language.id));
        };
        let key = (wasm.display(), queries.display());
        if let Some(result) = self.loaded.get(&key) {
            return result.clone();
        }
        let result = self.load(wasm, queries).map(Arc::new);
        if let Err(e) = &result {
            tracing::error!(target: "pw_syntax", language = %language.id, "cannot load the grammar: {e}");
        }
        self.loaded.insert(key, result.clone());
        result
    }

    fn load(&mut self, wasm: &ResolvedFile, queries: &ResolvedFile) -> Result<Grammar, String> {
        let name = grammar_name(wasm)?;
        let bytes = read_limited(wasm, MAX_GRAMMAR_BYTES)?;
        let highlights = queries.join(HIGHLIGHTS)?;
        let source = String::from_utf8(read_limited(&highlights, MAX_QUERY_BYTES)?)
            .map_err(|_| format!("{} is not UTF-8", highlights.display()))?;

        let mut store = self
            .parser
            .take_wasm_store()
            .ok_or("the parser lost its wasm store")?;
        let language = store.load_language(&name, &bytes);
        self.parser
            .set_wasm_store(store)
            .map_err(|e| format!("cannot attach the wasm store: {e}"))?;
        let language = language.map_err(|e| format!("{}: {e}", wasm.display()))?;

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
        Ok(Grammar {
            language,
            highlights,
            kinds,
        })
    }

    /// `old` 는 이 텍스트에 맞춰 편집(`Tree::edit`)해 둔 앞 트리 — 바뀐 곳만 다시 파싱한다.
    pub fn parse(
        &mut self,
        grammar: &Grammar,
        text: &str,
        old: Option<&Tree>,
    ) -> Result<Tree, String> {
        self.parser
            .set_language(&grammar.language)
            .map_err(|e| format!("cannot use the grammar: {e}"))?;
        let tree = self.parser.parse(text, old);
        if tree.is_none() {
            self.parser.reset();
        }
        tree.ok_or_else(|| "the parser stopped before the end".to_owned())
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
