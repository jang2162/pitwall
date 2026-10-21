use std::sync::Arc;
use std::time::{Duration, Instant};

use pw_ext::Registry;
use pw_fs::judge::{EDITABLE_MAX, LONG_LINE_CHARS};
use pw_rpc::{Caller, EventHub, Role, Router, Session, code};
use semver::Version;

use crate::methods::{
    HighlightSpan, SyntaxEditParams, SyntaxHighlightsParams, SyntaxHighlightsResult, SyntaxOff,
    SyntaxOpenParams, SyntaxOpenResult, TextChange, TextRange,
};
use crate::{MAX_DOCUMENTS, Syntax, SyntaxToken};

fn syntax() -> Syntax {
    Syntax::new(Arc::new(Registry::from_roots(
        &Version::new(0, 0, 0),
        pw_ext::builtin::roots(),
    )))
}

fn open(s: &Syntax, doc: &str, language: &str, text: &str) -> Option<String> {
    s.open(SyntaxOpenParams {
        doc_id: doc.to_owned(),
        language: language.to_owned(),
        text: text.to_owned(),
    })
    .unwrap()
    .grammar
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
    .map(|_| ())
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

fn highlights_result(s: &Syntax, doc: &str) -> SyntaxHighlightsResult {
    s.highlights(&SyntaxHighlightsParams {
        doc_id: doc.to_owned(),
        range: TextRange { from: 0, to: 100 },
    })
    .unwrap()
}

fn open_result(s: &Syntax, doc: &str, language: &str, text: String) -> SyntaxOpenResult {
    s.open(SyntaxOpenParams {
        doc_id: doc.to_owned(),
        language: language.to_owned(),
        text,
    })
    .unwrap()
}

/// 결정 M1-D9 의 경계: 5 MiB · 1만 자 줄까지는 열고, 한 바이트 · 한 글자 넘으면 열지 않는다(같은 id 의 앞 문서도 닫는다).
#[test]
fn open_refuses_texts_over_the_size_and_line_limits() {
    let s = syntax();
    let max = usize::try_from(EDITABLE_MAX).unwrap();
    // 100자 줄로 채운 정확히 5 MiB.
    let line = format!("{}\n", "x".repeat(99));
    let mut text = line.repeat(max / line.len());
    text.push_str(&"y".repeat(max - text.len()));
    assert_eq!(text.len(), max);
    let opened = open_result(&s, "big", "JSON", text.clone());
    assert_eq!(
        (opened.grammar.as_deref(), opened.off),
        (Some("json"), None)
    );
    text.push('z');
    let refused = open_result(&s, "big", "JSON", text);
    assert_eq!(
        (refused.grammar, refused.off),
        (None, Some(SyntaxOff::TooLarge))
    );
    assert_eq!(s.document_count(), 0);

    let long = |chars: usize| format!("a\n{}\nb", "가".repeat(chars));
    let opened = open_result(&s, "long", "JSON", long(LONG_LINE_CHARS));
    assert_eq!(opened.off, None);
    let refused = open_result(&s, "long", "JSON", long(LONG_LINE_CHARS + 1));
    assert_eq!(
        (refused.grammar, refused.off),
        (None, Some(SyntaxOff::LongLine))
    );
    assert_eq!(s.document_count(), 0);
}

/// 편집으로 상한을 넘으면 그 문서의 강조를 끄고 알린다. 끈 문서의 편집 · 강조 요청은 `off` 로 끝난다.
#[test]
fn an_edit_that_crosses_a_limit_turns_highlighting_off() {
    let s = syntax();
    open(&s, "d", "Rust", "fn a() {}\n").unwrap();
    let long = "x".repeat(LONG_LINE_CHARS - 2);
    // 정확히 1만 자 줄(`fn` + 공백 없이 이어 붙인다)은 괜찮다.
    let ok = s
        .edit(SyntaxEditParams {
            doc_id: "d".to_owned(),
            version: 1,
            changes: vec![TextChange {
                from: 0,
                to: 9,
                insert: format!("fn{long}"),
            }],
        })
        .unwrap();
    assert_eq!(ok.off, None);
    let over = s
        .edit(SyntaxEditParams {
            doc_id: "d".to_owned(),
            version: 2,
            changes: vec![TextChange {
                from: 0,
                to: 0,
                insert: "x".to_owned(),
            }],
        })
        .unwrap();
    assert_eq!(over.off, Some(SyntaxOff::LongLine));
    let result = highlights_result(&s, "d");
    assert_eq!(
        (result.version, result.spans.len(), result.off),
        (2, 0, Some(SyntaxOff::LongLine))
    );
    // 끈 문서는 편집을 받기만 한다(버전도 보지 않는다).
    assert_eq!(
        s.edit(SyntaxEditParams {
            doc_id: "d".to_owned(),
            version: 2,
            changes: vec![],
        })
        .unwrap()
        .off,
        Some(SyntaxOff::LongLine)
    );

    open(&s, "big", "Rust", "fn a() {}\n").unwrap();
    let max = usize::try_from(EDITABLE_MAX).unwrap();
    let grown = s
        .edit(SyntaxEditParams {
            doc_id: "big".to_owned(),
            version: 1,
            changes: vec![TextChange {
                from: 10,
                to: 10,
                insert: "// x\n".repeat(max / 5),
            }],
        })
        .unwrap();
    assert_eq!(grown.off, Some(SyntaxOff::TooLarge));
    assert_eq!(highlights_result(&s, "big").off, Some(SyntaxOff::TooLarge));
}

/// 앞당겨 온 편집이 범위 오류로 버려지면 그 문서는 웹뷰와 어긋났다 — 다음 편집이 곧바로 conflict 다(웹뷰가 다시 연다).
#[test]
fn a_dropped_early_edit_makes_the_next_edit_conflict() {
    let s = syntax();
    open(&s, "d", "Rust", "fn a() {}\n").unwrap();
    edit(&s, "d", 2, &[(100, 101, "")]).unwrap();
    edit(&s, "d", 3, &[(0, 0, "x")]).unwrap();
    // 버전 1 이 오면 2 를 적용하려다 버린다(3 도 함께 버린다).
    edit(&s, "d", 1, &[(0, 0, "pub ")]).unwrap();
    assert_eq!(edit(&s, "d", 4, &[]), Err(code::CONFLICT));
    assert_eq!(highlights(&s, "d", 0, 100).0, 1);
}

/// 여러 커서 편집(앞으로만 나아가는 바꾸기 여럿)과 되돌아가는 바꾸기도 증분 파싱 결과가 처음부터 파싱한 것과 같다.
#[test]
fn multi_change_edits_give_the_same_spans_as_parsing_the_final_text() {
    let s = syntax();
    let text = "fn a() {\n    let x = 1;\n    let y = 2;\n}\n";
    open(&s, "d", "Rust", text).unwrap();
    highlights(&s, "d", 0, 1000);
    // 두 `let` 을 `const` 로(위치는 앞 바꾸기를 적용한 뒤 기준), 그다음 맨 뒤 → 맨 앞 순서로.
    edit(&s, "d", 1, &[(13, 16, "const"), (30, 33, "const")]).unwrap();
    edit(&s, "d", 2, &[(45, 45, "// end\n"), (0, 0, "// é😀\n")]).unwrap();
    let expected = "// é😀\nfn a() {\n    const x = 1;\n    const y = 2;\n}\n// end\n";
    let (version, incremental) = highlights(&s, "d", 0, 1000);
    assert_eq!(version, 2);
    let fresh = syntax();
    open(&fresh, "f", "Rust", expected).unwrap();
    assert_eq!(incremental, highlights(&fresh, "f", 0, 1000).1);
    assert!(tokens(expected, &incremental).contains(&("const".to_owned(), SyntaxToken::Keyword)));
}

fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    let started = Instant::now();
    while !done() {
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "timed out waiting for {what}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// 문법은 그것을 쓰는 마지막 문서가 사라진 뒤에 내린다. 문서가 남아 있으면 내리지 않고, 내린 뒤 다시 열면 다시 올린다.
#[test]
fn a_grammar_is_unloaded_after_its_last_document_closes() {
    let s = Syntax::with_unload_after(
        Arc::new(Registry::from_roots(
            &Version::new(0, 0, 0),
            pw_ext::builtin::roots(),
        )),
        Duration::ZERO,
    );
    assert_eq!(s.loaded_grammars(), 0);
    open(&s, "a", "Rust", "fn a() {}").unwrap();
    open(&s, "b", "Rust", "fn b() {}").unwrap();
    // JSON 과 JSON with Comments 는 한 문법이다.
    open(&s, "c", "JSON", "{}").unwrap();
    open(&s, "d", "JSON with Comments", "{}").unwrap();
    assert_eq!(s.loaded_grammars(), 2);
    s.close("a");
    s.close("c");
    // 같은 id 로 다른 언어를 열면 앞 문서는 닫힌다.
    assert_eq!(open(&s, "d", "COBOL", ""), None);
    wait_until("the JSON grammar to unload", || s.loaded_grammars() == 1);
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(s.loaded_grammars(), 1, "Rust is still used by `b`");
    assert_eq!(highlights(&s, "b", 0, 9).1[0].token, SyntaxToken::Keyword);
    s.close("b");
    wait_until("the Rust grammar to unload", || s.loaded_grammars() == 0);
    open(&s, "a", "Rust", "fn a() {}").unwrap();
    assert_eq!(s.loaded_grammars(), 1);
    assert_eq!(highlights(&s, "a", 0, 9).1[0].token, SyntaxToken::Keyword);
}

/// 텍스트를 다시 만드는 편집은 비동기 핸들러 스레드가 아니라 블로킹 스레드에서 한다 — 요청을 넘기는 호출은 편집이 끝나기를
/// 기다리지 않는다.
#[tokio::test]
async fn edits_run_off_the_async_handler_thread() {
    let s = Arc::new(syntax());
    // 뒤에서 앞으로 가는 바꾸기는 바꾸기마다 텍스트를 다시 만든다(묶어 적용하지 못하는 모양).
    let text = "fn a() {}\n".repeat(100_000);
    let changes: Vec<TextChange> = (0..40)
        .rev()
        .map(|i| TextChange {
            from: i * 10,
            to: i * 10,
            insert: "x".to_owned(),
        })
        .collect();
    for doc in ["direct", "routed"] {
        open(&s, doc, "Rust", &text).unwrap();
    }
    let started = Instant::now();
    s.edit(SyntaxEditParams {
        doc_id: "direct".to_owned(),
        version: 1,
        changes: changes.clone(),
    })
    .unwrap();
    let direct = started.elapsed();

    let mut router = Router::new();
    crate::methods::register(&mut router, Arc::clone(&s));
    let (session, mut rx) =
        Session::new(Arc::new(router), EventHub::new(), Caller::new(Role::Owner));
    let started = Instant::now();
    session.handle_value(serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "syntax.edit",
        "params": { "docId": "routed", "version": 1, "changes": changes },
    }));
    let handed_over = started.elapsed();
    let reply = serde_json::to_value(rx.recv().await.unwrap()).unwrap();
    assert_eq!(
        reply["result"],
        serde_json::json!({ "off": null }),
        "{reply}"
    );
    assert!(
        handed_over * 4 < direct,
        "handing the edit over took {handed_over:?}; the edit itself takes {direct:?}"
    );
}

/// 앞으로만 나아가는 바꾸기 1,000개(5 MB 문서)는 텍스트를 한 번만 다시 만든다.
#[test]
fn a_thousand_forward_changes_on_a_5_mb_document_apply_in_one_pass() {
    let s = syntax();
    let text = "let x = 1;\n".repeat(450_000);
    open(&s, "d", "Rust", &text).unwrap();
    // 1,000 줄마다 `x` 를 `yy` 로. 위치는 앞 바꾸기들이 늘린 만큼 밀린다.
    let changes: Vec<TextChange> = (0..1000u32)
        .map(|i| TextChange {
            from: i * 450 * 11 + 4 + i,
            to: i * 450 * 11 + 5 + i,
            insert: "yy".to_owned(),
        })
        .collect();
    let started = Instant::now();
    s.edit(SyntaxEditParams {
        doc_id: "d".to_owned(),
        version: 1,
        changes,
    })
    .unwrap();
    let took = started.elapsed();
    assert!(took < Duration::from_secs(2), "the edit took {took:?}");
    let expected = (0..450_000)
        .map(|line| {
            if line % 450 == 0 {
                "let yy = 1;\n"
            } else {
                "let x = 1;\n"
            }
        })
        .collect::<String>();
    assert!(s.text_of("d") == expected);
}

fn builtin_language(linguist: &str) -> pw_ext::ResolvedLanguage {
    Registry::from_roots(&Version::new(0, 0, 0), pw_ext::builtin::roots())
        .language_for_linguist(linguist)
        .unwrap()
}

/// 문서 하나를 끝까지 파싱해 강조한 구간(바이트).
#[cfg(feature = "wasm")]
fn lane_spans(
    mut lane: crate::grammar::Lane,
    text: &str,
) -> Vec<(std::ops::Range<usize>, SyntaxToken)> {
    let go = &mut || std::ops::ControlFlow::Continue(());
    let tree = lane.parse(text, None, go).unwrap();
    let grammar = lane.grammar();
    crate::highlight::spans(
        &grammar.highlights,
        &grammar.kinds,
        &tree,
        text,
        0..text.len(),
        go,
    )
    .unwrap()
}

#[test]
fn an_unknown_builtin_grammar_name_is_an_error() {
    let language = pw_ext::ResolvedLanguage {
        builtin_grammar: Some("cobol".to_owned()),
        ..builtin_language("JSON")
    };
    let Err(e) = crate::grammar::Lane::native(&language) else {
        panic!("loaded a grammar that is not linked");
    };
    assert_eq!(e, "language `json`: no built-in grammar named `cobol`");
}

/// WASM 문법 경로(외부 확장의 문법, M8 — 앱은 `wasm` 기능을 켜지 않는다)가 같은 문법 · 커밋의 네이티브 경로와 같은 강조를
/// 낸다. 시험 입력 `tests/wasm/json.wasm` 은 `mise run grammars` 가 만든다(`scripts/grammars.toml` 의 `test_wasm`).
#[cfg(feature = "wasm")]
#[test]
fn a_wasm_grammar_highlights_like_the_native_one() {
    let native = builtin_language("JSON");
    let wasm = pw_ext::ResolvedLanguage {
        builtin_grammar: None,
        grammar: Some(pw_ext::ResolvedFile {
            root: pw_ext::ExtensionRoot::Dir(
                concat!(env!("CARGO_MANIFEST_DIR"), "/tests/wasm").into(),
            ),
            path: pw_ext::RelPath::parse("json.wasm").unwrap(),
        }),
        ..native.clone()
    };
    let text = "// c\n{\"a\": [1, -2.5e3, true, null, \"s\\n\"], \"b\": {}}\n";
    let engine = tree_sitter::wasmtime::Engine::default();
    let from_wasm = lane_spans(crate::grammar::Lane::wasm(&engine, &wasm).unwrap(), text);
    let from_native = lane_spans(crate::grammar::Lane::native(&native).unwrap(), text);
    assert!(from_native.len() > 8, "{from_native:?}");
    assert_eq!(from_wasm, from_native);
}

/// 진행을 확인하지 않는 일도 시한에 기다림을 끊는다 — 강조 쿼리는 진행 콜백 사이가 몇 초일 수 있다(`by_deadline` 주석).
#[test]
fn work_that_never_checks_its_deadline_is_left_at_the_deadline() {
    let (release, hold) = std::sync::mpsc::channel::<()>();
    let started = Instant::now();
    let out = crate::service::by_deadline(started + Duration::from_millis(100), move || {
        let _ = hold.recv_timeout(Duration::from_secs(30));
        Some(())
    })
    .unwrap();
    let took = started.elapsed();
    drop(release);
    assert_eq!(out, None);
    assert!(took < Duration::from_secs(10), "waited {took:?}");

    let out = crate::service::by_deadline(Instant::now() + Duration::from_secs(30), || Some(7));
    assert_eq!(out.unwrap(), Some(7));
}
