//! 파일 → 언어 → 문법과 정의 쿼리(`tags.scm`).
//!
//! - 언어는 경로로 판별한다(`pw_lang::detect` 의 Linguist 단계와 확장의 확장자 — 설정의 `[languages.mapping]` · `.gitattributes`
//!   의 `linguist-language` 는 보지 않는다). 먼저 경로만으로 고르고, 정의 쿼리가 있는 언어일 때만 파일을 읽어 앞부분으로 한 번 더
//!   판별한다 — 코드가 아닌 파일은 읽지 않는다. 확장자 없는 스크립트(shebang 만 있는 것)는 그래서 들지 않는다.
//! - 문법은 언어 확장의 `builtin_grammar`(앱에 링크한 문법 — `pw_syntax::native`), 쿼리는 그 확장의 `queries` 폴더의
//!   `tags.scm` 이다(`scripts/grammars.toml` 의 `tags`). 쿼리가 없는 언어는 색인하지 않는다.
//! - 언어마다 처음 쓸 때 한 번 올리고 Host 가 사는 동안 둔다(쿼리 몇 개 — 수백 KB).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use pw_ext::Registry;
use pw_lang::{Attrs, Contributed, Input, Mapping};
use tree_sitter::{Language, Query};

use crate::kind::Capture;

/// 정의 쿼리 파일 이름(쿼리 폴더 바로 아래).
pub const TAGS: &str = "tags.scm";

/// 정의 쿼리가 있는 언어 하나.
pub(crate) struct TagsLanguage {
    /// 언어 확장의 언어 id(`[contributes.languages.<id>]`).
    pub id: Arc<str>,
    pub ts: Language,
    pub query: Query,
    /// `@name` 캡처.
    pub name: u32,
    /// 캡처 색인 → 정의 종류(`@definition.<종류>`). 정의가 아닌 캡처(`@reference.*` · `@doc` 등)는 `None`.
    pub defs: Vec<Option<Capture>>,
}

/// 언어 표. Host 하나에 하나.
pub(crate) struct Languages {
    registry: Arc<Registry>,
    contributed: Contributed,
    /// Linguist 이름 → 올린 언어(쿼리가 없거나 올리지 못했으면 `None`).
    loaded: Mutex<HashMap<String, Option<Arc<TagsLanguage>>>>,
}

impl Languages {
    pub fn new(registry: Arc<Registry>) -> Self {
        Self {
            contributed: Contributed::from_registry(&registry),
            registry,
            loaded: Mutex::default(),
        }
    }

    /// `rel`(루트 기준, `/` 구분)의 Linguist 이름. `head` 는 파일 앞부분.
    pub fn detect(&self, rel: &str, head: Option<&str>) -> Option<String> {
        let input = Input {
            repo_path: rel,
            project_path: rel,
            head,
        };
        pw_lang::detect(
            input,
            &Attrs::default(),
            &Mapping::default(),
            &self.contributed,
        )
        .language
    }

    /// 경로와 (있으면) 앞부분으로 고른, 정의 쿼리가 있는 언어.
    pub fn for_path(&self, rel: &str, head: Option<&str>) -> Option<Arc<TagsLanguage>> {
        self.get(&self.detect(rel, head)?)
    }

    /// Linguist 이름의 언어. 처음이면 올린다.
    pub fn get(&self, linguist: &str) -> Option<Arc<TagsLanguage>> {
        let mut loaded = self.loaded.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(found) = loaded.get(linguist) {
            return found.clone();
        }
        let language = self.load(linguist).map(Arc::new);
        loaded.insert(linguist.to_owned(), language.clone());
        language
    }

    fn load(&self, linguist: &str) -> Option<TagsLanguage> {
        let resolved = self.registry.language_for_linguist(linguist)?;
        let (Some(grammar), Some(queries)) = (&resolved.builtin_grammar, &resolved.queries) else {
            return None;
        };
        let tags = queries.join(TAGS).ok()?;
        // 쿼리가 없는 언어(강조만 있는 것)는 조용히 빠진다.
        if !tags.root.size(&tags.path).is_ok() {
            return None;
        }
        let ts = pw_syntax::native::language(grammar)?;
        let loaded =
            read_query(&ts, &tags).and_then(|query| TagsLanguage::new(&resolved.id, ts, query));
        match loaded {
            Ok(language) => Some(language),
            Err(e) => {
                tracing::warn!(language = linguist, error = %e, "symbol index: tags query not loaded");
                None
            }
        }
    }
}

impl TagsLanguage {
    pub(crate) fn new(id: &str, ts: Language, query: Query) -> Result<Self, String> {
        let name = query
            .capture_index_for_name("name")
            .ok_or("the tags query has no @name capture")?;
        let defs = query
            .capture_names()
            .iter()
            .map(|n| n.strip_prefix("definition.").and_then(Capture::parse))
            .collect();
        Ok(Self {
            id: Arc::from(id),
            ts,
            query,
            name,
            defs,
        })
    }
}

fn read_query(ts: &Language, file: &pw_ext::ResolvedFile) -> Result<Query, String> {
    let size = file
        .root
        .size(&file.path)
        .map_err(|e| format!("{}: {e}", file.display()))?;
    if size > pw_syntax::grammar::MAX_QUERY_BYTES {
        return Err(format!(
            "{} is {size} bytes, over the query size limit",
            file.display()
        ));
    }
    let bytes = file
        .read()
        .map_err(|e| format!("{}: {e}", file.display()))?;
    let source =
        std::str::from_utf8(&bytes).map_err(|_| format!("{} is not UTF-8", file.display()))?;
    Query::new(ts, source).map_err(|e| {
        format!(
            "{}:{}:{}: {}",
            file.display(),
            e.row + 1,
            e.column + 1,
            e.message
        )
    })
}
