; 생성물 — scripts/grammars.mjs 가 아래 원본 쿼리를 이어 붙였다. 손으로 고치지 않는다(출처표 scripts/grammars.toml).

; ---- https://github.com/camdencheek/tree-sitter-dockerfile @ 868e44ce378deb68aac902a9db68ff82d2299dd0 — queries/highlights.scm

[
	"FROM"
	"AS"
	"RUN"
	"CMD"
	"LABEL"
	"EXPOSE"
	"ENV"
	"ADD"
	"COPY"
	"ENTRYPOINT"
	"VOLUME"
	"USER"
	"WORKDIR"
	"ARG"
	"ONBUILD"
	"STOPSIGNAL"
	"HEALTHCHECK"
	"SHELL"
	"MAINTAINER"
	"CROSS_BUILD"
	(heredoc_marker)
	(heredoc_end)
] @keyword

[
	":"
	"@"
] @operator

(comment) @comment


(image_spec
	(image_tag
		":" @punctuation.special)
	(image_digest
		"@" @punctuation.special))

[
	(double_quoted_string)
	(single_quoted_string)
	(json_string)
	(heredoc_line)
] @string

(expansion
  [
	"$"
	"{"
	"}"
  ] @punctuation.special
) @none

((variable) @constant
 (#match? @constant "^[A-Z][A-Z_0-9]*$"))
