//! 문서 심볼(15-m4-plan.md M4-10): `lsp.symbols.document` — 구조 툴 윈도우 · 파일 구조 팝업(`⌘F12`)의 아웃라인.
//!
//! 서버의 `textDocument/documentSymbol` 답은 두 꼴이다 — 계층(`DocumentSymbol[]` — 자식이 안에 든다)과 평면
//! (`SymbolInformation[]` — 위치만 있다). 둘 다 같은 평면 목록 + 부모 색인(`parent` — 앞쪽 색인, 부모가 자식보다 앞)으로 바꾼다.
//! 심볼 인덱스의 `index.file.symbols`(M4-15)와 같은 꼴이라 웹뷰가 두 출처를 같은 트리로 만든다. 평면 답은 범위가 담는 관계로
//! 계층을 세운다(앞에서 시작해 뒤에서 끝나는 심볼이 부모 — 같은 범위면 먼저 온 것이 부모).
//!
//! 위치는 웹뷰의 UTF-16 오프셋이다(요청을 보낼 때의 문서 기준 — [`DocView::offset`]). 서버가 없거나 준비 전 · 미신뢰면 이
//! 메서드는 「맡을 서버 없음」(`5001`)이다 — 웹뷰가 `index.file.symbols` 로 같은 꼴을 만든다(대체는 웹뷰가 한다 — pw-lsp 가
//! pw-index 에 의존하지 않는다, 메인 판정 2026-11-02).
//!
//! **상한**(망가지거나 악의 있는 서버): 심볼 [`MAX_SYMBOLS`] 개(앞에서부터 — 넘으면 `truncated`), 깊이 [`MAX_DEPTH`](더 깊은
//! 자식은 버리고 `truncated`), 이름 · 세부 [`MAX_TEXT`] 바이트(글자 경계에서 자른다). 다른 문서의 위치(평면 답의 `location.uri`)는
//! 버린다. 읽을 수 없는 항목(이름 · 범위가 없음)은 그 항목(과 자식)만 버린다.

use lsp_types::Position;
use pw_ext::LspFeature;
use pw_rpc::{Ctx, Router, RpcError, rpc_method};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::manager::{DocView, FeatureRequest, LspManager};

/// 문서 하나에서 돌려주는 심볼 수의 상한. 심볼 인덱스의 파일당 상한(M4-15 ⓕ)과 같다.
pub const MAX_SYMBOLS: usize = 20_000;
/// 계층의 깊이 상한(맨 위가 0). 실제 코드의 중첩은 이보다 훨씬 얕다.
pub const MAX_DEPTH: usize = 32;
/// 이름 · 세부의 길이 상한(UTF-8 바이트). 심볼 인덱스의 이름 상한(M4-15 ⓕ)과 같다.
pub const MAX_TEXT: usize = 256;

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct LspSymbolsDocumentParams {
    pub doc_id: String,
    /// 웹뷰가 요청을 만든 문서 버전. 요청을 보낼 때의 문서와 다르면 답에 `staleVersion`.
    #[serde(default)]
    pub version: Option<u32>,
}

/// 아웃라인의 심볼 하나.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct LspOutlineSymbol {
    /// 서버가 준 이름(글로만 그린다 — HTML 로 해석하지 않는다). [`MAX_TEXT`] 바이트까지.
    pub name: String,
    /// 서버가 준 세부(시그니처 · 타입 등). 없으면 `None`.
    pub detail: Option<String>,
    /// LSP `SymbolKind`(1 `File` ~ 26 `TypeParameter`). 범위 밖이면 0.
    pub kind: u32,
    /// 서버가 낡았다고 알렸다(`deprecated` · 태그 1).
    pub deprecated: bool,
    /// 부모의 색인(이 목록 안 — 늘 자기보다 앞). 맨 위면 `None`.
    pub parent: Option<u32>,
    /// 심볼 전체 범위(UTF-16 오프셋, 끝 제외).
    pub from: u32,
    pub to: u32,
    /// 이름 자리(고르면 커서가 가는 곳). 평면 답은 범위의 처음이다.
    pub selection_from: u32,
    pub selection_to: u32,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct LspDocumentSymbols {
    /// 앞에서부터(부모가 자식보다 앞) — 같은 부모의 자식은 서버가 준 차례.
    pub symbols: Vec<LspOutlineSymbol>,
    /// 답한 서버 id.
    pub server: String,
    /// 요청 사이에 문서가 바뀌었다 — 오프셋이 지금 문서와 어긋날 수 있다(웹뷰는 버린다).
    pub stale_version: bool,
    /// 상한(수 · 깊이)에 걸려 일부를 버렸다.
    pub truncated: bool,
}

rpc_method! {
    /// 문서의 심볼(구조 툴 윈도우 · 파일 구조 팝업 — M4-10). 서버의 계층 · 평면 답을 평면 목록 + 부모 색인으로 바꾼다(위치는
    /// UTF-16 오프셋). 맡을 서버가 없으면(서버 없음 · 준비 전 · 미신뢰) `5001` — 웹뷰는 `index.file.symbols` 로 대신한다. 없는
    /// 문서 · 다른 연결의 문서는 not found. 서버를 띄우지 않는다.
    pub LspSymbolsDocument: "lsp.symbols.document" {
        params: LspSymbolsDocumentParams,
        result: LspDocumentSymbols,
        tier: Read,
        roles: [Owner],
    }
}

/// 문서의 심볼을 묻는다.
pub async fn document_symbols(
    manager: &LspManager,
    ctx: &Ctx,
    params: LspSymbolsDocumentParams,
) -> Result<LspDocumentSymbols, RpcError> {
    let answer = manager
        .request(
            ctx,
            FeatureRequest {
                doc_id: &params.doc_id,
                feature: LspFeature::DocumentSymbol,
                method: "textDocument/documentSymbol",
                expected_version: params.version,
                timeout: None,
            },
            |view| Ok(json!({ "textDocument": view.text_document() })),
        )
        .await?;
    let (symbols, truncated) = outline(&answer.result, &answer.doc);
    Ok(LspDocumentSymbols {
        symbols,
        server: answer.server,
        stale_version: answer.stale_version,
        truncated,
    })
}

/// 핸들러를 붙인다(`crate::register` 가 부른다).
pub fn register(router: &mut Router, manager: &LspManager) {
    let m = manager.clone();
    router.unary(LspSymbolsDocument, move |ctx, params| {
        let m = m.clone();
        async move { document_symbols(&m, &ctx, params).await }
    });
}

/// 서버의 답 → 평면 목록 + 부모 색인. 두 번째 값은 상한에 걸렸는가.
pub fn outline(result: &Value, doc: &DocView) -> (Vec<LspOutlineSymbol>, bool) {
    convert(result, &doc.uri, &|p| doc.offset(p))
}

/// [`outline`] 의 몸통 — 위치 변환을 받아 시험이 문서 없이 부른다.
fn convert(
    result: &Value,
    uri: &str,
    offset: &dyn Fn(Position) -> u32,
) -> (Vec<LspOutlineSymbol>, bool) {
    let Some(items) = result.as_array() else {
        return (Vec::new(), false);
    };
    // 계층 꼴은 `selectionRange` 가, 평면 꼴은 `location` 이 있다. 섞여 오면 앞 항목의 꼴로 읽고 다른 꼴은 버린다.
    let hierarchical = items
        .iter()
        .find_map(|i| {
            if i.get("selectionRange").is_some() {
                Some(true)
            } else if i.get("location").is_some() {
                Some(false)
            } else {
                None
            }
        })
        .unwrap_or(true);
    let mut out = Outline::default();
    if hierarchical {
        for item in items {
            if out.full() {
                out.truncated = true;
                break;
            }
            out.walk(item, None, 0, offset);
        }
    } else {
        out.flat(items, uri, offset);
    }
    (out.symbols, out.truncated)
}

#[derive(Default)]
struct Outline {
    symbols: Vec<LspOutlineSymbol>,
    truncated: bool,
}

impl Outline {
    fn full(&self) -> bool {
        self.symbols.len() >= MAX_SYMBOLS
    }

    /// 계층 꼴의 항목 하나와 그 자식(앞에서부터). 상한에 걸리면 멈추고 `truncated`.
    fn walk(
        &mut self,
        item: &Value,
        parent: Option<u32>,
        depth: usize,
        offset: &dyn Fn(Position) -> u32,
    ) {
        let (Some(name), Some((from, to))) = (
            item.get("name").and_then(Value::as_str),
            item.get("range").and_then(|r| range(r, offset)),
        ) else {
            return;
        };
        let (selection_from, selection_to) = item
            .get("selectionRange")
            .and_then(|r| range(r, offset))
            .filter(|&(s, e)| from <= s && e <= to)
            .unwrap_or((from, from));
        let index = u32::try_from(self.symbols.len()).unwrap_or(u32::MAX);
        self.symbols.push(LspOutlineSymbol {
            name: clip(name),
            detail: item
                .get("detail")
                .and_then(Value::as_str)
                .filter(|d| !d.is_empty())
                .map(clip),
            kind: kind(item),
            deprecated: deprecated(item),
            parent,
            from,
            to,
            selection_from,
            selection_to,
        });
        let Some(children) = item.get("children").and_then(Value::as_array) else {
            return;
        };
        if children.is_empty() {
            return;
        }
        if depth + 1 >= MAX_DEPTH {
            self.truncated = true;
            return;
        }
        for child in children {
            if self.full() {
                self.truncated = true;
                return;
            }
            self.walk(child, Some(index), depth + 1, offset);
        }
    }

    /// 평면 꼴: 이 문서의 위치만 모아 범위가 담는 관계로 부모를 세운다.
    fn flat(&mut self, items: &[Value], uri: &str, offset: &dyn Fn(Position) -> u32) {
        struct Flat<'a> {
            item: &'a Value,
            name: &'a str,
            from: u32,
            to: u32,
            order: usize,
        }
        let mut found: Vec<Flat<'_>> = items
            .iter()
            .enumerate()
            .filter_map(|(order, item)| {
                let location = item.get("location")?;
                if location.get("uri").and_then(Value::as_str) != Some(uri) {
                    return None;
                }
                let (from, to) = range(location.get("range")?, offset)?;
                Some(Flat {
                    item,
                    name: item.get("name")?.as_str()?,
                    from,
                    to,
                    order,
                })
            })
            .collect();
        if found.len() > MAX_SYMBOLS {
            found.truncate(MAX_SYMBOLS);
            self.truncated = true;
        }
        // 앞에서 시작하는 것, 같으면 더 넓은 것, 같으면 먼저 온 것이 앞 — 그러면 부모가 늘 자식보다 앞이다.
        found.sort_by_key(|f| (f.from, std::cmp::Reverse(f.to), f.order));
        // 지금 열린 조상들(색인, 끝, 깊이).
        let mut open: Vec<(u32, u32, usize)> = Vec::new();
        for f in found {
            // 정렬해서 처음은 늘 조상의 처음 이후다 — 끝이 조상 안이면 담긴다.
            while open.last().is_some_and(|&(_, end, _)| f.to > end) {
                open.pop();
            }
            let depth = open.last().map_or(0, |&(_, _, d)| d + 1);
            if depth >= MAX_DEPTH {
                self.truncated = true;
                continue;
            }
            let index = u32::try_from(self.symbols.len()).unwrap_or(u32::MAX);
            self.symbols.push(LspOutlineSymbol {
                name: clip(f.name),
                // 평면 답의 `containerName` 은 계층으로 이미 보인다 — 세부로 겹쳐 적지 않는다.
                detail: None,
                kind: kind(f.item),
                deprecated: deprecated(f.item),
                parent: open.last().map(|&(i, _, _)| i),
                from: f.from,
                to: f.to,
                selection_from: f.from,
                selection_to: f.from,
            });
            open.push((index, f.to, depth));
        }
    }
}

/// LSP `Range` → (처음, 끝) 오프셋. 읽을 수 없거나 끝이 처음보다 앞이면 `None`.
fn range(value: &Value, offset: &dyn Fn(Position) -> u32) -> Option<(u32, u32)> {
    let position = |key: &str| {
        let p = value.get(key)?;
        Some(Position::new(
            u32::try_from(p.get("line")?.as_u64()?).ok()?,
            u32::try_from(p.get("character")?.as_u64()?).ok()?,
        ))
    };
    let (from, to) = (offset(position("start")?), offset(position("end")?));
    (from <= to).then_some((from, to))
}

fn kind(item: &Value) -> u32 {
    item.get("kind")
        .and_then(Value::as_u64)
        .and_then(|k| u32::try_from(k).ok())
        .filter(|k| (1..=26).contains(k))
        .unwrap_or(0)
}

fn deprecated(item: &Value) -> bool {
    item.get("deprecated").and_then(Value::as_bool) == Some(true)
        || item
            .get("tags")
            .and_then(Value::as_array)
            .is_some_and(|tags| tags.iter().any(|t| t.as_u64() == Some(1)))
}

/// [`MAX_TEXT`] 바이트까지(글자 경계에서). 넘으면 끝에 `…`.
fn clip(text: &str) -> String {
    if text.len() <= MAX_TEXT {
        return text.to_owned();
    }
    let mut end = MAX_TEXT;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 줄마다 100 글자인 문서처럼 센다(줄 * 100 + 글자).
    fn at(p: Position) -> u32 {
        p.line * 100 + p.character
    }

    fn r(sl: u32, sc: u32, el: u32, ec: u32) -> Value {
        json!({ "start": { "line": sl, "character": sc }, "end": { "line": el, "character": ec } })
    }

    fn names(symbols: &[LspOutlineSymbol]) -> Vec<(String, Option<u32>)> {
        symbols.iter().map(|s| (s.name.clone(), s.parent)).collect()
    }

    #[test]
    fn a_hierarchical_answer_becomes_a_parent_indexed_list() {
        let answer = json!([
            { "name": "Point", "detail": "struct", "kind": 23, "range": r(0, 0, 3, 1), "selectionRange": r(0, 7, 0, 12),
              "children": [
                { "name": "x", "kind": 8, "range": r(1, 4, 1, 10), "selectionRange": r(1, 4, 1, 5) },
                { "name": "y", "kind": 8, "range": r(2, 4, 2, 10), "selectionRange": r(2, 4, 2, 5), "tags": [1] }
              ] },
            { "name": "main", "kind": 12, "range": r(5, 0, 7, 1), "selectionRange": r(5, 3, 5, 7) }
        ]);
        let (symbols, truncated) = convert(&answer, "file:///a.rs", &at);
        assert!(!truncated);
        assert_eq!(
            names(&symbols),
            [
                ("Point".into(), None),
                ("x".into(), Some(0)),
                ("y".into(), Some(0)),
                ("main".into(), None)
            ]
        );
        assert_eq!(symbols[0].detail.as_deref(), Some("struct"));
        assert_eq!((symbols[0].from, symbols[0].to), (0, 301));
        assert_eq!(
            (symbols[0].selection_from, symbols[0].selection_to),
            (7, 12)
        );
        assert_eq!(symbols[0].kind, 23);
        assert!(symbols[2].deprecated && !symbols[1].deprecated);
    }

    #[test]
    fn a_flat_answer_is_nested_by_range_and_other_files_are_dropped() {
        let answer = json!([
            { "name": "main", "kind": 12, "location": { "uri": "file:///a.ts", "range": r(5, 0, 7, 1) } },
            { "name": "Point", "kind": 5, "location": { "uri": "file:///a.ts", "range": r(0, 0, 3, 1) } },
            { "name": "x", "kind": 8, "containerName": "Point", "location": { "uri": "file:///a.ts", "range": r(1, 4, 1, 10) } },
            { "name": "elsewhere", "kind": 12, "location": { "uri": "file:///b.ts", "range": r(1, 0, 1, 4) } },
            { "name": "y", "kind": 8, "location": { "uri": "file:///a.ts", "range": r(2, 4, 2, 10) } }
        ]);
        let (symbols, truncated) = convert(&answer, "file:///a.ts", &at);
        assert!(!truncated);
        assert_eq!(
            names(&symbols),
            [
                ("Point".into(), None),
                ("x".into(), Some(0)),
                ("y".into(), Some(0)),
                ("main".into(), None)
            ]
        );
        assert_eq!(
            (symbols[3].selection_from, symbols[3].selection_to),
            (500, 500)
        );
    }

    #[test]
    fn nesting_deeper_than_the_limit_is_cut() {
        let mut deep = json!({ "name": "leaf", "kind": 12, "range": r(0, 0, 0, 1), "selectionRange": r(0, 0, 0, 1) });
        for i in 0..MAX_DEPTH + 10 {
            deep = json!({ "name": format!("n{i}"), "kind": 2, "range": r(0, 0, 0, 1),
                           "selectionRange": r(0, 0, 0, 1), "children": [deep] });
        }
        let (symbols, truncated) = convert(&json!([deep]), "file:///a.rs", &at);
        assert!(truncated);
        assert_eq!(symbols.len(), MAX_DEPTH);
        // 깊이 = 조상 수.
        let depth = |mut i: usize| {
            let mut d = 0;
            while let Some(p) = symbols[i].parent {
                i = p as usize;
                d += 1;
            }
            d
        };
        assert_eq!(depth(symbols.len() - 1), MAX_DEPTH - 1);
    }

    #[test]
    fn more_symbols_than_the_limit_are_cut_from_the_end() {
        let one = |i: u32| json!({ "name": format!("f{i}"), "kind": 12, "range": r(i, 0, i, 1), "selectionRange": r(i, 0, i, 1) });
        let many: Vec<Value> = (0..u32::try_from(MAX_SYMBOLS).unwrap() + 5)
            .map(one)
            .collect();
        let (symbols, truncated) = convert(&Value::Array(many), "file:///a.rs", &at);
        assert!(truncated);
        assert_eq!(symbols.len(), MAX_SYMBOLS);
        assert_eq!(
            symbols.last().unwrap().name,
            format!("f{}", MAX_SYMBOLS - 1)
        );
        // 자식 쪽에서 넘어도 같다.
        let parent = json!([{ "name": "m", "kind": 2, "range": r(0, 0, 0, 1), "selectionRange": r(0, 0, 0, 1),
            "children": (0..u32::try_from(MAX_SYMBOLS).unwrap()).map(one).collect::<Vec<_>>() }]);
        let (symbols, truncated) = convert(&parent, "file:///a.rs", &at);
        assert!(truncated);
        assert_eq!(symbols.len(), MAX_SYMBOLS);
    }

    #[test]
    fn long_text_is_clipped_and_bad_items_are_skipped() {
        let answer = json!([
            { "name": "가".repeat(200), "detail": "d".repeat(1000), "kind": 99, "range": r(0, 0, 0, 1), "selectionRange": r(0, 0, 0, 1) },
            { "kind": 12, "range": r(1, 0, 1, 1), "selectionRange": r(1, 0, 1, 1) },
            { "name": "backwards", "kind": 12, "range": r(2, 5, 2, 1), "selectionRange": r(2, 5, 2, 5) },
            { "name": "<b>html</b>", "kind": 12, "range": r(3, 0, 3, 9), "selectionRange": r(9, 0, 9, 1) }
        ]);
        let (symbols, _) = convert(&answer, "file:///a.rs", &at);
        assert_eq!(symbols.len(), 2);
        assert!(symbols[0].name.len() <= MAX_TEXT + '…'.len_utf8());
        assert!(symbols[0].name.ends_with('…'));
        assert!(symbols[0].detail.as_ref().unwrap().len() <= MAX_TEXT + '…'.len_utf8());
        assert_eq!(symbols[0].kind, 0);
        // 이름은 그대로 — 그리는 쪽이 글로만 그린다. 범위 밖의 이름 자리는 범위의 처음으로.
        assert_eq!(symbols[1].name, "<b>html</b>");
        assert_eq!(
            (symbols[1].selection_from, symbols[1].selection_to),
            (300, 300)
        );
    }

    #[test]
    fn null_and_empty_answers_have_no_symbols() {
        assert_eq!(
            convert(&Value::Null, "file:///a.rs", &at),
            (Vec::new(), false)
        );
        assert_eq!(
            convert(&json!([]), "file:///a.rs", &at),
            (Vec::new(), false)
        );
    }
}
