//! 설정 창 「MCP」 쪽의 RPC(16-m5-plan.md M5-15): `mcp.settings.set` 이 전역 `[mcp]` 의 그 키만 쓰고(주석 · 다른 키 보존) 값을
//! Host 가 판정하는 것, `mcp.settings.get` 이 Host 가 쓰는 값과 프로젝트가 덮은 것을 내는 것, 둘 다 에이전트 범위 연결을 거절하는
//! 것, 그룹을 끄면 열린 MCP 세션이 `tools/list_changed` 를 받고 그 그룹의 툴이 목록에서 빠지는 것.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use pw_core::config::AppPaths;
use pw_core::registry::NewProject;
use pw_core::{HostRouter, LocalHost, Target};
use pw_mcp::audit::AuditLog;
use pw_mcp::methods::{self, HostParts, Mcp};
use pw_mcp::policy::Approval;
use pw_mcp::sessions::Sessions;
use pw_mcp::{CallError, Catalog, HostClient, HostKind, Services, SessionRequest};
use pw_rpc::{Caller, Role, RpcError, rpc_method};
use rmcp::service::{MaybeSendFuture, NotificationContext, RunningService};
use rmcp::{ClientHandler, RoleClient, ServiceExt};
use serde_json::{Value, json};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

rpc_method! {
    /// 시험용: project 그룹의 조회 툴.
    pub ProbeProject: "mcpsettingstest.probeProject" {
        params: (),
        result: (),
        tier: Read,
        mcp: { group: "project", name: "probe_project" },
    }

    /// 시험용: db 그룹의 조회 툴.
    pub ProbeDb: "mcpsettingstest.probeDb" {
        params: (),
        result: (),
        tier: Read,
        mcp: { group: "db", name: "probe_db" },
    }
}

/// 사람이 손으로 쓴 전역 설정 — 주석과 `[mcp]` 밖의 키가 있다.
const GLOBAL: &str = r#"schema = 1

# 사람이 남긴 주석
[ui]
language = "ko" # 꼬리 주석

[mcp]
# 프리셋 주석
preset = "standard"

[mcp.tools]
probe_project = true
"#;

struct Fixture {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    paths: AppPaths,
    host: Arc<LocalHost>,
    sessions: Sessions,
    alpha: String,
}

impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let base = pw_core::registry::path::simplify(std::fs::canonicalize(tmp.path()).unwrap());
        let paths = AppPaths::under(&base.join("app"));
        std::fs::create_dir_all(&paths.config).unwrap();
        std::fs::write(paths.config.join("settings.toml"), GLOBAL).unwrap();
        let sessions = Sessions::new();
        let mcp = Arc::new(Mcp::new(sessions.clone()));
        let mcp_methods = Arc::clone(&mcp);
        let host = Arc::new(
            LocalHost::builder("local")
                .config(paths.clone())
                .methods(|router| {
                    let ok = |_ctx, ()| async { Ok::<(), RpcError>(()) };
                    router.unary(ProbeProject, ok);
                    router.unary(ProbeDb, ok);
                })
                .methods(move |router| methods::register(router, mcp_methods))
                .build(),
        );
        let root = base.join("work/alpha");
        std::fs::create_dir_all(root.join(".pitwall")).unwrap();
        std::fs::write(
            root.join(".pitwall/project.toml"),
            "schema = 1\n[mcp]\nauto_approve = [\"write\"]\n[mcp.tools]\nprobe_project = false\n",
        )
        .unwrap();
        std::fs::write(
            root.join(".pitwall/project.local.toml"),
            "schema = 1\n[mcp.groups]\ndb = false\n",
        )
        .unwrap();
        let alpha = host
            .registry()
            .unwrap()
            .create(NewProject {
                root,
                ..NewProject::default()
            })
            .unwrap()
            .id;
        let config = host.config().unwrap();
        sessions.attach(host.bus().clone());
        mcp.attach(HostParts {
            paths: paths.clone(),
            global: config.global.clone(),
            projects: host.registry().unwrap().clone(),
            catalog: Arc::new(Catalog::linked().routed_by(host.router())),
            audit: Arc::new(AuditLog::under_state(&paths.state)),
        });
        Self {
            _tmp: tmp,
            base,
            paths,
            host,
            sessions,
            alpha,
        }
    }

    fn settings_file(&self) -> String {
        std::fs::read_to_string(self.paths.config.join("settings.toml")).unwrap()
    }

    async fn window(&self) -> HostClient {
        self.window_as(Caller::new(Role::Owner)).await
    }

    async fn window_as(&self, caller: Caller) -> HostClient {
        let router = HostRouter::new();
        router.add_host(self.host.clone()).unwrap();
        HostClient::new(
            router
                .connect(&Target::HostId("local".into()), caller)
                .await
                .unwrap(),
        )
    }

    fn services(&self) -> Arc<Services> {
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
            approval: Approval::Unavailable,
            transport: "test",
            host_kind: HostKind::App,
            sessions: self.sessions.clone(),
        })
    }

    /// 프로젝트 없는 MCP 세션(전역 설정만으로 노출을 정한다).
    async fn connect(
        &self,
    ) -> (
        RunningService<RoleClient, Client>,
        Client,
        CancellationToken,
    ) {
        let (server_io, client_io) = tokio::io::duplex(64 * 1024);
        let (r, w) = tokio::io::split(server_io);
        let cancel = CancellationToken::new();
        tokio::spawn(pw_mcp::serve(
            self.services(),
            SessionRequest {
                project: None,
                cwd: Some(self.base.clone()),
            },
            (r, w),
            cancel.clone(),
        ));
        let handler = Client::default();
        let client = handler.clone().serve(client_io).await.unwrap();
        (client, handler, cancel)
    }
}

#[derive(Clone, Default)]
struct Client {
    list_changed: Arc<AtomicUsize>,
    notify: Arc<Notify>,
}

impl ClientHandler for Client {
    fn on_tool_list_changed(
        &self,
        _context: NotificationContext<RoleClient>,
    ) -> impl std::future::Future<Output = ()> + MaybeSendFuture + '_ {
        self.list_changed.fetch_add(1, Ordering::SeqCst);
        self.notify.notify_one();
        std::future::ready(())
    }
}

async fn set(window: &HostClient, key: &str, name: Option<&str>, value: Value) -> Value {
    window
        .call(
            "mcp.settings.set",
            json!({ "key": key, "name": name, "value": value }),
        )
        .await
        .unwrap_or_else(|e| panic!("mcp.settings.set {key} {name:?} {value}: {e:?}"))
}

/// 거절이면 그 `data.reason`.
async fn refusal(window: &HostClient, key: &str, name: Option<&str>, value: Value) -> String {
    match window
        .call(
            "mcp.settings.set",
            json!({ "key": key, "name": name, "value": value }),
        )
        .await
    {
        Err(CallError::Rpc { code, data, .. }) => {
            assert_eq!(
                code,
                i64::from(pw_rpc::code::INVALID_PARAMS),
                "{key} {value}"
            );
            let data = data.unwrap();
            assert_eq!(data["key"], key);
            data["reason"].as_str().unwrap().to_owned()
        }
        other => panic!("{key} {value}: 거절돼야 한다: {other:?}"),
    }
}

async fn tool_names(client: &RunningService<RoleClient, Client>) -> Vec<String> {
    let mut names: Vec<String> = client
        .peer()
        .list_all_tools()
        .await
        .unwrap()
        .into_iter()
        .map(|t| t.name.to_string())
        .filter(|n| n.starts_with("probe_"))
        .collect();
    names.sort();
    names
}

/// 쓰기는 그 키 하나만 고친다 — 주석 · `[mcp]` 밖의 키 · 다른 `[mcp]` 키는 그대로다. 같은 값이면 파일을 고치지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn settings_set_writes_only_that_key_and_keeps_comments_and_other_keys() {
    let f = Fixture::new();
    let window = f.window().await;

    let view = set(&window, "group", Some("db"), json!(false)).await;
    assert_eq!(
        view["groups"]
            .as_array()
            .unwrap()
            .iter()
            .find(|g| g["group"] == "db")
            .unwrap()["enabled"],
        false
    );
    set(&window, "tool", Some("probe_db"), json!(true)).await;
    set(&window, "preset", None, json!("full")).await;
    set(
        &window,
        "autoApprove",
        None,
        json!(["exec", "read", "read"]),
    )
    .await;
    let view = set(&window, "retentionDays", None, json!(30)).await;
    assert_eq!(view["preset"], "full");
    assert_eq!(view["autoApprove"], json!(["read", "exec"]));
    assert_eq!(view["retentionDays"], 30);

    let text = f.settings_file();
    for kept in [
        "# 사람이 남긴 주석",
        "language = \"ko\" # 꼬리 주석",
        "# 프리셋 주석",
        "probe_project = true",
    ] {
        assert!(text.contains(kept), "`{kept}` 가 사라졌다:\n{text}");
    }
    let parsed: toml_edit::DocumentMut = text.parse().unwrap();
    let mcp = &parsed["mcp"];
    assert_eq!(mcp["preset"].as_str(), Some("full"));
    assert_eq!(mcp["tools"]["probe_project"].as_bool(), Some(true));
    assert_eq!(mcp["tools"]["probe_db"].as_bool(), Some(true));
    assert_eq!(mcp["groups"]["db"].as_bool(), Some(false));
    assert_eq!(mcp["audit"]["retention_days"].as_integer(), Some(30));
    let tiers: Vec<&str> = mcp["auto_approve"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(tiers, ["read", "exec"]);
    assert_eq!(parsed["ui"]["language"].as_str(), Some("ko"));
    // 바꾼 키 말고는 늘지 않았다: [mcp] 아래 다섯(preset · tools · groups · audit · auto_approve)과 [ui] · schema.
    let top: Vec<&str> = parsed.iter().map(|(k, _)| k).collect();
    assert_eq!(top, ["schema", "ui", "mcp"]);
    let tools: Vec<&str> = mcp["tools"]
        .as_table_like()
        .unwrap()
        .iter()
        .map(|(k, _)| k)
        .collect();
    assert_eq!(tools, ["probe_project", "probe_db"]);

    // 같은 값은 쓰지 않는다(파일이 그대로).
    let before = f.settings_file();
    set(&window, "preset", None, json!("full")).await;
    assert_eq!(f.settings_file(), before);

    // null 은 그 키를 지운다 — 다른 키는 남는다.
    let view = set(&window, "group", Some("db"), Value::Null).await;
    assert!(
        view["groups"]
            .as_array()
            .unwrap()
            .iter()
            .all(|g| g["enabled"].is_null())
    );
    let parsed: toml_edit::DocumentMut = f.settings_file().parse().unwrap();
    assert!(parsed["mcp"]["groups"].get("db").is_none());
    assert_eq!(parsed["mcp"]["tools"]["probe_db"].as_bool(), Some(true));
}

/// 값은 Host 가 판정한다: `auto_approve` 의 `dangerous` · 모르는 프리셋 · 모르는 그룹 · 모르는 툴 · 음수 · 정수 아닌 보관 일수는
/// 쓰지 않고 거절한다. 거절 뒤 파일은 바이트까지 그대로다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn settings_set_refuses_values_the_host_would_not_use() {
    let f = Fixture::new();
    let window = f.window().await;
    let before = f.settings_file();
    for (key, name, value, reason) in [
        (
            "autoApprove",
            None,
            json!(["write", "dangerous"]),
            "dangerousAutoApprove",
        ),
        ("autoApprove", None, json!(["sometimes"]), "unknownTier"),
        ("autoApprove", None, json!("write"), "badValue"),
        ("preset", None, json!("everything"), "unknownPreset"),
        ("preset", None, json!(1), "unknownPreset"),
        ("group", Some("files"), json!(false), "unknownGroup"),
        ("group", None, json!(false), "badValue"),
        ("group", Some("db"), json!("off"), "badValue"),
        ("tool", Some("no_such_tool"), json!(true), "unknownTool"),
        ("tool", Some("probe_*"), json!(true), "unknownTool"),
        ("retentionDays", None, json!(-1), "badRetentionDays"),
        ("retentionDays", None, json!(1.5), "badRetentionDays"),
        ("retentionDays", None, json!("90"), "badRetentionDays"),
        // 상한(`MAX_RETENTION_DAYS` = 3650)을 넘는 값.
        ("retentionDays", None, json!(3651), "badRetentionDays"),
        (
            "retentionDays",
            None,
            json!(4_294_967_295_u64),
            "badRetentionDays",
        ),
    ] {
        assert_eq!(
            refusal(&window, key, name, value.clone()).await,
            reason,
            "{key} {name:?} {value}"
        );
        assert_eq!(
            f.settings_file(),
            before,
            "{key} {value}: 거절했는데 파일이 바뀌었다"
        );
    }
    // 0 은 「지우지 않음」 이라 받는다. 상한 그 값도 받는다.
    let view = set(&window, "retentionDays", None, json!(0)).await;
    assert_eq!(view["retentionDays"], 0);
    let view = set(&window, "retentionDays", None, json!(3650)).await;
    assert_eq!(view["retentionDays"], 3650);
    assert_eq!(
        view["maxRetentionDays"],
        pw_mcp::settings::MAX_RETENTION_DAYS
    );
}

/// 읽기는 Host 가 판정에 쓰는 값이다 — 잘못된 프리셋은 `read-only`, `auto_approve` 의 `dangerous` 는 빠지고 경고가 실린다.
/// `projectId` 를 주면 그 프로젝트의 파일들과 그 파일들이 덮은 키, 그 프로젝트의 자동 승인(공유 설정은 넓히지 못한다)이 실린다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn settings_get_reports_what_the_host_uses_and_what_the_project_overrides() {
    let f = Fixture::new();
    std::fs::write(
        f.paths.config.join("settings.toml"),
        "schema = 1\n[mcp]\npreset = \"bogus\"\nauto_approve = [\"read\", \"dangerous\"]\n[mcp.audit]\nretention_days = 7\n",
    )
    .unwrap();
    f.host.config().unwrap().global.reload().unwrap();
    let window = f.window().await;

    let view = window.call("mcp.settings.get", json!({})).await.unwrap();
    assert_eq!(view["preset"], "read-only");
    assert_eq!(view["autoApprove"], json!(["read"]));
    assert_eq!(view["retentionDays"], 7);
    assert_eq!(view["defaultRetentionDays"], 90);
    assert!(view["project"].is_null());
    assert_eq!(
        view["file"],
        f.paths.config.join("settings.toml").display().to_string()
    );
    let warnings = view["warnings"].to_string();
    assert!(
        warnings.contains("bogus") && warnings.contains("dangerous"),
        "{warnings}"
    );
    let groups: Vec<&str> = view["groups"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| g["group"].as_str().unwrap())
        .collect();
    assert_eq!(groups, pw_mcp::catalog::GROUPS);

    let view = window
        .call("mcp.settings.get", json!({ "projectId": f.alpha }))
        .await
        .unwrap();
    let project = &view["project"];
    assert_eq!(project["projectId"], json!(f.alpha));
    let pitwall = f.base.join("work/alpha/.pitwall");
    assert_eq!(
        project["files"],
        json!([
            {"kind": "shared", "path": pitwall.join("project.toml").display().to_string(), "exists": true},
            {"kind": "local", "path": pitwall.join("project.local.toml").display().to_string(), "exists": true},
        ])
    );
    assert_eq!(
        project["overrides"],
        json!([
            {"key": "auto_approve", "value": ["write"], "file": "shared"},
            {"key": "tools.probe_project", "value": false, "file": "shared"},
            {"key": "groups.db", "value": false, "file": "local"},
        ])
    );
    // 공유 설정의 auto_approve = ["write"] 는 넓히지 못한다 — 전역 · 개인 설정에 write 가 없다.
    assert_eq!(project["autoApprove"], json!([]));
    // 전역 값은 그대로 실린다.
    assert_eq!(view["preset"], "read-only");

    match window
        .call("mcp.settings.get", json!({ "projectId": "nope" }))
        .await
    {
        Err(CallError::Rpc { code, .. }) => assert_ne!(code, 0),
        other => panic!("없는 프로젝트: {other:?}"),
    }
}

/// `mcp.settings.*` 는 에이전트 범위 연결(MCP 세션이 Host 에 붙는 연결)을 거절한다 — role 이 owner 라도. 파일 범위 연결(단일
/// 파일 창)도 부르지 못하고, 툴이 아니다. 거절된 쓰기는 파일을 고치지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn settings_rpcs_refuse_agent_connections_and_file_windows() {
    let f = Fixture::new();
    let before = f.settings_file();
    let catalog = Catalog::linked();
    for (method, params) in [
        ("mcp.settings.get", json!({})),
        (
            "mcp.settings.set",
            json!({"key": "preset", "name": null, "value": "full"}),
        ),
    ] {
        assert!(
            catalog.tools().iter().all(|t| t.method != method),
            "{method} 는 툴이 아니다"
        );
        let agent = f.window_as(Caller::agent(Role::Owner)).await;
        match agent.call(method, params.clone()).await {
            Err(CallError::Rpc { code, data, .. }) => {
                assert_eq!(code, i64::from(pw_rpc::code::PERMISSION_DENIED), "{method}");
                let data = data.unwrap();
                assert_eq!(data["reason"], "agentSession", "{method}");
                assert_eq!(data["method"], method);
            }
            other => panic!("{method}: 에이전트 범위 연결은 부르지 못한다: {other:?}"),
        }
        let file = f
            .window_as(Caller::file(Role::Owner, f.base.join("work/alpha/a.txt")))
            .await;
        match file.call(method, params.clone()).await {
            Err(CallError::Rpc { code, .. }) => {
                assert_eq!(code, i64::from(pw_rpc::code::PERMISSION_DENIED), "{method}");
            }
            other => panic!("{method}: 파일 범위 연결은 부르지 못한다: {other:?}"),
        }
    }
    assert_eq!(f.settings_file(), before, "거절된 쓰기가 파일을 고쳤다");
    // 창(owner)은 부른다 — 막는 것은 범위다.
    let window = f.window().await;
    window.call("mcp.settings.get", json!({})).await.unwrap();
    set(&window, "preset", None, json!("full")).await;
}

/// 시나리오 8 의 Rust 쪽: 열린 MCP 세션은 설정 창이 그룹을 끈 뒤 `tools/list_changed` 를 받고, `tools/list` 에서 그 그룹의 툴이
/// 곧바로 빠진다(파일 감시 없이 — 쓰기가 전역 설정을 다시 읽는다). 다시 켜면 돌아온다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn turning_a_group_off_notifies_open_sessions_and_hides_its_tools() {
    let f = Fixture::new();
    let window = f.window().await;
    let (client, handler, cancel) = f.connect().await;
    assert_eq!(tool_names(&client).await, ["probe_db", "probe_project"]);

    set(&window, "group", Some("db"), json!(false)).await;
    tokio::time::timeout(Duration::from_secs(10), handler.notify.notified())
        .await
        .expect("tools/list_changed 가 오지 않았다");
    assert_eq!(tool_names(&client).await, ["probe_project"]);
    let list = window.call("mcp.tools.list", json!({})).await.unwrap();
    let db = list
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "probe_db")
        .unwrap();
    assert_eq!(db["enabled"], false);
    assert_eq!(db["rule"], "group");
    assert_eq!(db["key"], "db");

    set(&window, "group", Some("db"), Value::Null).await;
    tokio::time::timeout(Duration::from_secs(10), handler.notify.notified())
        .await
        .expect("다시 켠 뒤 tools/list_changed 가 오지 않았다");
    assert_eq!(tool_names(&client).await, ["probe_db", "probe_project"]);
    assert_eq!(handler.list_changed.load(Ordering::SeqCst), 2);

    client.cancel().await.unwrap();
    cancel.cancel();
}

fn verdict_of(list: &Value, name: &str) -> (bool, String, Value) {
    let tool = list
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == name)
        .unwrap_or_else(|| panic!("{name} 이 목록에 없다"));
    (
        tool["enabled"].as_bool().unwrap(),
        tool["rule"].as_str().unwrap().to_owned(),
        tool["key"].clone(),
    )
}

/// 전역의 툴 키는 프로젝트가 그룹 · 프리셋 · glob 으로 바꾼 판정에도 닿는다(정확한 툴 키가 그보다 앞선다 — 리뷰어 탐침,
/// 16-m5-plan M5-15 리뷰). 그래서 화면은 프로젝트가 판정을 바꾼 툴의 전역 토글을 잠그지 않고 그 프로젝트에서의 판정을 나란히
/// 보인다. 반대로 공유 설정이 끈 툴은 전역 키로 켜지지 않는다(까닭 `shared`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_global_tool_key_reaches_a_tool_the_project_turned_off_by_group() {
    let f = Fixture::new();
    let window = f.window().await;
    let project = json!({ "projectId": f.alpha });

    // 프로젝트 local 의 `groups.db = false` 가 probe_db 를 끈다.
    let list = window
        .call("mcp.tools.list", project.clone())
        .await
        .unwrap();
    assert_eq!(
        verdict_of(&list, "probe_db"),
        (false, "group".into(), json!("db"))
    );

    // 전역에 그 툴의 키를 쓰면 그 프로젝트에서도 켜진다.
    set(&window, "tool", Some("probe_db"), json!(true)).await;
    let list = window
        .call("mcp.tools.list", project.clone())
        .await
        .unwrap();
    assert_eq!(
        verdict_of(&list, "probe_db"),
        (true, "tool".into(), json!("probe_db"))
    );

    // 커밋되는 공유 설정이 끈 툴(`probe_project = false`)은 전역 키로 켜지지 않는다 — 공유 설정은 좁히기만 하고 그 까닭이
    // `shared` 다. 전역은 이미 `probe_project = true` 이고 다시 써도 그대로다.
    let list = window
        .call("mcp.tools.list", project.clone())
        .await
        .unwrap();
    assert_eq!(
        verdict_of(&list, "probe_project"),
        (false, "shared".into(), Value::Null)
    );
    set(&window, "tool", Some("probe_project"), json!(true)).await;
    let list = window.call("mcp.tools.list", project).await.unwrap();
    assert_eq!(
        verdict_of(&list, "probe_project"),
        (false, "shared".into(), Value::Null)
    );
}

/// 재확인 리뷰 권고 2 ②: 프로젝트 설정 파일을 읽지 못하면 사람의 설정 창은 그 프로젝트의 자동 승인을 「없음」 으로 보이고(에이전트
/// 호출이 거절되므로), 읽지 못한 파일과 까닭을 경고에 싣는다 — 전역의 자동 승인을 보이면 실패.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_settings_window_shows_no_auto_approval_for_an_unreadable_project() {
    let f = Fixture::new();
    std::fs::write(
        f.paths.config.join("settings.toml"),
        "schema = 1\n[mcp]\nauto_approve = [\"read\", \"write\"]\n",
    )
    .unwrap();
    f.host.config().unwrap().global.reload().unwrap();
    let window = f.window().await;
    let view = window
        .call("mcp.settings.get", json!({ "projectId": f.alpha }))
        .await
        .unwrap();
    // 공유 `project.toml` 의 `auto_approve = ["write"]` 가 좁힌다.
    assert_eq!(view["project"]["autoApprove"], json!(["write"]));

    let broken = f.base.join("work/alpha/.pitwall/db/y.toml");
    std::fs::create_dir_all(broken.parent().unwrap()).unwrap();
    std::fs::write(&broken, "name = \"y\n").unwrap();
    let view = window
        .call("mcp.settings.get", json!({ "projectId": f.alpha }))
        .await
        .unwrap();
    let project = &view["project"];
    assert_eq!(project["autoApprove"], json!([]));
    let warnings = project["warnings"].to_string();
    assert!(
        warnings.contains("y.toml") && warnings.contains("refused"),
        "{warnings}"
    );
}
