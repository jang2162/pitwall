//! `pw-relay`: 원격 머신의 Relay 데몬(18-m7-plan M7-04 — `docs/design/08-remote-mobile.md` 「Relay」).
//!
//! 바이너리는 `apps/relay`(`pitwall-relay`)이고 여기에는 그 몸통이 있다:
//!
//! - [`paths`]: Relay 의 앱 경로 — `$CONFIG` · `$DATA` · `$STATE` · `$CACHE` 아래 `relay/` 가 뿌리다. 같은 머신의 Desktop 과
//!   레지스트리 · 신뢰 · 감사 · MCP 소켓 · 토큰을 나누지 않는다.
//! - [`config`]: `$CONFIG/relay/relay.toml`(`name` · `roots` · `[idle]`). RPC 로 쓰지 않는다(메인 판정 M7-P5).
//! - [`roots`]: `roots` 밖은 프로젝트로 등록하지 않는다(심링크를 푼 실제 경로로).
//! - [`socket`]: 사용자 전용 유닉스 소켓(`$STATE/relay/<채널>-<판>.sock`) — 폴더 0700 · 소켓 0600 · 상대 uid.
//! - [`command`]: 데몬 소켓의 좁은 명령(`open` · `askpass` · `status`). 첫 틀의 종류로 링크와 가른다.
//! - [`daemon`]: 받기 — 증명 전 연결 수 상한, 첫 틀 상한(`pw_link::HANDSHAKE_MAX_FRAME`), 링크 · 좁은 명령 가르기, 붙은 링크.
//! - [`idle`]: 링크 · 실행 · 터미널이 없으면 `[idle] exit_after` 뒤 끝난다.
//! - [`host`]: Relay 부품으로 조립한 Host(`pw_host::assemble_relay`)와 `roots` 판정.
//! - [`proxy`]: stdio ↔ 데몬 소켓. 데몬이 없으면 띄운다(`setsid`).

// Relay 는 macOS · Linux 만이다(18-m7-plan 결정 M7-D4) — Windows 에서는 빈 크레이트다(워크스페이스 빌드만 지난다).
#![cfg(unix)]

pub mod command;
pub mod config;
pub mod daemon;
pub mod host;
pub mod idle;
pub mod pairings;
pub mod paths;
pub mod proxy;
pub mod roots;
pub mod socket;

/// 이 바이너리의 판(링크 핸드셰이크 · 소켓 이름). 워크스페이스 판 하나다.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// 릴리스 채널(링크 핸드셰이크 — M7-03 「판이 다르면」). 릴리스 빌드가 `PITWALL_RELEASE_CHANNEL` 로 박는다(M7-05). 없으면 `dev`.
pub const RELEASE_CHANNEL: &str = match option_env!("PITWALL_RELEASE_CHANNEL") {
    Some(channel) => channel,
    None => "dev",
};

/// 개발 채널(`dev`) 빌드에서만 읽는 판 바꾸기 환경 변수(18-m7-plan M7-07 — 판 둘을 한 빌드로 흉내 내는 시험 · `relay-live`).
/// 릴리스 빌드는 늘 채널을 박으므로(`PITWALL_RELEASE_CHANNEL`) 이 변수를 보지 않는다.
pub const DEV_VERSION_ENV: &str = "PITWALL_RELAY_DEV_VERSION";

/// 이 데몬이 내세우는 판 — 소켓 이름 · 핸드셰이크 · `status` 가 모두 이것을 쓴다. 보통 [`VERSION`] 이다. 개발 채널 빌드에서만
/// [`DEV_VERSION_ENV`] 가 판으로 쓸 수 있는 글(`[0-9A-Za-z.+-]`, 64자까지)이면 그 값이다.
pub fn version() -> &'static str {
    static VALUE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    VALUE.get_or_init(|| {
        if RELEASE_CHANNEL == "dev"
            && let Ok(value) = std::env::var(DEV_VERSION_ENV)
            && dev_version_allowed(&value)
        {
            return value;
        }
        VERSION.to_owned()
    })
}

fn dev_version_allowed(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '+' | '-'))
        && !value.starts_with('-')
}

/// 링크 핸드셰이크에 싣는 판.
pub fn versions() -> pw_link::Versions {
    pw_link::Versions::new(version(), RELEASE_CHANNEL)
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_dev_version_is_a_plain_version_string() {
        assert!(super::dev_version_allowed("0.0.1"));
        assert!(super::dev_version_allowed("1.2.3-beta.1+x"));
        for bad in ["", "-1", "1/2", "1 2", "1;x", &"9".repeat(65)] {
            assert!(!super::dev_version_allowed(bad), "{bad}");
        }
    }
}
