//! 실행 파일로 확인하는 명령줄 동작. GUI 를 띄우지 않는 경로만 둔다.
//!
//! `pitwall mcp` 를 MCP 클라이언트처럼 stdio 로 대화해 본다(앱이 꺼진 경우 = 헤드리스).
//!
//! 지키는 것: stdout 에는 MCP 프로토콜 줄만 나온다(로그 · 경고는 stderr · 로그 파일), 앱을 띄우지 않는다, 레지스트리 파일을
//! 고치지 않는다, 클라이언트가 stdin 을 닫으면 0 으로 끝난다.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use pw_core::EventBus;
use pw_core::config::AppPaths;
use pw_core::registry::{NewProject, Registry};
use serde_json::{Value, json};

struct Mcp {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: mpsc::Receiver<String>,
    stdout: Vec<String>,
}

impl Mcp {
    fn spawn(dir: &Path, args: &[&str], cwd: &Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_pitwall"))
            .arg("mcp")
            .args(args)
            .current_dir(cwd)
            .env("PITWALL_CONFIG_DIR", dir.join("config"))
            .env("PITWALL_DATA_DIR", dir.join("data"))
            .env("PITWALL_STATE_DIR", dir.join("state"))
            .env("PITWALL_CACHE_DIR", dir.join("cache"))
            // 혹시 분기가 깨져 앱이 떠도 사람의 앱 · 소켓에 닿지 않게 키를 이 실행의 것으로 가른다.
            .env(
                "PITWALL_TEST_INSTANCE",
                format!("mcp{}", std::process::id()),
            )
            .env("PITWALL_DETACHED", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { return };
                if tx.send(line).is_err() {
                    return;
                }
            }
        });
        Self {
            stdin: child.stdin.take(),
            child,
            lines,
            stdout: Vec::new(),
        }
    }

    fn send(&mut self, message: Value) {
        let stdin = self.stdin.as_mut().unwrap();
        writeln!(stdin, "{message}").unwrap();
        stdin.flush().unwrap();
    }

    /// 다음 응답(그 사이의 알림은 건너뛴다). stdout 의 모든 줄은 [`Self::stdout`] 에 쌓인다.
    fn response(&mut self, id: i64) -> Value {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let line = self
                .lines
                .recv_timeout(left)
                .unwrap_or_else(|_| panic!("no response {id}; stdout so far: {:?}", self.stdout));
            self.stdout.push(line.clone());
            let value: Value = serde_json::from_str(&line)
                .unwrap_or_else(|e| panic!("stdout 에 MCP 가 아닌 줄: {line:?} ({e})"));
            if value["id"] == json!(id) {
                return value;
            }
        }
    }

    fn call(&mut self, id: i64, method: &str, params: Value) -> Value {
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        self.response(id)
    }

    /// stdin 을 닫고 끝나기를 기다린다. 남은 stdout 줄도 모은다.
    fn finish(mut self) -> (Option<i32>, Vec<String>, String) {
        drop(self.stdin.take());
        let deadline = Instant::now() + Duration::from_secs(20);
        let status = loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                break status;
            }
            if Instant::now() > deadline {
                self.child.kill().unwrap();
                panic!("stdin 을 닫았는데 `pitwall mcp` 가 끝나지 않았다");
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        while let Ok(line) = self.lines.recv_timeout(Duration::from_millis(500)) {
            self.stdout.push(line);
        }
        let mut stderr = String::new();
        self.child
            .stderr
            .take()
            .unwrap()
            .read_to_string(&mut stderr)
            .unwrap();
        (status.code(), self.stdout, stderr)
    }
}

fn initialize(mcp: &mut Mcp) -> Value {
    let init = mcp.call(
        1,
        "initialize",
        json!({
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": {"name": "pitwall-cli-test", "version": "0"}
        }),
    );
    mcp.send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
    init
}

/// 실제 경로. Windows 의 canonicalize 는 verbatim 접두(`\\?\`)를 붙인다 — 레지스트리가 저장하는 꼴로 맞춘다.
fn real(path: &std::path::Path) -> std::path::PathBuf {
    pw_core::registry::path::simplify(std::fs::canonicalize(path).unwrap())
}

#[test]
fn headless_mcp_serves_read_tools_over_clean_stdio() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = real(tmp.path());
    let paths = AppPaths::under(&dir);
    let registry = Registry::new(paths.clone(), None, EventBus::new(16));
    let mut ids = Vec::new();
    for name in ["alpha", "beta", "gamma"] {
        let root = dir.join("work").join(name);
        std::fs::create_dir_all(&root).unwrap();
        ids.push(
            registry
                .create(NewProject {
                    root,
                    ..NewProject::default()
                })
                .unwrap()
                .id,
        );
    }
    drop(registry);
    let db = paths.data.join("state.db");
    let before = std::fs::read(&db).unwrap();

    let mut mcp = Mcp::spawn(&dir, &[], &dir.join("work/beta"));
    let init = initialize(&mut mcp);
    assert_eq!(
        init["result"]["serverInfo"]["name"],
        json!("pitwall"),
        "{init}"
    );
    assert_eq!(
        init["result"]["capabilities"]["tools"]["listChanged"],
        json!(true)
    );

    let tools = mcp.call(2, "tools/list", json!({}));
    let names: Vec<_> = tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(names, ["project_info", "project_list"]);

    let list = mcp.call(
        3,
        "tools/call",
        json!({"name": "project_list", "arguments": {}}),
    );
    let text = list["result"]["content"][0]["text"].as_str().unwrap();
    let projects: Vec<Value> = serde_json::from_str(text).unwrap();
    assert_eq!(projects.len(), 3, "{list}");

    // cwd(work/beta)가 세션 프로젝트다.
    let info = mcp.call(
        4,
        "tools/call",
        json!({"name": "project_info", "arguments": {}}),
    );
    assert_eq!(
        info["result"]["structuredContent"]["project"]["id"],
        json!(ids[1]),
        "{info}"
    );

    let (code, stdout, stderr) = mcp.finish();
    assert_eq!(code, Some(0), "stderr: {stderr}");
    for line in &stdout {
        let value: Value = serde_json::from_str(line).unwrap();
        assert_eq!(value["jsonrpc"], json!("2.0"), "{line}");
    }
    assert!(!stderr.contains("panicked"), "{stderr}");
    assert_eq!(
        std::fs::read(&db).unwrap(),
        before,
        "헤드리스가 레지스트리 파일을 고쳤다"
    );
    let data: Vec<_> = std::fs::read_dir(&paths.data)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(data, [std::ffi::OsString::from("state.db")]);
    assert!(
        paths.state.join("logs").read_dir().unwrap().any(|e| e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("mcp.")),
        "로그는 mcp.<날짜>.log 로 간다"
    );
}

#[test]
fn a_bad_argument_is_a_usage_error_without_stdout() {
    let tmp = tempfile::tempdir().unwrap();
    let mcp = Mcp::spawn(tmp.path(), &["--verbose"], tmp.path());
    let (code, stdout, stderr) = mcp.finish();
    assert_eq!(code, Some(2));
    assert!(stdout.is_empty(), "{stdout:?}");
    assert!(stderr.contains("usage: pitwall mcp"), "{stderr}");
}

/// 앱 없이 뜬 `pitwall mcp` 는 레지스트리를 소유하지 않는다 — 손상된 파일을 앱처럼 옮겨 복구하지 않고 빈 목록으로 답한다.
#[test]
fn headless_mcp_leaves_a_corrupt_registry_alone() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = real(tmp.path());
    let data = dir.join("data");
    std::fs::create_dir_all(&data).unwrap();
    std::fs::write(data.join("state.db"), b"not a database").unwrap();

    let mut mcp = Mcp::spawn(&dir, &[], &dir);
    initialize(&mut mcp);
    let list = mcp.call(
        2,
        "tools/call",
        json!({"name": "project_list", "arguments": {}}),
    );
    assert_eq!(list["result"]["content"][0]["text"], json!("[]"), "{list}");
    let (code, _, stderr) = mcp.finish();
    assert_eq!(code, Some(0), "stderr: {stderr}");
    assert_eq!(
        std::fs::read(data.join("state.db")).unwrap(),
        b"not a database"
    );
    let entries: Vec<_> = std::fs::read_dir(&data)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(entries, [std::ffi::OsString::from("state.db")]);
}
