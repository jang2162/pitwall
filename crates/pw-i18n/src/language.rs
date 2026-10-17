//! 언어 설정 값과 언어 고르기. 규칙의 정본은 `docs/design/07-extensibility.md` §4.1 「언어 고르기」.

use serde_json::Value;

/// 전역 `settings.toml` 에서 UI 언어를 정하는 키. 내장 기본값은 `"system"`(`crates/pw-core/src/config/source.rs`).
pub const LANGUAGE_KEY: &str = "ui.language";

/// 설정 값 `"system"`: OS 언어를 따른다.
pub const SYSTEM: &str = "system";

/// 폴백 언어. 고른 언어에 없는 메시지를 채우고, 아무 언어도 맞지 않으면 이것을 쓴다. 내장 확장에 모든 메시지가 있다.
pub const FALLBACK: &str = "en";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LanguageSetting {
    System,
    /// 사람이 적은 그대로의 언어 태그.
    Tag(String),
}

impl LanguageSetting {
    /// 설정 값을 읽는다. 없으면 `System`. 문자열이 아니거나 언어 태그 모양이 아니면 `Err` 로 이유를 주고, 부르는 쪽은
    /// `System` 으로 다룬다(설정 파일 하나의 잘못된 값이 UI 를 읽을 수 없게 만들지 않게).
    pub fn from_value(value: Option<&Value>) -> Result<Self, String> {
        match value {
            None => Ok(Self::System),
            Some(Value::String(s)) if s == SYSTEM => Ok(Self::System),
            Some(Value::String(s)) if is_tag(s) => Ok(Self::Tag(s.clone())),
            Some(other) => Err(format!(
                "`{LANGUAGE_KEY}` must be \"{SYSTEM}\" or a language tag such as \"ko\", got {other}"
            )),
        }
    }

    /// 설정 파일에 쓰는 값.
    pub fn as_str(&self) -> &str {
        match self {
            Self::System => SYSTEM,
            Self::Tag(tag) => tag,
        }
    }
}

/// BCP 47 언어 태그의 모양만 본다(pw-ext 매니페스트의 기여 이름과 같은 규칙). OS 가 주는 `ko_KR` 도 받는다.
pub fn is_tag(s: &str) -> bool {
    let mut parts = s.split(['-', '_']);
    let language = parts.next().unwrap_or_default();
    (2..=3).contains(&language.len())
        && language.bytes().all(|b| b.is_ascii_alphabetic())
        && parts.all(|p| (1..=8).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_alphanumeric()))
}

/// 쓸 언어의 순서(앞의 것에 없는 메시지를 뒤의 것이 채운다). 첫째가 고른 언어, 끝이 [`FALLBACK`] 이다.
///
/// 요청 목록은 설정한 태그(있으면) 다음에 OS 선호 언어들이다. 요청을 앞에서부터 보며 처음 맞는 등록 언어를 고른다. 요청
/// 하나는 ① 대소문자를 무시하고 같은 태그 ② 끝 마디를 하나씩 떼어 같은 태그(`zh-Hant-TW` → `zh-Hant` → `zh`) ③ 주 언어
/// 마디가 같은 첫 등록 언어(`ko-KR` 요청에 `ko-KP` 만 있으면 그것) 순서로 맞춘다. 아무것도 맞지 않으면 [`FALLBACK`].
/// `available` 은 등록 순서(내장 → 사용자 확장)다.
pub fn negotiate(
    setting: &LanguageSetting,
    os_locales: &[String],
    available: &[String],
) -> Vec<String> {
    let requested = match setting {
        LanguageSetting::Tag(tag) => std::iter::once(tag.as_str())
            .chain(os_locales.iter().map(String::as_str))
            .collect::<Vec<_>>(),
        LanguageSetting::System => os_locales.iter().map(String::as_str).collect(),
    };
    let chosen = requested
        .iter()
        .find_map(|r| best_match(r, available))
        .unwrap_or(FALLBACK)
        .to_owned();
    let mut chain = vec![chosen];
    if !chain[0].eq_ignore_ascii_case(FALLBACK) {
        chain.push(FALLBACK.to_owned());
    }
    chain
}

fn best_match<'a>(requested: &str, available: &'a [String]) -> Option<&'a str> {
    let requested = requested.replace('_', "-");
    // OS 가 붙이는 유니코드 확장(`-u-…`)과 그 뒤는 고르는 데 쓰지 않는다.
    let requested = requested.split("-u-").next().unwrap_or_default();
    let mut range = requested;
    loop {
        if let Some(found) = available.iter().find(|a| a.eq_ignore_ascii_case(range)) {
            return Some(found);
        }
        match range.rsplit_once('-') {
            Some((head, _)) => range = head,
            None => break,
        }
    }
    let language = primary(requested);
    available
        .iter()
        .find(|a| primary(a).eq_ignore_ascii_case(language))
        .map(String::as_str)
}

fn primary(tag: &str) -> &str {
    tag.split(['-', '_']).next().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn strings(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn reads_the_setting_value() {
        assert_eq!(
            LanguageSetting::from_value(None),
            Ok(LanguageSetting::System)
        );
        assert_eq!(
            LanguageSetting::from_value(Some(&json!("system"))),
            Ok(LanguageSetting::System)
        );
        assert_eq!(
            LanguageSetting::from_value(Some(&json!("ko"))),
            Ok(LanguageSetting::Tag("ko".into()))
        );
        assert!(LanguageSetting::from_value(Some(&json!(3))).is_err());
        assert!(LanguageSetting::from_value(Some(&json!("korean language"))).is_err());
    }

    #[test]
    fn the_setting_wins_over_the_os_and_english_fills_the_gaps() {
        let available = strings(&["en", "ko"]);
        let os = strings(&["en-US"]);
        assert_eq!(
            negotiate(&LanguageSetting::Tag("ko".into()), &os, &available),
            ["ko", "en"]
        );
        assert_eq!(negotiate(&LanguageSetting::System, &os, &available), ["en"]);
    }

    #[test]
    fn system_follows_the_first_os_language_that_matches() {
        let available = strings(&["en", "ko"]);
        assert_eq!(
            negotiate(
                &LanguageSetting::System,
                &strings(&["fr-FR", "ko-KR", "en-US"]),
                &available
            ),
            ["ko", "en"]
        );
        assert_eq!(
            negotiate(&LanguageSetting::System, &strings(&["ko_KR"]), &available),
            ["ko", "en"]
        );
        assert_eq!(
            negotiate(&LanguageSetting::System, &strings(&["ja-JP"]), &available),
            ["en"]
        );
        assert_eq!(negotiate(&LanguageSetting::System, &[], &available), ["en"]);
    }

    #[test]
    fn an_unavailable_setting_falls_back_to_the_os_language() {
        let available = strings(&["en", "ko"]);
        assert_eq!(
            negotiate(
                &LanguageSetting::Tag("ja".into()),
                &strings(&["ko-KR"]),
                &available
            ),
            ["ko", "en"]
        );
    }

    #[test]
    fn matches_by_dropping_subtags_then_by_primary_language() {
        let available = strings(&["en", "zh-Hant", "pt-BR"]);
        assert_eq!(
            negotiate(&LanguageSetting::Tag("zh-Hant-TW".into()), &[], &available),
            ["zh-Hant", "en"]
        );
        assert_eq!(
            negotiate(&LanguageSetting::Tag("pt-PT".into()), &[], &available),
            ["pt-BR", "en"]
        );
        assert_eq!(
            negotiate(
                &LanguageSetting::System,
                &strings(&["en-GB-u-rg-uszzzz"]),
                &available
            ),
            ["en"]
        );
    }
}
