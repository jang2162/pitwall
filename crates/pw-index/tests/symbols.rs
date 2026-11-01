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
