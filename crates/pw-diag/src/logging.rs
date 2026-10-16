//! tracing 구독자: 구성요소별 회전 로그 파일(지금은 코어 · UI).
//!
//! 코어 로그는 UI 대상([`UI_TARGET`])이 아닌 모든 이벤트, UI 로그는 웹뷰가 `log.write` 로 보낸 이벤트만 받는다.

use std::io;
use std::num::NonZeroU32;
use std::path::PathBuf;
use std::sync::Arc;

use tracing::Subscriber;
use tracing::level_filters::LevelFilter;
use tracing_subscriber::filter::filter_fn;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::{Layer, Registry, fmt};

use crate::rolling::{Clock, RollingFile, SystemClock};

/// 코어 로그 파일의 구성요소 이름(`core.<날짜>.log`).
pub const CORE: &str = "core";
/// UI 로그 파일의 구성요소 이름(`ui.<날짜>.log`).
pub const UI: &str = "ui";
/// UI 로그로 가는 이벤트의 target. 이 target 의 이벤트는 코어 로그에 쓰지 않는다.
pub const UI_TARGET: &str = "pitwall::ui";
/// 패닉 훅이 남기는 이벤트의 target.
pub const PANIC_TARGET: &str = "pitwall::panic";

/// 기본 보관 기간(오늘 포함 날 수). `docs/design/03-architecture.md` 「로그 · 진단 (D22)」 절.
pub const DEFAULT_RETENTION_DAYS: NonZeroU32 = NonZeroU32::new(7).unwrap();

/// 수준을 바꾸는 환경 변수. 값은 `trace` · `debug` · `info` · `warn` · `error` · `off`.
pub const LEVEL_ENV: &str = "PITWALL_LOG";

#[derive(Clone)]
pub struct LogConfig {
    /// 로그 디렉터리(`$STATE/logs`). 경로 해석은 호출한 쪽이 한다.
    pub dir: PathBuf,
    pub retention_days: NonZeroU32,
    pub level: LevelFilter,
    /// 파일과 함께 stderr 에도 쓴다. 개발 실행에서 터미널로 보려고 둔다.
    pub stderr: bool,
    pub clock: Arc<dyn Clock>,
}

impl LogConfig {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            retention_days: DEFAULT_RETENTION_DAYS,
            level: LevelFilter::INFO,
            stderr: false,
            clock: Arc::new(SystemClock),
        }
    }

    /// [`LEVEL_ENV`] 가 있으면 그 수준을 쓴다. 읽을 수 없는 값은 무시한다.
    #[must_use]
    pub fn level_from_env(mut self) -> Self {
        if let Some(level) = std::env::var(LEVEL_ENV)
            .ok()
            .and_then(|v| v.trim().parse().ok())
        {
            self.level = level;
        }
        self
    }
}

/// 설정대로 구독자를 만든다. 로그 디렉터리를 만들고 오늘 파일을 열고 오래된 파일을 지운다.
pub fn subscriber(config: &LogConfig) -> io::Result<impl Subscriber + Send + Sync + 'static> {
    let open = |component: &str| {
        RollingFile::open(
            &config.dir,
            component,
            config.retention_days,
            config.clock.clone(),
        )
    };
    let level = config.level;
    let core = fmt::layer()
        .with_ansi(false)
        .with_writer(open(CORE)?)
        .with_filter(filter_fn(move |meta| {
            meta.target() != UI_TARGET && level >= *meta.level()
        }));
    // UI 로그의 target 은 모두 같으므로 줄에 적지 않는다. 웹뷰 쪽 출처는 `source` 필드가 말한다.
    let ui = fmt::layer()
        .with_ansi(false)
        .with_target(false)
        .with_writer(open(UI)?)
        .with_filter(filter_fn(move |meta| {
            meta.target() == UI_TARGET && level >= *meta.level()
        }));
    let stderr = config
        .stderr
        .then(|| fmt::layer().with_writer(io::stderr).with_filter(level));
    Ok(Registry::default().with(core).with(ui).with(stderr))
}

/// 전역 구독자로 설치한다. 프로세스에서 한 번만 부른다.
pub fn init(config: &LogConfig) -> io::Result<()> {
    tracing::subscriber::set_global_default(subscriber(config)?).map_err(io::Error::other)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::rolling::tests::FakeClock;
    use crate::test_dir::TestDir;

    #[test]
    fn core_and_ui_events_go_to_separate_files() {
        let dir = TestDir::new();
        let mut config = LogConfig::new(dir.path());
        config.clock = FakeClock::at("2026-10-16");
        let subscriber = subscriber(&config).unwrap();
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(target: "pw_core::host", "core started");
            tracing::warn!(target: UI_TARGET, source = "main.tsx", "ui hiccup");
            tracing::debug!("below the level");
        });
        let core = fs::read_to_string(dir.path().join("core.2026-10-16.log")).unwrap();
        let ui = fs::read_to_string(dir.path().join("ui.2026-10-16.log")).unwrap();
        assert!(core.contains("INFO pw_core::host: core started"), "{core}");
        assert!(!core.contains("ui hiccup"), "{core}");
        assert!(!core.contains("below the level"), "{core}");
        assert!(ui.contains("WARN ui hiccup source=\"main.tsx\""), "{ui}");
        assert!(!ui.contains("core started"), "{ui}");
    }
}
