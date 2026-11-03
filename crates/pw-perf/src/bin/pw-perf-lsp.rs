//! `pw-perf-lsp <config.json>`: LSP 측정용 가짜 언어 서버(동작은 `pw_perf::lsp_fake`). 측정 하네스가 회마다 전역 설정
//! `[lsp.rust-analyzer] path` 로 이 실행 파일을 끼운다(`pw_perf::lsp`). 앱 번들에 들지 않는다.

use std::io::{BufRead, BufReader, Write};
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use pw_perf::lsp_fake::{Config, Out, Server};

fn epoch_ms() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64() * 1000.0)
}

/// 틀 하나(`Content-Length` 머리 + 본문). 끝이면 `None`.
fn read_frame(input: &mut impl BufRead) -> Option<Vec<u8>> {
    let mut length = None;
    loop {
        let mut line = String::new();
        if input.read_line(&mut line).ok()? == 0 {
            return None;
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some(v) = line.strip_prefix("Content-Length:") {
            length = v.trim().parse::<usize>().ok();
        }
    }
    let mut body = vec![0; length?];
    input.read_exact(&mut body).ok()?;
    Some(body)
}

fn main() -> ExitCode {
    let Some(path) = std::env::args_os().nth(1) else {
        eprintln!("usage: pw-perf-lsp <config.json>");
        return ExitCode::from(2);
    };
    let config: Config = match std::fs::read(&path)
        .map_err(|e| e.to_string())
        .and_then(|b| serde_json::from_slice(&b).map_err(|e| e.to_string()))
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("pw-perf-lsp: cannot read {}: {e}", path.to_string_lossy());
            return ExitCode::from(2);
        }
    };
    let mut stamps = config.stamps.as_ref().and_then(|p| {
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(p)
            .ok()
    });
    let mut server = Server::new(config);
    let mut input = BufReader::new(std::io::stdin().lock());
    let mut output = std::io::stdout().lock();
    while let Some(body) = read_frame(&mut input) {
        let Ok(message) = serde_json::from_slice(&body) else {
            continue;
        };
        for out in server.handle(&message) {
            match out {
                Out::Message(value) => {
                    let body = value.to_string();
                    let frame = format!("Content-Length: {}\r\n\r\n{body}", body.len());
                    if output.write_all(frame.as_bytes()).is_err() || output.flush().is_err() {
                        return ExitCode::SUCCESS;
                    }
                }
                Out::Stamp(mut value) => {
                    if let Some(file) = &mut stamps {
                        value["epochMs"] = serde_json::json!(epoch_ms());
                        // 한 줄을 한 번의 write 로(O_APPEND).
                        let _ = file.write_all(format!("{value}\n").as_bytes());
                    }
                }
            }
        }
        if server.exit {
            break;
        }
    }
    ExitCode::SUCCESS
}
