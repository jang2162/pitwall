//! ⑤ 라이선스 · 출처 확인(4001)과 라이선스 덮어쓰기 표.

use crate::test_http::Reply;

use super::*;

const BIN: &str = "demo/bin/demo";

fn archive() -> Vec<u8> {
    targz(&[
        Entry::Dir("demo/"),
        Entry::Dir("demo/bin/"),
        Entry::File(BIN, b"#!/bin/sh\n".to_vec(), 0o755),
    ])
}

fn reg(version: &str, licenses: &[&str], path: &str, body: &[u8]) -> Arc<ToolsRegistry> {
    registry(
        vec![files_pkg(
            "demo",
            version,
            &[("demo.tar.gz", path)],
            &[("demo", BIN)],
            licenses,
        )],
        &[Row::file("demo", version, "demo.tar.gz", path, body)],
    )
}

fn confirmed_of(e: &RpcError) -> methods::ToolConfirmed {
    let d: methods::ToolConfirmRequired = serde_json::from_value(e.data.clone().unwrap()).unwrap();
    methods::ToolConfirmed {
        version: d.version,
        licenses: d.licenses,
        source_domains: d.source_domains,
    }
}

/// 확인 기록 없이 `tools.install` 은 4001 이고 아무것도 받지 않는다. 4001 의 값으로 다시 부르면 설치되고 기록이 남아, 다음 판(라이선스 ·
/// 출처 같음)은 묻지 않는다. 라이선스가 바뀐 판은 다시 4001 이다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn install_asks_once_and_again_when_the_license_changes() {
    let fx = Fx::new().await;
    let body = archive();
    fx.server.route("/demo-1.tar.gz", Reply::Body(body.clone()));
    let i = fx.installer(
        reg("1.0.0", &["MIT"], "/demo-1.tar.gz", &body),
        Limits::default(),
    );
    let err = i
        .install(&settings_grant(), "demo", None, None, false)
        .await
        .unwrap_err();
    assert_eq!(err.code, code::CONFIRM_REQUIRED);
    let data = err.data.clone().unwrap();
    assert_eq!(data["version"], "1.0.0");
    assert_eq!(data["licenses"], json!(["MIT"]));
    assert_eq!(data["sourceDomains"], json!(["fake.test"]));
    assert_eq!(data["size"], json!(body.len()));
    assert_eq!(data["rejected"], Value::Null);
    assert_eq!(
        fx.server.total(),
        0,
        "nothing is downloaded before the confirmation"
    );
    assert!(!fx.tools().exists() || fx.layout().read_record("demo").unwrap().is_none());

    let ok = i
        .install(
            &settings_grant(),
            "demo",
            None,
            Some(&confirmed_of(&err)),
            false,
        )
        .await
        .unwrap();
    assert_eq!(ok.version, "1.0.0");
    let entry = ConsentStore::new(&fx.state()).get("demo").unwrap();
    assert_eq!(entry.licenses, vec!["MIT".to_owned()]);
    assert_eq!(entry.source_domains, vec!["fake.test".to_owned()]);

    // 판만 바뀐 것은 묻지 않는다.
    let body2 = archive();
    fx.server
        .route("/demo-2.tar.gz", Reply::Body(body2.clone()));
    let i2 = fx.installer(
        reg("2.0.0", &["MIT"], "/demo-2.tar.gz", &body2),
        Limits::default(),
    );
    i2.install(&settings_grant(), "demo", None, None, true)
        .await
        .unwrap();

    // 라이선스가 바뀐 판은 다시 묻는다(업데이트도).
    fx.server
        .route("/demo-3.tar.gz", Reply::Body(body2.clone()));
    let i3 = fx.installer(
        reg("3.0.0", &["GPL-3.0-only"], "/demo-3.tar.gz", &body2),
        Limits::default(),
    );
    let hits = fx.server.total();
    let err = i3
        .install(&settings_grant(), "demo", None, None, true)
        .await
        .unwrap_err();
    assert_eq!(err.code, code::CONFIRM_REQUIRED);
    assert_eq!(
        err.data.as_ref().unwrap()["licenses"],
        json!(["GPL-3.0-only"])
    );
    assert_eq!(fx.server.total(), hits);
    // tools.updates 도 「다시 묻는다」 를 알린다.
    let updates = methods::updates_of(&i3).unwrap();
    assert_eq!(updates.items.len(), 1);
    assert!(updates.items[0].confirm_required);
    assert_eq!(updates.items[0].available, "3.0.0");
}

/// 다이얼로그를 띄운 뒤 판 · 라이선스 · 출처가 바뀌었으면(옛 값으로 다시 부르면) 다시 4001(`rejected: changed`)이고 받지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stale_confirmation_is_asked_again() {
    let fx = Fx::new().await;
    let body = archive();
    fx.server.route("/demo-1.tar.gz", Reply::Body(body.clone()));
    fx.server.route("/demo-2.tar.gz", Reply::Body(body.clone()));
    let old = fx.installer(
        reg("1.0.0", &["MIT"], "/demo-1.tar.gz", &body),
        Limits::default(),
    );
    let shown = confirmed_of(
        &old.install(&settings_grant(), "demo", None, None, false)
            .await
            .unwrap_err(),
    );
    let changes: Vec<(&str, Arc<ToolsRegistry>)> = vec![
        ("version", reg("2.0.0", &["MIT"], "/demo-2.tar.gz", &body)),
        (
            "license",
            reg("1.0.0", &["MIT", "Apache-2.0"], "/demo-1.tar.gz", &body),
        ),
        (
            "source",
            registry(
                vec![json!({
                    "name": "demo", "description": "t", "homepage": "https://e.test", "licenses": ["MIT"],
                    "languages": [], "categories": [],
                    "source": {"id": "pkg:generic/test/demo@1.0.0", "download": {"files": {"demo.tar.gz": "https://elsewhere.test/demo-1.tar.gz"}}},
                    "bin": {"demo": BIN},
                })],
                &[],
            ),
        ),
    ];
    for (label, r) in changes {
        let now = fx.installer(r, Limits::default());
        let err = now
            .install(&settings_grant(), "demo", None, Some(&shown), false)
            .await
            .expect_err(label);
        assert_eq!(err.code, code::CONFIRM_REQUIRED, "{label}");
        assert_eq!(err.data.as_ref().unwrap()["rejected"], "changed", "{label}");
    }
    assert_eq!(fx.server.total(), 0);
    assert!(ConsentStore::new(&fx.state()).get("demo").is_none());
}

/// 확인 기록 파일이 깨졌으면 확인 없음으로 본다 — 묻지 않고 설치하지 않는다. 다음 확인이 파일을 새로 쓴다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_corrupt_consent_file_asks_again() {
    let fx = Fx::new().await;
    let body = archive();
    fx.server.route("/demo-1.tar.gz", Reply::Body(body.clone()));
    let i = fx.installer(
        reg("1.0.0", &["MIT"], "/demo-1.tar.gz", &body),
        Limits::default(),
    );
    fx.consent(&i, "demo");
    let path = ConsentStore::new(&fx.state()).path().to_path_buf();
    for broken in [
        &b"{not json"[..],
        br#"{"schemaVersion": 99, "packages": {}}"#,
    ] {
        std::fs::write(&path, broken).unwrap();
        let err = i
            .install(&settings_grant(), "demo", None, None, false)
            .await
            .unwrap_err();
        assert_eq!(err.code, code::CONFIRM_REQUIRED);
        assert_eq!(fx.server.total(), 0);
    }
    let err = i
        .install(&settings_grant(), "demo", None, None, false)
        .await
        .unwrap_err();
    i.install(
        &settings_grant(),
        "demo",
        None,
        Some(&confirmed_of(&err)),
        false,
    )
    .await
    .unwrap();
    assert!(ConsentStore::new(&fx.state()).get("demo").is_some());
}

/// kotlin-lsp 의 확인 값(내장 스냅숏 · 덮어쓰기 표)에 덮어쓰기 표의 라이선스가 들어 있다 — Mason 정의의 `Apache-2.0` 만이 아니다.
#[test]
fn kotlin_lsp_is_confirmed_with_the_license_note() {
    let installer = ToolsInstaller::new(Arc::new(ToolsRegistry::new()));
    let index = installer.registry().index();
    let package = index.get("kotlin-lsp").unwrap();
    assert_eq!(package.licenses, vec!["Apache-2.0".to_owned()]);
    let resolved = package
        .resolve(
            Some(ToolTarget::DarwinArm64),
            installer.registry().overrides().get("kotlin-lsp"),
        )
        .unwrap();
    let licenses = installer.notes().effective("kotlin-lsp", &package.licenses);
    let rows = installer.registry().pinned().pinned_for(&resolved);
    let ask = installer.confirm_values(&resolved, &licenses, rows.as_deref());
    assert_eq!(
        ask.licenses,
        vec![
            "Apache-2.0".to_owned(),
            "LicenseRef-JetBrains-proprietary-components".to_owned()
        ]
    );
    assert!(ask.note.as_deref().unwrap().contains("JetBrains"));
    assert_eq!(
        ask.source_domains,
        vec!["download.jetbrains.com".to_owned()]
    );
    assert_eq!(ask.checksum, ToolChecksum::Pinned);
    assert!(ask.size.unwrap() > 300_000_000);
    // 다른 패키지는 Mason 의 것 그대로.
    assert_eq!(
        installer.notes().effective("ruff", &["MIT".to_owned()]),
        vec!["MIT".to_owned()]
    );
}

/// 묶인 덮어쓰기 표는 읽히고, 표의 패키지는 모두 번들 패키지다(오타로 조용히 안 먹는 일이 없게).
#[test]
fn the_builtin_license_notes_parse_and_name_bundled_packages() {
    let notes = LicenseNotes::builtin();
    let pinned = Pinned::builtin();
    assert!(notes.get("kotlin-lsp").is_some());
    for name in notes.names() {
        assert!(pinned.is_bundled(name), "{name} is not a bundled package");
    }
    assert!(
        LicenseNotes::parse("[x]\nlicenses = []\nnote = \"\"\nsource = \"\"\nchecked = \"\"\n")
            .is_err()
    );
    assert!(LicenseNotes::parse("[x]\nlicenses = [\"MIT\"]\nunknown = 1\n").is_err());
}
