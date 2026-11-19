//! TCP 연결의 상대 끝을 쥔 프로세스가 이 사용자의 것인가(17-m6-plan.md M6-19b — 메인 판정: 디버그 대상이 띄운 어댑터에 붙는
//! [`crate::Transport::Listening`] 은 상대를 믿지 않는다). loopback 포트는 같은 기기의 다른 사용자도 먼저 잡을 수 있다 — 그 상대에
//! DAP 를 보내지 않는다.
//!
//! 「상대 끝」 은 듣는 소켓이 아니라 **붙은 연결의 반대쪽**(로컬 = 그 포트, 원격 = 우리 쪽 임시 포트)이다 — 붙은 뒤에 보므로 확인과
//! 연결 사이에 상대가 바뀌는 틈이 없다.
//!
//! - **Linux** — `/proc/net/tcp` · `tcp6` 의 그 연결 줄의 `uid`(서버 쪽 소켓은 듣는 소켓의 uid 를 물려받는다 — `accept` 전에도 보인다).
//! - **macOS** — 이 사용자가 읽을 수 있는 프로세스의 소켓 fd(`proc_pidfdinfo` 의 `PROC_PIDFDSOCKETINFO`)에서 그 연결을 찾고, 그
//!   프로세스의 uid(`PROC_PIDTBSDINFO`)를 본다. 다른 사용자의 프로세스는 읽지 못하므로 찾지 못한다(root 면 읽히지만 uid 가 다르다).
//!   `accept` 하기 전의 연결은 어느 프로세스의 fd 에도 없다 — 부르는 쪽이 잠시 되풀이한다. 구조체 배치는 xnu `sys/proc_info.h`
//!   (`socket_fdinfo`)를 따라 오프셋으로 읽는다 — `libc` 에 그 구조체가 없다. 배치가 틀리면 우리 연결도 못 찾으므로 시험
//!   (`finds_the_other_end_of_a_connection_in_this_process`)이 잡는다.
//! - **그 밖(Windows 등)** — 보지 않는다([`PeerOwner::Unchecked`]).

use std::net::SocketAddr;

/// 이 플랫폼에서 상대 끝의 주인을 보는가(아니면 [`peer_owner`] 가 늘 [`PeerOwner::Unchecked`] 다 — 세션은 사람에게 그것을 알린다).
pub(crate) const OWNER_CHECKED: bool = cfg!(any(target_os = "linux", target_os = "macos"));

/// 상대 끝의 주인.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(
    not(any(target_os = "linux", target_os = "macos")),
    expect(dead_code, reason = "only Linux and macOS find the other end")
)]
pub(crate) enum PeerOwner {
    /// 이 사용자의 프로세스다.
    Ours,
    /// 다른 사용자의 것이다(그 uid).
    Theirs(u32),
    /// 이 사용자의 프로세스에서 찾지 못했다(아직 `accept` 전이거나 다른 사용자의 것).
    NotFound,
    /// 이 플랫폼에서는 보지 않는다.
    #[cfg_attr(
        any(target_os = "linux", target_os = "macos"),
        expect(dead_code, reason = "only the platforms without a check build it")
    )]
    Unchecked,
}

/// 우리가 `local` 에서 `peer` 로 연 연결의 반대쪽 주인.
pub(crate) fn peer_owner(local: SocketAddr, peer: SocketAddr) -> PeerOwner {
    imp::peer_owner(local, peer)
}

/// v4 에 대응된 v6(`::ffff:127.0.0.1`)을 v4 로.
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn canonical(addr: SocketAddr) -> SocketAddr {
    SocketAddr::new(addr.ip().to_canonical(), addr.port())
}

#[cfg(target_os = "linux")]
mod imp {
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

    use super::{PeerOwner, canonical};

    pub(super) fn peer_owner(local: SocketAddr, peer: SocketAddr) -> PeerOwner {
        let (local, peer) = (canonical(local), canonical(peer));
        // SAFETY: geteuid 는 실패하지 않는다.
        let me = unsafe { libc::geteuid() };
        for table in ["/proc/net/tcp", "/proc/net/tcp6"] {
            let Ok(text) = std::fs::read_to_string(table) else {
                continue;
            };
            for line in text.lines().skip(1) {
                let cols: Vec<&str> = line.split_whitespace().collect();
                let (Some(l), Some(r), Some(uid)) = (cols.get(1), cols.get(2), cols.get(7)) else {
                    continue;
                };
                if parse(l) == Some(peer) && parse(r) == Some(local) {
                    return match uid.parse::<u32>() {
                        Ok(uid) if uid == me => PeerOwner::Ours,
                        Ok(uid) => PeerOwner::Theirs(uid),
                        Err(_) => PeerOwner::NotFound,
                    };
                }
            }
        }
        PeerOwner::NotFound
    }

    /// `0100007F:1F90` — 주소는 메모리 그대로의 32 비트 낱말들을 16 진으로, 포트는 호스트 순서.
    fn parse(text: &str) -> Option<SocketAddr> {
        let (addr, port) = text.split_once(':')?;
        let port = u16::from_str_radix(port, 16).ok()?;
        let words = |n: usize| -> Option<Vec<[u8; 4]>> {
            (0..n)
                .map(|i| {
                    let word = addr.get(i * 8..i * 8 + 8)?;
                    Some(u32::from_str_radix(word, 16).ok()?.to_ne_bytes())
                })
                .collect()
        };
        let ip = match addr.len() {
            8 => IpAddr::V4(Ipv4Addr::from(words(1)?[0])),
            32 => {
                let mut bytes = [0u8; 16];
                for (i, w) in words(4)?.into_iter().enumerate() {
                    bytes[i * 4..i * 4 + 4].copy_from_slice(&w);
                }
                IpAddr::V6(Ipv6Addr::from(bytes))
            }
            _ => return None,
        };
        Some(canonical(SocketAddr::new(ip, port)))
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use std::ffi::c_void;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

    use super::{PeerOwner, canonical};

    /// xnu `sys/proc_info.h`.
    const PROC_PIDFDSOCKETINFO: libc::c_int = 3;
    const SOCKINFO_IN: i32 = 1;
    const SOCKINFO_TCP: i32 = 2;
    const INI_IPV4: u8 = 0x1;
    /// `socket_fdinfo` 안의 오프셋: `proc_fileinfo`(24) 뒤 `socket_info` — `vinfo_stat`(136) · `soi_so` · `soi_pcb`(16) · 정수
    /// 셋(12) · short 여덟(16) · `soi_oobmark`(4) · `sockbuf_info` 둘(48) 다음이 `soi_kind`, `rfu_1` 다음이 `soi_proto`
    /// (`in_sockinfo` — `tcp_sockinfo` 도 그것으로 시작한다). macOS 27(arm64)에서 듣는 소켓을 떠 맞춰 봤다.
    const KIND: usize = 24 + 232;
    const PROTO: usize = 24 + 240;
    /// `in_sockinfo` 안: `insi_fport` · `insi_lport`(네트워크 순서의 int), `insi_vflag`, `insi_faddr` · `insi_laddr`(16 바이트 —
    /// v4 는 마지막 4 바이트).
    const FPORT: usize = PROTO;
    const LPORT: usize = PROTO + 4;
    const VFLAG: usize = PROTO + 24;
    const FADDR: usize = PROTO + 32;
    const LADDR: usize = PROTO + 48;
    /// 읽는 칸 끝(`insi_laddr` 끝) — 커널이 이보다 적게 답하면 쓰지 않는다.
    const NEEDED: usize = LADDR + 16;

    pub(super) fn peer_owner(local: SocketAddr, peer: SocketAddr) -> PeerOwner {
        let (local, peer) = (canonical(local), canonical(peer));
        // SAFETY: geteuid 는 실패하지 않는다.
        let me = unsafe { libc::geteuid() };
        for pid in pids() {
            if holds(pid, local, peer) {
                return match uid_of(pid) {
                    Some(uid) if uid == me => PeerOwner::Ours,
                    Some(uid) => PeerOwner::Theirs(uid),
                    None => PeerOwner::NotFound,
                };
            }
        }
        PeerOwner::NotFound
    }

    fn pids() -> Vec<libc::pid_t> {
        // SAFETY: 빈 버퍼로 부르면 개수만 답한다.
        let count = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
        let Ok(count) = usize::try_from(count) else {
            return Vec::new();
        };
        let mut pids = vec![0 as libc::pid_t; count + 64];
        let bytes = libc::c_int::try_from(pids.len() * size_of::<libc::pid_t>()).unwrap_or(0);
        // SAFETY: 버퍼의 크기를 바이트로 넘긴다.
        let got = unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast::<c_void>(), bytes) };
        pids.truncate(usize::try_from(got).unwrap_or(0));
        pids
    }

    fn uid_of(pid: libc::pid_t) -> Option<u32> {
        let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::zeroed();
        let size = libc::c_int::try_from(size_of::<libc::proc_bsdinfo>()).ok()?;
        // SAFETY: `proc_bsdinfo` 크기의 버퍼.
        let got = unsafe {
            libc::proc_pidinfo(
                pid,
                libc::PROC_PIDTBSDINFO,
                0,
                info.as_mut_ptr().cast::<c_void>(),
                size,
            )
        };
        // SAFETY: 커널이 그 크기만큼 채웠다.
        (got == size).then(|| unsafe { info.assume_init() }.pbi_uid)
    }

    /// 그 프로세스의 소켓 fd 가운데 로컬 = `peer`, 원격 = `local` 인 TCP 연결이 있는가(읽지 못하면 없다).
    fn holds(pid: libc::pid_t, local: SocketAddr, peer: SocketAddr) -> bool {
        let mut fds = vec![
            libc::proc_fdinfo {
                proc_fd: 0,
                proc_fdtype: 0
            };
            256
        ];
        let got = loop {
            let bytes = libc::c_int::try_from(fds.len() * size_of::<libc::proc_fdinfo>())
                .unwrap_or(libc::c_int::MAX);
            // SAFETY: 버퍼의 크기를 바이트로 넘긴다.
            let got = unsafe {
                libc::proc_pidinfo(
                    pid,
                    libc::PROC_PIDLISTFDS,
                    0,
                    fds.as_mut_ptr().cast::<c_void>(),
                    bytes,
                )
            };
            if got <= 0 {
                return false;
            }
            let n = usize::try_from(got).unwrap_or(0) / size_of::<libc::proc_fdinfo>();
            if n < fds.len() || fds.len() >= 1 << 16 {
                break n;
            }
            fds.resize(fds.len() * 4, fds[0]);
        };
        fds[..got]
            .iter()
            .filter(|f| f.proc_fdtype == libc::PROX_FDTYPE_SOCKET as u32)
            .any(|f| socket_is(pid, f.proc_fd, local, peer))
    }

    fn socket_is(pid: libc::pid_t, fd: i32, local: SocketAddr, peer: SocketAddr) -> bool {
        // 8 바이트로 맞춘 넉넉한 버퍼(`socket_fdinfo` 는 1 KiB 보다 작다).
        let mut buf = [0u64; 128];
        let bytes = libc::c_int::try_from(size_of_val(&buf)).unwrap_or(0);
        // SAFETY: 버퍼의 크기를 바이트로 넘긴다.
        let got = unsafe {
            libc::proc_pidfdinfo(
                pid,
                fd,
                PROC_PIDFDSOCKETINFO,
                buf.as_mut_ptr().cast::<c_void>(),
                bytes,
            )
        };
        if usize::try_from(got).unwrap_or(0) < NEEDED {
            return false;
        }
        // SAFETY: `buf` 는 살아 있고 그 바이트 길이만큼 본다.
        let raw =
            unsafe { std::slice::from_raw_parts(buf.as_ptr().cast::<u8>(), size_of_val(&buf)) };
        let int = |at: usize| i32::from_ne_bytes(raw[at..at + 4].try_into().unwrap_or_default());
        let kind = int(KIND);
        if kind != SOCKINFO_TCP && kind != SOCKINFO_IN {
            return false;
        }
        // 포트는 네트워크 순서의 16 비트가 int 의 낮은 자리에 있다.
        let port = |at: usize| u16::from_be(int(at) as u16);
        let v4 = raw[VFLAG] & INI_IPV4 != 0;
        let addr = |at: usize| -> IpAddr {
            if v4 {
                IpAddr::V4(Ipv4Addr::new(
                    raw[at + 12],
                    raw[at + 13],
                    raw[at + 14],
                    raw[at + 15],
                ))
            } else {
                let mut b = [0u8; 16];
                b.copy_from_slice(&raw[at..at + 16]);
                IpAddr::V6(Ipv6Addr::from(b)).to_canonical()
            }
        };
        SocketAddr::new(addr(LADDR), port(LPORT)) == peer
            && SocketAddr::new(addr(FADDR), port(FPORT)) == local
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
mod imp {
    use std::net::SocketAddr;

    use super::PeerOwner;

    pub(super) fn peer_owner(_local: SocketAddr, _peer: SocketAddr) -> PeerOwner {
        PeerOwner::Unchecked
    }
}

#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
mod tests {
    use std::net::{TcpListener, TcpStream};

    use super::*;

    /// 이 프로세스가 연 연결의 반대쪽(같은 프로세스가 `accept` 한 소켓)은 우리 것이고, 그런 연결이 없는 짝은 찾지 못한다. 배치
    /// (macOS 의 오프셋 · Linux 의 줄 읽기)가 틀리면 앞의 것이 실패한다.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn finds_the_other_end_of_a_connection_in_this_process() {
        for host in ["127.0.0.1", "[::1]"] {
            let Ok(listener) = TcpListener::bind(format!("{host}:0")) else {
                continue; // IPv6 가 없는 기기
            };
            let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
            let (_server, _) = listener.accept().unwrap();
            let (local, peer) = (client.local_addr().unwrap(), client.peer_addr().unwrap());
            assert_eq!(peer_owner(local, peer), PeerOwner::Ours, "{host}");
            // 그 연결이 아닌 짝(상대 포트가 다르다)은 찾지 못한다.
            let nowhere = SocketAddr::new(peer.ip(), 1);
            assert_eq!(peer_owner(local, nowhere), PeerOwner::NotFound, "{host}");
        }
    }
}
