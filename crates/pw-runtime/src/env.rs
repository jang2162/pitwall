//! 앱이 띄우는 프로세스에 줄 환경(`RuntimeEnv`, 03 §5-1 「결과는 `RuntimeEnv { path, vars, tools }` 로 통일」).

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};

use crate::login_shell::RESOLVING_ENV;

/// 자식에게 넘기지 않는 변수. `PWD` 는 셸이 돈 폴더(자식의 cwd 와 다르다), `SHLVL` 은 셸의 깊이, `_` 는 셸이 마지막에 돌린
/// 명령이다 — 셸이 스스로 정하는 값이라 사용자의 환경이 아니다. [`RESOLVING_ENV`] 는 Pitwall 이 셸에 준 표식이다.
pub const EXCLUDED: [&str; 4] = ["PWD", "SHLVL", "_", RESOLVING_ENV];

/// 자식 프로세스의 환경 전체. 자식은 앱 프로세스의 환경을 물려받지 않고 이것만 받는다.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RuntimeEnv {
    /// PATH.
    pub path: OsString,
    /// PATH 를 뺀 나머지 변수.
    pub vars: BTreeMap<OsString, OsString>,
    /// 제공자가 정한 도구와 그 버전. 로그인 셸은 비어 있다.
    pub tools: Vec<RuntimeTool>,
}

/// 제공자가 고정한 도구(`node 22.11.0`, `mise.toml` 에서).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeTool {
    pub name: String,
    pub version: String,
    /// 버전을 정한 파일.
    pub source_file: Option<OsString>,
}

impl RuntimeEnv {
    /// 변수 목록으로. PATH 는 [`Self::path`] 로 가르고 [`EXCLUDED`] 는 버린다.
    pub fn from_vars(vars: impl IntoIterator<Item = (OsString, OsString)>) -> Self {
        let mut env = Self::default();
        for (name, value) in vars {
            if same_name(&name, "PATH") {
                env.path = value;
            } else if !EXCLUDED.iter().any(|x| same_name(&name, x)) {
                env.vars.insert(name, value);
            }
        }
        env
    }

    /// 이 프로세스의 환경. 로그인 셸을 읽지 못했을 때 물러나는 자리다.
    pub fn process() -> Self {
        Self::from_vars(std::env::vars_os())
    }

    /// 변수 하나. `PATH` 도 받는다.
    pub fn get(&self, name: &str) -> Option<&OsStr> {
        if same_name(OsStr::new(name), "PATH") {
            return Some(&self.path);
        }
        self.vars.get(OsStr::new(name)).map(OsString::as_os_str)
    }
}

/// Windows 의 환경 변수 이름은 대소문자를 가리지 않는다(`Path`).
fn same_name(name: &OsStr, wanted: &str) -> bool {
    if cfg!(windows) {
        name.eq_ignore_ascii_case(wanted)
    } else {
        name == wanted
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_path_and_drops_shell_variables() {
        let env = RuntimeEnv::from_vars(
            [
                ("PATH", "/a:/b"),
                ("JAVA_HOME", "/jdk"),
                ("PWD", "/somewhere"),
                ("SHLVL", "2"),
                ("_", "/usr/bin/env"),
                ("PITWALL_RESOLVING_ENVIRONMENT", "1"),
                ("OLDPWD", "/before"),
            ]
            .map(|(k, v)| (OsString::from(k), OsString::from(v))),
        );
        assert_eq!(env.path, OsString::from("/a:/b"));
        assert_eq!(env.get("PATH"), Some(OsStr::new("/a:/b")));
        assert_eq!(env.get("JAVA_HOME"), Some(OsStr::new("/jdk")));
        let names: Vec<_> = env.vars.keys().map(|k| k.to_str().unwrap()).collect();
        assert_eq!(names, ["JAVA_HOME", "OLDPWD"]);
    }
}
