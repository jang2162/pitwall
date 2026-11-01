//! rustup 프록시 판별(결정 M4-D7). rustup 은 컴포넌트가 없어도 `~/.cargo/bin/rust-analyzer` 프록시를 늘 깔고, 그것은 실행하면
//! 「not installed for the toolchain」 으로 끝난다 — PATH 에서 「찾음」 만 보고 고르면 서버가 곧바로 죽는다. 그래서 찾은 것이
//! 프록시면 그 디렉터리에서 `rustup which <이름>` 이 성공할 때만 쓴다.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use pw_runtime::RuntimeEnv;

/// `rustup which` 를 기다리는 시간. 넘으면 실패로 본다(프로세스는 끝낸다).
pub const WHICH_TIMEOUT: Duration = Duration::from_secs(5);

const RUSTUP: &str = if cfg!(windows) {
    "rustup.exe"
} else {
    "rustup"
};

/// `path` 가 rustup 프록시면 그 rustup 실행 파일. 프록시는 rustup 으로의 심볼릭 링크이거나 같은 폴더의 `rustup` 과 같은
/// 파일(하드 링크 — 같은 inode)이다. Windows 는 inode 를 견주지 못해 같은 폴더의 `rustup.exe` 와 크기가 같은 파일을 프록시로 본다.
pub fn proxy_of(path: &Path) -> Option<PathBuf> {
    let is_rustup = |p: &Path| {
        p.file_name()
            .is_some_and(|n| n.eq_ignore_ascii_case(RUSTUP))
    };
    if let Ok(real) = std::fs::canonicalize(path)
        && is_rustup(&real)
    {
        return Some(real);
    }
    let rustup = path.parent()?.join(RUSTUP);
    let (a, b) = (
        std::fs::metadata(path).ok()?,
        std::fs::metadata(&rustup).ok()?,
    );
    same_file(&a, &b).then_some(rustup)
}

#[cfg(unix)]
fn same_file(a: &std::fs::Metadata, b: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    a.dev() == b.dev() && a.ino() == b.ino()
}

#[cfg(not(unix))]
fn same_file(a: &std::fs::Metadata, b: &std::fs::Metadata) -> bool {
    a.is_file() && b.is_file() && a.len() == b.len()
}

/// `dir` 에서 `rustup which <command>` 를 돌려 그 툴체인의 실행 파일을 찾는다. 환경은 런타임 환경 전체에 `RUSTUP_AUTO_INSTALL=0`
/// 을 더한 것이다 — 고정된 툴체인이 없을 때 받지 않게. 실패하면 rustup 이 남긴 첫 줄(없으면 종료 코드)을 돌려준다.
pub fn which(
    rustup: &Path,
    command: &str,
    dir: &Path,
    env: &RuntimeEnv,
    timeout: Duration,
) -> Result<PathBuf, String> {
    let mut child = Command::new(rustup)
        .args(["which", command])
        .current_dir(dir)
        .env_clear()
        .envs(&env.vars)
        .env("PATH", &env.path)
        .env("RUSTUP_AUTO_INSTALL", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot run {}: {e}", rustup.display()))?;
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "`rustup which {command}` did not finish in {timeout:?}"
                ));
            }
            Err(e) => return Err(e.to_string()),
        }
    };
    let output = child.wait_with_output().map_err(|e| e.to_string())?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let found = PathBuf::from(stdout.trim());
    if status.success() && found.is_absolute() && found.is_file() {
        return Ok(found);
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(stderr
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map_or_else(
            || format!("`rustup which {command}` exited with {status}"),
            str::to_owned,
        ))
}
