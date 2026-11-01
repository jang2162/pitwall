//! 시험용 가짜 HTTP 서버(`http://127.0.0.1:<포트>`). 경로마다 정한 답을 준다 — 요청 수를 센다.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

#[derive(Clone)]
pub enum Reply {
    /// 200 과 본문(`Content-Length` 있음).
    Body(Vec<u8>),
    /// 200 과 본문을 `Content-Length` 없이(연결을 닫아 끝낸다).
    Unsized(Vec<u8>),
    /// 상태 코드만.
    Status(u16),
    /// 3xx 와 `Location`.
    Redirect(String),
    /// 답하지 않고 기다린다.
    Hang,
}

pub struct FakeServer {
    pub base: String,
    routes: Arc<Mutex<HashMap<String, Reply>>>,
    hits: Arc<Mutex<HashMap<String, usize>>>,
    total: Arc<AtomicUsize>,
}

impl FakeServer {
    pub async fn start() -> FakeServer {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let routes: Arc<Mutex<HashMap<String, Reply>>> = Arc::default();
        let hits: Arc<Mutex<HashMap<String, usize>>> = Arc::default();
        let total = Arc::new(AtomicUsize::new(0));
        let (r, h, t) = (Arc::clone(&routes), Arc::clone(&hits), Arc::clone(&total));
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    return;
                };
                let (r, h, t) = (Arc::clone(&r), Arc::clone(&h), Arc::clone(&t));
                tokio::spawn(async move {
                    let mut buf = Vec::new();
                    let mut chunk = [0u8; 1024];
                    while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                        match sock.read(&mut chunk).await {
                            Ok(0) | Err(_) => return,
                            Ok(n) => buf.extend_from_slice(&chunk[..n]),
                        }
                    }
                    let head = String::from_utf8_lossy(&buf);
                    let path = head.split_whitespace().nth(1).unwrap_or("/").to_owned();
                    t.fetch_add(1, Ordering::SeqCst);
                    *h.lock().unwrap().entry(path.clone()).or_default() += 1;
                    let reply = r
                        .lock()
                        .unwrap()
                        .get(&path)
                        .cloned()
                        .unwrap_or(Reply::Status(404));
                    let out = match reply {
                        Reply::Body(body) => {
                            let mut out = format!(
                                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                                body.len()
                            )
                            .into_bytes();
                            out.extend(body);
                            out
                        }
                        Reply::Unsized(body) => {
                            let mut out = b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n".to_vec();
                            out.extend(body);
                            out
                        }
                        Reply::Status(code) => format!(
                            "HTTP/1.1 {code} X\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                        )
                        .into_bytes(),
                        Reply::Redirect(to) => format!(
                            "HTTP/1.1 302 Found\r\nLocation: {to}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                        )
                        .into_bytes(),
                        Reply::Hang => {
                            tokio::time::sleep(Duration::from_secs(3600)).await;
                            return;
                        }
                    };
                    let _ = sock.write_all(&out).await;
                    let _ = sock.shutdown().await;
                });
            }
        });
        FakeServer {
            base,
            routes,
            hits,
            total,
        }
    }

    pub fn route(&self, path: &str, reply: Reply) {
        self.routes.lock().unwrap().insert(path.to_owned(), reply);
    }

    pub fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    pub fn hits(&self, path: &str) -> usize {
        self.hits.lock().unwrap().get(path).copied().unwrap_or(0)
    }

    pub fn total(&self) -> usize {
        self.total.load(Ordering::SeqCst)
    }
}

/// `registry.json` 하나를 담은 zip.
pub fn zip_of(entries: &[(&str, &[u8])]) -> Vec<u8> {
    use std::io::Write as _;
    let mut out = std::io::Cursor::new(Vec::new());
    {
        let mut w = zip::ZipWriter::new(&mut out);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for (name, body) in entries {
            w.start_file(*name, opts).unwrap();
            w.write_all(body).unwrap();
        }
        w.finish().unwrap();
    }
    out.into_inner()
}
