//! `pitwall-relay` — 원격 머신의 Relay(18-m7-plan M7-04, `docs/design/08-remote-mobile.md` 「Relay」). 창이 없다 — 창이 하는 일
//! (승인 · askpass · 열기 · 신뢰 묻기)은 링크로 붙은 Desktop 이 한다(M7-12 · M7-15 · M7-20).
//!
//! - `pitwall-relay proxy`: SSH 가 띄운다. stdio 를 이 판의 데몬 소켓에 잇고, 데몬이 없으면 띄운다.
//! - `pitwall-relay daemon`: 데몬(앞에서 돈다 — `proxy` 가 `setsid` 로 띄운다). 한 판에 하나.
//! - `pitwall-relay status`: 떠 있는 데몬의 상태(JSON 한 줄).
//! - `pitwall-relay pair --from-ssh <키> [<이름>]` · `unpair <기기 id>`: 페어링 표 파일에 직접 쓴다(데몬 소켓 명령이 아니다 — M7-06).
//! - `pitwall-relay --verify <파일> <sha256>`: 원격에 해시 도구가 없을 때 설치기가 올린 파일을 본다(M7-06).
//!
//! 몸통은 `crates/pw-relay` 다. 하위 명령은 M7-15(`askpass` · `open`) · M7-20(`mcp`)이 더한다.

// Relay 는 macOS · Linux 만이다(결정 M7-D4). 워크스페이스가 Windows 에서도 빌드되므로 그쪽에서는 알리고 끝난다.
#[cfg(unix)]
mod relay;

fn main() -> std::process::ExitCode {
    #[cfg(unix)]
    {
        relay::main()
    }
    #[cfg(not(unix))]
    {
        eprintln!("pitwall-relay runs on macOS and Linux only");
        std::process::ExitCode::from(2)
    }
}
