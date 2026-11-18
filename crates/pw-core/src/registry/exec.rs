//! 프로젝트 안에서 프로세스를 띄워도 되는가(Exec 등급 RPC 의 연결 범위, 03-architecture.md 「연결 범위」).
//!
//! 판정은 [`exec_grant`] 한 곳이고, 프로세스를 띄우는 함수(`pw_run` 의 터미널, `pw_open_with` 의 실행)는 그 결과인 [`ExecGrant`]
//! 를 받아야 한다 — 이 모듈 밖에서는 만들 수 없으므로 판정을 건너뛰고는 컴파일되지 않는다. 범위는 요청 값이 아니라 연결이 정한
//! 것이다(`fs.*` · git 쓰기와 같은 자리, [`super::state::scope_may_write`]).
//!
//! - 그 프로젝트의 창(`ConnectionScope::Project(그 id)`)은 무엇이든 띄운다. 작업 디렉터리가 그 프로젝트(활성 워크트리 포함) 안인지는
//!   띄우는 쪽이 이미 본다.
//! - 다른 프로젝트 창 · 파일 창은 아무것도 띄우지 못한다.
//! - Host 연결(웰컴 · 설정 창)은 하나만 된다: 웰컴 창의 「다음에서 열기」 — 등록된 외부 도구로 등록된 프로젝트 루트 그대로를 여는
//!   것. 워크트리 · 파일 · 줄 · 열을 넘기거나 목록에 없는 도구를 고르면 거절이다. 웹뷰는 도구 id 만 고르고 명령은 설정이 정한다.
//! - MCP 세션(`ConnectionScope::Agent`, 결정 M5-D27)은 [`exec_grant`] 로 아무것도 띄우지 못한다 — Host 연결의 「다음에서 열기」
//!   예외도 받지 않는다. 셸 · 외부 도구([`Launch::Process`] · [`Launch::OpenWith`])는 에이전트에게 늘 거절이다.
//!
//! # 에이전트의 실행 허가(17-m6-plan M6-10 ① — 결정 M6-P10)
//!
//! 실행 툴(`run_start` · `run_restart` — M6-14)만 [`agent_run_grant`] 로 [`AgentRunGrant`] 를 받는다. 재료는 둘이다: 그 프로젝트에
//! 닿는 MCP 세션의 증거([`AgentGrant`])와 **승인을 지난 계획**([`Approved`] — `plan::approve` 만 만든다). 띄울 것은 계획을 만들 때
//! 툴이 푼 실행 내용([`RunSpec`] — argv · 작업 폴더 · env 의 값 · 프로필, `.env` 는 그때 읽은 값)이고, 계획이 그것을 실어
//! (`PlanBuilder::runs`) 해시에 넣는다. 허가는 그 계획에 실린 [`RunSpec`] 목록만 띄운다 — 실행 관리자의 에이전트 갈래는 구성 id
//! 가 아니라 허가의 [`RunSpec`] 을 띄우고, 구성 · `.env` 를 다시 읽지 않는다(`plan` 머리 주석의 「`approve` 뒤의 부작용은 계획 때
//! 읽은 값으로」). 그래서 승인과 띄우기 사이에 바뀐 구성 · `.env` 는 돌지 않고, 에이전트가 고른 임의 argv · 셸은 띄울 길이 없다.
//!
//! 승인을 지나지 않은 값으로는 허가를 만들 수 없다 — [`Approved`] · [`AgentRunGrant`] 는 그 모듈 밖에서 만들 수 없다:
//!
//! ```compile_fail
//! let _ = pw_core::registry::plan::Approved { hash: String::new(), runs: Vec::new() };
//! ```
//!
//! ```compile_fail
//! let _ = pw_core::registry::exec::AgentRunGrant {
//!     project_id: String::new(),
//!     plan_hash: String::new(),
//!     runs: Vec::new(),
//! };
//! ```
//!
//! 계획 그 자체(승인 전의 값)로는 부를 수 없다:
//!
//! ```compile_fail
//! use pw_core::registry::agent::AgentGrant;
//! use pw_core::registry::exec::agent_run_grant;
//! use pw_core::registry::plan::ToolPlan;
//! fn launch(grant: &AgentGrant, plan: &ToolPlan) {
//!     let _ = agent_run_grant(grant, plan);
//! }
//! ```
//!
//! 바른 길(계획 → `approve` → 허가)은 컴파일된다:
//!
//! ```
//! use pw_core::registry::agent::AgentGrant;
//! use pw_core::registry::exec::{AgentRunGrant, agent_run_grant};
//! use pw_core::registry::plan::{ApprovalTicket, ToolPlan, approve};
//! use pw_rpc::RpcError;
//! fn launch(grant: &AgentGrant, ticket: Option<&ApprovalTicket>, plan: ToolPlan) -> Result<AgentRunGrant, RpcError> {
//!     let approved = approve(ticket, plan)?;
//!     agent_run_grant(grant, &approved)
//! }
//! ```

use std::collections::BTreeMap;
use std::path::PathBuf;

use pw_rpc::{ConnectionScope, RpcError, code};
use serde::{Deserialize, Serialize};

use super::agent::AgentGrant;
use super::path::{normalize, worktree_link};
use super::plan::Approved;
use super::state::scope_may_write;

/// 띄우려는 것.
pub enum Launch<'a> {
    /// 셸 · 명령(`terminal.create`). 그 프로젝트의 창만.
    Process,
    /// 외부 도구로 열기(`openWith.run`).
    OpenWith(OpenWithLaunch<'a>),
}

/// `openWith.run` 이 받은 것. Host 연결에는 이것이 등록된 프로젝트 루트를 등록된 도구로 여는 것뿐인지 본다.
pub struct OpenWithLaunch<'a> {
    /// 고른 도구. 없으면 설정의 기본 도구(목록에 있는 것만 고른다).
    pub tool: Option<&'a str>,
    /// 이 OS 의 도구 목록(확장 + 전역 설정)에 있는 id 인가.
    pub registered: &'a dyn Fn(&str) -> bool,
    pub worktree: Option<&'a str>,
    pub file: Option<&'a str>,
    pub line: Option<u32>,
    pub column: Option<u32>,
}

/// [`exec_grant`] 를 지난 허가. 띄울 프로젝트는 이것이 정한다([`Self::project_id`]).
#[derive(Debug)]
pub struct ExecGrant {
    project_id: String,
}

impl ExecGrant {
    pub fn project_id(&self) -> &str {
        &self.project_id
    }
}

/// 이 연결이 `project_id` 안에서 `launch` 를 띄워도 되는가(머리 주석). 거절은 `PERMISSION_DENIED` 이고 아무것도 띄우지 않는다.
pub fn exec_grant(
    scope: &ConnectionScope,
    project_id: &str,
    launch: &Launch<'_>,
) -> Result<ExecGrant, RpcError> {
    let granted = || ExecGrant {
        project_id: project_id.to_owned(),
    };
    if scope_may_write(scope, project_id) {
        return Ok(granted());
    }
    let reason = match (scope, launch) {
        (ConnectionScope::Host, Launch::OpenWith(open)) => match host_open(open) {
            None => return Ok(granted()),
            Some(reason) => reason,
        },
        _ => "only the project's own window may run processes in it",
    };
    Err(RpcError::new(code::PERMISSION_DENIED, reason)
        .with_data(serde_json::json!({ "projectId": project_id })))
}

/// Host 연결의 「다음에서 열기」 가 넘으면 안 되는 것. 없으면 `None`.
fn host_open(open: &OpenWithLaunch<'_>) -> Option<&'static str> {
    if open.worktree.is_some()
        || open.file.is_some()
        || open.line.is_some()
        || open.column.is_some()
    {
        return Some("a host connection opens only the registered project root");
    }
    if open.tool.is_some_and(|tool| !(open.registered)(tool)) {
        return Some("a host connection opens only with a registered tool");
    }
    None
}

/// 계획을 만들 때 툴이 푼 실행 내용 하나(머리 주석 「에이전트의 실행 허가」). 실행 관리자는 이것 그대로 띄운다 — 구성을 다시
/// 읽지 않는다. 복합 구성 · `before` 는 단계마다 하나다.
///
/// `Debug` 는 argv 의 원문 · env 의 값을 보이지 않는다(감사 · 로그에 명령줄 원문과 env 값을 남기지 않는다 — M5 의 규약).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunSpec {
    /// 그 실행의 구성 열쇠(구성 id · 감지된 태스크의 열쇠). 띄울 것이 아니라 보이고 묶는 이름이다.
    pub config: String,
    /// 띄울 argv(셸을 거치지 않는다). 첫째가 프로그램이다.
    pub argv: Vec<String>,
    /// 작업 폴더(절대 경로).
    pub cwd: PathBuf,
    /// 띄우는 환경 위에 얹는 env — 구성의 `env` 와 `.env` 파일(계획 때 읽은 값)을 합친 것.
    pub env: BTreeMap<String, String>,
    /// 고른 프로필.
    pub profile: Option<String>,
}

impl std::fmt::Debug for RunSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunSpec")
            .field("config", &self.config)
            .field("argc", &self.argv.len())
            .field("cwd", &self.cwd)
            .field("envKeys", &self.env.keys().collect::<Vec<_>>())
            .field("profile", &self.profile)
            .finish()
    }
}

/// [`agent_run_grant`] 를 지난 허가: 승인된 계획에 실린 [`RunSpec`] 만 띄운다. 이 모듈 밖에서는 만들 수 없다 — `Default` ·
/// `Deserialize` · `Clone` · 공개 생성자를 더하지 않는다.
#[derive(Debug)]
pub struct AgentRunGrant {
    project_id: String,
    plan_hash: String,
    runs: Vec<RunSpec>,
}

impl AgentRunGrant {
    /// 띄울 프로젝트(관문이 레지스트리에서 읽은 그 프로젝트).
    pub fn project_id(&self) -> &str {
        &self.project_id
    }

    /// 승인된 계획의 해시(감사 · 실행 기록).
    pub fn plan_hash(&self) -> &str {
        &self.plan_hash
    }

    /// 띄울 것 — 계획에 실린 차례 그대로.
    pub fn runs(&self) -> &[RunSpec] {
        &self.runs
    }
}

/// MCP 세션이 승인을 지난 계획의 [`RunSpec`] 을 띄워도 된다는 허가(머리 주석). 계획이 띄울 것을 싣지 않았거나(`PlanBuilder::runs`
/// 를 부르지 않은 계획 — 실행 툴이 아니다) argv 가 빈 [`RunSpec`] 이 있으면 툴의 버그로 거절한다(`INTERNAL_ERROR`).
///
/// **작업 폴더는 그 프로젝트 안이어야 한다**(17-m6-plan M6-10): 승인을 지난 값이어도 각 `cwd` 를 정규화(심링크 · `..` 를 푼다 —
/// [`normalize`])한 뒤 프로젝트 루트 아래이거나, 그 프로젝트의 연결 워크트리 안에서 프로젝트 루트에 대응하는 폴더 아래가 아니면
/// 아무것도 허가하지 않는다(`PERMISSION_DENIED`). 허가의 `cwd` 는 정규화한 경로다 — 띄우는 쪽은 그 경로로 띄운다.
pub fn agent_run_grant(agent: &AgentGrant, approved: &Approved) -> Result<AgentRunGrant, RpcError> {
    let runs = approved.runs();
    let problem = if runs.is_empty() {
        Some("the approved plan carries nothing to run")
    } else if runs.iter().any(|r| r.argv.is_empty()) {
        Some("the approved plan has a run without a program")
    } else if runs.iter().any(|r| !r.cwd.is_absolute()) {
        Some("the approved plan has a run whose working directory is not absolute")
    } else {
        None
    };
    if let Some(problem) = problem {
        return Err(RpcError::new(code::INTERNAL_ERROR, problem)
            .with_data(serde_json::json!({ "projectId": agent.project_id() })));
    }
    let project_root =
        normalize(agent.project().root()).unwrap_or_else(|_| agent.project().root().to_path_buf());
    let mut granted = Vec::with_capacity(runs.len());
    for run in runs {
        let Some(cwd) = inside_project(&project_root, &run.cwd) else {
            return Err(RpcError::new(
                code::PERMISSION_DENIED,
                "a run's working directory is outside the project and its worktrees",
            )
            .with_data(serde_json::json!({ "projectId": agent.project_id() })));
        };
        granted.push(RunSpec { cwd, ..run.clone() });
    }
    Ok(AgentRunGrant {
        project_id: agent.project_id().to_owned(),
        plan_hash: approved.hash().to_owned(),
        runs: granted,
    })
}

/// `cwd` 를 정규화한 경로가 프로젝트 루트 아래이거나 그 프로젝트의 연결 워크트리 안의 대응 루트 아래면 그 경로. 비교는 구성 요소
/// 단위 접두사다(`registry::path` 의 규칙).
fn inside_project(project_root: &std::path::Path, cwd: &std::path::Path) -> Option<PathBuf> {
    let cwd = normalize(cwd).ok()?;
    if cwd.starts_with(project_root) {
        return Some(cwd);
    }
    let link = worktree_link(&cwd)?;
    let rel = project_root.strip_prefix(&link.main_top).ok()?;
    cwd.starts_with(link.worktree_top.join(rel)).then_some(cwd)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use pw_rpc::{Caller, Role};

    use super::*;

    fn open(tool: Option<&str>) -> OpenWithLaunch<'_> {
        OpenWithLaunch {
            tool,
            registered: &|id| id == "fake",
            worktree: None,
            file: None,
            line: None,
            column: None,
        }
    }

    fn denied(result: Result<ExecGrant, RpcError>) -> bool {
        matches!(result, Err(e) if e.code == code::PERMISSION_DENIED)
    }

    #[test]
    fn the_project_window_runs_anything_in_its_project() {
        let scope = ConnectionScope::Project("p1".into());
        let grant = exec_grant(&scope, "p1", &Launch::Process).unwrap();
        assert_eq!(grant.project_id(), "p1");
        let with_file = OpenWithLaunch {
            worktree: Some("/w"),
            file: Some("/w/a.txt"),
            line: Some(3),
            ..open(Some("anything"))
        };
        assert!(exec_grant(&scope, "p1", &Launch::OpenWith(with_file)).is_ok());
    }

    #[test]
    fn other_project_and_file_windows_run_nothing() {
        let file = Caller::file(Role::Owner, PathBuf::from("/p1/a.txt")).scope;
        for scope in [ConnectionScope::Project("p2".into()), file] {
            assert!(denied(exec_grant(&scope, "p1", &Launch::Process)));
            assert!(denied(exec_grant(
                &scope,
                "p1",
                &Launch::OpenWith(open(Some("fake")))
            )));
        }
    }

    /// MCP 세션(결정 M5-D27)은 창 범위가 아니다 — Host 연결의 「다음에서 열기」 예외도 받지 않는다. 쓰기 판정도 거절이다.
    #[test]
    fn an_mcp_session_runs_nothing_and_writes_nothing() {
        let agent = ConnectionScope::Agent;
        assert!(!scope_may_write(&agent, "p1"));
        assert!(denied(exec_grant(&agent, "p1", &Launch::Process)));
        assert!(denied(exec_grant(
            &agent,
            "p1",
            &Launch::OpenWith(open(Some("fake")))
        )));
        assert!(denied(exec_grant(
            &agent,
            "p1",
            &Launch::OpenWith(open(None))
        )));
    }

    #[test]
    fn a_host_connection_only_opens_the_registered_root_with_a_registered_tool() {
        let host = ConnectionScope::Host;
        assert!(denied(exec_grant(&host, "p1", &Launch::Process)));
        assert!(exec_grant(&host, "p1", &Launch::OpenWith(open(Some("fake")))).is_ok());
        assert!(exec_grant(&host, "p1", &Launch::OpenWith(open(None))).is_ok());
        assert!(denied(exec_grant(
            &host,
            "p1",
            &Launch::OpenWith(open(Some("evil")))
        )));
        let beyond_root = [
            OpenWithLaunch {
                worktree: Some("/w"),
                ..open(None)
            },
            OpenWithLaunch {
                file: Some("/p1/a.txt"),
                ..open(None)
            },
            OpenWithLaunch {
                line: Some(1),
                ..open(None)
            },
            OpenWithLaunch {
                column: Some(1),
                ..open(None)
            },
        ];
        for launch in beyond_root {
            assert!(denied(exec_grant(&host, "p1", &Launch::OpenWith(launch))));
        }
    }

    mod agent_runs {
        use super::super::*;
        use crate::bus::{DEFAULT_CAPACITY, EventBus};
        use crate::config::AppPaths;
        use crate::registry::agent::agent_grant;
        use crate::registry::plan::{ApprovalTicket, ToolPlan, approve};
        use crate::registry::{NewProject, Registry};
        use pw_rpc::Tier;

        struct Fixture {
            _tmp: tempfile::TempDir,
            grant: AgentGrant,
            root: PathBuf,
        }

        fn fixture() -> Fixture {
            let tmp = tempfile::tempdir().unwrap();
            let root = crate::registry::path::simplify(std::fs::canonicalize(tmp.path()).unwrap())
                .join("work");
            std::fs::create_dir_all(&root).unwrap();
            let registry = Registry::new(
                AppPaths::under(&tmp.path().join("app")),
                None,
                EventBus::new(DEFAULT_CAPACITY),
            );
            let id = registry
                .create(NewProject {
                    root: root.clone(),
                    ..NewProject::default()
                })
                .unwrap()
                .id;
            let grant = agent_grant(&ConnectionScope::Agent, &registry, &id).unwrap();
            Fixture {
                _tmp: tmp,
                grant,
                root,
            }
        }

        fn spec(root: &std::path::Path, token: &str) -> RunSpec {
            RunSpec {
                config: "dev".into(),
                argv: vec!["npm".into(), "run".into(), "dev".into()],
                cwd: root.to_path_buf(),
                env: BTreeMap::from([("API_TOKEN".to_owned(), token.to_owned())]),
                profile: None,
            }
        }

        fn plan(runs: Vec<RunSpec>) -> ToolPlan {
            ToolPlan::builder(
                "run.tool.start",
                Tier::Exec,
                &serde_json::json!({"configId": "dev"}),
            )
            .summary("run dev")
            .runs(runs)
            .build()
        }

        fn approved(plan: ToolPlan) -> Approved {
            let ticket = ApprovalTicket {
                hash: plan.hash.clone(),
            };
            approve(Some(&ticket), plan).unwrap()
        }

        /// 허가는 승인된 계획에 실린 것 그대로를 띄운다(차례 · env 의 값까지) — 프로젝트는 관문이 읽은 것.
        #[test]
        fn the_grant_launches_exactly_the_runs_of_the_approved_plan() {
            let f = fixture();
            let before = spec(&f.root, "old");
            let step = RunSpec {
                config: "build".into(),
                ..spec(&f.root, "old")
            };
            let grant = agent_run_grant(
                &f.grant,
                &approved(plan(vec![step.clone(), before.clone()])),
            )
            .unwrap();
            assert_eq!(grant.runs(), [step, before]);
            assert_eq!(grant.project_id(), f.grant.project_id());
            assert!(grant.plan_hash().starts_with("sha256:"));
        }

        /// 띄울 것은 해시에 든다 — 승인 뒤 `.env` 의 값이 바뀌어 다시 만든 계획은 승인 표와 맞지 않는다.
        #[test]
        fn the_runs_are_part_of_the_hash() {
            let f = fixture();
            let seen = plan(vec![spec(&f.root, "old")]);
            let changed = plan(vec![spec(&f.root, "new")]);
            assert_ne!(seen.hash, changed.hash);
            assert_ne!(seen.hash, plan(Vec::new()).hash);
            let ticket = ApprovalTicket { hash: seen.hash };
            assert!(approve(Some(&ticket), changed).is_err());
        }

        /// 띄울 것을 싣지 않은 계획(실행 툴이 아닌 것)의 승인으로는 아무것도 띄우지 못한다. 직렬화한 계획(세션 · 창이 받은 꼴)에는
        /// 띄울 것이 실리지 않는다 — 되살려 승인해도 빈 허가다.
        #[test]
        fn a_plan_without_runs_grants_nothing() {
            let f = fixture();
            let e = agent_run_grant(&f.grant, &approved(plan(Vec::new()))).unwrap_err();
            assert_eq!(e.code, code::INTERNAL_ERROR);

            let original = plan(vec![spec(&f.root, "secret-value")]);
            let wire = serde_json::to_value(&original).unwrap();
            assert!(!wire.to_string().contains("secret-value"), "{wire}");
            let revived: ToolPlan = serde_json::from_value(wire).unwrap();
            assert_eq!(revived.hash, original.hash);
            let e = agent_run_grant(&f.grant, &approved(revived)).unwrap_err();
            assert_eq!(e.code, code::INTERNAL_ERROR);
        }

        #[test]
        fn a_run_without_a_program_or_with_a_relative_cwd_is_refused() {
            let f = fixture();
            let no_program = RunSpec {
                argv: Vec::new(),
                ..spec(&f.root, "x")
            };
            let relative = RunSpec {
                cwd: PathBuf::from("work"),
                ..spec(&f.root, "x")
            };
            for bad in [no_program, relative] {
                let runs = vec![spec(&f.root, "x"), bad];
                let e = agent_run_grant(&f.grant, &approved(plan(runs))).unwrap_err();
                assert_eq!(e.code, code::INTERNAL_ERROR);
            }
        }

        /// 승인을 지난 값이어도 작업 폴더가 프로젝트 밖이면 허가가 없다 — 밖의 폴더, `..` 로 나가는 경로, 밖을 가리키는 심링크
        /// 모두(정규화한 뒤 본다). 안의 하위 폴더는 정규화한 경로로 허가한다.
        #[test]
        fn a_run_outside_the_project_is_refused_even_when_approved() {
            let f = fixture();
            let outside = f.root.parent().unwrap().join("outside");
            std::fs::create_dir_all(&outside).unwrap();
            std::fs::create_dir_all(f.root.join("app")).unwrap();
            #[cfg(unix)]
            std::os::unix::fs::symlink(&outside, f.root.join("escape")).unwrap();
            let mut escapes = vec![
                outside.clone(),
                f.root.join("app/../../outside"),
                f.root.parent().unwrap().join("work-sibling"),
            ];
            if cfg!(unix) {
                escapes.push(f.root.join("escape"));
                escapes.push(f.root.join("escape/deeper"));
            }
            for cwd in escapes {
                let run = RunSpec {
                    cwd: cwd.clone(),
                    ..spec(&f.root, "x")
                };
                let runs = vec![spec(&f.root, "x"), run];
                let e = agent_run_grant(&f.grant, &approved(plan(runs))).unwrap_err();
                assert_eq!(e.code, code::PERMISSION_DENIED, "{}", cwd.display());
            }
            let inside = RunSpec {
                cwd: f.root.join("app/./sub/.."),
                ..spec(&f.root, "x")
            };
            let grant = agent_run_grant(&f.grant, &approved(plan(vec![inside]))).unwrap();
            assert_eq!(grant.runs()[0].cwd, f.root.join("app"));
        }

        /// 연결 워크트리 안의 대응 루트는 프로젝트 안이다. 다른 저장소의 워크트리는 아니다.
        #[test]
        fn a_run_in_a_linked_worktree_of_the_project_is_granted() {
            let f = fixture();
            let base = f.root.parent().unwrap();
            let wt = base.join("wt");
            fake_worktree(&f.root, &wt);
            let other_main = base.join("other");
            std::fs::create_dir_all(&other_main).unwrap();
            let other_wt = base.join("other-wt");
            fake_worktree(&other_main, &other_wt);
            std::fs::create_dir_all(wt.join("app")).unwrap();
            let grant = agent_run_grant(
                &f.grant,
                &approved(plan(vec![RunSpec {
                    cwd: wt.join("app"),
                    ..spec(&f.root, "x")
                }])),
            )
            .unwrap();
            assert_eq!(grant.runs()[0].cwd, wt.join("app"));
            let e = agent_run_grant(
                &f.grant,
                &approved(plan(vec![RunSpec {
                    cwd: other_wt.clone(),
                    ..spec(&f.root, "x")
                }])),
            )
            .unwrap_err();
            assert_eq!(e.code, code::PERMISSION_DENIED);
        }

        /// `git worktree add` 가 만드는 모양(`main` 의 `.git` 폴더 + `wt` 의 `.git` 파일).
        fn fake_worktree(main: &std::path::Path, wt: &std::path::Path) {
            let admin = main.join(".git/worktrees").join(wt.file_name().unwrap());
            std::fs::create_dir_all(&admin).unwrap();
            std::fs::write(admin.join("commondir"), "../..\n").unwrap();
            std::fs::create_dir_all(wt).unwrap();
            std::fs::write(wt.join(".git"), format!("gitdir: {}\n", admin.display())).unwrap();
            std::fs::write(
                admin.join("gitdir"),
                format!("{}\n", wt.join(".git").display()),
            )
            .unwrap();
        }

        /// 로그에 찍혀도 명령줄 원문 · env 값이 나가지 않는다.
        #[test]
        fn debug_shows_neither_the_command_line_nor_env_values() {
            let f = fixture();
            let mut s = spec(&f.root, "s3cr3t");
            s.argv.push("--password=hunter2".into());
            let grant = agent_run_grant(&f.grant, &approved(plan(vec![s]))).unwrap();
            let shown = format!("{grant:?}");
            for hidden in ["s3cr3t", "hunter2", "npm"] {
                assert!(!shown.contains(hidden), "{shown}");
            }
            assert!(shown.contains("API_TOKEN"), "{shown}");
        }
    }
}
