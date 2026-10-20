//! 터미널에 띄울 셸과 그 환경(결정 M1-D14, `docs/design/12-m1-plan.md` §4).
//!
//! - 셸: 전역 설정 `[terminal] shell = [argv]` 가 있으면 그것. 없으면 `$SHELL -l`(`$SHELL` 은 런타임 환경의 것, 없으면 사용자
//!   계정의 셸, 그것도 없으면 `/bin/sh`). Windows 는 런타임 PATH 의 `pwsh.exe`, 없으면 `powershell.exe`.
//! - 환경: 런타임 환경(`pw_runtime::RuntimeEnv`) 전체에 [`TERMINAL_VARS`] 를 덮는다. 런타임 환경은 보통 로그인 셸의 것이고, 로그인
//!   셸을 읽지 못했으면 앱 프로세스의 환경이다(`pw_runtime` 의 물러남). 그 밖에 앱 프로세스의 환경을 따로 물려주지 않는다.
//! - 로캘: [`LOCALE_VARS`] 가 모두 비어 있으면 `LANG` 을 시스템 로캘의 UTF-8 판으로 넣는다([`default_lang`]). 셸 · 설정이 정한
//!   값은 덮지 않는다. 왜: GUI 앱의 환경(launchd)에는 `LANG` 이 없고, macOS 의 `/etc/zprofile` 만 그것을 채운다 — bash · fish
//!   이거나 로그인 셸을 읽지 못해 물러났으면 셸이 C 로캘로 떠 한글이 깨진다(입력이 `\354\225\210` 처럼 보인다).

use std::ffi::{OsStr, OsString};

use pw_runtime::RuntimeEnv;
use serde_json::Value;

/// 전역 설정(`settings`)에서 셸을 덮는 키.
pub const SETTINGS_KEY: &str = "terminal.shell";

/// 런타임 환경 위에 덮는 변수.
pub const TERMINAL_VARS: [(&str, &str); 3] = [
    ("TERM", "xterm-256color"),
    ("COLORTERM", "truecolor"),
    ("TERM_PROGRAM", "Pitwall"),
];

/// 셸의 문자 집합을 정하는 변수. 하나라도 값이 있으면 `LANG` 을 넣지 않는다.
pub const LOCALE_VARS: [&str; 3] = ["LC_ALL", "LC_CTYPE", "LANG"];

/// 시스템 로캘을 알 수 없을 때의 `LANG`(macOS).
pub const FALLBACK_LANG: &str = "en_US.UTF-8";

/// 런타임 환경에서 빼는 변수. 앱을 터미널에서 띄웠으면 그 터미널의 버전이 남아 `TERM_PROGRAM=Pitwall` 과 어긋난다.
pub const DROPPED_VARS: [&str; 1] = ["TERM_PROGRAM_VERSION"];

/// 설정의 `[terminal] shell`. 없으면 `Ok(None)`. 빈 배열 · 문자열이 아닌 요소 · 빈 첫 요소는 오류(메시지).
pub fn configured(value: Option<&Value>) -> Result<Option<Vec<String>>, String> {
    let Some(value) = value else {
        return Ok(None);
    };
    let invalid = || {
        format!("`{SETTINGS_KEY}` must be a non-empty array of strings like [\"/bin/zsh\", \"-l\"]")
    };
    let items = value.as_array().ok_or_else(invalid)?;
    let argv: Vec<String> = items
        .iter()
        .map(|v| v.as_str().map(str::to_owned))
        .collect::<Option<_>>()
        .ok_or_else(invalid)?;
    match argv.first() {
        Some(program) if !program.is_empty() => Ok(Some(argv)),
        _ => Err(invalid()),
    }
}

/// 설정이 없을 때의 셸.
#[cfg(unix)]
pub fn default_argv(env: &RuntimeEnv) -> Vec<String> {
    let shell = env
        .get("SHELL")
        .map(std::path::PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(pw_runtime::login_shell::user_shell)
        .map_or_else(|| "/bin/sh".to_owned(), |p| p.display().to_string());
    vec![shell, "-l".to_owned()]
}

/// 설정이 없을 때의 셸.
#[cfg(windows)]
pub fn default_argv(env: &RuntimeEnv) -> Vec<String> {
    let pwsh = std::env::split_paths(&env.path)
        .map(|dir| dir.join("pwsh.exe"))
        .find(|p| p.is_file());
    vec![pwsh.map_or_else(|| "powershell.exe".to_owned(), |p| p.display().to_string())]
}

/// 셸의 argv. 설정이 틀렸으면 경고하고 기본 셸을 쓴다.
pub fn argv(setting: Option<&Value>, env: &RuntimeEnv) -> Vec<String> {
    match configured(setting) {
        Ok(Some(argv)) => argv,
        Ok(None) => default_argv(env),
        Err(message) => {
            tracing::warn!(target: "pw_run", "settings: {message}; using the default shell");
            default_argv(env)
        }
    }
}

/// 로캘 변수가 하나도 없을 때 넣을 `LANG`. macOS 는 시스템 설정의 지역(`AppleLocale`, 예: `ko_KR`)의 UTF-8 판이 있으면 그것,
/// 없으면 [`FALLBACK_LANG`]. Linux 등은 어디에나 있는 `C.UTF-8`. Windows 는 넣지 않는다(셸이 로캘 변수를 보지 않는다).
#[cfg(target_os = "macos")]
pub fn default_lang() -> Option<String> {
    static LANG: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    let lang = LANG.get_or_init(|| {
        let apple = std::process::Command::new("/usr/bin/defaults")
            .args(["read", "-g", "AppleLocale"])
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
            .unwrap_or_default();
        let lang = utf8_locale(&apple, |name| {
            std::path::Path::new("/usr/share/locale").join(name).is_dir()
        });
        tracing::debug!(target: "pw_run", %apple, %lang, "terminal LANG when the environment has none");
        lang
    });
    Some(lang.clone())
}

#[cfg(all(unix, not(target_os = "macos")))]
pub fn default_lang() -> Option<String> {
    Some("C.UTF-8".to_owned())
}

#[cfg(windows)]
pub fn default_lang() -> Option<String> {
    None
}

/// `AppleLocale`(`ko_KR`, `zh-Hans_CN`, `en_US@rg=krzzzz`)의 `<언어>_<지역>.UTF-8`. 그런 로캘이 없으면(`available` 이 거짓)
/// [`FALLBACK_LANG`].
pub fn utf8_locale(apple: &str, available: impl Fn(&str) -> bool) -> String {
    let base = apple.split('@').next().unwrap_or_default();
    let candidate = base.split_once('_').and_then(|(language, region)| {
        // 언어 뒤의 문자 체계(`zh-Hans`)는 로캘 이름에 없다.
        let language = language.split('-').next().unwrap_or_default();
        let ok = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric());
        (ok(language) && ok(region)).then(|| format!("{language}_{region}.UTF-8"))
    });
    candidate
        .filter(|name| available(name))
        .unwrap_or_else(|| FALLBACK_LANG.to_owned())
}

/// 자식의 환경 전체.
pub fn child_env(env: &RuntimeEnv) -> Vec<(OsString, OsString)> {
    child_env_with(env, default_lang)
}

fn child_env_with(
    env: &RuntimeEnv,
    lang: impl FnOnce() -> Option<String>,
) -> Vec<(OsString, OsString)> {
    let overridden = |name: &OsStr| {
        TERMINAL_VARS
            .iter()
            .map(|(n, _)| *n)
            .chain(DROPPED_VARS)
            .any(|n| name == n)
    };
    let mut vars: Vec<(OsString, OsString)> = env
        .vars
        .iter()
        .filter(|(name, _)| !overridden(name))
        .map(|(n, v)| (n.clone(), v.clone()))
        .collect();
    if !env.path.is_empty() {
        vars.push(("PATH".into(), env.path.clone()));
    }
    vars.extend(
        TERMINAL_VARS
            .iter()
            .map(|(n, v)| ((*n).into(), (*v).into())),
    );
    // 빈 값은 없는 것과 같다(로캘이 C 로 떨어진다).
    let has_locale = vars
        .iter()
        .any(|(n, v)| !v.is_empty() && LOCALE_VARS.iter().any(|l| n == l));
    if !has_locale && let Some(lang) = lang() {
        vars.retain(|(n, _)| n != "LANG");
        vars.push(("LANG".into(), lang.into()));
    }
    vars
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn the_setting_must_be_a_non_empty_argv() {
        assert_eq!(configured(None), Ok(None));
        assert_eq!(
            configured(Some(&json!(["/bin/zsh", "-l"]))),
            Ok(Some(vec!["/bin/zsh".to_owned(), "-l".to_owned()]))
        );
        for bad in [
            json!("/bin/zsh -l"),
            json!([]),
            json!([""]),
            json!(["/bin/zsh", 1]),
        ] {
            assert!(configured(Some(&bad)).is_err(), "{bad}");
        }
    }

    #[test]
    fn an_invalid_setting_falls_back_to_the_default_shell() {
        let env = RuntimeEnv::default();
        assert_eq!(argv(Some(&json!("zsh")), &env), default_argv(&env));
        assert_eq!(argv(Some(&json!(["fish"])), &env), ["fish"]);
    }

    #[cfg(unix)]
    #[test]
    fn the_default_shell_is_the_runtime_shell_as_a_login_shell() {
        let env = RuntimeEnv::from_vars([("SHELL".into(), "/opt/bin/fish".into())]);
        assert_eq!(default_argv(&env), ["/opt/bin/fish", "-l"]);
        // 상대 경로는 믿지 않고 계정의 셸로 간다.
        let relative = RuntimeEnv::from_vars([("SHELL".into(), "fish".into())]);
        assert_ne!(default_argv(&relative)[0], "fish");
        assert_eq!(default_argv(&relative)[1], "-l");
    }

    #[test]
    fn lang_is_filled_only_when_no_locale_variable_is_set() {
        let lang = || Some("ko_KR.UTF-8".to_owned());
        let get = |vars: &[(OsString, OsString)], name: &str| {
            vars.iter()
                .find(|(n, _)| n == name)
                .map(|(_, v)| v.to_str().unwrap().to_owned())
        };
        let none = child_env_with(&RuntimeEnv::default(), lang);
        assert_eq!(get(&none, "LANG").as_deref(), Some("ko_KR.UTF-8"));
        let empty = child_env_with(&RuntimeEnv::from_vars([("LANG".into(), "".into())]), lang);
        assert_eq!(get(&empty, "LANG").as_deref(), Some("ko_KR.UTF-8"));
        assert_eq!(empty.iter().filter(|(n, _)| n == "LANG").count(), 1);
        for (name, value) in [
            ("LANG", "fr_FR.UTF-8"),
            ("LC_ALL", "C"),
            ("LC_CTYPE", "UTF-8"),
        ] {
            let vars = child_env_with(&RuntimeEnv::from_vars([(name.into(), value.into())]), lang);
            assert_eq!(get(&vars, name).as_deref(), Some(value), "{name}");
            if name != "LANG" {
                assert_eq!(get(&vars, "LANG"), None, "{name}");
            }
        }
    }

    #[test]
    fn the_system_locale_becomes_its_utf8_variant() {
        let have = |name: &str| ["ko_KR.UTF-8", "zh_CN.UTF-8", "en_US.UTF-8"].contains(&name);
        assert_eq!(utf8_locale("ko_KR", have), "ko_KR.UTF-8");
        assert_eq!(utf8_locale("zh-Hans_CN", have), "zh_CN.UTF-8");
        assert_eq!(utf8_locale("ko_KR@rg=krzzzz", have), "ko_KR.UTF-8");
        // 없는 조합 · 읽지 못함 · 이상한 값은 en_US.UTF-8.
        assert_eq!(utf8_locale("en_KR", have), FALLBACK_LANG);
        assert_eq!(utf8_locale("", have), FALLBACK_LANG);
        assert_eq!(utf8_locale("ko", have), FALLBACK_LANG);
        assert_eq!(utf8_locale("../x_y", |_| true), FALLBACK_LANG);
    }

    #[test]
    fn terminal_variables_override_the_runtime_environment() {
        let env = RuntimeEnv::from_vars(
            [
                ("PATH", "/shell/bin"),
                ("JAVA_HOME", "/jdk"),
                ("TERM", "dumb"),
                ("TERM_PROGRAM", "iTerm.app"),
                ("TERM_PROGRAM_VERSION", "3.5"),
                ("LANG", "ko_KR.UTF-8"),
            ]
            .map(|(k, v)| (OsString::from(k), OsString::from(v))),
        );
        let mut vars = child_env(&env);
        vars.sort();
        let vars: Vec<(&str, &str)> = vars
            .iter()
            .map(|(k, v)| (k.to_str().unwrap(), v.to_str().unwrap()))
            .collect();
        assert_eq!(
            vars,
            [
                ("COLORTERM", "truecolor"),
                ("JAVA_HOME", "/jdk"),
                ("LANG", "ko_KR.UTF-8"),
                ("PATH", "/shell/bin"),
                ("TERM", "xterm-256color"),
                ("TERM_PROGRAM", "Pitwall"),
            ]
        );
    }
}
