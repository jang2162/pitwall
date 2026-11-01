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
