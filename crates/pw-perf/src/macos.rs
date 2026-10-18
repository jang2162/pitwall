//! macOS 의 프로세스 다루기: 책임 프로세스를 끊어 띄우기, 프로세스 목록, 메모리, sysctl.
//!
//! 앱 인스턴스의 프로세스를 「책임 프로세스(responsible pid)」 로 고른다. WKWebView 의 WebContent · GPU · Networking 은
//! 앱의 자식이 아니라 launchd 가 띄우는 XPC 프로세스라 부모로는 못 찾고, 그 책임 프로세스는 「요청한 프로세스의 책임
//! 프로세스」 를 물려받는다. 그런데 터미널에서 띄운 프로세스는 책임 프로세스가 터미널 앱이어서 앱도 WebKit 프로세스도
//! 그 터미널의 다른 프로세스와 같은 값을 갖는다(Orca 터미널에서 띄운 앱 · WebKit · 셸 · 에이전트가 모두 터미널 호스트
//! pid 였다). 그래서 띄우는 프로세스를 `responsibility_spawnattrs_setdisclaim` 으로 띄워 **그 pid 가 책임 프로세스**가 되게
//! 한다 — 떼어 다시 띄운 앱과 그 앱이 요청한 WebKit 프로세스가 모두 그 pid 를 물려받으므로, 그 값을 가진 프로세스가 곧
//! 그 앱 인스턴스의 것이다(다른 Pitwall · Safari · 다른 앱의 WebKit 은 섞이지 않는다).
//!
//! 두 함수는 공개 헤더에 없는 libSystem 의 심볼이다(Chromium · LLDB 가 같은 용도로 쓴다). 링크하지 않고 실행할 때
//! `dlsym` 으로 찾는다 — 앞으로의 OS 가 심볼을 빼도 이 크레이트의 빌드 · 시험 바이너리 적재는 깨지지 않고, 측정만
//! 「이 OS 에서는 못 잰다」 로 실패한다([`check_responsibility`]). 책임 프로세스는 TCC 권한 요청을 누구 이름으로 할지에만
//! 쓰이고 성능과는 무관하다.

use std::ffi::{CStr, CString, OsStr, OsString};
use std::os::unix::ffi::OsStrExt as _;
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::OnceLock;

type ResponsibleFn = unsafe extern "C" fn(libc::pid_t) -> libc::pid_t;
type DisclaimFn = unsafe extern "C" fn(*mut libc::posix_spawnattr_t, libc::c_int) -> libc::c_int;

const RESPONSIBLE: &CStr = c"responsibility_get_pid_responsible_for_pid";
const DISCLAIM: &CStr = c"responsibility_spawnattrs_setdisclaim";

struct Responsibility {
    responsible: ResponsibleFn,
    disclaim: DisclaimFn,
}

fn lookup(name: &CStr) -> Option<*mut libc::c_void> {
    // SAFETY: RTLD_DEFAULT 에서 이름으로 찾기만 한다.
    let symbol = unsafe { libc::dlsym(libc::RTLD_DEFAULT, name.as_ptr()) };
    (!symbol.is_null()).then_some(symbol)
}

fn responsibility() -> Option<&'static Responsibility> {
    static FOUND: OnceLock<Option<Responsibility>> = OnceLock::new();
    FOUND
        .get_or_init(|| {
            let (responsible, disclaim) = (lookup(RESPONSIBLE)?, lookup(DISCLAIM)?);
            // SAFETY: 두 심볼의 시그니처는 위 타입과 같다(Chromium 의 선언과 같은 것).
            Some(unsafe {
                Responsibility {
                    responsible: std::mem::transmute::<*mut libc::c_void, ResponsibleFn>(
                        responsible,
                    ),
                    disclaim: std::mem::transmute::<*mut libc::c_void, DisclaimFn>(disclaim),
                }
            })
        })
        .as_ref()
}

/// 이 OS 가 책임 프로세스 함수를 주는가. 없으면 앱 인스턴스의 WebKit 프로세스를 고를 수 없어 재지 않는다.
pub fn check_responsibility() -> Result<(), String> {
    match responsibility() {
        Some(_) => Ok(()),
        None => Err(format!(
            "this macOS does not provide {} / {}; the harness cannot tell the app's WebKit processes apart here, so it does not measure",
            RESPONSIBLE.to_string_lossy(),
            DISCLAIM.to_string_lossy()
        )),
    }
}

fn cstring(bytes: &[u8]) -> std::io::Result<CString> {
    CString::new(bytes).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))
}

fn check(code: libc::c_int, what: &str) -> std::io::Result<()> {
    if code == 0 {
        Ok(())
    } else {
        Err(std::io::Error::other(format!(
            "{what}: {}",
            std::io::Error::from_raw_os_error(code)
        )))
    }
}

/// `program args…` 를 책임 프로세스를 끊고 띄운다. stdio 는 `/dev/null`, 작업 폴더는 `cwd`, 환경은 `env` 그대로다.
/// 띄운 pid 를 돌려준다 — 이 pid 를 [`wait_exit`] 로 거둔다.
pub fn spawn_disclaimed(
    program: &Path,
    args: &[&OsStr],
    cwd: &Path,
    env: &[(OsString, OsString)],
) -> std::io::Result<u32> {
    let program_c = cstring(program.as_os_str().as_bytes())?;
    let mut argv_c = vec![program_c.clone()];
    for arg in args {
        argv_c.push(cstring(arg.as_bytes())?);
    }
    let mut envp_c = Vec::new();
    for (key, value) in env {
        let mut pair = key.as_bytes().to_vec();
        pair.push(b'=');
        pair.extend_from_slice(value.as_bytes());
        envp_c.push(cstring(&pair)?);
    }
    let argv: Vec<*mut libc::c_char> = argv_c
        .iter()
        .map(|s| s.as_ptr().cast_mut())
        .chain([ptr::null_mut()])
        .collect();
    let envp: Vec<*mut libc::c_char> = envp_c
        .iter()
        .map(|s| s.as_ptr().cast_mut())
        .chain([ptr::null_mut()])
        .collect();
    let cwd_c = cstring(cwd.as_os_str().as_bytes())?;
    let found = responsibility()
        .ok_or_else(|| std::io::Error::other(check_responsibility().unwrap_err()))?;
    let null = c"/dev/null";

    // SAFETY: attr · actions 는 init 이 성공한 뒤에만 쓰고 destroy 로 놓는다. 넘기는 문자열과 배열은 이 함수가 끝날 때까지 산다.
    unsafe {
        let mut attr: libc::posix_spawnattr_t = ptr::null_mut();
        check(
            libc::posix_spawnattr_init(&mut attr),
            "posix_spawnattr_init",
        )?;
        let mut actions: libc::posix_spawn_file_actions_t = ptr::null_mut();
        let init = libc::posix_spawn_file_actions_init(&mut actions);
        if init != 0 {
            libc::posix_spawnattr_destroy(&mut attr);
            return check(init, "posix_spawn_file_actions_init").map(|()| 0);
        }
        let mut pid: libc::pid_t = 0;
        let result = (|| {
            check(
                (found.disclaim)(&mut attr, 1),
                "responsibility_spawnattrs_setdisclaim",
            )?;
            for (fd, flags) in [
                (0, libc::O_RDONLY),
                (1, libc::O_WRONLY),
                (2, libc::O_WRONLY),
            ] {
                check(
                    libc::posix_spawn_file_actions_addopen(
                        &mut actions,
                        fd,
                        null.as_ptr(),
                        flags,
                        0,
                    ),
                    "posix_spawn_file_actions_addopen",
                )?;
            }
            check(
                libc::posix_spawn_file_actions_addchdir_np(&mut actions, cwd_c.as_ptr()),
                "posix_spawn_file_actions_addchdir_np",
            )?;
            check(
                libc::posix_spawn(
                    &mut pid,
                    program_c.as_ptr(),
                    &actions,
                    &attr,
                    argv.as_ptr(),
                    envp.as_ptr(),
                ),
                "posix_spawn",
            )
        })();
        libc::posix_spawn_file_actions_destroy(&mut actions);
        libc::posix_spawnattr_destroy(&mut attr);
        result.map(|()| pid as u32)
    }
}

/// [`spawn_disclaimed`] 로 띄운 자식이 끝났으면 종료 상태를 거둬 돌려준다. 아직이면 `None`.
pub fn try_wait(pid: u32) -> std::io::Result<Option<i32>> {
    let mut status = 0;
    // SAFETY: status 는 이 함수의 지역 변수다.
    let got = unsafe { libc::waitpid(pid as libc::pid_t, &mut status, libc::WNOHANG) };
    match got {
        0 => Ok(None),
        -1 => Err(std::io::Error::last_os_error()),
        _ if libc::WIFEXITED(status) => Ok(Some(libc::WEXITSTATUS(status))),
        _ => Ok(Some(-libc::WTERMSIG(status))),
    }
}

/// 책임 프로세스의 pid. 함수가 없는 OS 거나 모르는 pid 면 `None`.
pub fn responsible_pid(pid: u32) -> Option<u32> {
    let found = responsibility()?;
    // SAFETY: pid 값만 넘긴다. 모르는 pid 면 -1 이 온다.
    let responsible = unsafe { (found.responsible)(pid as libc::pid_t) };
    u32::try_from(responsible).ok()
}

pub fn all_pids() -> Vec<u32> {
    // SAFETY: 첫 호출은 개수만, 둘째는 그만큼(여유를 두고) 잡은 버퍼에 쓴다.
    unsafe {
        let n = libc::proc_listallpids(ptr::null_mut(), 0);
        if n <= 0 {
            return Vec::new();
        }
        let mut pids = vec![0 as libc::pid_t; n as usize + 64];
        let bytes = (pids.len() * size_of::<libc::pid_t>()) as libc::c_int;
        let got = libc::proc_listallpids(pids.as_mut_ptr().cast(), bytes);
        pids.truncate(got.max(0) as usize);
        pids.into_iter()
            .filter(|&p| p > 0)
            .map(|p| p as u32)
            .collect()
    }
}

/// 실행 파일 경로. 끝났거나 다른 사용자의 프로세스면 `None`.
pub fn exe_path(pid: u32) -> Option<PathBuf> {
    let mut buf = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    // SAFETY: 버퍼 크기를 그대로 넘긴다.
    let n = unsafe {
        libc::proc_pidpath(
            pid as libc::c_int,
            buf.as_mut_ptr().cast(),
            buf.len() as u32,
        )
    };
    (n > 0).then(|| PathBuf::from(OsStr::from_bytes(&buf[..n as usize])))
}

pub struct Usage {
    pub footprint: u64,
    pub resident: u64,
}

/// `phys_footprint`(Activity Monitor 의 「메모리」) 와 RSS.
pub fn usage(pid: u32) -> Option<Usage> {
    let mut info = std::mem::MaybeUninit::<libc::rusage_info_v4>::zeroed();
    // SAFETY: RUSAGE_INFO_V4 는 rusage_info_v4 를 채운다.
    let code = unsafe {
        libc::proc_pid_rusage(
            pid as libc::c_int,
            libc::RUSAGE_INFO_V4,
            info.as_mut_ptr().cast(),
        )
    };
    (code == 0).then(|| {
        // SAFETY: 성공했으면 채워졌다.
        let info = unsafe { info.assume_init() };
        Usage {
            footprint: info.ri_phys_footprint,
            resident: info.ri_resident_size,
        }
    })
}

pub fn alive(pid: u32) -> bool {
    // SAFETY: 신호 0 은 존재 확인만 한다.
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

pub fn signal(pid: u32, sig: libc::c_int) {
    // SAFETY: 이 측정이 띄운 pid 에만 보낸다(main.rs).
    unsafe {
        libc::kill(pid as libc::pid_t, sig);
    }
}

fn sysctl_bytes(name: &str) -> Option<Vec<u8>> {
    let name = CString::new(name).ok()?;
    let mut len = 0usize;
    // SAFETY: 첫 호출로 길이를 받고 그만큼 잡은 버퍼에 다시 받는다.
    unsafe {
        if libc::sysctlbyname(name.as_ptr(), ptr::null_mut(), &mut len, ptr::null_mut(), 0) != 0 {
            return None;
        }
        let mut buf = vec![0u8; len];
        if libc::sysctlbyname(
            name.as_ptr(),
            buf.as_mut_ptr().cast(),
            &mut len,
            ptr::null_mut(),
            0,
        ) != 0
        {
            return None;
        }
        buf.truncate(len);
        Some(buf)
    }
}

pub fn sysctl_string(name: &str) -> Option<String> {
    let bytes = sysctl_bytes(name)?;
    let text = CStr::from_bytes_until_nul(&bytes).ok()?.to_str().ok()?;
    Some(text.trim().to_owned())
}

pub fn sysctl_u64(name: &str) -> Option<u64> {
    let bytes = sysctl_bytes(name)?;
    match bytes.len() {
        8 => Some(u64::from_ne_bytes(bytes.try_into().ok()?)),
        4 => Some(u64::from(u32::from_ne_bytes(bytes.try_into().ok()?))),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 끊어 띄운 자식은 자기 pid 가 책임 프로세스다. 이것이 깨지면 앱 인스턴스의 WebKit 프로세스를 고르지 못한다.
    #[test]
    fn a_disclaimed_child_is_its_own_responsible_process() {
        let env: Vec<_> = std::env::vars_os().collect();
        let sleep = Path::new("/bin/sleep");
        let pid = spawn_disclaimed(sleep, &[OsStr::new("5")], Path::new("/"), &env).unwrap();
        let responsible = responsible_pid(pid);
        signal(pid, libc::SIGKILL);
        while try_wait(pid).unwrap().is_none() {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(responsible, Some(pid));
        assert_ne!(responsible_pid(std::process::id()), Some(pid));
    }

    #[test]
    fn this_process_is_listed_with_its_path_and_memory() {
        let me = std::process::id();
        assert!(all_pids().contains(&me));
        assert_eq!(
            exe_path(me).unwrap().canonicalize().unwrap(),
            std::env::current_exe().unwrap().canonicalize().unwrap()
        );
        let usage = usage(me).unwrap();
        assert!(usage.footprint > 0 && usage.resident > 0);
        assert!(sysctl_u64("hw.memsize").unwrap() > 0);
        assert!(!sysctl_string("kern.osproductversion").unwrap().is_empty());
    }
}
