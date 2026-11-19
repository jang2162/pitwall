//! MCP 툴 목록 시험과 툴 왕복 도우미(`docs/design/16-m5-plan.md` M5-01 ④). 모든 메서드 크레이트가 링크되는 자리라 어느 크레이트의
//! 선언에 `mcp` 를 달아도 여기서 잡힌다.
//!
//! 툴 행이 하는 것:
//! - [`APP_TOOLS`] 에 자기 툴 이름을 이름순 자리에 더한다. 코어 툴(헤드리스 Host 도 처리하는 것)이면 [`HEADLESS_TOOLS`] 에도.
//!   병합 충돌은 양쪽 줄을 다 살린다.
//! - [`ToolHost`] 로 앱과 같은 Host 에 진짜 MCP 세션을 열고 `tools/list` · `tools/call` 로 이름 · 그룹 · 등급 · 스키마 · 감사 행을 본다.
//!   도우미는 세션을 여는 공개 길(`pw_mcp::serve` · `mcp::services`)만 쓴다 — 세션의 안쪽 꼴(연결 범위 · 호출 길)에 기대지 않는다.
//!
//! 헤드리스 목록 단언은 앱 전용 툴(git — M5-05, DB — M5-06)이 생긴 뒤로 `mcp::services` 의 `routed_by` 를 가른다 — 빼면 헤드리스
//! 목록에 앱 전용 툴이 섞여 [`mcp_tools_are_exactly_these`] 가 실패한다(M5-05 · M5-06 이 각각 빼 보고 확인했다).
//!
//! 툴 행의 왕복 시험은 하위 모듈에 둔다(`git` · `db` · `code` · `project` · `project_write` · `run` — 이 파일의 병합 충돌을 줄이려고).
//! 계획을 내는 툴은 `plan_tiers` 의 회귀 표에도 더한다(17-m6-plan M6-10 ⑨).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use pw_core::LocalHost;
use pw_core::config::AppPaths;
use pw_core::registry::NewProject;
use pw_mcp::{Services, SessionRequest};
use rmcp::model::{CallToolRequestParams, CallToolResult};
use rmcp::service::RunningService;
use rmcp::{RoleClient, ServiceExt};
use serde_json::{Map, Value};
use tokio_util::sync::CancellationToken;

use crate::mcp;

mod code;
mod db;
mod debug;
mod git;
mod plan_tiers;
mod project;
mod project_write;
mod run;
mod run_chain;
mod run_exec;
mod run_read;

/// 앱 Host 가 내는 툴 전부. 이름순.
const APP_TOOLS: &[&str] = &[
    "breakpoint_list",
    "breakpoint_set",
    "breakpoint_set_expr",
    "db_connection_list",
    "db_connection_test",
    "db_connection_upsert",
    "db_execute",
    "db_kv_get",
    "db_kv_scan",
    "db_query",
    "db_schema",
    "debug_state",
    "diagnostics",
    "git_blame",
    "git_branch_list",
    "git_diff",
    "git_log",
    "git_status",
    "open_with_list",
    "open_with_upsert",
    "project_info",
    "project_list",
    "project_open",
    "project_register",
    "run_config_delete",
    "run_config_list",
    "run_config_upsert",
    "run_logs",
    "run_restart",
    "run_start",
    "run_status",
    "run_stop",
    "settings_get",
    "symbol_search",
];

/// 헤드리스 Host(앱 없이 뜬 `pitwall mcp` · `pitwall tool`)가 내는 툴 — 코어 툴만(결정 M5-D3). [`APP_TOOLS`] 의 부분집합. 이름순.
const HEADLESS_TOOLS: &[&str] = &[
    "project_info",
    "project_list",
    "project_register",
    "settings_get",
];

/// 프로젝트 툴 가운데 선택 입력 `worktree` 가 없어도 되는 것 — 체크아웃을 보지 않고 프로젝트 레코드만 읽는 툴. 받기만 하고 쓰지 않는
/// 입력은 에이전트를 속이므로 달지 않는다(16-m5-plan 결정 M5-D15 의 예외, 메인 판정 2026-11-13). 툴마다 까닭을 적는다.
const PROJECT_RECORD_TOOLS: &[&str] = &[
    // 레지스트리의 레코드(루트 · 이름)와 루트의 빌드 파일 · 설정 폴더만 본다 — 워크트리마다 다르지 않다.
    "project_info",
    // 실행은 Host 범위이고 프로젝트에 속한다(체크아웃이 아니다 — 17-m6-plan M6-P1). 실행마다 띄운 워크트리 · 작업 폴더를 싣는다.
    // `worktree` 로 거르면 세션 워크트리가 채워져 주 체크아웃에서 띄운 실행이 말없이 빠진다.
    "run_logs",
    // `runId` 로 겨눈다 — 다시 띄우기는 그 실행이 띄운 워크트리를 그대로 쓴다(17-m6-plan M6-14 ⑦).
    "run_restart",
    "run_status",
    "run_stop",
    // 디버그 세션도 실행이다(17-m6-plan M6-P15) — `run_status` 와 같은 까닭. 세션은 그 프로젝트의 것을 모두 본다(M6-20).
    "debug_state",
];

fn names(catalog: &pw_mcp::Catalog) -> Vec<&str> {
    catalog.tools().iter().map(|t| t.name.as_str()).collect()
}

/// MCP 툴이 되는 선언은 정확히 [`APP_TOOLS`] 이고, 앱 Host 는 그 전부를, 헤드리스 Host 는 [`HEADLESS_TOOLS`] 만 처리한다.
/// 헤드리스 목록에 앱 전용 툴을 넣으면 실패한다 — 헤드리스가 목록에 내고 부르면 method-not-found 인 툴이 없게.
#[test]
fn mcp_tools_are_exactly_these() {
    assert!(
        HEADLESS_TOOLS.iter().all(|t| APP_TOOLS.contains(t)),
        "헤드리스 목록은 앱 목록의 부분집합이다"
    );
    let linked = pw_mcp::Catalog::linked();
    assert_eq!(names(&linked), APP_TOOLS, "`mcp` 선언이 바뀌었다");

    let host = ToolHost::app();
    assert_eq!(
        names(&host.services.catalog),
        APP_TOOLS,
        "앱 Host 가 처리하지 않는 툴이 있다"
    );
    let headless = mcp::headless_host(&host.paths);
    assert_eq!(
        names(
            &mcp::services(
                &headless,
                &host.paths,
                "test",
                pw_mcp::HostKind::Headless,
                Default::default()
            )
            .catalog
        ),
        HEADLESS_TOOLS,
        "헤드리스 Host 가 처리하는 툴이 목록과 다르다"
    );
}

/// 앱 세션은 승인 경로를 받는다 — 앱의 `mcp.approvals` 를 듣는 창과 같은 허브(넘긴 세션 등록부의 것)다. 헤드리스는 받지 않는다
/// (16-m5-plan M5-12 ① · ⑦, `mcp::services`).
#[test]
fn app_sessions_get_the_windows_approval_route_and_headless_ones_none() {
    use pw_mcp::policy::Approval;
    let host = ToolHost::app();
    let sessions = pw_mcp::sessions::Sessions::new();
    let app = mcp::services(
        &host.host,
        &host.paths,
        "test",
        pw_mcp::HostKind::App,
        sessions.clone(),
    );
    let Approval::Route(route) = &app.approval else {
        panic!("the app session has no approval route: {:?}", app.approval)
    };
    let _window =
        sessions
            .approvals()
            .listen(Some("p"), None, pw_mcp::approval::Connection::named("w"));
    assert_eq!(route.listeners(), 1, "창이 듣는 허브와 다르다");
    let headless = mcp::headless_host(&host.paths);
    let services = mcp::services(
        &headless,
        &host.paths,
        "test",
        pw_mcp::HostKind::Headless,
        sessions,
    );
    assert!(matches!(services.approval, Approval::Unavailable));
}

/// 툴 메서드 규약(`crates/pw-mcp` 머리 주석)을 어긴 곳. 비면 규약대로다.
///
/// - 프로젝트 툴(입력에 `projectId`)의 입력이 struct 인 것(뒤에 선택 입력 `checkout` 을 깨지 않고 더한다, 결정 M5-D15)은 여기서
///   보지 않는다 — 카탈로그가 루트가 object 가 아닌 입력을 거절하고(`CatalogError::InputNotObject`), `()` 입력은 `projectId` 가 없어
///   프로젝트 툴이 될 수 없다(`Tool::project_scoped`).
/// - 프로젝트 툴은 선택 입력 `worktree`(문자열)를 받는다. `exempt`(실제 툴은 [`PROJECT_RECORD_TOOLS`])만 뺀다. 예외 목록의 툴이
///   `worktree` 를 갖거나 프로젝트 툴이 아니면 예외가 낡은 것이다.
/// - 쪽 단위 툴: 입력에 `limit` 이나 `cursor` 가 있으면 둘 다 있고 둘 다 선택이며, 결과가 object 이고 다음 쪽을 알리는 `nextCursor`
///   를 싣는다(없으면 마지막 쪽 — 저장소의 쪽 RPC 와 같은 이름). `limit` 은 `u32` 다 — 기본값 `pw_rpc::DEFAULT_LIST_LIMIT` 와 같은
///   타입. 어느 툴이 쪽 단위인지는 툴 행이 정한다 — 이 검사는 반쪽만 구현된 쪽 규약을 잡는다.
fn convention_problems(tool: &pw_mcp::Tool, exempt: &[&str]) -> Vec<String> {
    // `limit` 의 타입이 기본값 상수와 같다는 것을 컴파일 때 묶는다 — 상수 타입이 바뀌면 여기와 아래 `uint32` 검사를 함께 고친다.
    const _: u32 = pw_rpc::DEFAULT_LIST_LIMIT;
    let mut problems = Vec::new();
    let schema = tool.input_schema_for(false);
    let properties = schema.get("properties").and_then(Value::as_object);
    let has = |key: &str| properties.is_some_and(|p| p.contains_key(key));
    let required = |key: &str| {
        schema
            .get("required")
            .and_then(Value::as_array)
            .is_some_and(|r| r.iter().any(|k| k == key))
    };
    if tool.project_scoped() {
        let exempt = exempt.contains(&tool.name.as_str());
        match properties.and_then(|p| p.get("worktree")) {
            None if !exempt => {
                problems.push("project tool needs an optional `worktree`".to_owned())
            }
            None => {}
            Some(_) if exempt => {
                problems.push("has `worktree`; drop it from PROJECT_RECORD_TOOLS".to_owned())
            }
            Some(w) => {
                if !accepts_string(w) {
                    problems.push("`worktree` must be a string".to_owned());
                }
                if required("worktree") {
                    problems.push("`worktree` must be optional".to_owned());
                }
            }
        }
    } else if exempt.contains(&tool.name.as_str()) {
        problems.push("not a project tool; drop it from PROJECT_RECORD_TOOLS".to_owned());
    }
    if has("limit") || has("cursor") {
        for key in ["limit", "cursor"] {
            if !has(key) {
                problems.push(format!("paged tool needs `{key}` too"));
            } else if required(key) {
                problems.push(format!("`{key}` must be optional"));
            }
        }
        let limit_format = properties
            .and_then(|p| p.get("limit"))
            .and_then(|l| l.get("format"))
            .and_then(Value::as_str);
        if has("limit") && limit_format != Some("uint32") {
            problems.push("`limit` must be a u32 like DEFAULT_LIST_LIMIT".to_owned());
        }
        let next = tool
            .output_schema
            .as_ref()
            .and_then(|o| o.get("properties"))
            .and_then(Value::as_object)
            .is_some_and(|p| p.contains_key("nextCursor"));
        if !next {
            problems.push("paged tool result needs `nextCursor`".to_owned());
        }
    }
    problems
}

/// `{"type": "string"}` 또는 `{"type": ["string", "null"]}`.
fn accepts_string(schema: &Value) -> bool {
    match schema.get("type") {
        Some(Value::String(t)) => t == "string",
        Some(Value::Array(types)) => types.iter().any(|t| t == "string"),
        _ => false,
    }
}

#[test]
fn tools_follow_the_tool_method_convention() {
    let host = ToolHost::app();
    let problems: Vec<(String, Vec<String>)> = host
        .services
        .catalog
        .tools()
        .iter()
        .map(|t| (t.name.clone(), convention_problems(t, PROJECT_RECORD_TOOLS)))
        .filter(|(_, p)| !p.is_empty())
        .collect();
    assert!(problems.is_empty(), "{problems:#?}");
    for name in PROJECT_RECORD_TOOLS {
        assert!(
            host.services.catalog.get(name).is_some(),
            "{name} 은 툴이 아니다"
        );
    }
}

/// 규약 검사 자체를 가짜 툴로 본다 — 지금 실제 툴에는 쪽 단위 툴 · `worktree` 를 받는 프로젝트 툴이 없다.
#[test]
fn the_convention_check_catches_half_done_tools() {
    let catalog = pw_mcp::Catalog::from_methods(&[
        pw_rpc::MethodInfo::unary::<fake::Good>(),
        pw_rpc::MethodInfo::unary::<fake::NoWorktree>(),
        pw_rpc::MethodInfo::unary::<fake::RequiredWorktree>(),
        pw_rpc::MethodInfo::unary::<fake::LimitOnly>(),
        pw_rpc::MethodInfo::unary::<fake::NoNextCursor>(),
        pw_rpc::MethodInfo::unary::<fake::NumberWorktree>(),
        pw_rpc::MethodInfo::unary::<fake::WideLimit>(),
        pw_rpc::MethodInfo::unary::<fake::NotProject>(),
    ])
    .unwrap();
    let problems = |name: &str| convention_problems(catalog.get(name).unwrap(), &[]);
    assert_eq!(problems("good"), Vec::<String>::new());
    assert_eq!(
        problems("no_worktree"),
        ["project tool needs an optional `worktree`"]
    );
    assert_eq!(
        problems("required_worktree"),
        ["`worktree` must be optional"]
    );
    assert_eq!(problems("limit_only"), ["paged tool needs `cursor` too"]);
    assert_eq!(
        problems("no_next_cursor"),
        ["paged tool result needs `nextCursor`"]
    );
    assert_eq!(problems("number_worktree"), ["`worktree` must be a string"]);
    assert_eq!(
        problems("wide_limit"),
        ["`limit` must be a u32 like DEFAULT_LIST_LIMIT"]
    );
    assert_eq!(problems("not_project"), Vec::<String>::new());

    // 예외 목록이 낡았을 때: 예외인데 `worktree` 가 있다 · 예외인데 프로젝트 툴이 아니다. 예외가 맞으면 문제가 없다.
    let exempt = |name: &str| convention_problems(catalog.get(name).unwrap(), &[name]);
    assert_eq!(
        exempt("good"),
        ["has `worktree`; drop it from PROJECT_RECORD_TOOLS"]
    );
    assert_eq!(
        exempt("not_project"),
        ["not a project tool; drop it from PROJECT_RECORD_TOOLS"]
    );
    assert_eq!(exempt("no_worktree"), Vec::<String>::new());
}

/// CLI(`pitwall tool`, `tool_cli.rs`)와 겹치는 곳. 비면 겹치지 않는다.
///
/// - 입력의 최상위 필드가 예약 플래그(`tool_cli::RESERVED_FLAGS` — `--project` · `--input` · `--help`)가 된다(16-m5-plan 결정 M5-D12 ⓐ).
/// - 두 필드가 같은 플래그가 된다(`maxCount` · `max_count` → `--max-count`).
/// - 툴 이름이 명령 `pitwall tool list` 와 같다.
fn cli_flag_problems(tool: &pw_mcp::Tool) -> Vec<String> {
    let mut problems = Vec::new();
    if tool.name == crate::tool_cli::LIST {
        problems.push("the tool name is the `pitwall tool list` command".to_owned());
    }
    let mut seen = std::collections::BTreeMap::new();
    for flag in crate::tool_cli::flags(&tool.input_schema_for(false)) {
        if crate::tool_cli::RESERVED_FLAGS.contains(&flag.name.as_str()) {
            problems.push(format!(
                "field `{}` becomes the reserved flag --{}",
                flag.field, flag.name
            ));
        }
        if let Some(other) = seen.insert(flag.name.clone(), flag.field.clone()) {
            problems.push(format!(
                "fields `{other}` and `{}` both become --{}",
                flag.field, flag.name
            ));
        }
    }
    problems
}

#[test]
fn tool_inputs_leave_the_cli_flags_free() {
    let host = ToolHost::app();
    let problems: Vec<(String, Vec<String>)> = host
        .services
        .catalog
        .tools()
        .iter()
        .map(|t| (t.name.clone(), cli_flag_problems(t)))
        .filter(|(_, p)| !p.is_empty())
        .collect();
    assert!(problems.is_empty(), "{problems:#?}");
}

/// 예약 플래그 검사 자체를 가짜 툴로 본다 — 지금 실제 툴에는 겹치는 필드가 없다.
#[test]
fn the_cli_flag_check_catches_reserved_fields() {
    let catalog = pw_mcp::Catalog::from_methods(&[
        pw_rpc::MethodInfo::unary::<fake::Good>(),
        pw_rpc::MethodInfo::unary::<fake::ProjectField>(),
        pw_rpc::MethodInfo::unary::<fake::InputField>(),
        pw_rpc::MethodInfo::unary::<fake::HelpField>(),
        pw_rpc::MethodInfo::unary::<fake::TwoSpellings>(),
        pw_rpc::MethodInfo::unary::<fake::List>(),
    ])
    .unwrap();
    let problems = |name: &str| cli_flag_problems(catalog.get(name).unwrap());
    assert_eq!(problems("good"), Vec::<String>::new());
    assert_eq!(
        problems("project_field"),
        ["field `project` becomes the reserved flag --project"]
    );
    assert_eq!(
        problems("input_field"),
        ["field `input` becomes the reserved flag --input"]
    );
    assert_eq!(
        problems("help_field"),
        ["field `help` becomes the reserved flag --help"]
    );
    assert_eq!(
        problems("two_spellings"),
        ["fields `maxCount` and `max_count` both become --max-count"]
    );
    assert_eq!(
        problems("list"),
        ["the tool name is the `pitwall tool list` command"]
    );
}

/// 한글이 든 글자(음절 · 자모 · 호환 자모).
fn has_hangul(text: &str) -> bool {
    text.chars().any(|c| {
        matches!(c, '\u{AC00}'..='\u{D7A3}' | '\u{1100}'..='\u{11FF}' | '\u{3130}'..='\u{318F}')
    })
}

/// 툴이 에이전트에게 보이는 글(설명 · 입력 스키마 · 출력 스키마의 모든 글자열과 키) 가운데 한글이 든 것. 비면 영어로만 쓰였다
/// (05-mcp.md 「툴 레지스트리」 — 에이전트가 읽는 글은 영어다, 16-m5-plan 결정 M5-D39).
fn hangul_problems(tool: &pw_mcp::Tool) -> Vec<String> {
    fn walk(at: &str, value: &Value, out: &mut Vec<String>) {
        match value {
            Value::String(s) if has_hangul(s) => out.push(format!("{at}: {s}")),
            Value::Array(items) => {
                for (i, item) in items.iter().enumerate() {
                    walk(&format!("{at}[{i}]"), item, out);
                }
            }
            Value::Object(map) => {
                for (key, item) in map {
                    if has_hangul(key) {
                        out.push(format!("{at}: key {key}"));
                    }
                    walk(&format!("{at}.{key}"), item, out);
                }
            }
            _ => {}
        }
    }
    let mut problems = Vec::new();
    if has_hangul(&tool.description) {
        problems.push(format!("description: {}", tool.description));
    }
    walk(
        "input",
        &Value::Object(tool.input_schema_for(false)),
        &mut problems,
    );
    if let Some(output) = &tool.output_schema {
        walk(
            "output",
            &Value::Object(output.as_ref().clone()),
            &mut problems,
        );
    }
    problems
}

/// 툴 설명 · 입력 · 출력 스키마는 영어다(결정 M5-D39). 한글이 들면 어느 툴의 어느 자리인지 보인다 — 고칠 곳은 그 `rpc_method!`
/// 선언의 문서 주석이나 입출력 타입 필드의 문서 주석이다(스키마의 `description` 이 거기서 온다).
#[test]
fn tool_descriptions_and_schemas_are_english() {
    let host = ToolHost::app();
    let problems: Vec<(String, Vec<String>)> = host
        .services
        .catalog
        .tools()
        .iter()
        .map(|t| (t.name.clone(), hangul_problems(t)))
        .filter(|(_, p)| !p.is_empty())
        .collect();
    assert!(problems.is_empty(), "{problems:#?}");
}

/// 한글 검사 자체를 가짜 툴로 본다 — 설명 · 입력 필드 설명 · 출력 필드 설명의 한글을 각각 잡는다.
#[test]
fn the_hangul_check_catches_korean_text() {
    let catalog = pw_mcp::Catalog::from_methods(&[
        pw_rpc::MethodInfo::unary::<fake::Good>(),
        pw_rpc::MethodInfo::unary::<fake::Korean>(),
    ])
    .unwrap();
    assert_eq!(
        hangul_problems(catalog.get("good").unwrap()),
        Vec::<String>::new()
    );
    assert_eq!(
        hangul_problems(catalog.get("korean").unwrap()),
        [
            "description: 한글 설명",
            "input.properties.query.description: 찾을 글",
            "output.properties.found.description: 찾은 것",
        ]
    );
}

/// [`the_convention_check_catches_half_done_tools`] · [`the_cli_flag_check_catches_reserved_fields`] 의 가짜 선언. `rpc_method!` 로 만들면 레지스트리(`pw_rpc::methods`)에 들어가
/// 툴 목록 시험에 섞이므로 손으로 만든다.
#[allow(dead_code)] // 스키마만 본다 — 값을 읽지 않는다.
mod fake {
    use pw_rpc::{McpTool, Method, MethodKind, MethodMeta, RoleSet, Tier, UnaryMethod};
    use serde::{Deserialize, Serialize};

    #[derive(Deserialize, specta::Type, schemars::JsonSchema)]
    #[serde(rename_all = "camelCase")]
    pub struct Paged {
        pub project_id: String,
        #[serde(default)]
        pub worktree: Option<String>,
        #[serde(default)]
        pub cursor: Option<String>,
        #[serde(default)]
        pub limit: Option<u32>,
    }

    #[derive(Deserialize, specta::Type, schemars::JsonSchema)]
    #[serde(rename_all = "camelCase")]
    pub struct ProjectOnly {
        pub project_id: String,
    }

    #[derive(Deserialize, specta::Type, schemars::JsonSchema)]
    #[serde(rename_all = "camelCase")]
    pub struct WorktreeRequired {
        pub project_id: String,
        pub worktree: String,
    }

    #[derive(Deserialize, specta::Type, schemars::JsonSchema)]
    #[serde(rename_all = "camelCase")]
    pub struct WorktreeNumber {
        pub project_id: String,
        #[serde(default)]
        pub worktree: Option<u32>,
    }

    #[derive(Deserialize, specta::Type, schemars::JsonSchema)]
    pub struct Wide {
        #[serde(default)]
        pub cursor: Option<String>,
        #[serde(default)]
        pub limit: Option<u64>,
    }

    #[derive(Deserialize, specta::Type, schemars::JsonSchema)]
    pub struct Plain {
        pub query: String,
    }

    #[derive(Deserialize, specta::Type, schemars::JsonSchema)]
    pub struct OnlyLimit {
        #[serde(default)]
        pub limit: Option<u32>,
    }

    #[derive(Deserialize, specta::Type, schemars::JsonSchema)]
    pub struct WithProject {
        pub project: String,
    }

    #[derive(Deserialize, specta::Type, schemars::JsonSchema)]
    pub struct WithInput {
        pub input: String,
    }

    #[derive(Deserialize, specta::Type, schemars::JsonSchema)]
    pub struct WithHelp {
        #[serde(default)]
        pub help: Option<bool>,
    }

    #[derive(Deserialize, specta::Type, schemars::JsonSchema)]
    pub struct Spellings {
        #[serde(rename = "maxCount")]
        pub a: u32,
        #[serde(rename = "max_count")]
        pub b: u32,
    }

    #[derive(Serialize, specta::Type, schemars::JsonSchema)]
    #[serde(rename_all = "camelCase")]
    pub struct Page {
        pub items: Vec<String>,
        pub next_cursor: Option<String>,
    }

    // 한글 검사의 가짜 입력 · 결과.
    #[derive(Deserialize, specta::Type, schemars::JsonSchema)]
    pub struct KoreanQuery {
        /// 찾을 글
        pub query: String,
    }

    #[derive(Serialize, specta::Type, schemars::JsonSchema)]
    pub struct KoreanFound {
        /// 찾은 것
        pub found: bool,
    }

    macro_rules! fake {
        ($ty:ident, $name:literal, $params:ty, $result:ty) => {
            fake!($ty, $name, $params, $result, "");
        };
        ($ty:ident, $name:literal, $params:ty, $result:ty, $doc:literal) => {
            #[derive(Debug, Clone, Copy)]
            pub struct $ty;
            impl Method for $ty {
                type Params = $params;
                const META: MethodMeta = MethodMeta {
                    name: concat!("code.", $name),
                    kind: MethodKind::Unary,
                    tier: Tier::Read,
                    roles: RoleSet::default_for(Tier::Read),
                    params: "",
                    output: "",
                    doc: $doc,
                    mcp: Some(McpTool {
                        group: Some("code"),
                        name: Some($name),
                    }),
                    file_scope: false,
                };
            }
            impl UnaryMethod for $ty {
                type Result = $result;
            }
        };
    }

    fake!(Good, "good", Paged, Page);
    fake!(NoWorktree, "no_worktree", ProjectOnly, ());
    fake!(RequiredWorktree, "required_worktree", WorktreeRequired, ());
    fake!(LimitOnly, "limit_only", OnlyLimit, Page);
    fake!(NoNextCursor, "no_next_cursor", Paged, Vec<String>);
    fake!(NumberWorktree, "number_worktree", WorktreeNumber, ());
    fake!(WideLimit, "wide_limit", Wide, Page);
    fake!(NotProject, "not_project", Plain, ());
    fake!(ProjectField, "project_field", WithProject, ());
    fake!(InputField, "input_field", WithInput, ());
    fake!(HelpField, "help_field", WithHelp, ());
    fake!(TwoSpellings, "two_spellings", Spellings, ());
    fake!(List, "list", Plain, ());
    fake!(Korean, "korean", KoreanQuery, KoreanFound, "한글 설명");
}

/// 설정이 없을 때(프리셋 `standard` — `read` · `write`) 앱 세션의 `tools/list`: [`APP_TOOLS`] 에서 `exec` · `dangerous` 를 뺀 것
/// (`db_execute` — 16-m5-plan M5-18 — 는 `full` 이나 `[mcp.tools] db_execute = true` 에서만 보인다).
fn standard_tools(host: &ToolHost) -> Vec<&'static str> {
    APP_TOOLS
        .iter()
        .copied()
        .filter(|t| matches!(host.tool(t).tier, pw_rpc::Tier::Read | pw_rpc::Tier::Write))
        .collect()
}

/// 도우미 자체: 진짜 세션에서 목록 · 그룹 · 등급 · 호출 · 감사 행이 보인다. 툴 행의 시험은 이 꼴을 따른다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_tool_round_trip_helper_lists_calls_and_reads_the_audit_log() {
    let host = ToolHost::app();
    let alpha = host.project("alpha");
    let session = host.session(SessionRequest::default()).await;

    let listed = session.list().await;
    let names: Vec<_> = listed.iter().map(|t| t.name.to_string()).collect();
    assert_eq!(names, standard_tools(&host));
    let info = host.tool("project_info");
    assert_eq!((info.group, info.tier), ("project", pw_rpc::Tier::Read));
    let schema = &listed
        .iter()
        .find(|t| t.name == "project_info")
        .unwrap()
        .input_schema;
    assert!(
        schema["properties"].get("projectId").is_some(),
        "{schema:?}"
    );

    let result = session
        .call("project_info", serde_json::json!({ "projectId": alpha }))
        .await;
    assert_ne!(result.is_error, Some(true), "{result:?}");
    let rows = host.audit_rows(1);
    assert_eq!(rows[0].tool, "project_info");
    assert_eq!(rows[0].outcome, "ok");
    assert_eq!(rows[0].project_id.as_deref(), Some(alpha.as_str()));
    session.close().await;
}

/// 앱 Host 에서도 그룹을 끄면 그 그룹의 툴이 목록에서 빠지고, 부르면 거절이 감사 행에 남는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_group_turned_off_in_the_app_hides_its_tools() {
    let host = ToolHost::app();
    host.global_settings("[mcp.groups]\nproject = false\n");
    let session = host.session(SessionRequest::default()).await;
    let listed: Vec<String> = session
        .list()
        .await
        .iter()
        .map(|t| t.name.to_string())
        .collect();
    let others: Vec<&str> = standard_tools(&host)
        .into_iter()
        .filter(|t| host.tool(t).group != "project")
        .collect();
    assert_eq!(listed, others, "다른 그룹의 툴만 남는다");
    assert!(session.try_call("project_list", Value::Null).await.is_err());
    let rows = host.audit_rows(1);
    assert_eq!(
        (rows[0].tool.as_str(), rows[0].outcome.as_str()),
        ("project_list", "denied")
    );
    session.close().await;
}

/// 앱과 같은 Host(`crate::local_host`)를 임시 폴더에 만든 것. 세션은 [`Self::session`] 으로 연다.
pub(crate) struct ToolHost {
    _tmp: tempfile::TempDir,
    /// 임시 폴더(정규화한 실제 경로). 프로젝트 루트는 그 아래 `work/<이름>`.
    pub base: PathBuf,
    pub paths: AppPaths,
    pub host: Arc<LocalHost>,
    /// 앱이 소켓 세션에 주는 것과 같은 규칙으로 만든 것(`mcp::services`).
    pub services: Arc<Services>,
    /// 그 Host 의 DB 세션 — 시험이 실행 길에 끼워 넣는 자리(`test_hooks`)를 쓴다.
    pub db: Arc<pw_db::DbSessions>,
}

impl ToolHost {
    pub fn app() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        // macOS 의 /var → /private/var 처럼 레지스트리가 저장하는 꼴로 맞춘다.
        let base = pw_core::registry::path::simplify(std::fs::canonicalize(tmp.path()).unwrap());
        let paths = AppPaths::under(&base.join("app"));
        // 앱처럼 MCP 세션과 `mcp.*` RPC 가 같은 등록부(승인 허브)를 쓴다.
        let sessions = pw_mcp::sessions::Sessions::default();
        let (host, db) = crate::local_host_and_db(
            paths.clone(),
            Arc::new(pw_diag::Diagnostics::new(base.join("logs"), "0.0.0")),
            Arc::new(pw_ext::Registry::default()),
            Arc::new(pw_i18n::I18n::new(pw_i18n::Catalog::default(), Vec::new())),
            Arc::new(pw_fs::Fs::new()),
            pw_git::write::Askpass::default(),
            sessions.clone(),
        );
        let host = Arc::new(host);
        let services = mcp::services(&host, &paths, "test", pw_mcp::HostKind::App, sessions);
        Self {
            _tmp: tmp,
            base,
            paths,
            host,
            services,
            db,
        }
    }

    /// `work/<name>` 폴더를 만들어 프로젝트로 등록하고 그 id 를 돌려준다.
    pub fn project(&self, name: &str) -> String {
        let root = self.base.join("work").join(name);
        std::fs::create_dir_all(&root).unwrap();
        self.host
            .registry()
            .unwrap()
            .create(NewProject {
                root,
                ..NewProject::default()
            })
            .unwrap()
            .id
    }

    /// 전역 설정(`$CONFIG/settings.toml`)을 쓰고 다시 읽는다. `body` 는 `schema = 1` 뒤에 붙는다.
    pub fn global_settings(&self, body: &str) {
        std::fs::create_dir_all(&self.paths.config).unwrap();
        std::fs::write(
            self.paths.config.join("settings.toml"),
            format!("schema = 1\n{body}"),
        )
        .unwrap();
        self.host.config().unwrap().global.reload().unwrap();
    }

    /// 카탈로그의 툴(그룹 · 등급 · 메서드). MCP 의 `tools/list` 에는 그룹 · 등급이 없다.
    pub fn tool(&self, name: &str) -> &pw_mcp::Tool {
        self.services
            .catalog
            .get(name)
            .unwrap_or_else(|| panic!("`{name}` 은 툴이 아니다"))
    }

    /// 진짜 MCP 세션(rmcp 클라이언트 ↔ `pw_mcp::serve`)을 연다. 세션을 끝낼 때는 [`ToolSession::close`].
    pub async fn session(&self, request: SessionRequest) -> ToolSession {
        let (server_io, client_io) = tokio::io::duplex(256 * 1024);
        let cancel = CancellationToken::new();
        tokio::spawn(pw_mcp::serve(
            Arc::clone(&self.services),
            request,
            tokio::io::split(server_io),
            cancel.clone(),
        ));
        let client = rmcp::model::ClientConfig::default()
            .serve(client_io)
            .await
            .expect("MCP 세션이 열리지 않았다");
        ToolSession { client, cancel }
    }

    /// [`Self::session`] 인데 클라이언트 이름이 CLI(`pitwall-cli`)다 — 세션이 CLI 호출로 본다(셋째 답이 「CLI 허용」).
    pub async fn cli_session(&self) -> ToolSession {
        use rmcp::model::{ClientCapabilities, ClientConfig, Implementation};
        let (server_io, client_io) = tokio::io::duplex(256 * 1024);
        let cancel = CancellationToken::new();
        tokio::spawn(pw_mcp::serve(
            Arc::clone(&self.services),
            SessionRequest::default(),
            tokio::io::split(server_io),
            cancel.clone(),
        ));
        let client = ClientConfig::new(
            ClientCapabilities::default(),
            Implementation::new(crate::tool_cli::CLIENT_NAME, env!("CARGO_PKG_VERSION")),
        )
        .serve(client_io)
        .await
        .expect("MCP 세션이 열리지 않았다");
        ToolSession { client, cancel }
    }

    /// `pitwall tool <args>` 를 이 Host(앱)에 붙여 돌린다 — (종료 코드, stdout, stderr).
    pub async fn cli(&self, args: &[&str]) -> (i32, String, String) {
        use crate::tool_cli::{Action, Call, Request, execute, parse, read_input};
        let args: Vec<std::ffi::OsString> = args.iter().map(Into::into).collect();
        let invocation = parse(&args).unwrap();
        let input = match &invocation.action {
            Action::Call(Call {
                input: Some(input), ..
            }) => Some(read_input(input, std::io::empty()).unwrap()),
            _ => None,
        };
        let (server_io, client_io) = tokio::io::duplex(256 * 1024);
        tokio::spawn(pw_mcp::serve(
            Arc::clone(&self.services),
            SessionRequest::default(),
            tokio::io::split(server_io),
            CancellationToken::new(),
        ));
        let linked = pw_mcp::Catalog::linked();
        let request = Request {
            action: &invocation.action,
            input,
            linked: &linked,
            headless: None,
        };
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = execute(
            request,
            client_io,
            std::future::pending(),
            &mut out,
            &mut err,
        )
        .await;
        (
            code,
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    }

    /// 그 프로젝트 창의 연결(`ConnectionScope::Project`)로 RPC 하나를 부른다 — 사람이 창에서 한 일.
    pub async fn window_call(
        &self,
        project: &str,
        method: &str,
        params: Value,
    ) -> Result<Value, Value> {
        use pw_core::Host;
        let caller = pw_rpc::Caller {
            role: pw_rpc::Role::Owner,
            scope: pw_rpc::ConnectionScope::Project(project.to_owned()),
            window: Some("main".to_owned()),
        };
        let mut connection = self.host.connect(caller).await.unwrap();
        connection.sink.send(serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": method, "params": params,
        }));
        loop {
            let frame = tokio::time::timeout(Duration::from_secs(10), connection.source.recv())
                .await
                .expect("no response")
                .expect("closed");
            if frame.get("id") == Some(&Value::from(1)) {
                return match frame.get("error") {
                    Some(error) => Err(error.clone()),
                    None => Ok(frame["result"].clone()),
                };
            }
        }
    }

    /// 감사 로그의 행이 `n` 개가 되고 모두 끝 기록을 받을 때까지 기다려 전부 돌려준다. 오래된 것부터. 기록은 응답 뒤에 따로
    /// 끝난다 — 선기록한 행(`write` 이상 · 승인을 지난 호출)은 결과가 나간 뒤 큐로 `started` 에서 끝난 결과로 고쳐지므로, 행 수만
    /// 보고 읽으면 아직 `started` 일 수 있다(부하에서 흔들렸다).
    pub fn audit_rows(&self, n: usize) -> Vec<AuditRow> {
        let file = self.services.audit.file().to_path_buf();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            let rows = read_audit(&file);
            if rows.len() >= n && rows.iter().all(|r| r.outcome != "started") {
                return rows;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "감사 행이 {n} 개가 되어 끝나지 않았다: {rows:?}"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

/// 감사 로그 한 행 가운데 툴 행이 볼 칸.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AuditRow {
    pub tool: String,
    pub project_id: Option<String>,
    /// 인자 요약(`pw_mcp::audit::summarize`).
    pub args: String,
    /// `ok` · `error` · `denied`.
    pub outcome: String,
    pub detail: Option<String>,
}

fn read_audit(file: &Path) -> Vec<AuditRow> {
    let Ok(conn) =
        rusqlite::Connection::open_with_flags(file, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
    else {
        return Vec::new();
    };
    let Ok(mut stmt) =
        conn.prepare("SELECT tool, project_id, args, outcome, detail FROM calls ORDER BY id")
    else {
        return Vec::new();
    };
    stmt.query_map([], |r| {
        Ok(AuditRow {
            tool: r.get(0)?,
            project_id: r.get(1)?,
            args: r.get(2)?,
            outcome: r.get(3)?,
            detail: r.get(4)?,
        })
    })
    .unwrap()
    .map(Result::unwrap)
    .collect()
}

/// 열린 MCP 세션 하나.
pub(crate) struct ToolSession {
    client: RunningService<RoleClient, rmcp::model::ClientConfig>,
    cancel: CancellationToken,
}

impl ToolSession {
    /// `tools/list`(쪽을 끝까지).
    pub async fn list(&self) -> Vec<rmcp::model::Tool> {
        self.client.peer().list_all_tools().await.unwrap()
    }

    /// `tools/call`. 툴 오류 · 거절은 `is_error` 가 켜진 결과이고, 노출되지 않은 툴은 패닉이다 — 그것은 [`Self::try_call`].
    pub async fn call(&self, name: &str, args: Value) -> CallToolResult {
        self.try_call(name, args)
            .await
            .unwrap_or_else(|e| panic!("`{name}` 호출이 MCP 오류다: {e}"))
    }

    /// `tools/call` 의 MCP 오류(모르는 · 꺼진 툴, 잘못된 입력)까지 돌려준다.
    pub async fn try_call(
        &self,
        name: &str,
        args: Value,
    ) -> Result<CallToolResult, rmcp::ServiceError> {
        let mut params = CallToolRequestParams::new(name.to_owned());
        if let Value::Object(map) = args {
            params = params.with_arguments(map);
        }
        self.client.peer().call_tool(params).await
    }

    pub async fn close(self) {
        let _ = self.client.cancel().await;
        self.cancel.cancel();
    }
}

/// 결과의 글(`content` 의 text 를 이은 것).
#[allow(dead_code)] // 툴 행이 쓴다.
pub(crate) fn text(result: &CallToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|c| c.as_text().map(|t| t.text.clone()))
        .collect()
}

/// 결과의 `structuredContent`(결과가 object 인 툴).
#[allow(dead_code)] // 툴 행이 쓴다.
pub(crate) fn structured(result: &CallToolResult) -> Map<String, Value> {
    match &result.structured_content {
        Some(Value::Object(map)) => map.clone(),
        other => panic!("structuredContent 가 object 가 아니다: {other:?}"),
    }
}
