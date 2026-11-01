//! Mason 패키지 정의 안의 표현식 `{{ … }}` (15-m4-plan.md M4-03 ⑤).
//!
//! 기본 번들 패키지가 쓰는 것만 안다: 값 `version` · `source.asset.<필드…>` · `source.download.<필드…>`(값이 문자열일 때)와
//! 필터 `strip_prefix "<글>"`. 그 밖의 것(모르는 필터 · 함수 호출 `take_if_not(…)` · `||` · `{{{` 오타 · 닫히지 않은 `{{`)은
//! 오류다 — 부르는 쪽이 그 패키지(대상별 항목이면 그 대상)를 「지원하지 않음」 으로 둔다. 추측해 채우지 않는다.

use serde_json::Value;

/// 표현식 하나(`{{`·`}}` 안)의 상한(바이트).
const MAX_EXPRESSION_BYTES: usize = 256;

/// 값을 찾는 곳. `source` 는 `{"asset": …, "download": …}` 꼴의 객체다(고른 대상 항목 — 없으면 키가 없다).
pub struct Context<'a> {
    pub version: &'a str,
    pub source: &'a Value,
}

/// 알 수 없거나 값이 없는 표현식. 안의 글은 `{{`·`}}` 를 뺀 그대로다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExprError {
    pub expression: String,
}

impl std::fmt::Display for ExprError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "unsupported expression `{{{{{}}}}}`", self.expression)
    }
}

impl std::error::Error for ExprError {}

/// `template` 안의 표현식을 모두 값으로 바꾼다.
pub fn render(template: &str, ctx: &Context<'_>) -> Result<String, ExprError> {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else {
            return Err(ExprError {
                expression: truncate(after),
            });
        };
        let inner = &after[..end];
        out.push_str(&eval(inner, ctx)?);
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    Ok(out)
}

/// 글에 표현식이 있는가.
pub fn has_expression(s: &str) -> bool {
    s.contains("{{")
}

fn truncate(s: &str) -> String {
    s.chars().take(MAX_EXPRESSION_BYTES).collect()
}

fn eval(inner: &str, ctx: &Context<'_>) -> Result<String, ExprError> {
    let fail = || ExprError {
        expression: truncate(inner),
    };
    if inner.len() > MAX_EXPRESSION_BYTES {
        return Err(fail());
    }
    let mut tokens = Tokens { s: inner, at: 0 };
    let path = tokens.ident().ok_or_else(fail)?;
    let mut value = lookup(path, ctx).ok_or_else(fail)?;
    loop {
        tokens.skip_ws();
        if tokens.done() {
            break;
        }
        if !tokens.eat('|') {
            return Err(fail());
        }
        let filter = tokens.ident().ok_or_else(fail)?;
        match filter {
            "strip_prefix" => {
                let prefix = tokens.string().ok_or_else(fail)?;
                if let Some(stripped) = value.strip_prefix(prefix.as_str()) {
                    value = stripped.to_owned();
                }
            }
            _ => return Err(fail()),
        }
    }
    Ok(value)
}

fn lookup(path: &str, ctx: &Context<'_>) -> Option<String> {
    if path == "version" {
        return Some(ctx.version.to_owned());
    }
    let mut parts = path.split('.');
    if parts.next()? != "source" {
        return None;
    }
    let first = parts.next()?;
    if first != "asset" && first != "download" {
        return None;
    }
    let mut v = ctx.source.get(first)?;
    for part in parts {
        v = v.get(part)?;
    }
    v.as_str().map(str::to_owned)
}

struct Tokens<'a> {
    s: &'a str,
    at: usize,
}

impl<'a> Tokens<'a> {
    fn rest(&self) -> &'a str {
        &self.s[self.at..]
    }

    fn done(&self) -> bool {
        self.at >= self.s.len()
    }

    fn skip_ws(&mut self) {
        let rest = self.rest();
        self.at += rest.len() - rest.trim_start_matches([' ', '\t']).len();
    }

    fn eat(&mut self, c: char) -> bool {
        self.skip_ws();
        if self.rest().starts_with(c) {
            self.at += c.len_utf8();
            true
        } else {
            false
        }
    }

    /// 낱말 · 점으로 이은 경로(`source.asset.bin`). 비면 `None`.
    fn ident(&mut self) -> Option<&'a str> {
        self.skip_ws();
        let rest = self.rest();
        let len = rest
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '.'))
            .unwrap_or(rest.len());
        let word = &rest[..len];
        if word.is_empty()
            || word.starts_with('.')
            || word.ends_with('.')
            || word.contains("..")
            || word.starts_with(|c: char| c.is_ascii_digit())
        {
            return None;
        }
        self.at += len;
        Some(word)
    }

    /// `"…"` 문자열(이스케이프 없음).
    fn string(&mut self) -> Option<String> {
        self.skip_ws();
        let rest = self.rest().strip_prefix('"')?;
        let end = rest.find('"')?;
        let value = rest[..end].to_owned();
        self.at += 1 + end + 1;
        Some(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ctx_render(template: &str, version: &str, source: &Value) -> Result<String, ExprError> {
        render(template, &Context { version, source })
    }

    #[test]
    fn known_values_and_filters() {
        let source = json!({
            "asset": {"file": "ruff-aarch64-apple-darwin.tar.gz", "bin": "ruff-aarch64-apple-darwin/ruff", "nested": {"lsp": "x"}},
            "download": {"config": "config_mac/", "bin": "b"},
        });
        let table = [
            ("{{version}}", "1.2.3"),
            ("v{{ version }}-x", "v1.2.3-x"),
            ("{{source.asset.file}}", "ruff-aarch64-apple-darwin.tar.gz"),
            ("{{ source.asset.bin }}", "ruff-aarch64-apple-darwin/ruff"),
            ("{{source.asset.nested.lsp}}", "x"),
            ("{{source.download.config}}", "config_mac/"),
            ("{{ version | strip_prefix \"1.\" }}", "2.3"),
            ("{{ version | strip_prefix \"v\" }}", "1.2.3"),
            ("{{version|strip_prefix \"1\"|strip_prefix \".\"}}", "2.3"),
            ("no expression", "no expression"),
            ("a{{version}}b{{version}}c", "a1.2.3b1.2.3c"),
        ];
        for (template, want) in table {
            assert_eq!(
                ctx_render(template, "1.2.3", &source).as_deref(),
                Ok(want),
                "{template}"
            );
        }
        assert_eq!(
            ctx_render(
                "kotlin-server-{{ version | strip_prefix \"kotlin-lsp/v\" }}.sit",
                "kotlin-lsp/v263.7084.0",
                &json!({})
            )
            .unwrap(),
            "kotlin-server-263.7084.0.sit"
        );
    }

    #[test]
    fn unknown_expressions_are_errors() {
        let source = json!({"asset": {"bin": {"lsp": "x"}, "n": 1}});
        for template in [
            // kotlin-lsp 의 linux_arm64 항목의 오타.
            "kotlin-server-{{{ version | strip_prefix \"kotlin-lsp/v\" }}/bin/intellij-server",
            "{{ version || strip_prefix \"v\" }}",
            "{{ version | strip_suffix \"v\" }}",
            "{{ version | strip_prefix v }}",
            "{{ version | strip_prefix \"v }}",
            "{{ take_if_not(is_platform(\"win\"), \"x\") }}",
            "{{ source.build.bin }}",
            "{{ source.asset.missing }}",
            // 문자열이 아닌 값.
            "{{ source.asset.bin }}",
            "{{ source.asset.n }}",
            "{{ other }}",
            "{{}}",
            "{{ version",
            "{{ version }} {{ source..asset }}",
            "{{ 1version }}",
        ] {
            assert!(
                ctx_render(template, "1", &source).is_err(),
                "{template} should be unsupported"
            );
        }
        let e = ctx_render("a{{ nope | x }}", "1", &json!({})).unwrap_err();
        assert_eq!(e.expression, " nope | x ");
    }
}
