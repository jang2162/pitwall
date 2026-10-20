use std::sync::Arc;

use pw_ext::Registry;
use pw_rpc::code;
use semver::Version;

use crate::methods::{
    HighlightSpan, SyntaxEditParams, SyntaxHighlightsParams, SyntaxOpenParams, TextChange,
    TextRange,
};
use crate::{MAX_DOCUMENTS, Syntax, SyntaxToken};

fn syntax() -> Syntax {
    Syntax::new(Arc::new(Registry::from_roots(
        &Version::new(0, 0, 0),
        pw_ext::builtin::roots(),
    )))
}

fn open(s: &Syntax, doc: &str, language: &str, text: &str) -> Option<String> {
    let opened = s.open(SyntaxOpenParams {
        doc_id: doc.to_owned(),
        language: language.to_owned(),
        text: text.to_owned(),
    })?;
    s.prepare(opened).unwrap().grammar
}

fn edit(s: &Syntax, doc: &str, version: u32, changes: &[(u32, u32, &str)]) -> Result<(), i32> {
    s.edit(SyntaxEditParams {
        doc_id: doc.to_owned(),
        version,
        changes: changes
            .iter()
            .map(|(from, to, insert)| TextChange {
                from: *from,
                to: *to,
                insert: (*insert).to_owned(),
            })
            .collect(),
    })
    .map_err(|e| e.code)
}

fn highlights(s: &Syntax, doc: &str, from: u32, to: u32) -> (u32, Vec<HighlightSpan>) {
    let r = s
        .highlights(&SyntaxHighlightsParams {
            doc_id: doc.to_owned(),
            range: TextRange { from, to },
        })
        .unwrap();
    (r.version, r.spans)
}

/// `(텍스트, 토큰)` — UTF-16 위치로 원문을 자른다.
fn tokens(text: &str, spans: &[HighlightSpan]) -> Vec<(String, SyntaxToken)> {
    let units: Vec<u16> = text.encode_utf16().collect();
    spans
        .iter()
        .map(|s| {
            (
                String::from_utf16(&units[s.from as usize..s.to as usize]).unwrap(),
                s.token,
            )
        })
        .collect()
}

#[test]
fn a_language_without_a_grammar_is_not_opened() {
    let s = syntax();
    assert_eq!(open(&s, "a", "COBOL", "IDENTIFICATION DIVISION."), None);
    assert_eq!(s.document_count(), 0);
    // 문법이 있는 언어로 연 뒤 문법이 없는 언어로 다시 열면 앞 문서도 닫힌다.
    assert_eq!(open(&s, "a", "rust", "fn f() {}").as_deref(), Some("rust"));
    assert_eq!(open(&s, "a", "Text", "x"), None);
    assert_eq!(s.document_count(), 0);
}

#[test]
fn rust_keywords_strings_escapes_and_comments_get_their_tokens() {
    let s = syntax();
    let text = "fn main() {\n    let s = \"a\\n\"; // done\n}\n";
    assert_eq!(open(&s, "d", "Rust", text).as_deref(), Some("rust"));
    let (version, spans) = highlights(&s, "d", 0, 1000);
    assert_eq!(version, 0);
    let got = tokens(text, &spans);
    for expected in [
        ("fn", SyntaxToken::Keyword),
        ("main", SyntaxToken::Function),
        ("let", SyntaxToken::Keyword),
        ("\"a", SyntaxToken::String),
        ("\\n", SyntaxToken::StringEscape),
        ("\"", SyntaxToken::String),
        ("// done", SyntaxToken::Comment),
    ] {
        assert!(
            got.contains(&(expected.0.to_owned(), expected.1)),
            "{expected:?} not in {got:?}"
        );
    }
    // 겹치지 않고 차례다.
    assert!(spans.windows(2).all(|w| w[0].to <= w[1].from));
}

#[test]
fn spans_use_utf16_positions_and_are_clipped_to_the_range() {
    let s = syntax();
    let text = "let é = \"😀\"; // ünï\n";
    open(&s, "d", "Rust", text).unwrap();
    let (_, spans) = highlights(&s, "d", 0, 100);
    let got = tokens(text, &spans);
    assert!(
        got.contains(&("\"😀\"".to_owned(), SyntaxToken::String)),
        "{got:?}"
    );
    assert!(
        got.contains(&("// ünï".to_owned(), SyntaxToken::Comment)),
        "{got:?}"
    );
    // 주석 가운데부터 요청하면 그 위치에서 자른다(`// ünï` 의 `ü` 는 UTF-16 17).
    let (_, spans) = highlights(&s, "d", 17, 100);
    assert_eq!(
        tokens(text, &spans),
        [("ünï".to_owned(), SyntaxToken::Comment)]
    );
    assert_eq!(spans[0].from, 17);
    // 끝도 자른다 — 주석(14..20)의 앞 세 글자까지만.
    let (_, spans) = highlights(&s, "d", 0, 17);
    let last = spans.last().unwrap();
    assert_eq!(
        (last.from, last.to, last.token),
        (14, 17, SyntaxToken::Comment)
    );
}

#[test]
fn incremental_edits_give_the_same_spans_as_parsing_the_final_text() {
    let s = syntax();
    let mut text = String::from("fn a() {\n    let x = 1;\n}\n");
    open(&s, "d", "Rust", &text).unwrap();
    // 파싱된 트리를 만들어 두고 편집한다(편집이 앞 트리에 알려져야 하는 경로).
    highlights(&s, "d", 0, 1000);
    let steps: [(u32, u32, &str); 4] = [
        (13, 16, "const"),           // `let` → `const`
        (8, 8, "\n    // é😀 note"), // 줄 넣기(비ASCII)
        (0, 2, "pub fn"),            // 맨 앞
        (24, 40, ""),                // 여러 줄에 걸쳐 지우기
    ];
    for (i, (from, to, insert)) in steps.iter().enumerate() {
        let version = u32::try_from(i).unwrap() + 1;
        edit(&s, "d", version, &[(*from, *to, insert)]).unwrap();
        let units: Vec<u16> = text.encode_utf16().collect();
        let mut next = String::from_utf16(&units[..*from as usize]).unwrap();
        next.push_str(insert);
        next.push_str(&String::from_utf16(&units[*to as usize..]).unwrap());
        text = next;
        let (got_version, incremental) = highlights(&s, "d", 0, 1000);
        assert_eq!(got_version, version);
        let fresh = syntax();
        open(&fresh, "f", "Rust", &text).unwrap();
        assert_eq!(
            incremental,
            highlights(&fresh, "f", 0, 1000).1,
            "after step {i}: {text:?}"
        );
    }
}

#[test]
fn edits_that_arrive_early_wait_for_their_turn() {
    let s = syntax();
    open(&s, "d", "Rust", "fn a() {}\n").unwrap();
    // 버전 2(`a` → `bb`)가 버전 1(맨 앞에 `pub `)보다 먼저 왔다. 2 의 위치는 1 을 적용한 뒤의 텍스트 기준이다.
    edit(&s, "d", 2, &[(7, 8, "bb")]).unwrap();
    assert_eq!(highlights(&s, "d", 0, 100).0, 0);
    edit(&s, "d", 1, &[(0, 0, "pub ")]).unwrap();
    let (version, spans) = highlights(&s, "d", 0, 100);
    assert_eq!(version, 2);
    let got = tokens("pub fn bb() {}\n", &spans);
    assert!(
        got.contains(&("bb".to_owned(), SyntaxToken::Function)),
        "{got:?}"
    );
}

#[test]
fn stale_versions_conflict_and_bad_ranges_change_nothing() {
    let s = syntax();
    open(&s, "d", "Rust", "fn a() {}").unwrap();
    edit(&s, "d", 1, &[(0, 0, "pub ")]).unwrap();
    let e = s
        .edit(SyntaxEditParams {
            doc_id: "d".to_owned(),
            version: 1,
            changes: Vec::new(),
        })
        .unwrap_err();
    assert_eq!(e.code, code::CONFLICT);
    assert_eq!(e.data, Some(serde_json::json!({ "version": 1 })));
    // 두 번째 바꾸기가 텍스트 밖이면 첫 번째도 적용되지 않고 버전도 그대로다.
    assert_eq!(
        edit(&s, "d", 2, &[(0, 0, "x"), (100, 101, "")]),
        Err(code::INVALID_PARAMS)
    );
    edit(&s, "d", 2, &[(0, 4, "")]).unwrap();
    let (version, spans) = highlights(&s, "d", 0, 100);
    assert_eq!(version, 2);
    assert_eq!(
        tokens("fn a() {}", &spans)[0],
        ("fn".to_owned(), SyntaxToken::Keyword)
    );
}

#[test]
fn too_many_early_edits_conflict() {
    let s = syntax();
    open(&s, "d", "Rust", "").unwrap();
    for v in 0..crate::MAX_EARLY_EDITS {
        edit(&s, "d", u32::try_from(v).unwrap() + 2, &[]).unwrap();
    }
    assert_eq!(edit(&s, "d", 1000, &[]), Err(code::CONFLICT));
}

#[test]
fn closed_and_unknown_documents_are_not_found() {
    let s = syntax();
    open(&s, "d", "Rust", "fn a() {}").unwrap();
    s.close("d");
    s.close("d");
    let e = s
        .highlights(&SyntaxHighlightsParams {
            doc_id: "d".to_owned(),
            range: TextRange { from: 0, to: 1 },
        })
        .unwrap_err();
    assert_eq!(e.code, code::NOT_FOUND);
    assert_eq!(edit(&s, "d", 1, &[]), Err(code::NOT_FOUND));
}

#[test]
fn the_least_recently_used_document_is_closed_past_the_limit() {
    let s = syntax();
    for i in 0..MAX_DOCUMENTS {
        open(&s, &format!("d{i}"), "Rust", "").unwrap();
    }
    // d0 을 쓰면 가장 오래 쓰지 않은 것은 d1 이 된다.
    highlights(&s, "d0", 0, 0);
    open(&s, "new", "Rust", "").unwrap();
    assert_eq!(s.document_count(), MAX_DOCUMENTS);
    assert_eq!(edit(&s, "d1", 1, &[]), Err(code::NOT_FOUND));
    edit(&s, "d0", 1, &[]).unwrap();
}

#[test]
fn json_and_json_with_comments_share_one_grammar() {
    let s = syntax();
    assert_eq!(open(&s, "a", "JSON", "{\"a\": 1}").as_deref(), Some("json"));
    let text = "// c\n{\"a\": true}";
    assert_eq!(
        open(&s, "b", "JSON with Comments", text).as_deref(),
        Some("jsonc")
    );
    let got = tokens(text, &highlights(&s, "b", 0, 100).1);
    assert!(
        got.contains(&("// c".to_owned(), SyntaxToken::Comment)),
        "{got:?}"
    );
    assert!(
        got.contains(&("true".to_owned(), SyntaxToken::ConstantBuiltin)),
        "{got:?}"
    );
}

#[test]
fn reversed_ranges_are_invalid() {
    let s = syntax();
    open(&s, "d", "Rust", "fn a() {}").unwrap();
    let e = s
        .highlights(&SyntaxHighlightsParams {
            doc_id: "d".to_owned(),
            range: TextRange { from: 5, to: 2 },
        })
        .unwrap_err();
    assert_eq!(e.code, code::INVALID_PARAMS);
}
