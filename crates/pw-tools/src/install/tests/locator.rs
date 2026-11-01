//! ⑦(해석 연결) 설치본 찾기: 설치 기록의 지금 판만 고르고, 만드는 중인 판 · 깨진 설치본은 고르지 않는다.

use pw_lsp_servers::{Located, ToolLocator};

use crate::test_http::Reply;

use super::*;

const BIN: &str = "demo/bin/demo";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_locator_picks_the_current_version_and_skips_unfinished_or_broken_ones() {
    let fx = Fx::new().await;
    let body = targz(&[
        Entry::Dir("demo/"),
        Entry::Dir("demo/bin/"),
        Entry::File(BIN, b"#!/bin/sh\n".to_vec(), 0o755),
    ]);
    fx.server.route("/demo.tar.gz", Reply::Body(body.clone()));
    let installer = fx.installer(
        registry(
            vec![files_pkg(
                "demo",
                "1.0.0",
                &[("demo.tar.gz", "/demo.tar.gz")],
                &[("demo", BIN)],
                &["MIT"],
            )],
            &[Row::file(
                "demo",
                "1.0.0",
                "demo.tar.gz",
                "/demo.tar.gz",
                &body,
            )],
        ),
        Limits::default(),
    );
    let locator = installer.locator().unwrap();
    assert_eq!(locator.locate("demo", "demo"), Located::Absent);
    fx.consent(&installer, "demo");
    installer
        .install(&settings_grant(), "demo", None, None, false)
        .await
        .unwrap();
    let dir = fx.layout().version_dir("demo", "1.0.0");
    assert_eq!(
        locator.locate("demo", "demo"),
        Located::Installed {
            path: dir.join(BIN),
            version: "1.0.0".into()
        }
    );
    assert_eq!(locator.locate("demo", "other-bin"), Located::Absent);
    assert_eq!(locator.locate("../demo", "demo"), Located::Absent);

    // 만드는 중(표지)인 폴더는 고르지 않는다.
    std::fs::write(dir.join(layout::INSTALLING_MARK), b"").unwrap();
    assert!(matches!(
        locator.locate("demo", "demo"),
        Located::Broken { .. }
    ));
    std::fs::remove_file(dir.join(layout::INSTALLING_MARK)).unwrap();

    // 기록에 아직 없는 판(설치 중)은 보이지 않는다.
    let next = fx.layout().version_dir("demo", "2.0.0");
    std::fs::create_dir_all(next.join("demo/bin")).unwrap();
    std::fs::write(next.join(BIN), b"").unwrap();
    assert!(
        matches!(locator.locate("demo", "demo"), Located::Installed { version, .. } if version == "1.0.0")
    );

    // venv 를 만든 python 이 사라졌으면 「다시 설치 필요」.
    let mut record = fx.layout().read_record("demo").unwrap().unwrap();
    record.versions.get_mut("1.0.0").unwrap().python = Some(layout::UsedRuntime {
        path: fx.tmp.path().join("gone/python3"),
        version: Some("3.12.1".into()),
    });
    fx.layout().write_record("demo", &record).unwrap();
    match locator.locate("demo", "demo") {
        Located::Broken { reason } => assert!(reason.contains("python"), "{reason}"),
        other => panic!("{other:?}"),
    }
    let list = methods::list_of(&installer).unwrap();
    assert!(list.items[0].problem.as_deref().unwrap().contains("python"));

    // 깨진 기록.
    std::fs::write(fx.layout().record_path("demo"), b"{").unwrap();
    assert!(matches!(
        locator.locate("demo", "demo"),
        Located::Broken { .. }
    ));
}
