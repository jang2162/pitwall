//! 원본 쪽: 보낼 커밋의 묶음을 만든다(`git.send.prepare` — 모듈 머리 주석의 「원본」).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use gix::ObjectId;
use tokio::io::AsyncReadExt;
use tokio_util::sync::CancellationToken;

use super::{
    MAX_BUNDLE, branch_name, commit_id, contains, create_private_dir, create_private_file,
    own_prefix, send_git, state_dir,
};
use crate::Git;
use crate::cli::{Access, RunError};
use crate::write::WriteError;

/// 만든 묶음. 버리면 임시 파일을 지운다.
#[derive(Debug)]
pub struct Prepared {
    /// `$STATE/git-send/` 의 0600 임시 파일.
    pub file: PathBuf,
    pub size: u64,
    /// 원본 작업 트리 · index 에 커밋 안 한 변경이 있다(추적하지 않는 파일은 세지 않는다). 그 변경은 가지 않는다.
    pub uncommitted: bool,
    /// 묶음에서 뺀 커밋(`--not`). 대상이 가졌다고 본 것이다.
    pub excluded: Vec<String>,
}

impl Drop for Prepared {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.file);
    }
}

/// 임시 bare 저장소. 버리면 지운다.
struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// 원본에서 읽은 것.
struct Source {
    target: crate::Target,
    objects: PathBuf,
    uncommitted: bool,
    excluded: Vec<ObjectId>,
}

static NEXT: AtomicU64 = AtomicU64::new(1);

/// 묶음을 만든다. `theirs` 는 대상이 가진 커밋(대상 브랜치 · 전용 ref 끝) — 원본에 없는 것은 무시한다.
pub async fn prepare(
    git: &Arc<Git>,
    project_id: &str,
    worktree: Option<&str>,
    branch: &str,
    commit: &str,
    theirs: &[String],
    cancel: &CancellationToken,
) -> Result<Prepared, WriteError> {
    branch_name(branch)?;
    let commit = commit_id("commit", commit)?;
    let theirs = theirs
        .iter()
        .map(|t| commit_id("theirs", t))
        .collect::<Result<Vec<_>, _>>()?;
    let source = {
        let git = Arc::clone(git);
        let (project_id, worktree) = (project_id.to_owned(), worktree.map(str::to_owned));
        tokio::task::spawn_blocking(move || {
            read_source(&git, &project_id, worktree.as_deref(), commit, &theirs)
        })
        .await
        .map_err(|e| WriteError::Internal(e.to_string()))??
    };
    // 원본 저장소의 설정을 읽지 않으므로(임시 저장소에서 돈다) 신뢰와 상관없이 같은 명령이다 — 쓰기 환경(미신뢰면 로그인 셸만,
    // D35)과 버전 확인을 쓴다.
    let base = git
        .command(&source.target, Access::Write)
        .await
        .map_err(WriteError::WritesOff)?;
    let dir = state_dir(git)?;
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let scratch = Scratch(dir.join(format!("{}{n}-repo", own_prefix())));
    make_scratch(&scratch.0, &source.objects, branch, commit)
        .map_err(|e| WriteError::Internal(format!("cannot prepare the bundle repository: {e}")))?;
    let file = dir.join(format!("{}{n}-out.bundle", own_prefix()));
    let mut out = create_private_file(&file)
        .map_err(|e| WriteError::Internal(format!("cannot create {}: {e}", file.display())))?;
    let mut prepared = Prepared {
        file,
        size: 0,
        uncommitted: source.uncommitted,
        excluded: source.excluded.iter().map(ToString::to_string).collect(),
    };
    let mut command = send_git(base)
        .arg(format!("--git-dir={}", scratch.0.display()))
        .args([
            "bundle",
            "create",
            "--quiet",
            "-",
            &format!("refs/heads/{branch}"),
        ]);
    if !source.excluded.is_empty() {
        command = command
            .arg("--not")
            .args(source.excluded.iter().map(ToString::to_string));
    }
    let mut running = command.spawn().map_err(run_error)?;
    let mut stdout = running
        .stdout()
        .ok_or_else(|| WriteError::Internal("git bundle has no stdout".into()))?;
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let read = tokio::select! {
            biased;
            () = cancel.cancelled() => return Err(WriteError::Cancelled),
            read = stdout.read(&mut buf) => read.map_err(|e| WriteError::Internal(e.to_string()))?,
        };
        if read == 0 {
            break;
        }
        prepared.size += read as u64;
        if prepared.size > MAX_BUNDLE {
            return Err(WriteError::Rpc(too_large()));
        }
        std::io::Write::write_all(&mut out, &buf[..read])
            .map_err(|e| WriteError::Internal(format!("cannot write the bundle: {e}")))?;
    }
    drop(stdout);
    let output = running.wait(cancel).await.map_err(run_error)?;
    if !output.status.success() {
        return Err(WriteError::failed("bundle", &output));
    }
    std::io::Write::flush(&mut out).map_err(|e| WriteError::Internal(e.to_string()))?;
    Ok(prepared)
}

pub(crate) fn too_large() -> pw_rpc::RpcError {
    pw_rpc::RpcError::invalid_params(format!(
        "the bundle is larger than the {} byte limit",
        MAX_BUNDLE
    ))
}

fn run_error(e: RunError) -> WriteError {
    match e {
        RunError::Cancelled => WriteError::Cancelled,
        RunError::Spawn(e) => WriteError::Spawn(e),
    }
}

fn read_source(
    git: &Git,
    project_id: &str,
    worktree: Option<&str>,
    commit: ObjectId,
    theirs: &[ObjectId],
) -> Result<Source, WriteError> {
    let opened = git
        .open(project_id, worktree)
        .map_err(WriteError::Rpc)?
        .ok_or(WriteError::NoRepository)?;
    let repo = &opened.repo;
    if repo.object_hash() != gix::hash::Kind::Sha1 {
        return Err(WriteError::Rpc(pw_rpc::RpcError::invalid_params(format!(
            "sending commits from a {} repository is not supported (only sha1)",
            repo.object_hash()
        ))));
    }
    let parents: Vec<ObjectId> = match repo
        .find_object(commit)
        .ok()
        .and_then(|o| o.try_into_commit().ok())
    {
        Some(c) => c.parent_ids().map(gix::Id::detach).collect(),
        None => {
            return Err(WriteError::Rpc(pw_rpc::RpcError::invalid_params(format!(
                "commit `{commit}` is not in this repository"
            ))));
        }
    };
    // 대상이 가진 커밋 가운데 원본에 있는 것. 보낼 커밋을 이미 가진(그 커밋이 조상인) 끝은 `--not` 에 넣으면 묶음이 비므로 빼고, 대신
    // 그 커밋의 부모를 뺀다 — 대상은 그 커밋을 가졌으니 부모도 가졌다.
    let mut excluded = Vec::new();
    let mut has_commit = false;
    for &tip in theirs {
        if repo.find_object(tip).is_err() {
            continue;
        }
        match contains(repo, tip, commit) {
            Some(true) => has_commit = true,
            Some(false) => excluded.push(tip),
            None => {}
        }
    }
    if has_commit {
        excluded.extend(parents);
    }
    excluded.sort();
    excluded.dedup();
    let uncommitted = repo.is_dirty().unwrap_or_else(|e| {
        tracing::debug!(target: "pw_git", error = %e, "cannot tell whether the source has uncommitted changes");
        false
    });
    Ok(Source {
        target: opened.target.clone(),
        objects: repo.common_dir().join("objects"),
        uncommitted,
        excluded,
    })
}

/// 원본의 객체 폴더를 alternates 로만 보는 bare 저장소: `HEAD` · `objects/info/alternates` · `refs/heads/<branch>`. 설정 파일 ·
/// 훅이 없다.
fn make_scratch(dir: &Path, objects: &Path, branch: &str, commit: ObjectId) -> std::io::Result<()> {
    create_private_dir(dir)?;
    std::fs::create_dir_all(dir.join("objects/info"))?;
    std::fs::create_dir_all(dir.join("refs/tags"))?;
    std::fs::write(dir.join("HEAD"), format!("ref: refs/heads/{branch}\n"))?;
    // Windows 의 `\\?\` 꼴은 git 이 alternates 에서 읽지 못할 수 있어 레지스트리와 같은 정규화를 거친다.
    let objects = std::fs::canonicalize(objects)?;
    let objects = pw_core::registry::path::normalize(&objects).unwrap_or(objects);
    std::fs::write(
        dir.join("objects/info/alternates"),
        format!("{}\n", objects.display()),
    )?;
    let head = dir.join("refs/heads").join(branch);
    std::fs::create_dir_all(head.parent().expect("a ref path has a parent"))?;
    std::fs::write(head, format!("{commit}\n"))
}
