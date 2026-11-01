//! 망가진 · 악의적 서버에서 Host 메모리가 상한 안인가(M4-01 리뷰 H1 · H2), 허가가 루트 · 신뢰를 넘지 않는가(리뷰 M1 · M2).
//!
//! 메모리는 이 시험 프로세스의 RSS(`ps -o rss=`)로 잰다 — nextest 는 시험마다 프로세스를 따로 띄우므로 다른 시험이 섞이지 않는다.
//! 리뷰의 재현은 줄바꿈 없는 stderr 400 MB 에서 RSS 10 → 394 MiB, stdin 을 읽지 않는 서버의 4 KB-id 요청 10만 개에서 10 → 798 MiB
//! 였다.

mod common;

use std::time::Duration;

use common::{project, received};
use pw_lsp::fake::{Action, Scenario};
use pw_lsp::{InitializeOptions, RequestError};
use serde_json::Value;
// 메모리 시험(RSS — `ps`)은 unix 에서만 돈다.
#[cfg(unix)]
use {
    common::next_event,
    pw_lsp::{ExitReason, RequestOptions, ServerEvent},
    serde_json::json,
};

/// 이 프로세스의 RSS(MiB).
#[cfg(unix)]
fn rss_mib() -> u64 {
    let out = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse::<u64>()
        .unwrap()
        / 1024
}

/// 리뷰 H1: 줄바꿈 없는 stderr 400 MB 뒤에도 RSS 가 늘지 않고, 로그에는 줄 상한만큼만 남고, 서버는 계속 답한다.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_server_pouring_stderr_without_newlines_keeps_host_memory_flat() {
    let p = project();
    let scenario = Scenario::default().on(
        "flood/stderr",
        vec![
            Action::StderrFlood {
                bytes: 400_000_000,
                newline: false,
            },
            Action::Notify {
                method: "flood/done".into(),
                params: json!({}),
            },
        ],
    );
    let mut f = p.fake("stderr-flood", scenario, false).await;
    f.server
        .initialize(InitializeOptions::new(&p.root))
        .await
        .unwrap();
    let before = rss_mib();
    f.server.notify_raw("flood/stderr", json!({})).unwrap();
    next_event(&mut f.events, Duration::from_secs(120), |e| match e {
        ServerEvent::Notification { method, .. } if method == "flood/done" => Some(()),
        _ => None,
    })
    .await
    .expect("the fake server finished pouring");
    let after = rss_mib();
    assert!(
        after <= before + 32,
        "RSS grew from {before} MiB to {after} MiB"
    );
    // 서버는 그대로 답한다.
    let answer = f
        .server
        .request_raw("shutdown", Value::Null, RequestOptions::default())
        .await;
    assert_eq!(answer, Ok(Value::Null));
    f.server.shutdown().await;
    let read_log = || -> String {
        std::fs::read_dir(&f.log_dir)
            .unwrap()
            .map(|e| std::fs::read_to_string(e.unwrap().path()).unwrap())
            .collect()
    };
    // 「잘림」 줄은 stderr 의 끝(서버가 끝남)을 읽은 뒤에 쓰인다.
    common::eventually(Duration::from_secs(5), || {
        read_log().contains("more bytes dropped")
    })
    .await;
    let log = read_log();
    assert!(log.len() < 64 * 1024, "the log holds {} bytes", log.len());
    assert!(
        log.contains("more bytes dropped"),
        "{}",
        &log[..log.len().min(400)]
    );
}

/// 리뷰 H2(재현 그대로): stdin 을 읽지 않으면서 4 KB-id 서버 요청 10만 개를 쏟는 서버 — 답이 송신 큐 상한에 닿으면 그 서버를 끝내고,
/// RSS 는 상한 근처에 머문다.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_server_that_stops_reading_and_floods_requests_is_ended() {
    let p = project();
    let scenario = Scenario::default().on(
        "flood/requests",
        vec![
            Action::Hang,
            Action::Flood {
                method: "window/workDoneProgress/create".into(),
                count: 100_000,
                id_bytes: 4000,
            },
        ],
    );
    let mut f = p.fake("request-flood", scenario, false).await;
    f.server
        .initialize(InitializeOptions::new(&p.root))
        .await
        .unwrap();
    let before = rss_mib();
    f.server.notify_raw("flood/requests", json!({})).unwrap();
    let exit = next_event(&mut f.events, Duration::from_secs(120), |e| match e {
        ServerEvent::Exited(exit) => Some(exit),
        _ => None,
    })
    .await
    .expect("the server was ended");
    let after = rss_mib();
    assert!(
        matches!(&exit.reason, ExitReason::Killed(why) if why.contains("stopped reading its input")),
        "{exit:?}"
    );
    let cap = (pw_lsp::server::MAX_QUEUED_BYTES >> 20) as u64;
    assert!(
        after <= before + cap + 48,
        "RSS grew from {before} MiB to {after} MiB (queue cap {cap} MiB)"
    );
}

/// 리뷰 H2(현실 경로): 멈춰 stdin 을 읽지 않는 서버에 문서 바뀜을 계속 보내면 송신 큐 상한에서 그 서버를 끝내고 오류를 돌려준다.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn notifications_to_a_server_that_stopped_reading_end_it_at_the_queue_cap() {
    let p = project();
    let scenario = Scenario::default().on("fake/hang", vec![Action::Hang]);
    let mut f = p.fake("stuck", scenario, false).await;
    f.server
        .initialize(InitializeOptions::new(&p.root))
        .await
        .unwrap();
    f.server.notify_raw("fake/hang", json!({})).unwrap();
    // 가짜 서버가 `fake/hang` 을 읽고 멈출 틈.
    assert!(
        common::eventually(Duration::from_secs(5), || {
            received(&f.record)
                .iter()
                .any(|m| m.get("method").and_then(Value::as_str) == Some("fake/hang"))
        })
        .await
    );
    let before = rss_mib();
    let text = "x".repeat(1024 * 1024);
    let mut sent = 0;
    let refused = loop {
        let params = json!({ "contentChanges": [{ "text": text }] });
        match f.server.notify_raw("textDocument/didChange", params) {
            Ok(()) => sent += 1,
            Err(e) => break e,
        }
        assert!(sent < 1000, "the queue never filled");
    };
    assert!(
        matches!(&refused, RequestError::Closed(why) if why.contains("stopped reading its input")),
        "{refused:?}"
    );
    // 상한(64 MiB)은 잡은 메모리로 센다 — 1 MiB 알림 하나가 직렬화 버퍼로 2 MiB 까지 잡으므로 서른 개 남짓, 파이프에 들어간 만큼
    // 몇 개 더. 상한이 없으면 1000 에 닿는다.
    assert!((25..=80).contains(&sent), "sent {sent} notifications");
    let after = rss_mib();
    assert!(
        after <= before + 64 + 48,
        "RSS grew from {before} MiB to {after} MiB"
    );
    let exit = next_event(&mut f.events, Duration::from_secs(5), |e| match e {
        ServerEvent::Exited(exit) => Some(exit),
        _ => None,
    })
    .await
    .unwrap();
    assert!(matches!(exit.reason, ExitReason::Killed(_)), "{exit:?}");
}

/// 리뷰 M1: 허가의 루트 밖 작업 디렉터리로는 띄우지 않는다(프로세스가 뜨지 않는다), 루트 밖 작업 공간으로는 `initialize` 를 보내지
/// 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_grant_launches_only_inside_its_root() {
    let p = project();
    let outside = p.base.join("elsewhere-untrusted");
    std::fs::create_dir_all(&outside).unwrap();
    let record = p.base.join("outside.jsonl");
    let scenario = Scenario {
        record: Some(record.clone()),
        start: vec![Action::Stderr {
            text: "started".into(),
        }],
        ..Scenario::default()
    };
    let path = scenario.write(&p.base, "outside").unwrap();
    let spawned = pw_lsp::LanguageServer::spawn(
        &p.grant,
        common::fake_spec(&outside, &path),
        pw_lsp::ServerOptions::default(),
    )
    .await;
    let e = spawned.unwrap_err();
    assert_eq!(e.kind(), std::io::ErrorKind::PermissionDenied, "{e}");
    #[cfg(unix)]
    {
        // 루트 안의 링크로 밖을 가리켜도 밖이다.
        let link = p.root.join("escape");
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        let e = pw_lsp::LanguageServer::spawn(
            &p.grant,
            common::fake_spec(&link, &path),
            pw_lsp::ServerOptions::default(),
        )
        .await
        .unwrap_err();
        assert_eq!(e.kind(), std::io::ErrorKind::PermissionDenied);
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!record.exists(), "a server process started");

    // 루트 안에서 띄운 서버라도 작업 공간 루트가 밖이면 `initialize` 를 보내지 않는다.
    let f = p.fake("inside", Scenario::default(), false).await;
    let refused = f
        .server
        .initialize(InitializeOptions::new(&outside))
        .await
        .unwrap_err();
    assert!(matches!(refused, RequestError::Refused(_)), "{refused:?}");
    assert!(
        !received(&f.record)
            .iter()
            .any(|m| m.get("method").and_then(Value::as_str) == Some("initialize"))
    );
    let inner = p.root.join("member");
    std::fs::create_dir_all(&inner).unwrap();
    f.server
        .initialize(InitializeOptions::new(&inner))
        .await
        .unwrap();
    f.server.shutdown().await;
}

/// 리뷰 M2: 신뢰를 거둔 뒤에는 쥐고 있던 허가로 띄울 수 없다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_held_grant_cannot_spawn_after_trust_is_revoked() {
    let p = project();
    let record = p.base.join("revoked.jsonl");
    let scenario = Scenario {
        record: Some(record.clone()),
        start: vec![Action::Stderr {
            text: "started".into(),
        }],
        ..Scenario::default()
    };
    let path = scenario.write(&p.base, "revoked").unwrap();
    p.projects.set_trust(p.grant.project_id(), false).unwrap();
    let e = pw_lsp::LanguageServer::spawn(
        &p.grant,
        common::fake_spec(&p.root, &path),
        pw_lsp::ServerOptions::default(),
    )
    .await
    .unwrap_err();
    assert_eq!(e.kind(), std::io::ErrorKind::PermissionDenied, "{e}");
    assert!(e.to_string().contains("no longer trusted"), "{e}");
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!record.exists(), "a server process started");
    // 다시 신뢰하면 같은 허가로 뜬다(허가 자체가 상한 것이 아니라 그때의 신뢰를 본다).
    p.projects.set_trust(p.grant.project_id(), true).unwrap();
    let server = pw_lsp::LanguageServer::spawn(
        &p.grant,
        common::fake_spec(&p.root, &path),
        pw_lsp::ServerOptions::default(),
    )
    .await
    .unwrap();
    server.shutdown().await;
}
