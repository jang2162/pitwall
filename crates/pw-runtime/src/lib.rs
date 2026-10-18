//! 런타임 환경(`docs/design/03-architecture.md` §5-1). 지금은 4번 제공자 「로그인 셸」의 최소판만 있다 — 앱이 띄우는 프로세스
//! (외부 도구, `crates/pw-open-with`)에 줄 PATH 를 사용자의 로그인 셸에서 한 번 가져와 캐시한다([`login_shell`]).
//! mise · direnv · asdf 제공자, 셸의 다른 환경 변수, 디렉터리별 캐시와 무효화는 아직 없다.
//!
//! 왜: Finder · Dock · `open` 으로 띄운 GUI 앱의 PATH 는 launchd 의 것(`/usr/bin:/bin:/usr/sbin:/sbin`)이라 `zed` · `code` ·
//! `idea` 처럼 `/usr/local/bin` · `~/.local/bin` · Toolbox 스크립트 폴더에 있는 명령을 찾지 못한다. 터미널에서 띄운 앱(`mise run dev`)은
//! 터미널 PATH 를 물려받아 이 문제를 가린다.

pub mod login_shell;

pub use login_shell::{LOGIN_PATH, LoginPath, PathSource, ResolvedPath};
