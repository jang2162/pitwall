//! 내장 언어마다 픽스처 하나(`tests/fixtures/`)를 실제 내장 확장의 문법 · 쿼리로 강조해 스냅숏(`tests/snapshots/`)과 견준다.
//! 문법 판본을 올렸거나 쿼리를 다시 만들었거나(`mise run grammars`) 토큰 규칙을 바꿨으면 `cargo insta review` 로 차이를 보고
//! 받아들인다.

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
        let grammar = syntax
            .open(SyntaxOpenParams {
                doc_id: (*file).to_owned(),
                language: (*linguist).to_owned(),
                text: text.clone(),
            })
            .unwrap()
            .grammar;
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

/// 키워드 · 문자열 · 주석의 토큰 무리(`keyword` · `string.*` · `comment.*`). 에디터는 토큰마다 기본 테마의 `syntax.<토큰>` 색을
/// 칠한다(`apps/desktop/src/editor/extensions/highlight.ts`).
fn family(token: SyntaxToken) -> Option<&'static str> {
    let name = token.name();
    ["keyword", "string", "comment"]
        .into_iter()
        .find(|f| name == *f || name.starts_with(&format!("{f}.")))
}

/// 픽스처에 나오는 무리. 셋이 다 없는 언어는 그 문법에 그 무리가 없다(`keyword` 가 없는 데이터 · 마크업 언어, 주석이 없는 JSON,
/// 셋 다 없는 Markdown — 제목 · 코드 · 링크 토큰만 있다).
const FAMILIES: &[(&str, &[&str])] = &[
    ("html.html", &["string", "comment"]),
    ("json.json", &["string"]),
    ("markdown.md", &[]),
    ("toml.toml", &["string", "comment"]),
    ("yaml.yaml", &["string", "comment"]),
];

/// M1-17 완료 기준: 첫 문법 목록의 언어마다 파일을 열면 키워드 · 문자열 · 주석이 서로 다른 토큰 색이다(기본 테마의 두 변형 모두).
#[test]
fn keywords_strings_and_comments_get_different_colors_in_every_builtin_language() {
    let registry = Registry::from_roots(&Version::new(0, 0, 0), pw_ext::builtin::roots());
    let (_, theme) = registry.theme("pitwall-default").expect("default theme");
    let syntax = syntax();
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");
    let mut problems = Vec::new();
    for (file, linguist, _, _) in CASES {
        let text = std::fs::read_to_string(format!("{dir}/{file}")).unwrap();
        let doc = format!("colors-{file}");
        syntax
            .open(SyntaxOpenParams {
                doc_id: doc.clone(),
                language: (*linguist).to_owned(),
                text: text.clone(),
            })
            .unwrap();
        let (_, tokens) = render(&text, &syntax, &doc);
        let mut present: Vec<&str> = tokens.iter().filter_map(|t| family(*t)).collect();
        present.sort_unstable();
        present.dedup();
        let mut expected: Vec<&str> = FAMILIES
            .iter()
            .find(|(f, _)| f == file)
            .map_or(&["keyword", "string", "comment"][..], |(_, f)| f)
            .to_vec();
        expected.sort_unstable();
        if present != expected {
            problems.push(format!(
                "{file}: families {present:?}, expected {expected:?}"
            ));
        }
        for variant in ["light", "dark"] {
            // 무리마다 그 픽스처에 나온 토큰들의 색.
            let mut colors: Vec<(&str, String)> = Vec::new();
            for token in &tokens {
                let Some(f) = family(*token) else { continue };
                let key = format!("syntax.{}", token.name());
                let color = theme.content["variants"][variant][&key]
                    .as_str()
                    .unwrap_or_else(|| panic!("default theme has no {variant}.{key}"));
                colors.push((f, color.to_ascii_lowercase()));
            }
            for (a, ca) in &colors {
                for (b, cb) in &colors {
                    if a < b && ca == cb {
                        problems.push(format!("{file} {variant}: {a} and {b} share {ca}"));
                    }
                }
            }
        }
    }
    problems.dedup();
    assert_eq!(problems, Vec::<String>::new());
}
