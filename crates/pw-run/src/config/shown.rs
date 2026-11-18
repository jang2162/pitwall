//! 승인 계획에 보이는 값의 가림(17-m6-plan M6-14 ② 의 규칙 — 실행 구성 쓰기 툴 M6-13 과 실행 툴 M6-14 · 첫 실행 확인 M6-07 ⑥ 이
//! 같은 것을 쓴다).
//!
//! - **가리는 것은 env 의 값뿐이다**: 이름이 비밀 단어인 키의 값(`pw_core::secret::is_secret_key` — 감사 · `settings_get` 과 같은
//!   한 벌)과, 값 안의 `scheme://user:pass@…` 의 비밀번호 자리(비밀 단어로는 `DATABASE_URL` 의 비밀번호를 못 가린다). 가린 자리는
//!   늘 [`MASK`] 와 그 값의 sha256 앞 8자다 — 사람이 「값이 바뀌었는가」 는 본다(가린 값만 바뀐 diff 가 앞뒤 같아 보이지 않게).
//! - **실행될 글은 가리지 않는다**(M6-13 리뷰 B1 메인 판정): 명령 · 인자 · 스크립트 · 태스크 · `[debug]` 의 어댑터 값처럼 env 표
//!   밖의 글은 그대로 보인다 — 셸 명령 안의 `x://a:<코드>@b` 를 가리면 사람은 무엇이 도는지 못 본다. 그런 글에 비밀처럼 보이는
//!   것([`secret_looking`])이 있으면 가리는 대신 경고([`config_warnings`] · [`secret_warning`])를 덧붙인다.
//! - 예외: `GIT_CONFIG_KEY_<n>`(단어 `key`)은 비밀 이름이 아니다 — 가리면 짝인 `GIT_CONFIG_VALUE_<n>` 이 무슨 git 설정
//!   (`core.fsmonitor` · `core.sshCommand`)인지 사람이 못 본다(M6-03 리뷰에서 M6-14 로 넘긴 것).
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

/// env 하나의 보이는 값: 비밀 이름이면 [`MASK`] 와 값의 해시 앞 8자, 아니면 URL 의 비밀번호 자리만 가린 값. **env 의 값에만**
/// 쓴다 — 실행될 글(argv · 셸 명령)에 쓰지 않는다(머리 주석).
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

/// 값 안의 `scheme://user:pass@host` 마다의 `(앞, 비밀번호, 뒤)` 자리(바이트). 권한 자리는 다음 `/` · `?` · `#` · 공백 앞까지.
fn url_passwords(value: &str) -> Vec<(usize, usize)> {
    let mut found = Vec::new();
    let mut from = 0;
    while let Some(at) = value[from..].find("://") {
        let start = from + at + 3;
        let tail = &value[start..];
        let end = tail
            .find(|c: char| matches!(c, '/' | '?' | '#') || c.is_whitespace())
            .unwrap_or(tail.len());
        let authority = &tail[..end];
        if let Some(user_end) = authority.rfind('@')
            && let Some(colon) = authority[..user_end].find(':')
        {
            found.push((start + colon + 1, start + user_end));
        }
        from = start + end;
    }
    found
}

/// env 의 값 안의 `scheme://user:pass@host` 마다 `pass` 를 [`MASK`] 와 그 비밀번호의 해시 앞 8자로 바꾼다. 비밀번호 칸이 없으면
/// (`user@host`) 그대로다. env 의 값에만 쓴다.
pub fn url_password_masked(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut last = 0;
    for (start, end) in url_passwords(value) {
        out.push_str(&value[last..start]);
        out.push_str(&masked(&value[start..end]));
        last = end;
    }
    out.push_str(&value[last..]);
    out
}

/// 실행될 글(argv 한 칸 · 셸 명령 · 어댑터 값)에 비밀처럼 보이는 것이 있는가: 비밀번호가 든 URL(`x://a:b@h`), 또는 비밀 이름의
/// 대입(`API_TOKEN=…` · `--password=…`). 이 글은 가리지 않는다 — [`secret_warning`] 으로 경고만 단다.
pub fn secret_looking(text: &str) -> bool {
    !url_passwords(text).is_empty()
        || text.split_whitespace().any(|word| {
            word.split_once('=').is_some_and(|(name, value)| {
                !value.is_empty() && is_secret_name(name.trim_start_matches('-'))
            })
        })
}

/// 실행될 글들 가운데 비밀처럼 보이는 것이 있으면 사람에게 보일 경고 한 줄(값은 싣지 않는다). 없으면 `None`. 실행 계획(M6-14)은
/// argv 의 칸들을, 첫 실행 확인(M6-07 ⑥)은 그 구성의 명령을 넘긴다.
pub fn secret_warning<'a>(texts: impl IntoIterator<Item = &'a str>) -> Option<String> {
    texts.into_iter().any(secret_looking).then(|| {
        "This command line holds a value that looks like a secret (a URL with a password, or NAME=value with a secret \
         word in the name). It is shown as written, not hidden — check what runs."
            .to_owned()
    })
}

/// 구성 파일(TOML) 글의 보이는 꼴: **env 표**(최상위 `env`, `[profiles.<이름>.env]`, `[debug]` 아래의 `env` — 어느 깊이든 키
/// 이름이 `env` 인 표)의 값만 [`env_value`] 로 가린다. 그 밖(명령 · 인자 · 어댑터 값 · 모르는 키)은 그대로다 — 실행될 글을 가리지
/// 않는다(머리 주석). 줄 · 주석 · 키 순서는 그대로다(diff 가 읽히게). TOML 로 읽을 수 없는 글은 무엇이 비밀인지 가를 수 없어
/// 내용 대신 그렇다는 한 줄이다.
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

/// `env` 표 안이면(`in_env`) 키마다 그 이름으로 값을 가린다. 밖이면 `env` 라는 키의 표로만 들어간다.
fn mask_table(table: &mut dyn TableLike, in_env: bool) {
    for (key, item) in table.iter_mut() {
        let name = key.get().to_owned();
        if in_env {
            mask_env_item(item, &name);
        } else {
            let child_env = name == "env";
            match item {
                Item::Table(t) => mask_table(t, child_env),
                Item::Value(Value::InlineTable(t)) => mask_table(t, child_env),
                Item::ArrayOfTables(tables) => {
                    for t in tables.iter_mut() {
                        mask_table(t, false);
                    }
                }
                _ => {}
            }
        }
    }
}

/// env 표의 값 하나(이름 `name`). 글자 값만 가린다 — env 의 값은 글자다.
fn mask_env_item(item: &mut Item, name: &str) {
    if let Item::Value(Value::String(s)) = item {
        let shown = env_value(name, s.value());
        if shown != *s.value() {
            let decor = s.decor().clone();
            let mut next = toml_edit::Formatted::new(shown);
            *next.decor_mut() = decor;
            *s = next;
        }
    }
}

/// 구성 파일 글에서 env 표 밖의 글자 값 가운데 비밀처럼 보이는 것([`secret_looking`]) · 이름이 비밀 단어인 키의 자리(점 경로 —
/// 값은 싣지 않는다). 가리지 않고 보이므로 사람에게 경고한다. TOML 이 아니면 빈 목록.
pub fn config_warnings(text: &str) -> Vec<String> {
    let Ok(doc) = text.parse::<DocumentMut>() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    scan_table(doc.as_table(), "", &mut out);
    out
}

fn scan_table(table: &dyn TableLike, at: &str, out: &mut Vec<String>) {
    for (key, item) in table.iter() {
        let here = if at.is_empty() {
            key.to_owned()
        } else {
            format!("{at}.{key}")
        };
        if key == "env" {
            continue;
        }
        match item {
            Item::Table(t) => scan_table(t, &here, out),
            Item::ArrayOfTables(tables) => {
                for t in tables.iter() {
                    scan_table(t, &here, out);
                }
            }
            Item::Value(value) => scan_value(value, &here, is_secret_name(key), out),
            Item::None => {}
        }
    }
}

fn scan_value(value: &Value, at: &str, secret_name: bool, out: &mut Vec<String>) {
    match value {
        Value::String(s) => {
            if secret_name || secret_looking(s.value()) {
                out.push(at.to_owned());
            }
        }
        Value::Array(items) => {
            if items
                .iter()
                .any(|v| v.as_str().is_some_and(|t| secret_name || secret_looking(t)))
            {
                out.push(at.to_owned());
            }
        }
        Value::InlineTable(t) => scan_table(t, at, out),
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
