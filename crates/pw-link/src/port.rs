//! 루프백 포트를 듣기 전의 판정 — 이 컴퓨터의 다른 소켓이 쥔 포트 위에 바인드해 그 서버를 가리지 않게(M7-22 리뷰 F1).
//!
//! 포트 포워딩([`crate::forward::bind`])과 MCP HTTP(`pw_mcp::http`)가 같은 판정을 쓴다 — 한쪽만 고쳐 갈라지지 않게 여기 하나만 둔다.
//! macOS · Linux 는 `SO_REUSEADDR` 를 끄는 것으로 충분하고(부르는 쪽), Windows 만 [`exclusive`] 가 더 본다.

use std::net::SocketAddr;
#[cfg(windows)]
use std::net::{IpAddr, Ipv4Addr};

use tokio::net::TcpSocket;

/// 다른 프로그램처럼 `held` 의 빈 포트를 쥔다. `::` 는 `dual` 이면 두 가족을 함께 듣는 소켓이다 — macOS · Linux 의 기본이고,
/// Windows 는 기본이 IPv6 만이라(`IPV6_V6ONLY` 켜짐) 끈다. `dual` 이 아니면 각 OS 의 기본 그대로다(v6 전용은 Windows 만 시험에 쓴다).
/// 시험용(pw-link · pw-mcp 의 가림 시험이 같은 홀더를 쓴다) — 실패하면 멈춘다.
#[doc(hidden)]
pub fn hold(held: &str, dual: bool) -> std::net::TcpListener {
    let addr = SocketAddr::new(held.parse().unwrap(), 0);
    let _ = dual;
    let socket = if addr.is_ipv4() {
        TcpSocket::new_v4()
    } else {
        TcpSocket::new_v6()
    }
    .unwrap();
    #[cfg(windows)]
    if dual && addr.is_ipv6() && addr.ip().is_unspecified() {
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

/// Windows: 다른 소켓이 이 포트를 쥐었으면 `AddrInUse`, 아니면 `SO_EXCLUSIVEADDRUSE` 를 건다(남이 뒤에 더 좁은 주소로 들어오지 못하게).
///
/// `SO_EXCLUSIVEADDRUSE` 만으로는 앞서 쥔 소켓을 보지 못한다 — 다른 프로그램이 `0.0.0.0:P` 를 기본 옵션으로 쥐었어도 우리
/// `127.0.0.1:P` 바인드는 성공하고, 루프백 연결은 더 좁은 우리 쪽으로 온다(그 서버를 가림, M7-22 리뷰 F1 의 Windows 꼴 — CI run
/// 45316714445). 그래서 먼저 같은 가족의 와일드카드(`0.0.0.0:P` · `[::]:P`)에 배타 바인드를 해 본다: 그 포트를 어느 주소로든
/// 쥔 소켓이 있으면 실패한다(루프백이 아닌 주소로 쥔 것도 — 더 막는 쪽으로 틀린다). 두 가족 `[::]:P` 소켓은 어느 떠보기에도
/// 보이지 않아(CI run 45333303107 · 45348060085) 듣기 표도 본다([`listened`] — IPv4 를 들을 때는 IPv4 · IPv6 표 모두, IPv6 는
/// IPv6 표). 그래서 IPv6 만 듣는 `[::]:P`(Windows 의 `::` 기본 — `IPV6_V6ONLY` 켜짐)가 있어도 IPv4 를 「쓰는 중」 으로
/// 거절한다(가리지 않는데도 — 더 막는 쪽). `netstat` 에 `[::]:P` 만 보여도 그 포트는
/// 쓰는 중으로 센다. 떠본 소켓은 곧바로 닫는다. 포트 0(골라 받기)은 떠볼 포트가 없다.
#[cfg(windows)]
pub fn exclusive(socket: &TcpSocket, addr: SocketAddr) -> std::io::Result<()> {
    if addr.port() != 0 {
        let port = addr.port();
        if addr.is_ipv4() {
            probe(
                TcpSocket::new_v4()?,
                SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), port),
            )?;
            listened(port, true)?;
        } else {
            probe(
                TcpSocket::new_v6()?,
                SocketAddr::new(IpAddr::V6(std::net::Ipv6Addr::UNSPECIFIED), port),
            )?;
            listened(port, false)?;
        }
    }
    set_exclusive(socket)
}

/// Windows: 이 컴퓨터의 TCP 듣기 표(`GetExtendedTcpTable` — `netstat -a` 가 읽는 표)에 `port` 로 듣는 소켓이 있으면 `AddrInUse`.
/// IPv6 표는 늘 보고, `v4` 면 IPv4 표도 본다.
///
/// 배타 바인드 떠보기만으로는 두 가족 `[::]:P` 소켓(`IPV6_V6ONLY` 끔 — Node 등의 기본, IPv4 루프백 연결도 받는다)이 보이지
/// 않았다: IPv4 와일드카드 떠보기(CI run 45333303107)도, IPv6 와일드카드 떠보기(CI run 45348060085)도 성공했고 그 위의
/// `127.0.0.1:P` 가 그 서버를 가렸다. 표는 bind 규칙에 기대지 않는다 — 두 가족 소켓은 IPv6 표에 `[::]:P` 로 오른다. IPv6
/// 표의 어느 듣기든(IPv6 만 듣는 `[::]` · `[::1]` 도) 거절한다 — 더 막는 쪽. 표를 읽지 못하면 그 오류로 거절한다(닫힌 쪽).
#[cfg(windows)]
fn listened(port: u16, v4: bool) -> std::io::Result<()> {
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        MIB_TCP6ROW_OWNER_PID, MIB_TCPROW_OWNER_PID,
    };
    use windows_sys::Win32::Networking::WinSock::{AF_INET, AF_INET6};
    let v6 = listener_ports(
        u32::from(AF_INET6),
        std::mem::size_of::<MIB_TCP6ROW_OWNER_PID>(),
        |row| {
            // SAFETY: `row` 는 표 안의 한 줄 크기만큼의 바이트다.
            unsafe { std::ptr::read_unaligned(row.cast::<MIB_TCP6ROW_OWNER_PID>()) }.dwLocalPort
        },
    );
    let mut held = without_ipv6_is_empty(v6, ipv6_loopback)?.contains(&port);
    if v4 && !held {
        held = listener_ports(
            u32::from(AF_INET),
            std::mem::size_of::<MIB_TCPROW_OWNER_PID>(),
            |row| {
                // SAFETY: 위와 같다.
                unsafe { std::ptr::read_unaligned(row.cast::<MIB_TCPROW_OWNER_PID>()) }.dwLocalPort
            },
        )?
        .contains(&port);
    }
    if held {
        Err(std::io::Error::from(std::io::ErrorKind::AddrInUse))
    } else {
        Ok(())
    }
}

/// `family` 의 TCP 듣기 표에서 듣는 포트들. 줄마다 `local_port` 가 네트워크 순서의 포트(아래 16비트)를 꺼낸다.
#[cfg(windows)]
fn listener_ports(
    family: u32,
    row_size: usize,
    local_port: impl Fn(*const u8) -> u32,
) -> std::io::Result<Vec<u16>> {
    use windows_sys::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, NO_ERROR};
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        GetExtendedTcpTable, TCP_TABLE_OWNER_PID_LISTENER,
    };
    let mut size: u32 = 0;
    // 표가 두 부름 사이에 자랄 수 있다 — 크기를 다시 받아 몇 번 더 해 본다.
    for _ in 0..8 {
        // `u32` 로 잡아 표의 정렬(4바이트)을 맞춘다.
        let mut buf = vec![0u32; (size as usize).div_ceil(4).max(1)];
        let mut len = (buf.len() * 4) as u32;
        // SAFETY: `buf` 는 `len` 바이트다. 표가 모자라면 함수는 쓰지 않고 `len` 에 필요한 크기를 적는다.
        let rc = unsafe {
            GetExtendedTcpTable(
                buf.as_mut_ptr().cast(),
                &mut len,
                0,
                family,
                TCP_TABLE_OWNER_PID_LISTENER,
                0,
            )
        };
        if rc == ERROR_INSUFFICIENT_BUFFER {
            size = len;
            continue;
        }
        if rc != NO_ERROR {
            return Err(std::io::Error::from_raw_os_error(rc as i32));
        }
        // 표: `dwNumEntries: u32` 다음에 줄들(4바이트 정렬).
        let count = buf[0] as usize;
        let bytes = (len as usize).min(buf.len() * 4);
        let base = buf.as_ptr().cast::<u8>();
        let mut ports = Vec::with_capacity(count);
        for i in 0..count {
            let start = 4 + i * row_size;
            if start + row_size > bytes {
                break;
            }
            // SAFETY: `start + row_size` 가 받은 바이트 안이다.
            let port = local_port(unsafe { base.add(start) });
            ports.push(u16::from_be((port & 0xffff) as u16));
        }
        return Ok(ports);
    }
    Err(std::io::Error::other("the TCP listener table kept growing"))
}

/// 배타 바인드로 `any` 를 떠보고 곧 닫는다. 그 포트를 쥔 소켓이 있으면 `AddrInUse`(배타로 쥐었으면 `PermissionDenied`).
#[cfg(windows)]
fn probe(socket: TcpSocket, any: SocketAddr) -> std::io::Result<()> {
    set_exclusive(&socket)?;
    socket.bind(any)
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

/// IPv6 표 읽기 `read` 가 실패했을 때 그 기기에 IPv6 가 아예 없으면(`ipv6` 가 거짓 — `[::1]` 에 묶을 수 없다) 빈 표로 본다: 가릴
/// IPv6 소켓도 없다. 그 밖의 실패는 그대로 — 부르는 쪽이 거절한다(닫힌 쪽). IPv6 가 꺼진 Windows 에서 표 읽기가 오류를 내면 모든
/// IPv4 포워딩 · MCP HTTP 가 거절되지 않게(ci-fix5 리뷰 R1 — 그 API 가 그때 오류를 내는지 빈 표를 주는지는 문서로 확인하지 못했다).
#[cfg(any(windows, test))]
fn without_ipv6_is_empty(
    read: std::io::Result<Vec<u16>>,
    ipv6: impl FnOnce() -> bool,
) -> std::io::Result<Vec<u16>> {
    match read {
        Err(_) if !ipv6() => Ok(Vec::new()),
        other => other,
    }
}

/// 이 기기에 IPv6 루프백이 있는가(듣지 않는 UDP 소켓으로 본다 — 남에게 보이는 것이 없다).
#[cfg(windows)]
fn ipv6_loopback() -> bool {
    std::net::UdpSocket::bind((std::net::Ipv6Addr::LOCALHOST, 0)).is_ok()
}

#[cfg(test)]
mod tests {
    use super::without_ipv6_is_empty;

    /// IPv6 표를 못 읽었을 때: IPv6 가 없는 기기면 빈 표, IPv6 가 있으면 그 오류(거절)다. 읽었으면 IPv6 여부를 묻지 않는다.
    #[test]
    fn an_unreadable_ipv6_table_is_empty_only_without_ipv6() {
        let failed = || Err(std::io::Error::other("table"));
        assert_eq!(
            without_ipv6_is_empty(failed(), || false).unwrap(),
            Vec::<u16>::new()
        );
        assert_eq!(
            without_ipv6_is_empty(failed(), || true)
                .unwrap_err()
                .to_string(),
            "table"
        );
        assert_eq!(
            without_ipv6_is_empty(Ok(vec![80]), || panic!("not asked")).unwrap(),
            [80]
        );
    }
}
