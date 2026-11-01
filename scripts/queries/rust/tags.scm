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
