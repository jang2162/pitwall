//! macOS: libproc 의 fd 목록과 소켓 정보(`<sys/proc_info.h>`). libc 크레이트에 `socket_fdinfo` 가 없어 쓰는 칸까지 같은 꼴로 둔다 —
//! 크기 · 위치는 아래 `const` 단언이 헤더(SDK 의 `sizeof` · `offsetof` 를 잰 값)와 맞는지 컴파일 때 본다.

use std::collections::BTreeSet;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use libc::{c_int, c_void};

use super::Listener;

/// `PROC_PIDFDSOCKETINFO`.
const PROC_PIDFDSOCKETINFO: c_int = 3;
/// `SOCKINFO_TCP`.
const SOCKINFO_TCP: c_int = 2;
/// `TSI_S_LISTEN`.
const TSI_S_LISTEN: c_int = 1;
/// `INI_IPV4` · `INI_IPV6`.
const INI_IPV4: u8 = 0x1;
const INI_IPV6: u8 = 0x2;

#[repr(C)]
#[allow(dead_code)] // 헤더와 같은 꼴을 지키려 읽지 않는 칸도 둔다.
#[derive(Clone, Copy)]
struct ProcFileinfo {
    fi_openflags: u32,
    fi_status: u32,
    fi_offset: i64,
    fi_type: i32,
    fi_guardflags: u32,
}

#[repr(C)]
#[allow(dead_code)] // 헤더와 같은 꼴을 지키려 읽지 않는 칸도 둔다.
#[derive(Clone, Copy)]
struct VinfoStat {
    vst_dev: u32,
    vst_mode: u16,
    vst_nlink: u16,
    vst_ino: u64,
    vst_uid: u32,
    vst_gid: u32,
    vst_times: [i64; 8],
    vst_size: i64,
    vst_blocks: i64,
    vst_blksize: i32,
    vst_flags: u32,
    vst_gen: u32,
    vst_rdev: u32,
    vst_qspare: [i64; 2],
}

#[repr(C)]
#[allow(dead_code)] // 헤더와 같은 꼴을 지키려 읽지 않는 칸도 둔다.
#[derive(Clone, Copy)]
struct SockbufInfo {
    sbi_cc: u32,
    sbi_hiwat: u32,
    sbi_mbcnt: u32,
    sbi_mbmax: u32,
    sbi_lowat: u32,
    sbi_flags: i16,
    sbi_timeo: i16,
}

/// `in4in6_addr` · `in6_addr` 의 공용체(16 바이트, 4 바이트 정렬). IPv4 는 마지막 4 바이트다.
#[repr(C)]
#[allow(dead_code)] // 헤더와 같은 꼴을 지키려 읽지 않는 칸도 둔다.
#[derive(Clone, Copy)]
struct InAddr46 {
    words: [u32; 4],
}

#[repr(C)]
#[allow(dead_code)] // 헤더와 같은 꼴을 지키려 읽지 않는 칸도 둔다.
#[derive(Clone, Copy)]
struct InSockinfoV6 {
    in6_hlim: u8,
    in6_cksum: c_int,
    in6_ifindex: u16,
    in6_hops: i16,
}

#[repr(C)]
#[allow(dead_code)] // 헤더와 같은 꼴을 지키려 읽지 않는 칸도 둔다.
#[derive(Clone, Copy)]
struct InSockinfo {
    insi_fport: c_int,
    insi_lport: c_int,
    insi_gencnt: u64,
    insi_flags: u32,
    insi_flow: u32,
    insi_vflag: u8,
    insi_ip_ttl: u8,
    rfu_1: u32,
    insi_faddr: InAddr46,
    insi_laddr: InAddr46,
    insi_v4_tos: u8,
    insi_v6: InSockinfoV6,
}

#[repr(C)]
#[allow(dead_code)] // 헤더와 같은 꼴을 지키려 읽지 않는 칸도 둔다.
#[derive(Clone, Copy)]
struct TcpSockinfo {
    tcpsi_ini: InSockinfo,
    tcpsi_state: c_int,
    tcpsi_timer: [c_int; 4],
    tcpsi_mss: c_int,
    tcpsi_flags: u32,
    rfu_1: u32,
    tcpsi_tp: u64,
}

/// `soi_proto` 공용체 — TCP 칸만 쓰고 크기는 가장 큰 갈래(528 바이트)에 맞춘다.
#[repr(C)]
#[allow(dead_code)] // 헤더와 같은 꼴을 지키려 읽지 않는 칸도 둔다.
#[derive(Clone, Copy)]
union SoiProto {
    pri_tcp: TcpSockinfo,
    size: [u64; 66],
}

#[repr(C)]
#[allow(dead_code)] // 헤더와 같은 꼴을 지키려 읽지 않는 칸도 둔다.
#[derive(Clone, Copy)]
struct SocketInfo {
    soi_stat: VinfoStat,
    soi_so: u64,
    soi_pcb: u64,
    soi_type: c_int,
    soi_protocol: c_int,
    soi_family: c_int,
    soi_options: i16,
    soi_linger: i16,
    soi_state: i16,
    soi_qlen: i16,
    soi_incqlen: i16,
    soi_qlimit: i16,
    soi_timeo: i16,
    soi_error: u16,
    soi_oobmark: u32,
    soi_rcv: SockbufInfo,
    soi_snd: SockbufInfo,
    soi_kind: c_int,
    rfu_1: u32,
    soi_proto: SoiProto,
}

#[repr(C)]
#[allow(dead_code)] // 헤더와 같은 꼴을 지키려 읽지 않는 칸도 둔다.
#[derive(Clone, Copy)]
struct SocketFdinfo {
    pfi: ProcFileinfo,
    psi: SocketInfo,
}

// SDK(MacOSX.sdk `sys/proc_info.h`)에서 잰 값: sizeof(socket_fdinfo) 792, offsetof(psi) 24, sizeof(vinfo_stat) 136,
// offsetof(socket_info, soi_kind) 232 · soi_proto 240, sizeof(in_sockinfo) 80 · insi_vflag 24 · insi_laddr 48,
// offsetof(tcp_sockinfo, tcpsi_state) 80.
const _: () = {
    assert!(size_of::<SocketFdinfo>() == 792);
    assert!(std::mem::offset_of!(SocketFdinfo, psi) == 24);
    assert!(size_of::<VinfoStat>() == 136);
    assert!(std::mem::offset_of!(SocketInfo, soi_kind) == 232);
    assert!(std::mem::offset_of!(SocketInfo, soi_proto) == 240);
    assert!(size_of::<InSockinfo>() == 80);
    assert!(std::mem::offset_of!(InSockinfo, insi_vflag) == 24);
    assert!(std::mem::offset_of!(InSockinfo, insi_laddr) == 48);
    assert!(std::mem::offset_of!(TcpSockinfo, tcpsi_state) == 80);
};

pub(super) fn listeners(pids: &BTreeSet<u32>) -> Vec<Listener> {
    let mut out = Vec::new();
    for &pid in pids {
        let Ok(raw) = libc::pid_t::try_from(pid) else {
            continue;
        };
        for fd in socket_fds(raw) {
            if let Some(addr) = listening(raw, fd) {
                out.push(Listener { pid, addr });
            }
        }
    }
    out
}

/// 프로세스의 소켓 fd. 읽지 못하면(다른 사용자 · 사라짐) 빈 목록.
fn socket_fds(pid: libc::pid_t) -> Vec<c_int> {
    // SAFETY: 널 버퍼 · 크기 0 이면 필요한 바이트 수만 돌려준다.
    let bytes =
        unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDLISTFDS, 0, std::ptr::null_mut(), 0) };
    let Ok(bytes) = usize::try_from(bytes) else {
        return Vec::new();
    };
    // 그 사이 늘 수 있으므로 넉넉히 잡는다.
    let mut fds: Vec<libc::proc_fdinfo> = vec![
        libc::proc_fdinfo {
            proc_fd: 0,
            proc_fdtype: 0
        };
        bytes / size_of::<libc::proc_fdinfo>() + 32
    ];
    let size = c_int::try_from(fds.len() * size_of::<libc::proc_fdinfo>()).unwrap_or(c_int::MAX);
    // SAFETY: 버퍼와 그 바이트 크기를 함께 넘긴다.
    let filled = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDLISTFDS,
            0,
            fds.as_mut_ptr().cast::<c_void>(),
            size,
        )
    };
    fds.truncate(usize::try_from(filled).unwrap_or(0) / size_of::<libc::proc_fdinfo>());
    fds.into_iter()
        .filter(|fd| fd.proc_fdtype == libc::PROX_FDTYPE_SOCKET as u32)
        .map(|fd| fd.proc_fd)
        .collect()
}

/// 듣는 TCP 소켓이면 그 주소.
fn listening(pid: libc::pid_t, fd: c_int) -> Option<SocketAddr> {
    // SAFETY: 모두 0 인 C 구조체는 올바른 값이다.
    let mut info: SocketFdinfo = unsafe { std::mem::zeroed() };
    let size = c_int::try_from(size_of::<SocketFdinfo>()).ok()?;
    // SAFETY: 구조체와 그 바이트 크기를 함께 넘긴다.
    let filled = unsafe {
        libc::proc_pidfdinfo(
            pid,
            fd,
            PROC_PIDFDSOCKETINFO,
            (&raw mut info).cast::<c_void>(),
            size,
        )
    };
    if filled != size || info.psi.soi_kind != SOCKINFO_TCP {
        return None;
    }
    // SAFETY: `soi_kind` 가 TCP 이면 커널이 `pri_tcp` 갈래를 채운다.
    let tcp = unsafe { info.psi.soi_proto.pri_tcp };
    if tcp.tcpsi_state != TSI_S_LISTEN {
        return None;
    }
    let ini = tcp.tcpsi_ini;
    // 포트는 네트워크 바이트 순서로 int 에 든다.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let port = u16::from_be(ini.insi_lport as u16);
    let words = ini.insi_laddr.words;
    let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_ne_bytes()).collect();
    let ip = if ini.insi_vflag & INI_IPV6 != 0 {
        IpAddr::V6(Ipv6Addr::from(<[u8; 16]>::try_from(bytes.as_slice()).ok()?))
    } else if ini.insi_vflag & INI_IPV4 != 0 {
        IpAddr::V4(Ipv4Addr::from(<[u8; 4]>::try_from(&bytes[12..16]).ok()?))
    } else {
        return None;
    };
    Some(SocketAddr::new(ip, port))
}
