//! 진단 텍스트에서 경로 · 비밀 · 홈 디렉터리 이름을 가린다.
//!
//! 가리는 것: 홈 경로와 그 사용자 이름, 절대 경로(`/a/b` · `~/a` · `C:\a` · `\\host\share`), URL 의 사용자 정보,
//! `password=` 류 키의 값, `Bearer`/`Basic` 자격, 알려진 토큰 모양(GitHub · GitLab · Slack · AWS · JWT · `sk-`),
//! 영문자와 숫자가 섞인 32자 이상의 불투명 문자열.
//! 보지 않는 것: 상대 경로, 짧거나 사전 단어로 된 비밀, 위 모양에 맞지 않는 토큰. 그래서 로그에 비밀을 쓰지 않는 것이
//! 먼저다(크레이트 머리 주석의 규칙). 이 마스킹은 그 규칙이 새는 자리를 덮는 두 번째 방어선이다.

use std::path::Path;
use std::sync::LazyLock;

use regex::{Captures, Regex};

const PATH: &str = "<path>";
const SECRET: &str = "<secret>";
const REDACTED: &str = "<redacted>";
const USER: &str = "<user>";

static URL_USERINFO: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b([a-z][a-z0-9+.-]*://)[^/\s@]+@").unwrap());
static KEY_VALUE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?i)\b([a-z0-9_.-]*(?:password|passwd|pwd|secret|token|api[_-]?key|access[_-]?key|private[_-]?key|authorization|cookie|credential)s?)(["']?\s*[:=]\s*["']?)([^\s"',;&]+)"#,
    )
    .unwrap()
});
static AUTH_SCHEME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b(bearer|basic|token)\s+[A-Za-z0-9._~+/=-]{8,}").unwrap());
static KNOWN_TOKEN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"\b(?:",
        r"gh[pousr]_[A-Za-z0-9]{20,}",
        r"|github_pat_[A-Za-z0-9_]{20,}",
        r"|glpat-[A-Za-z0-9_-]{20,}",
        r"|sk-[A-Za-z0-9_-]{20,}",
        r"|xox[abposr]-[A-Za-z0-9-]{10,}",
        r"|AKIA[0-9A-Z]{16}",
        r"|eyJ[A-Za-z0-9_-]{5,}\.[A-Za-z0-9_-]{5,}\.[A-Za-z0-9_-]+",
        r")",
    ))
    .unwrap()
});
static OPAQUE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[A-Za-z0-9+/_-]{32,}={0,2}").unwrap());
// regex 크레이트에는 뒤보기가 없다. 앞 글자를 잡아 두고 그대로 돌려 넣어, URL 의 `//` · `:/` 나 낱말 안의 `/` 를
// 경로의 시작으로 보지 않게 한다.
static UNIX_PATH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(^|[^A-Za-z0-9_/:.~\\-])(~(?:/[^\s"'()<>\[\],;]*)?|/[^\s"'()<>\[\],;/]+(?:/[^\s"'()<>\[\],;]*)?)"#)
        .unwrap()
});
static WINDOWS_PATH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)(^|[^A-Za-z0-9_])([a-z]:\\[^\s"'()<>\[\],;]*|\\\\[^\s"'()<>\[\],;]+)"#)
        .unwrap()
});

/// `text` 를 가린다. `home` 은 이 기기의 홈 디렉터리(모르면 `None`). 홈 경로가 경로로 가려진 뒤에도 사용자 이름이
/// 다른 자리(메시지 본문 등)에 남을 수 있어 이름 자체도 낱말 단위로 가린다.
pub fn mask(text: &str, home: Option<&Path>) -> String {
    let mut out = text.to_owned();
    if let Some(home) = home.and_then(Path::to_str).filter(|h| h.len() > 1) {
        let home = home.trim_end_matches(['/', '\\']);
        out = out.replace(home, "~");
        let user = home.rsplit(['/', '\\']).next().unwrap_or_default();
        if user.len() >= 2 {
            let word = Regex::new(&format!(r"\b{}\b", regex::escape(user))).unwrap();
            out = word.replace_all(&out, USER).into_owned();
        }
    }
    out = URL_USERINFO
        .replace_all(&out, format!("${{1}}{REDACTED}@"))
        .into_owned();
    // `Authorization: Bearer x` 에서 키-값 규칙이 먼저 돌면 값으로 `Bearer` 만 가리고 토큰은 남는다.
    out = AUTH_SCHEME
        .replace_all(&out, format!("${{1}} {REDACTED}"))
        .into_owned();
    out = KEY_VALUE
        .replace_all(&out, format!("${{1}}${{2}}{REDACTED}"))
        .into_owned();
    out = KNOWN_TOKEN.replace_all(&out, SECRET).into_owned();
    out = WINDOWS_PATH
        .replace_all(&out, format!("${{1}}{PATH}"))
        .into_owned();
    out = UNIX_PATH
        .replace_all(&out, format!("${{1}}{PATH}"))
        .into_owned();
    // 경로를 먼저 걷는다. 숫자가 든 긴 경로가 불투명 문자열 규칙에 먼저 걸리면 `<secret>` 으로 잘못 이름 붙는다.
    OPAQUE
        .replace_all(&out, |c: &Captures<'_>| {
            let s = &c[0];
            let mixed =
                s.bytes().any(|b| b.is_ascii_digit()) && s.bytes().any(|b| b.is_ascii_alphabetic());
            if mixed {
                SECRET.to_owned()
            } else {
                s.to_owned()
            }
        })
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home() -> Option<&'static Path> {
        Some(Path::new("/Users/alice"))
    }

    #[test]
    fn home_paths_and_the_user_name_disappear() {
        let text = "opened /Users/alice/work/secret-project/src/main.rs:12 for alice";
        let masked = mask(text, home());
        assert!(!masked.contains("alice"), "{masked}");
        assert!(!masked.contains("secret-project"), "{masked}");
        assert_eq!(masked, "opened <path> for <user>");
    }

    #[test]
    fn other_absolute_paths_are_masked_on_every_os_shape() {
        let cases = [
            ("read /home/bob/.config/x.toml failed", "read <path> failed"),
            (
                "at /rustc/abc/library/std/src/panicking.rs:697:5",
                "at <path>",
            ),
            ("cwd=~/dev/repo", "cwd=<path>"),
            (r"open C:\Users\Carol\AppData\x.log", "open <path>"),
            (r"share \\fileserver\team\notes", "share <path>"),
            ("file (\"/var/tmp/a b\")", "file (\"<path> b\")"),
        ];
        for (text, want) in cases {
            assert_eq!(mask(text, None), want, "{text}");
        }
    }

    #[test]
    fn non_paths_are_kept() {
        let text =
            "version 0.0.0 · macos/aarch64 · 3/4 done · https://example.com/a/b · src/lib.rs:3";
        assert_eq!(mask(text, None), text);
    }

    #[test]
    fn token_shaped_strings_are_masked() {
        let ghp = format!("ghp_{}", "A1b2C3d4E5f6G7h8I9j0K1l2");
        // (원문, 가린 뒤 남으면 안 되는 조각, 가린 결과)
        let cases = [
            (
                format!("push with {ghp} rejected"),
                "A1b2C3",
                "push with <secret> rejected".to_owned(),
            ),
            (
                "Authorization: Bearer abc.def-ghi_jkl".into(),
                "abc.def",
                "Authorization: <redacted> <redacted>".into(),
            ),
            (
                "db password=hunter2 user=x".into(),
                "hunter2",
                "db password=<redacted> user=x".into(),
            ),
            (
                r#"{"apiKey": "k-123456"}"#.into(),
                "k-123456",
                r#"{"apiKey": "<redacted>"}"#.into(),
            ),
            (
                "push https://bob:s3cr3t@git.example.com/r.git".into(),
                "s3cr3t",
                "push https://<redacted>@git.example.com/r.git".into(),
            ),
            (
                "aws AKIAABCDEFGHIJKLMNOP".into(),
                "AKIAAB",
                "aws <secret>".into(),
            ),
            (
                "jwt eyJhbGciOi.eyJzdWIiOi.c2lnbmF0dXJl".into(),
                "eyJ",
                "jwt <secret>".into(),
            ),
            (
                "slack xoxb-1234567890-abcdef".into(),
                "xoxb",
                "slack <secret>".into(),
            ),
            (
                "blob 9f8e7d6c5b4a39281706f5e4d3c2b1a0ZZZZ".into(),
                "9f8e7d",
                "blob <secret>".into(),
            ),
        ];
        for (text, leaked, want) in cases {
            let masked = mask(&text, None);
            assert!(!masked.contains(leaked), "{text} → {masked}");
            assert_eq!(masked, want, "{text}");
        }
    }

    #[test]
    fn long_runs_without_digits_are_not_secrets() {
        let text = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        assert_eq!(mask(text, None), text);
    }
}
