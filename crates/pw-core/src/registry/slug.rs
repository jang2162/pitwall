//! 개인 전용 설정 폴더 이름(`$CONFIG/projects/<slug>/`). 규칙은 `docs/design/03-architecture.md` 「5. 데이터 저장」 —
//! 경로 한 칸, `[A-Za-z0-9._-]`, `.` 으로 시작하지 않음 — 이고 [`crate::config::ConfigSource::personal`] 이 검사한다.
//!
//! 여기서 더 좁힌다.
//! - **소문자만** 쓴다. 대소문자를 무시하는 파일 시스템(macOS 기본, Windows)에서는 `App` 과 `app` 이 같은 폴더라
//!   레지스트리에서는 달라도 디스크에서 겹친다.
//! - 끝의 `.` 를 뗀다(Windows 가 폴더 이름 끝의 `.` 을 버린다). Windows 장치 이름(`con`, `nul` …)은 `_` 를 붙여 피한다.
//! - 이름에서 아무것도 남지 않으면(한글만 있는 이름 등) `project`.
//!
//! slug 는 만들 때 한 번 정하고 이름을 바꿔도 그대로 둔다 — 바꾸면 개인 전용 설정 폴더를 옮겨야 한다.

/// slug 의 최대 길이(바이트). 번호 접미사(`-2`)를 붙이기 전의 길이다.
const MAX_LEN: usize = 48;
const FALLBACK: &str = "project";
const WINDOWS_DEVICES: &[&str] = &[
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// `name` 에서 slug 의 바탕을 만든다. 겹침은 보지 않는다.
pub fn base(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        let c = c.to_ascii_lowercase();
        if c.is_ascii_alphanumeric() || matches!(c, '.' | '_') {
            out.push(c);
        } else if !out.ends_with('-') {
            // 금지 문자 · 공백 · 연속된 `-` 는 `-` 하나로 줄인다.
            out.push('-');
        }
    }
    let trimmed = out.trim_start_matches(['.', '-']);
    let mut slug: String = trimmed.chars().take(MAX_LEN).collect();
    while slug.ends_with(['.', '-']) {
        slug.pop();
    }
    if slug.is_empty() {
        return FALLBACK.to_owned();
    }
    let stem = slug.split('.').next().unwrap_or_default();
    if WINDOWS_DEVICES.contains(&stem) {
        slug.insert(stem.len(), '_');
    }
    slug
}

/// `name` 에서 만든 slug 중 `taken` 이 아닌 첫 것. 겹치면 `-2`, `-3` … 을 붙인다.
pub fn unique(name: &str, mut taken: impl FnMut(&str) -> bool) -> String {
    let base = base(name);
    if !taken(&base) {
        return base;
    }
    (2u32..)
        .map(|n| format!("{base}-{n}"))
        .find(|candidate| !taken(candidate))
        .expect("u32 접미사가 바닥나기 전에 빈 자리가 있다")
}

#[cfg(test)]
mod tests {
    use crate::config::{AppPaths, ConfigSource};

    use super::*;

    #[test]
    fn forbidden_characters_collapse_to_single_dashes() {
        assert_eq!(base("My App"), "my-app");
        assert_eq!(base("a/b\\c:d"), "a-b-c-d");
        assert_eq!(base("  spaced   out  "), "spaced-out");
        assert_eq!(base("v1.2_beta"), "v1.2_beta");
    }

    #[test]
    fn leading_dots_trailing_dots_and_empty_names() {
        assert_eq!(base(".dotfiles"), "dotfiles");
        assert_eq!(base("...hidden..."), "hidden");
        assert_eq!(base("한글이름"), "project");
        assert_eq!(base(""), "project");
        assert_eq!(base("--"), "project");
        assert_eq!(base("웹 app"), "app");
    }

    #[test]
    fn windows_device_names_are_avoided() {
        assert_eq!(base("CON"), "con_");
        assert_eq!(base("nul.d"), "nul_.d");
        assert_eq!(base("console"), "console");
    }

    #[test]
    fn long_names_are_cut() {
        let slug = base(&"x".repeat(200));
        assert_eq!(slug.len(), MAX_LEN);
    }

    #[test]
    fn collisions_get_numbered_suffixes() {
        let taken = ["app", "app-2"];
        assert_eq!(unique("App", |s| taken.contains(&s)), "app-3");
        assert_eq!(unique("Other", |s| taken.contains(&s)), "other");
    }

    /// 만든 slug 는 모두 설정 계층(M0-08)이 받는 slug 다.
    #[test]
    fn every_generated_slug_is_accepted_by_config_source() {
        let paths = AppPaths::under(std::path::Path::new("/nonexistent-pitwall-root"));
        for name in [
            "My App",
            ".dotfiles",
            "한글",
            "CON",
            "a/b",
            "x.",
            "-lead",
            "...",
            "Ünïcödé",
            "tab\there",
        ] {
            let slug = unique(name, |_| false);
            assert!(
                ConfigSource::personal(&paths, &slug).is_ok(),
                "`{name}` → `{slug}`"
            );
        }
    }
}
