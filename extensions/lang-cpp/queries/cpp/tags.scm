; 생성물 — scripts/grammars.mjs 가 아래 원본 쿼리를 이어 붙였다. 손으로 고치지 않는다(출처표 scripts/grammars.toml).

; ---- https://github.com/tree-sitter/tree-sitter-cpp @ f41e1a044c8a84ea9fa8577fdd2eab92ec96de02 — queries/tags.scm

(struct_specifier name: (type_identifier) @name body:(_)) @definition.class

(declaration type: (union_specifier name: (type_identifier) @name)) @definition.class

(function_declarator declarator: (identifier) @name) @definition.function

(function_declarator declarator: (field_identifier) @name) @definition.function

(function_declarator declarator: (qualified_identifier scope: (namespace_identifier) @local.scope name: (identifier) @name)) @definition.method

(type_definition declarator: (type_identifier) @name) @definition.type

(enum_specifier name: (type_identifier) @name) @definition.type

(class_specifier name: (type_identifier) @name) @definition.class
