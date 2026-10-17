//! 실행 파일로 확인하는 명령줄 동작. GUI 를 띄우지 않는 경로만 둔다.

use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// 실행해 끝을 기다린다. 끝나지 않으면(분기가 깨져 앱이 떴다) 죽이고 실패한다.
///
/// `PITWALL_DETACHED` 를 줘 떼어 띄운 자식으로 보이게 한다 — 분기가 깨져도 앱이 이 시험 밖으로 떨어져 나가 남지 않고, 이
/// 자식 pid 로 끌 수 있다(src/detach.rs).
fn run(args: &[&str], dir: &std::path::Path) -> (Option<i32>, Vec<u8>, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_pitwall"))
        .args(args)
        .env("PITWALL_CONFIG_DIR", dir.join("config"))
        .env("PITWALL_DATA_DIR", dir.join("data"))
        .env("PITWALL_STATE_DIR", dir.join("state"))
        .env("PITWALL_CACHE_DIR", dir.join("cache"))
        .env("PITWALL_DETACHED", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() > deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("`pitwall {}` 이 끝나지 않았다 — 앱이 떴다", args.join(" "));
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let mut stdout = Vec::new();
    child
        .stdout
        .take()
        .unwrap()
        .read_to_end(&mut stdout)
        .unwrap();
    let mut stderr = String::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();
    (status.code(), stdout, stderr)
}

/// `pitwall mcp`(M0-17 전의 자리): 앱을 띄우지 않고 2 로 끝나며 stdout 에 아무것도 쓰지 않는다(MCP 가 stdio 를 쓴다).
/// 로그 · 데이터 폴더도 만들지 않는다 — `main()` 의 다른 일보다 먼저 가른다.
#[test]
fn mcp_exits_without_starting_the_app_or_writing_stdout() {
    for args in [&["mcp"][..], &["mcp", "--project", "/x"][..]] {
        let tmp = tempfile::tempdir().unwrap();
        let (code, stdout, stderr) = run(args, tmp.path());
        assert_eq!(code, Some(2), "{args:?}");
        assert!(stdout.is_empty(), "{args:?}: {stdout:?}");
        assert_eq!(stderr.trim(), "pitwall mcp: not implemented yet");
        assert_eq!(
            std::fs::read_dir(tmp.path()).unwrap().count(),
            0,
            "아무 폴더도 만들지 않는다"
        );
    }
}
