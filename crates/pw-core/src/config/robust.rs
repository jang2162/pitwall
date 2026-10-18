//! Windows 에서 다른 프로세스 · 스레드가 같은 파일을 rename 으로 바꿔 끼우는 순간의 일시 오류를 기다려 넘긴다.
//!
//! [`super::migrate::write_atomic`](임시 파일 → rename)은 POSIX 에서 원자적이지만, Windows 의 `fs::rename` 이 먼저 쓰는
//! `MoveFileExW(MOVEFILE_REPLACE_EXISTING)` 바꿔 끼우기는 그렇지 않다 — 그 순간 같은 이름을 여는 쪽이 「파일을 찾을 수 없다」
//! (`ERROR_FILE_NOT_FOUND`) · 접근 거부 · 공유 위반을 잠깐 받는다(같은 `.local.toml` 을 메인 체크아웃과 워크트리의 앱이 동시에
//! 마이그레이션하는 경우). Go 의 cmd/go 도 같은 이유로 Windows 에서만 이 오류들을 다시 시도한다(golang.org/issue/32188).
//! 다른 OS 에서는 한 번만 한다.
//!
//! 오류에는 [`labeled`] 로 연산 이름과 실제 경로(대상 · 임시 파일 · 보관본)를 붙인다. 설정 파일 하나의 마이그레이션 오류는 모두 그
//! 설정 파일 경로로 보고되므로, 붙이지 않으면 어느 연산이 실패했는지 로그로 알 수 없다.

use std::io;
use std::path::Path;
use std::time::{Duration, Instant};

/// 일시 오류를 다시 시도하는 시간. Go cmd/go 의 robustio 와 같은 값이다(`arbitraryTimeout = 2000 * time.Millisecond`,
/// `src/cmd/internal/robustio/robustio_flaky.go`). 500ms 일 때 Windows CI 에서 같은 시험이 다시 실패했다 — 시간이 모자랐는지
/// 감싸지 않은 연산이었는지는 [`labeled`] 의 메시지가 가린다.
const PATIENCE: Duration = Duration::from_millis(2000);

/// io 오류의 메시지 앞에 `<연산> <경로>: ` 를 붙인다. 종류(`kind`)는 그대로라 [`retrying`] 의 판정에 영향이 없다.
pub(crate) fn labeled<'a>(op: &'a str, path: &'a Path) -> impl FnOnce(io::Error) -> io::Error + 'a {
    move |e| io::Error::new(e.kind(), format!("{op} {}: {e}", path.display()))
}

/// `op` 를 하고, Windows 의 일시 오류면 [`PATIENCE`] 동안 다시 한다.
pub(crate) fn retrying<T>(op: impl FnMut() -> io::Result<T>) -> io::Result<T> {
    retry_while(op, ephemeral, PATIENCE)
}

/// `ERROR_SHARING_VIOLATION`.
const SHARING_VIOLATION: i32 = 32;

fn ephemeral(e: &io::Error) -> bool {
    cfg!(windows)
        && (matches!(
            e.kind(),
            io::ErrorKind::NotFound | io::ErrorKind::PermissionDenied
        ) || e.raw_os_error() == Some(SHARING_VIOLATION))
}

fn retry_while<T>(
    mut op: impl FnMut() -> io::Result<T>,
    ephemeral: impl Fn(&io::Error) -> bool,
    patience: Duration,
) -> io::Result<T> {
    let deadline = Instant::now() + patience;
    let mut pause = Duration::from_millis(1);
    loop {
        match op() {
            Err(e) if ephemeral(&e) && Instant::now() < deadline => {
                std::thread::sleep(pause);
                pause = (pause * 2).min(Duration::from_millis(50));
            }
            other => return other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn not_found() -> io::Error {
        io::Error::from(io::ErrorKind::NotFound)
    }

    #[test]
    fn ephemeral_errors_are_retried_until_they_clear() {
        let mut calls = 0;
        let result = retry_while(
            || {
                calls += 1;
                if calls < 4 {
                    Err(not_found())
                } else {
                    Ok(calls)
                }
            },
            |_| true,
            Duration::from_secs(5),
        );
        assert_eq!(result.unwrap(), 4);
    }

    #[test]
    fn other_errors_and_lasting_ones_are_returned() {
        let mut calls = 0;
        let result: io::Result<()> = retry_while(
            || {
                calls += 1;
                Err(not_found())
            },
            |_| false,
            Duration::from_secs(5),
        );
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::NotFound);
        assert_eq!(calls, 1, "일시 오류가 아니면 다시 하지 않는다");

        let started = Instant::now();
        let result: io::Result<()> =
            retry_while(|| Err(not_found()), |_| true, Duration::from_millis(50));
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::NotFound);
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    /// 다른 OS 에서는 rename 이 원자적이라 다시 시도할 일시 오류가 없다.
    #[test]
    fn only_windows_retries() {
        assert_eq!(ephemeral(&not_found()), cfg!(windows));
        assert!(!ephemeral(&io::Error::from(io::ErrorKind::InvalidData)));
    }
}
