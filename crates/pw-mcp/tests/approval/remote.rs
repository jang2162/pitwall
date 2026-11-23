//! 다른 머신을 겨누는 호출(18-m7-plan M7-18): `checkout` 입력 · 묶음 풀기 · 연결 풀 · 머신 허용.
//!
//! 다른 머신은 **진짜 `LocalHost`** 다(그 머신의 레지스트리 · `project.trust` · 툴의 `agent_grant` 가 실제 규칙대로 돈다) — 범위를
//! 흉내 내는 가짜가 실앱의 거절을 가린 일이 있었다(M7-11 B1 · M7-17 B1). 두 머신에 일부러 같은 이름의 프로젝트 · 같은 툴을 두어
//! 엉뚱한 머신으로 가면 드러나게 한다. 묶음 · 머신 이름 · 링크는 Desktop 이 꽂는 [`Machines`] 의 가짜가 안다 — 연결은 그 머신
//! Host 에 `Agent` 범위로 연다(실제 Desktop 구현과 같은 `HostRouter::connect`).

use std::sync::Mutex;
use std::sync::atomic::AtomicBool;

use pw_core::registry::bundle::{LOCAL_MACHINE, ProjectBundle, ProjectBundleMember};
use pw_core::{BoxFuture, Target};
use pw_mcp::checkout::{MachineError, MachineLink, Machines};

use super::*;

const MINI: &str = "SHA256:mini";
const BOX: &str = "SHA256:box";
const BUNDLE: &str = "b-00000000000000aa";
const BUNDLE_BETA: &str = "b-00000000000000bb";

/// 다른 머신 하나(진짜 Host · 레지스트리).
struct Machine {
    _tmp: tempfile::TempDir,
    host: Arc<LocalHost>,
    probe: Probe,
    registry: Registry,
    project: String,
    root: PathBuf,
}

impl Machine {
    fn new(id: &str, trusted: bool) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let base = pw_core::registry::path::simplify(std::fs::canonicalize(tmp.path()).unwrap());
        let paths = AppPaths::under(&base.join("app"));
        std::fs::create_dir_all(&paths.config).unwrap();
        std::fs::write(paths.config.join("settings.toml"), "schema = 1\n").unwrap();
        // 이 머신과 같은 이름 — 엉뚱한 머신으로 가면 드러난다.
        let root = base.join("work").join("alpha");
        std::fs::create_dir_all(&root).unwrap();
        let probe = Probe::default();
        let tools = probe.clone();
        // 그 머신의 `mcp.*`(원격만 묶음의 대상 판정 `mcp.target` — 18-m7-plan M7-20)도 실제 것이다.
        let mcp = Arc::new(Mcp::new(Sessions::default()));
        let methods = Arc::clone(&mcp);
        let host = Arc::new(
            LocalHost::builder(id)
                .config(paths.clone())
                .methods(move |router| {
                    register_tools(router, &tools);
                    methods::register(router, Arc::clone(&methods));
                })
                .build(),
        );
        let registry = host.registry().unwrap().clone();
        mcp.attach(HostParts {
            paths: paths.clone(),
            global: host.config().unwrap().global.clone(),
            projects: registry.clone(),
            catalog: Arc::new(fake_catalog()),
            audit: Arc::new(AuditLog::under_state(&paths.state)),
        });
        probe.registry.set(registry.clone()).ok().unwrap();
        let project = registry
            .create(NewProject {
                root: root.clone(),
                ..NewProject::default()
            })
            .unwrap()
            .id;
        registry.set_trust(&project, trusted).unwrap();
        Self {
            _tmp: tmp,
            host,
            probe,
            registry,
            project,
            root,
        }
    }

    fn runs(&self) -> usize {
        self.probe.runs.load(Ordering::SeqCst)
    }

    fn plans(&self) -> usize {
        self.probe.plans.load(Ordering::SeqCst)
    }
}

/// Desktop 이 꽂는 것의 가짜: 묶음 하나(이 머신 `alpha` + mini + box), 별명, 링크.
struct FakeMachines {
    router: Arc<HostRouter>,
    bundles: Mutex<Vec<ProjectBundle>>,
    connects: Mutex<Vec<String>>,
    refuse: Mutex<Option<MachineError>>,
    /// 참이면 연 연결의 `project.trust` 가 실패한다(그 머신의 신뢰를 읽지 못함 — 리뷰 F3).
    break_trust: AtomicBool,
    /// 연 연결의 `mcp.target` 을 어떻게 망가뜨리는가(리뷰 F1 — 그 머신이 답하지 못함): 0 그대로, 1 없는 메서드, 2 다른 꼴의 답
    /// (`host.info`), 3 버림(답이 오지 않음), 4 보내는 순간 링크를 끊음.
    break_target: std::sync::atomic::AtomicU8,
    /// 연 연결마다 끊는 단추(링크가 끊김 — 리뷰 F3).
    cuts: Mutex<Vec<CancellationToken>>,
}

/// `project.trust` 를 없는 메서드로 바꿔 보내는 연결 — 그 Host 가 오류로 답한다.
struct BreakTrust {
    inner: Box<dyn pw_core::FrameSink>,
    on: bool,
    target: u8,
    cut: CancellationToken,
}

impl pw_core::FrameSink for BreakTrust {
    fn send(&self, mut frame: Value) {
        if self.on && frame["method"] == json!("project.trust") {
            frame["method"] = json!("project.trustBroken");
        }
        if frame["method"] == json!("mcp.target") {
            match self.target {
                1 => frame["method"] = json!("mcp.targetBroken"),
                2 => {
                    frame["method"] = json!("host.info");
                    frame["params"] = Value::Null;
                }
                3 => return,
                4 => return self.cut.cancel(),
                _ => {}
            }
        }
        self.inner.send(frame);
    }
}

/// 끊는 단추가 눌리면 끝나는 받는 쪽 — 링크가 끊긴 연결.
struct Cut {
    inner: Box<dyn pw_core::FrameSource>,
    cut: CancellationToken,
}

impl pw_core::FrameSource for Cut {
    fn recv(&mut self) -> BoxFuture<'_, Option<Value>> {
        Box::pin(async move {
            tokio::select! {
                frame = self.inner.recv() => frame,
                () = self.cut.cancelled() => None,
            }
        })
    }
}

impl Machines for FakeMachines {
    fn bundle(&self, project_id: &str) -> Result<Option<ProjectBundle>, String> {
        Ok(self
            .bundles
            .lock()
            .unwrap()
            .iter()
            .find(|b| project_id == b.id || b.local_project() == Some(project_id))
            .cloned())
    }

    fn bundles(&self) -> Result<Vec<ProjectBundle>, String> {
        Ok(self.bundles.lock().unwrap().clone())
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
            self.connects.lock().unwrap().push(machine.to_owned());
            let connection = self
                .router
                .connect(
                    &Target::HostId(machine.to_owned()),
                    Caller::agent(Role::Owner),
                )
                .await
                .map_err(|e| MachineError::Unreachable(e.to_string()))?;
            let cut = CancellationToken::new();
            self.cuts.lock().unwrap().push(cut.clone());
            let connection = pw_core::Connection {
                sink: Box::new(BreakTrust {
                    inner: connection.sink,
                    on: self.break_trust.load(Ordering::SeqCst),
                    target: self.break_target.load(Ordering::SeqCst),
                    cut: cut.clone(),
                }),
                source: Box::new(Cut {
                    inner: connection.source,
                    cut,
                }),
            };
            Ok(MachineLink {
                connection,
                hold: Box::new(()),
            })
        })
    }
}

struct Remote {
    f: Fixture,
    mini: Machine,
    other: Machine,
    machines: Arc<FakeMachines>,
}

impl Remote {
    fn new() -> Self {
        Self::with_global("")
    }

    fn with_global(global: &str) -> Self {
        let f = Fixture::with_global(global);
        let mini = Machine::new(MINI, true);
        let other = Machine::new(BOX, true);
        let router = Arc::new(HostRouter::new());
        router.add_host(mini.host.clone()).unwrap();
        router.add_host(other.host.clone()).unwrap();
        let member = |machine: &str, project: &str| ProjectBundleMember {
            machine: machine.into(),
            project_id: project.into(),
            last_target: None,
        };
        let bundle = ProjectBundle {
            id: BUNDLE.into(),
            name: "alpha".into(),
            favorite: false,
            tags: Vec::new(),
            opened_at: None,
            implicit: false,
            members: vec![
                member(LOCAL_MACHINE, &f.alpha),
                member(MINI, &mini.project),
                member(BOX, &other.project),
            ],
        };
        let machines = Arc::new(FakeMachines {
            router,
            bundles: Mutex::new(vec![bundle]),
            connects: Mutex::default(),
            refuse: Mutex::default(),
            break_trust: AtomicBool::new(false),
            break_target: std::sync::atomic::AtomicU8::new(0),
            cuts: Mutex::default(),
        });
        Self {
            f,
            mini,
            other,
            machines,
        }
    }

    async fn agent(&self, client: &str) -> Agent {
        self.agent_in(client, "alpha").await
    }

    /// 세션을 `work/<project>` 에서 연다.
    async fn agent_in(&self, client: &str, project: &str) -> Agent {
        let machines: Arc<dyn Machines> = self.machines.clone();
        self.f
            .agent_with(
                client,
                &self.f.work(project),
                self.f.services_with(HostKind::App, Some(machines)),
            )
            .await
    }

    /// 둘째 묶음: 이 머신 `beta` + mini 의 다른 프로젝트(신뢰). 그 mini 프로젝트 id 를 돌려준다.
    fn beta_bundle(&self) -> String {
        let root = self.mini.root.parent().unwrap().join("beta");
        std::fs::create_dir_all(&root).unwrap();
        let project = self
            .mini
            .registry
            .create(NewProject {
                root,
                ..NewProject::default()
            })
            .unwrap()
            .id;
        self.mini.registry.set_trust(&project, true).unwrap();
        self.machines.bundles.lock().unwrap().push(ProjectBundle {
            id: BUNDLE_BETA.into(),
            name: "beta".into(),
            favorite: false,
            tags: Vec::new(),
            opened_at: None,
            implicit: false,
            members: vec![
                ProjectBundleMember {
                    machine: LOCAL_MACHINE.into(),
                    project_id: self.f.beta.clone(),
                    last_target: None,
                },
                ProjectBundleMember {
                    machine: MINI.into(),
                    project_id: project.clone(),
                    last_target: None,
                },
            ],
        });
        project
    }

    fn connects(&self) -> Vec<String> {
        self.machines.connects.lock().unwrap().clone()
    }

    /// 감사 행의 (툴, 프로젝트, 인자 요약, 결과, 승인).
    fn audit(&self, n: usize) -> Vec<AuditRow> {
        self.f.rows(n);
        let conn = rusqlite::Connection::open(self.f.paths.state.join("mcp/audit.db")).unwrap();
        let mut stmt = conn
            .prepare("SELECT tool, project_id, args, outcome, approval FROM calls ORDER BY id")
            .unwrap();
        stmt.query_map([], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        })
        .unwrap()
        .map(Result::unwrap)
        .collect()
    }
}

/// (툴, 프로젝트, 인자 요약, 결과, 승인).
type AuditRow = (String, Option<String>, String, String, Option<String>);

fn on(machine: &str, args: Value) -> Value {
    let mut args = args;
    args["checkout"] = json!({ "machine": machine });
    args
}

/// ① `auto_approve = ["write", "exec"]` 이어도 다른 머신의 첫 `write` 는 묻는다. 묻는 창은 묶음의 로컬 멤버 창이고 요청에 대상
/// 머신이 실린다. 같은 설정에서 이 머신의 `write` 는 묻지 않는다(대조).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn auto_approve_does_not_reach_another_machine() {
    let r = Remote::with_global("[mcp]\nauto_approve = [\"write\", \"exec\"]\n");
    let mut window = r.f.window(&r.f.alpha);
    let agent = r.agent("agent").await;
    assert_eq!(
        mark(&agent.call("fake_write", json!({"tag": "here"})).await),
        Mark::ok()
    );
    assert!(window.quiet(), "이 머신 대조: auto_approve 가 닿는다");

    let call = agent.spawn("fake_write", on("mini", json!({"tag": "there"})));
    let request = window.opened().await;
    assert_eq!(request.machine.as_deref(), Some(MINI));
    assert_eq!(request.project_id.as_deref(), Some(r.f.alpha.as_str()));
    assert_eq!(
        request.answers,
        [
            McpApprovalAnswer::Deny,
            McpApprovalAnswer::Once,
            McpApprovalAnswer::Machine
        ]
    );
    assert_eq!(r.mini.runs(), 0, "승인 전에는 돌지 않는다");
    window.answer(request.id, McpApprovalAnswer::Once).unwrap();
    assert_eq!(mark(&within(call).await.unwrap()), Mark::ok());
    assert_eq!((r.mini.runs(), r.other.runs()), (1, 0));
    assert_eq!(r.f.runs(), 1, "이 머신은 대조 한 번뿐");
}

/// ② 정책 자동 계획(사람이 본 구성)도 다른 머신이면 묻는다. 이 머신에서는 묻지 않는다(대조).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn policy_auto_does_not_reach_another_machine() {
    let r = Remote::new();
    let mut window = r.f.window(&r.f.alpha);
    let agent = r.agent("agent").await;
    assert_eq!(
        mark(&agent.call("fake_run", json!({"tag": "exec+policy"})).await),
        Mark::ok()
    );
    assert!(window.quiet());
    let call = agent.spawn("fake_run", on("mini", json!({"tag": "exec+policy"})));
    let request = window.opened().await;
    assert_eq!(request.tier, Tier::Exec);
    assert_eq!(request.machine.as_deref(), Some(MINI));
    window.answer(request.id, McpApprovalAnswer::Deny).unwrap();
    assert_eq!(
        mark(&within(call).await.unwrap()),
        Mark::denied(Reason::ApprovalDenied)
    );
    assert_eq!(r.mini.runs(), 0);
}

/// ③ 머신 허용 뒤 같은 머신의 `exec` · `write` 는 묻지 않고(감사의 승인 방식 `session` · 인자 요약에 그 머신), 다른 머신과 이 머신은 묻는다. 연결은 머신마다 하나다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_machine_allowance_reaches_that_machine_only() {
    let r = Remote::new();
    let mut window = r.f.window(&r.f.alpha);
    let agent = r.agent("agent").await;
    let call = agent.spawn("fake_run", on("mini", json!({"tag": "exec"})));
    let request = window.opened().await;
    window
        .answer(request.id, McpApprovalAnswer::Machine)
        .unwrap();
    assert_eq!(mark(&within(call).await.unwrap()), Mark::ok());
    window.closed(request.id).await;

    assert_eq!(
        mark(
            &agent
                .call("fake_run", on("mini", json!({"tag": "exec"})))
                .await
        ),
        Mark::ok()
    );
    assert_eq!(
        mark(
            &agent
                .call("fake_write", on("mini", json!({"tag": "w"})))
                .await
        ),
        Mark::ok()
    );
    assert!(window.quiet(), "허용한 머신인데 물었다");
    assert_eq!(r.mini.runs(), 3);

    let call = agent.spawn("fake_run", on("box", json!({"tag": "exec"})));
    let asked = window.opened().await;
    assert_eq!(asked.machine.as_deref(), Some(BOX));
    window.answer(asked.id, McpApprovalAnswer::Deny).unwrap();
    within(call).await.unwrap();
    window.closed(asked.id).await;

    let call = agent.spawn("fake_write", json!({"tag": "local"}));
    let asked = window.opened().await;
    assert_eq!(asked.machine, None, "이 머신 호출");
    window.answer(asked.id, McpApprovalAnswer::Deny).unwrap();
    within(call).await.unwrap();
    assert_eq!((r.other.runs(), r.f.runs()), (0, 0));
    assert_eq!(r.connects(), [MINI, BOX], "머신마다 연결 하나");

    let rows = r.audit(5);
    let approvals: Vec<_> = rows.iter().map(|row| row.4.as_deref()).collect();
    assert_eq!(
        approvals,
        [
            Some("session"),
            Some("session"),
            Some("session"),
            None,
            None
        ]
    );
    for row in &rows[..3] {
        let args: Value = serde_json::from_str(&row.2).unwrap();
        assert_eq!(args["checkout"]["machine"], json!(MINI), "{args}");
    }
}

/// ④ 미신뢰 대상은 머신 허용 뒤에도 묻고 [이 머신 허용] 을 싣지 않는다. 신뢰는 대상 머신의 것이다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_untrusted_checkout_asks_after_a_machine_allowance() {
    let r = Remote::new();
    let mut window = r.f.window(&r.f.alpha);
    let agent = r.agent("agent").await;
    let call = agent.spawn("fake_write", on("mini", json!({"tag": "1"})));
    let request = window.opened().await;
    window
        .answer(request.id, McpApprovalAnswer::Machine)
        .unwrap();
    within(call).await.unwrap();
    window.closed(request.id).await;

    r.mini.registry.set_trust(&r.mini.project, false).unwrap();
    for _ in 0..2 {
        let call = agent.spawn("fake_write", on("mini", json!({"tag": "2"})));
        let asked = window.opened().await;
        assert_eq!(
            asked.answers,
            [McpApprovalAnswer::Deny, McpApprovalAnswer::Once]
        );
        assert_eq!(
            window.answer(asked.id, McpApprovalAnswer::Machine),
            Err(AnswerError::NotOffered)
        );
        window.answer(asked.id, McpApprovalAnswer::Once).unwrap();
        assert_eq!(mark(&within(call).await.unwrap()), Mark::ok());
        window.closed(asked.id).await;
    }
    assert_eq!(r.mini.runs(), 3);
}

/// ⑤ 사람이 띄운 실행의 `run_restart`(계획 규칙 `auto_approve: false` — D69)는 머신 허용 뒤에도 묻는다. `dangerous` 도 매번.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_human_run_and_dangerous_ask_after_a_machine_allowance() {
    let r = Remote::new();
    let mut window = r.f.window(&r.f.alpha);
    let agent = r.agent("agent").await;
    let call = agent.spawn("fake_run", on("mini", json!({"tag": "exec"})));
    let request = window.opened().await;
    window
        .answer(request.id, McpApprovalAnswer::Machine)
        .unwrap();
    within(call).await.unwrap();
    window.closed(request.id).await;

    for (tool, tag) in [("fake_run", "exec+human"), ("fake_dangerous", "d")] {
        let call = agent.spawn(tool, on("mini", json!({ "tag": tag })));
        let asked = window.opened().await;
        assert_eq!(asked.tool, tool);
        assert!(
            !asked.answers.contains(&McpApprovalAnswer::Machine),
            "{tool}: {:?}",
            asked.answers
        );
        window.answer(asked.id, McpApprovalAnswer::Deny).unwrap();
        within(call).await.unwrap();
        window.closed(asked.id).await;
    }
    assert_eq!(r.mini.runs(), 1);
}

/// ⑥ 묶음 창이 열려 있지 않으면 다른 머신 호출은 읽기도 거절한다 — 그 머신에 붙지도, 툴을 부르지도 않는다. 다른 머신을 모르는
/// 세션(헤드리스 · `machines` 없음)도 거절한다(fail-closed).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn without_the_bundle_window_or_machines_another_machine_is_refused() {
    let r = Remote::new();
    let agent = r.agent("agent").await;
    for (tool, tag) in [("fake_echo", "x"), ("fake_write", "y")] {
        let result = agent.call(tool, on("mini", json!({ "tag": tag }))).await;
        assert_eq!(mark(&result), Mark::denied(Reason::MachineUnavailable));
        assert!(text(&result).contains("window"), "{}", text(&result));
    }
    assert!(r.connects().is_empty());
    assert_eq!((r.mini.plans(), r.mini.runs()), (0, 0));

    let _window = r.f.window(&r.f.alpha);
    for kind in [HostKind::App, HostKind::Headless] {
        let plain =
            r.f.agent_with("agent", &r.f.work("alpha"), r.f.services_with(kind, None))
                .await;
        let result = plain
            .call("fake_echo", on("mini", json!({ "tag": "x" })))
            .await;
        assert_eq!(
            mark(&result),
            Mark::denied(Reason::MachineUnavailable),
            "{kind:?}"
        );
    }
    assert!(r.connects().is_empty());
    assert_eq!(r.mini.runs(), 0);
}

/// ⑦ 원격 Host 로 가는 입력에 `checkout` 이 없다 — 그 머신의 프로젝트 id 와 `worktree` 뿐이고, 그 머신 레지스트리의 `agent_grant` 를
/// 지난다. ⑨ 감사 인자 요약에 대상 체크아웃(머신 id · 워크트리)이 든다 — 감사는 이 Desktop 의 것이고 프로젝트는 로컬 멤버다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_remote_host_gets_its_own_project_and_no_checkout() {
    let r = Remote::new();
    let _window = r.f.window(&r.f.alpha);
    let agent = r.agent("agent").await;
    let worktree = r.mini.root.display().to_string();
    let result = agent
        .call(
            "fake_echo",
            json!({"checkout": {"machine": "mini", "worktree": worktree}, "tag": "t"}),
        )
        .await;
    assert_eq!(mark(&result), Mark::ok(), "{}", text(&result));
    let echoed: Value = serde_json::from_str(&text(&result)).unwrap();
    assert_eq!(echoed["projectId"], json!(r.mini.project));
    assert_eq!(echoed["worktree"], json!(worktree));
    assert_eq!(echoed["keys"], json!(["tag"]), "checkout 이 Host 에 갔다");
    assert_eq!((r.mini.runs(), r.other.runs(), r.f.runs()), (1, 0, 0));

    let rows = r.audit(1);
    let (tool, project, args, outcome, _) = &rows[0];
    assert_eq!((tool.as_str(), outcome.as_str()), ("fake_echo", "ok"));
    assert_eq!(project.as_deref(), Some(r.f.alpha.as_str()));
    let args: Value = serde_json::from_str(args).unwrap();
    assert_eq!(args["checkout"]["machine"], json!(MINI), "{args}");
    assert_eq!(args["checkout"]["worktree"], json!(worktree), "{args}");
}

/// ⑧ `checkout` 은 `worktree` 를 받는 툴의 스키마에만 있다 — `project_info` 에는 없다. 다른 머신을 모르는 세션에는 어디에도 없다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn checkout_is_in_the_schema_of_worktree_tools_only() {
    let r = Remote::new();
    let agent = r.agent("agent").await;
    assert!(has(&agent, "fake_write").await);
    assert!(has(&agent, "fake_echo").await);
    assert!(!has(&agent, "project_info").await);
    assert!(!has(&agent, "fake_register").await);
    let plain =
        r.f.agent_with(
            "agent",
            &r.f.work("alpha"),
            r.f.services_with(HostKind::App, None),
        )
        .await;
    assert!(!has(&plain, "fake_write").await);
}

async fn has(agent: &Agent, tool: &str) -> bool {
    let tools = agent.running.peer().list_all_tools().await.unwrap();
    let tool = tools.iter().find(|t| t.name == tool).expect("listed");
    tool.input_schema
        .get("properties")
        .and_then(Value::as_object)
        .is_some_and(|p| p.contains_key("checkout"))
}

/// ⑩ 머신 허용 뒤에도 호출마다 청하는 승인(원격 프로덕션 연결의 `ask` 조회 · 「이 연결 열기」 의 자리)은 묻는다. 그 승인의 세션
/// 허용은 그 머신의 그 열쇠에만 닿는다 — 이 머신의 같은 열쇠 허용이 다른 머신에 닿지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_per_call_approval_asks_after_a_machine_allowance() {
    let r = Remote::new();
    let mut window = r.f.window(&r.f.alpha);
    let agent = r.agent("agent").await;
    let call = agent.spawn("fake_write", on("mini", json!({"tag": "w"})));
    let request = window.opened().await;
    window
        .answer(request.id, McpApprovalAnswer::Machine)
        .unwrap();
    within(call).await.unwrap();
    window.closed(request.id).await;

    // 이 머신의 그 열쇠를 세션 허용한다.
    let call = agent.spawn("fake_open", json!({"tag": "ask-prod"}));
    let local = window.opened().await;
    window.answer(local.id, McpApprovalAnswer::Session).unwrap();
    within(call).await.unwrap();
    window.closed(local.id).await;

    let call = agent.spawn("fake_open", on("mini", json!({"tag": "ask-prod"})));
    let asked = window.opened().await;
    assert_eq!(asked.tier, Tier::Read);
    assert_eq!(asked.machine.as_deref(), Some(MINI));
    assert_eq!(
        asked.answers,
        [
            McpApprovalAnswer::Deny,
            McpApprovalAnswer::Once,
            McpApprovalAnswer::Session
        ]
    );
    window.answer(asked.id, McpApprovalAnswer::Session).unwrap();
    within(call).await.unwrap();
    window.closed(asked.id).await;
    // 그 머신의 그 열쇠는 이제 묻지 않는다.
    assert_eq!(
        mark(
            &agent
                .call("fake_open", on("mini", json!({"tag": "ask-prod"})))
                .await
        ),
        Mark::ok()
    );
    assert!(window.quiet());
}

/// ⑪ 다른 머신의 프로젝트 id 를 `projectId` 로 주면 거절한다 — 그 머신에 붙지도 툴을 부르지도 않는다. 묶음 id 는 받는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn another_machines_project_id_is_refused_and_a_bundle_id_is_taken() {
    let r = Remote::new();
    let _window = r.f.window(&r.f.alpha);
    let agent = r.agent("agent").await;
    let result = agent
        .call(
            "fake_echo",
            on("mini", json!({"projectId": r.mini.project, "tag": "x"})),
        )
        .await;
    assert_eq!(mark(&result).outcome, pw_mcp::audit::Outcome::Error);
    assert!(
        text(&result).contains("another machine's project ids are not accepted"),
        "{}",
        text(&result)
    );
    assert!(r.connects().is_empty());
    assert_eq!(r.mini.runs(), 0);

    let result = agent
        .call(
            "fake_echo",
            on("mini", json!({"projectId": BUNDLE, "tag": "x"})),
        )
        .await;
    assert_eq!(mark(&result), Mark::ok(), "{}", text(&result));
    assert_eq!(r.mini.runs(), 1);
    // 묶음 id 에 체크아웃이 없으면 이 머신의 멤버다.
    let result = agent
        .call("fake_echo", json!({"projectId": BUNDLE, "tag": "x"}))
        .await;
    assert_eq!(mark(&result), Mark::ok(), "{}", text(&result));
    let echoed: Value = serde_json::from_str(&text(&result)).unwrap();
    assert_eq!(echoed["projectId"], json!(r.f.alpha));
    assert_eq!((r.f.runs(), r.mini.runs()), (1, 1));
}

/// 링크를 열려면 사람이 답할 것이 있으면(암호 · 호스트 키 · 처음 보는 머신) 열지 않고 거절한다. 모르는 머신 · 묶음에 없는 머신도
/// 거절한다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_machine_that_needs_a_person_to_connect_is_refused() {
    let r = Remote::new();
    let _window = r.f.window(&r.f.alpha);
    *r.machines.refuse.lock().unwrap() = Some(MachineError::NeedsPerson(
        "connect to mini in Pitwall first".into(),
    ));
    let agent = r.agent("agent").await;
    let result = agent
        .call("fake_echo", on("mini", json!({"tag": "x"})))
        .await;
    assert_eq!(mark(&result), Mark::denied(Reason::MachineUnavailable));
    assert!(
        text(&result).contains("connect to mini in Pitwall first"),
        "{}",
        text(&result)
    );
    let result = agent
        .call("fake_echo", on("nowhere", json!({"tag": "x"})))
        .await;
    assert!(
        text(&result).contains("unknown machine"),
        "{}",
        text(&result)
    );
    assert_eq!(r.mini.runs(), 0);
}

/// CLI 의 머신 허용은 「이 프로젝트 창이 열려 있는 동안 CLI 에 이 머신 허용」 이다 — 다음 CLI 세션에도 닿고, 창을 닫으면 사라진다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_cli_machine_allowance_lasts_while_the_window_is_open() {
    let r = Remote::new();
    let mut window = r.f.window(&r.f.alpha);
    let first = r.agent(pw_mcp::server::CLI_CLIENT).await;
    let call = first.spawn("fake_write", on("mini", json!({"tag": "1"})));
    let request = window.opened().await;
    assert!(request.cli);
    assert_eq!(
        request.answers,
        [
            McpApprovalAnswer::Deny,
            McpApprovalAnswer::Once,
            McpApprovalAnswer::Machine
        ]
    );
    window
        .answer(request.id, McpApprovalAnswer::Machine)
        .unwrap();
    assert_eq!(mark(&within(call).await.unwrap()), Mark::ok());
    window.closed(request.id).await;
    let second = r.agent(pw_mcp::server::CLI_CLIENT).await;
    assert_eq!(
        mark(
            &second
                .call("fake_run", on("mini", json!({"tag": "exec"})))
                .await
        ),
        Mark::ok()
    );
    assert!(window.quiet());
    drop(window);
    let mut reopened = r.f.window(&r.f.alpha);
    let third = r.agent(pw_mcp::server::CLI_CLIENT).await;
    let call = third.spawn("fake_write", on("mini", json!({"tag": "3"})));
    let asked = reopened.opened().await;
    reopened.answer(asked.id, McpApprovalAnswer::Deny).unwrap();
    within(call).await.unwrap();
    assert_eq!(r.mini.runs(), 2);
}

/// `project_list` 는 묶음을 낸다: 묶음에 든 프로젝트에 묶음 id 와 체크아웃(머신 · 이름)을 싣는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn project_list_carries_the_bundles() {
    let r = Remote::new();
    let agent = r.agent("agent").await;
    let result = agent.call("project_list", json!({})).await;
    let listed: Value = serde_json::from_str(&text(&result)).unwrap();
    let alpha = listed
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == json!(r.f.alpha))
        .unwrap();
    assert_eq!(alpha["bundleId"], json!(BUNDLE));
    assert_eq!(
        alpha["checkouts"],
        json!([
            {"machine": "local", "name": "local"},
            {"machine": MINI, "name": "mini"},
            {"machine": BOX, "name": "box"}
        ])
    );
}

/// 리뷰 B1: 다른 머신을 겨눠도 묶음의 이 머신 멤버(에이전트가 고른 Desktop 쪽 프로젝트)의 `[mcp]` 노출을 지난다 — 그 프로젝트가 끈
/// 툴은 `checkout` 을 달아도 어디서도 돌지 않고, 그 머신에 붙지도 않는다. 세션은 beta, 대상은 alpha 묶음(이 머신 멤버 alpha).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn another_projects_exposure_is_kept_when_targeting_another_machine() {
    let r = Remote::new();
    let alpha = r.f.work("alpha");
    std::fs::create_dir_all(alpha.join(".pitwall")).unwrap();
    std::fs::write(
        alpha.join(".pitwall/project.toml"),
        "schema = 1\n[mcp.tools]\nfake_echo = false\n",
    )
    .unwrap();
    let _window = r.f.window(&r.f.alpha);
    let agent = r.agent_in("agent", "beta").await;
    let here = agent
        .call("fake_echo", json!({"projectId": r.f.alpha, "tag": "x"}))
        .await;
    assert_eq!(
        mark(&here),
        Mark::denied(Reason::NotExposed),
        "대조: 이 머신"
    );
    for project in [r.f.alpha.as_str(), BUNDLE] {
        let there = agent
            .call(
                "fake_echo",
                on("mini", json!({ "projectId": project, "tag": "x" })),
            )
            .await;
        assert_eq!(
            mark(&there),
            Mark::denied(Reason::NotExposed),
            "{project}: {}",
            text(&there)
        );
    }
    assert!(r.connects().is_empty(), "끈 툴로 그 머신에 붙었다");
    assert_eq!((r.mini.runs(), r.f.runs()), (0, 0));
}

/// 리뷰 F1: 「이 세션 동안 이 머신 허용」 은 (프로젝트, 머신)이다 — alpha 묶음에서 허용한 mini 가 beta 묶음의 mini 멤버에 닿지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_machine_allowance_stays_in_its_project() {
    let r = Remote::new();
    r.beta_bundle();
    let mut alpha = r.f.window(&r.f.alpha);
    let mut beta = r.f.window(&r.f.beta);
    let agent = r.agent("agent").await;
    let call = agent.spawn("fake_write", on("mini", json!({"tag": "a"})));
    let request = alpha.opened().await;
    alpha
        .answer(request.id, McpApprovalAnswer::Machine)
        .unwrap();
    assert_eq!(mark(&within(call).await.unwrap()), Mark::ok());
    alpha.closed(request.id).await;
    // 대조: 같은 프로젝트 · 같은 머신은 묻지 않는다.
    assert_eq!(
        mark(
            &agent
                .call("fake_write", on("mini", json!({"tag": "a2"})))
                .await
        ),
        Mark::ok()
    );
    let call = agent.spawn(
        "fake_write",
        on("mini", json!({"projectId": BUNDLE_BETA, "tag": "b"})),
    );
    let asked = beta.opened().await;
    assert_eq!(asked.project_id.as_deref(), Some(r.f.beta.as_str()));
    assert_eq!(asked.machine.as_deref(), Some(MINI));
    beta.answer(asked.id, McpApprovalAnswer::Deny).unwrap();
    assert_eq!(
        mark(&within(call).await.unwrap()),
        Mark::denied(Reason::ApprovalDenied)
    );
    assert!(alpha.quiet());
    assert_eq!(r.mini.runs(), 2);
}

/// 리뷰 F3: 그 머신의 신뢰를 읽지 못하면 미신뢰로 본다 — 머신 허용 뒤에도 묻고 [이 머신 허용] 을 싣지 않는다(fail-closed).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unreadable_trust_is_untrusted() {
    let r = Remote::new();
    r.machines.break_trust.store(true, Ordering::SeqCst);
    let mut window = r.f.window(&r.f.alpha);
    let agent = r.agent("agent").await;
    for _ in 0..2 {
        let call = agent.spawn("fake_write", on("mini", json!({"tag": "1"})));
        let asked = window.opened().await;
        assert_eq!(
            asked.answers,
            [McpApprovalAnswer::Deny, McpApprovalAnswer::Once]
        );
        window.answer(asked.id, McpApprovalAnswer::Once).unwrap();
        assert_eq!(mark(&within(call).await.unwrap()), Mark::ok());
        window.closed(asked.id).await;
    }
    assert_eq!(r.mini.runs(), 2);
}

/// 리뷰 F3: 링크가 끊겨 연결이 닫히면 다음 호출은 연결을 다시 연다 — 닫힌 연결을 계속 쓰지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_closed_machine_connection_is_opened_again() {
    let r = Remote::new();
    let _window = r.f.window(&r.f.alpha);
    let agent = r.agent("agent").await;
    assert_eq!(
        mark(
            &agent
                .call("fake_echo", on("mini", json!({"tag": "1"})))
                .await
        ),
        Mark::ok()
    );
    for cut in r.machines.cuts.lock().unwrap().iter() {
        cut.cancel();
    }
    // 펌프가 닫힘을 볼 틈.
    tokio::time::sleep(Duration::from_millis(50)).await;
    let again = agent
        .call("fake_echo", on("mini", json!({"tag": "2"})))
        .await;
    assert_eq!(mark(&again), Mark::ok(), "{}", text(&again));
    assert_eq!(r.connects(), [MINI, MINI]);
    assert_eq!(r.mini.runs(), 2);
}

/// 델타 리뷰 F4: 묶음의 이 머신 멤버 설정을 읽지 못하면 다른 머신을 겨눈 호출도 거절한다(fail-closed — `project_config_unreadable`).
/// 그 머신에 붙지도, 툴을 부르지도 않는다. 읽기 툴이라 승인 경로를 지나지 않는다 — 판정은 호출 앞의 한 자리다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unreadable_settings_of_the_bundle_member_refuse_another_machine() {
    let r = Remote::new();
    let broken = r.f.work("alpha").join(".pitwall/db/y.toml");
    std::fs::create_dir_all(broken.parent().unwrap()).unwrap();
    std::fs::write(&broken, "name = \"y\n").unwrap();
    let _window = r.f.window(&r.f.alpha);
    let agent = r.agent("agent").await;
    let result = agent
        .call("fake_echo", on("mini", json!({"tag": "x"})))
        .await;
    assert_eq!(
        mark(&result),
        Mark::denied(Reason::ProjectConfigUnreadable),
        "{}",
        text(&result)
    );
    assert!(!text(&result).contains("name ="), "{}", text(&result));
    assert!(r.connects().is_empty(), "설정을 못 읽는데 그 머신에 붙었다");
    assert_eq!((r.mini.runs(), r.f.runs()), (0, 0));
    std::fs::remove_file(&broken).unwrap();
    assert_eq!(
        mark(
            &agent
                .call("fake_echo", on("mini", json!({"tag": "x"})))
                .await
        ),
        Mark::ok(),
        "대조: 고치면 돈다"
    );
}

/// 델타 리뷰 R7(M7-20 에서 고침): 이 머신 멤버가 없는 묶음(원격만)은 **그 묶음 창(묶음 id 로 듣는 창)** 이 없으면 거절한다 — 그 머신의
/// 프로젝트 id 와 같은 id 로 듣는 이 머신 창이 있어도 그 창으로 가지 않는다(프로젝트 id 는 머신 사이에 유일하지 않다 — M7-03 ⑥).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_bundle_without_a_member_here_is_refused() {
    let r = Remote::new();
    const REMOTE_ONLY: &str = "b-00000000000000cc";
    r.machines.bundles.lock().unwrap().push(ProjectBundle {
        id: REMOTE_ONLY.into(),
        name: "remote only".into(),
        favorite: false,
        tags: Vec::new(),
        opened_at: None,
        implicit: false,
        members: vec![ProjectBundleMember {
            machine: BOX.into(),
            project_id: r.other.project.clone(),
            last_target: None,
        }],
    });
    // 원격 프로젝트 id 와 같은 id 를 듣는 창(이 머신에 그 id 의 프로젝트가 있다고 치자).
    let _same_id = r.f.window(&r.other.project);
    let _alpha = r.f.window(&r.f.alpha);
    let agent = r.agent("agent").await;
    for tool in ["fake_echo", "fake_write"] {
        let result = agent
            .call(
                tool,
                on("box", json!({"projectId": REMOTE_ONLY, "tag": "x"})),
            )
            .await;
        assert_eq!(
            mark(&result),
            Mark::denied(Reason::MachineUnavailable),
            "{tool}: {}",
            text(&result)
        );
        assert!(
            text(&result).contains("needs the window of project `remote only`"),
            "{}",
            text(&result)
        );
    }
    assert!(r.connects().is_empty());
    assert_eq!((r.other.plans(), r.other.runs()), (0, 0));
}

const REMOTE_ONLY: &str = "b-00000000000000dd";

/// box 멤버만 있는 묶음(원격만)을 더한다.
fn remote_only(r: &Remote) {
    r.machines.bundles.lock().unwrap().push(ProjectBundle {
        id: REMOTE_ONLY.into(),
        name: "remote only".into(),
        favorite: false,
        tags: Vec::new(),
        opened_at: None,
        implicit: false,
        members: vec![ProjectBundleMember {
            machine: BOX.into(),
            project_id: r.other.project.clone(),
            last_target: None,
        }],
    });
}

/// 18-m7-plan M7-20(M7-11b 판정): 이 머신 체크아웃이 없는 묶음(원격만)도 그 묶음 창이 열려 있으면 다른 머신 호출이 된다. 승인은 그
/// 묶음 창(묶음 id 로 듣는 창)에 묻고 창이 답한다. 요청에 대상 머신이 실린다. 읽기 · 쓰기 모두 box 에서만 돈다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_remote_only_bundle_asks_its_bundle_window() {
    let r = Remote::new();
    remote_only(&r);
    let mut window = r.f.window(REMOTE_ONLY);
    let agent = r.agent("agent").await;
    let read = agent
        .call(
            "fake_echo",
            on("box", json!({"projectId": REMOTE_ONLY, "tag": "x"})),
        )
        .await;
    assert_eq!(mark(&read), Mark::ok(), "{}", text(&read));
    let call = agent.spawn(
        "fake_write",
        on("box", json!({"projectId": REMOTE_ONLY, "tag": "w"})),
    );
    let request = within(window.opened()).await;
    assert_eq!(request.project_id.as_deref(), Some(REMOTE_ONLY));
    assert_eq!(request.machine.as_deref(), Some(BOX));
    window.answer(request.id, McpApprovalAnswer::Once).unwrap();
    let result = within(call).await.unwrap();
    assert_eq!(mark(&result), Mark::ok(), "{}", text(&result));
    // 읽기 하나 · 쓰기 하나, 둘 다 box 에서.
    assert_eq!((r.other.runs(), r.mini.runs()), (2, 0));
}

/// 원격만 묶음의 노출 · 정책은 **그 머신의 그 프로젝트 설정**이다(그 머신 Host 가 판정 — `mcp.target`): 그 프로젝트에서 끈 툴은
/// 막히고(전역 설정만으로 판정하지 않는다), 그 프로젝트 설정을 읽지 못하면 거절한다(fail-closed). 둘 다 계획 · 실행 0.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_remote_only_bundle_follows_that_machines_project_settings() {
    let r = Remote::new();
    remote_only(&r);
    let _window = r.f.window(REMOTE_ONLY);
    let agent = r.agent("agent").await;
    let pitwall = r.other.root.join(".pitwall");
    std::fs::create_dir_all(pitwall.join("db")).unwrap();
    std::fs::write(
        pitwall.join("project.toml"),
        "schema = 1\n[mcp.tools]\nfake_echo = false\nfake_write = false\n",
    )
    .unwrap();
    for tool in ["fake_echo", "fake_write"] {
        let off = agent
            .call(
                tool,
                on("box", json!({"projectId": REMOTE_ONLY, "tag": "x"})),
            )
            .await;
        assert_eq!(
            mark(&off),
            Mark::denied(Reason::NotExposed),
            "{tool}: {}",
            text(&off)
        );
    }
    std::fs::write(pitwall.join("project.toml"), "schema = 1\n").unwrap();
    std::fs::write(pitwall.join("db/y.toml"), "name = \"y\n").unwrap();
    for tool in ["fake_echo", "fake_write"] {
        let broken = agent
            .call(
                tool,
                on("box", json!({"projectId": REMOTE_ONLY, "tag": "x"})),
            )
            .await;
        assert_eq!(
            mark(&broken),
            Mark::denied(Reason::ProjectConfigUnreadable),
            "{tool}: {}",
            text(&broken)
        );
    }
    assert_eq!((r.other.plans(), r.other.runs()), (0, 0));
    std::fs::remove_file(pitwall.join("db/y.toml")).unwrap();
    let fixed = agent
        .call(
            "fake_echo",
            on("box", json!({"projectId": REMOTE_ONLY, "tag": "x"})),
        )
        .await;
    assert_eq!(
        mark(&fixed),
        Mark::ok(),
        "대조: 고치면 돈다 — {}",
        text(&fixed)
    );
}

/// 리뷰 F1: 원격만 묶음에서 그 머신이 `mcp.target` 에 답하지 못하면(옛 판 — 없는 메서드, 다른 꼴의 답, 답이 오지 않음, 끊김) 노출 판정
/// 없이 넘어가지 않고 거절한다 — 계획 · 실행 0. 다른 꼴의 답(`exposed` 칸 없음)은 끈 것으로 본다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_remote_only_bundle_is_refused_when_that_machine_cannot_judge_it() {
    for (mode, expected) in [
        (1, Reason::MachineUnavailable),
        (2, Reason::NotExposed),
        (3, Reason::MachineUnavailable),
        (4, Reason::MachineUnavailable),
    ] {
        let r = Remote::new();
        remote_only(&r);
        r.machines.break_target.store(mode, Ordering::SeqCst);
        let _window = r.f.window(REMOTE_ONLY);
        let agent = r.agent("agent").await;
        for tool in ["fake_echo", "fake_write"] {
            let result = agent
                .call(
                    tool,
                    on("box", json!({"projectId": REMOTE_ONLY, "tag": "x"})),
                )
                .await;
            assert_eq!(
                mark(&result),
                Mark::denied(expected),
                "mode {mode} {tool}: {}",
                text(&result)
            );
        }
        assert_eq!((r.other.plans(), r.other.runs()), (0, 0), "mode {mode}");
    }
}

/// 리뷰 R1: `mcp.target` 은 에이전트 범위 연결만 부른다 — Host 범위(웰컴 · 설정 창, 링크의 Host 연결) · 그 프로젝트 창(`Project`)이
/// 부르면 `PERMISSION_DENIED`(문의 문장)이고, 에이전트 범위는 받는다(대조). 둘째 겹(`agent_grant`)은 같은 범위 판정과 레지스트리
/// 확인이라 첫 겹을 지나는 입력으로는 따로 드러나지 않는다(보고서 「리뷰 반영」).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn only_an_agent_connection_asks_for_a_target() {
    let r = Remote::new();
    let ask = |caller: Caller| {
        let host = r.other.host.clone();
        let project = r.other.project.clone();
        async move {
            let client =
                pw_mcp::HostClient::new(pw_core::Host::connect(&*host, caller).await.unwrap());
            client
                .call(
                    "mcp.target",
                    json!({"projectId": project, "tool": "fake_echo"}),
                )
                .await
        }
    };
    for caller in [
        Caller::new(Role::Owner),
        Caller {
            role: Role::Owner,
            scope: ConnectionScope::Project(r.other.project.clone()),
            window: Some("project-1".into()),
        },
    ] {
        let what = format!("{caller:?}");
        match ask(caller).await {
            Err(pw_mcp::CallError::Rpc { code, message, .. }) => {
                assert_eq!(code, i64::from(pw_rpc::code::PERMISSION_DENIED), "{what}");
                assert_eq!(
                    message, "only an mcp session asks about its target",
                    "{what}"
                );
            }
            other => panic!("{what}: {other:?}"),
        }
    }
    let agent = ask(Caller::agent(Role::Owner)).await.unwrap();
    assert_eq!(agent["exposed"], true, "{agent}");
}
