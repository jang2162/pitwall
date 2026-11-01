; 생성물 — scripts/grammars.mjs 가 아래 원본 쿼리를 이어 붙였다. 손으로 고치지 않는다(출처표 scripts/grammars.toml).

; ---- https://github.com/tree-sitter/tree-sitter-java @ 94703d5a6bed02b98e438d7cad1136c01a60ba2c — queries/tags.scm

(class_declaration
  name: (identifier) @name) @definition.class

(method_declaration
  name: (identifier) @name) @definition.method

(method_invocation
  name: (identifier) @name
  arguments: (argument_list) @reference.call)

(interface_declaration
  name: (identifier) @name) @definition.interface

(type_list
  (type_identifier) @name) @reference.implementation

(object_creation_expression
  type: (type_identifier) @name) @reference.class

(superclass (type_identifier) @name) @reference.class

; ---- Pitwall scripts/queries/java/tags.scm (MIT — 원본에 없는 정의를 이 저장소가 직접 썼다)

; Pitwall 보충 — tree-sitter-java 의 tags.scm 이 정의로 잡지 않는 것(열거 · 레코드 · 애너테이션 타입 · 생성자 · 열거 상수 ·
; 필드). 캡처 이름은 tree-sitter 의 tags 규칙(`@definition.<종류>` · `@name`)을 따른다.
(enum_declaration
  name: (identifier) @name) @definition.enum

(record_declaration
  name: (identifier) @name) @definition.class

(annotation_type_declaration
  name: (identifier) @name) @definition.interface

(constructor_declaration
  name: (identifier) @name) @definition.constructor

(compact_constructor_declaration
  name: (identifier) @name) @definition.constructor

(enum_constant
  name: (identifier) @name) @definition.enum_member

(field_declaration
  declarator: (variable_declarator
    name: (identifier) @name)) @definition.field
