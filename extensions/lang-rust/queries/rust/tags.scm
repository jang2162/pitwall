; 생성물 — scripts/grammars.mjs 가 아래 원본 쿼리를 이어 붙였다. 손으로 고치지 않는다(출처표 scripts/grammars.toml).

; ---- https://github.com/tree-sitter/tree-sitter-rust @ 77a3747266f4d621d0757825e6b11edcbf991ca5 — queries/tags.scm

; ADT definitions

(struct_item
    name: (type_identifier) @name) @definition.class

(enum_item
    name: (type_identifier) @name) @definition.class

(union_item
    name: (type_identifier) @name) @definition.class

; type aliases

(type_item
    name: (type_identifier) @name) @definition.class

; method definitions

(declaration_list
    (function_item
        name: (identifier) @name) @definition.method)

; function definitions

(function_item
    name: (identifier) @name) @definition.function

; trait definitions
(trait_item
    name: (type_identifier) @name) @definition.interface

; module definitions
(mod_item
    name: (identifier) @name) @definition.module

; macro definitions

(macro_definition
    name: (identifier) @name) @definition.macro

; references

(call_expression
    function: (identifier) @name) @reference.call

(call_expression
    function: (field_expression
        field: (field_identifier) @name)) @reference.call

(macro_invocation
    macro: (identifier) @name) @reference.call

; implementations

(impl_item
    trait: (type_identifier) @name) @reference.implementation

(impl_item
    type: (type_identifier) @name
    !trait) @reference.implementation

; ---- Pitwall scripts/queries/rust/tags.scm (MIT — 원본에 없는 정의를 이 저장소가 직접 썼다)

; Pitwall 보충 — tree-sitter-rust 의 tags.scm 이 정의로 잡지 않는 것(상수 · 정적 변수 · 트레이트의 메서드 선언 · 연관 타입 · 열거
; 변형 · 필드 · impl 블록). 캡처 이름은 tree-sitter 의 tags 규칙(`@definition.<종류>` · `@name`)을 따른다. impl 블록은 그 안
; 메서드의 컨테이너가 되게 정의로 잡는다(이름은 대상 타입) — 심볼 검색은 impl 을 고르지 않는다(crates/pw-index).
(const_item
  name: (identifier) @name) @definition.constant

(static_item
  name: (identifier) @name) @definition.constant

(declaration_list
  (function_signature_item
    name: (identifier) @name) @definition.method)

(declaration_list
  (associated_type
    name: (type_identifier) @name) @definition.type)

(enum_variant
  name: (identifier) @name) @definition.enum_member

(field_declaration
  name: (field_identifier) @name) @definition.field

(impl_item
  type: (_) @name) @definition.implementation
