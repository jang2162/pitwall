//! 진단 텍스트에서 경로 · 비밀 · 홈 디렉터리 이름을 가린다.
//!
//! 가리는 것:
//! - 홈 경로와 그 아래 경로 전체(구분자 `/` · `\` · JSON 이스케이프 `\\`, 대소문자 무시)와 사용자 이름(낱말 단위,
//!   대소문자 무시).
//! - 절대 경로: `/a/b` · `~/a` · `~user/a` · `C:\a` · `C:/a` · `\\host\share` · `file://…` · vite 의 `/@fs/…`.
//!   `%2F` · `%5C` · `%3A` 로 퍼센트 인코딩된 경로(Tauri `convertFileSrc` 의 `asset://` 형태)는 먼저 풀고 본다.
//! - URL 의 사용자 정보, `password=` 류 키의 값, `Bearer`/`Basic` 자격, 알려진 토큰 모양(GitHub · GitLab · Slack · AWS ·
//!   JWT · `sk-`), 영문자와 숫자가 섞인 32자 이상의 불투명 문자열.
//!
//! 보지 않는 것(새는 것):
//! - 상대 경로. 공백이 든 경로는 공백 뒤 조각이 구분자로 이어질 때만 함께 가린다 — `/Users/a/My Projects/x` 는
//!   `<path>` 지만 `/Users/a/My Projects` 는 `<path> Projects` 가 된다(마지막 조각이 남는다).
//! - `//host/share` 처럼 슬래시로 쓴 UNC 경로.
//! - 목록에 없는 키의 값(`code=` · `sig=` 같은 URL 쿼리), 공백으로 구분한 비밀(`password hunter2`), Digest
//!   `response="…"`, 위 모양에 맞지 않는 짧은 토큰(`sk_live_…` 등).
//!
//! 지나치게 가리는 것:
//! - 사용자 이름이 흔한 낱말이면(`log` · `dev`) 본문의 같은 낱말도 `<user>` 가 된다.
//! - UUID · git SHA 처럼 쓸모 있는 식별자도 32자 이상이면 `<secret>` 이 된다.
//!
//! 그래서 로그에 비밀을 쓰지 않는 것이 먼저다(크레이트 머리 주석의 규칙). 이 마스킹은 그 규칙이 새는 자리를 덮는 두 번째
//! 방어선이다.

use std::path::Path;
use std::sync::LazyLock;

use regex::{Captures, Regex};

const PATH: &str = "<path>";
const SECRET: &str = "<secret>";
const REDACTED: &str = "<redacted>";
const USER: &str = "<user>";

/// 경로 안에 올 수 있는 글자. 공백 · 따옴표 · 괄호 · 구분 기호에서 끊는다.
const PATH_CHARS: &str = r#"[^\s"'()<>\[\],;]"#;
/// 경로의 나머지. 공백 뒤 조각이 구분자로 이어지면(`My Documents\acme`) 같은 경로로 본다 — 공백 뒤 마지막 조각은 남는다.
static PATH_TAIL: LazyLock<String> =
    LazyLock::new(|| format!(r#"(?:{PATH_CHARS}| [^\s"'()<>\[\],;/\\]+[/\\])*"#));

static PERCENT_SEPARATOR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)%(2F|5C|3A)").unwrap());
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
static FILE_URL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!(r"(?i)\bfile:/{}", *PATH_TAIL)).unwrap());
// vite 개발 서버가 저장소 밖 파일을 내주는 URL(`http://127.0.0.1:1420/@fs/<절대 경로>`). 경로 앞 글자가 숫자라
// 아래 UNIX_PATH 의 앞 글자 규칙에 걸리지 않는다.
static VITE_FS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!(r"/@fs/{}", *PATH_TAIL)).unwrap());
// regex 크레이트에는 뒤보기가 없다. 앞 글자를 잡아 두고 그대로 돌려 넣어, URL 의 `//` · `:/` 나 낱말 안의 `/` 를
// 경로의 시작으로 보지 않게 한다. `~` 는 뒤에 구분자가 올 때만 경로다(`~/a` · `~bob/a`) — 「약 5분」의 `~5` 는 남긴다.
static UNIX_PATH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r#"(^|[^A-Za-z0-9_/:.~\\-])(~[A-Za-z0-9_.-]*[/\\]{tail}|/[^\s"'()<>\[\],;/]+(?:/{tail})?)"#,
        tail = *PATH_TAIL
    ))
    .unwrap()
});
static WINDOWS_PATH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r#"(?i)(^|[^A-Za-z0-9_])([a-z]:[\\/]{tail}|\\\\{PATH_CHARS}{tail})"#,
        tail = *PATH_TAIL
    ))
    .unwrap()
});

/// `text` 를 가린다. `home` 은 이 기기의 홈 디렉터리(모르면 `None`). 홈 경로가 경로로 가려진 뒤에도 사용자 이름이
/// 다른 자리(메시지 본문 등)에 남을 수 있어 이름 자체도 낱말 단위로 가린다.
pub fn mask(text: &str, home: Option<&Path>) -> String {
    let mut out = PERCENT_SEPARATOR
        .replace_all(text, |c: &Captures<'_>| {
            match c[1].to_ascii_uppercase().as_str() {
                "2F" => "/",
                "5C" => "\\",
                _ => ":",
            }
            .to_owned()
        })
        .into_owned();
    // URL 모양의 경로는 홈 규칙보다 먼저 통째로 걷는다. 뒤에 돌리면 홈 규칙이 가운데만 가려 `file://<path>` 류가 둘로 갈린다.
    out = FILE_URL.replace_all(&out, PATH).into_owned();
    out = VITE_FS
        .replace_all(&out, format!("/@fs/{PATH}"))
        .into_owned();
    if let Some(home) = home.and_then(Path::to_str) {
        let parts: Vec<&str> = home.split(['/', '\\']).filter(|p| !p.is_empty()).collect();
        if !parts.is_empty() {
            // 홈과 그 아래 경로를 한 덩어리로 가린다. 홈만 바꾸고 나머지를 경로 규칙에 맡기면, 경로 규칙이 모르는
            // 구분자(`~\a`)에서 홈 뒤쪽이 그대로 샌다. 홈의 구분자는 `/` · `\` · `\\` 를 모두 받는다.
            // 앞 구분자는 하나(JSON 이스케이프면 `\\`)만 먹는다 — 앞의 `//`(URL 등)를 남긴다.
            let lead = if home.starts_with(['/', '\\']) {
                r"(?:\\\\|[/\\])"
            } else {
                ""
            };
            let body = parts
                .iter()
                .map(|p| regex::escape(p))
                .collect::<Vec<_>>()
                .join(r"[/\\]+");
            let home_path =
                Regex::new(&format!(r"(?i){lead}{body}(?:[/\\]{}|\b|$)", *PATH_TAIL)).unwrap();
            out = home_path.replace_all(&out, PATH).into_owned();
        }
        let user = parts.last().copied().unwrap_or_default();
        if user.len() >= 2 {
            let word = Regex::new(&format!(r"(?i)\b{}\b", regex::escape(user))).unwrap();
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
    fn windows_home_paths_are_masked_as_a_whole() {
        let home = Some(Path::new(r"C:\Users\Carol"));
        let cases = [
            (
                r"open C:\Users\Carol\acme-client\src\main.rs failed",
                "open <path> failed",
            ),
            (r"open C:\Users\Carol\My Documents\acme", "open <path>"),
            (
                r#"{"cwd":"C:\\Users\\Carol\\acme-client\\x"}"#,
                r#"{"cwd":"<path>"}"#,
            ),
            (r"c:\users\carol\x and CAROL said", "<path> and <user> said"),
            ("cwd C:/Users/Carol/acme-client/x.ts", "cwd <path>"),
        ];
        for (text, want) in cases {
            let masked = mask(text, home);
            assert!(
                !masked.to_lowercase().contains("carol"),
                "{text} → {masked}"
            );
            assert!(!masked.contains("acme"), "{text} → {masked}");
            assert_eq!(masked, want, "{text}");
        }
    }

    #[test]
    fn the_user_name_is_masked_regardless_of_case() {
        assert_eq!(mask("/users/ALICE/x by Alice", home()), "<path> by <user>");
    }

    #[test]
    fn url_drive_percent_and_vite_path_shapes_are_masked() {
        let cases = [
            ("at C:/Work/acme-client/x.ts:3", "at <path>"),
            ("load file:///Volumes/Data/acme-client/a.ts", "load <path>"),
            ("load file:///Users/alice/acme-client/a.ts", "load <path>"),
            (
                "http://localhost:1420/@fs/Users/alice/acme-client/a.ts",
                "http://localhost:1420/@fs/<path>",
            ),
            (
                "http://localhost:1420/@fs/Volumes/Data/acme-client/a.ts",
                "http://localhost:1420/@fs/<path>",
            ),
            (
                "asset://localhost/%2FUsers%2Falice%2Facme-client",
                "asset://localhost/<path>",
            ),
            (
                "img asset://localhost/C%3A%5CWork%5Cacme",
                "img asset://localhost/<path>",
            ),
            ("see ~bob/acme/x", "see <path>"),
            ("about ~5 min", "about ~5 min"),
        ];
        for (text, want) in cases {
            let masked = mask(text, home());
            assert!(!masked.contains("acme"), "{text} → {masked}");
            assert!(!masked.contains("alice"), "{text} → {masked}");
            assert_eq!(masked, want, "{text}");
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
