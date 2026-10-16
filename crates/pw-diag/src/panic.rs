//! 패닉 훅과 크래시 리포트 설정 자리.
//!
//! 크래시 리포트는 기본 꺼짐이고 전송 코드는 아직 없다(`docs/design/03-architecture.md` 「로그 · 진단 (D22)」 절).
//! 지금 하는 것은 패닉의 메시지 · 위치 · 스택을 코어 로그에 남기는 것까지다. 전송을 붙일 때는 D22 대로
//! 스택 · 버전 · OS 만 보내고(경로 · 코드 · 비밀 제외) 보내기 전에 내용을 보여 준다.

use std::backtrace::Backtrace;
use std::panic::PanicHookInfo;

use crate::logging::PANIC_TARGET;

/// 크래시 리포트를 보낼지. 기본 꺼짐(opt-in, D22).
// M0-08 설정으로 이전 예정 — 설정 계층이 들어오면 `settings.toml` 의 값으로 바꾸고 이 상수를 걷는다.
pub const CRASH_REPORTS_ENABLED: bool = false;

/// 패닉을 코어 로그에 남기는 훅을 건다. 원래 훅(stderr 출력)도 이어서 부른다.
///
/// 스택은 `RUST_BACKTRACE` 와 상관없이 항상 잡는다 — 사용자가 이슈를 낼 때 다시 재현하라고 할 수 없다.
pub fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        log_panic(info);
        previous(info);
    }));
}

fn log_panic(info: &PanicHookInfo<'_>) {
    let message = info
        .payload()
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| info.payload().downcast_ref::<String>().map(String::as_str))
        .unwrap_or("<non-string panic payload>");
    let location = info
        .location()
        .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
        .unwrap_or_default();
    let thread = std::thread::current();
    let thread = thread.name().unwrap_or("<unnamed>");
    let backtrace = Backtrace::force_capture();
    tracing::error!(
        target: PANIC_TARGET,
        thread,
        location,
        "panic: {message}\nstack backtrace:\n{backtrace}"
    );
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::logging::{LogConfig, subscriber};
    use crate::rolling::tests::FakeClock;
    use crate::test_dir::TestDir;

    #[test]
    fn panic_hook_writes_message_location_and_stack_to_the_core_log() {
        let dir = TestDir::new();
        let mut config = LogConfig::new(dir.path());
        config.clock = FakeClock::at("2026-10-16");
        let subscriber = subscriber(&config).unwrap();
        // nextest 는 시험마다 프로세스를 따로 띄우므로 전역 훅을 바꿔도 다른 시험에 새지 않는다.
        install_panic_hook();
        tracing::subscriber::with_default(subscriber, || {
            let result = std::panic::catch_unwind(|| panic!("boom {}", 42));
            assert!(result.is_err());
        });
        let _ = std::panic::take_hook();
        let core = fs::read_to_string(dir.path().join("core.2026-10-16.log")).unwrap();
        assert!(
            core.contains("ERROR pitwall::panic: panic: boom 42"),
            "{core}"
        );
        assert!(core.contains("stack backtrace:"), "{core}");
        assert!(core.contains("src/panic.rs:"), "{core}");
        assert!(
            core.contains("panic_hook_writes_message_location_and_stack"),
            "스택에 시험 함수 프레임이 있어야 한다: {core}"
        );
    }
}
