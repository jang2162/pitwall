//! 한 문서가 서비스를 붙잡지 않는가(`src/service.rs` 머리 주석의 상한들). 입력은 M1-10 리뷰가 찾은, 오류 회복이 폭주하는 문서들이다
//! — 시간 상한이 없을 때 release 로 400 KB 괄호 53초, 1 MB `[` 120초 넘게, 5 MB 깨진 Rust 12초가 걸렸다.

use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use pw_ext::Registry;
use pw_syntax::methods::{
    SyntaxEditParams, SyntaxHighlightsParams, SyntaxHighlightsResult, SyntaxOff, SyntaxOpenParams,
    TextChange, TextRange,
};
use pw_syntax::{Syntax, parse_budget};
use semver::Version;

fn syntax() -> Syntax {
    Syntax::new(Arc::new(Registry::from_roots(
        &Version::new(0, 0, 0),
        pw_ext::builtin::roots(),
    )))
}

/// 닫히지 않은 괄호 20만 줄(400 KB).
fn rust_parens() -> String {
    format!("fn f() {{\n{}", "(\n".repeat(200_000))
}

/// 닫히지 않은 `[` 50만 줄(1 MB).
fn json_brackets() -> String {
    "[\n".repeat(500_000)
}

/// 짝이 맞지 않는 괄호 · 따옴표 · 주석 시작이 섞인 줄 40만 개(5.2 MB, 5 MiB 안).
fn broken_rust() -> String {
    "}{)(][\"'#!/*\n".repeat(400_000)
}

fn open(s: &Syntax, doc: &str, language: &str, text: String) {
    let opened = s
        .open(SyntaxOpenParams {
            doc_id: doc.to_owned(),
            language: language.to_owned(),
            text,
        })
        .unwrap();
    assert_eq!(opened.off, None, "{doc}");
    assert!(opened.grammar.is_some(), "{doc}");
}

fn highlights(s: &Syntax, doc: &str) -> SyntaxHighlightsResult {
    s.highlights(&SyntaxHighlightsParams {
        doc_id: doc.to_owned(),
        range: TextRange { from: 0, to: 2000 },
    })
    .unwrap()
}

/// 파싱이 상한에 걸리면 그 문서의 강조를 끄고 상한 안에 끝난다.
#[test]
fn runaway_parses_stop_at_the_time_budget() {
    let cases = [
        ("Rust", rust_parens()),
        ("JSON", json_brackets()),
        ("Rust", broken_rust()),
    ];
    let runs: Vec<_> = cases
        .into_iter()
        .map(|(language, text)| {
            thread::spawn(move || {
                let s = syntax();
                let budget = parse_budget(text.len());
                open(&s, "d", language, text);
                let started = Instant::now();
                let result = highlights(&s, "d");
                let took = started.elapsed();
                assert_eq!(
                    (result.off, result.spans.len()),
                    (Some(SyntaxOff::TimedOut), 0),
                    "{language}"
                );
                assert!(
                    took < budget + Duration::from_secs(2),
                    "{language}: took {took:?}, budget {budget:?}"
                );
                // 끈 문서는 다시 파싱하지 않는다.
                let started = Instant::now();
                assert_eq!(highlights(&s, "d").off, Some(SyntaxOff::TimedOut));
                assert!(started.elapsed() < Duration::from_millis(100));
            })
        })
        .collect();
    for run in runs {
        run.join().unwrap();
    }
}

/// 한 문서의 파싱이 상한까지 도는 동안 다른 언어의 문서는 곧바로 강조되고, 같은 언어의 문서는 그 파싱이 끝나면 강조된다.
#[test]
fn other_documents_highlight_while_one_parse_runs_away() {
    let s = Arc::new(syntax());
    open(&s, "stuck", "Rust", broken_rust());
    open(&s, "json", "JSON", "{\"a\": [1, true]}".to_owned());
    open(&s, "rust", "Rust", "fn a() {}".to_owned());
    let stuck = {
        let s = Arc::clone(&s);
        thread::spawn(move || highlights(&s, "stuck"))
    };
    thread::sleep(Duration::from_millis(300));

    let started = Instant::now();
    let json = highlights(&s, "json");
    let took = started.elapsed();
    assert!(!json.spans.is_empty());
    assert!(took < Duration::from_secs(1), "JSON waited {took:?}");
    assert!(
        !stuck.is_finished(),
        "the runaway parse ended too early to tell"
    );

    let rust = highlights(&s, "rust");
    assert!(!rust.spans.is_empty());
    assert_eq!(stuck.join().unwrap().off, Some(SyntaxOff::TimedOut));
}

/// 새 편집이 오거나 문서가 닫히면 하던 파싱을 그만두고 그 요청은 파싱하던 버전 · 빈 구간으로 끝난다.
#[test]
fn a_new_edit_or_closing_stops_the_parse_in_progress() {
    let s = Arc::new(syntax());
    open(&s, "d", "Rust", broken_rust());
    let parse = |s: &Arc<Syntax>| {
        let s = Arc::clone(s);
        thread::spawn(move || highlights(&s, "d"))
    };

    let running = parse(&s);
    thread::sleep(Duration::from_millis(300));
    let edited = Instant::now();
    s.edit(SyntaxEditParams {
        doc_id: "d".to_owned(),
        version: 1,
        changes: vec![TextChange {
            from: 0,
            to: 0,
            insert: "x".to_owned(),
        }],
    })
    .unwrap();
    let result = running.join().unwrap();
    let took = edited.elapsed();
    assert_eq!(
        (result.version, result.spans.len(), result.off),
        (0, 0, None)
    );
    assert!(
        took < Duration::from_secs(1),
        "stopped {took:?} after the edit"
    );

    let running = parse(&s);
    thread::sleep(Duration::from_millis(300));
    let closed = Instant::now();
    s.close("d");
    let result = running.join().unwrap();
    let took = closed.elapsed();
    assert_eq!(
        (result.version, result.spans.len(), result.off),
        (1, 0, None)
    );
    assert!(
        took < Duration::from_secs(1),
        "stopped {took:?} after closing"
    );
}
