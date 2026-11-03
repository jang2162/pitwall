//! 덮어쓰기 정의 `crates/pw-tools/overrides/<패키지>.toml` (15-m4-plan.md M4-03 ⑦).
//!
//! Pitwall 이 registry 정의 위에 받을 판 · 주소 · `bin` 을 덮어 고정하는 틀이다. 덮어쓰기가 있는 패키지는 [`Package::resolve`]
//! 에서 이 값이 registry 의 것을 이긴다(판은 바꾸고, `download` 는 registry 의 `asset` · `download` 를 통째로 바꾸고, `bin` ·
//! `share` 는 키마다 덮는다). 내용(jdtls 의 정식 마일스톤 · 판 고정 lombok)은 쓰는 행이 더한다(M4-18 — 결정 M4-D4).
//!
//! 형식(모르는 키는 오류 — 우리 파일이라 조용히 버리지 않는다):
//!
//! ```toml
//! reason = "registry 의 스냅숏 주소는 서버에서 지워진다"   # 필수 — 왜 덮는가
//! version = "v1.63.0"                                     # 받을 판(purl 의 판 대신). 표현식의 `{{version}}` 도 이 값이다
//!
//! [[download]]                                            # registry 의 `source.asset` · `source.download` 대신
//! target = ["darwin_arm64", "darwin_x64"]                 # Mason 대상 이름(없으면 모든 대상)
//! files = { "jdtls.tar.gz" = "https://download.eclipse.org/jdtls/milestones/{{ version | strip_prefix \"v\" }}/…" }
//! config = "config_mac/"                                  # 그 밖의 키는 표현식 `{{source.download.<키>}}` 의 값
//!
//! [bin]
//! jdtls = "bin/jdtls"
//!
//! [share]
//! "jdtls/lombok.jar" = "lombok.jar"
//!
//! [bin_alias]                                             # 새 이름 → 이미 있는 `bin` 이름(대상별로 풀린 경로를 따른다)
//! kotlin-lsp = "intellij-server"
//!
//! [launcher."pitwall/jdtls"]                              # 설치기가 풀기 뒤 설치 폴더에 쓰는 런처(설치 폴더 기준 경로)
//! sh = "#!/bin/sh\n…"                                     # 유닉스: 그 경로에 0755 로
//! cmd = "@echo off\r\n…"                                 # Windows: `<경로>.cmd` 로 — 그 경로를 가리키는 `bin` 도 `.cmd` 를 쓴다
//! ```
//!
//! 런처는 registry 정의의 실행 방법을 쓸 수 없을 때(jdtls 의 python 런처 — 결정 M4-D4) Pitwall 이 둔다. 받은 보관 파일에 같은
//! 경로가 있으면 설치가 실패한다(받은 배포물을 덮어쓰지 않는다).
//!
//! `reason` 만 쓰고 `version` · `download` 가 없는 덮어쓰기(별칭만 등)는 받을 것을 바꾸지 않으므로 고정 해시도 그대로다.
//!
//! [`Package::resolve`]: crate::package::Package::resolve

use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::{Map, Value};

mod generated {
    include!(concat!(env!("OUT_DIR"), "/overrides.rs"));
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Override {
    pub reason: String,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub download: Option<Vec<Map<String, Value>>>,
    #[serde(default)]
    pub bin: BTreeMap<String, String>,
    #[serde(default)]
    pub share: BTreeMap<String, String>,
    /// 새 실행 파일 이름 → 이미 있는(registry 또는 위 `bin` 의) 실행 파일 이름. 언어 확장의 `command` 가 registry 의 `bin`
    /// 이름과 다를 때 쓴다(kotlin-lsp: `kotlin-lsp` → `intellij-server` — 메인 결정, 리뷰 X1). 대상마다 다른 경로를 다시 적지
    /// 않고 그 대상에서 풀린 경로를 그대로 따른다.
    #[serde(default)]
    pub bin_alias: BTreeMap<String, String>,
    /// 설치 폴더 기준 경로 → 런처 내용. 설치기가 풀기를 마친 뒤 쓴다([`Launcher`]).
    #[serde(default)]
    pub launcher: BTreeMap<String, Launcher>,
}

/// 설치 폴더에 쓰는 런처 한 벌(15-m4-plan.md M4-18 — jdtls 를 python 없이 `java -jar` 로 띄운다).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Launcher {
    /// 유닉스 셸 스크립트(그 경로에 0755).
    pub sh: String,
    /// Windows 배치 파일(`<경로>.cmd`).
    pub cmd: String,
}

/// 런처 경로가 설치 폴더 안의 평범한 상대 경로인가(`/` 구분, 빈 · `.` · `..` 마디 없음).
fn valid_launcher_path(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path.contains('\\')
        && !path.contains(':')
        && path
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}

impl Override {
    /// TOML 글을 읽는다.
    pub fn parse(text: &str) -> Result<Override, String> {
        let doc: toml_edit::DocumentMut = text.parse().map_err(|e| format!("{e}"))?;
        let json = Value::Object(pw_core::config::merge::to_json(doc.as_table()));
        let over: Override = serde_json::from_value(json).map_err(|e| e.to_string())?;
        if over.reason.trim().is_empty() {
            return Err("`reason` is empty".into());
        }
        if let Some(entries) = &over.download {
            if entries.is_empty() {
                return Err("`download` is empty".into());
            }
            for entry in entries {
                if !entry.get("files").is_some_and(Value::is_object) {
                    return Err("a `download` entry has no `files` table".into());
                }
            }
        }
        for (path, launcher) in &over.launcher {
            if !valid_launcher_path(path) {
                return Err(format!(
                    "launcher path `{path}` is not a plain relative path"
                ));
            }
            if launcher.sh.trim().is_empty() || launcher.cmd.trim().is_empty() {
                return Err(format!("launcher `{path}` has an empty script"));
            }
        }
        Ok(over)
    }

    /// `download` 를 registry 의 `source.download` 와 같은 꼴(목록)로.
    pub(crate) fn download_json(&self) -> Option<Value> {
        self.download
            .as_ref()
            .map(|entries| Value::Array(entries.iter().cloned().map(Value::Object).collect()))
    }
}

/// 패키지 이름 → 덮어쓰기.
#[derive(Debug, Clone, Default)]
pub struct Overrides {
    by_name: BTreeMap<String, Override>,
}

impl Overrides {
    /// 바이너리에 묶인 `overrides/*.toml`. 읽지 못한 파일은 경고하고 뺀다(시험 `builtin_overrides_all_parse` 가 막는다).
    pub fn builtin() -> Overrides {
        let (overrides, errors) = Overrides::from_files(generated::FILES);
        for (name, e) in errors {
            tracing::warn!(target: "pw_tools", "override {name}.toml: {e}");
        }
        overrides
    }

    /// `(패키지 이름, TOML 글)` 들. 읽지 못한 것은 `(이름, 오류)` 로 돌려준다.
    pub fn from_files(files: &[(&str, &str)]) -> (Overrides, Vec<(String, String)>) {
        let mut by_name = BTreeMap::new();
        let mut errors = Vec::new();
        for (name, text) in files {
            match Override::parse(text) {
                Ok(o) => {
                    by_name.insert((*name).to_owned(), o);
                }
                Err(e) => errors.push(((*name).to_owned(), e)),
            }
        }
        (Overrides { by_name }, errors)
    }

    pub fn get(&self, name: &str) -> Option<&Override> {
        self.by_name.get(name)
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.by_name.keys().map(String::as_str)
    }
}

/// 묶인 덮어쓰기 파일(시험 · 검사용).
#[cfg(test)]
pub(crate) fn builtin_files() -> &'static [(&'static str, &'static str)] {
    generated::FILES
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_overrides_all_parse() {
        let (_, errors) = Overrides::from_files(builtin_files());
        assert!(errors.is_empty(), "{errors:?}");
    }

    #[test]
    fn parse_rejects_unknown_keys_and_missing_reason() {
        assert!(Override::parse("reason = \"x\"\nversion = \"1\"\n").is_ok());
        assert!(Override::parse("version = \"1\"\n").is_err());
        assert!(Override::parse("reason = \" \"\n").is_err());
        assert!(Override::parse("reason = \"x\"\nurl = \"https://a\"\n").is_err());
        assert!(Override::parse("reason = \"x\"\n[[download]]\ntarget = \"linux\"\n").is_err());
        assert!(Override::parse("reason = \"x\"\ndownload = []\n").is_err());
        assert!(Override::parse("reason = \"x\"\n[bin]\na = 1\n").is_err());
        assert!(Override::parse("reason = \"x\"\n[bin_alias]\na = \"b\"\n").is_ok());
        assert!(Override::parse("reason = \"x\"\n[bin_alias]\na = 1\n").is_err());
        let launcher = |path: &str, body: &str| {
            Override::parse(&format!("reason = \"x\"\n[launcher.\"{path}\"]\n{body}\n"))
        };
        assert!(launcher("pitwall/jdtls", "sh = \"#!/bin/sh\"\ncmd = \"@echo off\"").is_ok());
        assert!(launcher("pitwall/jdtls", "sh = \"#!/bin/sh\"").is_err());
        assert!(launcher("pitwall/jdtls", "sh = \" \"\ncmd = \"@echo off\"").is_err());
        assert!(
            launcher("pitwall/jdtls", "sh = \"a\"\ncmd = \"b\"\nps1 = \"c\"").is_err(),
            "unknown keys"
        );
        for bad in ["../jdtls", "/abs", "a//b", "./a", "a\\\\b", "c:x"] {
            assert!(
                launcher(bad, "sh = \"a\"\ncmd = \"b\"").is_err(),
                "{bad} must be refused"
            );
        }
    }

    fn jdtls_cmd() -> String {
        let (overrides, _) = Overrides::from_files(builtin_files());
        overrides.get("jdtls").unwrap().launcher["pitwall/jdtls"]
            .cmd
            .clone()
    }

    /// Windows 프로그램이 명령줄을 인자로 나누는 대로(따옴표 밖의 공백에서) 나누고 따옴표를 걷는다. `""` 는 빈 인자 하나다.
    fn split_command_line(line: &str) -> Vec<String> {
        let (mut args, mut current, mut quoted, mut started) =
            (Vec::new(), String::new(), false, false);
        for c in line.chars() {
            match c {
                '"' => {
                    quoted = !quoted;
                    started = true;
                }
                ' ' | '\t' if !quoted => {
                    if started {
                        args.push(std::mem::take(&mut current));
                        started = false;
                    }
                }
                _ => {
                    current.push(c);
                    started = true;
                }
            }
        }
        if started {
            args.push(current);
        }
        args
    }

    /// `set "NAME=VALUE"` 은 따옴표를 걷고, `set NAME=VALUE` 는 따옴표까지 값이다(cmd 의 규칙).
    fn set_value(statement: &str) -> String {
        let assignment = statement.trim();
        let assignment = assignment
            .strip_prefix('"')
            .and_then(|a| a.strip_suffix('"'))
            .unwrap_or(assignment);
        assignment.split_once('=').unwrap().1.to_owned()
    }

    /// jdtls 의 cmd 런처가 설치 폴더 경로(사용자 폴더 이름에 공백이 있을 수 있다 — `C:\Users\Jane Doe`)를 javaagent 인자 하나로
    /// 넘기고, lombok 이 없으면 빈 인자를 넘기지 않는다(리뷰 M2). cmd 를 돌릴 수 없는 곳에서도 보도록 `%변수%` 를 펼쳐 나눈다 —
    /// 실제 cmd 실행은 아래 Windows 시험.
    #[test]
    fn the_jdtls_cmd_launcher_passes_a_spaced_lombok_path_as_one_argument() {
        let cmd = jdtls_cmd();
        let base = r"C:\Users\Jane Doe\AppData\Local\pitwall\tools\jdtls\v1.63.0\pitwall\..";
        let agent_line = cmd
            .lines()
            .find_map(|l| {
                l.trim()
                    .strip_prefix(r#"if exist "%BASE%\lombok.jar" set "#)
            })
            .expect("the AGENT line");
        let agent = set_value(agent_line).replace("%BASE%", base);
        let java_line = cmd
            .lines()
            .find(|l| l.starts_with(r#""%JAVA%""#))
            .expect("the java line");
        let expand = |agent: &str| {
            java_line
                .replace("%JAVA%", r"C:\Program Files\Java\jdk-25\bin\java.exe")
                .replace("%BASE%", base)
                .replace("%LAUNCHER%", &format!(r"{base}\plugins\launcher.jar"))
                .replace("%DATA%", r"C:\Users\Jane Doe\AppData\Local\jdtls\pitwall-x")
                .replace("%AGENT%", agent)
                .replace("%*", "")
        };

        let args = split_command_line(&expand(&agent));
        let want = format!(r"-javaagent:{base}\lombok.jar");
        assert!(args.contains(&want), "{args:#?}");
        let jar = args.iter().position(|a| a == "-jar").unwrap();
        assert_eq!(args[jar + 1], format!(r"{base}\plugins\launcher.jar"));
        // lombok 이 없으면 AGENT 는 정의되지 않아 `%AGENT%` 는 빈 글이 된다.
        let args = split_command_line(&expand(""));
        assert!(!args.iter().any(String::is_empty), "{args:#?}");
        assert!(
            !args.iter().any(|a| a.starts_with("-javaagent")),
            "{args:#?}"
        );
    }

    /// 위 시험을 실제 cmd 로 — 공백이 든 설치 폴더에서 런처를 띄우고, PATH 의 가짜 `java.cmd` 가 받은 인자를 한 줄씩 적는다.
    #[cfg(windows)]
    #[test]
    fn the_jdtls_cmd_launcher_runs_from_a_spaced_install_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().join("Jane Doe").join("jdtls");
        std::fs::create_dir_all(base.join("pitwall")).unwrap();
        std::fs::create_dir_all(base.join("plugins")).unwrap();
        std::fs::write(
            base.join("plugins")
                .join("org.eclipse.equinox.launcher_1.0.0.jar"),
            "",
        )
        .unwrap();
        std::fs::write(base.join("lombok.jar"), "").unwrap();
        let script = base.join("pitwall").join("jdtls.cmd");
        std::fs::write(&script, jdtls_cmd()).unwrap();
        let fake = tmp.path().join("fake");
        std::fs::create_dir_all(&fake).unwrap();
        std::fs::write(
            fake.join("java.cmd"),
            "@echo off\r\n:next\r\nif \"%~1\"==\"\" exit /b 0\r\n>>\"%PW_ARGS%\" echo(%~1\r\nshift\r\ngoto next\r\n",
        )
        .unwrap();
        let record = tmp.path().join("args.txt");
        let system = std::env::var_os("SystemRoot").map_or_else(
            || std::path::PathBuf::from(r"C:\Windows\System32"),
            |root| std::path::PathBuf::from(root).join("System32"),
        );
        let output = std::process::Command::new(&script)
            .current_dir(tmp.path())
            .env_remove("JAVA_HOME")
            .env(
                "PATH",
                std::env::join_paths([fake.clone(), system]).unwrap(),
            )
            .env("JDTLS_DATA", tmp.path().join("data"))
            .env("PW_ARGS", &record)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        let args = std::fs::read_to_string(&record).unwrap();
        let agents: Vec<&str> = args
            .lines()
            .filter_map(|l| l.strip_prefix("-javaagent:"))
            .collect();
        assert_eq!(agents.len(), 1, "{args}");
        assert_eq!(
            std::fs::canonicalize(agents[0]).ok(),
            std::fs::canonicalize(base.join("lombok.jar")).ok(),
            "{args}"
        );
    }
}
