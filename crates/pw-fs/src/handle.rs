//! 연 파일 핸들이 실제로 가리키는 경로. 범위 판정([`crate::scope`])은 경로 글자로 하고 파일은 그 뒤에 열므로, 그 사이 경로의
//! 구성 요소가 루트 밖 링크로 바뀌면(깨진 링크의 대상이 생기는 등) 판정과 다른 파일이 열린다. 연 핸들의 경로를 다시 판정해 막는다.
//!
//! macOS `fcntl(F_GETPATH)`, Linux `/proc/self/fd/<n>`, Windows `GetFinalPathNameByHandleW`. 얻지 못하면(`/proc` 이 없는
//! 환경 등) `None` 이고 판정만 한다.

use std::fs::File;
use std::path::PathBuf;

#[cfg(target_os = "macos")]
pub fn path_of(file: &File) -> Option<PathBuf> {
    use std::ffi::{CStr, OsStr};
    use std::os::fd::AsRawFd as _;
    use std::os::unix::ffi::OsStrExt as _;

    let mut buf = vec![0u8; libc::PATH_MAX as usize];
    // SAFETY: F_GETPATH 는 PATH_MAX 바이트 버퍼에 NUL 로 끝나는 경로를 쓴다. 버퍼는 그 크기로 살아 있다.
    let rc = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETPATH, buf.as_mut_ptr()) };
    if rc == -1 {
        return None;
    }
    let path = CStr::from_bytes_until_nul(&buf).ok()?;
    Some(PathBuf::from(OsStr::from_bytes(path.to_bytes())))
}

#[cfg(target_os = "linux")]
pub fn path_of(file: &File) -> Option<PathBuf> {
    use std::os::fd::AsRawFd as _;

    std::fs::read_link(format!("/proc/self/fd/{}", file.as_raw_fd())).ok()
}

#[cfg(windows)]
pub fn path_of(file: &File) -> Option<PathBuf> {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt as _;
    use std::os::windows::io::AsRawHandle as _;

    use windows_sys::Win32::Storage::FileSystem::{
        FILE_NAME_NORMALIZED, GetFinalPathNameByHandleW, VOLUME_NAME_DOS,
    };

    let handle = file.as_raw_handle();
    let mut buf = vec![0u16; 512];
    loop {
        let len = u32::try_from(buf.len()).ok()?;
        // SAFETY: `handle` 은 살아 있는 파일 핸들이고 버퍼는 `len` 개의 u16 이다. 반환값이 `len` 보다 크면 필요한 크기다.
        let n = unsafe {
            GetFinalPathNameByHandleW(
                handle,
                buf.as_mut_ptr(),
                len,
                FILE_NAME_NORMALIZED | VOLUME_NAME_DOS,
            )
        };
        if n == 0 {
            return None;
        }
        if n < len {
            buf.truncate(n as usize);
            return Some(PathBuf::from(OsString::from_wide(&buf)));
        }
        buf.resize(n as usize, 0);
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
pub fn path_of(_file: &File) -> Option<PathBuf> {
    None
}
