//! 시험 전용 가짜 언어 서버(15-m4-plan.md M4-01 ⑥). `pw-lsp-fake <시나리오.json>` — 형식은 `pw_lsp::fake` 머리 주석. 배포물에
//! 들지 않는다.

use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::PathBuf;
use std::process::{ExitCode, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use pw_lsp::fake::{Action, Reply, Scenario};
use pw_lsp::message::{self, Incoming, RequestId, ResponseError, code};
use pw_lsp::transport::FrameReader;
use serde_json::{Value, json};
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;

struct Fake {
    scenario: Scenario,
    out: mpsc::UnboundedSender<Vec<u8>>,
    next_id: AtomicU64,
}

impl Fake {
    fn record(&self, line: &Value) {
        let Some(path) = &self.scenario.record else {
            return;
        };
        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
            let _ = writeln!(file, "{line}");
        }
    }

    fn frame(&self, body: &[u8]) {
        let mut bytes = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
        bytes.extend_from_slice(body);
        let _ = self.out.send(bytes);
    }

    async fn act(&self, actions: &[Action]) {
        for action in actions {
            match action {
                Action::Notify { method, params } => {
                    self.frame(&message::notification(method, params));
                }
                Action::Request { method, params } => {
                    let n = self.next_id.fetch_add(1, Ordering::SeqCst);
                    let id = RequestId::String(format!("fake-{n}"));
                    self.frame(&message::request(&id, method, params));
                }
                Action::Stderr { text } => {
                    eprintln!("{text}");
                }
                Action::Raw { text } => {
                    let _ = self.out.send(text.as_bytes().to_vec());
                }
                Action::Sleep { ms } => tokio::time::sleep(Duration::from_millis(*ms)).await,
                Action::Exit { code } => {
                    // 보낼 것을 먼저 내보낼 틈을 준다.
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    std::process::exit(*code);
                }
                Action::SpawnChild { seconds } => self.spawn_child(*seconds),
            }
        }
    }

    fn spawn_child(&self, seconds: u64) {
        #[cfg(unix)]
        let mut command = {
            let mut c = std::process::Command::new("sleep");
            c.arg(seconds.to_string());
            c
        };
        #[cfg(windows)]
        let mut command = {
            let mut c = std::process::Command::new("ping");
            c.args(["-n", &(seconds + 1).to_string(), "127.0.0.1"]);
            c
        };
        match command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(child) => self.record(&json!({ "childPid": child.id() })),
            Err(e) => eprintln!("pw-lsp-fake: cannot spawn a child: {e}"),
        }
    }

    async fn answer(self: &Arc<Self>, id: RequestId, method: String) {
        let reply = self.scenario.replies.get(&method).cloned();
        let result = match &reply {
            Some(Reply { no_reply: true, .. }) => return,
            Some(Reply {
                error: Some(error), ..
            }) => Err(serde_json::from_value::<ResponseError>(error.clone())
                .unwrap_or_else(|_| ResponseError::new(code::INTERNAL_ERROR, error.to_string()))),
            Some(Reply { result, .. }) => Ok(result.clone().unwrap_or(Value::Null)),
            None => match method.as_str() {
                "initialize" => Ok(json!({
                    "capabilities": self.scenario.capabilities,
                    "serverInfo": self.scenario.server_info.clone()
                        .unwrap_or_else(|| json!({ "name": "pw-lsp-fake", "version": "0" })),
                })),
                "shutdown" => Ok(Value::Null),
                _ => Err(ResponseError::method_not_found(&method)),
            },
        };
        let delay = reply.as_ref().map_or(0, |r| r.delay_ms);
        let then = reply.map(|r| r.then).unwrap_or_default();
        let fake = Arc::clone(self);
        let send = async move {
            if delay > 0 {
                tokio::time::sleep(Duration::from_millis(delay)).await;
            }
            fake.frame(&message::response(&id, &result));
            fake.act(&then).await;
        };
        if delay > 0 {
            tokio::spawn(send);
        } else {
            send.await;
        }
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let Some(path) = std::env::args_os().nth(1).map(PathBuf::from) else {
        eprintln!("usage: pw-lsp-fake <scenario.json>");
        return ExitCode::from(2);
    };
    let scenario = match Scenario::read(&path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("pw-lsp-fake: cannot read {}: {e}", path.display());
            return ExitCode::from(2);
        }
    };
    let (out, mut out_rx) = mpsc::unbounded_channel::<Vec<u8>>();
    let writer = tokio::spawn(async move {
        let mut stdout = tokio::io::stdout();
        while let Some(bytes) = out_rx.recv().await {
            if stdout.write_all(&bytes).await.is_err() || stdout.flush().await.is_err() {
                break;
            }
        }
    });
    let fake = Arc::new(Fake {
        scenario,
        out,
        next_id: AtomicU64::new(1),
    });
    fake.act(&fake.scenario.start.clone()).await;
    let mut reader = FrameReader::new(tokio::io::stdin());
    loop {
        let body = match reader.read_frame().await {
            Ok(Some(body)) => body,
            Ok(None) | Err(_) => break,
        };
        let value: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
        fake.record(&value);
        let Ok(incoming) = message::parse(&body) else {
            continue;
        };
        let method = match incoming {
            Incoming::Request { id, method, .. } => {
                fake.answer(id, method.clone()).await;
                method
            }
            Incoming::Notification { method, .. } => {
                if method == "exit" && !fake.scenario.ignore_exit {
                    writer.abort();
                    return ExitCode::SUCCESS;
                }
                method
            }
            Incoming::Response { .. } => continue,
        };
        if let Some(actions) = fake.scenario.on.get(&method).cloned() {
            fake.act(&actions).await;
        }
    }
    if fake.scenario.ignore_exit {
        // stdin 이 닫혀도 끝나지 않는다 — 클라이언트가 그룹째 죽일 때까지.
        std::future::pending::<()>().await;
    }
    ExitCode::SUCCESS
}
