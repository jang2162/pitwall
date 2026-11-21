//! Relay 의 앱 경로(18-m7-plan M7-04 ⑦). 네 뿌리(`$CONFIG` · `$DATA` · `$STATE` · `$CACHE` — `PITWALL_*_DIR` · XDG 규칙 그대로)
//! 아래 `relay/` 를 Relay 의 뿌리로 쓴다. Host 가 이 경로로 여는 것(`$DATA/relay/state.db` 레지스트리 · 신뢰 · 감사, `$STATE/relay/`
//! 의 MCP 소켓 · 로그, `$CONFIG/relay/settings.toml`)은 같은 머신의 Desktop 것과 겹치지 않는다. Desktop 의 `settings.toml` 은 읽지
//! 않는다.

use std::path::PathBuf;

use pw_core::config::AppPaths;

/// 각 뿌리 아래 Relay 의 폴더 이름.
pub const RELAY_DIR: &str = "relay";
/// 사람이 쓰는 Relay 설정(`$CONFIG/relay/relay.toml`). Relay 의 전역 설정 `settings.toml` 과 별개다.
pub const RELAY_TOML: &str = "relay.toml";
/// 로그 구성요소(`$STATE/relay/logs/relay.<날짜>.log` — 03 「로그 · 진단 (D22)」).
pub const LOG_COMPONENT: &str = "relay";

/// Desktop 과 같은 규칙으로 푼 앱 경로에서 Relay 의 앱 경로를 낸다. 필드를 모두 풀어 쓴다 — `AppPaths` 에 뿌리가 늘면 여기서
/// 컴파일이 멈춘다(그 뿌리도 `relay/` 아래로 정해야 한다).
pub fn relay_paths(base: &AppPaths) -> AppPaths {
    let AppPaths {
        config,
        data,
        state,
        cache,
    } = base;
    AppPaths {
        config: config.join(RELAY_DIR),
        data: data.join(RELAY_DIR),
        state: state.join(RELAY_DIR),
        cache: cache.join(RELAY_DIR),
    }
}

/// 이 프로세스의 환경으로 푼 Relay 경로.
pub fn from_env() -> Result<AppPaths, pw_core::config::PathError> {
    AppPaths::from_env().map(|base| relay_paths(&base))
}

/// `relay.toml` 의 자리. `paths` 는 [`relay_paths`] 가 낸 것.
pub fn relay_toml(paths: &AppPaths) -> PathBuf {
    paths.config.join(RELAY_TOML)
}

/// 로그 폴더(`$STATE/relay/logs`).
pub fn logs_dir(paths: &AppPaths) -> PathBuf {
    paths.state.join("logs")
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use pw_core::config::{OsFamily, PathEnv};

    use super::*;

    fn env(root: &Path) -> PathEnv {
        let mut env = PathEnv::empty(OsFamily::Unix);
        for (var, leaf) in [
            ("PITWALL_CONFIG_DIR", "config"),
            ("PITWALL_DATA_DIR", "data"),
            ("PITWALL_STATE_DIR", "state"),
            ("PITWALL_CACHE_DIR", "cache"),
        ] {
            env.vars
                .insert(var.into(), root.join(leaf).into_os_string());
        }
        env
    }

    /// 같은 환경으로 푼 Desktop 과 Relay 의 경로: Relay 의 뿌리는 모두 그 뿌리의 `relay/` 이고, 그래서 Host 가 여는 것(레지스트리
    /// `state.db` · MCP 소켓 · 전역 설정)이 Desktop 과 갈린다(18-m7-plan M7-04 시험 ⑥). 표는 필드를 모두 풀어 쓴다 — 뿌리가 늘면
    /// 컴파일이 멈춘다.
    #[test]
    fn every_relay_root_is_under_relay_and_apart_from_the_desktop() {
        let root = Path::new("/tmp/pw-relay-paths");
        let desktop = AppPaths::resolve(&env(root)).unwrap();
        let relay = relay_paths(&desktop);
        let AppPaths {
            config,
            data,
            state,
            cache,
        } = &relay;
        let table: [(&str, &PathBuf, &PathBuf); 4] = [
            ("config", config, &desktop.config),
            ("data", data, &desktop.data),
            ("state", state, &desktop.state),
            ("cache", cache, &desktop.cache),
        ];
        for (name, relay_root, desktop_root) in table {
            assert_eq!(
                relay_root,
                &desktop_root.join(RELAY_DIR),
                "{name} is not under relay/"
            );
        }

        // Host 가 여는 것: 레지스트리 파일 · MCP 소켓 · 전역 설정 · 감사 로그 · 로그.
        let desktop_registry = pw_core::registry::Registry::new(
            desktop.clone(),
            None,
            pw_core::bus::EventBus::new(16),
        );
        let relay_registry =
            pw_core::registry::Registry::new(relay.clone(), None, pw_core::bus::EventBus::new(16));
        assert_ne!(desktop_registry.file(), relay_registry.file());
        assert!(relay_registry.file().starts_with(data));
        let temp = Path::new("/tmp");
        let mcp = |p: &AppPaths| pw_mcp::socket::endpoint("same-key", &p.state, temp, 501).unwrap();
        assert_ne!(mcp(&desktop), mcp(&relay));
        assert!(mcp(&relay).starts_with(state));
        let global = |p: &AppPaths| pw_core::config::ConfigSource::global(p).dir().to_path_buf();
        assert_ne!(global(&desktop), global(&relay));
        assert!(global(&relay).starts_with(config));
        assert!(relay_toml(&relay).starts_with(config));
        assert!(logs_dir(&relay).starts_with(state));
    }
}
