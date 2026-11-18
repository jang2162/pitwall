//! Linux: `/proc/<pid>/fd` 의 소켓 inode 와 `/proc/net/tcp{,6}` 의 듣는 소켓(상태 `0A`).

use std::collections::{BTreeMap, BTreeSet};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use super::Listener;

/// `/proc/net/tcp` 의 `TCP_LISTEN`.
const LISTEN: &str = "0A";

pub(super) fn listeners(pids: &BTreeSet<u32>) -> Vec<Listener> {
    let mut owners: BTreeMap<u64, Vec<u32>> = BTreeMap::new();
    for &pid in pids {
        for inode in socket_inodes(pid) {
            owners.entry(inode).or_default().push(pid);
        }
    }
    if owners.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for (file, v6) in [("/proc/net/tcp", false), ("/proc/net/tcp6", true)] {
        let Ok(table) = std::fs::read_to_string(file) else {
            continue;
        };
        for (inode, addr) in parse_table(&table, v6) {
            for &pid in owners.get(&inode).into_iter().flatten() {
                out.push(Listener { pid, addr });
            }
        }
    }
    out
}

/// 프로세스가 연 소켓의 inode(`socket:[N]`). 읽지 못하면(다른 사용자 · 사라짐) 빈 목록.
fn socket_inodes(pid: u32) -> Vec<u64> {
    let Ok(entries) = std::fs::read_dir(format!("/proc/{pid}/fd")) else {
        return Vec::new();
    };
    entries
        .filter_map(|entry| {
            let link = std::fs::read_link(entry.ok()?.path()).ok()?;
            link.to_str()?
                .strip_prefix("socket:[")?
                .strip_suffix(']')?
                .parse()
                .ok()
        })
        .collect()
}

/// 표의 듣는 소켓마다 (inode, 주소). 줄: `sl local_address rem_address st tx:rx tr:when retrnsmt uid timeout inode …`.
pub(super) fn parse_table(table: &str, v6: bool) -> Vec<(u64, SocketAddr)> {
    table
        .lines()
        .skip(1)
        .filter_map(|line| {
            let cols: Vec<&str> = line.split_whitespace().collect();
            if cols.get(3)? != &LISTEN {
                return None;
            }
            let addr = parse_addr(cols.get(1)?, v6)?;
            let inode = cols.get(9)?.parse().ok()?;
            Some((inode, addr))
        })
        .collect()
}

/// `0100007F:1F90` — 주소는 커널이 32비트 낱말마다 호스트 순서로 찍은 16진, 포트는 16진.
fn parse_addr(text: &str, v6: bool) -> Option<SocketAddr> {
    let (ip, port) = text.split_once(':')?;
    let port = u16::from_str_radix(port, 16).ok()?;
    let mut bytes = Vec::with_capacity(16);
    for i in (0..ip.len()).step_by(8) {
        let word = u32::from_str_radix(ip.get(i..i + 8)?, 16).ok()?;
        bytes.extend_from_slice(&word.to_ne_bytes());
    }
    let ip = if v6 {
        IpAddr::V6(Ipv6Addr::from(<[u8; 16]>::try_from(bytes.as_slice()).ok()?))
    } else {
        IpAddr::V4(Ipv4Addr::from(<[u8; 4]>::try_from(bytes.as_slice()).ok()?))
    };
    Some(SocketAddr::new(ip, port))
}
