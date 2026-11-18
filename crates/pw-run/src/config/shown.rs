//! 승인 계획에 보이는 값의 가림(17-m6-plan M6-14 ② 의 규칙 — 실행 구성 쓰기 툴 M6-13 과 실행 툴 M6-14 가 같은 것을 쓴다).
//!
//! - **env 의 값은 보이고, 이름이 비밀 단어인 키의 값만 가린다**(`pw_core::secret::is_secret_key` — 감사 · `settings_get` 과 같은
//!   한 벌). 가린 자리는 [`MASK`] 와 값의 sha256 앞 8자다 — 사람이 「값이 바뀌었는가」 는 볼 수 있다.
//! - 예외: `GIT_CONFIG_KEY_<n>`(단어 `key`)은 가리지 않는다 — 가리면 짝인 `GIT_CONFIG_VALUE_<n>` 이 무슨 git 설정
//!   (`core.fsmonitor` · `core.sshCommand`)인지 사람이 못 본다(M6-03 리뷰에서 M6-14 로 넘긴 것).
//! - 값이 `scheme://user:pass@…` 꼴이면 비밀번호 자리를 가린다 — 비밀 단어로는 `DATABASE_URL` 의 비밀번호를 못 가린다.
//! - 이 글은 사람이 승인하는 화면에만 쓴다. 감사 · 로그에는 이것도 쓰지 않는다(계획 해시만 남는다).

use std::path::Path;

use sha2::{Digest, Sha256};
use toml_edit::{DocumentMut, Item, TableLike, Value};

use super::RunConfig;
use crate::env::RunEnv;

/// 가린 값의 자리.
pub const MASK: &str = "••••";

/// 승인 화면에서 값을 가리는 이름인가 — 비밀 단어가 든 이름. `GIT_CONFIG_KEY_<n>` 은 아니다(머리 주석).
pub fn is_secret_name(name: &str) -> bool {
    pw_core::secret::is_secret_key(name) && !is_git_config_key(name)
}

/// `GIT_CONFIG_KEY_<숫자>`.
fn is_git_config_key(name: &str) -> bool {
    name.strip_prefix("GIT_CONFIG_KEY_")
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

/// env 하나의 보이는 값: 비밀 이름이면 [`MASK`] 와 값의 해시 앞 8자, 아니면 URL 의 비밀번호 자리만 가린 값.
pub fn env_value(name: &str, value: &str) -> String {
    if is_secret_name(name) {
        masked(value)
    } else {
        url_password_masked(value)
    }
}

/// `•••• (sha256 1a2b3c4d)`.
fn masked(value: &str) -> String {
    let digest = Sha256::digest(value.as_bytes());
    let hex: String = digest.iter().take(4).map(|b| format!("{b:02x}")).collect();
    format!("{MASK} (sha256 {hex})")
}

/// 값 안의 `scheme://user:pass@host` 마다 `pass` 를 [`MASK`] 로 바꾼다. 비밀번호 칸이 없으면(`user@host`) 그대로다.
pub fn url_password_masked(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(at) = rest.find("://") {
        let (head, tail) = rest.split_at(at + 3);
        out.push_str(head);
        // 권한 자리는 다음 `/` · `?` · `#` · 공백 앞까지.
        let end = tail
            .find(|c: char| matches!(c, '/' | '?' | '#') || c.is_whitespace())
            .unwrap_or(tail.len());
        let authority = &tail[..end];
        match authority.rfind('@') {
            Some(user_end) => {
                let userinfo = &authority[..user_end];
                match userinfo.find(':') {
                    Some(colon) => {
                        out.push_str(&userinfo[..=colon]);
                        out.push_str(MASK);
                        out.push_str(&authority[user_end..]);
                    }
                    None => out.push_str(authority),
                }
            }
            None => out.push_str(authority),
        }
        rest = &tail[end..];
    }
    out.push_str(rest);
    out
}

/// 구성 파일(TOML) 글의 보이는 꼴: 이름이 비밀 단어인 키의 글자 값과 URL 의 비밀번호 자리를 가린다 — `env` 표만이 아니라 모든
/// 깊이(`[profiles.<이름>.env]` · `[debug]` · 모르는 키)다. 줄 · 주석 · 키 순서는 그대로다(diff 가 읽히게). TOML 로 읽을 수 없는
/// 글은 무엇이 비밀인지 가를 수 없어 내용 대신 그렇다는 한 줄이다.
pub fn config_text(text: &str) -> String {
    let mut doc: DocumentMut = match text.parse() {
        Ok(doc) => doc,
        Err(_) => {
            return format!(
                "(not valid TOML, {} bytes; not shown because secret values cannot be told apart)",
                text.len()
            );
        }
    };
    mask_table(doc.as_table_mut(), false);
    doc.to_string()
}

fn mask_table(table: &mut dyn TableLike, inherited: bool) {
    for (key, item) in table.iter_mut() {
        let secret = inherited || is_secret_name(key.get());
        mask_item(item, secret);
    }
}

fn mask_item(item: &mut Item, secret: bool) {
    match item {
        Item::Value(value) => mask_value(value, secret),
        Item::Table(table) => mask_table(table, secret),
        Item::ArrayOfTables(tables) => {
            for table in tables.iter_mut() {
                mask_table(table, secret);
            }
        }
        Item::None => {}
    }
}

fn mask_value(value: &mut Value, secret: bool) {
    match value {
        Value::String(s) => {
            let shown = if secret {
                masked(s.value())
            } else {
                url_password_masked(s.value())
            };
            if shown != *s.value() {
                let decor = s.decor().clone();
                let mut next = toml_edit::Formatted::new(shown);
                *next.decor_mut() = decor;
                *s = next;
            }
        }
        Value::Array(items) => {
            for item in items.iter_mut() {
                mask_value(item, secret);
            }
        }
        Value::InlineTable(table) => mask_table(table, secret),
        // 수 · 참거짓 · 날짜는 비밀 키여도 그대로다(값이 짧아 가려도 해시 앞 8자로 풀린다 — 글자 값만 가린다).
        _ => {}
    }
}

/// `config` 의 `envFile` 마다 그 파일이 낼 변수(이름과 보이는 값 — [`env_value`])와 내용 해시. 파일마다 따로 읽어 어느 파일에서
/// 온 키인지 보인다. 읽을 수 없는 파일은 까닭(값 없이 — `EnvError` 의 글)이다. `envFile` 이 없으면 `None`.
pub fn env_files_text(config: &RunConfig, root: &Path) -> Option<String> {
    if config.env_file.is_empty() {
        return None;
    }
    let mut lines = Vec::new();
    for rel in &config.env_file {
        // 그 파일 하나만 읽는다(`RunEnv::resolve` 의 루트 안 판정 · 크기 상한 · 문법을 그대로 쓴다).
        let one = RunConfig {
            env: Default::default(),
            env_file: vec![rel.clone()],
            profiles: Default::default(),
            ..config.clone()
        };
        match RunEnv::resolve(&one, None, root) {
            Ok(env) => {
                let sha = env
                    .files()
                    .first()
                    .map(|d| d.sha256.get(..12).unwrap_or(&d.sha256).to_owned())
                    .unwrap_or_default();
                lines.push(format!("{rel} (sha256 {sha}…):"));
                if env.vars().is_empty() {
                    lines.push("  (no variables)".to_owned());
                }
                for (name, value) in env.vars() {
                    lines.push(format!("  {name}={}", env_value(name, value)));
                }
            }
            Err(e) => lines.push(format!("{rel}: {e}")),
        }
    }
    Some(lines.join("\n"))
}
