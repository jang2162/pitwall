//! 승인 경로(16-m5-plan.md M5-12): 진짜 rmcp 클라이언트 · 세션 · Host · 감사 파일, 가짜 쓰기 툴 메서드(계획을 만들고 승인 표를
//! 본다), 가짜 창(승인 허브를 직접 듣고 답한다 — `mcp.approvals` 를 연 창과 같은 자리), 시한은 설정의 짧은 값과 허브 단위 시험의
//! 멈춘 시계(`approval.rs`). RPC 길(`mcp.approvals` · `mcp.approval.answer` 와 에이전트 범위 거절)은 끝의 시험들이 본다.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use pw_core::config::AppPaths;
use pw_core::registry::agent::agent_grant;
use pw_core::registry::exec::{RunSpec, agent_run_grant};
use pw_core::registry::plan::{APPROVAL_KEY, PlanChange, PlanRules, Planned, Remember, ToolPlan};
use pw_core::registry::{NewProject, Registry};
use pw_core::{HostRouter, LocalHost};
use pw_mcp::approval::{
    AnswerError, Answerer, Connection, Listener, McpApprovalAnswer, McpApprovalEvent,
    McpApprovalRequest,
};
use pw_mcp::audit::AuditLog;
use pw_mcp::marker::{Mark, Reason};
use pw_mcp::methods::{self, HostParts, Mcp};
use pw_mcp::policy::Approval;
use pw_mcp::sessions::Sessions;
use pw_mcp::{Catalog, HostKind, Services, SessionRequest};
use pw_rpc::{Caller, ConnectionScope, Role, RpcError, Tier, rpc_method};
use rmcp::model::{
    CallToolRequest, CallToolRequestParams, CallToolResult, ClientCapabilities, ClientConfig,
    ClientRequest, Implementation,
};
use rmcp::service::{PeerRequestOptions, RunningService};
use rmcp::{RoleClient, ServiceExt};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ToolParams {
    pub project_id: String,
    #[serde(default)]
    pub worktree: Option<String>,
    /// 계획이 내용을 덮고 실행이 `tag` 를 쓰는 파일. `worktree` 가 있으면 그 아래 상대 경로, 없으면 절대 경로.
    #[serde(default)]
    pub file: Option<String>,
    #[serde(default)]
    pub tag: String,
    /// 승인을 청하는 조회 툴이 계획에 싣는 기억 열쇠(`Remember::Key`). 없으면 `tag`.
    #[serde(default)]
    pub key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
pub struct NoProjectParams {
    #[serde(default)]
    pub tag: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
pub struct ToolResult {
    pub audit: String,
}

rpc_method! {
    /// 가짜 쓰기 툴: 계획(파일 내용 · 입력)을 만들고 승인 표가 맞을 때만 그 파일에 `tag` 를 쓴다.
    pub FakeWrite: "approvaltest.write" {
        params: Planned<ToolParams>,
        result: ToolResult,
        tier: Write,
        mcp: { group: "project", name: "fake_write" },
    }

    /// 다른 쓰기 툴(세션 허용이 툴마다인지 본다).
    pub OtherWrite: "approvaltest.other" {
        params: Planned<ToolParams>,
        result: ToolResult,
        tier: Write,
        mcp: { group: "project", name: "other_write" },
    }

    /// `dangerous` 툴.
    pub FakeDangerous: "approvaltest.dangerous" {
        params: Planned<ToolParams>,
        result: ToolResult,
        tier: Dangerous,
        mcp: { group: "project", name: "fake_dangerous" },
    }

    /// 명령을 정의하는 쓰기(`open_with_upsert` 의 자리): `auto_approve` 도 세션 허용도 없다.
    pub FakeDefine: "approvaltest.define" {
        params: Planned<ToolParams>,
        result: ToolResult,
        tier: Write,
        mcp: { group: "project", name: "fake_define" },
    }

    /// 프로젝트 없는 쓰기(`project_register` 의 자리).
    pub FakeRegister: "approvaltest.register" {
        params: Planned<NoProjectParams>,
        result: ToolResult,
        tier: Write,
        mcp: { group: "project", name: "fake_register" },
    }

    /// 호출마다 승인을 청할 수 있는 조회 툴(「이 연결 열기」 의 자리): `tag` 가 `ask` 로 시작하면 승인을 청한다.
    pub FakeOpen: "approvaltest.open" {
        params: Planned<ToolParams>,
        result: ToolResult,
        tier: Read,
        mcp: { group: "project", name: "fake_open" },
    }

    /// `fake_open` 과 같은 열쇠를 내는 다른 조회 툴(허용이 툴을 넘지 않는지 본다).
    pub FakeOpenOther: "approvaltest.openOther" {
        params: Planned<ToolParams>,
        result: ToolResult,
        tier: Read,
        mcp: { group: "project", name: "fake_open_other" },
    }

    /// 규약을 어긴 쓰기 툴: 승인 표 없이 돈다(첫 호출에 부작용).
    pub FakeRogue: "approvaltest.rogue" {
        params: Planned<ToolParams>,
        result: ToolResult,
        tier: Write,
        mcp: { group: "project", name: "fake_rogue" },
    }

    /// 규약을 어긴 쓰기 툴: 선언(`write`)보다 높은 등급(`tag` — `exec` · `dangerous`)의 계획을 낸다.
    pub FakeOverTier: "approvaltest.overTier" {
        params: Planned<ToolParams>,
        result: ToolResult,
        tier: Write,
        mcp: { group: "project", name: "fake_over_tier" },
    }

    /// 규약을 어긴 쓰기 툴: 다른 메서드(`approvaltest.write`)의 계획을 낸다.
    pub FakeWrongMethod: "approvaltest.wrongMethod" {
        params: Planned<ToolParams>,
        result: ToolResult,
        tier: Write,
        mcp: { group: "project", name: "fake_wrong_method" },
    }

    /// `dangerous` 로 선언했지만 이 호출의 계획은 `write` 다(입력에 따라 등급이 낮은 호출).
    pub FakeDangerousLow: "approvaltest.dangerousLow" {
        params: Planned<ToolParams>,
        result: ToolResult,
        tier: Dangerous,
        mcp: { group: "project", name: "fake_dangerous_low" },
    }

    /// 실행 툴의 자리(17-m6-plan M6-10 — `run_start` 는 M6-14): 선언은 도달할 수 있는 가장 높은 `dangerous`, 계획 등급 · 규칙은
    /// `tag`(`<등급>[+policy][+human]`)가 정한다. 계획 때 프로젝트 루트의 `.env.local` 을 읽어 [`RunSpec`] 을 싣고, 승인 뒤
    /// [`agent_run_grant`] 의 허가가 준 것만 「띄운다」([`Probe::launched`] 에 남긴다 — 프로세스는 띄우지 않는다).
    pub FakeRun: "approvaltest.run" {
        params: Planned<ToolParams>,
        result: ToolResult,
        tier: Dangerous,
        mcp: { group: "project", name: "fake_run" },
    }

    /// 승인 뒤 「되돌릴 수 없는 지점」(`pw_rpc::PointOfNoReturn` — `db_execute` 의 커밋 자리)을 세우는 쓰기 툴. 시험이
    /// [`PointGate`] 로 그 앞뒤를 쥔다.
    pub FakeIrreversible: "approvaltest.irreversible" {
        params: Planned<ToolParams>,
        result: ToolResult,
        tier: Write,
        mcp: { group: "project", name: "fake_irreversible" },
    }
}

/// [`FakeIrreversible`] 의 앞뒤를 쥐는 자리. 효과를 내는 쪽은 핸들러가 버려져도 도는 태스크다(`db_execute` 의 실행 태스크처럼).
#[derive(Default)]
struct PointGate {
    /// 승인을 지나 지점 앞에 왔다.
    reached: tokio::sync::Notify,
    /// 지점을 세우러 간다.
    go: tokio::sync::Notify,
    /// 세웠는가(세우려 한 뒤).
    passed: std::sync::Mutex<Option<bool>>,
    decided: tokio::sync::Notify,
    /// 세운 뒤 Host 의 호출 토큰이 취소됐다(`request.cancel` 이 Host 에 닿았다).
    saw_cancel: tokio::sync::Notify,
    /// 끝낸다.
    finish: tokio::sync::Notify,
    /// 지점 뒤에 오류로 끝낸다(커밋 실패의 자리).
    fail: std::sync::atomic::AtomicBool,
}

/// `fake_run` 이 승인과 허가 사이에 한 번 부르는 것.
type Between = Box<dyn FnOnce() + Send>;

/// 가짜 툴이 불린 수.
#[derive(Clone, Default)]
struct Probe {
    /// 계획을 만든 수(첫 호출 · 승인 뒤 호출 모두).
    plans: Arc<AtomicUsize>,
    /// 승인을 지나 실제로 돈 수.
    runs: Arc<AtomicUsize>,
    /// `fake_irreversible` 의 앞뒤.
    gate: Arc<PointGate>,
    /// `fake_run` 이 허가로 받아 「띄운」 것.
    launched: Arc<std::sync::Mutex<Vec<RunSpec>>>,
    /// `fake_run` 이 승인을 지난 뒤 · 허가를 받기 전에 한 번 부르는 것(그 사이에 파일이 바뀌는 자리).
    between: Arc<std::sync::Mutex<Option<Between>>>,
    /// `fake_run` 이 관문(`agent_grant`)에 쓰는 레지스트리 — Host 를 만든 뒤 채운다.
    registry: Arc<std::sync::OnceLock<Registry>>,
}

/// `fake_run` 의 계획 등급과 규칙: `tag` 는 `<read|write|exec|dangerous>[+policy][+human]`. `human` 은 사람이 띄운 실행을 겨누는
/// 꼴이다(결정 D69 — `auto_approve` 없음, 그 실행 하나의 열쇠).
fn run_plan_rules(p: &ToolParams) -> (Tier, PlanRules) {
    let mut parts = p.tag.split('+');
    let tier = match parts.next() {
        Some("read") => Tier::Read,
        Some("write") => Tier::Write,
        Some("dangerous") => Tier::Dangerous,
        _ => Tier::Exec,
    };
    let mut rules = PlanRules::default();
    for flag in parts {
        match flag {
            "policy" => rules.policy_auto = true,
            "human" => {
                rules.auto_approve = false;
                rules.remember = Remember::Key {
                    key: p.key.clone().unwrap_or_else(|| "human:r1".into()),
                };
            }
            other => panic!("unknown flag {other}"),
        }
    }
    (tier, rules)
}

/// `.env` 꼴(`KEY=VALUE` 줄)을 읽는다. 없으면 빈 것.
fn read_env(file: &Path) -> std::collections::BTreeMap<String, String> {
    std::fs::read_to_string(file)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))
        .collect()
}

fn target_file(p: &ToolParams) -> Option<PathBuf> {
    p.file.as_ref().map(|f| match &p.worktree {
        Some(w) => Path::new(w).join(f),
        None => PathBuf::from(f),
    })
}

fn plan_of(method: &str, tier: Tier, rules: PlanRules, p: &ToolParams) -> ToolPlan {
    let file = target_file(p);
    let content = file.as_ref().map(|f| std::fs::read_to_string(f).ok());
    let mut plan = ToolPlan::builder(method, tier, p)
        .summary(format!("{method} {}", p.tag))
        .target("content", &content)
        .rules(rules);
    if let Some(file) = &file {
        plan = plan.change(PlanChange::File {
            path: file.display().to_string(),
            before: content.flatten(),
            after: Some(p.tag.clone()),
        });
    }
    plan.build()
}

async fn execute(
    probe: Probe,
    plan: ToolPlan,
    p: Planned<ToolParams>,
) -> Result<ToolResult, RpcError> {
    probe.plans.fetch_add(1, Ordering::SeqCst);
    let approved = p.approve(plan)?;
    probe.runs.fetch_add(1, Ordering::SeqCst);
    if let Some(file) = target_file(&p.params) {
        std::fs::write(file, &p.params.tag).unwrap();
    }
    Ok(ToolResult {
        audit: format!("tag={} {}", p.params.tag, approved.hash()),
    })
}

fn register_tools(router: &mut pw_rpc::Router, probe: &Probe) {
    let plain = PlanRules::default();
    let never = PlanRules {
        auto_approve: false,
        remember: Remember::Never,
        policy_auto: false,
    };
    for (method, tier, rules) in [
        ("approvaltest.write", Tier::Write, plain.clone()),
        ("approvaltest.other", Tier::Write, plain.clone()),
        ("approvaltest.dangerous", Tier::Dangerous, plain.clone()),
        ("approvaltest.define", Tier::Write, never.clone()),
    ] {
        let probe = probe.clone();
        let handler = move |_ctx, p: Planned<ToolParams>| {
            let plan = plan_of(method, tier, rules.clone(), &p.params);
            execute(probe.clone(), plan, p)
        };
        match method {
            "approvaltest.write" => router.unary(FakeWrite, handler),
            "approvaltest.other" => router.unary(OtherWrite, handler),
            "approvaltest.dangerous" => router.unary(FakeDangerous, handler),
            _ => router.unary(FakeDefine, handler),
        };
    }
    let register = probe.clone();
    router.unary(FakeRegister, move |_ctx, p: Planned<NoProjectParams>| {
        let probe = register.clone();
        async move {
            probe.plans.fetch_add(1, Ordering::SeqCst);
            let plan = ToolPlan::builder("approvaltest.register", Tier::Write, &p.params)
                .summary("register")
                .build();
            p.approve(plan)?;
            probe.runs.fetch_add(1, Ordering::SeqCst);
            Ok::<_, RpcError>(ToolResult {
                audit: format!("tag={}", p.params.tag),
            })
        }
    });
    for method in ["approvaltest.open", "approvaltest.openOther"] {
        let probe = probe.clone();
        let handler = move |_ctx, p: Planned<ToolParams>| {
            let probe = probe.clone();
            async move {
                if !p.params.tag.starts_with("ask") {
                    probe.runs.fetch_add(1, Ordering::SeqCst);
                    return Ok(ToolResult {
                        audit: "no approval".into(),
                    });
                }
                let rules = PlanRules {
                    auto_approve: false,
                    remember: Remember::Key {
                        key: p.params.key.clone().unwrap_or_else(|| p.params.tag.clone()),
                    },
                    policy_auto: false,
                };
                let plan = plan_of(method, Tier::Read, rules, &p.params);
                execute(probe, plan, p).await
            }
        };
        match method {
            "approvaltest.open" => router.unary(FakeOpen, handler),
            _ => router.unary(FakeOpenOther, handler),
        };
    }
    let rogue = probe.clone();
    router.unary(FakeRogue, move |_ctx, p: Planned<ToolParams>| {
        let probe = rogue.clone();
        async move {
            probe.runs.fetch_add(1, Ordering::SeqCst);
            Ok::<_, RpcError>(ToolResult {
                audit: format!("secret result {}", p.params.tag),
            })
        }
    });
    let over = probe.clone();
    router.unary(FakeOverTier, move |_ctx, p: Planned<ToolParams>| {
        let tier = if p.params.tag == "dangerous" {
            Tier::Dangerous
        } else {
            Tier::Exec
        };
        let plan = plan_of(
            "approvaltest.overTier",
            tier,
            PlanRules::default(),
            &p.params,
        );
        execute(over.clone(), plan, p)
    });
    let wrong = probe.clone();
    router.unary(FakeWrongMethod, move |_ctx, p: Planned<ToolParams>| {
        let plan = plan_of(
            "approvaltest.write",
            Tier::Write,
            PlanRules::default(),
            &p.params,
        );
        execute(wrong.clone(), plan, p)
    });
    let irreversible = probe.clone();
    router.unary(FakeIrreversible, move |ctx, p: Planned<ToolParams>| {
        let probe = irreversible.clone();
        let point = ctx.point_of_no_return();
        let token = ctx.cancellation().clone();
        async move {
            probe.plans.fetch_add(1, Ordering::SeqCst);
            let plan = plan_of(
                "approvaltest.irreversible",
                Tier::Write,
                PlanRules::default(),
                &p.params,
            );
            let approved = p.approve(plan)?;
            let gate = Arc::clone(&probe.gate);
            let task = tokio::spawn(async move {
                gate.reached.notify_one();
                gate.go.notified().await;
                let passed = point.pass();
                *gate.passed.lock().unwrap() = Some(passed);
                gate.decided.notify_one();
                if passed {
                    token.cancelled().await;
                    gate.saw_cancel.notify_one();
                    gate.finish.notified().await;
                }
                passed
            });
            if task.await.unwrap() {
                probe.runs.fetch_add(1, Ordering::SeqCst);
                if probe.gate.fail.load(Ordering::SeqCst) {
                    return Err(RpcError::new(3003, "the commit failed"));
                }
            }
            Ok::<_, RpcError>(ToolResult {
                audit: format!("irreversible {}", approved.hash()),
            })
        }
    });
    let run = probe.clone();
    router.unary(FakeRun, move |ctx, p: Planned<ToolParams>| {
        let probe = run.clone();
        let scope = ctx.scope().clone();
        async move {
            probe.plans.fetch_add(1, Ordering::SeqCst);
            let registry = probe.registry.get().expect("registry").clone();
            let agent = agent_grant(&scope, &registry, &p.params.project_id)?;
            let root = PathBuf::from(&agent.project().roots[0]);
            let spec = RunSpec {
                config: "dev".into(),
                argv: vec!["node".into(), "server.js".into()],
                env: read_env(&root.join(".env.local")),
                cwd: root,
                profile: None,
            };
            let (tier, rules) = run_plan_rules(&p.params);
            let plan = ToolPlan::builder("approvaltest.run", tier, &p.params)
                .summary(format!("run dev ({})", p.params.tag))
                .change(PlanChange::Text {
                    label: "run".into(),
                    text: format!("node server.js · env {:?}", spec.env.keys()),
                })
                .rules(rules)
                .runs([spec])
                .build();
            let approved = p.approve(plan)?;
            let between = probe.between.lock().unwrap().take();
            if let Some(between) = between {
                between();
            }
            // 띄우는 쪽은 허가의 RunSpec 만 받는다 — 구성 · `.env.local` 을 다시 읽지 않는다.
            let grant = agent_run_grant(&agent, &approved)?;
            probe.runs.fetch_add(1, Ordering::SeqCst);
            probe
                .launched
                .lock()
                .unwrap()
                .extend(grant.runs().iter().cloned());
            Ok::<_, RpcError>(ToolResult {
                audit: format!("tag={}", p.params.tag),
            })
        }
    });
    let low = probe.clone();
    router.unary(FakeDangerousLow, move |_ctx, p: Planned<ToolParams>| {
        let plan = plan_of(
            "approvaltest.dangerousLow",
            Tier::Write,
            PlanRules::default(),
            &p.params,
        );
        execute(low.clone(), plan, p)
    });
}

/// `[mcp.tools]` 의 공통 줄 — `dangerous` 툴은 프리셋 `standard` 에 없다.
const TOOLS: &str =
    "[mcp.tools]\nfake_dangerous = true\nfake_dangerous_low = true\nfake_run = true\n";

struct Fixture {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    paths: AppPaths,
    host: Arc<LocalHost>,
    sessions: Sessions,
    probe: Probe,
    alpha: String,
    beta: String,
}

impl Fixture {
    fn new() -> Self {
        Self::with_global("")
    }

    /// 전역 `settings.toml` 에 `global` 을 넣은 Fixture(`[mcp]` 의 키를 쓰면 `[mcp.tools]` 앞에 온다).
    fn with_global(global: &str) -> Self {
        Self::build(global, |_| {})
    }

    /// 프로젝트를 등록하기 전에 그 루트에 파일을 둔다(`.pitwall/*.toml`).
    fn build(global: &str, before: impl FnOnce(&Path)) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let base = pw_core::registry::path::simplify(std::fs::canonicalize(tmp.path()).unwrap());
        let paths = AppPaths::under(&base.join("app"));
        std::fs::create_dir_all(&paths.config).unwrap();
        std::fs::write(
            paths.config.join("settings.toml"),
            format!("schema = 1\n{global}{TOOLS}"),
        )
        .unwrap();
        for name in ["alpha", "beta"] {
            std::fs::create_dir_all(base.join("work").join(name)).unwrap();
        }
        before(&base.join("work"));
        let sessions = Sessions::new();
        let mcp = Arc::new(Mcp::new(sessions.clone()));
        let mcp_methods = Arc::clone(&mcp);
        let probe = Probe::default();
        let tools = probe.clone();
        let host = Arc::new(
            LocalHost::builder("local")
                .config(paths.clone())
                .methods(move |router| register_tools(router, &tools))
                .methods(move |router| methods::register(router, mcp_methods))
                .build(),
        );
        let registry = host.registry().unwrap();
        probe.registry.set(registry.clone()).ok().unwrap();
        let mut ids = ["alpha", "beta"].iter().map(|name| {
            registry
                .create(NewProject {
                    root: base.join("work").join(name),
                    ..NewProject::default()
                })
                .unwrap()
                .id
        });
        let (alpha, beta) = (ids.next().unwrap(), ids.next().unwrap());
        let config = host.config().unwrap();
        mcp.attach(HostParts {
            paths: paths.clone(),
            global: config.global.clone(),
            projects: registry.clone(),
            catalog: Arc::new(Catalog::linked().routed_by(host.router())),
            audit: Arc::new(AuditLog::under_state(&paths.state)),
        });
        Self {
            _tmp: tmp,
            base,
            paths,
            host,
            sessions,
            probe,
            alpha,
            beta,
        }
    }

    fn services(&self, host_kind: HostKind) -> Arc<Services> {
        let router = Arc::new(HostRouter::new());
        router.add_host(self.host.clone()).unwrap();
        Arc::new(Services {
            router,
            host_id: "local".into(),
            paths: self.paths.clone(),
            global: self.host.config().unwrap().global.clone(),
            bus: self.host.bus().clone(),
            audit: Arc::new(AuditLog::under_state(&self.paths.state)),
            catalog: Arc::new(Catalog::linked().routed_by(self.host.router())),
            approval: Approval::for_host(host_kind, &self.sessions),
            transport: "test",
            host_kind,
            sessions: self.sessions.clone(),
        })
    }

    fn work(&self, name: &str) -> PathBuf {
        self.base.join("work").join(name)
    }

    /// 세션 하나: 클라이언트 이름 `client`, 세션 프로젝트는 `cwd` 를 담은 프로젝트.
    async fn agent(&self, client: &str, cwd: &Path) -> Agent {
        self.agent_on(client, cwd, HostKind::App).await
    }

    async fn agent_on(&self, client: &str, cwd: &Path, host_kind: HostKind) -> Agent {
        let (server_io, client_io) = tokio::io::duplex(64 * 1024);
        let (r, w) = tokio::io::split(server_io);
        let cancel = CancellationToken::new();
        tokio::spawn(pw_mcp::serve(
            self.services(host_kind),
            SessionRequest {
                project: None,
                cwd: Some(cwd.to_path_buf()),
            },
            (r, w),
            cancel.clone(),
        ));
        let config = ClientConfig::new(
            ClientCapabilities::default(),
            Implementation::new(client, "1"),
        );
        let running = config.serve(client_io).await.unwrap();
        Agent {
            running,
            _cancel: cancel,
        }
    }

    /// 그 프로젝트 창(가짜): 승인 허브를 그 창의 연결로 듣는다.
    fn window(&self, project: &str) -> Window {
        let connection = Connection::named(&format!("window-{project}"));
        Window {
            listener: self.sessions.approvals().listen(
                Some(project),
                Some(&format!("w-{project}")),
                connection.clone(),
            ),
            project: Some(project.to_owned()),
            connection,
            sessions: self.sessions.clone(),
        }
    }

    /// 프로젝트 없는 창(웰컴 창의 자리).
    fn welcome(&self) -> Window {
        let connection = Connection::named("welcome");
        Window {
            listener: self
                .sessions
                .approvals()
                .listen(None, Some("welcome"), connection.clone()),
            project: None,
            connection,
            sessions: self.sessions.clone(),
        }
    }

    fn runs(&self) -> usize {
        self.probe.runs.load(Ordering::SeqCst)
    }

    /// `fake_run` 이 「띄운」 것.
    fn launched(&self) -> Vec<RunSpec> {
        self.probe.launched.lock().unwrap().clone()
    }

    fn plans(&self) -> usize {
        self.probe.plans.load(Ordering::SeqCst)
    }

    /// 감사 행(쓰기는 큐로 끝난다 — `n` 개가 될 때까지 기다린다).
    fn rows(&self, n: usize) -> Vec<Row> {
        let file = self.paths.state.join("mcp/audit.db");
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            let rows = rusqlite::Connection::open(&file)
                .ok()
                .and_then(|conn| {
                    let mut stmt = conn
                        .prepare(
                            "SELECT session, tool, outcome, detail, approval, plan_hash FROM calls ORDER BY id",
                        )
                        .ok()?;
                    let rows = stmt
                        .query_map([], |r| {
                            Ok(Row {
                                session: r.get(0)?,
                                tool: r.get(1)?,
                                outcome: r.get(2)?,
                                detail: r.get(3)?,
                                approval: r.get(4)?,
                                plan_hash: r.get(5)?,
                            })
                        })
                        .ok()?
                        .map(Result::unwrap)
                        .collect::<Vec<_>>();
                    Some(rows)
                })
                .unwrap_or_default();
            // 선기록한 행은 끝 기록이 따로 온다 — `started` 가 없을 때까지.
            if rows.len() >= n && rows.iter().all(|r| r.outcome != "started") {
                return rows;
            }
            assert!(std::time::Instant::now() < deadline, "audit rows: {rows:?}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Row {
    session: String,
    tool: String,
    outcome: String,
    detail: Option<String>,
    approval: Option<String>,
    plan_hash: Option<String>,
}

struct Agent {
    running: RunningService<RoleClient, ClientConfig>,
    _cancel: CancellationToken,
}

impl Agent {
    /// 호출을 띄운다(답을 기다리지 않는다).
    fn spawn(&self, tool: &str, args: Value) -> tokio::task::JoinHandle<CallToolResult> {
        let peer = self.running.peer().clone();
        let mut params = CallToolRequestParams::new(tool.to_owned());
        if let Value::Object(map) = args {
            params = params.with_arguments(map);
        }
        tokio::spawn(async move { peer.call_tool(params).await.unwrap() })
    }

    async fn call(&self, tool: &str, args: Value) -> CallToolResult {
        within(self.spawn(tool, args)).await.unwrap()
    }
}

struct Window {
    listener: Listener,
    project: Option<String>,
    connection: Connection,
    sessions: Sessions,
}

impl Window {
    async fn next(&mut self) -> McpApprovalEvent {
        within(self.listener.recv()).await.expect("listener ended")
    }

    async fn opened(&mut self) -> McpApprovalRequest {
        match self.next().await {
            McpApprovalEvent::Open { request } => *request,
            other => panic!("expected a request, got {other:?}"),
        }
    }

    async fn closed(&mut self, id: u64) {
        assert_eq!(self.next().await, McpApprovalEvent::Closed { id });
    }

    fn answer(&self, id: u64, answer: McpApprovalAnswer) -> Result<(), AnswerError> {
        let scope = match &self.project {
            Some(p) => ConnectionScope::Project(p.clone()),
            None => ConnectionScope::Host,
        };
        self.sessions.approvals().answer(
            &Answerer {
                scope: &scope,
                connection: self.connection.clone(),
            },
            id,
            answer,
        )
    }

    fn quiet(&mut self) -> bool {
        self.listener.try_recv().is_none()
    }
}

async fn within<F: Future>(f: F) -> F::Output {
    tokio::time::timeout(Duration::from_secs(10), f)
        .await
        .expect("timed out")
}

fn mark(result: &CallToolResult) -> Mark {
    Mark::read(&result.meta.as_ref().expect("no _meta").0).expect("no outcome mark")
}

fn text(result: &CallToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|c| c.as_text().map(|t| t.text.clone()))
        .collect()
}

/// 허용 → 실행, 승인을 지난 호출의 감사 행에 승인 방식과 해시가 있다. 계획 단계(첫 호출)는 부작용이 없다 — 사람이 답하기 전에는
/// 파일이 그대로다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_allowed_call_runs_once_and_its_row_keeps_the_approval_and_the_hash() {
    let f = Fixture::new();
    let file = f.work("alpha").join("a.txt");
    std::fs::write(&file, "before").unwrap();
    let mut window = f.window(&f.alpha);
    let agent = f.agent("agent", &f.work("alpha")).await;
    let call = agent.spawn(
        "fake_write",
        json!({"file": file.display().to_string(), "tag": "after"}),
    );
    let request = window.opened().await;
    assert_eq!(request.tool, "fake_write");
    assert_eq!(request.tier, Tier::Write);
    assert_eq!(request.project_id.as_deref(), Some(f.alpha.as_str()));
    assert_eq!(request.client.as_deref(), Some("agent 1"));
    assert!(!request.cli);
    assert_eq!(
        request.answers,
        [
            McpApprovalAnswer::Deny,
            McpApprovalAnswer::Once,
            McpApprovalAnswer::Session
        ]
    );
    assert_eq!(
        request.plan.changes,
        [PlanChange::File {
            path: file.display().to_string(),
            before: Some("before".into()),
            after: Some("after".into()),
        }]
    );
    // 계획 단계: 툴은 계획만 만들었다.
    assert_eq!((f.plans(), f.runs()), (1, 0));
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "before");
    assert_eq!(
        f.rows(0).len(),
        0,
        "답하기 전에는 감사 행도 없다(선기록은 승인 뒤)"
    );

    window.answer(request.id, McpApprovalAnswer::Once).unwrap();
    let result = within(call).await.unwrap();
    assert_eq!(mark(&result), Mark::ok(), "{result:?}");
    window.closed(request.id).await;
    assert_eq!((f.plans(), f.runs()), (2, 1));
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "after");
    let rows = f.rows(1);
    assert_eq!(rows.len(), 1);
    assert_eq!(
        (
            rows[0].tool.as_str(),
            rows[0].outcome.as_str(),
            rows[0].approval.as_deref(),
            rows[0].plan_hash.as_deref()
        ),
        (
            "fake_write",
            "ok",
            Some("once"),
            Some(request.plan.hash.as_str())
        )
    );
    // [이번만 허용] 은 다음 호출에 닿지 않는다.
    let again = agent.spawn("fake_write", json!({"tag": "x"}));
    let second = window.opened().await;
    window.answer(second.id, McpApprovalAnswer::Deny).unwrap();
    within(again).await.unwrap();
}

/// 거절 · 시한 초과 → 실행하지 않고 감사 `denied` 와 까닭.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn denied_and_timed_out_calls_do_not_run() {
    let f = Fixture::with_global("[mcp.policy]\napproval_timeout_secs = 1\n");
    let mut window = f.window(&f.alpha);
    let agent = f.agent("agent", &f.work("alpha")).await;

    let call = agent.spawn("fake_write", json!({"tag": "no"}));
    let request = window.opened().await;
    assert_eq!(request.timeout_secs, 1, "전역 [mcp.policy] 의 시한");
    window.answer(request.id, McpApprovalAnswer::Deny).unwrap();
    let denied = within(call).await.unwrap();
    assert_eq!(mark(&denied), Mark::denied(Reason::ApprovalDenied));
    assert_eq!(denied.is_error, Some(true));
    window.closed(request.id).await;

    let call = agent.spawn("fake_write", json!({"tag": "late"}));
    let request = window.opened().await;
    let started = std::time::Instant::now();
    let timed_out = within(call).await.unwrap();
    assert!(started.elapsed() >= Duration::from_millis(900));
    assert_eq!(mark(&timed_out), Mark::denied(Reason::ApprovalTimeout));
    assert!(
        text(&timed_out).contains("within 1 seconds"),
        "{}",
        text(&timed_out)
    );
    window.closed(request.id).await;
    // 시한이 지난 요청에는 답할 수 없다.
    assert_eq!(
        window.answer(request.id, McpApprovalAnswer::Once),
        Err(AnswerError::NotFound)
    );

    assert_eq!(f.runs(), 0);
    let rows = f.rows(2);
    let summary: Vec<_> = rows
        .iter()
        .map(|r| (r.outcome.as_str(), r.approval.as_deref()))
        .collect();
    assert_eq!(summary, [("denied", None), ("denied", None)]);
    assert!(
        rows[0]
            .detail
            .as_deref()
            .unwrap()
            .contains("denied in Pitwall")
    );
    assert!(
        rows[1]
            .detail
            .as_deref()
            .unwrap()
            .contains("within 1 seconds")
    );
}

/// 승인 뒤 계획이 바뀌면(파일 내용) 돌지 않고, 다시 묻지 않고 `plan_changed` 로 거절한다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_plan_that_changes_after_approval_does_not_run_and_is_not_asked_again() {
    let f = Fixture::new();
    let file = f.work("alpha").join("a.txt");
    std::fs::write(&file, "seen by the person").unwrap();
    let mut window = f.window(&f.alpha);
    let agent = f.agent("agent", &f.work("alpha")).await;
    let call = agent.spawn(
        "fake_write",
        json!({"file": file.display().to_string(), "tag": "x"}),
    );
    let request = window.opened().await;
    std::fs::write(&file, "changed behind the dialog").unwrap();
    window.answer(request.id, McpApprovalAnswer::Once).unwrap();
    let result = within(call).await.unwrap();
    assert_eq!(mark(&result), Mark::denied(Reason::PlanChanged));
    assert!(
        text(&result).contains("changed after it was approved"),
        "{}",
        text(&result)
    );
    assert!(
        !text(&result).contains("sha256:"),
        "계획 · 해시가 에이전트에게 나갔다"
    );
    window.closed(request.id).await;
    assert!(window.quiet(), "다시 물었다");
    assert_eq!(f.runs(), 0);
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "changed behind the dialog"
    );
    let rows = f.rows(1);
    assert_eq!(
        (rows[0].outcome.as_str(), rows[0].approval.as_deref()),
        ("denied", Some("once")),
        "선기록한 행이 거절로 끝난다"
    );
}

/// `git worktree add` 가 만드는 모양: `main` 의 연결 워크트리. 돌려주는 것은 워크트리 꼭대기.
fn fake_worktree(main: &Path, wt: &Path) -> PathBuf {
    let admin = main.join(".git/worktrees/wt");
    std::fs::create_dir_all(&admin).unwrap();
    std::fs::write(admin.join("commondir"), "../..\n").unwrap();
    std::fs::create_dir_all(wt).unwrap();
    std::fs::write(wt.join(".git"), format!("gitdir: {}\n", admin.display())).unwrap();
    std::fs::write(
        admin.join("gitdir"),
        format!("{}\n", wt.join(".git").display()),
    )
    .unwrap();
    pw_core::registry::path::simplify(std::fs::canonicalize(wt).unwrap())
}

/// 서버가 채운 `worktree` 도 계획에 든다: 세션이 연결 워크트리 안에서 열려 서버가 `worktree` 를 채우고, 툴은 그 워크트리의
/// 파일을 계획에 넣는다. 승인 뒤 그 워크트리의 파일이 바뀌면 돌지 않는다. 승인 표의 해시는 채운 입력을 덮는다 — 같은 입력을
/// 채우지 않은 계획(메인 체크아웃)과 해시가 다르다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_server_filled_worktree_is_part_of_the_plan() {
    let f = Fixture::new();
    let top = fake_worktree(&f.work("alpha"), &f.base.join("wt-alpha"));
    std::fs::write(top.join("a.txt"), "worktree").unwrap();
    std::fs::write(f.work("alpha").join("a.txt"), "worktree").unwrap();
    let mut window = f.window(&f.alpha);
    let in_worktree = f.agent("agent", &top).await;
    let call = in_worktree.spawn("fake_write", json!({"file": "a.txt", "tag": "x"}));
    let request = window.opened().await;
    let PlanChange::File { path, .. } = &request.plan.changes[0] else {
        panic!("{:?}", request.plan.changes)
    };
    assert_eq!(Path::new(path), top.join("a.txt"), "서버가 채운 워크트리");
    std::fs::write(top.join("a.txt"), "changed").unwrap();
    window.answer(request.id, McpApprovalAnswer::Once).unwrap();
    assert_eq!(
        mark(&within(call).await.unwrap()),
        Mark::denied(Reason::PlanChanged)
    );
    window.closed(request.id).await;

    // 같은 입력이라도 채운 워크트리가 다르면 다른 해시다.
    let in_main = f.agent("agent", &f.work("alpha")).await;
    let call = in_main.spawn(
        "fake_write",
        json!({"file": top.join("a.txt").display().to_string(), "tag": "x"}),
    );
    let main_request = window.opened().await;
    assert_ne!(main_request.plan.hash, request.plan.hash);
    window
        .answer(main_request.id, McpApprovalAnswer::Deny)
        .unwrap();
    within(call).await.unwrap();
    assert_eq!(f.runs(), 0);
}

/// 에이전트가 승인 키를 직접 실어도 버려진다 — 사람이 본 계획의 해시를 그대로 실어도 다시 묻는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_approval_key_the_agent_sends_is_dropped() {
    let f = Fixture::new();
    let mut window = f.window(&f.alpha);
    let agent = f.agent("agent", &f.work("alpha")).await;
    let call = agent.spawn("fake_write", json!({"tag": "x"}));
    let request = window.opened().await;
    window.answer(request.id, McpApprovalAnswer::Deny).unwrap();
    within(call).await.unwrap();
    window.closed(request.id).await;

    // 같은 입력 · 같은 해시를 에이전트가 승인 표 자리에 싣는다.
    let forged = json!({"tag": "x", APPROVAL_KEY: {"hash": request.plan.hash}});
    let call = agent.spawn("fake_write", forged.clone());
    let asked = window.opened().await;
    assert_eq!(asked.plan.hash, request.plan.hash, "같은 계획이다");
    window.answer(asked.id, McpApprovalAnswer::Deny).unwrap();
    assert_eq!(
        mark(&within(call).await.unwrap()),
        Mark::denied(Reason::ApprovalDenied)
    );
    window.closed(asked.id).await;
    // 승인을 청하는 조회 툴도 같다.
    let call = agent.spawn(
        "fake_open",
        json!({"tag": "ask", APPROVAL_KEY: {"hash": "x"}}),
    );
    let asked = window.opened().await;
    window.answer(asked.id, McpApprovalAnswer::Deny).unwrap();
    within(call).await.unwrap();
    assert_eq!(f.runs(), 0);
    // 감사 행의 인자 요약에도 그 키가 없다.
    f.rows(3);
    let conn = rusqlite::Connection::open(f.paths.state.join("mcp/audit.db")).unwrap();
    let args: Vec<String> = conn
        .prepare("SELECT args FROM calls")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(args.len(), 3);
    assert!(args.iter().all(|a| !a.contains(APPROVAL_KEY)), "{args:?}");
}

/// [이 세션 동안 허용] 은 그 세션 · 그 프로젝트 · 그 툴에만 닿는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_session_allowance_reaches_only_that_session_tool_and_project() {
    let f = Fixture::new();
    let mut alpha = f.window(&f.alpha);
    let mut beta = f.window(&f.beta);
    let agent = f.agent("agent", &f.work("alpha")).await;
    let call = agent.spawn("fake_write", json!({"tag": "1"}));
    let request = alpha.opened().await;
    alpha
        .answer(request.id, McpApprovalAnswer::Session)
        .unwrap();
    assert_eq!(mark(&within(call).await.unwrap()), Mark::ok());
    alpha.closed(request.id).await;

    // 같은 세션 · 프로젝트 · 툴: 묻지 않는다.
    assert_eq!(
        mark(&agent.call("fake_write", json!({"tag": "2"})).await),
        Mark::ok()
    );
    assert!(alpha.quiet());
    // 다른 툴: 묻는다.
    let call = agent.spawn("other_write", json!({"tag": "3"}));
    let other_tool = alpha.opened().await;
    assert_eq!(other_tool.tool, "other_write");
    alpha
        .answer(other_tool.id, McpApprovalAnswer::Deny)
        .unwrap();
    within(call).await.unwrap();
    alpha.closed(other_tool.id).await;
    // 다른 프로젝트: 그 프로젝트 창에 묻는다.
    let call = agent.spawn("fake_write", json!({"projectId": f.beta, "tag": "4"}));
    let other_project = beta.opened().await;
    assert_eq!(other_project.project_id.as_deref(), Some(f.beta.as_str()));
    beta.answer(other_project.id, McpApprovalAnswer::Deny)
        .unwrap();
    within(call).await.unwrap();
    // 다른 세션: 묻는다.
    let second = f.agent("agent", &f.work("alpha")).await;
    let call = second.spawn("fake_write", json!({"tag": "5"}));
    let other_session = alpha.opened().await;
    assert_ne!(other_session.session, request.session);
    alpha
        .answer(other_session.id, McpApprovalAnswer::Deny)
        .unwrap();
    within(call).await.unwrap();

    assert_eq!(f.runs(), 2);
    let rows = f.rows(5);
    let approvals: Vec<_> = rows
        .iter()
        .map(|r| (r.tool.as_str(), r.outcome.as_str(), r.approval.as_deref()))
        .collect();
    assert_eq!(
        approvals,
        [
            ("fake_write", "ok", Some("session")),
            ("fake_write", "ok", Some("session")),
            ("other_write", "denied", None),
            ("fake_write", "denied", None),
            ("fake_write", "denied", None),
        ]
    );
    assert_ne!(
        rows[0].plan_hash, rows[1].plan_hash,
        "호출마다 그 계획의 해시"
    );
}

/// CLI 허용: 다른 CLI 세션의 같은 툴도 묻지 않고(창이 열려 있는 동안), 그 창을 닫으면 사라진다. CLI 가 아닌 세션에는 닿지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_cli_allowance_lasts_while_the_window_is_open() {
    let f = Fixture::new();
    let mut window = f.window(&f.alpha);
    let first = f.agent(pw_mcp::server::CLI_CLIENT, &f.work("alpha")).await;
    let call = first.spawn("fake_write", json!({"tag": "1"}));
    let request = window.opened().await;
    assert!(request.cli);
    assert_eq!(
        request.answers,
        [
            McpApprovalAnswer::Deny,
            McpApprovalAnswer::Once,
            McpApprovalAnswer::Window
        ]
    );
    assert_eq!(
        window.answer(request.id, McpApprovalAnswer::Session),
        Err(AnswerError::NotOffered)
    );
    window
        .answer(request.id, McpApprovalAnswer::Window)
        .unwrap();
    assert_eq!(mark(&within(call).await.unwrap()), Mark::ok());
    window.closed(request.id).await;

    // 다른 CLI 세션(호출마다 새 세션이다)의 같은 툴: 묻지 않는다.
    let second = f.agent(pw_mcp::server::CLI_CLIENT, &f.work("alpha")).await;
    assert_eq!(
        mark(&second.call("fake_write", json!({"tag": "2"})).await),
        Mark::ok()
    );
    assert!(window.quiet());
    // CLI 가 아닌 세션 · 다른 툴은 묻는다.
    let agent = f.agent("agent", &f.work("alpha")).await;
    let call = agent.spawn("fake_write", json!({"tag": "3"}));
    let asked = window.opened().await;
    window.answer(asked.id, McpApprovalAnswer::Deny).unwrap();
    within(call).await.unwrap();
    window.closed(asked.id).await;
    let call = second.spawn("other_write", json!({"tag": "4"}));
    let asked = window.opened().await;
    window.answer(asked.id, McpApprovalAnswer::Deny).unwrap();
    within(call).await.unwrap();

    // 창을 닫으면(듣기를 그만두면) 사라진다 — 다시 연 창은 다시 묻는다.
    drop(window);
    let mut reopened = f.window(&f.alpha);
    let third = f.agent(pw_mcp::server::CLI_CLIENT, &f.work("alpha")).await;
    let call = third.spawn("fake_write", json!({"tag": "5"}));
    let asked = reopened.opened().await;
    reopened.answer(asked.id, McpApprovalAnswer::Deny).unwrap();
    within(call).await.unwrap();

    let rows = f.rows(5);
    let approvals: Vec<_> = rows.iter().map(|r| r.approval.as_deref()).collect();
    assert_eq!(
        approvals,
        [Some("window"), Some("window"), None, None, None]
    );
    assert_ne!(
        rows[0].session, rows[1].session,
        "CLI 는 호출마다 새 세션이다"
    );
}

/// `auto_approve = ["write"]` 면 묻지 않는다(감사 행의 승인 방식 `auto`). `dangerous` 는 `auto_approve` 에 있어도 매번 묻고
/// 셋째 답이 없다. 명령을 정의하는 쓰기(`Remember::Never` · `auto_approve: false`)도 묻고 셋째 답이 없다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn auto_approve_skips_the_question_except_for_dangerous_and_defining_tools() {
    let f = Fixture::with_global("[mcp]\nauto_approve = [\"write\", \"dangerous\"]\n");
    let mut window = f.window(&f.alpha);
    let agent = f.agent("agent", &f.work("alpha")).await;
    assert_eq!(
        mark(&agent.call("fake_write", json!({"tag": "1"})).await),
        Mark::ok()
    );
    assert!(window.quiet(), "auto_approve 인데 물었다");

    for tool in ["fake_dangerous", "fake_dangerous", "fake_define"] {
        let call = agent.spawn(tool, json!({"tag": "x"}));
        let request = window.opened().await;
        assert_eq!(request.tool, tool);
        assert_eq!(
            request.answers,
            [McpApprovalAnswer::Deny, McpApprovalAnswer::Once],
            "{tool}"
        );
        assert_eq!(
            window.answer(request.id, McpApprovalAnswer::Session),
            Err(AnswerError::NotOffered)
        );
        window.answer(request.id, McpApprovalAnswer::Once).unwrap();
        assert_eq!(mark(&within(call).await.unwrap()), Mark::ok(), "{tool}");
        window.closed(request.id).await;
    }
    let rows = f.rows(4);
    let approvals: Vec<_> = rows
        .iter()
        .map(|r| (r.tool.as_str(), r.approval.as_deref()))
        .collect();
    assert_eq!(
        approvals,
        [
            ("fake_write", Some("auto")),
            ("fake_dangerous", Some("once")),
            ("fake_dangerous", Some("once")),
            ("fake_define", Some("once")),
        ]
    );
    assert!(rows.iter().all(|r| r.plan_hash.is_some()));
}

/// 커밋되는 공유 `project.toml` 의 `auto_approve` 는 효과가 없고 `.local.toml` 의 것은 먹는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_shared_project_auto_approve_has_no_effect() {
    let f = Fixture::build("", |work| {
        let shared = work.join("alpha/.pitwall");
        std::fs::create_dir_all(&shared).unwrap();
        std::fs::write(
            shared.join("project.toml"),
            "schema = 1\n[mcp]\nauto_approve = [\"write\"]\n",
        )
        .unwrap();
        let local = work.join("beta/.pitwall");
        std::fs::create_dir_all(&local).unwrap();
        std::fs::write(
            local.join("project.local.toml"),
            "schema = 1\n[mcp]\nauto_approve = [\"write\"]\n",
        )
        .unwrap();
    });
    let mut alpha = f.window(&f.alpha);
    let beta = f.window(&f.beta);
    let agent = f.agent("agent", &f.work("alpha")).await;
    let call = agent.spawn("fake_write", json!({"tag": "shared"}));
    let request = alpha.opened().await;
    alpha.answer(request.id, McpApprovalAnswer::Deny).unwrap();
    assert_eq!(
        mark(&within(call).await.unwrap()),
        Mark::denied(Reason::ApprovalDenied)
    );
    // 대상 프로젝트(`beta`)의 `.local.toml` 이 정한다 — 세션 프로젝트의 것이 아니다.
    assert_eq!(
        mark(
            &agent
                .call("fake_write", json!({"projectId": f.beta, "tag": "local"}))
                .await
        ),
        Mark::ok()
    );
    drop(beta);
    assert_eq!(f.runs(), 1);
    let rows = f.rows(2);
    assert_eq!(rows[1].approval.as_deref(), Some("auto"));
}

/// 공유 `project.toml` 의 `[mcp.policy] approval_timeout_secs` 는 효과가 없고 경고가 남는다. 전역 · `.local.toml` 의 값은 먹는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_shared_policy_table_has_no_effect_and_warns() {
    let f = Fixture::build("", |work| {
        let shared = work.join("alpha/.pitwall");
        std::fs::create_dir_all(&shared).unwrap();
        std::fs::write(
            shared.join("project.toml"),
            "schema = 1\n[mcp.policy]\napproval_timeout_secs = 1\n",
        )
        .unwrap();
        let local = work.join("beta/.pitwall");
        std::fs::create_dir_all(&local).unwrap();
        std::fs::write(
            local.join("project.local.toml"),
            "schema = 1\n[mcp.policy]\napproval_timeout_secs = 7\n",
        )
        .unwrap();
    });
    let registry = f.host.registry().unwrap();
    let global = &f.host.config().unwrap().global;
    let (policy, warnings) =
        pw_mcp::policy::read_project(&f.paths, global, &registry.get(&f.alpha).unwrap());
    assert_eq!(policy.approval_timeout, Duration::from_secs(300));
    assert!(
        warnings
            .iter()
            .any(|w| w.contains("mcp.policy in the shared .pitwall/project.toml has no effect")),
        "{warnings:?}"
    );
    let (policy, warnings) =
        pw_mcp::policy::read_project(&f.paths, global, &registry.get(&f.beta).unwrap());
    assert_eq!(policy.approval_timeout, Duration::from_secs(7));
    assert!(warnings.is_empty(), "{warnings:?}");

    // 세션도 같은 판정이다: 창에 실린 시한.
    let mut alpha = f.window(&f.alpha);
    let mut beta = f.window(&f.beta);
    let agent = f.agent("agent", &f.work("alpha")).await;
    let call = agent.spawn("fake_write", json!({"tag": "a"}));
    let request = alpha.opened().await;
    assert_eq!(request.timeout_secs, 300);
    alpha.answer(request.id, McpApprovalAnswer::Deny).unwrap();
    within(call).await.unwrap();
    let call = agent.spawn("fake_write", json!({"projectId": f.beta, "tag": "b"}));
    let request = beta.opened().await;
    assert_eq!(request.timeout_secs, 7);
    beta.answer(request.id, McpApprovalAnswer::Deny).unwrap();
    within(call).await.unwrap();
}

/// 세션당 대기 상한(16-m5-plan M5-14): 한 세션이 상한만큼 기다리게 하면 다음 호출은 창에 보이지 않고 곧바로
/// `approval_backlog` 로 거절되고 감사 행이 남는다. 앞의 요청은 그대로 기다리고, 하나가 끝나면 다시 묻는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_session_over_the_pending_limit_is_refused_without_asking() {
    let f = Fixture::new();
    let mut window = f.window(&f.alpha);
    let agent = f.agent("agent", &f.work("alpha")).await;
    let limit = pw_mcp::approval::MAX_PENDING_PER_SESSION;
    let mut calls = Vec::new();
    let mut shown = Vec::new();
    for i in 0..limit {
        calls.push(agent.spawn("fake_write", json!({"tag": format!("t{i}")})));
        shown.push(window.opened().await);
    }
    let started = std::time::Instant::now();
    let refused = agent
        .call("fake_write", json!({"tag": "one too many"}))
        .await;
    assert!(started.elapsed() < Duration::from_secs(5), "기다렸다");
    assert_eq!(mark(&refused), Mark::denied(Reason::ApprovalBacklog));
    assert!(
        text(&refused).contains("already has 8 approval requests waiting"),
        "{}",
        text(&refused)
    );
    assert!(window.quiet(), "넘친 요청은 창에 보이지 않는다");
    assert!(
        calls.iter().all(|c| !c.is_finished()),
        "앞의 요청은 그대로 기다린다"
    );
    // 하나가 끝나면 다시 묻는다.
    window.answer(shown[0].id, McpApprovalAnswer::Deny).unwrap();
    within(calls.remove(0)).await.unwrap();
    window.closed(shown[0].id).await;
    let again = agent.spawn("fake_write", json!({"tag": "again"}));
    let request = window.opened().await;
    for r in shown.iter().skip(1).chain([&request]) {
        window.answer(r.id, McpApprovalAnswer::Deny).unwrap();
    }
    for c in calls.into_iter().chain([again]) {
        within(c).await.unwrap();
    }
    assert_eq!(f.runs(), 0);
    let rows = f.rows(limit + 2);
    assert!(
        rows.iter().any(|r| r.outcome == "denied"
            && r.detail
                .as_deref()
                .is_some_and(|d| d.contains("already has 8"))),
        "넘친 호출의 감사 행"
    );
}

/// 대상 프로젝트 창이 없으면 기다리지 않고 거절한다(`needs_approval`). 다른 프로젝트 창 · 웰컴 창은 대신 답하지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn without_the_target_project_window_the_call_is_refused_at_once() {
    let f = Fixture::new();
    let mut beta = f.window(&f.beta);
    let mut welcome = f.welcome();
    let agent = f.agent("agent", &f.work("alpha")).await;
    let started = std::time::Instant::now();
    let result = agent.call("fake_write", json!({"tag": "x"})).await;
    assert!(started.elapsed() < Duration::from_secs(5), "기다렸다");
    assert_eq!(mark(&result), Mark::denied(Reason::NeedsApproval));
    assert!(
        text(&result).contains(&format!(
            "no Pitwall window of project `{}` is open",
            f.alpha
        )),
        "{}",
        text(&result)
    );
    assert!(beta.quiet() && welcome.quiet());
    assert_eq!(f.runs(), 0);
    assert_eq!(f.rows(1)[0].outcome, "denied");
}

/// 프로젝트 없는 쓰기는 가장 최근에 앞에 있던 창에 「이 프로젝트의 것이 아니다」 로 묻고 그 창만 답한다. 창이 없으면 거절.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_projectless_write_asks_the_front_window() {
    let f = Fixture::new();
    let agent = f.agent("agent", &f.base).await;
    assert_eq!(
        mark(&agent.call("fake_register", json!({"tag": "none"})).await),
        Mark::denied(Reason::NeedsApproval),
        "창이 하나도 없다"
    );
    let mut alpha = f.window(&f.alpha);
    let mut welcome = f.welcome();
    f.sessions.approvals().focused("welcome");
    f.sessions.approvals().focused(&format!("w-{}", f.alpha));
    let call = agent.spawn("fake_register", json!({"tag": "x"}));
    let request = alpha.opened().await;
    assert!(request.not_this_project);
    assert_eq!(request.project_id, None);
    assert!(welcome.quiet(), "다른 창에도 떴다");
    assert_eq!(
        welcome.answer(request.id, McpApprovalAnswer::Once),
        Err(AnswerError::NotYours)
    );
    alpha.answer(request.id, McpApprovalAnswer::Once).unwrap();
    assert_eq!(mark(&within(call).await.unwrap()), Mark::ok());
    assert_eq!(f.runs(), 1);
}

/// 호출 취소가 대기를 거둔다: 창의 다이얼로그에 닫힘이 가고, 감사 행은 `error` · `cancelled`, 툴은 돌지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelling_the_call_withdraws_the_request() {
    let f = Fixture::new();
    let mut window = f.window(&f.alpha);
    let agent = f.agent("agent", &f.work("alpha")).await;
    let handle = agent
        .running
        .peer()
        .send_cancellable_request(
            ClientRequest::CallToolRequest(CallToolRequest::new(
                CallToolRequestParams::new("fake_write")
                    .with_arguments(json!({"tag": "x"}).as_object().unwrap().clone()),
            )),
            PeerRequestOptions::no_options(),
        )
        .await
        .unwrap();
    let request = window.opened().await;
    handle.cancel(Some("test".into())).await.unwrap();
    window.closed(request.id).await;
    assert_eq!(
        window.answer(request.id, McpApprovalAnswer::Once),
        Err(AnswerError::NotFound)
    );
    let rows = f.rows(1);
    assert_eq!(
        (rows[0].outcome.as_str(), rows[0].detail.as_deref()),
        ("error", Some("cancelled"))
    );
    assert_eq!(f.runs(), 0);
}

/// 승인을 지난 `fake_irreversible` 을 띄우고 지점 앞까지 데려간다. 취소할 수 있는 요청의 손잡이를 돌려준다.
async fn irreversible_at_the_point(
    f: &Fixture,
    window: &mut Window,
    agent: &Agent,
) -> rmcp::service::RequestHandle<RoleClient> {
    let handle = agent
        .running
        .peer()
        .send_cancellable_request(
            ClientRequest::CallToolRequest(CallToolRequest::new(
                CallToolRequestParams::new("fake_irreversible")
                    .with_arguments(json!({"tag": "x"}).as_object().unwrap().clone()),
            )),
            PeerRequestOptions::no_options(),
        )
        .await
        .unwrap();
    let request = window.opened().await;
    window.answer(request.id, McpApprovalAnswer::Once).unwrap();
    within(f.probe.gate.reached.notified()).await;
    handle
}

/// 「되돌릴 수 없는 지점」 을 넘은 뒤 클라이언트가 취소하면(16-m5-plan M5-18b) Host 는 핸들러를 버리지 않고 끝까지 돌려 답하고,
/// 세션은 그 답을 기다려 선기록한 행을 결과 `ok` 로 끝낸다 — 상세는 툴의 상세 뒤에 「취소됐지만 끝까지 돌았다」.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cancel_after_the_point_of_no_return_finishes_the_row_with_the_result() {
    let f = Fixture::new();
    let mut window = f.window(&f.alpha);
    let agent = f.agent("agent", &f.work("alpha")).await;
    let handle = irreversible_at_the_point(&f, &mut window, &agent).await;
    f.probe.gate.go.notify_one();
    within(f.probe.gate.decided.notified()).await;
    assert_eq!(*f.probe.gate.passed.lock().unwrap(), Some(true));
    handle.cancel(Some("test".into())).await.unwrap();
    // 취소가 Host 의 핸들러에 닿은 뒤에 끝낸다 — 취소 뒤의 답이다.
    within(f.probe.gate.saw_cancel.notified()).await;
    f.probe.gate.finish.notify_one();
    let rows = f.rows(1);
    let detail = rows[0].detail.clone().unwrap_or_default();
    assert_eq!(rows[0].outcome, "ok", "{rows:?}");
    assert!(detail.starts_with("irreversible "), "{detail}");
    assert!(
        detail.ends_with(&format!(" · {}", pw_mcp::server::CANCELLED_AFTER_THE_POINT)),
        "{detail}"
    );
    assert_eq!(rows[0].approval.as_deref(), Some("once"));
    assert_eq!(f.runs(), 1);
}

/// 지점 뒤에 취소했고 Host 가 **오류**로 답했다(커밋 실패의 자리 — 리뷰 R5): 행은 결과 `error` 이고 상세 끝에 같은 꼬리 글이
/// 붙는다 — 취소가 아니라 끝까지 돈 호출의 오류다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_error_after_the_point_of_no_return_keeps_the_tail_note() {
    let f = Fixture::new();
    f.probe.gate.fail.store(true, Ordering::SeqCst);
    let mut window = f.window(&f.alpha);
    let agent = f.agent("agent", &f.work("alpha")).await;
    let handle = irreversible_at_the_point(&f, &mut window, &agent).await;
    f.probe.gate.go.notify_one();
    within(f.probe.gate.decided.notified()).await;
    handle.cancel(Some("test".into())).await.unwrap();
    within(f.probe.gate.saw_cancel.notified()).await;
    f.probe.gate.finish.notify_one();
    let rows = f.rows(1);
    let detail = rows[0].detail.clone().unwrap_or_default();
    assert_eq!(rows[0].outcome, "error", "{rows:?}");
    assert!(detail.starts_with("the commit failed (3003)"), "{detail}");
    assert!(
        detail.ends_with(&format!(" · {}", pw_mcp::server::CANCELLED_AFTER_THE_POINT)),
        "{detail}"
    );
}

/// 지점 전의 취소는 지금처럼 버려진다 — Host 가 `cancelled` 로 답하고 행은 결과 `error` · 상세 `cancelled`, 태스크는 지점을 세우지
/// 못한다(효과를 내지 않는다).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cancel_before_the_point_of_no_return_is_a_cancel() {
    let f = Fixture::new();
    let mut window = f.window(&f.alpha);
    let agent = f.agent("agent", &f.work("alpha")).await;
    let handle = irreversible_at_the_point(&f, &mut window, &agent).await;
    handle.cancel(Some("test".into())).await.unwrap();
    let rows = f.rows(1);
    assert_eq!(
        (rows[0].outcome.as_str(), rows[0].detail.as_deref()),
        ("error", Some("cancelled"))
    );
    f.probe.gate.go.notify_one();
    within(f.probe.gate.decided.notified()).await;
    assert_eq!(*f.probe.gate.passed.lock().unwrap(), Some(false));
    assert_eq!(f.runs(), 0);
}

/// 지점 뒤에 취소했는데 Host 가 [`pw_mcp::server::SETTLE_AFTER_CANCEL`](5 s) 안에 답하지 않으면 지금처럼 취소로 끝내되, 상세가
/// 「효과가 났는지 모른다」 다 — 「취소됨」(위)과 구분된다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cancel_whose_answer_does_not_come_in_time_is_logged_as_unknown() {
    let f = Fixture::new();
    let mut window = f.window(&f.alpha);
    let agent = f.agent("agent", &f.work("alpha")).await;
    let handle = irreversible_at_the_point(&f, &mut window, &agent).await;
    f.probe.gate.go.notify_one();
    within(f.probe.gate.decided.notified()).await;
    let cancelled = std::time::Instant::now();
    handle.cancel(Some("test".into())).await.unwrap();
    within(f.probe.gate.saw_cancel.notified()).await;
    let rows = f.rows(1);
    assert!(
        cancelled.elapsed() >= pw_mcp::server::SETTLE_AFTER_CANCEL,
        "{:?}",
        cancelled.elapsed()
    );
    assert_eq!(rows[0].outcome, "error", "{rows:?}");
    assert_eq!(
        rows[0].detail.as_deref(),
        Some(format!("cancelled · {}", pw_mcp::server::CANCELLED_UNSETTLED).as_str())
    );
    f.probe.gate.finish.notify_one();
}

/// 끊긴 세션의 요청에 사람이 [허용] 을 눌러도 돌지 않는다. rmcp 는 끊긴 뒤 5초까지 핸들러를 돌리지만, 세션은 읽기가 끝나는 것을
/// 보고 곧바로 요청을 거둔다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_disconnected_sessions_request_is_withdrawn_before_anyone_can_allow_it() {
    let f = Fixture::new();
    let mut window = f.window(&f.alpha);
    let agent = f.agent("agent", &f.work("alpha")).await;
    let _call = agent.spawn("fake_write", json!({"tag": "x"}));
    let request = window.opened().await;
    let disconnected = std::time::Instant::now();
    agent.running.cancel().await.unwrap();
    window.closed(request.id).await;
    assert!(
        disconnected.elapsed() < Duration::from_secs(3),
        "5초 drain 을 기다렸다: {:?}",
        disconnected.elapsed()
    );
    assert_eq!(
        window.answer(request.id, McpApprovalAnswer::Once),
        Err(AnswerError::NotFound)
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(f.runs(), 0);
    assert_eq!(f.rows(1)[0].detail.as_deref(), Some("cancelled"));
}

/// 승인을 청하는 조회 툴: 승인이 필요 없는 호출은 첫 호출의 결과가 곧 결과(선기록 없음 · 승인 칸 없음), 청한 호출은 승인을 지나
/// 선기록되고 승인 방식 · 해시가 남는다. `auto_approve` 를 적용하지 않는 계획이면 `auto_approve = ["read", "write"]` 여도 묻고,
/// 세션 허용은 계획의 열쇠(그 연결)에 묶인다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_read_tool_can_ask_per_call() {
    let f = Fixture::with_global("[mcp]\nauto_approve = [\"read\", \"write\"]\n");
    let mut window = f.window(&f.alpha);
    let agent = f.agent("agent", &f.work("alpha")).await;
    assert_eq!(
        mark(&agent.call("fake_open", json!({"tag": "plain"})).await),
        Mark::ok()
    );
    assert!(window.quiet());
    let call = agent.spawn("fake_open", json!({"tag": "ask-1"}));
    let request = window.opened().await;
    assert_eq!(request.tier, Tier::Read);
    window
        .answer(request.id, McpApprovalAnswer::Session)
        .unwrap();
    assert_eq!(mark(&within(call).await.unwrap()), Mark::ok());
    window.closed(request.id).await;
    // 같은 열쇠: 묻지 않는다. 다른 열쇠(다른 연결): 묻는다.
    assert_eq!(
        mark(&agent.call("fake_open", json!({"tag": "ask-1"})).await),
        Mark::ok()
    );
    let call = agent.spawn("fake_open", json!({"tag": "ask-2"}));
    let other = window.opened().await;
    window.answer(other.id, McpApprovalAnswer::Deny).unwrap();
    within(call).await.unwrap();
    let rows = f.rows(4);
    let summary: Vec<_> = rows
        .iter()
        .map(|r| {
            (
                r.outcome.as_str(),
                r.approval.as_deref(),
                r.plan_hash.is_some(),
            )
        })
        .collect();
    assert_eq!(
        summary,
        [
            ("ok", None, false),
            ("ok", Some("session"), true),
            ("ok", Some("session"), true),
            ("denied", None, false),
        ]
    );
}

/// 헤드리스: 승인 경로가 없어 `write` 이상은 거절한다(`headless_write`) — 툴을 부르지도 않는다(계획도 없다).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_headless_session_refuses_writes_without_calling_the_tool() {
    let f = Fixture::with_global("[mcp]\nauto_approve = [\"write\"]\n");
    let _window = f.window(&f.alpha);
    let agent = f
        .agent_on("agent", &f.work("alpha"), HostKind::Headless)
        .await;
    let result = agent.call("fake_write", json!({"tag": "x"})).await;
    assert_eq!(mark(&result), Mark::denied(Reason::HeadlessWrite));
    assert_eq!((f.plans(), f.runs()), (0, 0));
    // 승인을 청하는 조회 툴도 헤드리스에서는 승인을 받을 길이 없다.
    let result = agent.call("fake_open", json!({"tag": "ask"})).await;
    assert_eq!(mark(&result), Mark::denied(Reason::NeedsApproval));
    assert_eq!(f.runs(), 0);
}

/// 앱이 끝날 때(`Sessions::shutdown`): 대기를 거두고, 그 호출의 감사 행이 큐에서 쓰인 뒤 돌아온다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutting_down_withdraws_requests_and_flushes_the_audit_queue() {
    let f = Fixture::new();
    let mut window = f.window(&f.alpha);
    let agent = f.agent("agent", &f.work("alpha")).await;
    // 끝난 read 호출의 기록(큐)도 함께 비운다.
    agent.call("project_list", json!({})).await;
    let call = agent.spawn("fake_write", json!({"tag": "x"}));
    let request = window.opened().await;
    f.rows(1);
    // 다른 프로세스가 감사 파일의 쓰기 잠금을 잠시 쥔다 — 쓰기 스레드가 그동안 기다리므로, 종료가 큐를 비우기를 기다리지 않으면
    // 돌아온 때 거절 행이 아직 없다.
    let audit_file = f.paths.state.join("mcp/audit.db");
    let locker = rusqlite::Connection::open(&audit_file).unwrap();
    locker.execute_batch("BEGIN IMMEDIATE").unwrap();
    let release = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(500));
        locker.execute_batch("COMMIT").unwrap();
    });
    let sessions = f.sessions.clone();
    let done = tokio::task::spawn_blocking(move || sessions.shutdown(Duration::from_secs(2)))
        .await
        .unwrap();
    release.join().unwrap();
    assert!(done, "시한 안에 끝나지 않았다");
    // 돌아온 때 행이 이미 있다(기다리지 않고 읽는다).
    let conn = rusqlite::Connection::open(f.paths.state.join("mcp/audit.db")).unwrap();
    let rows: Vec<(String, String, Option<String>)> = conn
        .prepare("SELECT tool, outcome, detail FROM calls ORDER BY id")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(
        rows,
        [
            ("project_list".to_owned(), "ok".to_owned(), None),
            (
                "fake_write".to_owned(),
                "error".to_owned(),
                Some("cancelled".to_owned())
            ),
        ]
    );
    window.closed(request.id).await;
    assert_eq!(
        mark(&within(call).await.unwrap()),
        Mark::error(Some(Reason::Cancelled))
    );
    assert_eq!(f.runs(), 0);
}

// ---- RPC 길: 창이 하는 그대로 Host 연결로 `mcp.approvals` 를 열고 `mcp.approval.answer` 로 답한다 ----

struct Rpc(pw_core::Connection, i64);

impl Rpc {
    async fn connect(f: &Fixture, scope: ConnectionScope) -> Self {
        let caller = Caller {
            role: Role::Owner,
            scope,
            window: Some("rpc-window".into()),
        };
        Self(pw_core::Host::connect(&*f.host, caller).await.unwrap(), 0)
    }

    fn send(&mut self, method: &str, params: Value) -> i64 {
        self.1 += 1;
        self.0
            .sink
            .send(json!({"jsonrpc": "2.0", "id": self.1, "method": method, "params": params}));
        self.1
    }

    async fn next(&mut self) -> Value {
        within(self.0.source.recv())
            .await
            .expect("connection closed")
    }

    async fn reply(&mut self, id: i64) -> Value {
        loop {
            let frame = self.next().await;
            if frame["id"] == json!(id) {
                return frame;
            }
        }
    }

    async fn call(&mut self, method: &str, params: Value) -> Value {
        let id = self.send(method, params);
        self.reply(id).await
    }

    /// 스트림을 연다. 열리면 `Ok(streamId)`, 거절되면(열기 응답 · 곧바로 끝) `Err(error)`.
    async fn open(&mut self, params: Value) -> Result<Value, Value> {
        let opened = self.call("mcp.approvals", params).await;
        if !opened["error"].is_null() {
            return Err(opened["error"].clone());
        }
        let stream = opened["result"]["streamId"].clone();
        self.0.sink.send(json!({
            "jsonrpc": "2.0", "method": "stream.credit",
            "params": {"streamId": stream, "credit": 16}
        }));
        Ok(stream)
    }

    async fn chunk(&mut self) -> Value {
        loop {
            let frame = self.next().await;
            if frame["method"] == "stream.chunk" {
                return frame["params"]["data"].clone();
            }
            if frame["method"] == "stream.end" {
                return json!({"ended": frame["params"]["error"]});
            }
        }
    }
}

/// 에이전트 범위 연결(MCP 세션이 Host 에 붙는 연결)은 `mcp.approvals` 를 열 수 없다 — `mcp.*` 문의 스트림 갈래(첫 시험).
/// 자기 요청에 `mcp.approval.answer` 로 스스로 답할 수도 없다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_agent_connection_can_neither_hear_nor_answer_approvals() {
    let f = Fixture::new();
    let mut window = f.window(&f.alpha);
    let mut agent_rpc = Rpc::connect(&f, ConnectionScope::Agent).await;
    let refused = match agent_rpc.open(json!({"projectId": f.alpha})).await {
        Err(error) => error,
        Ok(_) => agent_rpc.chunk().await["ended"].clone(),
    };
    assert_eq!(
        refused["code"],
        json!(pw_rpc::code::PERMISSION_DENIED),
        "{refused}"
    );
    assert_eq!(refused["data"]["reason"], "agentSession", "{refused}");

    let agent = f.agent("agent", &f.work("alpha")).await;
    let call = agent.spawn("fake_write", json!({"tag": "x"}));
    let request = window.opened().await;
    let answered = agent_rpc
        .call(
            "mcp.approval.answer",
            json!({"id": request.id, "answer": "once"}),
        )
        .await;
    assert_eq!(
        answered["error"]["code"],
        json!(pw_rpc::code::PERMISSION_DENIED)
    );
    assert_eq!(answered["error"]["data"]["reason"], "agentSession");
    // 에이전트에게 열린 툴에도 답하는 길이 없다.
    let tools = agent.running.peer().list_all_tools().await.unwrap();
    assert!(
        tools.iter().all(|t| !t.name.contains("approval")),
        "{:?}",
        tools.iter().map(|t| t.name.to_string()).collect::<Vec<_>>()
    );
    assert_eq!(f.runs(), 0);
    window.answer(request.id, McpApprovalAnswer::Deny).unwrap();
    within(call).await.unwrap();
}

/// 프로젝트 창의 연결이 `mcp.approvals` 로 요청을 받고 `mcp.approval.answer` 로 답한다. 다른 프로젝트 창은 그 프로젝트를 듣거나
/// 그 요청에 답하지 못한다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_project_window_hears_and_answers_over_rpc() {
    let f = Fixture::new();
    let mut other = Rpc::connect(&f, ConnectionScope::Project(f.beta.clone())).await;
    let refused = match other.open(json!({"projectId": f.alpha})).await {
        Err(error) => error,
        Ok(_) => other.chunk().await["ended"].clone(),
    };
    assert_eq!(refused["data"]["reason"], "notProjectWindow", "{refused}");

    let mut rpc = Rpc::connect(&f, ConnectionScope::Project(f.alpha.clone())).await;
    rpc.open(json!({"projectId": f.alpha})).await.unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while f.sessions.approvals().listeners() == 0 {
        assert!(
            std::time::Instant::now() < deadline,
            "the stream did not listen"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let agent = f.agent("agent", &f.work("alpha")).await;
    let call = agent.spawn("fake_write", json!({"tag": "x"}));
    let event = rpc.chunk().await;
    assert_eq!(event["kind"], "open", "{event}");
    let id = event["request"]["id"].clone();
    assert_eq!(event["request"]["tool"], "fake_write");
    assert_eq!(event["request"]["plan"]["tier"], "write");

    let not_mine = other
        .call("mcp.approval.answer", json!({"id": id, "answer": "once"}))
        .await;
    assert_eq!(not_mine["error"]["data"]["reason"], "notProjectWindow");
    let not_offered = rpc
        .call("mcp.approval.answer", json!({"id": id, "answer": "window"}))
        .await;
    assert_eq!(
        not_offered["error"]["code"],
        json!(pw_rpc::code::INVALID_PARAMS)
    );
    let ok = rpc
        .call("mcp.approval.answer", json!({"id": id, "answer": "once"}))
        .await;
    assert!(ok["error"].is_null(), "{ok}");
    assert_eq!(mark(&within(call).await.unwrap()), Mark::ok());
    let closed = rpc.chunk().await;
    assert_eq!(closed, json!({"kind": "closed", "id": id}));
    let again = rpc
        .call("mcp.approval.answer", json!({"id": id, "answer": "once"}))
        .await;
    assert_eq!(again["error"]["code"], json!(pw_rpc::code::NOT_FOUND));
    assert_eq!(f.runs(), 1);
}

/// 「승인 필요」 오류는 에이전트에게 그대로 나가지 않는다 — `rmcp` 결과가 정해진 거절 까닭이고 글에 계획이 없다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_approval_required_error_never_reaches_the_agent() {
    let f = Fixture::new();
    let agent = f.agent("agent", &f.work("alpha")).await;
    let result = agent.call("fake_write", json!({"tag": "x"})).await;
    assert_eq!(mark(&result), Mark::denied(Reason::NeedsApproval));
    let body = serde_json::to_string(&result).unwrap();
    assert!(!body.contains("sha256:"), "{body}");
    assert!(!body.contains("6001"), "{body}");
    let read = agent.call("fake_open", json!({"tag": "ask"})).await;
    assert_eq!(mark(&read), Mark::denied(Reason::NeedsApproval));
    let body = serde_json::to_string(&read).unwrap();
    assert!(
        !body.contains("sha256:") && !body.contains("\"plan\""),
        "{body}"
    );
}

/// 허용은 툴을 넘지 않는다: 같은 열쇠를 내는 다른 툴, 다른 툴의 이름과 같은 열쇠를 내는 툴 모두 다시 묻는다(세션 허용 · CLI 허용).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_allowance_never_reaches_another_tool() {
    let f = Fixture::new();
    let mut window = f.window(&f.alpha);
    for client in ["agent", pw_mcp::server::CLI_CLIENT] {
        let third = if client == "agent" {
            McpApprovalAnswer::Session
        } else {
            McpApprovalAnswer::Window
        };
        let agent = f.agent(client, &f.work("alpha")).await;
        // `fake_open` 의 열쇠 `k` 를 허용한다.
        let call = agent.spawn("fake_open", json!({"tag": "ask", "key": "k"}));
        let request = window.opened().await;
        window.answer(request.id, third).unwrap();
        assert_eq!(mark(&within(call).await.unwrap()), Mark::ok());
        window.closed(request.id).await;
        // CLI 는 호출마다 새 세션이다 — 다음 호출은 다른 세션에서.
        let next = f.agent(client, &f.work("alpha")).await;
        let same = if client == "agent" { &agent } else { &next };
        assert_eq!(
            mark(
                &same
                    .call("fake_open", json!({"tag": "ask", "key": "k"}))
                    .await
            ),
            Mark::ok(),
            "{client}: 같은 툴 · 같은 열쇠는 묻지 않는다"
        );
        assert!(window.quiet());
        // 같은 열쇠를 내는 다른 툴: 묻는다.
        let call = same.spawn("fake_open_other", json!({"tag": "ask", "key": "k"}));
        let asked = window.opened().await;
        assert_eq!(asked.tool, "fake_open_other", "{client}");
        window.answer(asked.id, McpApprovalAnswer::Deny).unwrap();
        within(call).await.unwrap();
        window.closed(asked.id).await;
        // `fake_write`(열쇠 없는 툴 단위)를 허용해도, 그 이름을 열쇠로 내는 다른 툴은 묻는다.
        let call = same.spawn("fake_write", json!({"tag": client}));
        let asked = window.opened().await;
        window.answer(asked.id, third).unwrap();
        assert_eq!(mark(&within(call).await.unwrap()), Mark::ok());
        window.closed(asked.id).await;
        let call = same.spawn("fake_open", json!({"tag": "ask", "key": "fake_write"}));
        let asked = window.opened().await;
        assert_eq!(asked.tool, "fake_open", "{client}");
        window.answer(asked.id, McpApprovalAnswer::Deny).unwrap();
        within(call).await.unwrap();
        window.closed(asked.id).await;
    }
}

/// CLI 요청에 [이번만 허용] 으로 답하면 기억하지 않는다 — 다른 CLI 세션의 같은 툴은 다시 묻는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn allowing_a_cli_call_once_is_not_remembered() {
    let f = Fixture::new();
    let mut window = f.window(&f.alpha);
    let first = f.agent(pw_mcp::server::CLI_CLIENT, &f.work("alpha")).await;
    let call = first.spawn("fake_write", json!({"tag": "1"}));
    let request = window.opened().await;
    window.answer(request.id, McpApprovalAnswer::Once).unwrap();
    assert_eq!(mark(&within(call).await.unwrap()), Mark::ok());
    window.closed(request.id).await;
    let second = f.agent(pw_mcp::server::CLI_CLIENT, &f.work("alpha")).await;
    let call = second.spawn("fake_write", json!({"tag": "2"}));
    let asked = window.opened().await;
    window.answer(asked.id, McpApprovalAnswer::Deny).unwrap();
    assert_eq!(
        mark(&within(call).await.unwrap()),
        Mark::denied(Reason::ApprovalDenied)
    );
}

/// 계획을 청하는 쓰기 툴이 승인 표 없이 돌았다(규약 위반 — 첫 호출의 부작용): 결과를 에이전트에게 주지 않고 「Pitwall bug」
/// 오류로 끝내고 감사 행은 `error` 다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_planned_tool_that_runs_without_a_ticket_is_reported_as_a_bug() {
    let f = Fixture::with_global("[mcp]\nauto_approve = [\"write\"]\n");
    let _window = f.window(&f.alpha);
    let agent = f.agent("agent", &f.work("alpha")).await;
    let result = agent.call("fake_rogue", json!({"tag": "x"})).await;
    assert_eq!(mark(&result), Mark::error(None));
    assert!(text(&result).contains("Pitwall bug"), "{}", text(&result));
    assert!(
        !serde_json::to_string(&result)
            .unwrap()
            .contains("secret result"),
        "툴의 결과가 나갔다"
    );
    let rows = f.rows(1);
    assert_eq!(
        (rows[0].tool.as_str(), rows[0].outcome.as_str()),
        ("fake_rogue", "error")
    );
    assert!(rows[0].detail.as_deref().unwrap().contains("Pitwall bug"));
}

/// 세션은 툴의 계획을 검증한다: 다른 메서드의 계획이나 선언보다 높은 등급의 계획은 묻지도 돌리지도 않고 「Pitwall bug」 오류다
/// (`auto_approve` 에 선언 등급이 있어도 — 17-m6-plan M6-10 ④, 선언은 상한). 계획 등급이 선언보다 낮으면 계획 등급으로 판정한다
/// (결정 M6-P9) — `dangerous` 로 선언한 툴의 `write` 계획은 `write` 로 묻고 셋째 답이 있다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_session_checks_the_plan_against_the_tool() {
    let f = Fixture::with_global("[mcp]\nauto_approve = [\"write\", \"exec\"]\n");
    let mut window = f.window(&f.alpha);
    let agent = f.agent("agent", &f.work("alpha")).await;
    for (tool, tag) in [
        ("fake_over_tier", "exec"),
        ("fake_over_tier", "dangerous"),
        ("fake_wrong_method", "x"),
    ] {
        let result = agent.call(tool, json!({"tag": tag})).await;
        assert_eq!(mark(&result), Mark::error(None), "{tool} {tag}");
        assert!(text(&result).contains("Pitwall bug"), "{}", text(&result));
    }
    assert!(window.quiet(), "규약을 어긴 계획으로 물었다");
    assert_eq!(f.runs(), 0);

    // `auto_approve` 의 `write` 가 계획 등급 `write` 에 닿는다 — 묻지 않는다.
    assert_eq!(
        mark(&agent.call("fake_dangerous_low", json!({"tag": "x"})).await),
        Mark::ok()
    );
    assert!(window.quiet(), "계획 등급이 아니라 선언 등급으로 판정했다");
    assert_eq!(f.runs(), 1);

    let f = Fixture::new();
    let mut window = f.window(&f.alpha);
    let agent = f.agent("agent", &f.work("alpha")).await;
    let call = agent.spawn("fake_dangerous_low", json!({"tag": "x"}));
    let request = window.opened().await;
    assert_eq!(request.tier, Tier::Write, "승인 요청의 등급은 계획 등급");
    assert_eq!(request.plan.tier, Tier::Write);
    assert_eq!(
        request.answers,
        [
            McpApprovalAnswer::Deny,
            McpApprovalAnswer::Once,
            McpApprovalAnswer::Session
        ]
    );
    window.answer(request.id, McpApprovalAnswer::Once).unwrap();
    assert_eq!(mark(&within(call).await.unwrap()), Mark::ok());
    assert_eq!(f.runs(), 1);
}

// ---- 계획 등급 · 정책 자동 · 에이전트의 실행 허가(17-m6-plan M6-10) ----

fn run_rows(f: &Fixture, n: usize) -> Vec<(String, String, Option<String>)> {
    f.rows(n)
        .into_iter()
        .map(|r| (r.tool, r.outcome, r.approval))
        .collect()
}

/// ③ `dangerous` 로 선언한 툴의 `exec` 계획은 `auto_approve = ["exec"]` 로 묻지 않는다(높은 쪽 — 선언으로 판정하면 묻는다).
/// 같은 툴의 `dangerous` 계획은 그래도 매번 묻고 셋째 답이 없다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_dangerous_tool_with_an_exec_plan_is_judged_by_the_plan_tier() {
    let f = Fixture::with_global("[mcp]\nauto_approve = [\"exec\"]\n");
    let mut window = f.window(&f.alpha);
    let agent = f.agent("agent", &f.work("alpha")).await;
    assert_eq!(
        mark(&agent.call("fake_run", json!({"tag": "exec"})).await),
        Mark::ok()
    );
    assert!(window.quiet(), "선언 등급(dangerous)으로 판정해 물었다");
    assert_eq!(f.launched().len(), 1);

    let call = agent.spawn("fake_run", json!({"tag": "dangerous"}));
    let request = window.opened().await;
    assert_eq!(request.tier, Tier::Dangerous);
    assert_eq!(
        request.answers,
        [McpApprovalAnswer::Deny, McpApprovalAnswer::Once]
    );
    window.answer(request.id, McpApprovalAnswer::Once).unwrap();
    assert_eq!(mark(&within(call).await.unwrap()), Mark::ok());
    window.closed(request.id).await;
    assert_eq!(
        run_rows(&f, 2),
        [
            ("fake_run".into(), "ok".into(), Some("auto".into())),
            ("fake_run".into(), "ok".into(), Some("once".into())),
        ]
    );
}

/// ⑤ 정책 자동 계획은 묻지 않고 돈다 — 창이 없어도. 그래도 두 단계(계획 둘)이고, 선기록한 행의 승인 방식은 `policy`, 계획 해시가
/// 남는다. `auto_approve` 가 없어도 된다(툴이 판정한 자동).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_policy_auto_plan_runs_without_asking_and_is_logged_as_policy() {
    let f = Fixture::new();
    let agent = f.agent("agent", &f.work("alpha")).await;
    let result = agent.call("fake_run", json!({"tag": "exec+policy"})).await;
    assert_eq!(mark(&result), Mark::ok(), "{}", text(&result));
    assert_eq!((f.plans(), f.runs()), (2, 1), "승인 표를 실어 다시 불렀다");
    let rows = f.rows(1);
    assert_eq!(
        (rows[0].outcome.as_str(), rows[0].approval.as_deref()),
        ("ok", Some("policy"))
    );
    assert!(rows[0].plan_hash.as_deref().unwrap().starts_with("sha256:"));

    // 정책 자동이 없는 같은 계획은 묻는다 — 묻지 않은 것은 표지 때문이다.
    let mut window = f.window(&f.alpha);
    let call = agent.spawn("fake_run", json!({"tag": "exec"}));
    let request = window.opened().await;
    assert_eq!(request.tier, Tier::Exec);
    window.answer(request.id, McpApprovalAnswer::Deny).unwrap();
    assert_eq!(
        mark(&within(call).await.unwrap()),
        Mark::denied(Reason::ApprovalDenied)
    );
    assert_eq!(f.runs(), 1);
}

/// ⑥ `dangerous` 계획의 정책 자동은 툴의 버그다 — 묻지도 돌리지도 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_dangerous_plan_cannot_carry_policy_auto() {
    let f = Fixture::with_global("[mcp]\nauto_approve = [\"write\", \"exec\"]\n");
    let mut window = f.window(&f.alpha);
    let agent = f.agent("agent", &f.work("alpha")).await;
    let result = agent
        .call("fake_run", json!({"tag": "dangerous+policy"}))
        .await;
    assert_eq!(mark(&result), Mark::error(None));
    assert!(text(&result).contains("Pitwall bug"), "{}", text(&result));
    assert!(window.quiet(), "규약을 어긴 계획으로 물었다");
    assert_eq!((f.plans(), f.runs()), (1, 0));
    assert!(f.launched().is_empty());
    let rows = f.rows(1);
    assert_eq!(
        (rows[0].outcome.as_str(), rows[0].approval.as_deref()),
        ("error", None)
    );
}

/// ⑦ 헤드리스는 선언 등급으로 거절한다 — 툴을 부르기 전(계획을 만들기 전)이고, 계획이 `read` · 정책 자동이어도 같다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_headless_session_refuses_by_the_declared_tier_before_planning() {
    let f = Fixture::with_global("[mcp]\nauto_approve = [\"write\", \"exec\"]\n");
    let agent = f
        .agent_on("agent", &f.work("alpha"), HostKind::Headless)
        .await;
    for tag in ["exec+policy", "read+policy", "exec"] {
        let result = agent.call("fake_run", json!({"tag": tag})).await;
        assert_eq!(mark(&result), Mark::denied(Reason::HeadlessWrite), "{tag}");
    }
    assert_eq!((f.plans(), f.runs()), (0, 0));
}

/// ⑧ 승인 뒤 띄우기 전에 `.env.local` 이 바뀌어도 계획 때 읽은 값으로 돈다(띄우는 쪽이 다시 읽으면 실패). 승인을 묻는 동안
/// 바뀌면 다시 만든 계획의 해시가 달라 돌지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_launch_uses_the_env_read_when_planning() {
    let f = Fixture::new();
    let env = f.work("alpha").join(".env.local");
    std::fs::write(&env, "API_URL=seen\n").unwrap();
    let changed = env.clone();
    *f.probe.between.lock().unwrap() = Some(Box::new(move || {
        std::fs::write(changed, "API_URL=swapped\n").unwrap();
    }));
    let agent = f.agent("agent", &f.work("alpha")).await;
    let result = agent.call("fake_run", json!({"tag": "exec+policy"})).await;
    assert_eq!(mark(&result), Mark::ok(), "{}", text(&result));
    assert_eq!(std::fs::read_to_string(&env).unwrap(), "API_URL=swapped\n");
    let launched = f.launched();
    assert_eq!(launched.len(), 1);
    assert_eq!(launched[0].env["API_URL"], "seen");
    assert_eq!(launched[0].cwd, f.work("alpha"));

    let mut window = f.window(&f.alpha);
    let call = agent.spawn("fake_run", json!({"tag": "exec"}));
    let request = window.opened().await;
    std::fs::write(&env, "API_URL=behind-the-dialog\n").unwrap();
    window.answer(request.id, McpApprovalAnswer::Once).unwrap();
    assert_eq!(
        mark(&within(call).await.unwrap()),
        Mark::denied(Reason::PlanChanged)
    );
    assert_eq!(f.launched().len(), 1, "바뀐 .env.local 로 띄웠다");
}

/// 사람이 띄운 실행을 겨누는 꼴(결정 D69 — 실제 툴은 M6-14): 계획 `exec` 라도 규칙이 `auto_approve` 를 끄면
/// `auto_approve = ["exec"]` 로 묻지 않고 돌지 않는다. 세션 허용은 그 열쇠(그 실행 하나)에만 닿는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn auto_approve_does_not_reach_a_plan_that_turns_it_off() {
    let f = Fixture::with_global("[mcp]\nauto_approve = [\"exec\"]\n");
    let mut window = f.window(&f.alpha);
    let agent = f.agent("agent", &f.work("alpha")).await;
    let call = agent.spawn("fake_run", json!({"tag": "exec+human", "key": "human:r1"}));
    let request = window.opened().await;
    assert_eq!(request.tier, Tier::Exec);
    assert_eq!(
        request.answers,
        [
            McpApprovalAnswer::Deny,
            McpApprovalAnswer::Once,
            McpApprovalAnswer::Session
        ]
    );
    window
        .answer(request.id, McpApprovalAnswer::Session)
        .unwrap();
    assert_eq!(mark(&within(call).await.unwrap()), Mark::ok());
    window.closed(request.id).await;
    // 같은 실행은 세션 허용으로, 다른 실행은 다시 묻는다.
    assert_eq!(
        mark(
            &agent
                .call("fake_run", json!({"tag": "exec+human", "key": "human:r1"}))
                .await
        ),
        Mark::ok()
    );
    assert!(window.quiet());
    let call = agent.spawn("fake_run", json!({"tag": "exec+human", "key": "human:r2"}));
    let request = window.opened().await;
    window.answer(request.id, McpApprovalAnswer::Deny).unwrap();
    within(call).await.unwrap();
    assert_eq!(
        run_rows(&f, 3).into_iter().map(|r| r.2).collect::<Vec<_>>(),
        [Some("session".into()), Some("session".into()), None]
    );
}
