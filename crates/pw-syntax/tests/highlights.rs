//! 내장 언어마다 픽스처 하나(`tests/fixtures/`)를 실제 내장 확장의 문법 · 쿼리로 강조해 스냅숏(`tests/snapshots/`)과 견준다.
//! 문법 WASM · 쿼리를 다시 만들었거나(`mise run grammars`) 토큰 규칙을 바꿨으면 `cargo insta review` 로 차이를 보고 받아들인다.

use std::sync::Arc;

use pw_ext::Registry;
use pw_syntax::methods::{SyntaxHighlightsParams, SyntaxOpenParams, TextRange};
use pw_syntax::{Syntax, SyntaxToken};
use semver::Version;

/// (픽스처 파일, Linguist 이름, 언어 기여 id, 반드시 나와야 하는 토큰).
const CASES: &[(&str, &str, &str, &[SyntaxToken])] = {
    use SyntaxToken::{
        Comment, ConstantBuiltin, Keyword, MarkupHeading, Number, Property, String, Tag,
    };
    &[
        ("bash.sh", "Shell", "bash", &[Comment, Keyword, String]),
        ("c.c", "C", "c", &[Comment, Keyword, String, Number]),
        ("cpp.cpp", "C++", "cpp", &[Comment, Keyword, String, Number]),
        (
            "css.css",
            "CSS",
            "css",
            &[Comment, Keyword, String, Property],
        ),
        (
            "Dockerfile",
            "Dockerfile",
            "dockerfile",
            &[Comment, Keyword, String],
        ),
        ("go.go", "Go", "go", &[Comment, Keyword, String, Number]),
        ("html.html", "HTML", "html", &[Comment, Tag, String]),
        (
            "java.java",
            "Java",
            "java",
            &[Comment, Keyword, String, Number],
        ),
        (
            "javascript.js",
            "JavaScript",
            "javascript",
            &[Comment, Keyword, String, Number],
        ),
        ("json.json", "JSON", "json", &[String, Number]),
        (
            "kotlin.kt",
            "Kotlin",
            "kotlin",
            &[Comment, Keyword, String, Number],
        ),
        ("markdown.md", "Markdown", "markdown", &[MarkupHeading]),
        (
            "python.py",
            "Python",
            "python",
            &[Comment, Keyword, String, Number],
        ),
        // 숫자를 constant.builtin 으로 잡는다(원본 쿼리).
        (
            "rust.rs",
            "Rust",
            "rust",
            &[Comment, Keyword, String, ConstantBuiltin],
        ),
        // 숫자 리터럴을 따로 잡지 않는다(원본 쿼리).
        ("sql.sql", "SQL", "sql", &[Comment, Keyword, String]),
        (
            "toml.toml",
            "TOML",
            "toml",
            &[Comment, Property, String, Number],
        ),
        ("tsx.tsx", "TSX", "tsx", &[Comment, Keyword, String, Tag]),
        (
            "typescript.ts",
            "TypeScript",
            "typescript",
            &[Comment, Keyword, String, Number],
        ),
        (
            "yaml.yaml",
            "YAML",
            "yaml",
            &[Comment, Property, String, Number],
        ),
    ]
};

fn syntax() -> Syntax {
    Syntax::new(Arc::new(Registry::from_roots(
        &Version::new(0, 0, 0),
        pw_ext::builtin::roots(),
    )))
}

/// 구간마다 한 줄: `줄:열 토큰 "원문"`(줄 · 열은 1부터, 열은 UTF-16 단위).
fn render(text: &str, syntax: &Syntax, doc: &str) -> (String, Vec<SyntaxToken>) {
    let units: Vec<u16> = text.encode_utf16().collect();
    let result = syntax
        .highlights(&SyntaxHighlightsParams {
            doc_id: doc.to_owned(),
            range: TextRange {
                from: 0,
                to: u32::try_from(units.len()).unwrap(),
            },
        })
        .unwrap();
    let mut out = String::new();
    let mut tokens = Vec::new();
    for span in &result.spans {
        let before = &units[..span.from as usize];
        let line = before.iter().filter(|u| **u == u16::from(b'\n')).count() + 1;
        let column = before
            .iter()
            .rev()
            .take_while(|u| **u != u16::from(b'\n'))
            .count()
            + 1;
        let piece = String::from_utf16(&units[span.from as usize..span.to as usize]).unwrap();
        out.push_str(&format!(
            "{line}:{column} {} {piece:?}\n",
            span.token.name()
        ));
        tokens.push(span.token);
    }
    (out, tokens)
}

#[test]
fn every_builtin_language_highlights_its_fixture() {
    let syntax = syntax();
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");
    let mut fixtures: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    fixtures.sort();
    let mut covered: Vec<&str> = CASES.iter().map(|c| c.0).collect();
    covered.sort_unstable();
    assert_eq!(
        fixtures, covered,
        "every fixture has a case and every case a fixture"
    );

    let mut missing = Vec::new();
    for (file, linguist, id, expected) in CASES {
        let text = std::fs::read_to_string(format!("{dir}/{file}")).unwrap();
        let opened = syntax
            .open(SyntaxOpenParams {
                doc_id: (*file).to_owned(),
                language: (*linguist).to_owned(),
                text: text.clone(),
            })
            .unwrap_or_else(|| panic!("{linguist} has no grammar"));
        let grammar = syntax.prepare(opened).unwrap().grammar;
        assert_eq!(grammar.as_deref(), Some(*id), "{file}");
        let (rendered, tokens) = render(&text, &syntax, file);
        for token in *expected {
            if !tokens.contains(token) {
                missing.push(format!("{file}: no `{}` span", token.name()));
            }
        }
        insta::assert_snapshot!(*id, rendered);
    }
    assert_eq!(missing, Vec::<String>::new());
}
