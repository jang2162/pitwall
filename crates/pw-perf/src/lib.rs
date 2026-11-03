//! 데스크톱 앱의 콜드 스타트 · 유휴 메모리 측정(docs/design/06-roadmap.md 「성능 목표 (검증 기준)」). 무엇을 어떻게 재는지와
//! 사용법은 CONTRIBUTING.md 「성능 기준선」 절이 정본이다. 실행 본체는 main.rs, 앱이 남기는 표시는
//! `apps/desktop/src-tauri/src/perf.rs`.

pub mod db;
pub mod git;
pub mod lsp;
pub mod lsp_fake;
pub mod marks;
pub mod report;
pub mod stats;

#[cfg(target_os = "macos")]
pub mod macos;

/// 앱에 표시 파일을 알리는 변수(`apps/desktop/src-tauri/src/perf.rs` 의 `MARK_ENV`).
pub const MARK_ENV: &str = "PITWALL_PERF_MARK";

/// 앱에 측정 시나리오를 알리는 변수(`apps/desktop/src-tauri/src/perf.rs` 의 `SCENARIO_ENV`).
pub const SCENARIO_ENV: &str = "PITWALL_PERF_SCENARIO";

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

/// 측정할 프로젝트 `root` 를 `paths` 의 레지스트리에 미신뢰로 등록한다 — 처음 연 사람이 신뢰 확인에서 「신뢰하지 않음」 을 고른
/// 것과 같은 저장 값이다(`Registry::claim_trust_prompt` 가 미신뢰를 적고, 「신뢰하지 않음」 의 `set_trust(false)` 는 같은 값이라
/// 쓰지 않는다). 묻지 않은 채로 두면 앱이 첫 열기에서 신뢰 확인 다이얼로그를 띄운다: 키맵 팝업이라 시나리오의 ⌘9 를 버리고
/// (Git 로그가 열리지 않는다), [신뢰] 에 포커스가 있어 그 창에 들어간 사람의 키 하나가 프로젝트를 신뢰로 바꾼다.
pub fn register_untrusted(
    paths: &pw_core::config::AppPaths,
    root: &std::path::Path,
) -> Result<pw_core::registry::Project, String> {
    let registry =
        pw_core::registry::Registry::new(paths.clone(), None, pw_core::EventBus::new(16));
    let project = registry
        .create(pw_core::registry::NewProject {
            root: root.to_path_buf(),
            ..pw_core::registry::NewProject::default()
        })
        .map_err(|e| format!("cannot register the project: {e}"))?;
    registry
        .claim_trust_prompt(&project.id)
        .map_err(|e| format!("cannot record the project as untrusted: {e}"))?;
    Ok(project)
}

/// 측정할 프로젝트 `root` 를 `paths` 의 레지스트리에 신뢰로 등록한다 — 처음 연 사람이 신뢰 확인에서 「신뢰」 를 고른 것과 같은
/// 저장 값이다(묻기 차례를 잡고 `set_trust(true)`). DB 시나리오(M3-21)가 쓴다: 미신뢰 프로젝트는 연결을 열지 못한다(결정 M3-D13).
pub fn register_trusted(
    paths: &pw_core::config::AppPaths,
    root: &std::path::Path,
) -> Result<pw_core::registry::Project, String> {
    let registry =
        pw_core::registry::Registry::new(paths.clone(), None, pw_core::EventBus::new(16));
    let project = registry
        .create(pw_core::registry::NewProject {
            root: root.to_path_buf(),
            ..pw_core::registry::NewProject::default()
        })
        .map_err(|e| format!("cannot register the project: {e}"))?;
    registry
        .claim_trust_prompt(&project.id)
        .map_err(|e| format!("cannot claim the trust prompt: {e}"))?;
    registry
        .set_trust(&project.id, true)
        .map_err(|e| format!("cannot record the project as trusted: {e}"))?;
    Ok(project)
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

    /// 시험마다 지우는 임시 폴더(이 크레이트는 `tempfile` 에 의존하지 않는다).
    struct Scratch(std::path::PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("pw-perf-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(dir.join("project")).unwrap();
            Self(dir)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn the_project_is_registered_as_if_the_trust_prompt_was_answered_untrusted() {
        use pw_core::config::AppPaths;
        use pw_core::registry::{NewProject, Registry, TrustState};

        let harness = Scratch::new("harness");
        let paths = AppPaths::under(&harness.0.join("app"));
        let project = register_untrusted(&paths, &harness.0.join("project")).unwrap();

        // 사람의 첫 열기: 창이 묻기 차례를 잡아(다이얼로그가 뜬다) 「신뢰하지 않음」 을 고른다(trust.tsx `answer(false)`).
        let person = Scratch::new("person");
        let person_paths = AppPaths::under(&person.0.join("app"));
        let registry = Registry::new(person_paths.clone(), None, pw_core::EventBus::new(16));
        let theirs = registry
            .create(NewProject {
                root: person.0.join("project"),
                ..NewProject::default()
            })
            .unwrap();
        assert!(registry.claim_trust_prompt(&theirs.id).unwrap());
        registry.set_trust(&theirs.id, false).unwrap();

        // 앱이 여는 레지스트리(새로 연다)에서 두 프로젝트가 같아 보인다: 미신뢰이고, 창이 다이얼로그를 다시 띄우지 않는다.
        for (paths, id) in [(&paths, &project.id), (&person_paths, &theirs.id)] {
            let app = Registry::new(paths.clone(), None, pw_core::EventBus::new(16));
            assert_eq!(app.trust(id).unwrap().state, TrustState::Untrusted);
            assert!(!app.is_trusted(id));
            assert!(
                !app.claim_trust_prompt(id).unwrap(),
                "the window would show the trust prompt"
            );
        }
    }

    #[test]
    fn a_trusted_registration_opens_without_a_prompt() {
        use pw_core::config::AppPaths;
        use pw_core::registry::{Registry, TrustState};

        let harness = Scratch::new("trusted");
        let paths = AppPaths::under(&harness.0.join("app"));
        let project = register_trusted(&paths, &harness.0.join("project")).unwrap();
        let app = Registry::new(paths.clone(), None, pw_core::EventBus::new(16));
        assert_eq!(app.trust(&project.id).unwrap().state, TrustState::Trusted);
        assert!(app.is_trusted(&project.id));
        assert!(
            !app.claim_trust_prompt(&project.id).unwrap(),
            "the window would show the trust prompt"
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
