//! 강조 성능(`docs/design/12-m1-plan.md` M1-10 완료 기준): 5,000줄 Rust 파일의 첫 파싱 + 화면 범위 강조가 100 ms 안, 한 글자
//! 편집 뒤 재강조가 16 ms 안. release 로 잰다 — debug 빌드는 wasmtime(cranelift)이 최적화되지 않아 문법을 올리는 데만 1초쯤
//! 걸린다. 기본 시험에서는 빠진다(`#[ignore]`):
//!
//! ```sh
//! cargo test --release -p pw-syntax --test perf -- --ignored --nocapture
//! ```

use std::sync::Arc;
use std::time::{Duration, Instant};

use pw_ext::Registry;
use pw_syntax::Syntax;
use pw_syntax::methods::{
    SyntaxEditParams, SyntaxHighlightsParams, SyntaxOpenParams, TextChange, TextRange,
};
use semver::Version;

const LINES: usize = 5_000;
/// 화면 한 장(줄 수).
const SCREEN: usize = 60;
const EDITS: usize = 50;

fn syntax() -> Syntax {
    Syntax::new(Arc::new(Registry::from_roots(
        &Version::new(0, 0, 0),
        pw_ext::builtin::roots(),
    )))
}

/// 픽스처를 이름을 바꿔 가며 이어 붙인 5,000줄.
fn source() -> String {
    let fixture = include_str!("fixtures/rust.rs");
    let mut out = String::new();
    let mut i = 0;
    while out.lines().count() < LINES {
        out.push_str(&fixture.replace("main", &format!("main_{i}")));
        i += 1;
    }
    out.lines().take(LINES).collect::<Vec<_>>().join("\n") + "\n"
}

/// `line` 줄의 시작(UTF-16 위치 — 픽스처는 ASCII 라 바이트와 같다).
fn line_start(text: &str, line: usize) -> u32 {
    let byte: usize = text.split_inclusive('\n').take(line).map(str::len).sum();
    u32::try_from(byte).unwrap()
}

fn highlight(s: &Syntax, doc: &str, from: u32, to: u32) -> usize {
    s.highlights(&SyntaxHighlightsParams {
        doc_id: doc.to_owned(),
        range: TextRange { from, to },
    })
    .unwrap()
    .spans
    .len()
}

fn open(s: &Syntax, doc: &str, text: &str) {
    let opened = s
        .open(SyntaxOpenParams {
            doc_id: doc.to_owned(),
            language: "Rust".to_owned(),
            text: text.to_owned(),
        })
        .unwrap();
    s.prepare(opened).unwrap();
}

#[test]
#[ignore = "release 로 따로 돈다(머리 주석)"]
fn a_5000_line_rust_file_highlights_within_budget() {
    let text = source();
    assert_eq!(text.lines().count(), LINES);
    let screen = line_start(&text, SCREEN);

    // 문법을 처음 올리는 것까지 포함한 첫 화면.
    let s = syntax();
    let started = Instant::now();
    open(&s, "cold", &text);
    let loaded = started.elapsed();
    let spans = highlight(&s, "cold", 0, screen);
    let cold = started.elapsed();
    assert!(spans > 0);

    // 문법이 이미 올라간 뒤에 연 문서의 첫 화면.
    let started = Instant::now();
    open(&s, "warm", &text);
    highlight(&s, "warm", 0, screen);
    let warm = started.elapsed();

    // 가운데 줄에서 한 글자씩 넣고 그 화면을 다시 강조한다.
    let middle = line_start(&text, LINES / 2);
    let around = (
        line_start(&text, LINES / 2 - SCREEN / 2),
        line_start(&text, LINES / 2 + SCREEN / 2),
    );
    let mut times = Vec::with_capacity(EDITS);
    for i in 0..EDITS {
        let at = middle + u32::try_from(i).unwrap();
        let started = Instant::now();
        s.edit(SyntaxEditParams {
            doc_id: "warm".to_owned(),
            version: u32::try_from(i).unwrap() + 1,
            changes: vec![TextChange {
                from: at,
                to: at,
                insert: "x".to_owned(),
            }],
        })
        .unwrap();
        highlight(
            &s,
            "warm",
            around.0,
            around.1 + u32::try_from(i).unwrap() + 1,
        );
        times.push(started.elapsed());
    }
    times.sort();
    let median = times[EDITS / 2];
    let max = *times.last().unwrap();

    println!(
        "5000-line Rust: first screen incl. grammar load {cold:?} (open + load {loaded:?}); \
         first screen with the grammar loaded {warm:?}; \
         one-char edit + re-highlight median {median:?}, max {max:?} over {EDITS} edits"
    );
    assert!(
        cold < Duration::from_millis(100),
        "first screen took {cold:?}"
    );
    assert!(max < Duration::from_millis(16), "re-highlight took {max:?}");
}
