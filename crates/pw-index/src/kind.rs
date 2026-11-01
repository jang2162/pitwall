//! 심볼 종류: 정의 쿼리의 캡처 이름(`@definition.<종류>`)을 정의 노드의 문법 종류로 다듬는다.
//!
//! 원본 쿼리는 종류가 굵다 — tree-sitter-rust 는 구조체 · 열거 · 타입 별칭을 모두 `class` 로, 트레이트를 `interface` 로,
//! tree-sitter-kotlin 은 속성 · 열거 항목을 `constant` 로 잡는다. 「클래스로 이동」(M4-16)이 함수와 타입을 가르고 아웃라인(M4-10)이
//! 아이콘을 고르려면 더 가늘어야 하므로, 쿼리를 고치지 않고 정의 노드의 종류 이름과 그 바로 아래 키워드 · 자식 종류로 다듬는다
//! ([`refine`]). 언어마다 표를 두지 않는 대신 노드 이름의 낱말(`struct` · `enum` · `trait` …)을 본다 — 내장 문법들이 같은 낱말을
//! 쓴다.

use serde::{Deserialize, Serialize};
use tree_sitter::Node;

/// 심볼 종류.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    Serialize,
    Deserialize,
    specta::Type,
    schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum IndexSymbolKind {
    Module,
    Class,
    Struct,
    Interface,
    Trait,
    Enum,
    EnumMember,
    TypeAlias,
    Function,
    Method,
    Constructor,
    Field,
    Property,
    Constant,
    Variable,
    Macro,
    /// 구현 블록(Rust 의 `impl`). 이름은 대상 타입이다. 그 안 메서드의 컨테이너가 되려고 잡는다 — 심볼 검색은 `kinds` 로 따로 고르지
    /// 않으면 내지 않는다(같은 이름의 타입과 겹친다).
    Impl,
}

impl IndexSymbolKind {
    /// 타입을 만드는 종류(「클래스로 이동」 이 고르는 것 — 15-m4-plan.md M4-16).
    pub fn is_type(self) -> bool {
        matches!(
            self,
            Self::Class
                | Self::Struct
                | Self::Interface
                | Self::Trait
                | Self::Enum
                | Self::TypeAlias
        )
    }
}

/// 쿼리의 정의 캡처(`@definition.<이것>`). tree-sitter 의 tags 규칙의 이름과 Pitwall 보충 쿼리(`scripts/queries/`)가 더한 이름.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Capture {
    Class,
    Interface,
    Type,
    Function,
    Method,
    Constructor,
    Module,
    Macro,
    Constant,
    Field,
    Property,
    Enum,
    EnumMember,
    Variable,
    Implementation,
}

impl Capture {
    /// 모르는 이름은 `None` — 그 캡처는 정의로 보지 않는다.
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "class" => Self::Class,
            "interface" => Self::Interface,
            "type" => Self::Type,
            "function" => Self::Function,
            "method" => Self::Method,
            "constructor" => Self::Constructor,
            "module" | "namespace" => Self::Module,
            "macro" => Self::Macro,
            "constant" => Self::Constant,
            "field" => Self::Field,
            "property" => Self::Property,
            "enum" => Self::Enum,
            "enum_member" => Self::EnumMember,
            "variable" => Self::Variable,
            "implementation" => Self::Implementation,
            _ => return None,
        })
    }
}

/// 정의 노드 `node` 로 캡처 종류를 다듬는다. 함수가 타입 안에 있으면 메서드인 것은 컨테이너를 안 뒤에 정한다([`in_type`]).
pub(crate) fn refine(capture: Capture, node: Node<'_>) -> IndexSymbolKind {
    use IndexSymbolKind as K;
    let kind = node.kind();
    let has = |word: &str| kind.contains(word);
    match capture {
        Capture::Class | Capture::Type => {
            if has("struct") || has("union") || child(node, &["struct", "struct_type"]) {
                K::Struct
            } else if has("enum") || child(node, &["enum", "enum_class_body", "enum_specifier"]) {
                K::Enum
            } else if has("trait") {
                K::Trait
            } else if has("interface") || child(node, &["interface", "interface_type"]) {
                K::Interface
            } else if capture == Capture::Type
                || kind == "type_item"
                || has("type_alias")
                || kind == "type_definition"
            {
                K::TypeAlias
            } else {
                K::Class
            }
        }
        Capture::Interface => {
            if has("trait") {
                K::Trait
            } else {
                K::Interface
            }
        }
        Capture::Function => K::Function,
        Capture::Method => {
            if has("constructor") {
                K::Constructor
            } else {
                K::Method
            }
        }
        Capture::Constructor => K::Constructor,
        Capture::Module => K::Module,
        Capture::Macro => K::Macro,
        Capture::Constant => {
            if has("property") {
                K::Property
            } else if has("enum") {
                K::EnumMember
            } else {
                K::Constant
            }
        }
        Capture::Field => K::Field,
        Capture::Property => K::Property,
        Capture::Enum => K::Enum,
        Capture::EnumMember => K::EnumMember,
        Capture::Variable => K::Variable,
        Capture::Implementation => K::Impl,
    }
}

/// 타입 안의 함수는 메서드다(Python 의 클래스 안 `def`, Kotlin 의 클래스 안 `fun`). 모듈 안의 메서드는 함수다(tree-sitter-rust 는
/// `mod` 본문도 impl 본문과 같은 `declaration_list` 라 그 안의 함수를 `method` 로 잡는다).
pub(crate) fn in_type(
    kind: IndexSymbolKind,
    container: Option<IndexSymbolKind>,
) -> IndexSymbolKind {
    use IndexSymbolKind as K;
    match (kind, container) {
        (K::Function, Some(c)) if c.is_type() || c == K::Impl => K::Method,
        (K::Method, Some(K::Module)) => K::Function,
        _ => kind,
    }
}

/// `node` 의 바로 아래 자식(이름 없는 키워드 포함) 중 종류가 `kinds` 에 있는 것이 있는가.
fn child(node: Node<'_>, kinds: &[&str]) -> bool {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .any(|c| kinds.contains(&c.kind()))
}
