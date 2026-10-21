//! 런타임 환경(`docs/design/03-architecture.md` §5-1). 앱이 띄우는 프로세스(외부 도구 `crates/pw-open-with`, 이후 터미널 · LSP ·
//! 실행 구성)에 줄 환경 전체를 (프로젝트, 디렉터리)마다 만들어 캐시한다.
//!
//! - [`provider`]: 제공자 trait. 환경은 앱 프로세스 → 로그인 셸 → 디렉터리의 제공자 하나 순으로 쌓인다.
//! - [`login_shell`]: 4번 제공자 「로그인 셸」 — 셸이 만든 환경 전체. 모든 환경의 바탕이다.
//! - [`mise`]: 1번 제공자 「mise」 — 디렉터리의 mise 설정이 고정한 도구 버전(`mise env --json`). 신뢰한 프로젝트에서만.
//!   direnv · asdf 는 아직 없다(12-m1-plan.md §3 이월).
//! - [`runtime`]: 캐시, 「환경 다시 읽기」, 버전 파일 변경(파일 감시), `runtime.changed`, 프로젝트 설정 `[runtime] provider`.
//!   실패는 남기지 않아 다음 요청이 다시 시도한다(시간 초과만 잠시 냉각한다, [`login_shell`]).
//! - [`methods`]: `runtime.get` · `runtime.reload` RPC.
//!
//! 왜: Finder · Dock · `open` 으로 띄운 GUI 앱의 환경은 launchd 의 것(PATH `/usr/bin:/bin:/usr/sbin:/sbin`)이라 `zed` · `code` ·
//! `idea` 처럼 `/usr/local/bin` · `~/.local/bin` · Toolbox 스크립트 폴더에 있는 명령을 찾지 못하고, 셸에서 정한 `JAVA_HOME` 같은
//! 변수도 없다. 터미널에서 띄운 앱(`mise run dev`)은 터미널 환경을 물려받아 이 문제를 가린다.

mod cache;
pub mod env;
pub mod login_shell;
pub mod methods;
pub mod mise;
mod process;
pub mod provider;
pub mod runtime;

pub use env::{RuntimeEnv, RuntimeTool};
pub use login_shell::LoginShell;
pub use mise::Mise;
pub use provider::{Layer, ProviderError, RuntimeProvider};
pub use runtime::{
    ProviderSetting, Resolved, Runtime, RuntimeHost, TOPIC_CHANGED, follow_files, subscribe_files,
};

#[cfg(all(test, unix))]
mod tests;
