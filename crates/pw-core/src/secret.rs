//! 이름으로 가리는 비밀 키. 값을 밖으로 내보내는 곳(MCP 감사 로그의 인자 요약 `pw_mcp::audit::summarize`, 설정 조회 툴
//! `settings_get`)이 같은 한 벌을 쓴다 — 목록이 둘이면 한쪽만 고쳐져 조용히 갈라진다.
//!
//! 키 이름으로만 거르므로 이름이 평범한 키에 든 비밀(값 안의 토큰)은 남는다. 비밀은 설정 파일 · 툴 인자에 두지 않는다는 설계 규칙
//! (키체인 · env 참조 — 06-roadmap.md D47, `db_connection_upsert` 는 비밀번호를 인자로 받지 않는다)이 첫 번째 방어선이고 이것은
//! 두 번째다.

/// 값을 지우는 키의 단어(camelCase · snake_case · kebab-case 로 가른 단어 하나가 이것과 같으면). 비교는 소문자로. 복수형도 따로
/// 적는다 — 단어 하나가 통째로 같아야 걸린다(`tokens` 는 `token` 과 다른 단어다).
pub const SECRET_WORDS: &[&str] = &[
    "password",
    "passwords",
    "passwd",
    "pass",
    "passphrase",
    "passphrases",
    "secret",
    "secrets",
    "token",
    "tokens",
    "key",
    "keys",
    "apikey",
    "apikeys",
    "auth",
    "authorization",
    "credential",
    "credentials",
    "cookie",
    "cookies",
    "private",
    "otp",
];

/// 지운 값 자리에 두는 글.
pub const REDACTED: &str = "[redacted]";

/// 키를 단어로 갈랐을 때 [`SECRET_WORDS`] 의 단어가 있는가(`apiKey`, `db_password`, `X-Auth-Token`). `keymap` 처럼 한 단어 안에
/// 든 것은 아니다.
pub fn is_secret_key(key: &str) -> bool {
    words(key)
        .iter()
        .any(|w| SECRET_WORDS.contains(&w.as_str()))
}

/// `apiKey` → [api, key], `db_password` → [db, password], `X-Auth-Token` → [x, auth, token], `APIToken` → [api, token](대문자
/// 약어 뒤에 소문자로 이어지는 단어가 오면 그 단어의 첫 대문자에서 가른다).
fn words(key: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut previous_lower = false;
    let mut previous_upper = false;
    let mut chars = key.chars().peekable();
    while let Some(c) = chars.next() {
        if !c.is_alphanumeric() {
            if !current.is_empty() {
                words.push(std::mem::take(&mut current));
            }
            previous_lower = false;
            previous_upper = false;
            continue;
        }
        let next_lower = chars.peek().is_some_and(|n| n.is_lowercase());
        let boundary = c.is_uppercase() && (previous_lower || (previous_upper && next_lower));
        if boundary && !current.is_empty() {
            words.push(std::mem::take(&mut current));
        }
        previous_lower = c.is_lowercase() || c.is_ascii_digit();
        previous_upper = c.is_uppercase();
        current.extend(c.to_lowercase());
    }
    if !current.is_empty() {
        words.push(current);
    }
    words
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_split_into_words() {
        for key in [
            "apiKey",
            "db_password",
            "X-Auth-Token",
            "private_key",
            "token",
            // 단어 목록에 그대로 없고 대문자 경계에서 갈라야만 걸린다.
            "dbPassword",
            "authToken",
            // 대문자 약어 뒤의 경계와 복수형.
            "APIToken",
            "HTTPAuth",
            "api_keys",
            "accessTokens",
            "secrets",
        ] {
            assert!(is_secret_key(key), "{key}");
        }
        for key in [
            "keymap",
            "passthrough",
            "projectId",
            "tokenizer",
            "path",
            "URLPath",
            "HTTPServer",
        ] {
            assert!(!is_secret_key(key), "{key}");
        }
    }
}
