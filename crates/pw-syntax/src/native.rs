//! 앱에 링크한 내장 문법(결정 M1-D3 의 19개). 내장 언어 확장의 `builtin_grammar` 가 여기의 이름을 가리킨다. 판본 · 출처는
//! `scripts/grammars.toml` 이고, 문법 크레이트는 `Cargo.toml`, 저장소에 넣은 C 소스는 `build.rs` 가 빌드한다.

use tree_sitter::Language;
use tree_sitter_language::LanguageFn;

unsafe extern "C" {
    fn tree_sitter_dockerfile() -> *const ();
}

/// SAFETY: `build.rs` 가 tree-sitter CLI 가 만든 Dockerfile 파서(`grammars/dockerfile/parser.c`)를 링크한다.
const DOCKERFILE: LanguageFn = unsafe { LanguageFn::from_raw(tree_sitter_dockerfile) };

/// (이름, 문법). 이름은 tree-sitter 의 문법 이름이다.
const GRAMMARS: &[(&str, LanguageFn)] = &[
    ("bash", tree_sitter_bash::LANGUAGE),
    ("c", tree_sitter_c::LANGUAGE),
    ("cpp", tree_sitter_cpp::LANGUAGE),
    ("css", tree_sitter_css::LANGUAGE),
    ("dockerfile", DOCKERFILE),
    ("go", tree_sitter_go::LANGUAGE),
    ("html", tree_sitter_html::LANGUAGE),
    ("java", tree_sitter_java::LANGUAGE),
    ("javascript", tree_sitter_javascript::LANGUAGE),
    ("json", tree_sitter_json::LANGUAGE),
    ("kotlin", tree_sitter_kotlin::LANGUAGE),
    // 블록 문법만. 인라인 문법(`INLINE_LANGUAGE`)은 언어 주입이 생길 때 함께 쓴다(`scripts/grammars.toml` 의 markdown 행).
    ("markdown", tree_sitter_md::LANGUAGE),
    ("python", tree_sitter_python::LANGUAGE),
    ("rust", tree_sitter_rust::LANGUAGE),
    ("sql", tree_sitter_sequel::LANGUAGE),
    ("toml", tree_sitter_toml_ng::LANGUAGE),
    ("tsx", tree_sitter_typescript::LANGUAGE_TSX),
    ("typescript", tree_sitter_typescript::LANGUAGE_TYPESCRIPT),
    ("yaml", tree_sitter_yaml::LANGUAGE),
];

/// 이름의 내장 문법.
pub fn language(name: &str) -> Option<Language> {
    GRAMMARS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, f)| Language::new(*f))
}

/// 내장 문법 이름들(시험용).
pub fn names() -> impl Iterator<Item = &'static str> {
    GRAMMARS.iter().map(|(n, _)| *n)
}
