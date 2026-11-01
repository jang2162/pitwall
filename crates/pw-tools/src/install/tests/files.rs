//! ② 리다이렉트 · ③ 크기 상한 · ④ 고정 해시 · ⑥ 동시 설치 · ⑦ 업데이트 취소 · 되돌리기, 설치 기록 · 배치.

use std::time::Duration;

use crate::test_http::Reply;

use super::*;

const BIN: &str = "demo/bin/demo";

fn demo_archive(marker: &str) -> Vec<u8> {
    targz(&[
        Entry::Dir("demo/"),
        Entry::Dir("demo/bin/"),
        Entry::File(
            BIN,
            format!("#!/bin/sh\necho {marker}\n").into_bytes(),
            0o755,
        ),
    ])
}

/// 판 하나의 registry(고정 해시 포함 또는 없음).
fn demo_registry(version: &str, path: &str, body: &[u8], pinned: bool) -> Arc<ToolsRegistry> {
    let rows = if pinned {
        vec![Row::file("demo", version, "demo.tar.gz", path, body)]
    } else {
        Vec::new()
    };
    registry(
        vec![files_pkg(
            "demo",
            version,
            &[("demo.tar.gz", path)],
            &[("demo", BIN)],
            &["MIT"],
        )],
        &rows,
    )
}

async fn install(installer: &Arc<ToolsInstaller>) -> Result<methods::ToolInstallResult, RpcError> {
    installer
        .install(&settings_grant(), "demo", None, None, false)
        .await
}

/// 설치 기록에 판 · 출처 · 받은 파일 해시 · 체크섬 유무 · 라이선스 · 크기 · 때가 남고, 실행 링크 · 설치본 찾기가 그 판을 가리킨다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_install_records_its_source_hash_license_size_and_links_the_bin() {
    let fx = Fx::new().await;
    let body = demo_archive("v1");
    fx.server.route("/demo-1.tar.gz", Reply::Body(body.clone()));
    let installer = fx.installer(
        demo_registry("1.0.0", "/demo-1.tar.gz", &body, true),
        Limits::default(),
    );
    fx.consent(&installer, "demo");
    let before = now_ms();
    let result = install(&installer).await.unwrap();
    assert_eq!(result.outcome, InstallOutcome::Installed);
    assert_eq!(result.checksum, ToolChecksum::Pinned);
    assert!(result.task_id.is_some());
    let record = fx.layout().read_record("demo").unwrap().unwrap();
    assert_eq!(record.current, "1.0.0");
    assert_eq!(record.previous, None);
    let entry = record.current_entry().unwrap();
    assert_eq!(entry.source_urls, vec![format!("{FAKE}/demo-1.tar.gz")]);
    assert_eq!(entry.files[0].sha256, sha256_hex(&body));
    assert_eq!(entry.files[0].size, body.len() as u64);
    assert_eq!(entry.checksum, ToolChecksum::Pinned);
    assert_eq!(entry.licenses, vec!["MIT".to_owned()]);
    assert!(entry.size > 0);
    assert!(entry.installed_at_ms >= before);
    assert_eq!(entry.bins.get("demo").map(String::as_str), Some(BIN));
    #[cfg(unix)]
    {
        let link = fx.tools().join("bin").join("demo");
        assert_eq!(
            std::fs::canonicalize(&link).unwrap(),
            std::fs::canonicalize(fx.layout().version_dir("demo", "1.0.0").join(BIN)).unwrap()
        );
    }
    assert!(
        !fx.layout()
            .version_dir("demo", "1.0.0")
            .join(layout::INSTALLING_MARK)
            .exists()
    );

    // 같은 판을 다시 부르면 받지 않는다.
    let again = install(&installer).await.unwrap();
    assert_eq!(again.outcome, InstallOutcome::AlreadyInstalled);
    assert_eq!(fx.server.hits("/demo-1.tar.gz"), 1);
}

/// 고정 해시도 registry 체크섬도 없는 패키지는 설치되고 「체크섬 없음」 이 기록 · 확인 값에 남는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn without_a_pinned_hash_the_record_says_no_checksum() {
    let fx = Fx::new().await;
    let body = demo_archive("v1");
    fx.server.route("/demo-1.tar.gz", Reply::Body(body.clone()));
    let installer = fx.installer(
        demo_registry("1.0.0", "/demo-1.tar.gz", &body, false),
        Limits::default(),
    );
    let ask = installer
        .install(&settings_grant(), "demo", None, None, false)
        .await
        .expect_err("asks first");
    assert_eq!(ask.code, code::CONFIRM_REQUIRED);
    assert_eq!(ask.data.as_ref().unwrap()["checksum"], "none");
    assert_eq!(ask.data.as_ref().unwrap()["size"], Value::Null);
    fx.consent(&installer, "demo");
    let result = install(&installer).await.unwrap();
    assert_eq!(result.checksum, ToolChecksum::None);
    let record = fx.layout().read_record("demo").unwrap().unwrap();
    assert_eq!(record.current_entry().unwrap().checksum, ToolChecksum::None);
}

/// ④ 받은 파일이 고정 해시와 다르면 설치 실패이고 폴더 · 기록이 남지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_download_that_differs_from_the_pinned_hash_fails() {
    let fx = Fx::new().await;
    let pinned_body = demo_archive("v1");
    let served = demo_archive("evil");
    fx.server.route("/demo-1.tar.gz", Reply::Body(served));
    let installer = fx.installer(
        demo_registry("1.0.0", "/demo-1.tar.gz", &pinned_body, true),
        Limits::default(),
    );
    fx.consent(&installer, "demo");
    let err = install(&installer).await.unwrap_err();
    assert_eq!(
        code_and_stage(&err),
        (code::INSTALL_FAILED, Some("checksum".into()))
    );
    assert!(!fx.layout().version_dir("demo", "1.0.0").exists());
    assert!(fx.layout().read_record("demo").unwrap().is_none());
    assert!(!fx.tools().join("bin").join("demo").exists());
}

/// ② HTTP 로 리다이렉트되면 실패(받기 전에 멈춘다)하고 폴더가 남지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_redirect_to_http_fails() {
    let fx = Fx::new().await;
    let body = demo_archive("v1");
    let port = fx.server.base.rsplit(':').next().unwrap().to_owned();
    fx.server.route("/real.tar.gz", Reply::Body(body.clone()));
    fx.server.route(
        "/demo-1.tar.gz",
        Reply::Redirect(format!("http://localhost:{port}/real.tar.gz")),
    );
    let installer = fx.installer(
        demo_registry("1.0.0", "/demo-1.tar.gz", &body, true),
        Limits::default(),
    );
    fx.consent(&installer, "demo");
    let err = install(&installer).await.unwrap_err();
    assert_eq!(
        code_and_stage(&err),
        (code::INSTALL_FAILED, Some("download".into()))
    );
    assert!(err.message.contains("non-https"), "{}", err.message);
    assert_eq!(fx.server.hits("/real.tar.gz"), 0);
    assert!(!fx.layout().version_dir("demo", "1.0.0").exists());
}

/// ③ 받기 상한 + 1 바이트(크기를 알리는 답 · 알리지 않는 답)와 풀린 크기 상한 + 1 바이트에서 멈추고 폴더가 지워진다. 상한과 같으면
/// 설치된다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_size_limits_stop_one_byte_over_and_remove_the_folder() {
    let body = demo_archive("v1");
    let n = body.len() as u64;
    // 풀린 바이트: 파일 하나의 내용 길이.
    let unpacked = "#!/bin/sh\necho v1\n".len() as u64;
    let cases = [
        (
            "sized download",
            Reply::Body(body.clone()),
            Limits {
                download: n - 1,
                unpacked: 1 << 20,
            },
            Some("download"),
        ),
        (
            "unsized download",
            Reply::Unsized(body.clone()),
            Limits {
                download: n - 1,
                unpacked: 1 << 20,
            },
            Some("download"),
        ),
        (
            "unpacked",
            Reply::Body(body.clone()),
            Limits {
                download: n,
                unpacked: unpacked - 1,
            },
            Some("unpack"),
        ),
        (
            "exact",
            Reply::Unsized(body.clone()),
            Limits {
                download: n,
                unpacked,
            },
            None,
        ),
    ];
    for (label, reply, limits, stage) in cases {
        let fx = Fx::new().await;
        fx.server.route("/demo-1.tar.gz", reply);
        let installer = fx.installer(
            demo_registry("1.0.0", "/demo-1.tar.gz", &body, true),
            limits,
        );
        fx.consent(&installer, "demo");
        let result = install(&installer).await;
        match stage {
            Some(stage) => {
                let err = result.expect_err(label);
                assert_eq!(
                    code_and_stage(&err),
                    (code::INSTALL_FAILED, Some(stage.into())),
                    "{label}: {err:?}"
                );
                assert!(
                    !fx.layout().version_dir("demo", "1.0.0").exists(),
                    "{label}"
                );
                assert!(
                    fx.layout().read_record("demo").unwrap().is_none(),
                    "{label}"
                );
            }
            None => {
                result.expect(label);
            }
        }
    }
}

/// ⑥ 같은 패키지를 두 창(설정 창 · 프로젝트 창)이 동시에 설치하면 한 번만 받고 다른 하나는 그 결과(같은 작업)를 받는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_windows_installing_at_once_download_once() {
    let fx = Fx::new().await;
    let body = demo_archive("v1");
    fx.server.route(
        "/demo-1.tar.gz",
        Reply::Trickle(body.clone(), Duration::from_millis(150)),
    );
    let installer = fx.installer(
        demo_registry("1.0.0", "/demo-1.tar.gz", &body, true),
        Limits::default(),
    );
    fx.consent(&installer, "demo");
    let a = {
        let i = Arc::clone(&installer);
        tokio::spawn(async move {
            i.install(&settings_grant(), "demo", None, None, false)
                .await
        })
    };
    // 첫 설치가 받기 시작한 뒤에 두 번째 창이 부른다.
    for _ in 0..200 {
        if fx.server.hits("/demo-1.tar.gz") > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let b = installer
        .install(&project_grant("p1"), "demo", None, None, false)
        .await
        .unwrap();
    let a = a.await.unwrap().unwrap();
    assert_eq!(fx.server.hits("/demo-1.tar.gz"), 1);
    assert_eq!(a.task_id, b.task_id);
    assert!(!a.joined);
    assert!(b.joined);
    assert_eq!(a.outcome, InstallOutcome::Installed);
    assert_eq!(b.outcome, InstallOutcome::Installed);
}

/// ⑥ 다른 앱 프로세스(같은 `$DATA` 를 쓰는 다른 설치기)는 잠금 파일을 기다린 뒤 이미 설치된 것을 보고 받지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn another_process_waits_for_the_lock_and_does_not_download_again() {
    let fx = Fx::new().await;
    let body = demo_archive("v1");
    fx.server.route(
        "/demo-1.tar.gz",
        Reply::Trickle(body.clone(), Duration::from_millis(150)),
    );
    let first = fx.installer(
        demo_registry("1.0.0", "/demo-1.tar.gz", &body, true),
        Limits::default(),
    );
    let second = fx.installer(
        demo_registry("1.0.0", "/demo-1.tar.gz", &body, true),
        Limits::default(),
    );
    fx.consent(&first, "demo");
    let a = {
        let i = Arc::clone(&first);
        tokio::spawn(async move {
            i.install(&settings_grant(), "demo", None, None, false)
                .await
        })
    };
    for _ in 0..200 {
        if fx.server.hits("/demo-1.tar.gz") > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let b = second
        .install(&settings_grant(), "demo", None, None, false)
        .await
        .unwrap();
    let a = a.await.unwrap().unwrap();
    assert_eq!(a.outcome, InstallOutcome::Installed);
    assert_eq!(b.outcome, InstallOutcome::AlreadyInstalled);
    assert_eq!(fx.server.hits("/demo-1.tar.gz"), 1);
}

/// ⑦ 업데이트 중 취소하면 새 판의 폴더가 지워지고 지금 판 · 실행 링크는 그대로다. 그 뒤 업데이트를 끝내면 이전 판 하나를 남기고,
/// `tools.rollback` 이 이전 판으로 링크를 돌린다(되돌린 판에서 다시 업데이트하면 받지 않고 링크만 돌린다).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancelling_an_update_keeps_the_old_link_and_rollback_switches_back() {
    let fx = Fx::new().await;
    let v1 = demo_archive("v1");
    let v2 = demo_archive("v2");
    fx.server.route("/demo-1.tar.gz", Reply::Body(v1.clone()));
    let first = fx.installer(
        demo_registry("1.0.0", "/demo-1.tar.gz", &v1, true),
        Limits::default(),
    );
    fx.consent(&first, "demo");
    install(&first).await.unwrap();

    // 2.0.0 — 천천히 받는 동안 취소한다.
    let big: Vec<u8> = {
        // 받는 데 시간이 걸리게 내용을 키운다(풀린 것도 작다 — 받기 중에 취소된다).
        let mut entries = vec![
            Entry::Dir("demo/"),
            Entry::Dir("demo/bin/"),
            Entry::File(BIN, b"#!/bin/sh\necho v2\n".to_vec(), 0o755),
        ];
        entries.push(Entry::File("demo/pad", vec![7u8; 200_000], 0o644));
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::none());
        let raw = {
            let t = targz(&entries);
            let mut d = flate2::read::GzDecoder::new(t.as_slice());
            let mut out = Vec::new();
            std::io::Read::read_to_end(&mut d, &mut out).unwrap();
            out
        };
        std::io::Write::write_all(&mut gz, &raw).unwrap();
        gz.finish().unwrap()
    };
    let _ = v2;
    fx.server.route(
        "/demo-2.tar.gz",
        Reply::Trickle(big.clone(), Duration::from_millis(20)),
    );
    let second = fx.installer(
        demo_registry("2.0.0", "/demo-2.tar.gz", &big, true),
        Limits::default(),
    );
    let update = {
        let i = Arc::clone(&second);
        tokio::spawn(async move { i.install(&settings_grant(), "demo", None, None, true).await })
    };
    let task = loop {
        if let Some((_, (_, id))) = second.running().into_iter().next()
            && fx.server.hits("/demo-2.tar.gz") > 0
        {
            break id;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    };
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(fx.tasks.cancel(task));
    let err = update.await.unwrap().unwrap_err();
    assert_eq!(err.code, code::CANCELLED, "{err:?}");
    assert!(!fx.layout().version_dir("demo", "2.0.0").exists());
    let record = fx.layout().read_record("demo").unwrap().unwrap();
    assert_eq!(record.current, "1.0.0");
    #[cfg(unix)]
    assert!(
        std::fs::read_link(fx.tools().join("bin/demo"))
            .unwrap()
            .to_string_lossy()
            .contains("1.0.0")
    );

    // 끝까지 업데이트한다.
    fx.server.route("/demo-2.tar.gz", Reply::Body(big.clone()));
    let done = second
        .install(&settings_grant(), "demo", None, None, true)
        .await
        .unwrap();
    assert_eq!(
        (done.version.as_str(), done.previous.as_deref()),
        ("2.0.0", Some("1.0.0"))
    );
    assert!(
        fx.layout().version_dir("demo", "1.0.0").is_dir(),
        "the previous version is kept"
    );
    #[cfg(unix)]
    assert!(
        std::fs::read_link(fx.tools().join("bin/demo"))
            .unwrap()
            .to_string_lossy()
            .contains("2.0.0")
    );

    let back = second.rollback(&settings_grant(), "demo").await.unwrap();
    assert_eq!(
        (back.current.as_str(), back.previous.as_deref()),
        ("1.0.0", Some("2.0.0"))
    );
    #[cfg(unix)]
    assert!(
        std::fs::read_link(fx.tools().join("bin/demo"))
            .unwrap()
            .to_string_lossy()
            .contains("1.0.0")
    );
    let locator = second.locator().unwrap();
    assert_eq!(
        pw_lsp_servers::ToolLocator::locate(&locator, "demo", "demo"),
        pw_lsp_servers::Located::Installed {
            path: fx.layout().version_dir("demo", "1.0.0").join(BIN),
            version: "1.0.0".into()
        }
    );

    let hits = fx.server.hits("/demo-2.tar.gz");
    let switched = second
        .install(&settings_grant(), "demo", None, None, true)
        .await
        .unwrap();
    assert_eq!(switched.outcome, InstallOutcome::Switched);
    assert_eq!(fx.server.hits("/demo-2.tar.gz"), hits);
    let record = fx.layout().read_record("demo").unwrap().unwrap();
    assert_eq!(
        (record.current.as_str(), record.previous.as_deref()),
        ("2.0.0", Some("1.0.0"))
    );
}

/// 업데이트를 거듭해도 판은 둘(지금 · 이전)만 남는다. 되돌릴 판이 없으면 4006, 제거하면 기록 · 폴더 · 링크가 사라진다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn only_two_versions_stay_and_uninstall_removes_everything() {
    let fx = Fx::new().await;
    let mut last = None;
    for v in ["1.0.0", "2.0.0", "3.0.0"] {
        let body = demo_archive(v);
        let path = format!("/demo-{v}.tar.gz");
        fx.server.route(&path, Reply::Body(body.clone()));
        let i = fx.installer(demo_registry(v, &path, &body, true), Limits::default());
        fx.consent(&i, "demo");
        i.install(&settings_grant(), "demo", None, None, false)
            .await
            .unwrap();
        last = Some(i);
    }
    let i = last.unwrap();
    assert!(!fx.layout().version_dir("demo", "1.0.0").exists());
    assert!(fx.layout().version_dir("demo", "2.0.0").is_dir());
    assert!(fx.layout().version_dir("demo", "3.0.0").is_dir());
    let record = fx.layout().read_record("demo").unwrap().unwrap();
    assert_eq!(
        record.versions.keys().cloned().collect::<Vec<_>>(),
        vec!["2.0.0", "3.0.0"]
    );

    i.uninstall(&settings_grant(), "demo").await.unwrap();
    assert!(!fx.layout().package_dir("demo").exists());
    assert!(!fx.tools().join("bin/demo").exists());
    let err = i.rollback(&settings_grant(), "demo").await.unwrap_err();
    assert_eq!(err.code, code::NOT_INSTALLED);
    let err = i.uninstall(&settings_grant(), "demo").await.unwrap_err();
    assert_eq!(err.code, code::NOT_INSTALLED);
}
