//! 터미널에 띄울 셸과 그 환경(결정 M1-D14, `docs/design/12-m1-plan.md` §4).
//!
//! - 셸: 전역 설정 `[terminal] shell = [argv]` 가 있으면 그것. 없으면 `$SHELL -l`(`$SHELL` 은 런타임 환경의 것, 없으면 사용자
//!   계정의 셸, 그것도 없으면 `/bin/sh`). Windows 는 런타임 PATH 의 `pwsh.exe`, 없으면 `powershell.exe`.
//! - 환경: 런타임 환경(`pw_runtime::RuntimeEnv`) 전체에 [`TERMINAL_VARS`] 를 덮는다. 앱 프로세스의 환경은 물려주지 않는다.

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

/// 자식의 환경 전체.
pub fn child_env(env: &RuntimeEnv) -> Vec<(OsString, OsString)> {
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
    fn terminal_variables_override_the_runtime_environment() {
        let env = RuntimeEnv::from_vars(
            [
                ("PATH", "/shell/bin"),
                ("JAVA_HOME", "/jdk"),
                ("TERM", "dumb"),
                ("TERM_PROGRAM", "iTerm.app"),
                ("TERM_PROGRAM_VERSION", "3.5"),
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
                ("PATH", "/shell/bin"),
                ("TERM", "xterm-256color"),
                ("TERM_PROGRAM", "Pitwall"),
            ]
        );
    }
}
