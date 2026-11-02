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
    // 크기가 있는 채널 — 쏟기 동작이 가짜 서버 자신의 메모리를 끝없이 쓰지 않게 stdout 에 맞춰 기다린다.
    out: mpsc::Sender<Vec<u8>>,
    next_id: AtomicU64,
    /// `Hang` 뒤로 stdin 을 읽지 않는다.
    hung: std::sync::atomic::AtomicBool,
    /// 메서드마다 받은 요청 수(`sequence` 의 차례).
    asked: std::sync::Mutex<std::collections::HashMap<String, usize>>,
}

impl Fake {
    fn record(&self, line: &Value) {
        let Some(path) = &self.scenario.record else {
            return;
        };
        // 한 줄을 한 번의 `write` 로 — 같은 기록 파일을 쓰는 가짜 서버 여럿(인스턴스 여럿)의 줄이 섞이지 않게(O_APPEND).
        let mut text = line.to_string();
        text.push('\n');
        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
            let _ = file.write_all(text.as_bytes());
        }
    }

    async fn frame(&self, body: &[u8]) {
        let mut bytes = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
        bytes.extend_from_slice(body);
        let _ = self.out.send(bytes).await;
    }

    async fn act(&self, actions: &[Action]) {
        for action in actions {
            match action {
                Action::Notify { method, params } => {
                    self.frame(&message::notification(method, params)).await;
                }
                Action::Request { method, params } => {
                    let n = self.next_id.fetch_add(1, Ordering::SeqCst);
                    let id = RequestId::String(format!("fake-{n}"));
                    self.frame(&message::request(&id, method, params)).await;
                }
                Action::Stderr { text } => {
                    eprintln!("{text}");
                }
                Action::Raw { text } => {
                    let _ = self.out.send(text.as_bytes().to_vec()).await;
                }
                Action::Sleep { ms } => tokio::time::sleep(Duration::from_millis(*ms)).await,
                Action::Exit { code } => {
                    // 보낼 것을 먼저 내보낼 틈을 준다.
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    std::process::exit(*code);
                }
                Action::SpawnChild { seconds } => self.spawn_child(*seconds),
                Action::StderrFlood { bytes, newline } => {
                    use std::io::Write as _;
                    let chunk = vec![b'a'; 64 * 1024];
                    let mut stderr = std::io::stderr().lock();
                    let mut left = *bytes;
                    while left > 0 {
                        let n =
                            usize::try_from(left.min(chunk.len() as u64)).unwrap_or(chunk.len());
                        if stderr.write_all(&chunk[..n]).is_err() {
                            break;
                        }
                        left -= n as u64;
                    }
                    if *newline {
                        let _ = stderr.write_all(b"\n");
                    }
                    let _ = stderr.flush();
                }
                Action::Flood {
                    method,
                    count,
                    id_bytes,
                } => {
                    let pad = "i".repeat(*id_bytes);
                    for n in 0..*count {
                        let id = RequestId::String(format!("{n}-{pad}"));
                        self.frame(&message::request(&id, method, &Value::Null))
                            .await;
                    }
                }
                Action::Hang => self.hung.store(true, Ordering::SeqCst),
            }
        }
    }

    fn spawn_child(&self, seconds: u64) {
        // 자기 자신을 `--sleep` 으로 띄운다 — `/bin/sleep` 같은 플랫폼 바이너리는 macOS 가 환경을 가려 거두기의 표지를 읽을 수
        // 없다(`pw_lsp::ledger` 머리 주석). 실 서버의 손자(node · java · 빌드 도구)처럼 환경을 읽을 수 있는 자식이다.
        let Ok(me) = std::env::current_exe() else {
            eprintln!("pw-lsp-fake: cannot find itself");
            return;
        };
        let mut command = std::process::Command::new(me);
        command.args(["--sleep", &seconds.to_string()]);
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
        let nth = {
            let mut asked = self
                .asked
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let n = asked.entry(method.clone()).or_default();
            *n += 1;
            *n - 1
        };
        let reply = self
            .scenario
            .sequence
            .get(&method)
            .and_then(|replies| replies.get(nth))
            .or_else(|| self.scenario.replies.get(&method))
            .cloned();
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
            fake.frame(&message::response(&id, &result)).await;
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
    // `pw-lsp-fake --sleep <초>`: 그만큼 자고 끝난다(`Action::SpawnChild` 의 자식).
    let mut args = std::env::args();
    if args.nth(1).as_deref() == Some("--sleep") {
        let seconds = args.next().and_then(|s| s.parse().ok()).unwrap_or(0);
        std::thread::sleep(Duration::from_secs(seconds));
        return ExitCode::SUCCESS;
    }
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
    let (out, mut out_rx) = mpsc::channel::<Vec<u8>>(256);
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
        hung: std::sync::atomic::AtomicBool::new(false),
        asked: std::sync::Mutex::default(),
    });
    fake.act(&fake.scenario.start.clone()).await;
    let mut reader = FrameReader::new(tokio::io::stdin());
    loop {
        if fake.hung.load(Ordering::SeqCst) {
            // 읽지 않는다 — 쓰기 태스크(쏟기)는 계속 돈다.
            std::future::pending::<()>().await;
        }
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
