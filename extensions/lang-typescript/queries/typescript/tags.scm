; 생성물 — scripts/grammars.mjs 가 아래 원본 쿼리를 이어 붙였다. 손으로 고치지 않는다(출처표 scripts/grammars.toml).

; ---- https://github.com/tree-sitter/tree-sitter-typescript @ f975a621f4e7f532fe322e13c4f79495e0a7b2e7 — queries/tags.scm

(function_signature
  name: (identifier) @name) @definition.function

(method_signature
  name: (property_identifier) @name) @definition.method

(abstract_method_signature
  name: (property_identifier) @name) @definition.method

(abstract_class_declaration
  name: (type_identifier) @name) @definition.class

(module
  name: (identifier) @name) @definition.module

(interface_declaration
  name: (type_identifier) @name) @definition.interface

(type_annotation
  (type_identifier) @name) @reference.type

(new_expression
  constructor: (identifier) @name) @reference.class

; ---- https://github.com/tree-sitter/tree-sitter-javascript @ 44c892e0be055ac465d5eeddae6d3e194424e7de — queries/tags.scm

(
  (comment)* @doc
  .
  (method_definition
    name: (property_identifier) @name) @definition.method
  (#not-eq? @name "constructor")
  (#strip! @doc "^[\\s\\*/]+|^[\\s\\*/]$")
  (#select-adjacent! @doc @definition.method)
)

(
  (comment)* @doc
  .
  [
    (class
      name: (_) @name)
    (class_declaration
      name: (_) @name)
  ] @definition.class
  (#strip! @doc "^[\\s\\*/]+|^[\\s\\*/]$")
  (#select-adjacent! @doc @definition.class)
)

(
  (comment)* @doc
  .
  [
    (function_expression
      name: (identifier) @name)
    (function_declaration
      name: (identifier) @name)
    (generator_function
      name: (identifier) @name)
    (generator_function_declaration
      name: (identifier) @name)
  ] @definition.function
  (#strip! @doc "^[\\s\\*/]+|^[\\s\\*/]$")
  (#select-adjacent! @doc @definition.function)
)

(
  (comment)* @doc
  .
  (lexical_declaration
    (variable_declarator
      name: (identifier) @name
      value: [(arrow_function) (function_expression)]) @definition.function)
  (#strip! @doc "^[\\s\\*/]+|^[\\s\\*/]$")
  (#select-adjacent! @doc @definition.function)
)

(
  (comment)* @doc
  .
  (variable_declaration
    (variable_declarator
      name: (identifier) @name
      value: [(arrow_function) (function_expression)]) @definition.function)
  (#strip! @doc "^[\\s\\*/]+|^[\\s\\*/]$")
  (#select-adjacent! @doc @definition.function)
)

(assignment_expression
  left: [
    (identifier) @name
    (member_expression
      property: (property_identifier) @name)
  ]
  right: [(arrow_function) (function_expression)]
) @definition.function

(pair
  key: (property_identifier) @name
  value: [(arrow_function) (function_expression)]) @definition.function

(
  (call_expression
    function: (identifier) @name) @reference.call
  (#not-match? @name "^(require)$")
)

(call_expression
  function: (member_expression
    property: (property_identifier) @name)
  arguments: (_) @reference.call)

(new_expression
  constructor: (_) @name) @reference.class

(export_statement value: (assignment_expression left: (identifier) @name right: ([
 (number)
 (string)
 (identifier)
 (undefined)
 (null)
 (new_expression)
 (binary_expression)
 (call_expression)
]))) @definition.constant

; ---- Pitwall scripts/queries/typescript/tags.scm (MIT — 원본에 없는 정의를 이 저장소가 직접 썼다)

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
