//! 원격 호스트 설정 `[[remote_hosts]]` 의 읽기(18-m7-plan M7-06 ⑥ — 메인 판정 M7-P16, 꼴은 08-remote-mobile.md 「원격 호스트 등록
//! 예」). **전역 설정(`settings.toml`)에서만 읽는다** — `ssh_args` 는 ssh 옵션(`ProxyCommand` 등)이라 명령 실행에 닿고, 클론한
//! 저장소의 커밋된 설정이 그것을 넣으면 안 된다(DB 의 「설정 파일은 ssh 옵션을 줄 수 없다」 — D47 과 같은 방향). 프로젝트 설정의 그
//! 표는 효과가 없고 경고를 남긴다.
//!
//! 키: `nickname` · `host`(필수 — `~/.ssh/config` 의 호스트 이름 또는 호스트 이름 · 주소) · `username` · `port` · `ssh_args`,
//! 그리고 `[[remote_hosts.port_forwards]]`(18-m7-plan M7-22 — `local_port` · `remote_port` 필수, `remote_host` · `local_host`). 틀린
//! 항목은 빼고 경고한다(다른 항목은 쓴다) — 틀린 포워딩 하나는 그 포워딩만 뺀다. `local_host` 가 루프백인지는 여기서 보지 않는다:
//! 항목은 남기고 열 때 열지 않은 까닭을 목록에 보인다(결정 M7-D12 — `crate::forward::local_addr`).

use serde_json::Value;

use crate::forward::ForwardSetting;
use crate::ssh::{SshTarget, check_options};

/// 설정의 키(`settings.toml` 의 `[[remote_hosts]]`).
pub const KEY: &str = "remote_hosts";

/// 원격 호스트 항목 하나.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteHostSettings {
    /// 사람이 붙인 이름(없으면 호스트 이름).
    pub nickname: Option<String>,
    pub target: SshTarget,
    /// 사람의 ssh 옵션([`check_options`] 를 지난 것).
    pub ssh_args: Vec<String>,
    /// 붙을 때 여는 포트 포워딩(18-m7-plan M7-22 ①).
    pub port_forwards: Vec<ForwardSetting>,
}

impl RemoteHostSettings {
    /// 목록에 쓰는 이름(별명, 없으면 호스트 이름).
    pub fn name(&self) -> &str {
        self.nickname.as_deref().unwrap_or(&self.target.host)
    }
}

/// 레이어별 `remote_hosts` 값에서 항목을 읽는다. `global` 은 전역 `settings.toml` 의 값, `project` 는 프로젝트 설정의 값(읽지 않고
/// 있으면 경고만). 경고는 사람이 읽을 문장이다.
pub fn read(
    global: Option<&Value>,
    project: Option<&Value>,
) -> (Vec<RemoteHostSettings>, Vec<String>) {
    let mut warnings: Vec<String> = project_table_warning(project).into_iter().collect();
    let mut hosts = Vec::new();
    let items = match global {
        None => return (hosts, warnings),
        Some(Value::Array(items)) => items,
        Some(other) => {
            warnings.push(format!(
                "remote_hosts must be an array of tables ([[remote_hosts]]), found {}",
                kind(other)
            ));
            return (hosts, warnings);
        }
    };
    for (i, item) in items.iter().enumerate() {
        match entry(item) {
            Ok(mut host) => {
                host.port_forwards = forwards(item, i, &mut warnings);
                hosts.push(host);
            }
            Err(why) => warnings.push(format!("remote_hosts[{i}] is ignored: {why}")),
        }
    }
    (hosts, warnings)
}

/// 프로젝트 설정에 `remote_hosts` 가 있으면 그것이 효과가 없다는 경고.
pub fn project_table_warning(project: Option<&Value>) -> Option<String> {
    project.map(|_| {
        "remote_hosts in the project settings has no effect; remote hosts are read only from the global settings \
         (settings.toml), because ssh_args reach the ssh command line"
            .to_owned()
    })
}

fn kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "nothing",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "a table",
    }
}

/// 항목의 `port_forwards`. 틀린 것은 빼고 경고한다.
fn forwards(item: &Value, i: usize, warnings: &mut Vec<String>) -> Vec<ForwardSetting> {
    let items = match item.get("port_forwards") {
        None => return Vec::new(),
        Some(Value::Array(items)) => items,
        Some(other) => {
            warnings.push(format!(
                "remote_hosts[{i}].port_forwards must be an array of tables ([[remote_hosts.port_forwards]]), found {}",
                kind(other)
            ));
            return Vec::new();
        }
    };
    let mut out = Vec::new();
    for (j, forward) in items.iter().enumerate() {
        match port_forward(forward) {
            Ok(forward) => out.push(forward),
            Err(why) => warnings.push(format!(
                "remote_hosts[{i}].port_forwards[{j}] is ignored: {why}"
            )),
        }
    }
    out
}

fn port_forward(item: &Value) -> Result<ForwardSetting, String> {
    let Value::Object(table) = item else {
        return Err(format!(
            "a port forward must be a table, found {}",
            kind(item)
        ));
    };
    let port = |key: &str| -> Result<u16, String> {
        match table.get(key) {
            None => Err(format!("{key} is missing")),
            Some(Value::Number(n)) => n
                .as_u64()
                .and_then(|p| u16::try_from(p).ok())
                .filter(|p| *p > 0)
                .ok_or_else(|| format!("{key} must be 1 to 65535")),
            Some(other) => Err(format!("{key} must be a number, found {}", kind(other))),
        }
    };
    let text = |key: &str| -> Result<Option<String>, String> {
        match table.get(key) {
            None => Ok(None),
            Some(Value::String(s)) => Ok(Some(s.clone())),
            Some(other) => Err(format!("{key} must be a string, found {}", kind(other))),
        }
    };
    let setting = ForwardSetting {
        local_port: port("local_port")?,
        remote_port: port("remote_port")?,
        remote_host: text("remote_host")?,
        local_host: text("local_host")?,
    };
    if let Some(host) = &setting.remote_host {
        crate::forward::ForwardTo::new(host.clone(), setting.remote_port).check()?;
    }
    Ok(setting)
}

fn entry(item: &Value) -> Result<RemoteHostSettings, String> {
    let Value::Object(table) = item else {
        return Err(format!("an entry must be a table, found {}", kind(item)));
    };
    let text = |key: &str| -> Result<Option<String>, String> {
        match table.get(key) {
            None => Ok(None),
            Some(Value::String(s)) => Ok(Some(s.clone())),
            Some(other) => Err(format!("{key} must be a string, found {}", kind(other))),
        }
    };
    let host = text("host")?.ok_or("host is missing")?;
    let port = match table.get("port") {
        None => None,
        Some(Value::Number(n)) => Some(
            n.as_u64()
                .and_then(|p| u16::try_from(p).ok())
                .filter(|p| *p > 0)
                .ok_or("port must be 1 to 65535")?,
        ),
        Some(other) => return Err(format!("port must be a number, found {}", kind(other))),
    };
    let ssh_args = match table.get("ssh_args") {
        None => Vec::new(),
        Some(Value::Array(args)) => args
            .iter()
            .map(|a| a.as_str().map(str::to_owned))
            .collect::<Option<Vec<String>>>()
            .ok_or("ssh_args must be an array of strings")?,
        Some(other) => {
            return Err(format!(
                "ssh_args must be an array of strings, found {}",
                kind(other)
            ));
        }
    };
    check_options(&ssh_args)?;
    let target = SshTarget {
        host,
        user: text("username")?,
        port,
    };
    target.check().map_err(|e| e.to_string())?;
    Ok(RemoteHostSettings {
        nickname: text("nickname")?,
        target,
        ssh_args,
        port_forwards: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use pw_core::config::ConfigSource;

    use super::*;

    /// 전역 설정의 항목은 읽고, 프로젝트 설정의 `[[remote_hosts]]` 는 읽지 않고 경고한다(메인 판정 M7-P16). 프로젝트 쪽 항목을
    /// 합치면 이 시험이 실패한다.
    #[test]
    fn remote_hosts_come_from_the_global_settings_only() {
        let dir = tempfile::tempdir().unwrap();
        let global = dir.path().join("config");
        let project = dir.path().join("repo");
        std::fs::create_dir_all(&global).unwrap();
        std::fs::create_dir_all(project.join(".pitwall")).unwrap();
        std::fs::write(
            global.join("settings.toml"),
            "schema = 1\n[[remote_hosts]]\nnickname = \"build\"\nhost = \"build.ts.net\"\nusername = \"dev\"\nport = 2222\n\
             ssh_args = [\"-o\", \"ServerAliveInterval=30\"]\n",
        )
        .unwrap();
        std::fs::write(
            project.join(".pitwall/project.toml"),
            "schema = 1\n[[remote_hosts]]\nhost = \"evil\"\nssh_args = [\"-o\", \"ProxyCommand=touch /tmp/pwned\"]\n",
        )
        .unwrap();
        let paths = pw_core::config::AppPaths {
            config: global.clone(),
            data: dir.path().join("data"),
            state: dir.path().join("state"),
            cache: dir.path().join("cache"),
        };
        let global_snapshot = ConfigSource::global(&paths)
            .without_writes()
            .load()
            .unwrap();
        let project_snapshot = ConfigSource::project(&project)
            .without_writes()
            .load()
            .unwrap();
        let project_value = project_snapshot.get("project", KEY);
        assert!(project_value.is_some(), "the project file has the table");
        let (hosts, warnings) = read(global_snapshot.get("settings", KEY), project_value);
        assert_eq!(
            hosts,
            vec![RemoteHostSettings {
                nickname: Some("build".into()),
                target: SshTarget {
                    host: "build.ts.net".into(),
                    user: Some("dev".into()),
                    port: Some(2222),
                },
                ssh_args: vec!["-o".into(), "ServerAliveInterval=30".into()],
                port_forwards: Vec::new(),
            }]
        );
        assert!(
            warnings.iter().any(|w| w.contains("no effect")),
            "{warnings:?}"
        );
    }

    /// `[[remote_hosts.port_forwards]]` 를 읽는다(08 「원격 호스트 등록 예」 의 꼴). 루프백이 아닌 `local_host` 도 항목으로 남는다 —
    /// 열지 않는 것은 여는 쪽의 판정이다. 틀린 포워딩 · 그 머신의 루프백이 아닌 `remote_host` 는 그것만 빼고 경고한다.
    #[test]
    fn port_forwards_are_read_from_the_host_entry() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("settings.toml"),
            "schema = 1\n[[remote_hosts]]\nnickname = \"build\"\nhost = \"build.ts.net\"\n\
             [[remote_hosts.port_forwards]]\nlocal_port = 8080\nremote_port = 8080\n\
             [[remote_hosts.port_forwards]]\nlocal_port = 5005\nremote_port = 5005\nremote_host = \"localhost\"\nlocal_host = \"0.0.0.0\"\n\
             [[remote_hosts.port_forwards]]\nlocal_port = 70000\nremote_port = 1\n\
             [[remote_hosts.port_forwards]]\nlocal_port = 5432\nremote_port = 5432\nremote_host = \"db.internal\"\n",
        )
        .unwrap();
        let paths = pw_core::config::AppPaths {
            config: dir.path().to_path_buf(),
            data: dir.path().join("data"),
            state: dir.path().join("state"),
            cache: dir.path().join("cache"),
        };
        let snapshot = ConfigSource::global(&paths)
            .without_writes()
            .load()
            .unwrap();
        let (hosts, warnings) = read(snapshot.get("settings", KEY), None);
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].name(), "build");
        assert_eq!(
            hosts[0].port_forwards,
            vec![
                ForwardSetting {
                    local_port: 8080,
                    remote_port: 8080,
                    remote_host: None,
                    local_host: None,
                },
                ForwardSetting {
                    local_port: 5005,
                    remote_port: 5005,
                    remote_host: Some("localhost".into()),
                    local_host: Some("0.0.0.0".into()),
                },
            ]
        );
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert!(warnings[0].contains("port_forwards[2]"), "{warnings:?}");
        // 원격을 거쳐 다른 호스트로 가는 포워딩은 받지 않는다(메인 판정 2026-11-24).
        assert!(
            warnings[1].contains("port_forwards[3]") && warnings[1].contains("loopback"),
            "{warnings:?}"
        );
    }

    /// 틀린 항목(호스트 글자 · 호스트 키 확인을 끄는 `ssh_args` · 포트)은 빼고 경고한다 — 다른 항목은 쓴다.
    #[test]
    fn bad_entries_are_dropped_with_a_warning() {
        let global = serde_json::json!([
            {"host": "-oProxyCommand=x"},
            {"host": "a", "ssh_args": ["-o", "StrictHostKeyChecking=no"]},
            {"host": "b", "port": 70000},
            {"host": "mini"},
        ]);
        let (hosts, warnings) = read(Some(&global), None);
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].target, SshTarget::alias("mini"));
        assert_eq!(warnings.len(), 3, "{warnings:?}");
    }
}
