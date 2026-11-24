//! 포트 포워딩(18-m7-plan M7-22 — F7-11 · F9-6): 이 컴퓨터의 루프백 포트로 들어온 TCP 연결을 원격 머신의 `host:port` 로 잇는다.
//!
//! - **방향은 하나다**: Desktop 이 루프백에서 받은 TCP 연결 하나마다 링크 채널 하나를 [`Kind::Forward`] 로 열고, Relay 가 그
//!   머신에서 `host:port` 로 TCP 를 연다. Relay 는 Desktop 쪽으로 채널을 열지 않는다 — Desktop 은 Relay 가 보낸 `forward` ·
//!   `connect` 를 규칙 위반으로 보고 링크를 끊는다(`desktop.rs` 의 받은 틀 갈래). 그래서 원격 머신의 프로세스가 포워딩으로 이
//!   컴퓨터 루프백의 다른 서비스에 닿는 길(원격 → 로컬)은 없다.
//! - **원격 쪽은 그 머신의 루프백만**([`ForwardTo::check`] — 메인 판정 2026-11-24): 정직한 Relay 는 원격을 거쳐 다른 호스트로 넘어가지
//!   않는다. **신뢰 경계**(리뷰 F2): 원격 쪽 대상을 정하는 것은 그 머신의 Relay 다 — 털린 Relay 는 어디로든 이을 수 있고 청한 대상을
//!   그대로 되돌려 말할 수도 있다. Desktop 이 지키는 것은 듣는 쪽이 이 컴퓨터의 루프백뿐이라는 것과 설정이 전역 설정에서만 온다는
//!   것이다. Relay 는 연 대상을 `accepted` 에 싣고 Desktop 은 청한 것과 다른 답을 받지 않는다 — 이것은 정직한 Relay 의 버그 · 판
//!   어긋남을 잡는 확인이지 보호가 아니다.
//! - **owner 링크만**(메인 판정 M7-P15): Relay 는 owner 가 아닌 기기의 `forward` 를 거절한다. MCP 툴은 없다 — 포워딩은 사람의
//!   브라우저 · 도구를 위한 것이고, 에이전트가 만들면 로컬에 열린 포트로 원격 내부망에 닿는다.
//! - **로컬은 루프백에만**(결정 M7-D12): [`local_addr`] 이 루프백이 아닌 주소를 거절하고, [`bind`] 가 한 번 더 본다.
//! - 바이트는 채널의 `msg` 본문(base64 글 한 조각, 원문 [`CHUNK`] 이하)이고 한쪽 끝(EOF)은 `{"eof": true}` 다 — 링크의 흐름 제어 ·
//!   받기 예산 · 이어 붙기를 그대로 탄다. 보내는 쪽은 `Mux::send_msg_paced` 로 보내기 버퍼에 자리가 날 때까지 TCP 읽기를 멈춘다:
//!   큰 응답도 틀 상한 · 채널 보내기 버퍼 · 링크 보내기 예산에 닿지 않아 채널이 닫히지 않는다(이월 「링크 후속」 의 큰 응답 문제는
//!   RPC 채널의 것이고 포워딩에는 생기지 않는다). base64 라 와이어 바이트는 원문의 4/3 안팎이다.
//! - 한쪽 TCP 가 읽기 끝(EOF)을 보면 상대에게 `eof` 를 보내고 상대는 자기 TCP 의 쓰기를 닫는다(반쪽 닫기 — 요청을 보내고 쓰기를
//!   닫는 클라이언트도 답을 받는다). 두 방향이 다 끝나면 채널을 순서대로 닫는다. TCP 오류 · 채널 닫힘은 곧바로 양쪽을 닫는다.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpSocket, TcpStream};
use tokio_util::sync::CancellationToken;

#[cfg(doc)]
use crate::frame::Kind;
use crate::mux::{ChannelRx, Mux};

/// 틀 하나에 싣는 원문 바이트의 상한. 기본 창(256 KiB)보다 훨씬 작아 `want` 없이 나가고, 틀 상한(64 MiB)에 닿지 않는다.
pub const CHUNK: usize = 16 * 1024;

/// Relay 가 원격 쪽 TCP 를 여는 시한.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// 원격 쪽 대상의 기본 호스트(08 「원격 호스트 등록 예」).
pub const DEFAULT_REMOTE_HOST: &str = "localhost";

/// `forward` 틀의 본문 — 원격 머신에서 열 TCP 대상.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForwardTo {
    pub host: String,
    pub port: u16,
}

impl ForwardTo {
    pub fn new(host: impl Into<String>, port: u16) -> Self {
        Self {
            host: host.into(),
            port,
        }
    }

    /// 대상은 **그 머신의 루프백만**이다 — `localhost` 또는 루프백 IP(`127.0.0.1` · `::1` 등). 원격을 거쳐 다른 호스트(원격의 내부망 ·
    /// Docker 망)로 가는 길은 두지 않는다(메인 판정 2026-11-24 — M7-P15 를 좁힘). Relay 와 Desktop 이 둘 다 본다.
    pub fn check(&self) -> Result<(), String> {
        if self.port == 0 {
            return Err("the remote port must be 1 to 65535".to_owned());
        }
        if self.addrs().is_none() {
            return Err(format!(
                "the remote host must be localhost or a loopback address (127.0.0.1, ::1), not {:?}",
                self.host
            ));
        }
        Ok(())
    }

    /// Relay 가 이을 주소. `localhost` 는 이름을 풀지 않고 `127.0.0.1` 과 `::1` 로 읽는다(원격의 hosts 파일이 `localhost` 를 다른
    /// 주소로 돌려도 루프백 밖으로 가지 않게 — 개발 서버가 `::1` 에만 듣는 경우도 닿는다). 루프백이 아니면 `None`.
    pub fn addrs(&self) -> Option<Vec<SocketAddr>> {
        let host = self.host.trim();
        if host.eq_ignore_ascii_case(DEFAULT_REMOTE_HOST) {
            return Some(vec![
                SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), self.port),
                SocketAddr::new(IpAddr::V6(std::net::Ipv6Addr::LOCALHOST), self.port),
            ]);
        }
        let text = host
            .strip_prefix('[')
            .and_then(|t| t.strip_suffix(']'))
            .unwrap_or(host);
        let ip = text.parse::<IpAddr>().ok().filter(IpAddr::is_loopback)?;
        Some(vec![SocketAddr::new(ip, self.port)])
    }
}

/// 원격 호스트 설정의 `[[remote_hosts.port_forwards]]` 항목 하나(08 「원격 호스트 등록 예」). `local_host` 는 설정 그대로다 —
/// 루프백인지는 열 때 [`local_addr`] 가 본다(루프백이 아니면 그 포워딩을 열지 않고 목록에 까닭을 보인다).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForwardSetting {
    pub local_port: u16,
    pub remote_port: u16,
    /// 없으면 [`DEFAULT_REMOTE_HOST`].
    pub remote_host: Option<String>,
    /// 없으면 `127.0.0.1`.
    pub local_host: Option<String>,
}

/// 로컬 쪽 오류.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForwardError {
    /// `local_host` 가 루프백이 아니다(결정 M7-D12) — 열지 않는다.
    NotLoopback(String),
    /// 이 컴퓨터에서 그 포트를 이미 누가 쓴다.
    PortInUse(u16),
    /// 그 밖의 듣기 실패.
    Bind(String),
}

impl std::fmt::Display for ForwardError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotLoopback(host) => write!(
                f,
                "local_host {host:?} is not a loopback address; port forwards listen only on 127.0.0.1 or ::1"
            ),
            Self::PortInUse(port) => {
                write!(f, "local port {port} is already in use on this computer")
            }
            Self::Bind(why) => write!(f, "cannot listen: {why}"),
        }
    }
}

impl std::error::Error for ForwardError {}

/// 로컬 듣기 주소. `local_host` 가 없으면 `127.0.0.1`, `localhost` 는 `127.0.0.1` 로 읽는다(이름을 풀지 않는다 — hosts 파일이
/// `localhost` 를 다른 주소로 돌려도 루프백 밖에서 듣지 않게). 그 밖에는 루프백 IP 주소만 받는다.
pub fn local_addr(local_host: Option<&str>, port: u16) -> Result<SocketAddr, ForwardError> {
    let ip = match local_host.map(str::trim) {
        None | Some("localhost") => IpAddr::V4(Ipv4Addr::LOCALHOST),
        Some(text) => {
            let text = text
                .strip_prefix('[')
                .and_then(|t| t.strip_suffix(']'))
                .unwrap_or(text);
            text.parse::<IpAddr>()
                .ok()
                .filter(IpAddr::is_loopback)
                .ok_or_else(|| ForwardError::NotLoopback(text.to_owned()))?
        }
    };
    Ok(SocketAddr::new(ip, port))
}

/// 루프백에서 듣는다. 루프백이 아닌 주소는 다시 거절한다(부른 쪽이 [`local_addr`] 를 건너뛰어도). 포트가 쓰이고 있으면
/// [`ForwardError::PortInUse`]. 포트 0 은 OS 가 빈 포트를 고른다.
///
/// **이 컴퓨터에서 이미 쓰는 포트를 덮지 않는다**(메인 판정 「쓰는 포트 덮지 않음」 — 리뷰 F1): `tokio::net::TcpListener::bind` 는
/// Unix 에서 `SO_REUSEADDR` 를 켠다(mio). macOS · BSD 에서는 그 소켓이 다른 프로세스가 `0.0.0.0:P` · `[::]:P` 로 듣는 중에도 더
/// 좁은 `127.0.0.1:P` · `[::1]:P` 에 바인드되고, 루프백으로 오는 연결이 이쪽(원격)으로 새 사람이 보던 로컬 서버가 조용히 가려진다.
/// 그래서 소켓을 직접 만들고 `SO_REUSEADDR` 를 끈다 — 그러면 그 경우 모두 `EADDRINUSE` 다. Linux 는 듣는 소켓이 있으면 어느 쪽이든
/// 거절한다. Windows 는 std · mio 가 `SO_REUSEADDR` 를 켜지 않고, 이쪽 소켓에 `SO_EXCLUSIVEADDRUSE` 를 걸어 뒤에 다른 프로세스가
/// 이 포트의 더 좁은 주소로 이 포워딩을 가로채지 못하게 한다 — 다른 프로세스가 먼저 와일드카드로 든 포트 위에 Windows 가 이 바인드를
/// 거절하는지는 실기로 확인하지 않았다. 대가: 듣기를 닫은 직후 같은 포트를 다시 열 때 `TIME_WAIT` 에 걸릴 수 있다(듣는 소켓 자체는
/// 대개 `TIME_WAIT` 를 만들지 않는다).
pub async fn bind(addr: SocketAddr) -> Result<TcpListener, ForwardError> {
    if !addr.ip().is_loopback() {
        return Err(ForwardError::NotLoopback(addr.ip().to_string()));
    }
    let failed = |e: std::io::Error| {
        if e.kind() == std::io::ErrorKind::AddrInUse {
            ForwardError::PortInUse(addr.port())
        } else {
            ForwardError::Bind(e.to_string())
        }
    };
    let socket = if addr.is_ipv4() {
        TcpSocket::new_v4()
    } else {
        TcpSocket::new_v6()
    }
    .map_err(failed)?;
    socket.set_reuseaddr(false).map_err(failed)?;
    #[cfg(windows)]
    exclusive(&socket, addr).map_err(failed)?;
    socket.bind(addr).map_err(failed)?;
    socket.listen(1024).map_err(failed)
}

/// Windows: 다른 소켓이 이 포트를 쥐었으면 `AddrInUse`, 아니면 `SO_EXCLUSIVEADDRUSE` 를 건다(남이 뒤에 더 좁은 주소로 들어오지 못하게).
///
/// `SO_EXCLUSIVEADDRUSE` 만으로는 앞서 쥔 소켓을 보지 못한다 — 다른 프로그램이 `0.0.0.0:P` 를 기본 옵션으로 쥐었어도 우리
/// `127.0.0.1:P` 바인드는 성공하고, 루프백 연결은 더 좁은 우리 쪽으로 온다(그 서버를 가림, M7-22 리뷰 F1 의 Windows 꼴 — CI run
/// 45316714445). 그래서 먼저 같은 가족의 와일드카드(`0.0.0.0:P` · `[::]:P`)에 배타 바인드를 해 본다: 그 포트를 어느 주소로든
/// 쥔 소켓이 있으면 실패한다(루프백이 아닌 주소로 쥔 것도 — 더 막는 쪽으로 틀린다). 떠본 소켓은 곧바로 닫는다. 포트 0(골라
/// 받기)은 떠볼 포트가 없다.
#[cfg(windows)]
fn exclusive(socket: &TcpSocket, addr: SocketAddr) -> std::io::Result<()> {
    if addr.port() != 0 {
        let (probe, any) = if addr.is_ipv4() {
            (TcpSocket::new_v4()?, IpAddr::V4(Ipv4Addr::UNSPECIFIED))
        } else {
            (
                TcpSocket::new_v6()?,
                IpAddr::V6(std::net::Ipv6Addr::UNSPECIFIED),
            )
        };
        set_exclusive(&probe)?;
        probe.bind(SocketAddr::new(any, addr.port()))?;
    }
    set_exclusive(socket)
}

#[cfg(windows)]
fn set_exclusive(socket: &TcpSocket) -> std::io::Result<()> {
    use std::os::windows::io::AsRawSocket;
    use windows_sys::Win32::Networking::WinSock::{
        SO_EXCLUSIVEADDRUSE, SOCKET, SOL_SOCKET, setsockopt,
    };
    let on: i32 = 1;
    // SAFETY: 살아 있는 소켓 핸들과 길이가 맞는 4바이트 값을 넘긴다.
    let rc = unsafe {
        setsockopt(
            socket.as_raw_socket() as SOCKET,
            SOL_SOCKET,
            SO_EXCLUSIVEADDRUSE,
            (&raw const on).cast(),
            std::mem::size_of::<i32>() as i32,
        )
    };
    if rc == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

/// 한 방향이 끝난 꼴.
enum Ended {
    /// 상대에게 끝을 알렸다 · 상대의 끝을 받아 쓰기를 닫았다.
    Eof,
    /// 채널 · 연결이 닫혔다 — 양쪽을 곧바로 닫는다.
    Closed,
}

/// 열린 포워딩 채널 하나와 TCP 연결 하나를 잇는다. 두 방향이 다 끝나면 채널을 순서대로 닫는다.
pub(crate) async fn pump(
    mux: Mux,
    channel: u32,
    mut rx: ChannelRx,
    cancel: CancellationToken,
    tcp: TcpStream,
) {
    let _ = tcp.set_nodelay(true);
    let (mut read, mut write) = tcp.into_split();
    let up = async {
        let mut buf = vec![0u8; CHUNK];
        loop {
            let n = tokio::select! {
                () = cancel.cancelled() => return Ended::Closed,
                n = read.read(&mut buf) => n,
            };
            match n {
                Ok(0) => {
                    return if mux.send_msg_paced(channel, json!({ "eof": true })).await {
                        Ended::Eof
                    } else {
                        Ended::Closed
                    };
                }
                Ok(n) => {
                    if !mux
                        .send_msg_paced(channel, Value::String(B64.encode(&buf[..n])))
                        .await
                    {
                        return Ended::Closed;
                    }
                }
                Err(_) => {
                    mux.close_channel(channel, "the connection was reset");
                    return Ended::Closed;
                }
            }
        }
    };
    let down = async {
        // 채널이 닫혀도 받아 둔 것은 다 꺼내 쓴다(닫는 틀은 데이터 뒤에 온다) — 그래서 취소 토큰을 보지 않는다.
        while let Some(message) = rx.recv().await {
            if message.get("eof").and_then(Value::as_bool) == Some(true) {
                let _ = write.shutdown().await;
                return Ended::Eof;
            }
            let Some(bytes) = message.as_str().and_then(|t| B64.decode(t).ok()) else {
                mux.close_channel(channel, "malformed forward data");
                return Ended::Closed;
            };
            if write.write_all(&bytes).await.is_err() {
                mux.close_channel(channel, "the connection was reset");
                return Ended::Closed;
            }
        }
        Ended::Closed
    };
    tokio::pin!(up, down);
    let (mut up_done, mut down_done) = (false, false);
    while !(up_done && down_done) {
        let ended = tokio::select! {
            ended = &mut up, if !up_done => { up_done = true; ended }
            ended = &mut down, if !down_done => { down_done = true; ended }
        };
        if matches!(ended, Ended::Closed) {
            // 상대가 닫았거나 이쪽 TCP 가 끊겼다 — 남은 방향도 버린다(채널은 이미 닫혔거나 아래에서 닫는다).
            mux.close_channel(channel, "closed");
            return;
        }
    }
    mux.finish_channel(channel);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 로컬 듣기 주소는 늘 루프백이다(결정 M7-D12). `0.0.0.0` · `::` · 다른 기기 주소 · 이름은 거절한다.
    #[test]
    fn the_local_address_is_always_loopback() {
        assert_eq!(
            local_addr(None, 8080).unwrap(),
            "127.0.0.1:8080".parse().unwrap()
        );
        assert_eq!(
            local_addr(Some("localhost"), 8080).unwrap(),
            "127.0.0.1:8080".parse().unwrap()
        );
        assert_eq!(
            local_addr(Some("::1"), 5005).unwrap(),
            "[::1]:5005".parse().unwrap()
        );
        assert_eq!(
            local_addr(Some("[::1]"), 5005).unwrap(),
            "[::1]:5005".parse().unwrap()
        );
        for host in [
            "0.0.0.0",
            "::",
            "192.168.0.10",
            "10.0.0.1",
            "example.com",
            "",
        ] {
            assert!(
                matches!(
                    local_addr(Some(host), 80),
                    Err(ForwardError::NotLoopback(_))
                ),
                "{host}"
            );
        }
    }

    /// 부른 쪽이 판정을 건너뛰어도 [`bind`] 가 루프백 밖에서 듣지 않는다.
    #[tokio::test]
    async fn bind_refuses_a_non_loopback_address() {
        let err = bind("0.0.0.0:0".parse().unwrap()).await.unwrap_err();
        assert!(matches!(err, ForwardError::NotLoopback(_)), "{err}");
        let listener = bind("127.0.0.1:0".parse().unwrap()).await.unwrap();
        assert!(listener.local_addr().unwrap().ip().is_loopback());
    }

    /// 다른 프로그램처럼 `held` 의 빈 포트를 쥔다. `::` 는 두 가족을 함께 듣는 소켓이다 — macOS · Linux 의 기본이고, Windows 는
    /// 기본이 IPv6 만이라(`IPV6_V6ONLY` 켜짐) 그대로면 IPv4 루프백을 쥐지 않는다(가릴 것이 없다). 그래서 Windows 에서는 끈다.
    fn hold(held: &str) -> std::net::TcpListener {
        let addr = SocketAddr::new(held.parse().unwrap(), 0);
        let socket = if addr.is_ipv4() {
            TcpSocket::new_v4()
        } else {
            TcpSocket::new_v6()
        }
        .unwrap();
        #[cfg(windows)]
        if addr.is_ipv6() && addr.ip().is_unspecified() {
            use std::os::windows::io::AsRawSocket;
            use windows_sys::Win32::Networking::WinSock::{
                IPPROTO_IPV6, IPV6_V6ONLY, SOCKET, setsockopt,
            };
            let off: i32 = 0;
            // SAFETY: 살아 있는 소켓 핸들과 길이가 맞는 4바이트 값을 넘긴다.
            let rc = unsafe {
                setsockopt(
                    socket.as_raw_socket() as SOCKET,
                    IPPROTO_IPV6,
                    IPV6_V6ONLY,
                    (&raw const off).cast(),
                    std::mem::size_of::<i32>() as i32,
                )
            };
            assert_eq!(rc, 0, "{}", std::io::Error::last_os_error());
        }
        socket.bind(addr).unwrap();
        socket.listen(16).unwrap().into_std().unwrap()
    }

    /// 이 컴퓨터의 다른 소켓이 와일드카드(`0.0.0.0` · `::`)나 같은 루프백으로 쥔 포트 위에 바인드하지 않는다 — macOS 에서
    /// `SO_REUSEADDR` 를 켜면 더 좁은 루프백 주소에 바인드돼 그 서버를 가린다(리뷰 F1). `SO_REUSEADDR` 를 켜면 이 시험이 실패한다.
    #[tokio::test]
    async fn bind_does_not_cover_a_port_another_socket_holds() {
        let mut cases: Vec<(&str, &str)> =
            vec![("0.0.0.0", "127.0.0.1"), ("127.0.0.1", "127.0.0.1")];
        // IPv6 가 없는 기기(일부 CI)에서는 IPv6 경우를 건너뛴다.
        if std::net::TcpListener::bind("[::1]:0").is_ok() {
            cases.extend([("::", "127.0.0.1"), ("::", "::1"), ("::1", "::1")]);
        }
        for (held, ours) in cases {
            let holder = hold(held);
            let port = holder.local_addr().unwrap().port();
            let err = bind(local_addr(Some(ours), port).unwrap())
                .await
                .map(|l| l.local_addr().unwrap())
                .unwrap_err();
            assert_eq!(
                err,
                ForwardError::PortInUse(port),
                "{held} held, bound {ours}"
            );
            assert!(
                err.to_string()
                    .contains(&format!("port {port} is already in use"))
            );
        }
    }

    /// 이미 쓰이는 로컬 포트는 그 포트 번호를 실은 [`ForwardError::PortInUse`] 다(18-m7-plan M7-22 ④).
    #[tokio::test]
    async fn a_local_port_in_use_is_reported_with_its_number() {
        let taken = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = taken.local_addr().unwrap().port();
        let err = bind(local_addr(None, port).unwrap()).await.unwrap_err();
        assert_eq!(err, ForwardError::PortInUse(port));
        assert!(err.to_string().contains(&port.to_string()));
    }

    /// 원격 쪽 대상은 그 머신의 루프백만이다 — 다른 호스트 이름 · 주소(원격의 내부망)는 거절한다(메인 판정 2026-11-24).
    #[test]
    fn the_remote_target_is_only_the_machines_loopback() {
        for host in [
            "localhost",
            "LOCALHOST",
            "127.0.0.1",
            "::1",
            "[::1]",
            "127.0.0.2",
        ] {
            assert!(ForwardTo::new(host, 8080).check().is_ok(), "{host}");
        }
        assert_eq!(
            ForwardTo::new("localhost", 5173).addrs().unwrap(),
            vec![
                "127.0.0.1:5173".parse().unwrap(),
                "[::1]:5173".parse().unwrap()
            ]
        );
        for host in [
            "db.internal",
            "10.0.0.5",
            "192.168.1.1",
            "0.0.0.0",
            "::",
            "host.docker.internal",
            "localhost.evil.com",
            "",
        ] {
            assert!(ForwardTo::new(host, 80).check().is_err(), "{host}");
        }
        assert!(ForwardTo::new("localhost", 0).check().is_err());
    }
}
