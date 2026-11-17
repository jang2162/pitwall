//! `project_register` 의 두 단계 · 거절하는 루트 · 신뢰. 승인 경로(세션 · 창)를 지나는 왕복은 데스크톱의
//! `mcp_tests::project_write` 가 본다.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use pw_rpc::{ConnectionScope, code};
use serde_json::Value;

use super::*;
use crate::bus::{DEFAULT_CAPACITY, EventBus, Filter, Scope};
use crate::config::{AppPaths, PROJECT_DIR};
use crate::registry::TrustState;
use crate::registry::path::simplify;
use crate::registry::plan::{APPROVAL_REQUIRED, ApprovalRequired, ApprovalTicket};

struct Fixture {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    bus: EventBus,
    registry: Registry,
}

impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let base = simplify(fs::canonicalize(tmp.path()).unwrap());
        fs::create_dir_all(base.join("home")).unwrap();
        let bus = EventBus::new(DEFAULT_CAPACITY);
        let registry = Registry::new(
            AppPaths::under(&base.join("app")),
            Some(base.join("home")),
            bus.clone(),
        );
        Self {
            _tmp: tmp,
            base,
            bus,
            registry,
        }
    }

    fn dir(&self, rel: &str) -> PathBuf {
        let dir = self.base.join("work").join(rel);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// 승인 표 없이 부른다 — 계획을 실은 「승인 필요」 여야 한다.
    fn plan(&self, params: &ProjectRegisterToolParams) -> ApprovalRequired {
        let e = self.call(params, None).unwrap_err();
        assert_eq!(e.code, APPROVAL_REQUIRED, "{e:?}");
        serde_json::from_value(e.data.unwrap()).unwrap()
    }

    fn call(
        &self,
        params: &ProjectRegisterToolParams,
        hash: Option<&str>,
    ) -> Result<ProjectRegisterToolResult, RpcError> {
        let input = Planned {
            params: params.clone(),
            ticket: hash.map(|h| ApprovalTicket { hash: h.to_owned() }),
        };
        register_project(&ConnectionScope::Agent, &self.registry, &input)
    }

    /// 계획을 받고 그 해시로 다시 부른다(사람이 [이번만 허용] 을 누른 것).
    fn approve(&self, params: &ProjectRegisterToolParams) -> ProjectRegisterToolResult {
        let plan = self.plan(params).plan;
        self.call(params, Some(&plan.hash)).unwrap()
    }
}

fn params(root: &Path) -> ProjectRegisterToolParams {
    ProjectRegisterToolParams {
        root: root.display().to_string(),
        name: None,
        config_mode: None,
    }
}

/// invalid params 오류의 자세한 글(`data`).
fn detail(e: &RpcError) -> String {
    e.data
        .as_ref()
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

fn plan_text(plan: &ToolPlan) -> &str {
    match plan.changes.as_slice() {
        [PlanChange::Text { text, .. }] => text,
        other => panic!("{other:?}"),
    }
}

/// 계획 단계(승인 표 없음)는 레지스트리 · 폴더 · 이벤트에 아무것도 남기지 않는다. 승인 표가 맞으면 계획 때 푼 루트 · 이름으로
/// 등록하고 감지 요약을 싣는다. 새 프로젝트는 신뢰되지 않는다(아직 묻지 않음).
#[tokio::test]
async fn the_plan_step_has_no_side_effect_and_the_approved_call_registers_untrusted() {
    let f = Fixture::new();
    let root = f.dir("alpha");
    fs::write(root.join("Cargo.toml"), "[package]\n").unwrap();
    let mut events = f.bus.subscribe(
        Filter::all()
            .scope(Scope::Host)
            .topics([crate::registry::TOPIC_CHANGED]),
    );
    let required = f.plan(&params(&root));
    assert!(!required.mismatch);
    let plan = required.plan;
    assert_eq!(plan.method, "project.tool.register");
    assert_eq!(plan.tier, Tier::Write);
    assert_eq!(
        plan.summary,
        format!("Register {} as the project `alpha`", root.display())
    );
    assert!(plan_text(&plan).contains(&format!("root: {}", root.display())));
    assert!(plan_text(&plan).contains("trusted: no"));
    assert_eq!(plan.rules, crate::registry::plan::PlanRules::default());
    assert_eq!(
        f.registry.list().unwrap(),
        Vec::new(),
        "계획 단계가 등록했다"
    );
    assert!(!root.join(PROJECT_DIR).exists());
    assert!(events.try_recv().is_none(), "계획 단계가 이벤트를 냈다");

    let result = f.call(&params(&root), Some(&plan.hash)).unwrap();
    let project = &result.info.project;
    assert_eq!(project.roots, vec![root.display().to_string()]);
    assert_eq!(project.name, "alpha");
    assert_eq!(project.config_mode, ConfigMode::Repository);
    assert_eq!(result.info.build_files, ["Cargo.toml"]);
    assert_eq!(result.info.languages, ["rust"]);
    assert!(!result.trusted);
    let trust = f.registry.trust(&project.id).unwrap();
    assert_eq!(trust.state, TrustState::Unknown);
    assert_eq!(
        trust.owner_id, project.id,
        "다른 프로젝트의 신뢰를 따르지 않는다"
    );
    assert_eq!(f.registry.list().unwrap().len(), 1);
}

/// 이름 · 설정 저장 방식은 계획에 보이고 그대로 등록된다. 빈 이름은 묻기 전에 거절.
#[test]
fn the_name_and_settings_mode_are_planned_and_used() {
    let f = Fixture::new();
    let root = f.dir("beta");
    let p = ProjectRegisterToolParams {
        name: Some("  Beta app ".into()),
        config_mode: Some(ConfigMode::Personal),
        ..params(&root)
    };
    let plan = f.plan(&p).plan;
    assert!(plan_text(&plan).contains("name: Beta app\nsettings: personal"));
    let project = f.call(&p, Some(&plan.hash)).unwrap().info.project;
    assert_eq!(
        (project.name.as_str(), project.config_mode),
        ("Beta app", ConfigMode::Personal)
    );
    let e = f
        .call(
            &ProjectRegisterToolParams {
                name: Some(" ".into()),
                ..params(&f.dir("gamma"))
            },
            None,
        )
        .unwrap_err();
    assert_eq!(e.code, code::INVALID_PARAMS);
}

/// 심볼릭 링크와 `..` 는 계획 때 풀린다 — 계획과 등록된 루트가 실제 폴더다.
#[cfg(unix)]
#[test]
fn links_and_dots_resolve_to_the_real_folder_in_the_plan() {
    let f = Fixture::new();
    let real = f.dir("real");
    let link = f.base.join("work/link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let dotted = f.base.join("work/other/../link");
    f.dir("other");
    let plan = f.plan(&params(&dotted)).plan;
    assert!(
        plan_text(&plan).starts_with(&format!("root: {}\n", real.display())),
        "{}",
        plan_text(&plan)
    );
    let project = f
        .call(&params(&dotted), Some(&plan.hash))
        .unwrap()
        .info
        .project;
    assert_eq!(project.roots, vec![real.display().to_string()]);
    assert_eq!(project.name, "real", "이름은 실제 폴더의 것");
}

/// 승인 뒤 링크가 다른 폴더를 가리키면 다시 만든 계획의 해시가 달라 등록하지 않는다(`mismatch`).
#[cfg(unix)]
#[test]
fn a_link_moved_after_approval_changes_the_plan() {
    let f = Fixture::new();
    let (a, b) = (f.dir("a"), f.dir("b"));
    let link = f.base.join("work/link");
    std::os::unix::fs::symlink(&a, &link).unwrap();
    let approved = f.plan(&params(&link)).plan;
    fs::remove_file(&link).unwrap();
    std::os::unix::fs::symlink(&b, &link).unwrap();
    let e = f.call(&params(&link), Some(&approved.hash)).unwrap_err();
    assert_eq!(e.code, APPROVAL_REQUIRED);
    let again: ApprovalRequired = serde_json::from_value(e.data.unwrap()).unwrap();
    assert!(again.mismatch);
    assert!(plan_text(&again.plan).contains(&b.display().to_string()));
    assert_eq!(f.registry.list().unwrap(), Vec::new());
}

/// 승인을 지난 등록은 계획 때 푼 루트로만 한다 — 그 사이 그 경로가 다른 폴더로 풀리게 되면(폴더를 치우고 그 자리에 링크) 등록하지
/// 않는다. 입력을 다시 풀어 다른 폴더를 등록하지 않는다(TOCTOU 규약).
#[cfg(unix)]
#[test]
fn the_approved_registration_uses_the_planned_root_only() {
    let f = Fixture::new();
    let a = f.dir("a");
    let elsewhere = f.dir("elsewhere");
    let planned = f.registry.plan_registration(&params(&a)).unwrap();
    fs::rename(&a, f.base.join("work/a-moved")).unwrap();
    std::os::unix::fs::symlink(&elsewhere, &a).unwrap();
    let e = f.registry.register_planned(&planned).unwrap_err();
    assert_eq!(e.code, code::PERMISSION_DENIED, "{e:?}");
    assert_eq!(e.data.unwrap()["reason"], "planChanged");
    assert!(
        e.message.contains("not the folder that was approved"),
        "{}",
        e.message
    );
    assert_eq!(f.registry.list().unwrap(), Vec::new());
}

/// 넓은 경로(파일 시스템 루트 · 홈 · 홈의 위)와 상대 경로 · 없는 폴더 · 파일은 승인을 묻기 전에 거절한다.
#[test]
fn broad_relative_and_missing_roots_are_refused_before_asking() {
    let f = Fixture::new();
    let file = f.dir("x").join("file.txt");
    fs::write(&file, "").unwrap();
    let home = f.base.join("home");
    for (root, says) in [
        ("/".to_owned(), "too broad"),
        (home.display().to_string(), "too broad"),
        (f.base.display().to_string(), "too broad"),
        (home.join("..").display().to_string(), "too broad"),
        ("work/x".to_owned(), "absolute"),
        (
            f.base.join("nope").display().to_string(),
            "existing directory",
        ),
        (file.display().to_string(), "existing directory"),
    ] {
        let p = ProjectRegisterToolParams {
            root: root.clone(),
            name: None,
            config_mode: None,
        };
        let e = f.call(&p, None).unwrap_err();
        assert_eq!(e.code, code::INVALID_PARAMS, "{root}: {e:?}");
        assert!(detail(&e).contains(says), "{root}: {}", detail(&e));
    }
    // 홈 안쪽의 폴더는 된다.
    let code_dir = home.join("code");
    fs::create_dir_all(&code_dir).unwrap();
    f.plan(&params(&code_dir));
    assert_eq!(f.registry.list().unwrap(), Vec::new());
}

/// 이미 등록된 루트와 그 안쪽은 승인을 묻기 전에 거절한다. 등록된 프로젝트를 품는 위 폴더는 받는다.
#[test]
fn a_root_inside_a_registered_project_is_refused_before_asking() {
    let f = Fixture::new();
    let outer = f.dir("outer");
    f.approve(&params(&outer));
    for (root, says) in [
        (outer.clone(), "already the root"),
        (f.dir("outer/sub/deeper"), "inside the registered project"),
    ] {
        let e = f.call(&params(&root), None).unwrap_err();
        assert_eq!(e.code, code::INVALID_PARAMS, "{e:?}");
        assert!(detail(&e).contains(says), "{}", detail(&e));
    }
    let wide = f.approve(&params(&f.base.join("work")));
    assert!(!wide.trusted);
    assert_eq!(f.registry.list().unwrap().len(), 2);
}

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args([
            "-c",
            "user.name=pitwall-test",
            "-c",
            "user.email=test@pitwall.invalid",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "init.defaultBranch=main",
        ])
        .args(args)
        .current_dir(dir)
        .output()
        .expect("git 실행 파일이 있어야 워크트리 시험을 돌린다");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// `main` 저장소와 `wt` 연결 워크트리(둘 다 실제 경로).
fn repo_with_worktree(main: &Path, wt: &Path) -> (PathBuf, PathBuf) {
    fs::create_dir_all(main).unwrap();
    git(main, &["init", "-q"]);
    git(
        main,
        &["commit", "-q", "--allow-empty", "--no-verify", "-m", "init"],
    );
    git(
        main,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "feature",
            &wt.display().to_string(),
        ],
    );
    let real = |p: &Path| simplify(fs::canonicalize(p).unwrap());
    (real(main), real(wt))
}

/// 신뢰한 프로젝트의 git 워크트리를 따로 등록하면 그 신뢰를 따른다 — 그래서 승인을 묻기 전에 거절한다(워크트리 안쪽도).
/// 메인 저장소가 등록되지 않은 워크트리는 등록되고 미신뢰다.
#[test]
fn a_worktree_of_a_registered_project_is_refused_so_no_trust_is_inherited() {
    let f = Fixture::new();
    let (main, wt) = repo_with_worktree(&f.dir("repo"), &f.base.join("work/wt"));
    fs::create_dir_all(wt.join("app")).unwrap();
    let main_project = f.approve(&params(&main)).info.project;
    f.registry.set_trust(&main_project.id, true).unwrap();
    for root in [wt.clone(), wt.join("app")] {
        let e = f.call(&params(&root), None).unwrap_err();
        assert_eq!(e.code, code::INVALID_PARAMS, "{e:?}");
        assert!(detail(&e).contains("git worktree"), "{}", detail(&e));
    }
    assert_eq!(f.registry.list().unwrap().len(), 1);

    let (_other_main, other_wt) =
        repo_with_worktree(&f.dir("other-repo"), &f.base.join("work/other-wt"));
    let registered = f.approve(&params(&other_wt));
    assert!(!registered.trusted);
    let trust = f.registry.trust(&registered.info.project.id).unwrap();
    assert_eq!(trust.state, TrustState::Unknown);
}

/// 툴 메서드는 MCP 세션만 부른다 — 창 · Host 범위는 계획도 만들지 않는다.
#[test]
fn only_an_mcp_session_may_call_it() {
    let f = Fixture::new();
    let root = f.dir("alpha");
    let input = Planned {
        params: params(&root),
        ticket: None,
    };
    for scope in [ConnectionScope::Host, ConnectionScope::Project("p".into())] {
        let e = register_project(&scope, &f.registry, &input).unwrap_err();
        assert_eq!(e.code, code::PERMISSION_DENIED, "{scope:?}");
        assert_eq!(e.data.unwrap()["reason"], Value::from("notAgentSession"));
    }
}
