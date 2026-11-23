//! `pitwall tool` 의 시험(16-m5-plan M5-04): 인자 표, 스키마 → 플래그 표, 종료 코드 표.
//!
//! 종료 코드 표는 진짜 MCP 세션으로 본다 — 가짜 툴을 단 Host 에 `pw_mcp::serve` 를 붙이고 CLI 의 [`execute`] 가 프로세스 안
//! duplex 로 붙는다(앱 없는 CLI 가 헤드리스 Host 에 붙는 꼴과 같다). 가짜 툴은 손으로 만든 선언이다 — `rpc_method!` 로 만들면
//! 레지스트리(`pw_rpc::methods`)에 들어가 툴 목록 시험(`mcp_tests.rs`)에 섞인다.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use pw_core::{HostRouter, LocalHost};
use pw_mcp::audit::AuditLog;
use pw_mcp::policy::Approval;
use pw_mcp::{HostKind, Services};
use pw_rpc::{MethodInfo, RpcError};
use serde_json::{Map, Value, json};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

use super::*;

fn os(args: &[&str]) -> Vec<OsString> {
    args.iter().map(OsString::from).collect()
}

fn call(tool: &str, input: Option<Input>, flags: &[(&str, &str)]) -> Action {
    Action::Call(Call {
        tool: tool.into(),
        input,
        flags: flags
            .iter()
            .map(|(n, v)| ((*n).to_owned(), (*v).to_owned()))
            .collect(),
    })
}

// ---- 인자 표 ----

#[test]
fn the_argument_table() {
    let parsed = |args: &[&str]| parse(&os(args));
    let action = |args: &[&str]| parsed(args).unwrap().action;
    assert_eq!(action(&[]), Action::Usage, "툴 없이 `pitwall tool`");
    assert_eq!(action(&["--help"]), Action::Help);
    assert_eq!(action(&["-h"]), Action::Help);
    assert_eq!(action(&["list"]), Action::List);
    assert_eq!(action(&["list", "--help"]), Action::Help);
    assert!(parsed(&["list", "extra"]).is_err());
    assert_eq!(
        action(&["git_status", "--help", "--max-count", "3"]),
        Action::ToolHelp("git_status".into())
    );
    assert_eq!(
        action(&["git_log", "--max-count", "3", "--path=src/a b.rs"]),
        call(
            "git_log",
            None,
            &[("max-count", "3"), ("path", "src/a b.rs")]
        )
    );
    // 값이 `-` 로 시작해도 플래그 다음 자리면 값이다.
    assert_eq!(
        action(&["t", "--offset", "-5"]),
        call("t", None, &[("offset", "-5")])
    );
    assert_eq!(
        action(&["t", "--input", "-"]),
        call("t", Some(Input::Stdin), &[])
    );
    assert_eq!(
        action(&["t", "--input={\"a\":1}", "--a", "2"]),
        call("t", Some(Input::Json("{\"a\":1}".into())), &[("a", "2")])
    );
    // 플래그 다음 자리는 값이다 — `--help` · `-h` · `--project` · `--input` 처럼 보여도 가로채지 않는다.
    assert_eq!(
        action(&["t", "--project-id", "--help"]),
        call("t", None, &[("project-id", "--help")])
    );
    assert_eq!(
        action(&["t", "--pattern", "-h", "--x=1"]),
        call("t", None, &[("pattern", "-h"), ("x", "1")])
    );
    assert_eq!(
        action(&["t", "--a", "1", "--help"]),
        Action::ToolHelp("t".into()),
        "이름 자리의 `--help` 는 도움말이다"
    );
    assert_eq!(
        action(&["t", "--x=--help", "-h"]),
        Action::ToolHelp("t".into())
    );
    assert_eq!(
        action(&["t", "--input", "--help"]),
        call("t", Some(Input::Json("--help".into())), &[])
    );
    let message = parsed(&["t", "--message", "--project", "--project", "/p"]).unwrap();
    assert_eq!(
        (message.project, message.action),
        (
            Some(PathBuf::from("/p")),
            call("t", None, &[("message", "--project")])
        )
    );
    // 사용법 오류: 모르는 옵션 · 값 없는 플래그 · 두 번 준 플래그 · 툴 뒤의 위치 인자.
    for bad in [
        &["--verbose"][..],
        &["t", "--a"],
        &["t", "--a", "1", "--a", "2"],
        &["t", "--input", "{}", "--input", "-"],
        &["t", "loose"],
        &["t", "--"],
        &["--project"],
    ] {
        assert!(parsed(bad).is_err(), "{bad:?}");
    }
}

#[test]
fn project_goes_anywhere_after_tool() {
    let project = |args: &[&str]| {
        let invocation = parse(&os(args)).unwrap();
        (invocation.project, invocation.action)
    };
    let expected = (Some(PathBuf::from("/p")), call("t", None, &[("a", "1")]));
    assert_eq!(project(&["--project", "/p", "t", "--a", "1"]), expected);
    assert_eq!(project(&["t", "--project", "/p", "--a", "1"]), expected);
    assert_eq!(project(&["t", "--a", "1", "--project=/p"]), expected);
    assert_eq!(
        project(&["list", "--project", "rel"]),
        (Some(PathBuf::from("rel")), Action::List)
    );
    assert_eq!(
        project(&["--project", "/p"]),
        (Some(PathBuf::from("/p")), Action::Usage)
    );
    assert!(parse(&os(&["--project", "/a", "t", "--project", "/b"])).is_err());
}

#[test]
fn input_reads_json_or_stdin_and_must_be_an_object() {
    assert_eq!(
        read_input(&Input::Json("{\"a\":1}".into()), std::io::empty()).unwrap(),
        json!({"a": 1})
    );
    assert_eq!(
        read_input(&Input::Stdin, std::io::Cursor::new(b"{\"b\": [true]}\n")).unwrap(),
        json!({"b": [true]})
    );
    assert!(read_input(&Input::Json("[1]".into()), std::io::empty()).is_err());
    assert!(read_input(&Input::Json("{".into()), std::io::empty()).is_err());
    assert!(read_input(&Input::Stdin, std::io::Cursor::new(b"")).is_err());
}

// ---- 스키마 → 플래그 표 ----

/// `--checkout <machine>:<path>`(18-m7-plan M7-18 ⑥ — D61 의 꼴): 세션이 스키마에 더한 `checkout` 객체로 읽는다. 머신 id 의 `:` 는
/// 경로의 시작이 아니다. 경로는 그 머신의 것이라 펼치지 않는다.
#[test]
fn the_checkout_flag_takes_machine_and_path() {
    let mut schema = json!({
        "type": "object",
        "properties": { "projectId": { "type": "string" }, "worktree": { "type": ["string", "null"] } }
    })
    .as_object()
    .unwrap()
    .clone();
    schema = pw_mcp::checkout::with_checkout(schema);
    let read = |raw: &str| {
        arguments(
            "run_start",
            &schema,
            None,
            &[("checkout".to_owned(), raw.to_owned())],
        )
        .map(|a| a["checkout"].clone())
    };
    assert_eq!(
        read("mini:~/work/pitwall").unwrap(),
        json!({"machine": "mini", "worktree": "~/work/pitwall"})
    );
    assert_eq!(
        read("mini").unwrap(),
        json!({"machine": "mini", "worktree": null})
    );
    assert_eq!(
        read("SHA256:abc:/srv/app").unwrap(),
        json!({"machine": "SHA256:abc", "worktree": "/srv/app"})
    );
    assert_eq!(
        read(r#"{"machine":"mini","worktree":"/w"}"#).unwrap(),
        json!({"machine": "mini", "worktree": "/w"})
    );
    assert!(read(":/w").is_err());
    assert!(read("{nope").is_err());
}

#[test]
fn flag_names_are_kebab_case() {
    for (field, flag) in [
        ("projectId", "project-id"),
        ("maxCount", "max-count"),
        ("max_count", "max-count"),
        ("path", "path"),
        ("includeHTML", "include-h-t-m-l"),
        ("a1B2", "a1-b2"),
    ] {
        assert_eq!(flag_name(field), flag, "{field}");
    }
}

/// schemars 가 만드는 꼴: `Option<T>` 는 `["T", "null"]`, enum 은 `$ref` → `$defs` 의 `enum`.
fn schema() -> Map<String, Value> {
    json!({
        "type": "object",
        "properties": {
            "projectId": {"type": "string", "description": "the project"},
            "path": {"type": ["string", "null"]},
            "maxCount": {"type": ["integer", "null"], "format": "uint32"},
            "ratio": {"type": "number"},
            "staged": {"type": ["boolean", "null"]},
            "paths": {"type": ["array", "null"], "items": {"type": "string"}},
            "options": {"type": ["object", "null"]},
            "mode": {"$ref": "#/$defs/Mode"},
            "either": {"anyOf": [{"type": "integer"}, {"type": "string"}]},
            "anything": {}
        },
        "required": ["projectId"],
        "$defs": {"Mode": {"type": "string", "enum": ["soft", "hard"]}}
    })
    .as_object()
    .unwrap()
    .clone()
}

#[test]
fn the_schema_to_flag_table() {
    let args = |input: Option<Value>, flags: &[(&str, &str)]| {
        let given: Vec<_> = flags
            .iter()
            .map(|(n, v)| ((*n).to_owned(), (*v).to_owned()))
            .collect();
        arguments("t", &schema(), input, &given).map(Value::Object)
    };
    // 타입별 읽기: 글자열은 그대로(JSON 처럼 보여도), 수 · 불리언 · 배열 · 객체는 JSON.
    assert_eq!(
        args(
            None,
            &[
                ("project-id", "p1"),
                ("path", "42"),
                ("max-count", "7"),
                ("ratio", "0.5"),
                ("staged", "true"),
                ("paths", "[\"a\",\"b\"]"),
                ("options", "{\"x\":1}"),
                ("mode", "hard"),
            ]
        )
        .unwrap(),
        json!({
            "projectId": "p1", "path": "42", "maxCount": 7, "ratio": 0.5, "staged": true,
            "paths": ["a", "b"], "options": {"x": 1}, "mode": "hard"
        })
    );
    // 정수 · 글자열을 다 받는 필드는 JSON 으로 읽히면 수, 아니면 글자열. 타입을 모르는 필드도 같다.
    assert_eq!(
        args(
            Some(json!({"projectId": "p"})),
            &[("either", "3"), ("anything", "x y")]
        )
        .unwrap(),
        json!({"projectId": "p", "either": 3, "anything": "x y"})
    );
    // `null` 을 받는 필드는 `null` 을 받는다. 글자열 필드의 `null` 은 글자열이다.
    assert_eq!(
        args(
            Some(json!({"projectId": "p"})),
            &[("max-count", "null"), ("path", "null")]
        )
        .unwrap(),
        json!({"projectId": "p", "maxCount": null, "path": "null"})
    );
    // 플래그가 `--input` 을 덮는다. `--input` 의 다른 필드는 남는다.
    assert_eq!(
        args(
            Some(json!({"projectId": "from-input", "maxCount": 1, "extra": true})),
            &[("project-id", "from-flag")]
        )
        .unwrap(),
        json!({"projectId": "from-flag", "maxCount": 1, "extra": true})
    );
    // 모르는 플래그(필드 이름 그대로 써도 모른다 — 플래그는 kebab-case).
    for unknown in ["nope", "projectId", "max_count"] {
        let e = args(Some(json!({"projectId": "p"})), &[(unknown, "1")]).unwrap_err();
        assert!(e.contains(&format!("--{unknown}")), "{e}");
    }
    // 타입 불일치.
    for (flag, value) in [
        ("max-count", "seven"),
        ("max-count", "1.5"),
        ("max-count", "\"7\""),
        ("ratio", "x"),
        ("staged", "yes"),
        ("paths", "a,b"),
        ("options", "[1]"),
    ] {
        let e = args(Some(json!({"projectId": "p"})), &[(flag, value)]).unwrap_err();
        assert!(
            e.contains(&format!("--{flag} expects")),
            "{flag} {value}: {e}"
        );
    }
    // 빠진 필수 필드.
    let e = args(None, &[("path", "a")]).unwrap_err();
    assert!(e.contains("--project-id"), "{e}");
}

#[test]
fn flags_describe_type_and_requiredness() {
    let flags = flags(&schema());
    let get = |name: &str| flags.iter().find(|f| f.name == name).unwrap();
    assert!(get("project-id").required);
    assert!(!get("max-count").required);
    assert_eq!(get("max-count").value_label(), "<integer|null>");
    assert_eq!(get("mode").value_label(), "<string>");
    assert_eq!(get("either").value_label(), "<string|integer>");
    assert_eq!(get("anything").value_label(), "<json>");
    assert_eq!(
        get("project-id").description.as_deref(),
        Some("the project")
    );
}

// ---- 종료 코드 표 ----

mod fake {
    use pw_rpc::{McpTool, Method, MethodKind, MethodMeta, RoleSet, Tier, UnaryMethod};
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
    #[serde(rename_all = "camelCase")]
    pub struct EchoParams {
        pub text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub max_count: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub verbose: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub tags: Option<Vec<String>>,
    }

    /// 입력 없는 쓰기 툴의 입력. `write` 이상 툴은 계획을 만드는 `Planned` struct 입력이어야 한다(카탈로그 규칙 — 16-m5-plan M5-12).
    #[derive(Debug, Deserialize, specta::Type, schemars::JsonSchema)]
    pub struct NoInput {}

    pub type WriteParams = pw_core::registry::plan::Planned<NoInput>;

    macro_rules! fake {
        ($ty:ident, $method:literal, $name:literal, $tier:ident, $params:ty, $result:ty) => {
            #[derive(Debug, Clone, Copy)]
            pub struct $ty;
            impl Method for $ty {
                type Params = $params;
                const META: MethodMeta = MethodMeta {
                    name: $method,
                    kind: MethodKind::Unary,
                    tier: Tier::$tier,
                    roles: RoleSet::default_for(Tier::$tier),
                    params: "",
                    output: "",
                    doc: concat!("Fake tool `", $name, "`.\nSecond line."),
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

    fake!(Echo, "cli.echo", "cli_echo", Read, EchoParams, EchoParams);
    fake!(Fail, "cli.fail", "cli_fail", Read, (), ());
    fake!(Write, "cli.write", "cli_write", Write, WriteParams, ());
    fake!(Slow, "cli.slow", "cli_slow", Read, (), ());
    fake!(AppOnly, "cli.appOnly", "cli_app_only", Read, (), ());
}

/// 오래 도는 가짜 핸들러가 남기는 것: 시작했다, 끝나기 전에 버려졌다.
#[derive(Clone, Default)]
struct Probe {
    started: Arc<Notify>,
    dropped: Arc<AtomicBool>,
}

struct DropFlag(Arc<AtomicBool>);

impl Drop for DropFlag {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

/// 가짜 툴을 단 Host 하나. 앱 Host 는 가짜 툴 전부를, 헤드리스 Host 는 `cli_app_only` 를 빼고 처리한다 — 그 툴은 「링크된
/// 카탈로그에는 있고 헤드리스 Host 가 처리하지 않는」 앱 전용 툴이다.
struct Fixture {
    _tmp: tempfile::TempDir,
    paths: AppPaths,
    host: Arc<LocalHost>,
    kind: HostKind,
    /// CLI 가 보는 「링크된 카탈로그」: 가짜 툴과 이 시험 실행 파일에 실제로 링크된 선언.
    linked: Catalog,
    probe: Probe,
}

impl Fixture {
    fn new(kind: HostKind) -> Self {
        Self::with_settings(kind, "")
    }

    /// `settings` 는 전역 설정(`schema = 1` 뒤)이다. Host 를 만들기 전에 쓴다.
    fn with_settings(kind: HostKind, settings: &str) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::under(tmp.path());
        std::fs::create_dir_all(&paths.config).unwrap();
        std::fs::write(
            paths.config.join("settings.toml"),
            format!("schema = 1\n{settings}"),
        )
        .unwrap();
        let probe = Probe::default();
        let slow = probe.clone();
        let mut builder = LocalHost::builder(crate::LOCAL_HOST_ID).config(paths.clone());
        if kind == HostKind::Headless {
            builder = builder.read_only_registry();
        }
        let host = Arc::new(
            builder
                .methods(move |router| {
                    router.unary(fake::Echo, |_ctx, params: fake::EchoParams| async move {
                        Ok::<_, RpcError>(params)
                    });
                    router.unary(fake::Fail, |_ctx, ()| async {
                        Err::<(), _>(RpcError::internal("the fake tool failed"))
                    });
                    router.unary(fake::Write, |_ctx, _p: fake::WriteParams| async {
                        Ok::<(), RpcError>(())
                    });
                    router.unary(fake::Slow, move |_ctx, ()| {
                        let probe = slow.clone();
                        async move {
                            let flag = DropFlag(probe.dropped.clone());
                            probe.started.notify_one();
                            tokio::time::sleep(Duration::from_secs(60)).await;
                            std::mem::forget(flag);
                            Ok::<(), RpcError>(())
                        }
                    });
                    if kind == HostKind::App {
                        router.unary(fake::AppOnly, |_ctx, ()| async { Ok::<(), RpcError>(()) });
                    }
                })
                .build(),
        );
        let fakes = [
            MethodInfo::unary::<fake::Echo>(),
            MethodInfo::unary::<fake::Fail>(),
            MethodInfo::unary::<fake::Write>(),
            MethodInfo::unary::<fake::Slow>(),
            MethodInfo::unary::<fake::AppOnly>(),
        ];
        let linked = Catalog::from_methods(fakes.iter().chain(pw_rpc::methods())).unwrap();
        Self {
            _tmp: tmp,
            paths,
            host,
            kind,
            linked,
            probe,
        }
    }

    /// `mcp::services` 와 같은 규칙으로 만든 것 — 카탈로그만 가짜 툴이 든 「링크된 카탈로그」에서 거른다.
    fn services(&self) -> Arc<Services> {
        let router = Arc::new(HostRouter::new());
        router.add_host(self.host.clone()).unwrap();
        Arc::new(Services {
            router,
            host_id: crate::LOCAL_HOST_ID.into(),
            paths: self.paths.clone(),
            global: self.host.config().unwrap().global.clone(),
            bus: self.host.bus().clone(),
            audit: Arc::new(AuditLog::under_state(&self.paths.state)),
            catalog: Arc::new(self.linked.clone().routed_by(self.host.router())),
            approval: Approval::Unavailable,
            transport: "test",
            host_kind: self.kind,
            sessions: Default::default(),
            machines: None,
        })
    }

    /// `pitwall tool <args>` 를 이 Host 에 붙여 돌린다. `interrupt` 는 Ctrl+C 자리, `cut` 을 취소하면 CLI 와 서버 사이의 선을
    /// 끊는다(앱이 도중에 죽은 꼴 — 서버는 응답을 보내지 못한다). (종료 코드, stdout, stderr).
    async fn run_with(
        &self,
        args: &[&str],
        interrupt: impl Future<Output = ()>,
        cut: CancellationToken,
    ) -> (i32, String, String) {
        let invocation = parse(&os(args)).unwrap();
        let input = match &invocation.action {
            Action::Call(Call {
                input: Some(input), ..
            }) => Some(read_input(input, std::io::empty()).unwrap()),
            _ => None,
        };
        let services = self.services();
        let headless = (self.kind == HostKind::Headless).then(|| Arc::clone(&services.catalog));
        let (client_io, mut wire_client) = tokio::io::duplex(64 * 1024);
        let (mut wire_server, server_io) = tokio::io::duplex(64 * 1024);
        tokio::spawn(async move {
            tokio::select! {
                _ = tokio::io::copy_bidirectional(&mut wire_client, &mut wire_server) => {}
                () = cut.cancelled() => {}
            }
        });
        tokio::spawn(pw_mcp::serve(
            services,
            pw_mcp::SessionRequest::default(),
            tokio::io::split(server_io),
            CancellationToken::new(),
        ));
        let request = Request {
            action: &invocation.action,
            input,
            linked: &self.linked,
            headless: headless.as_deref(),
        };
        let (mut out, mut err) = (Vec::new(), Vec::new());
        // 서버 세션은 CLI 가 전송을 닫으면 스스로 끝난다. 여기서 세션 토큰을 취소하지 않는다 — 취소하면 돌던 호출이 그 토큰으로
        // 거둬져, CLI 가 취소 알림을 보냈는지를 시험이 가르지 못한다.
        let code = execute(request, client_io, interrupt, &mut out, &mut err).await;
        (
            code,
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    }

    async fn run(&self, args: &[&str]) -> (i32, String, String) {
        self.run_with(args, std::future::pending(), CancellationToken::new())
            .await
    }

    /// 감사 로그의 (툴, 결과, 클라이언트, 상세) — `n` 행이 될 때까지 기다린다.
    fn audit(&self, n: usize) -> Vec<(String, String, Option<String>, Option<String>)> {
        let file = self.paths.state.join("mcp").join("audit.db");
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            let rows = read_audit(&file);
            if rows.len() >= n {
                return rows;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "감사 행이 {n} 개가 되지 않았다: {rows:?}"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

fn read_audit(file: &Path) -> Vec<(String, String, Option<String>, Option<String>)> {
    let Ok(conn) =
        rusqlite::Connection::open_with_flags(file, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
    else {
        return Vec::new();
    };
    let Ok(mut stmt) = conn.prepare("SELECT tool, outcome, client, detail FROM calls ORDER BY id")
    else {
        return Vec::new();
    };
    stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn success_prints_one_json_document_and_exits_0() {
    let f = Fixture::new(HostKind::App);
    let (code, out, err) = f
        .run(&[
            "cli_echo",
            "--input",
            "{\"text\":\"base\",\"verbose\":true}",
            "--text",
            "from a flag",
            "--max-count",
            "3",
            "--tags",
            "[\"a\",\"b\"]",
        ])
        .await;
    assert_eq!(code, exit::OK, "{err}");
    let document: Value = serde_json::from_str(&out).expect("stdout 은 JSON 문서 하나다");
    assert_eq!(
        document,
        json!({"text": "from a flag", "maxCount": 3, "verbose": true, "tags": ["a", "b"]})
    );
    assert_eq!(err, "");
    let rows = f.audit(1);
    assert_eq!((rows[0].0.as_str(), rows[0].1.as_str()), ("cli_echo", "ok"));
    assert_eq!(
        rows[0].2.as_deref(),
        Some(concat!("pitwall-cli ", env!("CARGO_PKG_VERSION")))
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_tool_error_exits_1_with_the_reason_on_stderr() {
    let f = Fixture::new(HostKind::App);
    let (code, out, err) = f.run(&["cli_fail"]).await;
    assert_eq!(code, exit::FAILED);
    assert_eq!(out, "");
    assert!(err.contains("the fake tool failed"), "{err}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn denials_exit_3() {
    // 승인이 필요한데 받을 길이 없다(앱).
    let f = Fixture::new(HostKind::App);
    let (code, out, err) = f.run(&["cli_write"]).await;
    assert_eq!((code, out.as_str()), (exit::DENIED, ""), "{err}");
    assert!(err.contains("approval"), "{err}");

    // 헤드리스의 `write` 이상.
    let f = Fixture::new(HostKind::Headless);
    let (code, _, err) = f.run(&["cli_write"]).await;
    assert_eq!(code, exit::DENIED, "{err}");

    // 노출 설정이 끈 툴: 목록에 없지만 링크된 툴이라 부르고, 서버가 거절과 감사 행을 남긴다.
    let f = Fixture::with_settings(HostKind::App, "[mcp.tools]\ncli_echo = false\n");
    let (code, _, list_err) = f.run(&["list"]).await;
    assert_eq!(code, exit::OK, "{list_err}");
    let (code, out, err) = f.run(&["cli_echo", "--text", "x"]).await;
    assert_eq!((code, out.as_str()), (exit::DENIED, ""), "{err}");
    assert!(err.contains("turned off"), "{err}");
    let rows = f.audit(1);
    assert_eq!(
        (rows[0].0.as_str(), rows[0].1.as_str()),
        ("cli_echo", "denied")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn usage_errors_exit_2_without_calling() {
    let f = Fixture::new(HostKind::App);
    for args in [
        &["no_such_tool"][..],
        &["cli_echo", "--text", "x", "--nope", "1"],
        &["cli_echo", "--text", "x", "--max-count", "many"],
        &["cli_echo"],
        &["no_such_tool", "--help"],
    ] {
        let (code, out, err) = f.run(args).await;
        assert_eq!((code, out.as_str()), (exit::USAGE, ""), "{args:?}: {err}");
    }
    // 부르지 않았다 — 감사 행은 그 뒤 성공한 호출 하나뿐이다.
    assert_eq!(f.run(&["project_list"]).await.0, exit::OK);
    let rows = f.audit(1);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].0, "project_list");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_app_tool_without_the_app_exits_3_without_calling() {
    let f = Fixture::new(HostKind::Headless);
    let (code, out, _) = f.run(&["list"]).await;
    assert_eq!(code, exit::OK);
    assert!(!out.contains("cli_app_only"), "{out}");
    assert!(out.contains("cli_echo"), "{out}");

    let (code, out, err) = f.run(&["cli_app_only"]).await;
    assert_eq!((code, out.as_str()), (exit::DENIED, ""), "{err}");
    assert!(err.contains("start the Pitwall app and try again"), "{err}");
    // 부르지 않았다 — 감사 행은 그 뒤 성공한 호출 하나뿐이다(서버가 거절했다면 `denied` 행이 남는다).
    assert_eq!(f.run(&["project_list"]).await.0, exit::OK);
    let rows = f.audit(1);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].0, "project_list");
    // 도움말은 보인다 — 앱이 있어야 한다는 줄과 함께.
    let (code, out, _) = f.run(&["cli_app_only", "--help"]).await;
    assert_eq!(code, exit::OK);
    assert!(out.contains("needs the Pitwall app"), "{out}");

    // 같은 툴을 앱 Host 에서는 부른다.
    let app = Fixture::new(HostKind::App);
    let (code, out, err) = app.run(&["cli_app_only"]).await;
    assert_eq!(code, exit::OK, "{err}");
    assert_eq!(out.trim(), "null");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lost_connection_exits_1() {
    let f = Fixture::new(HostKind::App);
    let cut = CancellationToken::new();
    let started = f.probe.started.clone();
    let cutter = cut.clone();
    tokio::spawn(async move {
        started.notified().await;
        cutter.cancel();
    });
    let (code, out, err) = f.run_with(&["cli_slow"], std::future::pending(), cut).await;
    assert_eq!((code, out.as_str()), (exit::FAILED, ""), "{err}");
    assert!(err.contains("lost the connection"), "{err}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_interrupt_cancels_the_call_and_exits_130() {
    let f = Fixture::new(HostKind::Headless);
    let started = f.probe.started.clone();
    let (code, out, err) = f
        .run_with(
            &["cli_slow"],
            async move { started.notified().await },
            CancellationToken::new(),
        )
        .await;
    assert_eq!((code, out.as_str()), (exit::INTERRUPTED, ""), "{err}");
    // Host 의 핸들러 future 가 버려지고 감사 행이 결과 `error` · 까닭 `cancelled`. 연결이 끊긴 뒤 rmcp 가 남은 응답을 5초
    // 기다렸다 거두는 길이 아니라 취소 알림으로 거둔다 — 그보다 짧게 기다린다.
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while !f.probe.dropped.load(Ordering::SeqCst) {
        assert!(
            std::time::Instant::now() < deadline,
            "Host 의 핸들러가 버려지지 않았다"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let rows = f.audit(1);
    assert_eq!(
        (rows[0].0.as_str(), rows[0].1.as_str(), rows[0].3.as_deref()),
        ("cli_slow", "error", Some("cancelled"))
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_and_help_show_name_tier_and_flags() {
    let f = Fixture::new(HostKind::App);
    let (code, out, _) = f.run(&["list"]).await;
    assert_eq!(code, exit::OK);
    let write = out.lines().find(|l| l.starts_with("cli_write")).unwrap();
    assert!(write.contains(" write "), "{write}");
    assert!(
        write.ends_with("Fake tool `cli_write`."),
        "설명은 한 줄: {write}"
    );

    let (code, out, _) = f.run(&["cli_echo", "--help"]).await;
    assert_eq!(code, exit::OK);
    assert!(out.contains("(read)"), "{out}");
    assert!(out.contains("Second line."), "도움말은 설명 전부: {out}");
    let line = |flag: &str| {
        out.lines()
            .find(|l| l.trim_start().starts_with(flag))
            .unwrap_or_else(|| panic!("{flag} 줄이 없다: {out}"))
            .to_owned()
    };
    assert!(line("--text <string>").contains("required"), "{out}");
    assert!(
        line("--max-count <integer|null>").contains("optional"),
        "{out}"
    );
    assert!(line("--tags <array|null>").contains("optional"), "{out}");
}
