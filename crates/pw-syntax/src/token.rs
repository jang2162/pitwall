//! 강조 쿼리의 캡처 이름 → 구문 토큰(`docs/design/07-extensibility.md` 「3. 테마」 표의 「구문 색상」 행).
//!
//! 문법 저장소마다 캡처 이름이 다르다 — 옛 이름(`@conditional`, `@text.title`)과 새 이름(`@keyword`, `@markup.heading`)이 섞이고,
//! 같은 뜻을 마디 수만 달리 쓴다(`@function.method.call`). 화면 · 테마가 이 차이를 알지 않도록 서비스가 캡처를 정해진 토큰
//! 목록([`SyntaxToken`])으로 바꿔 내보낸다. 테마의 토큰 이름은 `syntax.` + 토큰 이름이다(`syntax.function.method`).
//!
//! 바꾸는 규칙([`classify`]):
//! 1. `_` 로 시작하는 캡처(쿼리 안에서 술어에만 쓰는 이름), `spell` · `nospell`, `injection.*` · `local.*` 은 강조가 아니다 — 버린다.
//! 2. `none` 은 「강조하지 않음」 이다 — 그 노드를 차지해 바깥 강조를 덮는다(토큰 없음).
//! 3. 캡처 이름을 마디 단위로 뒤에서부터 줄여 가며(`a.b.c` → `a.b` → `a`) 처음 맞는 것을 고른다. 맞추는 대상은 토큰 이름과
//!    옛 이름 별칭표([`ALIASES`])다.
//! 4. 아무것도 맞지 않으면 버린다.

use serde::{Deserialize, Serialize};

macro_rules! tokens {
    ($($variant:ident = $name:literal,)*) => {
        /// 구문 토큰. 와이어에는 이름(`"function.method"`)으로 나간다. 테마 토큰은 `syntax.<이름>` 이다.
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, specta::Type,
            schemars::JsonSchema,
        )]
        pub enum SyntaxToken {
            $(#[serde(rename = $name)] $variant,)*
        }

        impl SyntaxToken {
            /// 전부. 이름순이다.
            pub const ALL: &'static [SyntaxToken] = &[$(SyntaxToken::$variant,)*];

            pub fn name(self) -> &'static str {
                match self {
                    $(SyntaxToken::$variant => $name,)*
                }
            }
        }
    };
}

tokens! {
    Attribute = "attribute",
    Boolean = "boolean",
    Comment = "comment",
    CommentDocumentation = "comment.documentation",
    Constant = "constant",
    ConstantBuiltin = "constant.builtin",
    Constructor = "constructor",
    Embedded = "embedded",
    Function = "function",
    FunctionBuiltin = "function.builtin",
    FunctionMacro = "function.macro",
    FunctionMethod = "function.method",
    Keyword = "keyword",
    Label = "label",
    MarkupBold = "markup.bold",
    MarkupHeading = "markup.heading",
    MarkupItalic = "markup.italic",
    MarkupLink = "markup.link",
    MarkupLinkUrl = "markup.link.url",
    MarkupRaw = "markup.raw",
    Module = "module",
    Number = "number",
    Operator = "operator",
    Property = "property",
    Punctuation = "punctuation",
    PunctuationBracket = "punctuation.bracket",
    PunctuationDelimiter = "punctuation.delimiter",
    PunctuationSpecial = "punctuation.special",
    String = "string",
    StringEscape = "string.escape",
    StringRegexp = "string.regexp",
    StringSpecial = "string.special",
    StringSpecialKey = "string.special.key",
    Tag = "tag",
    Type = "type",
    TypeBuiltin = "type.builtin",
    Variable = "variable",
    VariableBuiltin = "variable.builtin",
    VariableParameter = "variable.parameter",
}

/// 옛 캡처 이름(nvim-treesitter 0.9 이전 관례 — 내장 문법의 쿼리 일부가 아직 쓴다) → 토큰. 앞마디 맞춤([`classify`] 3)에서
/// 토큰 이름과 함께 본다.
pub const ALIASES: &[(&str, SyntaxToken)] = &[
    ("character", SyntaxToken::String),
    ("conditional", SyntaxToken::Keyword),
    ("define", SyntaxToken::Keyword),
    ("delimiter", SyntaxToken::PunctuationDelimiter),
    ("escape", SyntaxToken::StringEscape),
    ("exception", SyntaxToken::Keyword),
    ("field", SyntaxToken::Property),
    ("float", SyntaxToken::Number),
    ("function.special", SyntaxToken::FunctionMacro),
    ("include", SyntaxToken::Keyword),
    ("method", SyntaxToken::FunctionMethod),
    ("namespace", SyntaxToken::Module),
    ("parameter", SyntaxToken::VariableParameter),
    ("preproc", SyntaxToken::Keyword),
    ("repeat", SyntaxToken::Keyword),
    ("storageclass", SyntaxToken::Keyword),
    ("string.regex", SyntaxToken::StringRegexp),
    ("text.emphasis", SyntaxToken::MarkupItalic),
    ("text.literal", SyntaxToken::MarkupRaw),
    ("text.reference", SyntaxToken::MarkupLink),
    ("text.strong", SyntaxToken::MarkupBold),
    ("text.title", SyntaxToken::MarkupHeading),
    ("text.uri", SyntaxToken::MarkupLinkUrl),
    ("type.qualifier", SyntaxToken::Keyword),
];

/// 캡처 하나가 강조에서 하는 일.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureKind {
    /// 강조가 아니다. 노드를 차지하지 않는다.
    Ignore,
    /// `@none` — 노드를 차지하지만 토큰이 없다.
    Plain,
    Token(SyntaxToken),
}

/// 모듈 머리 주석의 규칙 1 ~ 4.
pub fn classify(capture: &str) -> CaptureKind {
    if capture.starts_with('_')
        || matches!(capture, "spell" | "nospell")
        || capture.starts_with("injection.")
        || capture.starts_with("local.")
    {
        return CaptureKind::Ignore;
    }
    if capture == "none" {
        return CaptureKind::Plain;
    }
    let mut name = capture;
    loop {
        if let Some(token) = SyntaxToken::ALL.iter().find(|t| t.name() == name) {
            return CaptureKind::Token(*token);
        }
        if let Some((_, token)) = ALIASES.iter().find(|(alias, _)| *alias == name) {
            return CaptureKind::Token(*token);
        }
        match name.rsplit_once('.') {
            Some((shorter, _)) => name = shorter,
            None => return CaptureKind::Ignore,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_names_map_to_their_token() {
        for token in SyntaxToken::ALL {
            assert_eq!(classify(token.name()), CaptureKind::Token(*token));
        }
    }

    #[test]
    fn longer_names_fall_back_segment_by_segment() {
        assert_eq!(
            classify("function.method.call"),
            CaptureKind::Token(SyntaxToken::FunctionMethod)
        );
        assert_eq!(
            classify("keyword.return"),
            CaptureKind::Token(SyntaxToken::Keyword)
        );
        assert_eq!(
            classify("string.special.key.yaml"),
            CaptureKind::Token(SyntaxToken::StringSpecialKey)
        );
    }

    #[test]
    fn legacy_names_use_the_alias_table_before_shorter_prefixes() {
        // 앞마디 `string` · `type` 보다 별칭이 먼저다.
        assert_eq!(
            classify("string.regex"),
            CaptureKind::Token(SyntaxToken::StringRegexp)
        );
        assert_eq!(
            classify("type.qualifier"),
            CaptureKind::Token(SyntaxToken::Keyword)
        );
        assert_eq!(
            classify("text.title.1"),
            CaptureKind::Token(SyntaxToken::MarkupHeading)
        );
        assert_eq!(
            classify("conditional"),
            CaptureKind::Token(SyntaxToken::Keyword)
        );
    }

    #[test]
    fn non_highlight_captures_are_ignored_and_none_is_plain() {
        for capture in [
            "_name",
            "spell",
            "injection.content",
            "local.scope",
            "nonsense",
            "nonsense.keyword",
        ] {
            assert_eq!(classify(capture), CaptureKind::Ignore, "{capture}");
        }
        assert_eq!(classify("none"), CaptureKind::Plain);
    }

    #[test]
    fn tokens_are_sorted_and_serialize_as_their_name() {
        let names: Vec<&str> = SyntaxToken::ALL.iter().map(|t| t.name()).collect();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        assert_eq!(names, sorted);
        assert_eq!(
            serde_json::to_value(SyntaxToken::FunctionMethod).unwrap(),
            "function.method"
        );
    }
}
