//! `pitwall mcp` 의 stdio ↔ 앱 소켓 프록시. MCP stdio 는 줄 단위 JSON-RPC 라 줄을 그대로 옮긴다.
//!
//! 내용을 고치지 않는다. 다만 응답을 기다리는 요청의 id 는 적어 둔다 — 앱이 도중에 꺼지면(소켓이 닫히면) 그 요청마다 오류
//! 응답([`APP_CLOSED`])을 stdout 에 써서 클라이언트가 영원히 기다리지 않게 하고 [`ProxyEnd::AppClosed`] 로 끝난다.
//! 클라이언트가 stdin 을 닫으면 소켓의 쓰기 쪽을 닫아 앱 쪽 세션을 끝내고, 앱이 소켓을 닫을 때까지(최대 [`DRAIN`]) 남은 응답을
//! 옮긴 뒤 [`ProxyEnd::ClientClosed`] 로 끝난다.

use std::collections::HashMap;
use std::io;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};

/// 앱이 닫혔을 때 기다리던 요청에 돌려주는 JSON-RPC 오류 코드(서버 정의 영역).
pub const APP_CLOSED: i64 = -32001;
/// 클라이언트가 닫은 뒤 앱의 남은 응답을 기다리는 시간.
pub const DRAIN: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProxyEnd {
    /// 클라이언트가 stdin 을 닫았다. 정상 종료.
    ClientClosed,
    /// 앱이 먼저 소켓을 닫았다. `pending` 개의 요청에 오류 응답을 써 주었다.
    AppClosed { pending: usize },
}

type Pending = Arc<Mutex<HashMap<String, Value>>>;

pub async fn run<I, O, R, W>(
    stdin: I,
    stdout: &mut O,
    from_app: R,
    mut to_app: W,
) -> io::Result<ProxyEnd>
where
    I: AsyncRead + Unpin,
    O: AsyncWrite + Unpin,
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let pending: Pending = Arc::default();
    let up_pending = Arc::clone(&pending);
    let up = async move {
        let mut stdin = BufReader::new(stdin);
        let mut line = Vec::new();
        loop {
            line.clear();
            if stdin.read_until(b'\n', &mut line).await? == 0 {
                // 앱 쪽 세션이 읽기 끝(EOF)을 보고 닫힌다.
                to_app.shutdown().await?;
                return Ok::<_, io::Error>(());
            }
            if let Some((key, id)) = request_id(&line) {
                lock(&up_pending).insert(key, id);
            }
            if line.last() != Some(&b'\n') {
                line.push(b'\n');
            }
            to_app.write_all(&line).await?;
            to_app.flush().await?;
        }
    };
    let down_pending = Arc::clone(&pending);
    let ended = {
        let down = async {
            let mut from_app = BufReader::new(from_app);
            let mut line = Vec::new();
            loop {
                line.clear();
                if from_app.read_until(b'\n', &mut line).await? == 0 {
                    return Ok::<_, io::Error>(());
                }
                if let Some(key) = response_id(&line) {
                    lock(&down_pending).remove(&key);
                }
                stdout.write_all(&line).await?;
                stdout.flush().await?;
            }
        };
        let drain = tokio::time::sleep(DRAIN);
        tokio::pin!(up, down, drain);
        let mut client_closed = false;
        loop {
            tokio::select! {
                result = &mut up, if !client_closed => {
                    // 앱에 쓰기가 실패해도(앱이 닫힘) 아래 down 이 EOF 로 끝나 AppClosed 로 판정한다.
                    if let Err(e) = result {
                        tracing::debug!(%e, "mcp proxy: writing to the app failed");
                    }
                    client_closed = true;
                    drain.as_mut().reset(tokio::time::Instant::now() + DRAIN);
                }
                result = &mut down => {
                    result?;
                    break client_closed;
                }
                () = &mut drain, if client_closed => break true,
            }
        }
    };
    if ended {
        return Ok(ProxyEnd::ClientClosed);
    }
    let waiting: Vec<Value> = lock(&pending).drain().map(|(_, id)| id).collect();
    for id in &waiting {
        let mut line = serde_json::to_vec(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {"code": APP_CLOSED, "message": "the Pitwall app closed the connection"},
        }))
        .expect("JSON 값은 직렬화된다");
        line.push(b'\n');
        stdout.write_all(&line).await?;
    }
    stdout.flush().await?;
    Ok(ProxyEnd::AppClosed {
        pending: waiting.len(),
    })
}

/// 응답을 기다리는 요청(`id` 와 `method` 가 있는 메시지)의 id.
fn request_id(line: &[u8]) -> Option<(String, Value)> {
    let value: Value = serde_json::from_slice(line).ok()?;
    let id = value.get("id")?;
    if value.get("method").is_none() || id.is_null() {
        return None;
    }
    Some((id.to_string(), id.clone()))
}

/// 응답(`id` 가 있고 `method` 가 없는 메시지)의 id.
fn response_id(line: &[u8]) -> Option<String> {
    let value: Value = serde_json::from_slice(line).ok()?;
    if value.get("method").is_some() {
        return None;
    }
    value.get("id").map(Value::to_string)
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncReadExt, duplex};

    use super::*;

    /// 앱이 응답 하나를 보낸 뒤 닫히면, 남은 요청 하나에 오류 응답이 나가고 AppClosed 로 끝난다.
    #[tokio::test]
    async fn app_closing_mid_session_answers_pending_requests() {
        let (mut client, stdin) = duplex(4096);
        let (stdout_w, mut stdout_r) = duplex(4096);
        let (app_side, proxy_side) = duplex(4096);
        let (from_app, to_app) = tokio::io::split(proxy_side);
        let (mut app_r, mut app_w) = tokio::io::split(app_side);
        let proxy = tokio::spawn(async move {
            let mut stdout_w = stdout_w;
            run(stdin, &mut stdout_w, from_app, to_app).await
        });
        client
            .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"a\"}\n{\"jsonrpc\":\"2.0\",\"id\":\"x\",\"method\":\"b\"}\n{\"jsonrpc\":\"2.0\",\"method\":\"note\"}\n")
            .await
            .unwrap();
        let mut seen = vec![0u8; 0];
        let mut buf = [0u8; 256];
        while seen.iter().filter(|b| **b == b'\n').count() < 3 {
            let n = app_r.read(&mut buf).await.unwrap();
            seen.extend_from_slice(&buf[..n]);
        }
        app_w
            .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}\n")
            .await
            .unwrap();
        drop(app_w);
        drop(app_r);
        let end = proxy.await.unwrap().unwrap();
        assert_eq!(end, ProxyEnd::AppClosed { pending: 1 });
        let mut out = String::new();
        stdout_r.read_to_string(&mut out).await.unwrap();
        let lines: Vec<Value> = out
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(lines.len(), 2, "{out}");
        assert_eq!(lines[0]["id"], json!(1));
        assert_eq!(lines[1]["id"], json!("x"));
        assert_eq!(lines[1]["error"]["code"], json!(APP_CLOSED));
    }

    /// 클라이언트가 닫으면 앱 쪽 쓰기가 닫히고(앱이 EOF 를 본다) 정상 종료다.
    #[tokio::test]
    async fn client_closing_shuts_the_app_side_and_ends_cleanly() {
        let (client, stdin) = duplex(4096);
        let (stdout_w, _stdout_r) = duplex(4096);
        let (app_side, proxy_side) = duplex(4096);
        let (from_app, to_app) = tokio::io::split(proxy_side);
        let (mut app_r, app_w) = tokio::io::split(app_side);
        let proxy = tokio::spawn(async move {
            let mut stdout_w = stdout_w;
            run(stdin, &mut stdout_w, from_app, to_app).await
        });
        drop(client);
        let mut rest = Vec::new();
        app_r.read_to_end(&mut rest).await.unwrap();
        assert!(rest.is_empty());
        drop((app_r, app_w));
        let started = std::time::Instant::now();
        assert_eq!(proxy.await.unwrap().unwrap(), ProxyEnd::ClientClosed);
        assert!(
            started.elapsed() < DRAIN,
            "앱이 닫았는데 DRAIN 을 다 기다렸다"
        );
    }
}
