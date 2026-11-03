//! 덮어쓰기 정의의 런처(15-m4-plan.md M4-18 — jdtls 의 registry 정의는 python 런처라 Pitwall 이 셸 · cmd 런처를 둔다).

use crate::test_http::Reply;

use super::*;

/// registry 정의는 `python:` 런처(설치기가 만들지 않는다 — 4004), 보관 파일에는 `extra` 항목을 더 넣는다.
fn launcher_registry(body: &[u8], with_override: bool) -> Arc<ToolsRegistry> {
    let package = files_pkg(
        "demo",
        "1.0.0",
        &[("demo.tar.gz", "/demo.tar.gz")],
        &[("demo", "python:bin/demo.py")],
        &["MIT"],
    );
    let rows = [Row::file(
        "demo",
        "1.0.0",
        "demo.tar.gz",
        "/demo.tar.gz",
        body,
    )];
    let over = "reason = \"test\"\n[bin]\ndemo = \"pitwall/demo\"\n\
                [launcher.\"pitwall/demo\"]\nsh = \"#!/bin/sh\\necho launched from \\\"$0\\\"\\n\"\n\
                cmd = \"@echo launched\\r\\n\"\n";
    let overrides: &[(&str, &str)] = if with_override {
        &[("demo", over)]
    } else {
        &[]
    };
    registry_with_overrides(vec![package], &rows, overrides)
}

fn archive(extra: Option<&'static str>) -> Vec<u8> {
    let mut entries = vec![
        Entry::Dir("bin/"),
        Entry::File("bin/demo.py", b"print('python launcher')\n".to_vec(), 0o644),
    ];
    if let Some(path) = extra {
        entries.push(Entry::File(path, b"from the archive\n".to_vec(), 0o755));
    }
    targz(&entries)
}

/// 런처가 설치 폴더에 놓이고 실행 파일이 그것을 가리킨다(유닉스는 0755 셸 스크립트). 덮어쓰기가 없으면 python 런처라 4004.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_override_launcher_is_written_and_becomes_the_bin() {
    let fx = Fx::new().await;
    let body = archive(None);
    fx.server.route("/demo.tar.gz", Reply::Body(body.clone()));

    // 확인 값을 내기 전에(해석에서) 거절된다.
    let plain = fx.installer(launcher_registry(&body, false), Limits::default());
    let refused = plain
        .install(&settings_grant(), "demo", None, None, false)
        .await
        .unwrap_err();
    assert_eq!(refused.code, code::UNSUPPORTED, "{refused:?}");

    let installer = fx.installer(launcher_registry(&body, true), Limits::default());
    fx.consent(&installer, "demo");
    let result = installer
        .install(&settings_grant(), "demo", None, None, false)
        .await
        .unwrap();
    assert_eq!(result.outcome, InstallOutcome::Installed);
    let record = fx.layout().read_record("demo").unwrap().unwrap();
    let rel = if cfg!(windows) {
        "pitwall/demo.cmd"
    } else {
        "pitwall/demo"
    };
    assert_eq!(
        record
            .current_entry()
            .unwrap()
            .bins
            .get("demo")
            .map(String::as_str),
        Some(rel)
    );
    let launcher = fx.layout().version_dir("demo", "1.0.0").join(rel);
    let text = std::fs::read_to_string(&launcher).unwrap();
    #[cfg(unix)]
    {
        assert!(text.starts_with("#!/bin/sh\n"), "{text}");
        assert_eq!(mode_of(&launcher), 0o755);
        let out = std::process::Command::new(fx.tools().join("bin").join("demo"))
            .output()
            .unwrap();
        assert!(
            String::from_utf8_lossy(&out.stdout).starts_with("launched from"),
            "{out:?}"
        );
    }
    #[cfg(windows)]
    assert!(text.starts_with("@echo launched"), "{text}");
}

/// 받은 보관 파일에 런처와 같은 경로가 있으면 덮어쓰지 않고 설치가 실패한다(설치 폴더는 남지 않는다).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_launcher_does_not_overwrite_a_file_from_the_archive() {
    let fx = Fx::new().await;
    let clash = if cfg!(windows) {
        "pitwall/demo.cmd"
    } else {
        "pitwall/demo"
    };
    let body = archive(Some(clash));
    fx.server.route("/demo.tar.gz", Reply::Body(body.clone()));
    let installer = fx.installer(launcher_registry(&body, true), Limits::default());
    fx.consent(&installer, "demo");
    let e = installer
        .install(&settings_grant(), "demo", None, None, false)
        .await
        .unwrap_err();
    assert_eq!(
        code_and_stage(&e),
        (code::INSTALL_FAILED, Some("bin".into())),
        "{e:?}"
    );
    assert!(!fx.layout().version_dir("demo", "1.0.0").exists());
    assert!(fx.layout().read_record("demo").unwrap().is_none());
}
