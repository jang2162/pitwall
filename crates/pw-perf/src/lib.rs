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

/// 앱 로그(`$STATE/logs/core.*.log`)에서 `message` 가 든 첫 줄의 `field=값`. 앱이 계산한 단일 인스턴스 키(`single-instance
/// key` 의 `identifier`)와 웹뷰 저장소(`webview data store` 의 `store`)를 하네스가 다시 계산하지 않고 읽는다.
pub fn log_field(log: &str, message: &str, field: &str) -> Option<String> {
    let prefix = format!("{field}=");
    log.lines()
        .filter(|line| line.contains(message))
        .find_map(|line| {
            line.split_whitespace()
                .find_map(|token| token.strip_prefix(&prefix))
                .map(|v| v.trim_matches('"').to_owned())
        })
}

/// `tauri-plugin-single-instance` 2.5 macOS 의 소켓 경로(apps/desktop/src-tauri/src/instance.rs 머리 주석).
pub fn single_instance_socket(identifier: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(format!(
        "/tmp/{}_si.sock",
        identifier.replace(['.', '-'], "_")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fields_are_read_from_the_app_log() {
        let log = concat!(
            "2026-10-18T05:30:18.89Z  INFO pitwall::diag: pitwall starting version=\"0.0.0\"\n",
            "2026-10-18T05:30:18.90Z  INFO pitwall: single-instance key identifier=dev.pitwall.desktop.data-00ff\n",
            "2026-10-18T05:30:18.91Z  INFO pitwall: webview data store store=ABABABAB-ABAB-ABAB-ABAB-ABABABABABAB\n",
        );
        assert_eq!(
            log_field(log, "single-instance key", "identifier").as_deref(),
            Some("dev.pitwall.desktop.data-00ff")
        );
        assert_eq!(
            log_field(log, "webview data store", "store").as_deref(),
            Some("ABABABAB-ABAB-ABAB-ABAB-ABABABABABAB")
        );
        assert_eq!(
            log_field(log, "pitwall starting", "version").as_deref(),
            Some("0.0.0")
        );
        assert_eq!(log_field(log, "single-instance key", "store"), None);
    }

    #[test]
    fn the_single_instance_socket_follows_the_plugin() {
        assert_eq!(
            single_instance_socket("dev.pitwall.desktop.data-00ff"),
            std::path::Path::new("/tmp/dev_pitwall_desktop_data_00ff_si.sock")
        );
    }

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
