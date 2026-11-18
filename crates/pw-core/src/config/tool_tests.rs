use std::fs;

use serde_json::json;

use super::*;
use crate::bus::{DEFAULT_CAPACITY, EventBus};
use crate::config::{PROJECT_BACKUP_DIR, PROJECT_DIR};
use crate::registry::NewProject;
use crate::registry::path::normalize;

struct Fixture {
    _tmp: tempfile::TempDir,
    paths: AppPaths,
    registry: Registry,
    root: PathBuf,
    id: String,
}

fn fixture(mode: ConfigMode) -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let base = normalize(tmp.path()).unwrap();
    let root = base.join("work").join("alpha");
    fs::create_dir_all(&root).unwrap();
    let paths = AppPaths::under(&base.join("app"));
    let registry = Registry::new(paths.clone(), None, EventBus::new(DEFAULT_CAPACITY));
    let id = registry
        .create(NewProject {
            root: root.clone(),
            config_mode: mode,
            ..NewProject::default()
        })
        .unwrap()
        .id;
    Fixture {
        _tmp: tmp,
        paths,
        registry,
        root,
        id,
    }
}

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

impl Fixture {
    fn get(&self, project: bool, key: Option<&str>) -> SettingsGetToolResult {
        settings_get(
            &ConnectionScope::Agent,
            &self.paths,
            &self.registry,
            &SettingsGetToolParams {
                project_id: project.then(|| self.id.clone()),
                worktree: None,
                key: key.map(str::to_owned),
            },
        )
        .unwrap()
    }

    fn global(&self, body: &str) {
        write(&self.paths.config.join("settings.toml"), body);
    }
}

fn entry<'a>(result: &'a SettingsGetToolResult, key: &str) -> Option<&'a SettingEntry> {
    result.entries.iter().find(|e| e.key == key)
}

fn value_and_layer(result: &SettingsGetToolResult, key: &str) -> (Value, SettingLayer) {
    let e = entry(result, key).unwrap_or_else(|| panic!("no `{key}` in {:?}", result.entries));
    (e.value.clone(), e.layer)
}

/// 구버전(`schema` 키가 없는) 파일을 읽어도 그 파일은 바이트 그대로이고 `.backup/` 이 생기지 않는다 — 전역 · 프로젝트 둘 다.
#[test]
fn an_old_schema_file_is_read_without_writing_anything() {
    let f = fixture(ConfigMode::Repository);
    let project_text = "# hand written, schema 0\n[mcp]\npreset = \"read-only\"\n";
    let project_file = f.root.join(PROJECT_DIR).join("project.toml");
    write(&project_file, project_text);
    let global_text = "[ui]\nlanguage = \"ko\"\n";
    f.global(global_text);

    let result = f.get(true, None);
    assert_eq!(
        value_and_layer(&result, "mcp.preset"),
        (json!("read-only"), SettingLayer::Shared)
    );
    assert_eq!(
        value_and_layer(&result, "ui.language"),
        (json!("ko"), SettingLayer::Global)
    );
    assert_eq!(fs::read_to_string(&project_file).unwrap(), project_text);
    assert_eq!(
        fs::read_to_string(f.paths.config.join("settings.toml")).unwrap(),
        global_text
    );
    assert!(!f.root.join(PROJECT_DIR).join(PROJECT_BACKUP_DIR).exists());
    assert!(!f.paths.backup_dir().exists(), "전역 설정의 보관 폴더");
    // 읽은 파일은 메모리에서 올린 스키마로 보인다.
    let shared = result
        .files
        .iter()
        .find(|file| file.layer == SettingLayer::Shared)
        .unwrap();
    assert_eq!(shared.path, project_file.display().to_string());
}

/// 값마다 이긴 레이어: 내장 기본값 → 전역 → 공유 → 개인. 병합 규칙과 같게 — 위 레이어의 스칼라는 아래의 테이블을 통째로 이긴다.
#[test]
fn each_value_names_the_layer_it_came_from() {
    let f = fixture(ConfigMode::Repository);
    f.global("schema = 1\n[mcp]\npreset = \"read-only\"\n[a.b]\nc = 1\n");
    let pw = f.root.join(PROJECT_DIR);
    write(
        &pw.join("project.toml"),
        "schema = 1\n[mcp.tools]\n\"project_*\" = false\n[a]\nb = 2\n",
    );
    write(
        &pw.join("project.local.toml"),
        "schema = 1\n[mcp]\npreset = \"full\"\n",
    );

    let result = f.get(true, None);
    assert_eq!(
        value_and_layer(&result, "update.channel"),
        (json!("stable"), SettingLayer::Builtin)
    );
    assert_eq!(
        value_and_layer(&result, "mcp.preset"),
        (json!("full"), SettingLayer::Personal)
    );
    assert_eq!(
        value_and_layer(&result, "mcp.tools.\"project_*\""),
        (json!(false), SettingLayer::Shared)
    );
    assert_eq!(
        value_and_layer(&result, "a.b"),
        (json!(2), SettingLayer::Shared)
    );
    assert!(entry(&result, "a.b.c").is_none(), "{:?}", result.entries);
    assert_eq!(result.project_id.as_deref(), Some(f.id.as_str()));
    let layers: Vec<_> = result.files.iter().map(|file| file.layer).collect();
    assert_eq!(
        layers,
        [
            SettingLayer::Global,
            SettingLayer::Shared,
            SettingLayer::Personal
        ]
    );

    // 전역만.
    let global = f.get(false, None);
    assert_eq!(global.project_id, None);
    assert_eq!(
        value_and_layer(&global, "mcp.preset"),
        (json!("read-only"), SettingLayer::Global)
    );
    assert_eq!(
        value_and_layer(&global, "a.b.c"),
        (json!(1), SettingLayer::Global)
    );
    assert!(entry(&global, "mcp.tools.\"project_*\"").is_none());
}

/// git 이 추적하는(커밋된) `.local.toml` 의 값과 파일은 개인이 아니라 공유 레이어로 보인다.
#[test]
fn a_committed_local_file_is_the_shared_layer() {
    use crate::config::tracked::tests::git;
    let f = fixture(ConfigMode::Repository);
    let pw = f.root.join(PROJECT_DIR);
    write(&pw.join("project.toml"), "schema = 1\n");
    write(
        &pw.join("project.local.toml"),
        "schema = 1\n[mcp]\npreset = \"full\"\n",
    );
    git(&f.root, &["init", "-q"]);
    let layer_of_file = |result: &SettingsGetToolResult| {
        result
            .files
            .iter()
            .find(|file| file.path.ends_with("project.local.toml"))
            .unwrap()
            .layer
    };
    let result = f.get(true, None);
    assert_eq!(
        value_and_layer(&result, "mcp.preset"),
        (json!("full"), SettingLayer::Personal)
    );
    assert_eq!(layer_of_file(&result), SettingLayer::Personal);

    git(&f.root, &["add", "-f", ".pitwall/project.local.toml"]);
    let result = f.get(true, None);
    assert_eq!(
        value_and_layer(&result, "mcp.preset"),
        (json!("full"), SettingLayer::Shared)
    );
    assert_eq!(layer_of_file(&result), SettingLayer::Shared);
}

/// 개인 전용 프로젝트의 `project.toml` 은 커밋되지 않는 개인 설정이다.
#[test]
fn a_personal_project_file_is_the_personal_layer() {
    let f = fixture(ConfigMode::Personal);
    let project = f.registry.get(&f.id).unwrap();
    write(
        &f.paths
            .personal_project_dir(&project.slug)
            .join("project.toml"),
        "schema = 1\n[mcp]\npreset = \"full\"\n",
    );
    assert_eq!(
        value_and_layer(&f.get(true, None), "mcp.preset"),
        (json!("full"), SettingLayer::Personal)
    );
}

#[test]
fn key_narrows_to_that_table_or_key() {
    let f = fixture(ConfigMode::Repository);
    f.global("schema = 1\n[mcp]\npreset = \"full\"\n[mcp.groups]\ndb = false\n[mcpx]\nother = 1\n");
    let keys = |key| -> Vec<String> {
        f.get(false, Some(key))
            .entries
            .into_iter()
            .map(|e| e.key)
            .collect()
    };
    assert_eq!(keys("mcp"), ["mcp.groups.db", "mcp.preset"]);
    assert_eq!(keys("mcp.preset"), ["mcp.preset"]);
    assert_eq!(keys("mcp.pre"), Vec::<String>::new(), "접두는 키 단위다");
    assert_eq!(keys("nothing"), Vec::<String>::new());
}

/// 키 이름이 비밀을 가리키면 값을 가린다 — 배열 안의 테이블에 든 것도.
#[test]
fn secret_keys_are_redacted() {
    let f = fixture(ConfigMode::Repository);
    f.global(
        "schema = 1\n[db]\napi_token = \"sk-1\"\nmax_rows = 10\n[[hooks]]\nname = \"x\"\npassword = \"p\"\n[auth]\nuser = \"u1\"\n",
    );
    let result = f.get(false, None);
    let token = entry(&result, "db.api_token").unwrap();
    assert_eq!(
        (token.value.clone(), token.redacted),
        (json!(REDACTED), true)
    );
    let rows = entry(&result, "db.max_rows").unwrap();
    assert_eq!((rows.value.clone(), rows.redacted), (json!(10), false));
    let hooks = entry(&result, "hooks").unwrap();
    assert!(hooks.redacted);
    assert_eq!(hooks.value, json!([{ "name": "x", "password": REDACTED }]));
    // 잎 이름은 평범해도 위 표 이름이 비밀 단어면 가린다.
    let user = entry(&result, "auth.user").unwrap();
    assert_eq!((user.value.clone(), user.redacted), (json!(REDACTED), true));
    let all = serde_json::to_string(&result).unwrap();
    assert!(
        !all.contains("sk-1") && !all.contains("\"p\"") && !all.contains("u1"),
        "{all}"
    );
}

/// 파싱 오류의 글(문제 줄의 값을 인용한다)은 싣지 않고 파일만 알린다.
#[test]
fn a_broken_file_is_named_without_its_contents() {
    let f = fixture(ConfigMode::Repository);
    f.global("schema = 1\ntoken = \"sk-secret\" broken\n");
    let e = settings_get(
        &ConnectionScope::Agent,
        &f.paths,
        &f.registry,
        &SettingsGetToolParams::default(),
    )
    .unwrap_err();
    let text = serde_json::to_string(&e).unwrap();
    assert!(text.contains("settings.toml"), "{text}");
    assert!(!text.contains("sk-secret"), "{text}");
}

#[test]
fn only_an_mcp_session_may_call_it_and_worktree_needs_a_project() {
    let f = fixture(ConfigMode::Repository);
    for (scope, project) in [
        (ConnectionScope::Host, None),
        (ConnectionScope::Project(f.id.clone()), Some(f.id.clone())),
    ] {
        let e = settings_get(
            &scope,
            &f.paths,
            &f.registry,
            &SettingsGetToolParams {
                project_id: project,
                ..SettingsGetToolParams::default()
            },
        )
        .unwrap_err();
        assert_eq!(e.data.unwrap()["reason"], "notAgentSession");
    }
    let e = settings_get(
        &ConnectionScope::Agent,
        &f.paths,
        &f.registry,
        &SettingsGetToolParams {
            worktree: Some("/somewhere".into()),
            ..SettingsGetToolParams::default()
        },
    )
    .unwrap_err();
    assert_eq!(e.code, code::INVALID_PARAMS);
    let e = settings_get(
        &ConnectionScope::Agent,
        &f.paths,
        &f.registry,
        &SettingsGetToolParams {
            project_id: Some("nope".into()),
            ..SettingsGetToolParams::default()
        },
    )
    .unwrap_err();
    assert_eq!(e.code, code::INVALID_PARAMS, "agent_grant 의 오류 그대로");
}

/// 실제 git 워크트리와 같은 모양(`.git` 파일 → `<main>/.git/worktrees/wt` + `commondir`) — `config::source` 시험과 같다.
fn fake_worktree(base: &Path) -> (PathBuf, PathBuf) {
    let main = base.join("main");
    let wt = base.join("wt");
    let admin = main.join(".git/worktrees/wt");
    fs::create_dir_all(&admin).unwrap();
    fs::create_dir_all(&wt).unwrap();
    fs::write(admin.join("commondir"), "../..\n").unwrap();
    fs::write(wt.join(".git"), format!("gitdir: {}\n", admin.display())).unwrap();
    fs::write(
        admin.join("gitdir"),
        format!("{}\n", wt.join(".git").display()),
    )
    .unwrap();
    (main, wt)
}

/// `worktree` 를 주면 그 워크트리의 커밋된 `.pitwall/project.toml` 을 읽는다(브랜치마다 다를 수 있다). 비면 메인 체크아웃이고,
/// 그 프로젝트의 워크트리가 아니면 거절한다.
#[test]
fn worktree_reads_that_checkouts_project_settings() {
    let tmp = tempfile::tempdir().unwrap();
    let base = normalize(tmp.path()).unwrap();
    let (main, wt) = fake_worktree(&base);
    write(
        &main.join(".pitwall/project.toml"),
        "schema = 1\n[mcp]\npreset = \"main\"\n",
    );
    write(
        &wt.join(".pitwall/project.toml"),
        "schema = 1\n[mcp]\npreset = \"wt\"\n",
    );
    let paths = AppPaths::under(&base.join("app"));
    let registry = Registry::new(paths.clone(), None, EventBus::new(DEFAULT_CAPACITY));
    let id = registry
        .create(NewProject {
            root: main.clone(),
            ..NewProject::default()
        })
        .unwrap()
        .id;
    let get = |worktree: Option<&Path>| {
        settings_get(
            &ConnectionScope::Agent,
            &paths,
            &registry,
            &SettingsGetToolParams {
                project_id: Some(id.clone()),
                worktree: worktree.map(|w| w.display().to_string()),
                key: Some("mcp.preset".into()),
            },
        )
    };
    assert_eq!(
        value_and_layer(&get(Some(&wt)).unwrap(), "mcp.preset"),
        (json!("wt"), SettingLayer::Shared)
    );
    assert_eq!(
        value_and_layer(&get(None).unwrap(), "mcp.preset"),
        (json!("main"), SettingLayer::Shared)
    );
    let other = base.join("elsewhere");
    fs::create_dir_all(&other).unwrap();
    assert_eq!(get(Some(&other)).unwrap_err().code, code::INVALID_PARAMS);
}
