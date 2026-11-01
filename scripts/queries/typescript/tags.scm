; Pitwall 보충 — tree-sitter-typescript 의 tags.scm(과 이어 붙인 tree-sitter-javascript 의 것)이 정의로 잡지 않는 것(열거 ·
; 타입 별칭 · namespace · 클래스 필드 · 인터페이스 속성 · 열거 멤버). TypeScript 와 TSX 가 함께 쓴다. 캡처 이름은 tree-sitter 의
; tags 규칙(`@definition.<종류>` · `@name`)을 따른다.
(enum_declaration
  name: (identifier) @name) @definition.enum

(type_alias_declaration
  name: (type_identifier) @name) @definition.type

(internal_module
  name: (_) @name) @definition.module

(public_field_definition
  name: (property_identifier) @name) @definition.field

(property_signature
  name: (property_identifier) @name) @definition.property

(enum_body
  name: (property_identifier) @name @definition.enum_member)

(enum_assignment
  name: (property_identifier) @name) @definition.enum_member
