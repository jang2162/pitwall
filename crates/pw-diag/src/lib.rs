//! 로그 · 진단(`docs/design/03-architecture.md` 「로그 · 진단 (D22)」 절).
//!
//! - [`logging`]: tracing 구독자. `$STATE/logs/` 아래 구성요소별(코어 · UI) 회전 로그, 기본 7일 보관.
//! - [`methods`]: `log.write`(웹뷰 로그 받기) · `diag.collect`(진단 텍스트).
//! - [`Diagnostics`]: 「진단 정보 복사」 텍스트를 만들고 [`mask`] 로 가린다.
//! - [`install_panic_hook`]: 패닉의 메시지 · 위치 · 스택을 코어 로그에 남긴다. 크래시 리포트는 설정 자리만 있다.
//!
//! 로그 디렉터리는 인자로 받는다. 경로 해석(`$STATE`)은 설정 계층(M0-08)의 몫이다.
//!
//! # 로그에 비밀을 쓰지 않는다
//!
//! 로그 파일은 평문이고 7일 동안 남으며, 사용자가 이슈에 통째로 붙일 수 있다.
//!
//! - 토큰 · 비밀번호 · 패스프레이즈 · 쿠키 · `Authorization` 헤더 값을 필드나 메시지에 넣지 않는다. 비밀은 키체인의
//!   `secret_ref` 이름으로만 가리킨다.
//! - 설정 · 요청 · 환경 변수 구조체를 `{:?}` 로 통째로 찍지 않는다. 필요한 필드만 골라 쓴다 — 나중에 더해진 비밀 필드가
//!   조용히 따라 나간다.
//! - URL 은 사용자 정보(`user:pass@`)와 쿼리 문자열을 걷고 쓴다.
//! - 파일 내용 · 쿼리 결과 · 터미널 출력처럼 사용자 데이터를 로그에 옮기지 않는다. 크기나 개수만 쓴다.
//!
//! [`mask`] 는 이 규칙이 새는 자리를 덮는 두 번째 방어선이고, 진단 텍스트에만 적용한다 — 로그 파일 자체는 가리지 않는다.

pub mod logging;
mod mask;
pub mod methods;
mod panic;
mod report;
mod rolling;
#[cfg(test)]
mod test_dir;

pub use logging::{LogConfig, init, subscriber};
pub use mask::mask;
pub use panic::{CRASH_REPORTS_ENABLED, install_panic_hook};
pub use report::Diagnostics;
pub use rolling::{Clock, RollingFile, SystemClock};
