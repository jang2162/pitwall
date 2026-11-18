//! `open_with_upsert` 의 두 단계 · 형식 검사 · 계획의 diff. 승인 경로(세션 · 창 · `auto_approve`)를 지나는 왕복은 데스크톱의
//! `mcp_tests::project_write` 가 본다.

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use pw_core::bus::{DEFAULT_CAPACITY, EventBus, Scope};
use pw_core::config::{AppPaths, ConfigSource, ConfigStore};
use pw_core::registry::Registry as Projects;
use pw_core::registry::plan::{APPROVAL_REQUIRED, ApprovalRequired, ApprovalTicket};
use pw_runtime::{LoginShell, Runtime};
use semver::Version;

use super::*;
use crate::HostParts;

struct Fixture {
    _tmp: tempfile::TempDir,
    file: PathBuf,
    open_with: OpenWith,
}

impl Fixture {
    /// `settings` 가 있으면 전역 `settings.toml` 내용, 없으면 파일이 없다.
    fn new(settings: Option<&str>) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::under(&tmp.path().join("app"));
        fs::create_dir_all(&paths.config).unwrap();
        let file = paths.config.join("settings.toml");
        if let Some(settings) = settings {
            fs::write(&file, settings).unwrap();
        }
        let bus = EventBus::new(DEFAULT_CAPACITY);
        let global = ConfigStore::open(ConfigSource::global(&paths), bus.clone(), Scope::Host);
        let projects = Projects::new(paths.clone(), None, bus);
        let extensions = Arc::new(pw_ext::Registry::from_roots(
            &Version::new(0, 0, 0),
            pw_ext::builtin::roots(),
        ));
        let runtime = Runtime::new(LoginShell::new(None, Duration::from_secs(1)));
        let open_with = OpenWith::new(extensions, Arc::new(runtime));
        open_with.attach(HostParts {
            paths,
            global,
            projects,
        });
        Self {
            _tmp: tmp,
            file,
            open_with,
        }
    }

    fn call(
        &self,
        params: &OpenWithUpsertParams,
        hash: Option<&str>,
    ) -> Result<OpenWithUpsertResult, RpcError> {
        let input = Planned {
            params: params.clone(),
            ticket: hash.map(|h| ApprovalTicket { hash: h.to_owned() }),
        };
        self.open_with.upsert(&ConnectionScope::Agent, &input)
    }

    /// 승인 표 없이 부른다 — 계획을 실은 「승인 필요」 여야 한다.
    fn plan(&self, params: &OpenWithUpsertParams) -> ApprovalRequired {
        let e = self.call(params, None).unwrap_err();
        assert_eq!(e.code, APPROVAL_REQUIRED, "{e:?}");
        serde_json::from_value(e.data.unwrap()).unwrap()
    }

    fn text(&self) -> Option<String> {
        fs::read_to_string(&self.file).ok()
    }
}

fn argv(items: &[&str]) -> Option<Vec<String>> {
    Some(items.iter().map(|s| (*s).to_owned()).collect())
}

fn file_change(plan: &ToolPlan) -> (Option<String>, Option<String>) {
    match plan.changes.as_slice() {
        [PlanChange::File { before, after, .. }] => (before.clone(), after.clone()),
        other => panic!("{other:?}"),
    }
}

const OLD: &str = "schema = 1\n[ui]\nlanguage = \"ko\"\n\n[open_with.tools.zed]\ncommand = [\"/opt/old-zed\", \"{project_root}\"]\nfile_command = [\"/opt/old-zed\", \"{file}\"]\n";

/// 계획 단계는 파일을 고치지 않는다. 계획은 `settings.toml` 의 앞뒤 내용이고, 이미 있던 도구를 덮으면 옛 값이 diff 의 앞쪽에
/// 보이고 뒤쪽에는 남지 않는다(표를 통째로 바꾼다). 계획 규칙은 자동 승인 · 세션 허용이 없다. 승인 표가 맞으면 그 내용 그대로 쓴다.
#[test]
fn the_plan_step_writes_nothing_and_the_diff_shows_the_old_values() {
    let f = Fixture::new(Some(OLD));
    let params = OpenWithUpsertParams {
        id: "zed".into(),
        command: argv(&["zed", "{project_root}"]),
        ..Default::default()
    };
    let required = f.plan(&params);
    let plan = required.plan;
    assert_eq!(f.text().as_deref(), Some(OLD), "계획 단계가 파일을 고쳤다");
    assert_eq!(plan.method, "openWith.tool.upsert");
    assert_eq!(
        plan.rules,
        PlanRules {
            auto_approve: false,
            remember: Remember::Never,
            policy_auto: false,
        }
    );
    assert_eq!(
        plan.summary,
        "Change the external tool `zed` in the global settings"
    );
    let (before, after) = file_change(&plan);
    assert_eq!(before.as_deref(), Some(OLD));
    let after = after.unwrap();
    assert!(before.unwrap().contains("/opt/old-zed"));
    assert!(!after.contains("/opt/old-zed"), "{after}");
    assert!(after.contains("[open_with.tools.zed]\ncommand = [\"zed\", \"{project_root}\"]\n"));
    assert!(after.contains("language = \"ko\""), "다른 설정은 그대로");

    let result = f.call(&params, Some(&plan.hash)).unwrap();
    assert_eq!(f.text(), Some(after));
    assert_eq!(result.file, f.file.display().to_string());
    assert!(!result.overridden_locally);
    let tool = result.tool.unwrap();
    assert!(tool.from_settings);
    // `command` 만 적었으므로 확장의 `file_command` 도 버려졌다.
    assert!(!tool.opens_files);
    assert_eq!(tool.problem, None);
}

/// 없는 파일은 계획의 앞쪽이 `null` 이고 승인 뒤 만들어진다. 새 도구는 목록에 들어온다.
#[test]
fn a_new_tool_creates_the_settings_file_after_approval() {
    let f = Fixture::new(None);
    let params = OpenWithUpsertParams {
        id: "my-editor".into(),
        label: Some("My Editor".into()),
        command: argv(&["/usr/local/bin/my-editor", "{project_root}"]),
        file_command: argv(&["/usr/local/bin/my-editor", "{file}:{line}"]),
        platforms: Some(vec![OpenWithPlatform::Macos, OpenWithPlatform::Linux]),
    };
    let plan = f.plan(&params).plan;
    assert_eq!(f.text(), None, "계획 단계가 파일을 만들었다");
    assert_eq!(
        plan.summary,
        "Add the external tool `my-editor` to the global settings"
    );
    let (before, after) = file_change(&plan);
    assert_eq!(before, None);
    assert!(
        after
            .unwrap()
            .contains("platforms = [\"macos\", \"linux\"]")
    );
    let result = f.call(&params, Some(&plan.hash)).unwrap();
    if cfg!(any(target_os = "macos", target_os = "linux")) {
        let tool = result.tool.unwrap();
        assert_eq!((tool.label.as_str(), tool.opens_files), ("My Editor", true));
        assert_eq!(tool.extension, None);
    }
}

/// 형식 오류 · 모자란 입력은 승인을 묻기 전에(계획 없이) 거절하고 파일을 건드리지 않는다. 명령 검사는 목록의 것과 같은 글이다.
#[test]
fn malformed_commands_are_refused_before_asking() {
    let f = Fixture::new(Some(OLD));
    let base = OpenWithUpsertParams {
        id: "fake".into(),
        label: Some("Fake".into()),
        command: argv(&["/opt/fake", "{project_root}"]),
        ..Default::default()
    };
    for (params, says) in [
        (
            OpenWithUpsertParams {
                command: argv(&["{project_root}/run"]),
                ..base.clone()
            },
            "cannot use variables",
        ),
        (
            OpenWithUpsertParams {
                command: argv(&[]),
                ..base.clone()
            },
            "is empty",
        ),
        (
            OpenWithUpsertParams {
                command: argv(&["fake", "{file}"]),
                ..base.clone()
            },
            "only `file_command` can use",
        ),
        (
            OpenWithUpsertParams {
                file_command: argv(&["fake", "{project_root}"]),
                ..base.clone()
            },
            "{file}",
        ),
        (
            OpenWithUpsertParams {
                command: argv(&["fake", "{nope}"]),
                ..base.clone()
            },
            "nope",
        ),
        (
            OpenWithUpsertParams {
                label: Some("  ".into()),
                ..base.clone()
            },
            "non-empty",
        ),
        (
            OpenWithUpsertParams {
                label: None,
                ..base.clone()
            },
            "needs both `label` and `command`",
        ),
        (
            OpenWithUpsertParams {
                command: None,
                ..base.clone()
            },
            "needs both `label` and `command`",
        ),
        (
            OpenWithUpsertParams {
                id: "Bad Id".into(),
                ..base.clone()
            },
            "lowercase",
        ),
        (
            OpenWithUpsertParams {
                id: "zed".into(),
                ..Default::default()
            },
            "at least one",
        ),
    ] {
        let e = f.call(&params, None).unwrap_err();
        assert_eq!(e.code, code::INVALID_PARAMS, "{params:?}: {e:?}");
        let detail = e.data.as_ref().and_then(Value::as_str).unwrap_or_default();
        assert!(detail.contains(says), "{params:?}: {detail}");
    }
    assert_eq!(f.text().as_deref(), Some(OLD));
}

/// 승인 뒤 사람이 `settings.toml` 을 고쳤으면 다시 만든 계획의 해시가 달라 쓰지 않는다(`mismatch`). 승인과 쓰기 사이에 바뀌면
/// 계획 때 만든 내용을 쓰지 않는다(다시 만들지도 않는다).
#[test]
fn a_settings_file_changed_after_approval_is_not_overwritten() {
    let f = Fixture::new(Some(OLD));
    let params = OpenWithUpsertParams {
        id: "zed".into(),
        command: argv(&["zed", "{project_root}"]),
        ..Default::default()
    };
    let approved = f.plan(&params).plan;
    let edited = format!("{OLD}\n[db]\ngrid_max_rows = 5\n");
    fs::write(&f.file, &edited).unwrap();
    let e = f.call(&params, Some(&approved.hash)).unwrap_err();
    assert_eq!(e.code, APPROVAL_REQUIRED);
    let again: ApprovalRequired = serde_json::from_value(e.data.unwrap()).unwrap();
    assert!(again.mismatch);
    assert_eq!(f.text(), Some(edited.clone()));

    // 승인(해시 확인)을 지난 뒤 · 쓰기 전에 바뀐 경우.
    let planned = f.open_with.plan_upsert(&params).unwrap();
    fs::write(&f.file, OLD).unwrap();
    let e = f.open_with.apply_upsert("zed", &planned).unwrap_err();
    assert_eq!(e.code, code::PERMISSION_DENIED, "{e:?}");
    assert_eq!(e.data.unwrap()["reason"], "planChanged");
    assert_eq!(f.text().as_deref(), Some(OLD));
}

/// `settings.local.toml` 이 같은 도구를 적었으면 계획 요약과 결과가 알린다(그 파일은 고치지 않는다).
#[test]
fn a_local_override_is_reported() {
    let f = Fixture::new(Some(OLD));
    let local = f.file.with_file_name("settings.local.toml");
    let local_text = "schema = 1\n[open_with.tools.zed]\nlabel = \"Mine\"\n";
    fs::write(&local, local_text).unwrap();
    let params = OpenWithUpsertParams {
        id: "zed".into(),
        command: argv(&["zed", "{project_root}"]),
        ..Default::default()
    };
    let plan = f.plan(&params).plan;
    assert!(
        plan.summary.contains("settings.local.toml"),
        "{}",
        plan.summary
    );
    let result = f.call(&params, Some(&plan.hash)).unwrap();
    assert!(result.overridden_locally);
    assert_eq!(fs::read_to_string(&local).unwrap(), local_text);
}

/// 툴 메서드는 MCP 세션만 부른다.
#[test]
fn only_an_mcp_session_may_call_it() {
    let f = Fixture::new(Some(OLD));
    let input = Planned {
        params: OpenWithUpsertParams {
            id: "zed".into(),
            command: argv(&["zed", "{project_root}"]),
            ..Default::default()
        },
        ticket: None,
    };
    for scope in [ConnectionScope::Host, ConnectionScope::Project("p".into())] {
        let e = f.open_with.upsert(&scope, &input).unwrap_err();
        assert_eq!(e.code, code::PERMISSION_DENIED, "{scope:?}");
    }
    assert_eq!(f.text().as_deref(), Some(OLD));
}

#[test]
fn tool_ids_are_lowercase_words() {
    for good in ["zed", "my-editor", "idea_eap", "2fa", &"a".repeat(MAX_ID)] {
        assert!(valid_id(good), "{good}");
    }
    for bad in [
        "",
        "-x",
        "_x",
        "Zed",
        "a b",
        "a.b",
        "새",
        &"a".repeat(MAX_ID + 1),
    ] {
        assert!(!valid_id(bad), "{bad}");
    }
}
