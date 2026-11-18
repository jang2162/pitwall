//! 디버그 어댑터 정의의 파싱 · 검사(17-m6-plan.md M6-16 완료 기준 ①)와 요청 틀 채우기.

use serde_json::json;

use super::*;
use crate::manifest::{Diagnostic, LspServerRuntime, Manifest, Severity, parse_manifest};

const FILE: &str = "/ext/extension.toml";

const MINIMAL: &str = r#"
id = "dev.example.sample"
name = "Sample"
version = "0.3.0"
engines = { pitwall = ">=1.0" }
"#;

fn parse(body: &str) -> (Option<Manifest>, Vec<Diagnostic>) {
    parse_manifest(&format!("{MINIMAL}{body}"), FILE)
}

fn errors(diagnostics: &[Diagnostic]) -> Vec<String> {
    diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .map(ToString::to_string)
        .collect()
}

const ADAPTERS: &str = r#"
[contributes.debug_adapters.codelldb]
languages = ["rust", "c"]
mason = "codelldb"
command = "codelldb"
args = ["--port", "{port}", "--host={host}"]
env = { RUST_LOG = "error" }
transport = "tcp"
requires = { python = ">=3.9" }
project_local = "tools/codelldb"
launch = { type = "lldb", program = "{program}", args = "{args}", cwd = "{cwd}" }
attach = { type = "lldb", pid = "{pid}" }

[contributes.debug_adapters.java-debug]
languages = ["java", "kotlin"]
mason = "java-debug-adapter"
start = { lsp = "jdtls", command = "vscode.java.startDebugSession", bundles = ["java-debug-adapter/com.microsoft.java.debug.plugin.jar"] }
attach = { hostName = "127.0.0.1", port = "{port}" }

[contributes.debug_adapters.debugpy]
languages = ["python"]
command = "debugpy-adapter"
launch = { program = "{program}" }
"#;

/// 완료 기준 ①: 키마다 읽은 값. `transport` 는 적지 않으면 stdio, `start` 어댑터는 tcp 다.
#[test]
fn reads_debug_adapters_with_every_key() {
    let (manifest, diagnostics) = parse(ADAPTERS);
    assert_eq!(diagnostics, vec![]);
    let adapters = manifest.unwrap().contributes.debug_adapters;
    let ids: Vec<&str> = adapters.iter().map(|a| a.id.as_str()).collect();
    assert_eq!(ids, ["codelldb", "java-debug", "debugpy"]);

    let lldb = &adapters[0];
    let languages: Vec<&str> = lldb.languages.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(languages, ["rust", "c"]);
    assert_eq!(lldb.mason.as_deref(), Some("codelldb"));
    assert_eq!(lldb.command.as_deref(), Some("codelldb"));
    assert_eq!(lldb.args, ["--port", "{port}", "--host={host}"]);
    assert_eq!(lldb.env["RUST_LOG"], "error");
    assert_eq!(lldb.transport, DapTransport::Tcp);
    assert_eq!(lldb.requires[0].runtime, LspServerRuntime::Python);
    assert_eq!(
        lldb.project_local.as_ref().map(RelPath::as_str),
        Some("tools/codelldb")
    );
    assert_eq!(
        lldb.template(DapRequest::Launch),
        Some(&json!({"type": "lldb", "program": "{program}", "args": "{args}", "cwd": "{cwd}"}))
    );
    assert_eq!(
        lldb.template(DapRequest::Attach),
        Some(&json!({"type": "lldb", "pid": "{pid}"}))
    );
    assert_eq!(lldb.start, None);
    assert_eq!(
        lldb.args_for(Some(4711)),
        ["--port", "4711", "--host=127.0.0.1"]
    );

    let java = &adapters[1];
    assert_eq!(java.command, None);
    assert_eq!(java.transport, DapTransport::Tcp);
    assert_eq!(java.launch, None);
    let start = java.start.as_ref().unwrap();
    assert_eq!(start.lsp.name, "jdtls");
    assert_eq!(start.command, "vscode.java.startDebugSession");
    assert_eq!(
        start.bundles,
        ["java-debug-adapter/com.microsoft.java.debug.plugin.jar"]
    );

    let debugpy = &adapters[2];
    assert_eq!(debugpy.transport, DapTransport::Stdio);
    assert_eq!(debugpy.args_for(None), Vec::<String>::new());
    assert_eq!(debugpy.attach, None);
}

/// 완료 기준 ①: 모르는 키(어댑터 · `start` 안)는 경고하고 버린다 — 어댑터는 등록된다.
#[test]
fn warns_and_ignores_unknown_debug_adapter_keys() {
    let (manifest, diagnostics) = parse(
        "\n[contributes.debug_adapters.x]\nlanguages = [\"rust\"]\ncommand = \"x\"\nlaunch = {}\nport_range = [1, 2]\n\n[contributes.debug_adapters.y]\nlanguages = [\"java\"]\nstart = { lsp = \"jdtls\", command = \"c\", timeout = 3 }\nattach = {}\n",
    );
    let adapters = manifest
        .expect("warnings do not reject the extension")
        .contributes
        .debug_adapters;
    assert_eq!(adapters.len(), 2);
    let warnings: Vec<String> = diagnostics.iter().map(ToString::to_string).collect();
    assert_eq!(
        warnings,
        [
            format!("{FILE}:11:1: warning: debug adapter `x`: unknown key `port_range` is ignored"),
            format!(
                "{FILE}:15:41: warning: debug adapter `y`: `start`: unknown key `timeout` is ignored"
            ),
        ]
    );
}

/// 형식 오류. 오류가 하나라도 있으면 매니페스트가 없고, 진단은 그 값(또는 표 머리)의 줄 · 열을 가리킨다. 한 잘못에 오류 하나다.
#[test]
fn debug_adapter_format_errors_point_at_the_value() {
    let cases: [(&str, &str); 22] = [
        (
            "languages = [\"rust\"]\nlaunch = {}",
            "7:29: error: debug adapter `a`: missing `command` (or `start` for an adapter a language server opens)",
        ),
        (
            "languages = [\"rust\"]\ncommand = \"x\"",
            "7:29: error: debug adapter `a`: needs a `launch` or an `attach` request template (or both)",
        ),
        (
            "command = \"x\"\nlaunch = {}",
            "7:29: error: debug adapter `a`: missing `languages`",
        ),
        (
            "languages = [\"rust\"]\ncommand = \"bin/x\"\nlaunch = {}",
            "9:11: error: debug adapter `a`: `command` `bin/x` must be an executable name, not a path — set a path with `[dap.<id>] path` in settings",
        ),
        (
            "languages = [\"rust\"]\ncommand = \"x\"\ntransport = \"pipe\"\nlaunch = {}",
            "10:13: error: debug adapter `a`: `transport` `pipe` must be `stdio`, `tcp` or `socket`",
        ),
        (
            "languages = [\"rust\"]\ncommand = \"x\"\ntransport = \"tcp\"\nargs = [\"--stdio\"]\nlaunch = {}",
            "11:8: error: debug adapter `a`: a `tcp` adapter takes its port in `args` — write \"{port}\" where the port goes",
        ),
        (
            // 포트만 있고 주소 자리가 없다 — 어댑터가 어느 주소에 묶는지 정하지 못하므로 거절(`pw-dap` 의 `HostNotBound` 와 같은 규칙).
            "languages = [\"rust\"]\ncommand = \"x\"\ntransport = \"tcp\"\nargs = [\"--port\", \"{port}\"]\nlaunch = {}",
            "11:8: error: debug adapter `a`: a `tcp` adapter must be bound to 127.0.0.1 — write \"{host}\" where the address goes",
        ),
        (
            "languages = [\"rust\"]\ncommand = \"x\"\ntransport = \"tcp\"\nlaunch = {}",
            "7:29: error: debug adapter `a`: a `tcp` adapter takes its port in `args` — write \"{port}\" where the port goes",
        ),
        (
            "languages = [\"rust\"]\ncommand = \"x\"\nargs = [\"--port\", \"{port}\"]\nlaunch = {}",
            "10:8: error: debug adapter `a`: `{port}` and `{host}` are only for a `tcp` adapter",
        ),
        (
            "languages = [\"rust\"]\ncommand = \"x\"\ntransport = \"tcp\"\nargs = [\"{port}\", \"{program}\"]\nlaunch = {}",
            "11:19: error: debug adapter `a`: `args` can only use `{port}`, `{host}` and `{socket}`, not `{program}` — request values go in `launch` · `attach`",
        ),
        (
            "languages = [\"rust\"]\ncommand = \"x\"\ntransport = \"socket\"\nargs = [\"--port\", \"{port}\"]\nlaunch = {}",
            "11:8: error: debug adapter `a`: a `socket` adapter takes its socket path in `args` — write \"{socket}\" where the path goes",
        ),
        (
            "languages = [\"rust\"]\ncommand = \"x\"\ntransport = \"socket\"\nlaunch = {}",
            "7:29: error: debug adapter `a`: a `socket` adapter takes its socket path in `args` — write \"{socket}\" where the path goes",
        ),
        (
            "languages = [\"rust\"]\ncommand = \"x\"\ntransport = \"socket\"\nargs = [\"--listen={socket}\", \"{host}\"]\nlaunch = {}",
            "11:8: error: debug adapter `a`: `{port}` and `{host}` are only for a `tcp` adapter",
        ),
        (
            "languages = [\"rust\"]\ncommand = \"x\"\ntransport = \"tcp\"\nargs = [\"{port}\", \"{socket}\"]\nlaunch = {}",
            "11:8: error: debug adapter `a`: `{socket}` is only for a `socket` adapter",
        ),
        (
            "languages = [\"java\"]\nstart = { lsp = \"jdtls\", command = \"c\" }\ntransport = \"socket\"\nattach = {}",
            "10:13: error: debug adapter `a`: `transport` must be `tcp` with `start` — the language server answers a port",
        ),
        (
            "languages = [\"rust\"]\ncommand = \"x\"\nlaunch = \"{program}\"",
            "10:10: error: debug adapter `a`: `launch` must be a table",
        ),
        (
            "languages = [\"java\"]\ncommand = \"x\"\nstart = { lsp = \"jdtls\", command = \"c\" }\nattach = {}",
            "7:29: error: debug adapter `a`: `command` cannot be used with `start` — a language server opens the adapter's port, there is no adapter process",
        ),
        (
            "languages = [\"java\"]\nstart = { lsp = \"jdtls\", command = \"c\" }\ntransport = \"stdio\"\nattach = {}",
            "10:13: error: debug adapter `a`: `transport` must be `tcp` with `start` — the language server answers a port",
        ),
        (
            "languages = [\"java\"]\nstart = { lsp = \"jdtls\", command = \"c\", bundles = [\"x.jar\"] }\nattach = {}",
            "9:51: error: debug adapter `a`: `start.bundles` names files of the `mason` package — set `mason`",
        ),
        (
            "languages = [\"java\"]\nmason = \"m\"\nstart = { lsp = \"jdtls\", command = \"c\", bundles = [\"../x.jar\"] }\nattach = {}",
            "10:52: error: debug adapter `a`: `start.bundles`: `../x.jar` points outside the install folder",
        ),
        (
            "languages = [\"java\"]\nstart = { lsp = \"Jdtls\", command = \"c\" }\nattach = {}",
            "9:17: error: debug adapter `a`: `start.lsp` `Jdtls` must be a language server id",
        ),
        (
            "languages = [\"java\"]\nstart = { lsp = \"jdtls\", command = \"vscode java\" }\nattach = {}",
            "9:36: error: debug adapter `a`: `start.command` `vscode java` must be one command name",
        ),
    ];
    let mut wrong = Vec::new();
    for (body, expected) in cases {
        let (manifest, diagnostics) = parse(&format!("\n[contributes.debug_adapters.a]\n{body}\n"));
        assert!(manifest.is_none(), "{body}");
        let errors = errors(&diagnostics);
        if errors != [format!("{FILE}:{expected}")] {
            wrong.push(format!("{body:?}\n  want {expected}\n  got  {errors:?}"));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// 틀 채우기: 글 전체가 자리면 값이 형 그대로(없으면 그 키 · 칸이 빠진다), 글 안의 자리는 글로 잇는다(없으면 오류).
#[test]
fn fills_a_request_template_from_the_run_configuration() {
    let template = json!({
        "type": "pwa-node",
        "program": "{program}",
        "args": "{args}",
        "port": "{port}",
        "runtimeArgs": ["--inspect={port}", "{extra}"],
        "cwd": "${workspaceFolder}/{cwd}",
        "env": "{env}",
        "stopOnEntry": false,
    });
    assert_eq!(
        template_slots(&template).into_iter().collect::<Vec<_>>(),
        ["args", "cwd", "env", "extra", "port", "program"]
    );
    let values: BTreeMap<String, Json> = [
        ("program", json!("src/main.js")),
        ("args", json!(["--a", "b"])),
        ("port", json!(9229)),
        ("cwd", json!("app")),
        ("env", Json::Null),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v))
    .collect();
    assert_eq!(
        fill_request(&template, &values).unwrap(),
        json!({
            "type": "pwa-node",
            "program": "src/main.js",
            "args": ["--a", "b"],
            "port": 9229,
            "runtimeArgs": ["--inspect=9229"],
            "cwd": "${workspaceFolder}/app",
            "stopOnEntry": false,
        })
    );
    let mut missing = values.clone();
    missing.remove("port");
    assert_eq!(
        fill_request(&template, &missing),
        Err("`{port}` in \"--inspect={port}\" has no value".to_owned())
    );
    let mut table = values;
    table.insert("cwd".into(), json!({"a": 1}));
    assert_eq!(
        fill_request(&template, &table),
        Err(
            "`{cwd}` in \"${workspaceFolder}/{cwd}\" must be a string, a number or a boolean"
                .to_owned()
        )
    );
}

/// `transport = "socket"`: `args` 의 `{socket}` 을 Pitwall 이 고른 소켓 경로(Windows 는 named pipe 이름)로 채운다(M6-15 리뷰 판정).
#[test]
fn reads_a_socket_adapter_and_fills_its_socket_path() {
    let (manifest, diagnostics) = parse(
        "\n[contributes.debug_adapters.sock]\nlanguages = [\"rust\"]\ncommand = \"x\"\ntransport = \"socket\"\nargs = [\"--listen\", \"unix:{socket}\"]\nlaunch = {}\n",
    );
    assert_eq!(diagnostics, vec![]);
    let adapter = manifest.unwrap().contributes.debug_adapters.remove(0);
    assert_eq!(adapter.transport, DapTransport::Socket);
    assert_eq!(
        adapter.args_for_socket("/tmp/pw-0700/dap.sock"),
        ["--listen", "unix:/tmp/pw-0700/dap.sock"]
    );
}
