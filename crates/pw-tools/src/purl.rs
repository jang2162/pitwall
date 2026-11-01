//! Package URL(purl) — Mason 패키지 정의의 `source.id`(`pkg:github/rust-lang/rust-analyzer@2026-10-26` 꼴).
//!
//! 형식은 purl 명세(<https://github.com/package-url/purl-spec>)의 「파싱」 순서를 따른다: `#` 뒤 서브패스 → `?` 뒤 한정자 → 마지막
//! `@` 뒤 판 → 첫 `/` 앞 형식 → 마지막 `/` 뒤 이름 → 나머지 네임스페이스. 각 부분은 퍼센트 디코딩한다. Mason 의 판에는 `/` 가 든 것이
//! 있어(`pkg:generic/Kotlin/kotlin-lsp@kotlin-lsp/v263.7084.0`) 판은 마지막 `@` 뒤 전부다 — 명세도 판을 그렇게 자른다.
//!
//! 받은 데이터는 미신뢰다: 길이 상한을 넘거나 퍼센트 인코딩이 깨졌거나 디코딩한 값에 제어 문자가 있으면 오류다.

use std::collections::BTreeMap;
use std::fmt;

/// purl 문자열 하나의 상한(바이트). registry 의 가장 긴 것이 100 바이트 남짓이다.
pub const MAX_PURL_BYTES: usize = 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Purl {
    /// 소문자.
    pub kind: String,
    /// `/` 로 이은 세그먼트(디코딩함). 없으면 `None`.
    pub namespace: Option<String>,
    pub name: String,
    pub version: Option<String>,
    /// 키는 소문자.
    pub qualifiers: BTreeMap<String, String>,
    pub subpath: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PurlError(pub String);

impl fmt::Display for PurlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for PurlError {}

fn err(msg: impl Into<String>) -> PurlError {
    PurlError(msg.into())
}

impl Purl {
    pub fn parse(s: &str) -> Result<Purl, PurlError> {
        if s.len() > MAX_PURL_BYTES {
            return Err(err(format!("purl is longer than {MAX_PURL_BYTES} bytes")));
        }
        let rest = s
            .strip_prefix("pkg:")
            .ok_or_else(|| err("purl does not start with `pkg:`"))?;
        // 명세: `pkg:` 뒤의 `/` 는 몇 개든 버린다.
        let rest = rest.trim_start_matches('/');

        let (rest, subpath) = match rest.rsplit_once('#') {
            Some((r, sub)) => {
                let segments: Vec<String> = sub
                    .split('/')
                    .filter(|seg| !seg.is_empty() && *seg != "." && *seg != "..")
                    .map(decode)
                    .collect::<Result<_, _>>()?;
                (r, (!segments.is_empty()).then(|| segments.join("/")))
            }
            None => (rest, None),
        };

        let (rest, qualifiers) = match rest.rsplit_once('?') {
            Some((r, q)) => (r, parse_qualifiers(q)?),
            None => (rest, BTreeMap::new()),
        };

        let (rest, version) = match rest.rsplit_once('@') {
            Some((r, v)) if !v.is_empty() => (r, Some(decode(v)?)),
            Some(_) => return Err(err("purl has an empty version after `@`")),
            None => (rest, None),
        };

        let rest = rest.trim_end_matches('/');
        let (kind, rest) = rest
            .split_once('/')
            .ok_or_else(|| err("purl has no `type/name`"))?;
        if kind.is_empty()
            || !kind
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '+' | '-'))
            || kind.starts_with(|c: char| c.is_ascii_digit())
        {
            return Err(err(format!("purl type `{kind}` is not valid")));
        }
        let kind = kind.to_ascii_lowercase();

        let mut segments: Vec<&str> = rest.split('/').filter(|s| !s.is_empty()).collect();
        let name = segments
            .pop()
            .ok_or_else(|| err("purl has no name"))
            .and_then(decode)?;
        if name.is_empty() {
            return Err(err("purl has an empty name"));
        }
        let namespace = if segments.is_empty() {
            None
        } else {
            let decoded: Vec<String> =
                segments.into_iter().map(decode).collect::<Result<_, _>>()?;
            Some(decoded.join("/"))
        };
        Ok(Purl {
            kind,
            namespace,
            name,
            version,
            qualifiers,
            subpath,
        })
    }

    /// 네임스페이스와 이름을 이은 것(`rust-lang/rust-analyzer`, npm 의 `@scope/name`). 네임스페이스가 없으면 이름.
    pub fn full_name(&self) -> String {
        match &self.namespace {
            Some(ns) => format!("{ns}/{}", self.name),
            None => self.name.clone(),
        }
    }
}

fn parse_qualifiers(q: &str) -> Result<BTreeMap<String, String>, PurlError> {
    let mut out = BTreeMap::new();
    for pair in q.split('&').filter(|p| !p.is_empty()) {
        let (k, v) = pair
            .split_once('=')
            .ok_or_else(|| err(format!("purl qualifier `{pair}` has no `=`")))?;
        let key = k.to_ascii_lowercase();
        if key.is_empty()
            || !key
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
        {
            return Err(err(format!("purl qualifier key `{k}` is not valid")));
        }
        let value = decode(v)?;
        // 명세: 값이 빈 한정자는 버린다.
        if value.is_empty() {
            continue;
        }
        if out.insert(key, value).is_some() {
            return Err(err(format!("purl qualifier `{k}` is repeated")));
        }
    }
    Ok(out)
}

/// 퍼센트 디코딩. `%` 뒤 두 자리가 16진수가 아니거나 결과가 UTF-8 이 아니거나 제어 문자가 있으면 오류.
fn decode(s: &str) -> Result<String, PurlError> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = bytes
                .get(i + 1..i + 3)
                .and_then(|h| std::str::from_utf8(h).ok())
                .and_then(|h| u8::from_str_radix(h, 16).ok())
                .ok_or_else(|| err(format!("purl has a broken percent escape in `{s}`")))?;
            out.push(hex);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    let text = String::from_utf8(out).map_err(|_| err(format!("purl part `{s}` is not UTF-8")))?;
    if text.chars().any(char::is_control) {
        return Err(err(format!("purl part `{s}` has a control character")));
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[test]
    fn parse_table() {
        struct Case {
            input: &'static str,
            kind: &'static str,
            namespace: Option<&'static str>,
            name: &'static str,
            version: Option<&'static str>,
            qualifiers: &'static [(&'static str, &'static str)],
            subpath: Option<&'static str>,
        }
        let cases = [
            Case {
                input: "pkg:github/rust-lang/rust-analyzer@2026-10-26",
                kind: "github",
                namespace: Some("rust-lang"),
                name: "rust-analyzer",
                version: Some("2026-10-26"),
                qualifiers: &[],
                subpath: None,
            },
            // 판에 `/` 가 든다(kotlin-lsp).
            Case {
                input: "pkg:generic/Kotlin/kotlin-lsp@kotlin-lsp/v263.7084.0",
                kind: "generic",
                namespace: Some("Kotlin"),
                name: "kotlin-lsp",
                version: Some("kotlin-lsp/v263.7084.0"),
                qualifiers: &[],
                subpath: None,
            },
            // 인코딩하지 않은 npm 스코프 — `@` 가 둘이라 판은 마지막 `@` 뒤다.
            Case {
                input: "pkg:npm/@vue/language-server@3.1.0",
                kind: "npm",
                namespace: Some("@vue"),
                name: "language-server",
                version: Some("3.1.0"),
                qualifiers: &[],
                subpath: None,
            },
            // 네임스페이스 없음.
            Case {
                input: "pkg:pypi/basedpyright@1.40.2",
                kind: "pypi",
                namespace: None,
                name: "basedpyright",
                version: Some("1.40.2"),
                qualifiers: &[],
                subpath: None,
            },
            // npm 스코프(퍼센트 인코딩된 `@`).
            Case {
                input: "pkg:npm/%40vue/language-server@3.1.0",
                kind: "npm",
                namespace: Some("@vue"),
                name: "language-server",
                version: Some("3.1.0"),
                qualifiers: &[],
                subpath: None,
            },
            // 여러 단 네임스페이스 · 형식 대문자 · 한정자(키 소문자, 값 디코딩, 빈 값 버림) · 서브패스(`.` · `..` 버림).
            Case {
                input: "pkg:GOLANG/golang.org/x/tools/gopls@v0.20.0?Repository_URL=https%3A%2F%2Fgo.dev&empty=#cmd/./../gopls",
                kind: "golang",
                namespace: Some("golang.org/x/tools"),
                name: "gopls",
                version: Some("v0.20.0"),
                qualifiers: &[("repository_url", "https://go.dev")],
                subpath: Some("cmd/gopls"),
            },
            // 판 없음 · 앞의 `//`.
            Case {
                input: "pkg://cargo/ripgrep",
                kind: "cargo",
                namespace: None,
                name: "ripgrep",
                version: None,
                qualifiers: &[],
                subpath: None,
            },
            // 이름의 퍼센트 인코딩 · 판의 `+`.
            Case {
                input: "pkg:generic/eclipse/eclipse.jdt.ls%20x@v1.63.0+build",
                kind: "generic",
                namespace: Some("eclipse"),
                name: "eclipse.jdt.ls x",
                version: Some("v1.63.0+build"),
                qualifiers: &[],
                subpath: None,
            },
        ];
        for c in cases {
            let p = Purl::parse(c.input).unwrap_or_else(|e| panic!("{}: {e}", c.input));
            assert_eq!(p.kind, c.kind, "{}", c.input);
            assert_eq!(p.namespace.as_deref(), c.namespace, "{}", c.input);
            assert_eq!(p.name, c.name, "{}", c.input);
            assert_eq!(p.version.as_deref(), c.version, "{}", c.input);
            assert_eq!(p.qualifiers, q(c.qualifiers), "{}", c.input);
            assert_eq!(p.subpath.as_deref(), c.subpath, "{}", c.input);
        }
        assert_eq!(
            Purl::parse("pkg:npm/%40vue/language-server@3")
                .unwrap()
                .full_name(),
            "@vue/language-server"
        );
    }

    #[test]
    fn rejects_broken_input() {
        for bad in [
            "github/rust-lang/rust-analyzer@1",
            "pkg:github",
            "pkg:github/",
            "pkg:1github/a/b@1",
            "pkg:git hub/a@1",
            "pkg:npm/a@",
            "pkg:npm/a%2@1",
            "pkg:npm/a%zz@1",
            "pkg:npm/a%0a@1",
            "pkg:npm/%ff%fe@1",
            "pkg:npm/a@1?noequals",
            "pkg:npm/a@1?k=1&k=2",
            "pkg:npm/a@1?bad%20key=1",
        ] {
            assert!(Purl::parse(bad).is_err(), "{bad} should be rejected");
        }
        let long = format!("pkg:npm/{}@1", "a".repeat(MAX_PURL_BYTES));
        assert!(Purl::parse(&long).is_err());
    }
}
