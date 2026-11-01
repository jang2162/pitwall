//! 완료 기준 ①: 내장 언어마다 픽스처 하나(`tests/fixtures/`)를 실제 내장 확장의 문법 · 정의 쿼리(`tags.scm`)로 색인해 심볼 표(이름 ·
//! 종류 · 범위 · 컨테이너)를 스냅숏(`tests/snapshots/`)과 견준다. 쿼리를 다시 만들었거나(`mise run grammars`) 종류 규칙을 바꿔
//! 결과가 달라지면 `cargo insta review` 로 차이를 보고 받아들인다.

use std::fmt::Write as _;
use std::sync::Arc;

use pw_ext::Registry;
use pw_fs::Fs;
use pw_index::methods::{IndexFileSymbols, IndexRange};
use pw_index::{IndexSymbolKind, SymbolIndex};
use semver::Version;

/// (픽스처 파일, 언어 id, 반드시 나와야 하는 (종류, 이름)).
type Case = (
    &'static str,
    &'static str,
    &'static [(IndexSymbolKind, &'static str)],
);

const CASES: &[Case] = {
    use IndexSymbolKind::{
        Class, Constant, Constructor, Enum, EnumMember, Field, Function, Impl, Interface, Macro,
        Method, Module, Property, Struct, Trait, TypeAlias,
    };
    &[
        (
            "rust.rs",
            "rust",
            &[
                (Constant, "LIMIT"),
                (Struct, "Point"),
                (Field, "x"),
                (Enum, "Shape"),
                (EnumMember, "Circle"),
                (TypeAlias, "Pair"),
                (Trait, "Area"),
                (Impl, "Shape"),
                (Method, "area"),
                (Method, "new"),
                (Macro, "square"),
                (Module, "nested"),
                (Function, "helper"),
                (Function, "main"),
            ],
        ),
        (
            "typescript.ts",
            "typescript",
            &[
                (Interface, "Shape"),
                (TypeAlias, "Pair"),
                (Enum, "Color"),
                (EnumMember, "Red"),
                (Class, "Base"),
                (Class, "Circle"),
                (Field, "radius"),
                (Method, "area"),
                (Function, "makeCircle"),
                (Function, "square"),
                (Module, "Geometry"),
                (Function, "external"),
            ],
        ),
        (
            "tsx.tsx",
            "tsx",
            &[
                (Interface, "Props"),
                (TypeAlias, "Size"),
                (Function, "Title"),
                (Function, "Badge"),
                (Class, "Panel"),
                (Method, "render"),
            ],
        ),
        (
            "javascript.js",
            "javascript",
            &[
                (Class, "Counter"),
                (Method, "increment"),
                (Function, "makeCounter"),
                (Function, "double"),
                (Function, "onClick"),
                (Function, "ids"),
            ],
        ),
        (
            "python.py",
            "python",
            &[
                (Constant, "LIMIT"),
                (Class, "Shape"),
                (Method, "__init__"),
                (Class, "Circle"),
                (Function, "make_circle"),
                (Function, "inner"),
            ],
        ),
        (
            "java.java",
            "java",
            &[
                (Class, "Shapes"),
                (Field, "LIMIT"),
                (Field, "label"),
                (Constructor, "Shapes"),
                (Method, "area"),
                (Interface, "Shape"),
                (Enum, "Color"),
                (EnumMember, "RED"),
                (Class, "Point"),
                (Interface, "Marker"),
            ],
        ),
        (
            "kotlin.kt",
            "kotlin",
            &[
                (Property, "LIMIT"),
                (Interface, "Shape"),
                (Enum, "Color"),
                (EnumMember, "RED"),
                (Class, "Point"),
                (Class, "Circle"),
                (Method, "area"),
                (Class, "Factory"),
                (Class, "Registry"),
                (TypeAlias, "Shapes"),
                (Function, "makeCircle"),
            ],
        ),
        (
            "go.go",
            "go",
            &[
                (Struct, "Point"),
                (Interface, "Shape"),
                (TypeAlias, "ID"),
                (Method, "Area"),
                (Function, "NewPoint"),
            ],
        ),
        (
            "c.c",
            "c",
            &[
                (Struct, "point"),
                (TypeAlias, "point_t"),
                (Enum, "color"),
                (Function, "square"),
                (Function, "main"),
            ],
        ),
        (
            "cpp.cpp",
            "cpp",
            &[
                (Class, "Shape"),
                (Struct, "Point"),
                (Method, "area"),
                (Function, "square"),
            ],
        ),
    ]
};

fn index() -> SymbolIndex {
    let registry = Registry::from_roots(&Version::new(0, 0, 0), pw_ext::builtin::roots());
    SymbolIndex::new(Arc::new(Fs::new()), Arc::new(registry))
}

fn range(r: &IndexRange) -> String {
    format!(
        "{}:{}-{}:{}",
        r.start.line, r.start.character, r.end.line, r.end.character
    )
}

/// 한 줄에 심볼 하나: 깊이만큼 들여 쓰고 `종류 이름 범위 (이름 범위)`.
fn table(result: &IndexFileSymbols) -> String {
    let mut out = String::new();
    for s in &result.symbols {
        let mut depth = 0;
        let mut up = s.parent;
        while let Some(p) = up {
            depth += 1;
            up = result.symbols[p as usize].parent;
        }
        writeln!(
            out,
            "{}{:?} {} {} ({})",
            "  ".repeat(depth),
            s.kind,
            s.name,
            range(&s.range),
            range(&s.selection_range)
        )
        .unwrap();
    }
    out
}

#[test]
fn every_bundled_language_fixture_matches_its_symbol_snapshot() {
    let index = index();
    for (file, language, required) in CASES {
        let path = format!("{}/tests/fixtures/{file}", env!("CARGO_MANIFEST_DIR"));
        let text = std::fs::read_to_string(&path).unwrap();
        let result = index.parse_text(file, &text);
        assert_eq!(result.language.as_deref(), Some(*language), "{file}");
        for (kind, name) in *required {
            assert!(
                result
                    .symbols
                    .iter()
                    .any(|s| s.kind == *kind && s.name == *name),
                "{file}: no {kind:?} `{name}` in\n{}",
                table(&result)
            );
        }
        // 컨테이너는 자기보다 앞에 있고 자기 범위를 품는다.
        for (i, s) in result.symbols.iter().enumerate() {
            if let Some(p) = s.parent {
                assert!((p as usize) < i, "{file}: {} has a later parent", s.name);
            }
        }
        insta::assert_snapshot!(*language, table(&result));
    }
}

#[test]
fn a_language_without_a_tags_query_has_no_symbols() {
    let index = index();
    let result = index.parse_text("data.json", r#"{ "a": 1 }"#);
    assert_eq!(result.language, None);
    assert!(result.symbols.is_empty());
    let result = index.parse_text("notes.txt", "fn main() {}");
    assert_eq!(result.language, None);
}

#[test]
fn a_file_over_the_size_limit_has_no_symbols() {
    let index = index();
    let mut text = String::from("fn main() {}\n");
    while text.len() as u64 <= pw_index::MAX_FILE_BYTES {
        text.push_str("// filler line to pass the limit\n");
    }
    assert!(index.parse_text("big.rs", &text).symbols.is_empty());
    assert_eq!(
        index.parse_text("small.rs", "fn main() {}").symbols.len(),
        1
    );
}

/// 위치는 UTF-16 코드 단위다(한글 · 이모지 뒤의 이름).
#[test]
fn positions_are_utf16_columns() {
    let index = index();
    let result = index.parse_text("u.rs", "/* 한글🙂 */ fn name() {}\n");
    let s = &result.symbols[0];
    assert_eq!(s.name, "name");
    // `/* ` 3 + 한글 2 + 🙂 2 + ` */ fn ` 7 = 14.
    assert_eq!(s.selection_range.start.character, 14);
    assert_eq!(s.selection_range.end.character, 18);
}

/// 1 MiB 상한 바로 아래의 한 줄짜리 파일: 맨 앞 비ASCII 글자 하나 + 정의를 `unit` 으로 되풀이.
pub fn one_line(head: &str, unit: impl Fn(usize) -> String) -> String {
    let mut text = String::from(head);
    let mut i = 0;
    loop {
        let next = unit(i);
        if (text.len() + next.len()) as u64 > pw_index::MAX_FILE_BYTES {
            return text;
        }
        text.push_str(&next);
        i += 1;
    }
}

/// 한 줄이 긴 파일(minified 번들 꼴)도 파일 하나의 시한 안에 끝나거나 심볼 없이 건너뛴다 — 위치 계산이 줄 길이 × 심볼 수가 되지
/// 않는다(리뷰 H1: 1 MiB 한 줄 JS 하나에 45 s 였다). `index.file.symbols { text }` 가 이 길이다.
#[test]
fn a_one_line_megabyte_file_finishes_within_the_budget() {
    let index = index();
    for (file, text) in [
        (
            "bundle.js",
            one_line("var s=\"é\";", |i| format!("function a{i}(){{}}")),
        ),
        (
            "one.rs",
            one_line("const S:&str=\"é\";", |i| format!("fn a{i}(){{}}")),
        ),
    ] {
        let started = std::time::Instant::now();
        let result = index.parse_text(file, &text);
        let took = started.elapsed();
        println!("{file}: {} symbols in {took:?}", result.symbols.len());
        assert!(
            took < pw_index::extract::PARSE_BUDGET * 2,
            "{file}: {took:?} ({} symbols)",
            result.symbols.len()
        );
        // 다 뽑았으면 위치도 맞다: 마지막 심볼의 이름 자리는 그 이름 바로 앞까지의 UTF-16 글자 수.
        if let Some(last) = result.symbols.last() {
            let at = text
                .rfind(&format!(" {}(", last.name))
                .or_else(|| text.rfind(&format!("n {}(", last.name)).map(|i| i + 1))
                .unwrap()
                + 1;
            assert_eq!(
                last.selection_range.start.character as usize,
                text[..at].encode_utf16().count(),
                "{file}"
            );
        }
    }
}
