//! 다른 머신 체크아웃의 실행(18-m7-plan M7-21 — 원격 실행 · 결정 M7-D9 · D57 ⑤). 에이전트가 `checkout: { machine }` 으로 묶음의 다른
//! 머신에 실행 툴을 부른다. 다른 머신은 **이 앱과 같은 조립의 진짜 Host**(`ToolHost::app` — 그 머신의 레지스트리 · 신뢰 · 「사람이 본
//! 구성」 기록 · 실행 툴의 `agent_run_grant` 가 실제 규칙대로 돈다)이고, 이 머신에도 같은 이름 · 같은 구성을 두어 엉뚱한 머신에서 돌면
//! 드러나게 했다. 묶음 · 머신 이름 · 링크는 Desktop 이 꽂는 [`Machines`] 의 가짜이고, 연결은 그 머신 Host 에 `Agent` 범위로 연다
//! (Relay 가 링크로 받은 에이전트 연결과 같은 범위).
//!
//! 공격자 입장: 그 머신의 「사람이 본 구성」(정책 자동 — M6-10)이나 이 머신의 `auto_approve` 로 다른 머신 실행을 묻지 않고 지나가기,
//! 머신 허용으로 사람이 띄운 실행 · 미신뢰 프로젝트를 지나가기, 이 머신에서 사람이 본 구성으로 다른 머신 실행을 지나가기.

use std::sync::{Arc, Mutex};

use pw_core::BoxFuture;
use pw_core::registry::bundle::{LOCAL_MACHINE, ProjectBundle, ProjectBundleMember};
use pw_mcp::SessionRequest;
use pw_mcp::approval::McpApprovalAnswer;
use pw_mcp::checkout::{MachineError, MachineLink, Machines};
use pw_rpc::{Caller, Role};
use serde_json::json;

use super::run_exec::{
    enable, hub_answers, last_approval, listen, ok, sleeper, trusted, window_starts,
};
use super::{ToolHost, ToolSession};
use crate::mcp;

const MINI: &str = "SHA256:mini";
const BUNDLE: &str = "b-00000000000000aa";

/// Desktop 이 꽂는 것의 가짜: 묶음 하나(이 머신 `alpha` + mini 의 `alpha`), 별명, 그 머신 Host 로의 에이전트 연결.
struct FakeMachines {
    mini: Arc<pw_core::LocalHost>,
    bundle: ProjectBundle,
    connects: Mutex<usize>,
}

impl Machines for FakeMachines {
    fn bundle(&self, project_id: &str) -> Result<Option<ProjectBundle>, String> {
        let b = &self.bundle;
        Ok((project_id == b.id || b.local_project() == Some(project_id)).then(|| b.clone()))
    }

    fn bundles(&self) -> Result<Vec<ProjectBundle>, String> {
        Ok(vec![self.bundle.clone()])
    }

    fn machine_id(&self, name: &str) -> Option<String> {
        matches!(name, "mini" | MINI).then(|| MINI.to_owned())
    }

    fn machine_name(&self, machine: &str) -> Option<String> {
        (machine == MINI).then(|| "mini".to_owned())
    }

    fn connect<'a>(&'a self, machine: &'a str) -> BoxFuture<'a, Result<MachineLink, MachineError>> {
        Box::pin(async move {
            use pw_core::Host;
            assert_eq!(machine, MINI);
            *self.connects.lock().unwrap() += 1;
            let connection = self
                .mini
                .connect(Caller::agent(Role::Owner))
                .await
                .map_err(|e| MachineError::Unreachable(e.to_string()))?;
            Ok(MachineLink {
                connection,
                hold: Box::new(()),
            })
        })
    }
}

/// 이 머신(Desktop)과 다른 머신 `mini`. 두 머신 모두 `alpha` 프로젝트에 `dev` 구성이 있다.
struct Pair {
    desktop: ToolHost,
    mini: ToolHost,
    alpha: String,
    mini_alpha: String,
}

impl Pair {
    /// `desktop_global` · `mini_global` 은 각 머신 전역 설정의 실행 툴 켜기 뒤에 붙는 줄. `mini_trusted` 는 그 머신의 신뢰.
    fn new(desktop_global: &str, mini_global: &str, mini_trusted: bool) -> Self {
        let mut desktop = ToolHost::app();
        let mini = ToolHost::app();
        enable(&desktop, desktop_global);
        enable(&mini, mini_global);
        let (alpha, root) = trusted(&desktop, "alpha");
        sleeper(&root, "dev", "here");
        let (mini_alpha, mini_root) = trusted(&mini, "alpha");
        mini.host
            .registry()
            .unwrap()
            .set_trust(&mini_alpha, mini_trusted)
            .unwrap();
        sleeper(&mini_root, "dev", "mini");
        let member = |machine: &str, project: &str| ProjectBundleMember {
            machine: machine.into(),
            project_id: project.into(),
            last_target: None,
        };
        let machines = Arc::new(FakeMachines {
            mini: Arc::clone(&mini.host),
            bundle: ProjectBundle {
                id: BUNDLE.into(),
                name: "alpha".into(),
                favorite: false,
                tags: Vec::new(),
                opened_at: None,
                implicit: false,
                members: vec![member(LOCAL_MACHINE, &alpha), member(MINI, &mini_alpha)],
            },
            connects: Mutex::new(0),
        });
        // 앱처럼 같은 등록부(승인 허브)를 쓰고 다른 머신 부품을 꽂는다(`mcp::services_with`).
        desktop.services = mcp::services_with(
            &desktop.host,
            &desktop.paths,
            "test",
            pw_mcp::HostKind::App,
            desktop.services.sessions.clone(),
            Some(machines as Arc<dyn Machines>),
        );
        Self {
            desktop,
            mini,
            alpha,
            mini_alpha,
        }
    }

    async fn session(&self) -> ToolSession {
        self.desktop.session(SessionRequest::default()).await
    }

    /// `mini` 의 `alpha` 에서 `dev` 를 띄운다.
    async fn start_on_mini(&self, session: &ToolSession) -> rmcp::model::CallToolResult {
        session
            .call(
                "run_start",
                json!({ "projectId": self.alpha, "configId": "dev", "checkout": { "machine": "mini" } }),
            )
            .await
    }

    /// 그 머신의 그 프로젝트 실행 수(끝난 것 포함 — `run.list`).
    async fn runs(host: &ToolHost, project: &str) -> usize {
        host.window_call(project, "run.list", json!({ "projectId": project }))
            .await
            .unwrap()["runs"]
            .as_array()
            .unwrap()
            .len()
    }
}

/// 300 ms 안에 새 승인 요청이 열렸나(앞 요청이 닫힌 알림은 세지 않는다).
async fn asked(window: &mut pw_mcp::approval::Listener) -> bool {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(300);
    loop {
        match tokio::time::timeout_at(deadline, window.recv()).await {
            Ok(Some(pw_mcp::approval::McpApprovalEvent::Open { .. })) => return true,
            Ok(Some(_)) => {}
            Ok(None) | Err(_) => return false,
        }
    }
}

/// 결정 M7-D9 · D57 ⑤: 사람이 `mini` 에서 화면으로 한 번 돌린 구성(그 머신 `state.db` 의 「사람이 본 구성」 — 그 머신의 계획은
/// `policy_auto`)과 이 머신 · 그 머신의 `auto_approve = ["exec", "dangerous"]` 가 있어도, 에이전트의 `mini` 실행은 이 머신 창에 묻는다
/// — 승인 요청에 그 머신이 실린다. 대조: 이 머신에서 사람이 본 구성은 묻지 않는다(정책 자동).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn policy_auto_and_auto_approve_do_not_reach_a_run_on_another_machine() {
    let auto = "[mcp]\nauto_approve = [\"write\", \"exec\", \"dangerous\"]";
    let pair = Pair::new(auto, auto, true);
    window_starts(&pair.mini, &pair.mini_alpha, "dev").await;
    window_starts(&pair.desktop, &pair.alpha, "dev").await;
    let session = pair.session().await;
    let mut window = listen(&pair.desktop, &pair.alpha);

    // 대조: 이 머신은 사람이 본 구성이라 묻지 않는다.
    let here = session
        .call(
            "run_start",
            json!({ "projectId": pair.alpha, "configId": "dev" }),
        )
        .await;
    ok(&here);
    assert_eq!(
        last_approval(&pair.desktop, 1).await.as_deref(),
        Some("policy")
    );

    let mini_before = Pair::runs(&pair.mini, &pair.mini_alpha).await;
    let (denied, request) = tokio::join!(
        pair.start_on_mini(&session),
        hub_answers(
            &pair.desktop,
            &mut window,
            &pair.alpha,
            McpApprovalAnswer::Deny
        )
    );
    assert_eq!(denied.is_error, Some(true));
    assert_eq!(request.tool, "run_start");
    assert_eq!(request.machine.as_deref(), Some(MINI));
    // 거절했으니 그 머신에서 돌지 않았다.
    assert_eq!(Pair::runs(&pair.mini, &pair.mini_alpha).await, mini_before);

    // 한 번 허용하면 그 머신에서 돈다 — 이 머신의 같은 구성이 아니다.
    let here_before = Pair::runs(&pair.desktop, &pair.alpha).await;
    let (ran, _) = tokio::join!(
        pair.start_on_mini(&session),
        hub_answers(
            &pair.desktop,
            &mut window,
            &pair.alpha,
            McpApprovalAnswer::Once
        )
    );
    ok(&ran);
    assert_eq!(
        Pair::runs(&pair.mini, &pair.mini_alpha).await,
        mini_before + 1
    );
    assert_eq!(Pair::runs(&pair.desktop, &pair.alpha).await, here_before);
    pw_run::runs::project_closed(&pair.alpha);
    pw_run::runs::project_closed(&pair.mini_alpha);
    session.close().await;
}

/// D57 ⑤: 「이 세션 동안 이 머신 허용」 은 그 머신의 에이전트 실행에만 닿는다 — 그 머신에서 **사람이 띄운** 실행은 에이전트가
/// 다시 띄우지 못한다. `run_restart` 는 워크트리를 받지 않는 툴이라 `checkout` 을 받지 않고(M7-18 ⑧), `runId` 만으로는 이 머신의
/// 실행을 찾는다 — 그 머신의 같은 번호 실행에 닿지 않는다. 묻지도 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_machine_allowance_does_not_reach_a_persons_run_on_that_machine() {
    let pair = Pair::new("", "", true);
    let human = window_starts(&pair.mini, &pair.mini_alpha, "dev").await;
    let session = pair.session().await;
    let mut window = listen(&pair.desktop, &pair.alpha);

    let (first, request) = tokio::join!(
        pair.start_on_mini(&session),
        hub_answers(
            &pair.desktop,
            &mut window,
            &pair.alpha,
            McpApprovalAnswer::Machine
        )
    );
    ok(&first);
    assert!(
        request.answers.contains(&McpApprovalAnswer::Machine),
        "{:?}",
        request.answers
    );
    assert_eq!(request.machine.as_deref(), Some(MINI));
    // 머신 허용 뒤 같은 머신의 에이전트 실행은 묻지 않는다(대조).
    let again = pair.start_on_mini(&session).await;
    ok(&again);
    let mini_runs = Pair::runs(&pair.mini, &pair.mini_alpha).await;

    for params in [
        json!({ "projectId": pair.alpha, "runId": human, "checkout": { "machine": "mini" } }),
        json!({ "projectId": pair.alpha, "runId": human }),
    ] {
        let restart = session.call("run_restart", params.clone()).await;
        assert_eq!(
            restart.is_error,
            Some(true),
            "{params}: {}",
            super::text(&restart)
        );
    }
    // 어느 쪽도 묻지 않았고 그 머신의 사람 실행을 다시 띄우지 않았다.
    assert!(
        !asked(&mut window).await,
        "an approval was asked for a person's run"
    );
    assert_eq!(Pair::runs(&pair.mini, &pair.mini_alpha).await, mini_runs);
    pw_run::runs::project_closed(&pair.mini_alpha);
    session.close().await;
}

/// D57 ⑤ · M7-14: 그 머신에서 미신뢰인 프로젝트의 실행은 이 머신의 `auto_approve` 가 있어도 돌지 않는다 — 그 머신이 계획 단계에서
/// `untrusted` 로 거절해 승인을 묻지도 않는다(신뢰는 그 머신의 것 — 03 「원격」, 이 머신에서는 신뢰한 같은 이름 프로젝트다).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_untrusted_project_on_another_machine_does_not_run() {
    let pair = Pair::new(
        "[mcp]\nauto_approve = [\"write\", \"exec\", \"dangerous\"]",
        "",
        false,
    );
    let session = pair.session().await;
    let mut window = listen(&pair.desktop, &pair.alpha);
    let ran = pair.start_on_mini(&session).await;
    assert_eq!(ran.is_error, Some(true), "{}", super::text(&ran));
    assert!(
        super::text(&ran).contains("untrusted") || super::text(&ran).contains("trust"),
        "{}",
        super::text(&ran)
    );
    assert!(
        !asked(&mut window).await,
        "asked about an untrusted project's run"
    );
    assert_eq!(Pair::runs(&pair.mini, &pair.mini_alpha).await, 0);
    assert_eq!(Pair::runs(&pair.desktop, &pair.alpha).await, 0);
    session.close().await;
}
