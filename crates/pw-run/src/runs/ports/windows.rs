//! Windows: `GetExtendedTcpTable(TCP_TABLE_OWNER_PID_LISTENER)` 의 소유 pid 와 Toolhelp 스냅숏의 부모 pid · `GetProcessTimes` 의
//! 만든 시각.

use std::collections::BTreeSet;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_INSUFFICIENT_BUFFER, FILETIME, INVALID_HANDLE_VALUE, NO_ERROR,
};
use windows_sys::Win32::NetworkManagement::IpHelper::{
    GetExtendedTcpTable, MIB_TCP6ROW_OWNER_PID, MIB_TCPROW_OWNER_PID, TCP_TABLE_OWNER_PID_LISTENER,
};
use windows_sys::Win32::Networking::WinSock::{AF_INET, AF_INET6};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::Threading::{
    GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
};

use super::{Listener, Proc};

pub(super) fn processes() -> Vec<Proc> {
    // SAFETY: 플래그만 넘긴다. 실패하면 INVALID_HANDLE_VALUE.
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Vec::new();
    }
    let mut out = Vec::new();
    // SAFETY: 모두 0 인 C 구조체는 올바른 값이다. `dwSize` 를 채워 넘긴다.
    let mut entry: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    entry.dwSize = u32::try_from(size_of::<PROCESSENTRY32W>()).unwrap_or(u32::MAX);
    // SAFETY: 열린 스냅숏과 크기를 채운 구조체.
    let mut more = unsafe { Process32FirstW(snapshot, &raw mut entry) } != 0;
    while more {
        let pid = entry.th32ProcessID;
        if pid != 0 {
            out.push(Proc {
                pid,
                parent: (entry.th32ParentProcessID != 0).then_some(entry.th32ParentProcessID),
                session: None,
                start: start_time(pid),
            });
        }
        // SAFETY: 위와 같다.
        more = unsafe { Process32NextW(snapshot, &raw mut entry) } != 0;
    }
    // SAFETY: 이 함수가 연 핸들이다.
    unsafe { CloseHandle(snapshot) };
    out
}

/// 프로세스를 만든 시각(FILETIME 의 100 ns 단위). 열지 못하면 `None`.
pub(super) fn start_time(pid: u32) -> Option<u64> {
    // SAFETY: 권한 · pid 만 넘긴다. 실패하면 널.
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        return None;
    }
    let zero = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let (mut created, mut exited, mut kernel, mut user) = (zero, zero, zero, zero);
    // SAFETY: 연 핸들과 네 칸의 자리.
    let ok = unsafe {
        GetProcessTimes(
            handle,
            &raw mut created,
            &raw mut exited,
            &raw mut kernel,
            &raw mut user,
        )
    } != 0;
    // SAFETY: 이 함수가 연 핸들이다.
    unsafe { CloseHandle(handle) };
    ok.then(|| (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime))
}

pub(super) fn listeners(pids: &BTreeSet<u32>) -> Vec<Listener> {
    if pids.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    if let Some(table) = table(AF_INET) {
        for row in rows::<MIB_TCPROW_OWNER_PID>(&table) {
            if pids.contains(&row.dwOwningPid) {
                out.push(Listener {
                    pid: row.dwOwningPid,
                    addr: SocketAddr::new(
                        IpAddr::V4(Ipv4Addr::from(row.dwLocalAddr.to_ne_bytes())),
                        port(row.dwLocalPort),
                    ),
                });
            }
        }
    }
    if let Some(table) = table(AF_INET6) {
        for row in rows::<MIB_TCP6ROW_OWNER_PID>(&table) {
            if pids.contains(&row.dwOwningPid) {
                out.push(Listener {
                    pid: row.dwOwningPid,
                    addr: SocketAddr::new(
                        IpAddr::V6(Ipv6Addr::from(row.ucLocalAddr)),
                        port(row.dwLocalPort),
                    ),
                });
            }
        }
    }
    out
}

/// 포트는 아래 16비트에 네트워크 바이트 순서로 든다.
fn port(raw: u32) -> u16 {
    #[allow(clippy::cast_possible_truncation)] // 아래 16비트만 쓴다.
    u16::from_be(raw as u16)
}

/// 듣는 소켓 표(`dwNumEntries` 뒤에 줄들). 8 바이트 정렬 버퍼에 받는다.
fn table(family: u16) -> Option<Vec<u64>> {
    let mut size: u32 = 0;
    for _ in 0..4 {
        let mut buffer: Vec<u64> = vec![0; (size as usize).div_ceil(8).max(1)];
        let mut bytes = u32::try_from(buffer.len() * 8).ok()?;
        // SAFETY: 버퍼와 그 바이트 크기를 함께 넘긴다. 모자라면 필요한 크기를 `bytes` 에 쓴다.
        let result = unsafe {
            GetExtendedTcpTable(
                buffer.as_mut_ptr().cast(),
                &raw mut bytes,
                0,
                u32::from(family),
                TCP_TABLE_OWNER_PID_LISTENER,
                0,
            )
        };
        if result == NO_ERROR {
            return Some(buffer);
        }
        if result != ERROR_INSUFFICIENT_BUFFER {
            return None;
        }
        // 그 사이 늘 수 있으므로 넉넉히 잡는다.
        size = bytes.saturating_add(4096);
    }
    None
}

/// 표의 줄들. 머리(`dwNumEntries`)는 줄의 정렬에 맞춰 띄운다(`MIB_TCPTABLE_OWNER_PID` 의 `table` 칸 위치).
fn rows<T: Copy>(buffer: &[u64]) -> Vec<T> {
    let bytes = buffer.len() * 8;
    let base = buffer.as_ptr().cast::<u8>();
    // SAFETY: 버퍼는 적어도 8 바이트이고 첫 칸은 u32 다.
    let count = unsafe { base.cast::<u32>().read_unaligned() } as usize;
    let offset = align_of::<T>().max(size_of::<u32>());
    let fits = bytes.saturating_sub(offset) / size_of::<T>();
    (0..count.min(fits))
        .map(|i| {
            // SAFETY: `fits` 안의 줄만 읽는다.
            unsafe {
                base.add(offset + i * size_of::<T>())
                    .cast::<T>()
                    .read_unaligned()
            }
        })
        .collect()
}
