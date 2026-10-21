//! 한 문서가 서비스를 붙잡지 않는가(`src/service.rs` 머리 주석의 상한들). 입력은 M1-10 리뷰가 찾은, 오류 회복이 폭주하는 문서들이다
//! — 시간 상한이 없을 때 release 로 400 KB 괄호 53초, 1 MB `[` 120초 넘게, 5 MB 깨진 Rust 12초가 걸렸다.
//!
//! 벽시계로 「상한 + 몇 초 안」 을 재지 않는다 — tree-sitter 안에는 진행 콜백이 불리지 않는 구간(파싱을 마친 뒤 트리 만들기 ·
//! 그만둔 파싱 치우기)이 문서 크기만큼 있어, debug 빌드에 부하가 걸리면 그 구간만 몇 초다. 멈추는지는 상한을 0 으로 줘서, 그만둔
//! 까닭은 결과(`off` · `version`)로 본다. 실제 상한에서 걸리는 시간은 release 측정(`tests/perf.rs` 의 `measure_runaway_parses`).

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

/// 파싱이 상한에 걸리면 그 문서의 강조를 끈다 — 진행 확인에서 멈춘다. 상한을 0 으로 줘 첫 확인에서 멈추게 한다.
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
                let s = syntax().with_budget(|_| Duration::ZERO);
                open(&s, "d", language, text);
                let started = Instant::now();
                let result = highlights(&s, "d");
                let took = started.elapsed();
                assert_eq!(
                    (result.off, result.spans.len()),
                    (Some(SyntaxOff::TimedOut), 0),
                    "{language}"
                );
                // 멈추지 않으면 깨진 Rust 는 release 로도 12초, debug 로 몇 분이다. 첫 확인에서 멈추면 파싱 100 걸음이다.
                assert!(took < Duration::from_secs(20), "{language}: took {took:?}");
            })
        })
        .collect();
    for run in runs {
        run.join().unwrap();
    }
}

/// 끈 문서는 다시 파싱하지 않는다. 이 입력은 강조 쿼리가 상한 안에 끝나지 않아 다시 파싱하면 상한까지 또 돈다 — 상한의 절반이면
/// 기계 부하가 있어도 넉넉히 가른다.
#[test]
fn a_timed_out_document_is_not_parsed_again() {
    let s = syntax();
    let text = rust_parens();
    let budget = parse_budget(text.len());
    open(&s, "d", "Rust", text);
    assert_eq!(highlights(&s, "d").off, Some(SyntaxOff::TimedOut));
    let started = Instant::now();
    assert_eq!(highlights(&s, "d").off, Some(SyntaxOff::TimedOut));
    let again = started.elapsed();
    assert!(again < budget / 2, "took {again:?} again");
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

    // 기다렸다면 상한(6초)까지 막힌 파싱이 끝난 뒤다 — 그 파싱이 아직 도는 동안 끝났는지로 본다.
    let json = highlights(&s, "json");
    assert!(!json.spans.is_empty());
    assert!(
        !stuck.is_finished(),
        "JSON waited for the runaway parse (or it ended too early to tell)"
    );

    let rust = highlights(&s, "rust");
    assert!(!rust.spans.is_empty());
    assert_eq!(stuck.join().unwrap().off, Some(SyntaxOff::TimedOut));
}

/// 새 편집이 오거나 문서가 닫히면 하던 파싱을 그만두고 그 요청은 파싱하던 버전 · 빈 구간으로 끝난다. 그만두지 않으면 이 문서는
/// 상한까지 돌아 `TimedOut` 으로 끝나므로 결과의 `off` 가 그만둔 까닭이다.
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
    assert_eq!(
        (result.version, result.spans.len(), result.off),
        (0, 0, None)
    );

    let running = parse(&s);
    thread::sleep(Duration::from_millis(300));
    s.close("d");
    let result = running.join().unwrap();
    assert_eq!(
        (result.version, result.spans.len(), result.off),
        (1, 0, None)
    );
}
