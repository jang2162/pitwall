//! 띄울 어댑터와 붙는 길(17-m6-plan.md M6-15 ④).
//!
//! **stdio 를 먼저** — 어댑터가 stdio 로 DAP 를 말하면 stdin · stdout 이 프로토콜이다(debugpy 의 `debugpy.adapter`, codelldb 는
//! `--port` · `--connect` 없이 띄우면 stdio — v1.12.3 `src/codelldb/src/lib.rs` 를 확인했다).
//!
//! **TCP 는 `127.0.0.1` 에만** — 포트로만 말하는 어댑터(js-debug 의 `dapDebugServer.js [port] [host=localhost]` — v1.140.0
//! `src/dapDebugServer.ts` 를 확인했다)는 Pitwall 이 고른 빈 포트와 `127.0.0.1` 을 인자로 준다. 정의의 포트 인자 꼴
//! ([`Transport::Tcp::port_args`])에 `{port}` 와 `{host}` 자리가 **둘 다** 있어야 한다 — `{host}` 가 없는 꼴은 어댑터가 어느
//! 주소에 묶는지 Pitwall 이 정하지 못하므로 띄우지 않는다([`LaunchError::HostNotBound`]). 같은 기기의 다른 프로세스가 그 포트에
//! 붙어 `evaluate` 로 디버그 대상 안에서 코드를 돌리지 못하게 한다. 붙는 쪽도 [`LOOPBACK`] 으로만 붙는다.
//!
//! 언어 서버가 연 포트(java-debug — jdtls 의 `vscode.java.startDebugSession`)는 Pitwall 이 주소를 정하지 못한다. 그 길(M6-16 의
//! `start.lsp`)은 [`crate::DebugSession::start`] 에 [`Transport::Connect`] 로 받은 포트를 넘긴다 — 묶는 주소는 M6-19 가 실제로 본다.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fmt;
use std::net::Ipv4Addr;
use std::path::PathBuf;

use pw_runtime::RuntimeEnv;

/// 어댑터를 묶고 붙는 주소.
pub const LOOPBACK: Ipv4Addr = Ipv4Addr::LOCALHOST;
/// 포트 인자 꼴의 자리 표지.
pub const PORT_PLACEHOLDER: &str = "{port}";
pub const HOST_PLACEHOLDER: &str = "{host}";

/// 어댑터와 말하는 길.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Transport {
    /// 어댑터 프로세스의 stdin · stdout.
    Stdio,
    /// 어댑터 프로세스를 띄우고 그것이 연 포트에 붙는다. `port_args` 는 [`AdapterSpec::args`] 뒤에 붙는 인자 꼴 — `{port}` 와
    /// `{host}` 가 든다(예: js-debug 는 `["{port}", "{host}"]`).
    Tcp { port_args: Vec<String> },
    /// 이미 열린 `127.0.0.1` 의 포트에 붙기만 한다(프로세스를 띄우지 않는다 — 언어 서버가 연 포트).
    Connect { port: u16 },
}

/// 띄울 어댑터(해석 — M6-16 — 이 끝난 것).
#[derive(Debug, Clone)]
pub struct AdapterSpec {
    /// 정의의 id(로그 · `initialize` 의 `adapterID`).
    pub adapter_id: String,
    /// 실행 파일(해석이 끝난 절대 경로 — 이름만이면 [`Self::env`] 의 PATH 로 찾는다). [`Transport::Connect`] 면 쓰지 않는다.
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub transport: Transport,
    /// 작업 디렉터리(그 워크트리의 프로젝트 루트).
    pub cwd: PathBuf,
    /// 그 프로젝트 · 디렉터리의 런타임 환경. 어댑터는 이것만 받는다.
    pub env: RuntimeEnv,
    /// 런타임 환경 위에 덮을 변수(정의의 `env`).
    pub extra_env: BTreeMap<OsString, OsString>,
}

/// 어댑터를 띄우지 못한 까닭.
#[derive(Debug)]
pub enum LaunchError {
    /// TCP 포트 인자 꼴에 `{port}` 가 없다.
    PortMissing,
    /// TCP 포트 인자 꼴에 `{host}` 가 없다 — 묶는 주소를 정하지 못한다(머리 주석).
    HostNotBound,
    Io(std::io::Error),
}

impl fmt::Display for LaunchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PortMissing => {
                write!(f, "the adapter's port arguments have no {PORT_PLACEHOLDER}")
            }
            Self::HostNotBound => write!(
                f,
                "the adapter's port arguments have no {HOST_PLACEHOLDER}, so it cannot be bound to {LOOPBACK}"
            ),
            Self::Io(e) => write!(f, "cannot start the debug adapter: {e}"),
        }
    }
}

impl std::error::Error for LaunchError {}

/// TCP 어댑터의 argv(프로그램 다음): `args` 뒤에 `port_args` 의 `{port}` · `{host}` 를 채워 붙인다. `{host}` 는 늘 [`LOOPBACK`].
pub fn tcp_args(
    args: &[OsString],
    port_args: &[String],
    port: u16,
) -> Result<Vec<OsString>, LaunchError> {
    if !port_args.iter().any(|a| a.contains(PORT_PLACEHOLDER)) {
        return Err(LaunchError::PortMissing);
    }
    if !port_args.iter().any(|a| a.contains(HOST_PLACEHOLDER)) {
        return Err(LaunchError::HostNotBound);
    }
    let port = port.to_string();
    let host = LOOPBACK.to_string();
    Ok(args
        .iter()
        .cloned()
        .chain(port_args.iter().map(|a| {
            OsString::from(
                a.replace(PORT_PLACEHOLDER, &port)
                    .replace(HOST_PLACEHOLDER, &host),
            )
        }))
        .collect())
}

/// [`LOOPBACK`] 의 빈 포트 하나(묶었다 곧바로 놓는다 — 어댑터가 그 사이 다른 프로세스에 뺏기면 붙기에서 실패한다).
pub(crate) fn free_port() -> std::io::Result<u16> {
    let listener = std::net::TcpListener::bind((LOOPBACK, 0))?;
    Ok(listener.local_addr()?.port())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tcp_arguments_bind_the_loopback_address() {
        let args = tcp_args(
            &[OsString::from("dapDebugServer.js")],
            &["{port}".into(), "{host}".into()],
            41234,
        )
        .unwrap();
        assert_eq!(args, ["dapDebugServer.js", "41234", "127.0.0.1"]);
        let joined = tcp_args(&[], &["--listen={host}:{port}".into()], 7).unwrap();
        assert_eq!(joined, ["--listen=127.0.0.1:7"]);
    }

    #[test]
    fn port_arguments_without_a_host_are_refused() {
        assert!(matches!(
            tcp_args(&[], &["--port".into(), "{port}".into()], 1),
            Err(LaunchError::HostNotBound)
        ));
        assert!(matches!(
            tcp_args(&[], &["{host}".into()], 1),
            Err(LaunchError::PortMissing)
        ));
    }
}
