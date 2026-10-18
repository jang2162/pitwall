//! 데스크톱 앱의 콜드 스타트 · 유휴 메모리 측정(docs/design/06-roadmap.md 「성능 목표 (검증 기준)」). 무엇을 어떻게 재는지와
//! 사용법은 CONTRIBUTING.md 「성능 기준선」 절이 정본이다. 실행 본체는 main.rs, 앱이 남기는 표시는
//! `apps/desktop/src-tauri/src/perf.rs`.

pub mod marks;
pub mod report;
pub mod stats;

#[cfg(target_os = "macos")]
pub mod macos;

/// 앱에 표시 파일을 알리는 변수(`apps/desktop/src-tauri/src/perf.rs` 의 `MARK_ENV`).
pub const MARK_ENV: &str = "PITWALL_PERF_MARK";

/// `ioreg -n Root -d1 -a` 의 plist 에서 화면 잠금을 읽는다. 키가 없으면 잠기지 않은 것이다.
pub fn screen_locked(ioreg_plist: &str) -> bool {
    let key = "<key>CGSSessionScreenIsLocked</key>";
    ioreg_plist.find(key).is_some_and(|at| {
        ioreg_plist[at + key.len()..]
            .trim_start()
            .starts_with("<true/>")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lock_flag_is_read_from_the_root_plist() {
        let locked = "<dict>\n\t<key>CGSSessionScreenIsLocked</key>\n\t<true/>\n</dict>";
        assert!(screen_locked(locked));
        let unlocked = "<dict>\n\t<key>CGSSessionScreenIsLocked</key>\n\t<false/>\n</dict>";
        assert!(!screen_locked(unlocked));
        assert!(!screen_locked(
            "<dict><key>IOConsoleUsers</key><array/></dict>"
        ));
    }
}
