//! 설정 다시 읽기의 결과(`config.error` · `config.changed`)를 코어 로그에 남긴다.
//!
//! 설정 오류 메시지는 TOML 파서가 문제 줄을 그대로 옮겨 적는다(`1 | token = "…`). 설정 파일에 든 값이 로그로 새지 않게
//! 그 발췌 줄을 걷고 남긴다 — `crates/pw-diag/src/lib.rs` 「로그에 비밀을 쓰지 않는다」 절.

use pw_core::bus::{Event, Received};
use pw_core::config::{ConfigChanged, ConfigFault, ConfigNotice, TOPIC_CHANGED, TOPIC_ERROR};
use pw_core::{EventBus, Filter, Scope, Subscription};

/// 설정 이벤트 구독. Host 를 만든 직후, 파일 감시를 켜기 전에 연다 — 그 사이의 이벤트를 놓치지 않게.
pub fn subscribe(bus: &EventBus) -> Subscription {
    bus.subscribe(Filter::all().topics([TOPIC_ERROR, TOPIC_CHANGED]))
}

/// 구독이 닫힐 때까지 받은 이벤트를 로그로 남긴다.
pub async fn run(mut events: Subscription) {
    while let Some(received) = events.recv().await {
        match received {
            Received::Event(event) => log_event(&event),
            Received::Lagged(n) => tracing::warn!(dropped = n, "missed config events"),
        }
    }
}

pub fn log_event(event: &Event) {
    let scope = scope_name(&event.scope);
    match event.topic.as_str() {
        TOPIC_ERROR => match serde_json::from_value::<ConfigFault>(event.payload.clone()) {
            Ok(fault) => log_fault(&event.scope, &fault),
            Err(e) => tracing::warn!(scope, %e, "unreadable config.error event"),
        },
        TOPIC_CHANGED => match serde_json::from_value::<ConfigChanged>(event.payload.clone()) {
            Ok(changed) => {
                if !changed.changed.is_empty() {
                    tracing::info!(scope, changed = ?changed.changed, "config reloaded");
                }
                for notice in &changed.notices {
                    log_notice(scope.as_str(), notice);
                }
            }
            Err(e) => tracing::warn!(scope, %e, "unreadable config.changed event"),
        },
        _ => {}
    }
}

/// 읽기 실패. 앱은 이전 설정(처음이면 내장 기본값)으로 계속 돈다.
pub fn log_fault(scope: &Scope, fault: &ConfigFault) {
    // 메시지는 `ConfigError` 의 Display(`{경로}: {내용}`)라 경로를 앞에 단다. 경로는 `path` 필드가 따로 싣는다.
    let message = fault
        .path
        .as_deref()
        .and_then(|path| fault.message.strip_prefix(path))
        .and_then(|rest| rest.strip_prefix(": "))
        .unwrap_or(&fault.message);
    tracing::warn!(
        scope = scope_name(scope),
        path = fault.path.as_deref().unwrap_or("-"),
        error = %summarize(message),
        "config could not be read; keeping the previous values"
    );
}

fn log_notice(scope: &str, notice: &ConfigNotice) {
    match notice {
        ConfigNotice::Migrated {
            path,
            from,
            to,
            backup,
        } => tracing::warn!(scope, path, from, to, backup, "config file migrated"),
        ConfigNotice::FutureSchema {
            path,
            found,
            supported,
        } => tracing::warn!(
            scope,
            path,
            found,
            supported,
            "config file is newer than this app; opened read-only"
        ),
        ConfigNotice::BrokenLink { path } => {
            tracing::warn!(scope, path, "config symlink points nowhere; skipped")
        }
    }
}

fn scope_name(scope: &Scope) -> String {
    match scope {
        Scope::Host => "host".to_owned(),
        Scope::Project(id) => format!("project:{id}"),
    }
}

/// 오류 메시지를 한 줄로. TOML 파서의 발췌(`  |` 와 `N | …` 줄)를 걷는다.
fn summarize(message: &str) -> String {
    message
        .lines()
        .map(str::trim)
        .filter(|line| {
            !line.is_empty()
                && !line
                    .trim_start_matches(|c: char| c.is_ascii_digit())
                    .trim_start()
                    .starts_with('|')
        })
        .collect::<Vec<_>>()
        .join("; ")
}

#[cfg(test)]
mod tests {
    use std::fs;

    use pw_core::bus::DEFAULT_CAPACITY;
    use pw_core::config::{AppPaths, ConfigSource, ConfigStore};
    use pw_diag::LogConfig;

    use super::*;

    const SECRET: &str = "ghp_A1b2C3d4E5f6G7h8I9j0K1l2";

    fn core_log(dir: &std::path::Path) -> String {
        fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| {
                p.file_name()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .starts_with("core.")
            })
            .map(|p| fs::read_to_string(p).unwrap())
            .collect()
    }

    #[test]
    fn toml_excerpts_are_dropped_from_messages() {
        let message = "TOML parse error at line 2, column 9\n  |\n2 | token = \"ghp_x\n  |         ^\ninvalid basic string\n";
        assert_eq!(
            summarize(message),
            "TOML parse error at line 2, column 9; invalid basic string"
        );
    }

    #[test]
    fn reload_errors_and_notices_reach_the_core_log_without_file_contents() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::under(&tmp.path().join("app"));
        fs::create_dir_all(&paths.config).unwrap();
        let settings = paths.config.join("settings.toml");
        fs::write(&settings, "schema = 1\n").unwrap();
        let logs = tmp.path().join("logs");
        let subscriber = pw_diag::subscriber(&LogConfig::new(&logs)).unwrap();

        let bus = EventBus::new(DEFAULT_CAPACITY);
        let store = ConfigStore::open(ConfigSource::global(&paths), bus.clone(), Scope::Host);
        let mut events = subscribe(&bus);
        fs::write(&settings, format!("schema = 1\ntoken = \"{SECRET}\n")).unwrap();
        assert!(store.reload().is_err());
        // v0 파일(schema 없음)은 마이그레이션 알림을 낸다.
        fs::write(&settings, "[sample]\ngreeting = \"x\"\n").unwrap();
        store.reload().unwrap();

        tracing::subscriber::with_default(subscriber, || {
            while let Some(Received::Event(event)) = events.try_recv() {
                log_event(&event);
            }
        });
        let core = core_log(&logs);
        assert!(
            core.contains("WARN")
                && core.contains("config could not be read; keeping the previous values"),
            "{core}"
        );
        assert!(core.contains("settings.toml"), "{core}");
        assert!(core.contains("TOML parse error"), "{core}");
        assert!(!core.contains(SECRET), "설정 값이 로그로 샜다:\n{core}");
        assert!(core.contains("config file migrated"), "{core}");
        assert!(core.contains("config reloaded"), "{core}");
    }

    #[test]
    fn startup_faults_are_logged_too() {
        let tmp = tempfile::tempdir().unwrap();
        let logs = tmp.path().join("logs");
        let fault = ConfigFault {
            message:
                "/cfg/settings.toml: TOML parse error at line 1, column 1\n  |\n1 | x = = 1\n  | ^\nexpected value"
                    .into(),
            path: Some("/cfg/settings.toml".into()),
        };
        tracing::subscriber::with_default(
            pw_diag::subscriber(&LogConfig::new(&logs)).unwrap(),
            || {
                log_fault(&Scope::Project("p1".into()), &fault);
            },
        );
        let core = core_log(&logs);
        assert!(core.contains("scope=\"project:p1\""), "{core}");
        assert_eq!(
            core.matches("/cfg/settings.toml").count(),
            1,
            "경로는 path 필드에 한 번만: {core}"
        );
        assert!(
            core.contains("error=TOML parse error at line 1, column 1; expected value"),
            "{core}"
        );
        assert!(core.contains("expected value"), "{core}");
        assert!(!core.contains("x = = 1"), "{core}");
    }
}
