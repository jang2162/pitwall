//! 커밋 보내기 · 가져오기의 엔진(18-m7-plan.md M7-16 — 08 「커밋 보내기 · 가져오기」, D57 ① ②, D42, 메인 판정 M7-P9).
//!
//! 코드는 커밋으로만, origin 을 거치지 않고 체크아웃 사이를 건넌다. Host 는 다른 머신을 모른다 — 원본 머신의 Host 가 묶음을 내고
//! ([`prepare`]), Desktop 이 그 바이트를 대상 머신의 Host 로 잇고([`receive`]), 대상 Host 가 맞춘다([`apply`]). RPC 는 [`methods`].
//!
//! - **원본**(`git.send.prepare`, 읽기): 보낼 커밋 id 와 그 브랜치 이름을 받는다. 원본 저장소에 ref 를 쓰지 않으려고, 원본의
//!   객체 폴더를 alternates 로만 보는 임시 bare 저장소(`$STATE/git-send/`)에 `refs/heads/<branch>` 하나를 두고 거기서
//!   `git bundle create` 한다 — 원본 저장소의 설정 · 훅을 읽지 않고 원본에 쓰지 않는다. 대상이 이미 가진 커밋(`theirs` 가운데
//!   원본에 있는 것)은 `--not` 으로 뺀다. 원본에 커밋 안 한 변경이 있으면 표지만 단다(막지 않는다, D57 ①).
//! - **대상 받기**(`git.send.receive.*`, 쓰기 — 그 프로젝트 창의 연결만): 받은 바이트를 `$STATE/git-send/` 의 0600 임시 파일에
//!   쓰고(연결 · 부른 role 에 묶인 `uploadId`, 청크는 offset 으로 잇는다), `git bundle verify` → 묶음의 `refs/heads/<branch>` 가
//!   알린 커밋인지 → `git fetch <파일> +refs/heads/<branch>:refs/pitwall/incoming/<원본 머신>/<branch>`(refspec 은 Host 가 만든다 —
//!   묶음의 다른 ref 는 받지 않는다, 태그도) → 임시 파일 지움. 그 전에 다 쓴 전용 ref 를 치운다(그 커밋이 같은 이름의 브랜치에 든
//!   것 — ⑥).
//! - **대상 맞추기**(`git.send.apply`, 쓰기): 대상 브랜치가 `expected` 그대로이고 받은 커밋의 조상이어야 한다(아니면 멈춤). 그
//!   브랜치가 이 워크트리에 체크아웃돼 있으면 `git merge --ff-only`, 다른 워크트리가 쓰면 멈춤, 아니면 `update-ref` CAS(없으면
//!   만들기) → `git switch`(강제 없음) — `switch` 가 실패하면 ref 를 CAS 로 되돌린다(만들었으면 지운다). **멈춤이면 어느 갈래든 대상
//!   ref · HEAD · 작업 트리가 처음과 같다**(전용 ref 만 남는다).
//!
//! 모든 git 은 쓰기 실행기([`crate::write`])의 명령으로 돈다(D43 — 미신뢰도 사람이 누른 쓰기는 돈다, 훅은 git 이 하는 대로). origin ·
//! 원격 설정에 닿는 명령(`push` · `fetch <원격 이름>` · `remote` · `config`)은 부르지 않는다. 받기 · 맞추기는
//! `-c submodule.recurse=false`(+ 받는 옵션이 있는 명령은 `--no-recurse-submodules`)로 서브모듈 원격에 닿지 않고,
//! `GIT_LFS_SKIP_SMUDGE=1` 로 LFS 서버에 닿지 않는다 — 대상 작업 트리에는 LFS 포인터가 남고 결과에 그 수가 실린다.
//!
//! 묶음 바이트는 로그 · Task 진행 · 오류 문장에 넣지 않는다(크기 · 해시만).

pub mod apply;
pub mod methods;
pub mod prepare;
pub mod receive;
#[cfg(all(test, unix))]
mod tests;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use gix::ObjectId;
use gix::bstr::ByteSlice;
use pw_rpc::RpcError;

pub use methods::register;

use crate::Git;
use crate::cli::GitCommand;
use crate::write::WriteError;

/// 받는 묶음의 크기 상한(바이트, 출발값 1 GiB — 18 M7-16 ⑤). 원본도 이보다 큰 묶음은 내지 않는다.
pub const MAX_BUNDLE: u64 = 1 << 30;

/// 청크 하나의 상한(디코드한 바이트). 원본은 [`CHUNK`] 씩 내고 받는 쪽은 이것까지 받는다.
pub const MAX_CHUNK: usize = 1 << 20;

/// 원본이 내는 청크 크기(디코드한 바이트).
pub const CHUNK: usize = 512 * 1024;

/// 받은 커밋을 두는 전용 ref 의 뿌리. `<뿌리>/<원본 머신 id>/<브랜치>`.
pub const INCOMING_PREFIX: &str = "refs/pitwall/incoming/";

/// `$STATE` 아래 임시 파일 폴더 이름.
pub const STATE_DIR: &str = "git-send";

/// 전용 ref 이름.
pub fn incoming_ref(source_machine: &str, branch: &str) -> String {
    format!("{INCOMING_PREFIX}{source_machine}/{branch}")
}

/// 원본 머신 id: 비지 않고 128자 이하, `[A-Za-z0-9._-]` 만, `.` 으로 시작하지 않는다 — ref 경로 한 마디로 쓴다.
pub(crate) fn machine_id(value: &str) -> Result<(), WriteError> {
    let ok = !value.is_empty()
        && value.len() <= 128
        && !value.starts_with('.')
        && !value.ends_with(".lock")
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
    if ok {
        Ok(())
    } else {
        Err(invalid(format!(
            "source machine `{value}` must be 1-128 characters of [A-Za-z0-9._-] and must not start with `.`"
        )))
    }
}

/// 사람이 준 브랜치 이름(`git check-ref-format --branch` 의 규칙 — [`crate::branch::branch_name`]).
pub(crate) fn branch_name(value: &str) -> Result<(), WriteError> {
    crate::branch::branch_name("branch", value)
}

/// 커밋 id: 소문자 16진수 40자(sha1 — 줄임 · 리비전 표기를 받지 않는다). 64자(sha256)는 지원하지 않는다고 거절한다 — 사본
/// 저장소와 gix 가 sha1 만 다룬다(리뷰 권고 4).
pub(crate) fn commit_id(what: &str, value: &str) -> Result<ObjectId, WriteError> {
    let hex = |v: &str| {
        v.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    };
    if value.len() == 64 && hex(value) {
        return Err(invalid(format!(
            "{what} `{value}` is a sha256 id; sending commits between sha256 repositories is not supported (only sha1)"
        )));
    }
    let full = value.len() == 40 && hex(value);
    if !full {
        return Err(invalid(format!(
            "{what} `{value}` must be a full lowercase commit id"
        )));
    }
    ObjectId::from_hex(value.as_bytes()).map_err(|e| invalid(format!("{what} `{value}`: {e}")))
}

fn invalid(message: String) -> WriteError {
    WriteError::Rpc(RpcError::invalid_params(message))
}

/// 보내기의 git 명령: 서브모듈 원격 · LFS 서버에 닿지 않는 설정과 환경을 얹는다(모듈 머리 주석).
pub(crate) fn send_git(base: GitCommand) -> GitCommand {
    let mut env: Vec<(OsString, OsString)> = base
        .env()
        .iter()
        .filter(|(name, _)| name.as_os_str() != "GIT_LFS_SKIP_SMUDGE")
        .cloned()
        .collect();
    env.push(("GIT_LFS_SKIP_SMUDGE".into(), "1".into()));
    base.with_env(env).args(["-c", "submodule.recurse=false"])
}

/// `$STATE/git-send` (0700). 처음 쓸 때 앞선 프로세스가 남긴 임시 파일을 지운다([`sweep`]).
pub(crate) fn state_dir(git: &Git) -> Result<PathBuf, WriteError> {
    let host = git.host().map_err(WriteError::Rpc)?;
    let dir = host.projects.paths().state.join(STATE_DIR);
    create_private_dir(&dir)
        .map_err(|e| WriteError::Internal(format!("cannot create {}: {e}", dir.display())))?;
    static SWEPT: OnceLock<()> = OnceLock::new();
    SWEPT.get_or_init(|| sweep(&dir));
    Ok(dir)
}

/// 이 프로세스의 임시 이름 머리(`<pid>-`). [`sweep`] 이 다른 프로세스의 것을 가른다.
pub(crate) fn own_prefix() -> String {
    format!("{}-", std::process::id())
}

/// 살아 있지 않은 프로세스가 남긴 항목을 지운다(죽은 뒤의 찌꺼기). 이름이 `<pid>-` 로 시작하지 않는 것도 지운다.
pub(crate) fn sweep(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let me = std::process::id();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let pid = name
            .to_str()
            .and_then(|n| n.split_once('-'))
            .and_then(|(pid, _)| pid.parse::<u32>().ok());
        let keep = pid.is_some_and(|pid| pid == me || alive(pid));
        if keep {
            continue;
        }
        let path = entry.path();
        let removed = if path.is_dir() {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path)
        };
        if let Err(e) = removed {
            tracing::debug!(target: "pw_git", path = %path.display(), error = %e, "cannot remove a stale git-send file");
        }
    }
}

#[cfg(unix)]
fn alive(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    // SAFETY: kill(2) 의 신호 0 은 보내지 않고 있는지만 본다.
    let rc = unsafe { libc::kill(pid, 0) };
    rc == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(not(unix))]
fn alive(_pid: u32) -> bool {
    // 다른 프로세스의 것을 가리지 못하면 지우려 한다 — 그 프로세스가 연 파일은 Windows 가 지우지 않는다.
    false
}

pub(crate) fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// 0600 으로 새 파일을 만든다(있으면 실패).
pub(crate) fn create_private_file(path: &Path) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

/// `ancestor` 가 `tip` 이거나 그 조상인가. 읽지 못하면 `None`.
pub(crate) fn contains(repo: &gix::Repository, tip: ObjectId, ancestor: ObjectId) -> Option<bool> {
    if tip == ancestor {
        return Some(true);
    }
    let base = repo.merge_base(tip, ancestor).ok()?;
    Some(base.map(gix::Id::detach) == Some(ancestor))
}

/// `name`(전체 이름) 이 가리키는 커밋. 없으면 `None`.
pub(crate) fn ref_target(repo: &gix::Repository, name: &str) -> Option<ObjectId> {
    let mut reference = repo.try_find_reference(name.as_bytes().as_bstr()).ok()??;
    reference.peel_to_id().ok().map(gix::Id::detach)
}
