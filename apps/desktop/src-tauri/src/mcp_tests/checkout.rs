//! 세션 툴 `checkout_list` · `checkout_send`(18-m7-plan M7-19)의 왕복 — 에이전트 입장의 공격 시험.
//!
//! 두 머신 모두 **앱과 같은 진짜 Host**(`ToolHost::app` — git 쓰기 실행기 · 레지스트리 · `agent_grant` · `git.send.*` 의 범위 판정이
//! 실제 규칙대로 돈다)다. 다른 머신(`mini`, 필요하면 `box`)은 Desktop 이 꽂는 `Machines` 의 가짜가 묶음 · 별명 · 링크를 알고, 연결은
//! 그 Host 에 `Agent` 범위로 연다(실제 Desktop 구현과 같은 `HostRouter::connect`). 승인은 이 머신 프로젝트 창이 `mcp.approvals` 허브로
//! 듣고 답한다. 대상 머신의 미저장 확인은 그 머신 프로젝트 창의 연결(`Project` 범위)이 `git.prompts` 를 듣고 「진행」 으로 답한다 —
//! 듣지 않으면 실제처럼 `2006` 이다.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use pw_core::registry::NewProject;
use pw_core::registry::bundle::{LOCAL_MACHINE, ProjectBundle, ProjectBundleMember};
use pw_core::{BoxFuture, Host, HostRouter, Target};
use pw_git::testing::TestRepo;
use pw_mcp::approval::{
    Answerer, Connection, Listener, McpApprovalAnswer, McpApprovalEvent, McpApprovalRequest,
};
use pw_mcp::audit::Outcome;
use pw_mcp::checkout::{LinkState, MachineError, MachineLink, Machines};
use pw_mcp::marker::{Mark, Reason};
use pw_mcp::sessions::Sessions;
use pw_mcp::{HostKind, SessionRequest};
use pw_rpc::{Caller, ConnectionScope, Role};
use rmcp::model::CallToolResult;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::{ToolHost, ToolSession, structured, text};
use crate::mcp;

const MINI: &str = "SHA256:mini+/=";
const BOX: &str = "SHA256:box";
const BUNDLE: &str = "b-00000000000000aa";

/// 다른 머신 하나: 진짜 Host 와 그 머신의 저장소 · 프로젝트.
struct Machine {
    tools: ToolHost,
    repo: Arc<TestRepo>,
    project: String,
}

impl Machine {
    fn new(repo: TestRepo, trusted: bool) -> Self {
        let tools = ToolHost::app();
        let project = register(&tools, &repo, trusted);
        Self {
            tools,
            repo: Arc::new(repo),
            project,
        }
    }

    fn branch(&self, name: &str) -> Option<String> {
        let out = self.repo.try_git_in(
            self.repo.root(),
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("refs/heads/{name}"),
            ],
        );
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
    }
}

fn register(host: &ToolHost, repo: &TestRepo, trusted: bool) -> String {
    let registry = host.host.registry().unwrap();
    let id = registry
        .create(NewProject {
            root: repo.root().to_path_buf(),
            ..NewProject::default()
        })
        .unwrap()
        .id;
    registry.set_trust(&id, trusted).unwrap();
    id
}

/// `src` 를 받은 새 저장소(원격 설정 없이).
fn clone_of(src: &TestRepo) -> TestRepo {
    let dst = TestRepo::new();
    dst.git(&["pull", "--quiet", src.root().to_str().unwrap(), "main"]);
    dst
}

/// 연결로 보내는 프레임을 보고 한 번 무언가를 하는 sink(원본에서 `git.send.prepare` 가 나가기 직전 — ⑤).
struct OnSend {
    inner: Box<dyn pw_core::FrameSink>,
    method: &'static str,
    hook: Arc<Mutex<Option<Box<dyn FnOnce() + Send>>>>,
}

impl pw_core::FrameSink for OnSend {
    fn send(&self, frame: Value) {
        if frame["method"] == json!(self.method)
            && let Some(hook) = self.hook.lock().unwrap().take()
        {
            hook();
        }
        self.inner.send(frame);
    }
}

/// Desktop 이 꽂는 것의 가짜: 묶음 하나(이 머신 + mini [+ box]), 별명, 연결 상태, 링크.
struct FakeMachines {
    router: Arc<HostRouter>,
    bundle: ProjectBundle,
    state: Mutex<LinkState>,
    connects: AtomicUsize,
    refuse: Mutex<Option<MachineError>>,
    before_prepare: Arc<Mutex<Option<Box<dyn FnOnce() + Send>>>>,
}

impl Machines for FakeMachines {
    fn bundle(&self, project_id: &str) -> Result<Option<ProjectBundle>, String> {
        Ok(
            (project_id == self.bundle.id || self.bundle.local_project() == Some(project_id))
                .then(|| self.bundle.clone()),
        )
    }

    fn bundles(&self) -> Result<Vec<ProjectBundle>, String> {
        Ok(vec![self.bundle.clone()])
    }

    fn machine_id(&self, name: &str) -> Option<String> {
        match name {
            "mini" | MINI => Some(MINI.into()),
            "box" | BOX => Some(BOX.into()),
            _ => None,
        }
    }

    fn machine_name(&self, machine: &str) -> Option<String> {
        match machine {
            MINI => Some("mini".into()),
            BOX => Some("box".into()),
            _ => None,
        }
    }

    fn connect<'a>(&'a self, machine: &'a str) -> BoxFuture<'a, Result<MachineLink, MachineError>> {
        Box::pin(async move {
            if let Some(refused) = self.refuse.lock().unwrap().clone() {
                return Err(refused);
            }
            self.connects.fetch_add(1, Ordering::SeqCst);
            let connection = self
                .router
                .connect(
                    &Target::HostId(machine.to_owned()),
                    Caller::agent(Role::Owner),
                )
                .await
                .map_err(|e| MachineError::Unreachable(e.to_string()))?;
            let connection = pw_core::Connection {
                sink: Box::new(OnSend {
                    inner: connection.sink,
                    method: "git.send.prepare",
                    hook: Arc::clone(&self.before_prepare),
                }),
                source: connection.source,
            };
            Ok(MachineLink {
                connection,
                hold: Box::new(()),
            })
        })
    }

    fn link_state<'a>(&'a self, _machine: &'a str) -> BoxFuture<'a, LinkState> {
        Box::pin(async move { *self.state.lock().unwrap() })
    }
}

/// 다른 머신의 Host 를 라우터에 그 머신 id 로 붙인다(Desktop 에서는 붙은 원격 Host 가 그 머신 id 로 더해진다 — M7-08).
struct Named {
    id: String,
    inner: Arc<pw_core::LocalHost>,
}

impl Host for Named {
    fn id(&self) -> &str {
        &self.id
    }

    fn connect(
        &self,
        caller: Caller,
    ) -> BoxFuture<'_, Result<pw_core::Connection, pw_core::HostError>> {
        self.inner.connect(caller)
    }

    fn is_remote(&self) -> bool {
        true
    }
}

/// 묶음 하나: 이 머신(원본 저장소) + mini(그 저장소를 받은 것) [+ box].
struct Bundle {
    here: ToolHost,
    sessions: Sessions,
    src: TestRepo,
    local: String,
    mini: Machine,
    other: Option<Machine>,
    machines: Arc<FakeMachines>,
    /// 이 머신 프로젝트 창의 승인 듣기.
    window: Listener,
}

impl Bundle {
    fn new(mini_trusted: bool) -> Self {
        Self::with(mini_trusted, false, "")
    }

    fn with(mini_trusted: bool, with_box: bool, global: &str) -> Self {
        let src = TestRepo::with_commit();
        let mini = Machine::new(clone_of(&src), mini_trusted);
        let other = with_box.then(|| Machine::new(clone_of(&src), true));
        let here = ToolHost::app();
        here.global_settings(global);
        let local = register(&here, &src, true);
        let router = Arc::new(HostRouter::new());
        let mut members = vec![
            ProjectBundleMember {
                machine: LOCAL_MACHINE.into(),
                project_id: local.clone(),
                last_target: None,
            },
            ProjectBundleMember {
                machine: MINI.into(),
                project_id: mini.project.clone(),
                last_target: None,
            },
        ];
        router
            .add_host(Arc::new(Named {
                id: MINI.into(),
                inner: Arc::clone(&mini.tools.host),
            }))
            .unwrap();
        if let Some(other) = &other {
            members.push(ProjectBundleMember {
                machine: BOX.into(),
                project_id: other.project.clone(),
                last_target: None,
            });
            router
                .add_host(Arc::new(Named {
                    id: BOX.into(),
                    inner: Arc::clone(&other.tools.host),
                }))
                .unwrap();
        }
        let machines = Arc::new(FakeMachines {
            router,
            bundle: ProjectBundle {
                id: BUNDLE.into(),
                name: "alpha".into(),
                favorite: false,
                tags: Vec::new(),
                opened_at: None,
                implicit: false,
                members,
            },
            state: Mutex::new(LinkState::Connected),
            connects: AtomicUsize::new(0),
            refuse: Mutex::new(None),
            before_prepare: Arc::new(Mutex::new(None)),
        });
        let sessions = Sessions::new();
        let services = mcp::services_with(
            &here.host,
            &here.paths,
            "test",
            HostKind::App,
            sessions.clone(),
            Some(Arc::clone(&machines) as Arc<dyn Machines>),
        );
        let here = ToolHost { services, ..here };
        let window =
            sessions
                .approvals()
                .listen(Some(&local), None, Connection::named("alpha-window"));
        Self {
            here,
            sessions,
            src,
            local,
            mini,
            other,
            machines,
            window,
        }
    }

    async fn session(&self) -> Arc<ToolSession> {
        Arc::new(self.here.session(SessionRequest::default()).await)
    }

    fn answer(&self, id: u64, answer: McpApprovalAnswer) {
        self.sessions
            .approvals()
            .answer(
                &Answerer {
                    scope: &ConnectionScope::Project(self.local.clone()),
                    connection: Connection::named("alpha-window"),
                },
                id,
                answer,
            )
            .unwrap();
    }

    /// 원본 `feat` 브랜치에 커밋 하나.
    fn feature(&self, file: &str) -> String {
        if self
            .src
            .try_git_in(
                self.src.root(),
                &["rev-parse", "--verify", "--quiet", "refs/heads/feat"],
            )
            .status
            .success()
        {
            self.src.git(&["switch", "--quiet", "feat"]);
        } else {
            self.src.git(&["switch", "--quiet", "-c", "feat"]);
        }
        self.src.commit_file(file, "x\n", file)
    }
}

/// 그 머신 프로젝트 창이 `git.prompts` 를 듣고 미저장 확인에 「진행」 으로 답한다. 버리면 듣기를 그만둔다.
struct PromptsWindow {
    stop: CancellationToken,
}

impl Drop for PromptsWindow {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}

async fn prompts_window(machine: &Machine) -> PromptsWindow {
    prompts_window_on(&machine.tools, &machine.project).await
}

async fn prompts_window_on(host: &ToolHost, project: &str) -> PromptsWindow {
    let caller = Caller {
        role: Role::Owner,
        scope: ConnectionScope::Project(project.to_owned()),
        window: Some("main".into()),
    };
    let mut connection = host.host.connect(caller).await.unwrap();
    connection.sink.send(json!({
        "jsonrpc": "2.0", "id": 1, "method": "git.prompts", "params": {"projectId": project},
    }));
    let stream = loop {
        let frame = connection.source.recv().await.unwrap();
        if frame.get("id") == Some(&json!(1)) {
            break frame["result"]["streamId"].clone();
        }
    };
    connection.sink.send(json!({
        "jsonrpc": "2.0", "method": "stream.credit", "params": {"streamId": stream, "credit": 1000},
    }));
    let stop = CancellationToken::new();
    let token = stop.clone();
    let project = project.to_owned();
    tokio::spawn(async move {
        let mut next = 100;
        loop {
            let frame = tokio::select! {
                frame = connection.source.recv() => frame,
                () = token.cancelled() => return,
            };
            let Some(frame) = frame else { return };
            let event = &frame["params"]["data"];
            if frame["method"] == json!("stream.chunk") && event["kind"] == json!("open") {
                next += 1;
                connection.sink.send(json!({
                    "jsonrpc": "2.0", "id": next, "method": "git.prompt.answer",
                    "params": {"projectId": project, "promptId": event["prompt"]["promptId"], "answer": {"kind": "proceed"}},
                }));
            }
        }
    });
    PromptsWindow { stop }
}

async fn next_request(window: &mut Listener) -> McpApprovalRequest {
    loop {
        match tokio::time::timeout(Duration::from_secs(20), window.recv())
            .await
            .expect("승인 요청이 오지 않았다")
        {
            Some(McpApprovalEvent::Open { request }) => return *request,
            Some(McpApprovalEvent::Closed { .. }) => {}
            None => panic!("듣기가 끝났다"),
        }
    }
}

/// 승인 요청이 오지 않는다(이미 열린 것도 없다).
async fn no_request(window: &mut Listener) {
    loop {
        match tokio::time::timeout(Duration::from_millis(200), window.recv()).await {
            Err(_) => return,
            Ok(Some(McpApprovalEvent::Closed { .. })) => {}
            Ok(other) => panic!("묻지 않아야 한다: {other:?}"),
        }
    }
}

fn mark(result: &CallToolResult) -> Mark {
    result
        .meta
        .as_ref()
        .and_then(|m| Mark::read(m))
        .expect("결과 표지가 없다")
}

fn spawn_call(
    session: &Arc<ToolSession>,
    name: &'static str,
    args: Value,
) -> tokio::task::JoinHandle<CallToolResult> {
    let session = Arc::clone(session);
    tokio::spawn(async move { session.call(name, args).await })
}

fn send_args(target: &str) -> Value {
    json!({
        "projectId": BUNDLE,
        "source": {"machine": "local"},
        "target": {"machine": target},
        "branch": "feat",
    })
}

/// ⑧ · D57 ⑤ · M7-D9: 첫 보내기는 `auto_approve = ["write"]` 여도 묻고 [이 머신 허용] 이 있다. 허용하면 그 세션의 mini 보내기는 다시
/// 묻지 않는다. 대상은 그 커밋 · 그 브랜치가 되고, 감사 인자 요약에 대상 머신 id 가 든다. 승인 화면에 「대상에서 훅이 돈다」 가 있다.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_send_to_another_machine_asks_once_per_machine_and_lands_there() {
    let mut b = Bundle::with(true, false, "[mcp]\nauto_approve = [\"write\", \"exec\"]\n");
    let _mini_window = prompts_window(&b.mini).await;
    let session = b.session().await;
    let listed = session.list().await;
    for name in ["checkout_list", "checkout_send"] {
        assert!(
            listed.iter().any(|t| t.name == name),
            "{name} 이 목록에 없다"
        );
        assert_eq!(b.here.services.catalog.get(name).unwrap().group, "checkout");
    }
    let commit = b.feature("a.txt");
    let call = spawn_call(&session, "checkout_send", send_args("mini"));
    let request = next_request(&mut b.window).await;
    assert_eq!(request.machine.as_deref(), Some(MINI));
    assert!(
        request.answers.contains(&McpApprovalAnswer::Machine),
        "{:?}",
        request.answers
    );
    let shown = serde_json::to_string(&request.plan).unwrap();
    assert!(shown.contains("Git hooks"), "훅이 바뀔 것에 없다: {shown}");
    assert!(shown.contains("mini"), "{shown}");
    b.answer(request.id, McpApprovalAnswer::Machine);
    let result = call.await.unwrap();
    assert_eq!(mark(&result), Mark::ok(), "{}", text(&result));
    let out = structured(&result);
    assert_eq!(out["commit"], commit);
    assert_eq!(out["created"], true);
    assert_eq!(b.mini.branch("feat").as_deref(), Some(commit.as_str()));
    assert_eq!(b.mini.repo.head(), commit);

    // 같은 세션 · 같은 머신: 다시 묻지 않는다(fast-forward).
    let next = b.feature("b.txt");
    let again = session.call("checkout_send", send_args("mini")).await;
    assert_eq!(mark(&again), Mark::ok(), "{}", text(&again));
    assert_eq!(structured(&again)["previous"], commit);
    assert_eq!(b.mini.branch("feat").as_deref(), Some(next.as_str()));
    no_request(&mut b.window).await;

    let rows = b.here.audit_rows(2);
    let sends: Vec<_> = rows.iter().filter(|r| r.tool == "checkout_send").collect();
    assert_eq!(sends.len(), 2, "{rows:?}");
    for row in sends {
        assert_eq!(row.outcome, "ok", "{row:?}");
        assert_eq!(row.project_id.as_deref(), Some(b.local.as_str()));
        assert!(
            row.args.contains(MINI),
            "대상 머신이 인자 요약에 없다: {}",
            row.args
        );
    }
}

/// 미신뢰 대상은 머신 허용이 없고 매번 묻는다(D57 ⑤).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_untrusted_target_asks_every_time_without_a_machine_allowance() {
    let mut b = Bundle::new(false);
    let _mini_window = prompts_window(&b.mini).await;
    let session = b.session().await;
    for file in ["a.txt", "b.txt"] {
        let commit = b.feature(file);
        let call = spawn_call(&session, "checkout_send", send_args("mini"));
        let request = next_request(&mut b.window).await;
        assert!(
            !request.answers.contains(&McpApprovalAnswer::Machine),
            "{:?}",
            request.answers
        );
        b.answer(request.id, McpApprovalAnswer::Once);
        let result = call.await.unwrap();
        assert_eq!(mark(&result), Mark::ok(), "{}", text(&result));
        assert_eq!(b.mini.branch("feat").as_deref(), Some(commit.as_str()));
    }
}

/// ①: 계획 해시가 원본 끝 커밋을 덮는다 — 승인을 기다리는 사이 원본에 커밋을 더하면 `plan_changed` 이고 대상은 그대로다.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_commit_added_to_the_source_while_asking_changes_the_plan() {
    let mut b = Bundle::new(true);
    let _mini_window = prompts_window(&b.mini).await;
    let session = b.session().await;
    b.feature("a.txt");
    let call = spawn_call(&session, "checkout_send", send_args("mini"));
    let request = next_request(&mut b.window).await;
    b.feature("late.txt");
    b.answer(request.id, McpApprovalAnswer::Once);
    let result = call.await.unwrap();
    assert_eq!(
        mark(&result),
        Mark::denied(Reason::PlanChanged),
        "{}",
        text(&result)
    );
    assert_eq!(b.mini.branch("feat"), None, "대상이 바뀌었다");

    // 대상 브랜치가 움직인 것(대상 끝 커밋)도 같다.
    let call = spawn_call(&session, "checkout_send", send_args("mini"));
    let request = next_request(&mut b.window).await;
    b.mini.repo.git(&["branch", "feat", "main"]);
    b.answer(request.id, McpApprovalAnswer::Once);
    let result = call.await.unwrap();
    assert_eq!(
        mark(&result),
        Mark::denied(Reason::PlanChanged),
        "{}",
        text(&result)
    );
    assert_eq!(b.mini.branch("feat"), Some(b.mini.repo.head()));
}

/// ②: 갈라짐이면 묻지도 보내지도 않고 오류, 대상은 그대로다.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_diverged_target_is_an_error_and_stays_as_it_was() {
    let mut b = Bundle::new(true);
    let session = b.session().await;
    b.src.commit_file("src.txt", "s\n", "source only");
    b.mini.repo.commit_file("dst.txt", "d\n", "target only");
    let before = b.mini.branch("main");
    let args =
        json!({"projectId": BUNDLE, "source": {"machine": "local"}, "target": {"machine": "mini"}});
    let result = session.call("checkout_send", args).await;
    assert_eq!(mark(&result).outcome, Outcome::Error, "{}", text(&result));
    assert!(text(&result).contains("diverged"), "{}", text(&result));
    assert_eq!(b.mini.branch("main"), before);
    no_request(&mut b.window).await;
}

/// ⑤: 해시 재확인 뒤 · prepare 전에 원본에 더한 커밋은 대상에 가지 않는다 — prepare 는 계획 때 읽은 끝 커밋 id 로 묶는다. 원본은
/// mini(다른 머신), 대상은 이 머신(가져오기 — 다른 머신의 커밋이 이 머신에서 돌므로 `auto_approve` 와 상관없이 묻는다).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_commit_added_after_the_recheck_does_not_go() {
    let mut b = Bundle::with(true, false, "[mcp]\nauto_approve = [\"write\"]\n");
    b.mini.repo.git(&["switch", "--quiet", "-c", "feat"]);
    let planned = b.mini.repo.commit_file("m.txt", "m\n", "m");
    let mini = Arc::clone(&b.mini.repo);
    *b.machines.before_prepare.lock().unwrap() = Some(Box::new(move || {
        mini.commit_file("late.txt", "late\n", "late");
    }));
    let _here_window = prompts_window_on(&b.here, &b.local).await;
    let session = b.session().await;
    let args = json!({"projectId": BUNDLE, "source": {"machine": "mini"}, "target": {"machine": "local"}, "branch": "feat"});
    let call = spawn_call(&session, "checkout_send", args);
    let request = next_request(&mut b.window).await;
    assert_eq!(request.machine, None, "대상은 이 머신이다");
    b.answer(request.id, McpApprovalAnswer::Once);
    let result = call.await.unwrap();
    assert_eq!(mark(&result), Mark::ok(), "{}", text(&result));
    assert_ne!(
        b.mini.branch("feat").as_deref(),
        Some(planned.as_str()),
        "덫이 돌지 않았다"
    );
    assert_eq!(structured(&result)["commit"], planned);
    let here = b
        .src
        .git(&["rev-parse", "refs/heads/feat"])
        .trim()
        .to_owned();
    assert_eq!(here, planned, "계획 뒤에 더한 커밋이 갔다");
}

/// 메인 판정 Q2: 대상 체크아웃을 듣는 창이 없으면(활성 체크아웃이 아님) 영어 툴 오류이고 대상 브랜치 · HEAD 는 그대로다.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_target_without_a_window_is_not_changed() {
    let mut b = Bundle::new(true);
    let session = b.session().await;
    b.feature("a.txt");
    let head = b.mini.repo.head();
    let call = spawn_call(&session, "checkout_send", send_args("mini"));
    let request = next_request(&mut b.window).await;
    b.answer(request.id, McpApprovalAnswer::Once);
    let result = call.await.unwrap();
    assert_eq!(mark(&result).outcome, Outcome::Error, "{}", text(&result));
    assert!(
        text(&result).contains("make it the active checkout"),
        "{}",
        text(&result)
    );
    assert_eq!(b.mini.branch("feat"), None);
    assert_eq!(b.mini.repo.head(), head);
}

/// M7-18 메인 판정 3: 사람 질문이 필요한 링크는 열지 않는다 — 묻지 않고 `machine_unavailable`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_machine_that_needs_a_person_is_refused_without_asking() {
    let mut b = Bundle::new(true);
    *b.machines.refuse.lock().unwrap() = Some(MachineError::NeedsPerson(
        "connect to `mini` in Pitwall first".into(),
    ));
    let session = b.session().await;
    b.feature("a.txt");
    let result = session.call("checkout_send", send_args("mini")).await;
    assert_eq!(
        mark(&result),
        Mark::denied(Reason::MachineUnavailable),
        "{}",
        text(&result)
    );
    no_request(&mut b.window).await;
}

/// 이 머신 체크아웃끼리 · 같은 머신끼리는 같은 저장소라 거절, 묶음에 없는 머신 · 다른 머신의 프로젝트 id 도 거절.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bad_sides_are_refused_before_asking() {
    let mut b = Bundle::new(true);
    let session = b.session().await;
    for args in [
        json!({"projectId": BUNDLE, "source": {"machine": "local"}, "target": {"machine": "local", "worktree": "/x"}}),
        json!({"projectId": BUNDLE, "source": {"machine": "mini"}, "target": {"machine": "mini"}}),
        json!({"projectId": BUNDLE, "source": {"machine": "local"}, "target": {"machine": "nowhere"}}),
        json!({"projectId": b.mini.project, "source": {"machine": "local"}, "target": {"machine": "mini"}}),
        json!({"projectId": BUNDLE, "source": {"machine": "local"}, "target": {"machine": "mini"}, "pitwall/approval": {"hash": "x"}, "extra": 1}),
    ] {
        let result = session.call("checkout_send", args.clone()).await;
        assert_ne!(mark(&result), Mark::ok(), "{args}: {}", text(&result));
    }
    no_request(&mut b.window).await;
    assert_eq!(b.mini.branch("feat"), None);
}

/// 두 다른 머신 사이(mini → box): 대상 box 의 머신 허용은 mini 에 닿지 않는다 — 승인의 머신은 대상이다.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_allowance_belongs_to_the_target_machine() {
    let mut b = Bundle::with(true, true, "");
    let other = b.other.as_ref().unwrap();
    let _box_window = prompts_window(other).await;
    let _mini_window = prompts_window(&b.mini).await;
    b.mini.repo.git(&["switch", "--quiet", "-c", "feat"]);
    let commit = b.mini.repo.commit_file("m.txt", "m\n", "m");
    let session = b.session().await;
    let to_box = json!({"projectId": BUNDLE, "source": {"machine": "mini"}, "target": {"machine": "box"}, "branch": "feat"});
    let mut call = spawn_call(&session, "checkout_send", to_box);
    let request = tokio::select! {
        request = next_request(&mut b.window) => request,
        result = &mut call => panic!("묻지 않고 끝났다: {}", text(&result.unwrap())),
    };
    assert_eq!(request.machine.as_deref(), Some(BOX));
    b.answer(request.id, McpApprovalAnswer::Machine);
    let result = call.await.unwrap();
    assert_eq!(mark(&result), Mark::ok(), "{}", text(&result));
    assert_eq!(other.branch("feat").as_deref(), Some(commit.as_str()));
    // box 허용은 mini 로 가는 보내기에 닿지 않는다(mini 의 `feat` 는 갈라졌으니 다른 브랜치).
    b.src.git(&["switch", "--quiet", "-c", "other"]);
    b.src.commit_file("o.txt", "o\n", "o");
    let to_mini = json!({"projectId": BUNDLE, "source": {"machine": "local"}, "target": {"machine": "mini"}, "branch": "other"});
    let mut call = spawn_call(&session, "checkout_send", to_mini);
    let request = tokio::select! {
        request = next_request(&mut b.window) => request,
        result = &mut call => panic!("묻지 않고 끝났다: {}", text(&result.unwrap())),
    };
    assert_eq!(request.machine.as_deref(), Some(MINI));
    b.answer(request.id, McpApprovalAnswer::Deny);
    let result = call.await.unwrap();
    assert_eq!(
        mark(&result),
        Mark::denied(Reason::ApprovalDenied),
        "{}",
        text(&result)
    );
}

/// ④: `checkout_list` 는 링크를 열지 않는다 — 붙지 않은 머신은 상태만, 붙은 머신만 그 체크아웃 · 브랜치를 읽는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn checkout_list_does_not_open_a_link() {
    let b = Bundle::new(true);
    let session = b.session().await;
    for state in [
        LinkState::Disconnected,
        LinkState::Available,
        LinkState::Connecting,
    ] {
        *b.machines.state.lock().unwrap() = state;
        let result = session
            .call("checkout_list", json!({"projectId": BUNDLE}))
            .await;
        assert_eq!(mark(&result), Mark::ok(), "{}", text(&result));
        let out = structured(&result);
        assert_eq!(out["machines"][0]["state"], "local");
        assert_eq!(
            out["machines"][0]["checkouts"][0]["branch"], "main",
            "{out:?}"
        );
        assert_eq!(out["machines"][1]["machine"], "mini");
        assert_eq!(out["machines"][1]["state"], state.as_str());
        assert_eq!(out["machines"][1]["checkouts"], Value::Null);
    }
    assert_eq!(
        b.machines.connects.load(Ordering::SeqCst),
        0,
        "링크를 열었다"
    );
    *b.machines.state.lock().unwrap() = LinkState::Connected;
    let result = session
        .call("checkout_list", json!({"projectId": BUNDLE}))
        .await;
    let out = structured(&result);
    assert_eq!(out["machines"][1]["state"], "connected");
    assert_eq!(
        out["machines"][1]["checkouts"][0]["branch"], "main",
        "{out:?}"
    );
    assert_eq!(
        out["machines"][1]["checkouts"][0]["path"],
        b.mini.repo.root().display().to_string()
    );
}

/// ③: 다른 머신을 모르는 세션(헤드리스 · 원격 머신 Relay 의 세션 — `machines` 없음)에는 두 툴이 없고 부르면 없는 툴이다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_session_without_machines_has_no_checkout_tools() {
    let host = ToolHost::app();
    let session = host.session(SessionRequest::default()).await;
    let listed = session.list().await;
    assert!(
        !listed
            .iter()
            .any(|t| t.name == "checkout_send" || t.name == "checkout_list"),
        "{:?}",
        listed.iter().map(|t| t.name.as_ref()).collect::<Vec<_>>()
    );
    let refused = session
        .try_call("checkout_send", send_args("mini"))
        .await
        .unwrap_err();
    assert!(refused.to_string().contains("unknown tool"), "{refused}");
}
