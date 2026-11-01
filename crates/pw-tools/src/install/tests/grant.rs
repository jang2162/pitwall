//! ⑥(권한) · ⑧ 파일 창 · MCP · 웰컴 창 연결의 설치 RPC 는 `PERMISSION_DENIED` 이고 아무것도 받지 않는다. 바꾸는 RPC 는 tier exec,
//! 어느 것도 MCP 툴이 아니다.

use std::time::Duration;

use pw_rpc::{Caller, ConnectionScope, EventHub, Role, Router, Session, code as rpc_code};

use crate::test_http::Reply;

use super::methods::*;
use super::*;

const BIN: &str = "demo/bin/demo";

#[test]
fn only_the_settings_window_and_project_windows_get_a_grant() {
    let settings = |l: &str| l == "settings";
    assert_eq!(
        tool_grant(&ConnectionScope::Host, Some("settings"), &settings)
            .unwrap()
            .project(),
        None
    );
    assert_eq!(
        tool_grant(
            &ConnectionScope::Project("p1".into()),
            Some("project-p1"),
            &settings
        )
        .unwrap()
        .project(),
        Some("p1")
    );
    let file = Caller::file(Role::Owner, PathBuf::from("/tmp/a.txt")).scope;
    for (label, scope, window, reason) in [
        ("file window", file, Some("file-1"), "fileWindow"),
        ("mcp session", ConnectionScope::Host, None, "noWindow"),
        (
            "welcome window",
            ConnectionScope::Host,
            Some("welcome"),
            "notSettingsWindow",
        ),
    ] {
        let e = tool_grant(&scope, window, &settings).expect_err(label);
        assert_eq!(e.code, rpc_code::PERMISSION_DENIED, "{label}");
        assert_eq!(e.data.unwrap()["reason"], reason, "{label}");
    }
}

async fn call(
    router: &Arc<Router>,
    caller: Caller,
    method: &str,
    params: Value,
) -> Result<Value, RpcError> {
    let (session, mut rx) = Session::new(Arc::clone(router), EventHub::new(), caller);
    session.handle_value(json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}));
    let response = tokio::time::timeout(Duration::from_secs(10), rx.recv())
        .await
        .unwrap()
        .unwrap()
        .into_response()
        .unwrap();
    response.outcome
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn file_mcp_and_welcome_connections_cannot_install() {
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
    fx.consent(&installer, "demo");
    let mut router = Router::new();
    register(&mut router, Arc::clone(&installer));
    let router = Arc::new(router);
    let callers = [
        (
            "file window",
            Caller::file(Role::Owner, fx.tmp.path().join("a.txt")).in_window("file-1"),
        ),
        // MCP 세션은 창 없는 Host 연결이다(`pw-mcp` 의 `Caller::new(Role::Owner)`).
        ("mcp session", Caller::new(Role::Owner)),
        (
            "welcome window",
            Caller::new(Role::Owner).in_window("welcome"),
        ),
    ];
    let calls = [
        ("tools.install", json!({"name": "demo"})),
        ("tools.update", json!({"name": "demo"})),
        ("tools.rollback", json!({"name": "demo"})),
        ("tools.uninstall", json!({"name": "demo"})),
    ];
    for (label, caller) in &callers {
        for (method, params) in &calls {
            let e = call(&router, caller.clone(), method, params.clone())
                .await
                .expect_err(label);
            assert_eq!(
                e.code,
                rpc_code::PERMISSION_DENIED,
                "{label} {method}: {e:?}"
            );
        }
    }
    assert_eq!(fx.server.total(), 0);
    assert!(fx.layout().read_record("demo").unwrap().is_none());

    // 모바일 · 읽기 전용 role 은 tier 로 막힌다.
    for role in [Role::Mobile, Role::Readonly] {
        let caller = Caller {
            role,
            scope: ConnectionScope::Host,
            window: Some("settings".into()),
        };
        let e = call(&router, caller, "tools.install", json!({"name": "demo"}))
            .await
            .unwrap_err();
        assert_eq!(e.code, rpc_code::PERMISSION_DENIED, "{role:?}");
    }

    // 설정 창은 설치한다.
    let ok = call(
        &router,
        Caller::new(Role::Owner).in_window("settings"),
        "tools.install",
        json!({"name": "demo"}),
    )
    .await
    .unwrap();
    assert_eq!(ok["version"], "1.0.0");
    // 목록은 누구나 읽는다(Read).
    let list = call(
        &router,
        Caller::new(Role::Readonly),
        "tools.list",
        json!(null),
    )
    .await
    .unwrap();
    assert_eq!(list["items"][0]["name"], "demo");
    assert_eq!(list["items"][0]["checksum"], "pinned");
}

#[test]
fn changing_methods_are_exec_and_none_is_an_mcp_tool() {
    use pw_rpc::{Method as _, Tier};
    for meta in [
        ToolsInstallMethod::META,
        ToolsUpdateMethod::META,
        ToolsRollbackMethod::META,
        ToolsUninstallMethod::META,
    ] {
        assert_eq!(meta.tier, Tier::Exec, "{}", meta.name);
        assert!(!meta.mcp, "{}", meta.name);
    }
    for meta in [
        ToolsListMethod::META,
        ToolsUpdatesMethod::META,
        ToolsLogMethod::META,
    ] {
        assert_eq!(meta.tier, Tier::Read, "{}", meta.name);
        assert!(!meta.mcp, "{}", meta.name);
    }
}
