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
        format!(
            "Register {:?} as the project \"alpha\"",
            root.display().to_string()
        )
    );
    assert!(plan_text(&plan).contains(&format!("root: {:?}", root.display().to_string())));
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
    assert!(plan_text(&plan).contains("name: \"Beta app\"\nsettings: personal"));
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
        plan_text(&plan).starts_with(&format!("root: {:?}\n", real.display().to_string())),
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
        (
            f.dir("outer/sub/deeper"),
            "already opens as the registered project",
        ),
    ] {
        let e = f.call(&params(&root), None).unwrap_err();
        assert_eq!(e.code, code::INVALID_PARAMS, "{e:?}");
        assert!(detail(&e).contains(says), "{}", detail(&e));
    }
    // 등록된 프로젝트를 품는 위 폴더도 거절한다(리뷰 D1 의 메인 판정 — 안쪽에 등록 루트가 있는 루트는 받지 않는다).
    let e = f.call(&params(&f.base.join("work")), None).unwrap_err();
    assert!(
        detail(&e).contains("contains the registered project"),
        "{}",
        detail(&e)
    );
    assert_eq!(f.registry.list().unwrap().len(), 1);
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
        assert!(
            detail(&e).contains("git worktree") && detail(&e).contains("its trust would follow"),
            "{}",
            detail(&e)
        );
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

/// 리뷰 B1 의 탐침: 사람이 git 워크트리만 등록 · 신뢰한 저장소에서 에이전트가 그 메인 체크아웃(또는 그 안의, 워크트리 프로젝트에
/// 대응하는 폴더)을 등록하면 워크트리 프로젝트의 신뢰 주인이 새 프로젝트로 바뀐다 — 그래서 묻기 전에 거절하고, 워크트리의 신뢰는
/// 그대로다. 워크트리가 메인 체크아웃 안(`<main>/.worktrees/x` — 이 저장소의 꼴)이든 밖이든 같다.
#[test]
fn registering_the_main_checkout_of_a_trusted_worktree_project_is_refused() {
    for inside in [true, false] {
        let f = Fixture::new();
        let main = f.dir("repo");
        fs::create_dir_all(main.join("app")).unwrap();
        let wt = if inside {
            main.join(".worktrees/x")
        } else {
            f.base.join("work/wt")
        };
        let (main, wt) = repo_with_worktree(&main, &wt);
        fs::create_dir_all(wt.join("app")).unwrap();
        // 사람(창)이 워크트리와 그 안의 하위 폴더를 등록하고 신뢰했다.
        let human = |root: &Path| {
            f.registry
                .create(crate::registry::NewProject {
                    root: root.to_path_buf(),
                    ..crate::registry::NewProject::default()
                })
                .unwrap()
        };
        let worktree = human(&wt);
        f.registry.set_trust(&worktree.id, true).unwrap();
        // 메인 체크아웃 안의 다른 폴더(`main/app`)는 `wt` 의 주인이 되지 않는다(trust.rs: 대응 자리 `main` 을 품어야 한다).
        assert_eq!(
            f.registry.trust_shift(&main.join("app")).unwrap().moved,
            Vec::new()
        );
        let e = f.call(&params(&main), None).unwrap_err();
        assert_eq!(e.code, code::INVALID_PARAMS, "inside={inside}: {e:?}");
        assert!(
            detail(&e).contains("would become the trust owner of the registered project")
                && detail(&e).contains(&worktree.id),
            "{}",
            detail(&e)
        );
        let trust = f.registry.trust(&worktree.id).unwrap();
        assert_eq!(
            (trust.owner_id.as_str(), trust.state),
            (worktree.id.as_str(), TrustState::Trusted),
            "inside={inside}"
        );
        assert_eq!(f.registry.list().unwrap().len(), 1, "inside={inside}");

        // 워크트리 안의 하위 폴더 프로젝트만 있어도 그 메인 쪽 자리를 등록하면 주인이 바뀐다.
        let sub = human(&wt.join("app"));
        f.registry.set_trust(&sub.id, true).unwrap();
        let e = f.call(&params(&main.join("app")), None).unwrap_err();
        assert!(detail(&e).contains("trust owner"), "{}", detail(&e));
    }
}

/// `trust_shift` 는 신뢰 판정을 바꾸지 않는 등록(남의 저장소 · 품는 위 폴더)에는 아무것도 내지 않는다.
#[test]
fn an_unrelated_root_shifts_no_trust() {
    let f = Fixture::new();
    let (main, wt) = repo_with_worktree(&f.dir("repo"), &f.base.join("work/wt"));
    f.registry
        .create(crate::registry::NewProject {
            root: wt.clone(),
            ..crate::registry::NewProject::default()
        })
        .unwrap();
    let shift = f.registry.trust_shift(&f.dir("elsewhere")).unwrap();
    assert_eq!(
        (shift.moved.len(), shift.follows, shift.shadowed.len()),
        (0, None, 0)
    );
    // 품는 위 폴더는 신뢰 주인을 바꾸지 않지만 안쪽에 등록 루트가 있다.
    let shift = f.registry.trust_shift(&f.base.join("work")).unwrap();
    assert_eq!(
        (shift.moved.len(), shift.follows, shift.shadowed.len()),
        (0, None, 1)
    );
    assert_eq!(f.registry.trust_shift(&main).unwrap().moved.len(), 1);
}

/// 리뷰 F1: 줄바꿈 · 제어 문자 · 양방향 제어 문자가 든 이름 · 루트는 묻기 전에 거절한다 — 계획 글에 `trusted: yes` 같은 가짜
/// 줄을 심지 못한다. 거절을 지나는 값은 계획 글에서 인용돼 한 줄이다.
#[test]
fn line_breaks_and_control_characters_are_refused_and_values_are_quoted() {
    let f = Fixture::new();
    let root = f.dir("beta");
    for name in [
        "beta\ntrusted: yes",
        "beta\r\ntrusted: yes",
        "beta\u{7}",
        "be\u{85}ta",
        "beta\n",
        "a\u{202E}gnp.exe",
        "a\u{2066}b",
        "a\u{200F}b",
        // 리뷰 R7: 0폭 · LS · PS · BOM · SHY · 단어 결합자 · 태그 문자.
        "pit\u{200B}wall",
        "beta\u{2028}trusted: yes",
        "beta\u{2029}x",
        "\u{FEFF}beta",
        "be\u{00AD}ta",
        "be\u{2060}ta",
        "beta\u{E0041}",
    ] {
        let p = ProjectRegisterToolParams {
            name: Some(name.into()),
            ..params(&root)
        };
        let e = f.call(&p, None).unwrap_err();
        assert_eq!(e.code, code::INVALID_PARAMS, "{name:?}: {e:?}");
        assert!(detail(&e).contains("`name`"), "{name:?}: {}", detail(&e));
    }
    #[cfg(unix)]
    {
        let odd = f.dir("odd\ntrusted: yes");
        let e = f.call(&params(&odd), None).unwrap_err();
        assert_eq!(e.code, code::INVALID_PARAMS);
        assert!(detail(&e).contains("`root`"), "{}", detail(&e));
        // 이름이 깨끗한 링크로 그 폴더를 가리켜도 실제 경로에서 걸린다.
        let link = f.base.join("work/clean");
        std::os::unix::fs::symlink(&odd, &link).unwrap();
        let e = f.call(&params(&link), None).unwrap_err();
        assert!(detail(&e).contains("`root`"), "{}", detail(&e));
    }
    assert_eq!(f.registry.list().unwrap(), Vec::new());

    // 따옴표가 든 폴더 · 이름은 받고, 계획 글에서 인용돼 줄 구조가 그대로다.
    let quoted = f.dir("we\"ird");
    let p = ProjectRegisterToolParams {
        name: Some("say \"hi\"".into()),
        ..params(&quoted)
    };
    let plan = f.plan(&p).plan;
    let text = plan_text(&plan);
    assert_eq!(text.lines().count(), 4, "{text}");
    assert!(text.contains("name: \"say \\\"hi\\\"\""), "{text}");
    assert!(text.ends_with("\ntrusted: no"), "{text}");
}

/// 리뷰 R7: 에이전트가 정하는 이름은 200자(`MAX_NAME_CHARS`)까지다. 앞뒤 공백은 세지 않는다.
#[test]
fn names_longer_than_the_limit_are_refused() {
    let f = Fixture::new();
    let root = f.dir("beta");
    let named = |name: String| ProjectRegisterToolParams {
        name: Some(name),
        ..params(&root)
    };
    let e = f
        .call(&named("가".repeat(MAX_NAME_CHARS + 1)), None)
        .unwrap_err();
    assert_eq!(e.code, code::INVALID_PARAMS);
    assert!(detail(&e).contains("the limit is 200"), "{}", detail(&e));
    f.plan(&named(format!("  {}  ", "가".repeat(MAX_NAME_CHARS))));
    assert_eq!(f.registry.list().unwrap(), Vec::new());
}

/// 리뷰 D1 의 탐침: 사람이 메인 체크아웃의 하위 폴더 `M/app` 만 등록 · 신뢰하고 워크트리 `W` 는 등록하지 않았다 — `W/app/…` 은
/// `M/app` 프로젝트 + 활성 워크트리 `W` 로 열린다. 에이전트가 `W` 꼭대기를 등록하면 그 경로가 새(미신뢰) 프로젝트로 풀리게 되므로
/// 묻기 전에 거절하고, 해석은 그대로다.
#[test]
fn registering_a_worktree_top_that_would_take_over_a_main_subfolder_is_refused() {
    let f = Fixture::new();
    let main = f.dir("repo");
    fs::create_dir_all(main.join("app/src")).unwrap();
    let (main, wt) = repo_with_worktree(&main, &f.base.join("work/repo-wt"));
    fs::create_dir_all(wt.join("app/src")).unwrap();
    let app = f
        .registry
        .create(crate::registry::NewProject {
            root: main.join("app"),
            ..crate::registry::NewProject::default()
        })
        .unwrap();
    f.registry.set_trust(&app.id, true).unwrap();
    let opened = |f: &Fixture| f.registry.resolve(&wt.join("app/src")).unwrap().unwrap();
    let before = opened(&f);
    assert_eq!(before.project.id, app.id);
    assert_eq!(
        before.worktree.as_ref().map(|w| w.top.clone()),
        Some(wt.display().to_string())
    );

    let e = f.call(&params(&wt), None).unwrap_err();
    assert_eq!(e.code, code::INVALID_PARAMS, "{e:?}");
    assert!(
        detail(&e).contains("contains the registered project") && detail(&e).contains(&app.id),
        "{}",
        detail(&e)
    );
    let after = opened(&f);
    assert_eq!(
        (after.project.id, after.worktree),
        (app.id.clone(), before.worktree)
    );
    assert!(f.registry.is_trusted(&app.id));
    assert_eq!(f.registry.list().unwrap().len(), 1);
}
