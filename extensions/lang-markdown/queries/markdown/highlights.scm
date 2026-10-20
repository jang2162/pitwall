; 생성물 — scripts/grammars.mjs 가 아래 원본 쿼리를 이어 붙였다. 손으로 고치지 않는다(출처표 scripts/grammars.toml).

; ---- https://github.com/tree-sitter-grammars/tree-sitter-markdown @ f969cd3ae3f9fbd4e43205431d0ae286014c05b5 — tree-sitter-markdown/queries/highlights.scm

;From nvim-treesitter/nvim-treesitter
(atx_heading
  (inline) @text.title)

(setext_heading
  (paragraph) @text.title)

[
  (atx_h1_marker)
  (atx_h2_marker)
  (atx_h3_marker)
  (atx_h4_marker)
  (atx_h5_marker)
  (atx_h6_marker)
  (setext_h1_underline)
  (setext_h2_underline)
] @punctuation.special

[
  (link_title)
  (indented_code_block)
  (fenced_code_block)
] @text.literal

(fenced_code_block_delimiter) @punctuation.delimiter

(code_fence_content) @none

(link_destination) @text.uri

(link_label) @text.reference

[
  (list_marker_plus)
  (list_marker_minus)
  (list_marker_star)
  (list_marker_dot)
  (list_marker_parenthesis)
  (thematic_break)
] @punctuation.special

[
  (block_continuation)
  (block_quote_marker)
] @punctuation.special

(backslash_escape) @string.escape
