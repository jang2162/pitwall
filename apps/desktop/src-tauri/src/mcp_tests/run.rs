//! 실행 구성 쓰기 툴(17-m6-plan M6-13)의 왕복: `run_config_upsert` · `run_config_delete` 를 앱과 같은 Host 에 진짜 MCP 세션으로
//! 부르고, 그 프로젝트 창(`mcp.approvals` 허브를 듣는 연결)이 승인에 답한다. 두 단계 · 입력 검증 · 비밀 · 가린 계획 · 추적 파일
//! 거절 자체는 `pw-run` 의 `config::tool::tests` 가 본다.

use std::time::Duration;

use pw_core::registry::plan::PlanChange;
use pw_core::registry::run_seen::{SeenBy, SeenPlace};
use pw_mcp::SessionRequest;
use pw_mcp::approval::{Connection, Listener, McpApprovalAnswer};
use pw_rpc::ConnectionScope;
use serde_json::{Value, json};

use super::{ToolHost, structured, text};

/// 그 프로젝트 창이 승인 요청 하나를 받아 `answer` 로 답한다(`db` 의 것과 같다 — 앞 요청의 거둠은 건너뛴다).
async fn window_answers(
    host: &ToolHost,
    listener: &mut Listener,
    project: &str,
    answer: McpApprovalAnswer,
) -> pw_mcp::approval::McpApprovalRequest {
    let request = loop {
        let event = tokio::time::timeout(Duration::from_secs(10), listener.recv())
            .await
            .expect("no approval request")
            .unwrap();
        if let pw_mcp::approval::McpApprovalEvent::Open { request } = event {
            break request;
        }
    };
    host.services
        .sessions
        .approvals()
        .answer(
            &pw_mcp::approval::Answerer {
                scope: &ConnectionScope::Project(project.to_owned()),
                connection: Connection::named("window"),
            },
            request.id,
            answer,
        )
        .unwrap();
    *request
}

fn listen(host: &ToolHost, project: &str) -> Listener {
    host.services
        .sessions
        .approvals()
        .listen(Some(project), None, Connection::named("window"))
}

/// 신뢰한 프로젝트 `alpha`.
fn trusted(host: &ToolHost) -> String {
    let alpha = host.project("alpha");
    host.host
        .registry()
        .unwrap()
        .set_trust(&alpha, true)
        .unwrap();
    alpha
}

fn dev(project: &str, command: &str) -> Value {
    json!({ "projectId": project, "id": "dev", "config": {
        "name": "Dev", "type": "shell", "command": command, "env": { "NODE_ENV": "env-value-5e1f" },
    }})
}

/// 둘 다 그룹 `run` 의 `write` 프로젝트 툴이고 계획을 만든다. 입력 스키마는 구성 스키마(`type` 마다의 필드 · `envFile`)를 싣고
/// 승인 표의 자리는 없다. 헤드리스 목록에 없는 것은 `mcp_tools_are_exactly_these` 가 본다(앱 전용 — 헤드리스 Host 는 실행 구성
/// 메서드를 붙이지 않는다).
#[test]
fn the_run_config_tools_are_project_write_tools_in_the_run_group() {
    let host = ToolHost::app();
    for (name, method) in [
        ("run_config_upsert", "run.tool.configUpsert"),
        ("run_config_delete", "run.tool.configDelete"),
    ] {
        let tool = host.tool(name);
        assert_eq!(
            (tool.group, tool.tier, tool.method),
            ("run", pw_rpc::Tier::Write, method)
        );
        assert!(tool.planned, "{name}");
        assert!(tool.project_scoped(), "{name}");
        let schema = Value::Object(tool.input_schema_for(false)).to_string();
        assert!(!schema.contains("pitwall/approval"), "{name}: {schema}");
    }
    let schema = Value::Object(host.tool("run_config_upsert").input_schema_for(false)).to_string();
    for word in ["envFile", "docker-compose", "mise-task", "profiles"] {
        assert!(schema.contains(word), "{word}: {schema}");
    }
}

/// `run_config_upsert` 를 그 프로젝트 창이 승인하면 파일이 생긴다 — 계획은 파일 diff 이고 감사 행에는 명령줄 원문 · env 값이
/// 없다. 그 승인은 「사람이 본 구성」 이 아니다(M6-13 ⑤ · M6-11 ⑥ 의 기록하지 않는 자리): 쓴 구성의 해시는 `human_saw` 가
/// 거짓이다. 대조로, 같은 해시를 그 프로젝트 창의 증거로 기록하면 참이 된다(판정하는 해시가 그 구성의 것임을 보인다).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_upserted_config_is_not_one_the_person_saw() {
    let host = ToolHost::app();
    let alpha = trusted(&host);
    let root = host.base.join("work/alpha");
    let session = host.session(SessionRequest::default()).await;
    let mut window = listen(&host, &alpha);
    let command = "echo cmd-literal-9b3a";
    let (saved, request) = tokio::join!(
        session.call("run_config_upsert", dev(&alpha, command)),
        window_answers(&host, &mut window, &alpha, McpApprovalAnswer::Once)
    );
    assert_ne!(saved.is_error, Some(true), "{}", text(&saved));
    let file = root.join(".pitwall/run/dev.toml");
    assert_eq!(
        structured(&saved)["file"],
        json!(file.display().to_string())
    );
    assert_eq!(request.tool, "run_config_upsert");
    assert_eq!(request.project_id.as_deref(), Some(alpha.as_str()));
    match &request.plan.changes[..] {
        [
            PlanChange::File {
                path,
                before: None,
                after: Some(after),
            },
        ] => {
            assert_eq!(path, &file.display().to_string());
            assert_eq!(std::fs::read_to_string(&file).unwrap(), *after);
        }
        other => panic!("{other:?}"),
    }
    let rows = host.audit_rows(1);
    let row = rows.iter().find(|r| r.tool == "run_config_upsert").unwrap();
    assert_eq!(row.outcome, "ok");
    for raw in ["cmd-literal-9b3a", "env-value-5e1f"] {
        assert!(
            !row.args.contains(raw),
            "`{raw}` is in the audit row: {}",
            row.args
        );
    }

    // 쓴 구성의 「사람이 본 구성」 해시.
    let registry = host.host.registry().unwrap();
    let project = registry.get(&alpha).unwrap();
    let place = pw_run::config::RunConfigPlace::new(&host.paths, &project, None).unwrap();
    let set = pw_run::config::read(&place);
    let config = &set.get("dev").expect("the written config is usable").config;
    let shell = vec!["/bin/zsh".to_owned(), "-l".to_owned()];
    let argv = pw_run::config::argv::argv(
        config,
        None,
        &pw_run::config::argv::ArgvContext {
            root: &place.root,
            cwd: &place.root,
            login_shell: &shell,
        },
    )
    .unwrap();
    let env = pw_run::env::RunEnv::resolve(config, None, &place.root).unwrap();
    let hash = pw_run::seen::hash(&pw_run::seen::SeenParts {
        key: "dev",
        root: &place.root,
        config,
        argv: &argv,
        cwd: &place.root,
        home: None,
        env: &env,
        definition: None,
    })
    .unwrap();
    assert!(
        !registry.human_saw(&alpha, "dev", &hash),
        "the upsert approval recorded the config as seen"
    );
    assert_eq!(registry.run_seen(&alpha, "dev").unwrap(), None);
    let by = SeenBy::window(
        &ConnectionScope::Project(alpha.clone()),
        &alpha,
        SeenPlace::Save,
    )
    .unwrap();
    registry
        .record_run_seen(&by, &[("dev".to_owned(), hash.clone())])
        .unwrap();
    assert!(registry.human_saw(&alpha, "dev", &hash), "the control");
    session.close().await;
}

/// 「이 세션 동안 허용」 은 그 구성 하나에 닿는다(메인 판정 2026-11-18 — `Remember::Key`): `dev` 에 준 허용 뒤 `dev` 를 다시
/// 고치는 것은 묻지 않고, 다른 구성 `api` 를 만드는 것 · `dev` 를 지우는 것(다른 툴)은 묻는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_session_allowance_reaches_only_that_config() {
    let host = ToolHost::app();
    let alpha = trusted(&host);
    let root = host.base.join("work/alpha");
    let session = host.session(SessionRequest::default()).await;
    let mut window = listen(&host, &alpha);
    let (first, request) = tokio::join!(
        session.call("run_config_upsert", dev(&alpha, "npm run dev")),
        window_answers(&host, &mut window, &alpha, McpApprovalAnswer::Session)
    );
    assert_ne!(first.is_error, Some(true), "{}", text(&first));
    assert!(request.answers.contains(&McpApprovalAnswer::Session));

    let again = tokio::time::timeout(
        Duration::from_secs(10),
        session.call("run_config_upsert", dev(&alpha, "npm run dev -- --host")),
    )
    .await
    .expect("the session allowance for `dev` asked again");
    assert_ne!(again.is_error, Some(true), "{}", text(&again));
    assert!(
        std::fs::read_to_string(root.join(".pitwall/run/dev.toml"))
            .unwrap()
            .contains("--host")
    );

    let mut api = dev(&alpha, "cargo run");
    api["id"] = json!("api");
    let (denied, request) = tokio::join!(
        session.call("run_config_upsert", api),
        window_answers(&host, &mut window, &alpha, McpApprovalAnswer::Deny)
    );
    assert_eq!(request.tool, "run_config_upsert");
    assert_eq!(denied.is_error, Some(true));
    assert!(!root.join(".pitwall/run/api.toml").exists());

    let (deleted, request) = tokio::join!(
        session.call(
            "run_config_delete",
            json!({ "projectId": alpha, "id": "dev" })
        ),
        window_answers(&host, &mut window, &alpha, McpApprovalAnswer::Once)
    );
    assert_ne!(deleted.is_error, Some(true), "{}", text(&deleted));
    assert_eq!(request.tool, "run_config_delete");
    assert!(!root.join(".pitwall/run/dev.toml").exists());
    session.close().await;
}
