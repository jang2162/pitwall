//! 강조 성능(`docs/design/12-m1-plan.md` M1-10 완료 기준): 5,000줄 Rust 파일의 첫 파싱 + 화면 범위 강조가 100 ms 안, 한 글자
//! 편집 뒤 재강조가 16 ms 안. 그리고 상한 · 메모리의 측정(`measure_*` — 단언 없이 수치만 찍는다): 정상 5 MiB 문서의 첫 파싱,
//! 오류 회복이 폭주하는 문서가 끝나는 시간, 큰 편집, 문법을 올리고 내린 메모리. release 로 잰다 — debug 빌드는 wasmtime 의
//! 컴파일 · tree-sitter 의 C 코드가 최적화되지 않는다. 기본 시험에서는 빠진다(`#[ignore]`):
//!
//! ```sh
//! cargo test --release -p pw-syntax --test perf -- --ignored --nocapture --test-threads 1
//! cargo test --release -p pw-syntax --test perf -- --ignored --nocapture --exact measure_memory
//! ```
//!
//! `measure_memory` 는 따로 돌린다 — 다른 측정이 같은 프로세스에서 먼저 돌면 그 메모리가 기준선에 섞인다.

use std::sync::Arc;
use std::time::{Duration, Instant};

use pw_ext::Registry;
use pw_syntax::Syntax;
use pw_syntax::methods::{
    SyntaxEditParams, SyntaxHighlightsParams, SyntaxOpenParams, TextChange, TextRange,
};
use semver::Version;

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");

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
    assert_eq!(opened.grammar.as_deref(), Some("rust"));
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

/// (픽스처, Linguist 이름) — 내장 언어 모두.
const LANGUAGES: &[(&str, &str)] = &[
    ("bash.sh", "Shell"),
    ("c.c", "C"),
    ("cpp.cpp", "C++"),
    ("css.css", "CSS"),
    ("Dockerfile", "Dockerfile"),
    ("go.go", "Go"),
    ("html.html", "HTML"),
    ("java.java", "Java"),
    ("javascript.js", "JavaScript"),
    ("json.json", "JSON"),
    ("kotlin.kt", "Kotlin"),
    ("markdown.md", "Markdown"),
    ("python.py", "Python"),
    ("rust.rs", "Rust"),
    ("sql.sql", "SQL"),
    ("toml.toml", "TOML"),
    ("tsx.tsx", "TSX"),
    ("typescript.ts", "TypeScript"),
    ("yaml.yaml", "YAML"),
];

fn open_as(s: &Syntax, doc: &str, language: &str, text: String) {
    let opened = s
        .open(SyntaxOpenParams {
            doc_id: doc.to_owned(),
            language: language.to_owned(),
            text,
        })
        .unwrap();
    assert!(opened.grammar.is_some(), "{language}: {opened:?}");
}

/// 정상 문서(픽스처를 5 MiB 까지 이어 붙인 것)의 첫 파싱 + 첫 화면. `parse_budget` 의 근거다.
#[test]
#[ignore = "release 로 따로 돈다(머리 주석)"]
fn measure_5mib_first_parse() {
    let s = syntax();
    for (file, language) in LANGUAGES {
        let fixture = std::fs::read_to_string(format!("{FIXTURES}/{file}")).unwrap();
        let mut text = String::new();
        while text.len() + fixture.len() < 5 * 1024 * 1024 {
            text.push_str(&fixture);
            text.push('\n');
        }
        let budget = pw_syntax::parse_budget(text.len());
        open_as(&s, file, language, text);
        let started = Instant::now();
        let result = s
            .highlights(&SyntaxHighlightsParams {
                doc_id: (*file).to_owned(),
                range: TextRange { from: 0, to: 3000 },
            })
            .unwrap();
        println!(
            "{file:14} first parse + screen {:?} (budget {budget:?}, off {:?})",
            started.elapsed(),
            result.off
        );
        s.close(file);
    }
}

/// 오류 회복이 폭주하는 문서(`tests/limits.rs` 와 같은 입력)가 강조를 끄기까지.
#[test]
#[ignore = "release 로 따로 돈다(머리 주석)"]
fn measure_runaway_parses() {
    let cases = [
        (
            "400 KB unclosed parens",
            "Rust",
            format!("fn f() {{\n{}", "(\n".repeat(200_000)),
        ),
        ("1 MB unclosed [", "JSON", "[\n".repeat(500_000)),
        (
            "5.2 MB broken Rust",
            "Rust",
            "}{)(][\"'#!/*\n".repeat(400_000),
        ),
    ];
    for (name, language, text) in cases {
        let s = syntax();
        let budget = pw_syntax::parse_budget(text.len());
        open_as(&s, "d", language, text);
        let started = Instant::now();
        let result = s
            .highlights(&SyntaxHighlightsParams {
                doc_id: "d".to_owned(),
                range: TextRange { from: 0, to: 2000 },
            })
            .unwrap();
        println!(
            "{name}: off {:?} after {:?} (budget {budget:?})",
            result.off,
            started.elapsed()
        );
    }
}

/// 5 MB 문서에 바꾸기 1,000개짜리 편집 하나(여러 커서 · 모두 바꾸기 모양).
#[test]
#[ignore = "release 로 따로 돈다(머리 주석)"]
fn measure_a_large_edit() {
    let s = syntax();
    let line = "let x = 1;\n";
    open_as(&s, "d", "Rust", line.repeat(450_000));
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
    println!("5 MB, 1,000 changes in one edit: {:?}", started.elapsed());
}

/// 이 프로세스의 RSS(MB). `ps` 의 KB 를 읽는다.
fn rss_mb() -> f64 {
    let out = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .unwrap();
    String::from_utf8(out.stdout)
        .unwrap()
        .trim()
        .parse::<f64>()
        .unwrap()
        / 1024.0
}

/// 결정 M1-D2 의 되돌림 조건 「유휴 메모리 +50 MB」: ① 문서 없음 ② 흔한 5개 언어의 문서를 연 상태 ③ 그 문서들을 닫고 문법을
/// 내린 뒤. 문법은 곧바로 내린다(`with_unload_after(0)`).
#[test]
#[ignore = "release 로 따로 돈다(머리 주석)"]
fn measure_memory() {
    let s = Syntax::with_unload_after(
        Arc::new(Registry::from_roots(
            &Version::new(0, 0, 0),
            pw_ext::builtin::roots(),
        )),
        Duration::ZERO,
    );
    let none = rss_mb();
    let common = [
        ("rust.rs", "Rust"),
        ("typescript.ts", "TypeScript"),
        ("json.json", "JSON"),
        ("markdown.md", "Markdown"),
        ("toml.toml", "TOML"),
    ];
    for (file, language) in common {
        let text = std::fs::read_to_string(format!("{FIXTURES}/{file}")).unwrap();
        open_as(&s, file, language, text);
        highlight(&s, file, 0, 3000);
        println!("  + {language}: {:.1} MB", rss_mb());
    }
    let open = rss_mb();
    for (file, _) in common {
        s.close(file);
    }
    let started = Instant::now();
    while s.loaded_grammars() > 0 {
        assert!(started.elapsed() < Duration::from_secs(10));
        std::thread::sleep(Duration::from_millis(10));
    }
    std::thread::sleep(Duration::from_millis(200));
    let unloaded = rss_mb();
    println!(
        "RSS: (1) no documents {none:.1} MB, (2) 5 languages open {open:.1} MB (+{:.1}), \
         (3) closed and unloaded {unloaded:.1} MB (+{:.1})",
        open - none,
        unloaded - none
    );
}
