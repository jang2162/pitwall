//! 대상 쪽 받기(`git.send.receive.begin` · `chunk` · `finish` — 모듈 머리 주석의 「대상 받기」).
//!
//! RPC 에 클라이언트 → 서버 스트림이 없어 unary 셋으로 받는다(메인 판정 2026-11-21): `begin` 이 크기 상한 · 이름을 받아 0600 임시
//! 파일과 `uploadId` 를 만들고, `chunk` 가 offset 을 맞춰 잇고(빠지거나 겹친 청크는 거절 — 응답을 받아야 다음 청크를 보내므로 한
//! 번에 청크 하나다), `finish` 가 받기를 쓰기 실행기로 돌린다. `uploadId` 는 연 연결(`SessionId`)과 부른 role 에 묶인다 — 다른
//! 연결이 쓰면 없는 id 와 같은 오류다. 연결이 닫히면 그 받기를 버린다(임시 파일을 지운다).

use std::collections::HashMap;
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use base64::Engine as _;
use gix::ObjectId;
use gix::bstr::ByteSlice;
use pw_core::registry::state::scope_may_write;
use pw_rpc::{ConnectionScope, Role, RpcError, SessionId, code as rpc_code};
use tokio_util::sync::CancellationToken;

use super::prepare::too_large;
use super::{
    INCOMING_PREFIX, MAX_BUNDLE, MAX_CHUNK, branch_name, commit_id, contains, create_private_file,
    incoming_ref, machine_id, own_prefix, ref_target, send_git, state_dir,
};
use crate::write::{Changes, GitWrite, WriteCtx, WriteError, WriteRequest};

/// 받기를 연 쪽: 연결과 role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct UploadOwner {
    key: OwnerKey,
    role: Role,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum OwnerKey {
    Session(SessionId),
    #[cfg(test)]
    Test(u64),
}

impl UploadOwner {
    pub fn of(ctx: &pw_rpc::Ctx) -> Self {
        Self {
            key: OwnerKey::Session(ctx.session()),
            role: ctx.caller().role,
        }
    }

    #[cfg(test)]
    pub(crate) fn test(connection: u64, role: Role) -> Self {
        Self {
            key: OwnerKey::Test(connection),
            role,
        }
    }
}

/// `begin` 이 받는 것.
#[derive(Debug, Clone)]
pub struct Begin {
    pub caller: ConnectionScope,
    pub project_id: String,
    pub worktree: Option<String>,
    pub source_machine: String,
    pub branch: String,
    pub commit: String,
    pub size: u64,
}

/// 받아 둔 것.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Received {
    pub work_tree: String,
    pub incoming_ref: String,
    pub commit: String,
    /// 받기 전 전용 ref 의 커밋(없었으면 `None`).
    pub previous: Option<String>,
    /// 이번에 치운 전용 ref(그 커밋이 같은 이름의 브랜치에 든 것 — ⑥).
    pub pruned: Vec<String>,
}

struct Upload {
    owner: UploadOwner,
    begin: Begin,
    commit: ObjectId,
    written: u64,
    path: PathBuf,
    file: Option<std::fs::File>,
}

impl Drop for Upload {
    fn drop(&mut self) {
        self.file.take();
        let _ = std::fs::remove_file(&self.path);
    }
}

/// 열린 받기들.
#[derive(Default)]
pub struct Uploads {
    open: Mutex<HashMap<u64, Upload>>,
    next: AtomicU64,
}

impl Uploads {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<u64, Upload>> {
        self.open.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// 열린 받기 수(시험).
    pub fn len(&self) -> usize {
        self.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// `owner` 의 받기 `id` 를 꺼낸다. 없거나 다른 쪽의 것이면 없는 id 와 같은 오류.
    fn take(&self, owner: UploadOwner, id: u64) -> Result<Upload, WriteError> {
        let mut open = self.lock();
        match open.get(&id) {
            Some(upload) if upload.owner == owner => Ok(open.remove(&id).expect("just found")),
            _ => Err(unknown(id)),
        }
    }

    /// 받기를 연다. `closed` 가 취소되면(연결이 닫히면) 그 받기를 버린다.
    pub async fn begin(
        self: &Arc<Self>,
        write: &Arc<GitWrite>,
        owner: UploadOwner,
        begin: Begin,
        closed: CancellationToken,
    ) -> Result<u64, WriteError> {
        if !scope_may_write(&begin.caller, &begin.project_id) {
            return Err(WriteError::Rpc(RpcError::new(
                rpc_code::PERMISSION_DENIED,
                "only the project's own window may write to its git repository",
            )));
        }
        machine_id(&begin.source_machine)?;
        branch_name(&begin.branch)?;
        let commit = commit_id("commit", &begin.commit)?;
        if begin.size > MAX_BUNDLE {
            return Err(WriteError::Rpc(too_large()));
        }
        if begin.size == 0 {
            return Err(WriteError::Rpc(RpcError::invalid_params(
                "the bundle size must not be 0",
            )));
        }
        {
            let git = Arc::clone(write.git());
            let (project, worktree) = (begin.project_id.clone(), begin.worktree.clone());
            tokio::task::spawn_blocking(move || {
                git.open(&project, worktree.as_deref()).map(|o| o.is_some())
            })
            .await
            .map_err(|e| WriteError::Internal(e.to_string()))?
            .map_err(WriteError::Rpc)?
            .then_some(())
            .ok_or(WriteError::NoRepository)?;
        }
        let dir = state_dir(write.git())?;
        let id = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        let path = dir.join(format!("{}u{id}.bundle", own_prefix()));
        let file = create_private_file(&path)
            .map_err(|e| WriteError::Internal(format!("cannot create {}: {e}", path.display())))?;
        self.lock().insert(
            id,
            Upload {
                owner,
                begin,
                commit,
                written: 0,
                path,
                file: Some(file),
            },
        );
        let uploads = Arc::clone(self);
        tokio::spawn(async move {
            closed.cancelled().await;
            uploads
                .lock()
                .retain(|key, upload| !(*key == id && upload.owner == owner));
        });
        Ok(id)
    }

    /// 청크 하나(`data` 는 base64). `offset` 은 지금까지 받은 바이트 수여야 한다. 받은 바이트 수를 돌려준다. 실패하면 그 받기를
    /// 버린다.
    pub async fn chunk(
        &self,
        owner: UploadOwner,
        id: u64,
        offset: u64,
        data: &str,
    ) -> Result<u64, WriteError> {
        let mut upload = self.take(owner, id)?;
        // 디코드 전 길이로 먼저 거른다 — 상한을 크게 넘는 글을 디코드하지 않는다.
        if data.len() > MAX_CHUNK.div_ceil(3) * 4 {
            return Err(chunk_too_large());
        }
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(data)
            .map_err(|e| invalid(format!("the chunk is not base64: {e}")))?;
        if bytes.len() > MAX_CHUNK {
            return Err(chunk_too_large());
        }
        if offset != upload.written {
            return Err(invalid(format!(
                "the chunk starts at {offset} but {} bytes were received",
                upload.written
            )));
        }
        let end = upload.written + bytes.len() as u64;
        if end > upload.begin.size {
            return Err(invalid(format!(
                "the chunk ends at {end}, past the announced size {}",
                upload.begin.size
            )));
        }
        let upload = tokio::task::spawn_blocking(move || {
            let file = upload.file.as_mut().expect("an open upload has its file");
            file.write_all(&bytes)
                .map_err(|e| WriteError::Internal(format!("cannot write the bundle: {e}")))?;
            upload.written = end;
            Ok::<_, WriteError>(upload)
        })
        .await
        .map_err(|e| WriteError::Internal(e.to_string()))??;
        self.lock().insert(id, upload);
        Ok(end)
    }

    /// 다 받았으면 확인하고 전용 ref 로 가져온다. 성공 · 실패 어느 쪽이든 받기를 닫고 임시 파일을 지운다.
    pub async fn finish(
        &self,
        write: &Arc<GitWrite>,
        owner: UploadOwner,
        id: u64,
    ) -> Result<Received, WriteError> {
        let mut upload = self.take(owner, id)?;
        if upload.written != upload.begin.size {
            return Err(invalid(format!(
                "received {} of {} bytes",
                upload.written, upload.begin.size
            )));
        }
        if let Some(file) = upload.file.take() {
            file.sync_all()
                .map_err(|e| WriteError::Internal(format!("cannot write the bundle: {e}")))?;
        }
        let request = WriteRequest {
            caller: upload.begin.caller.clone(),
            project_id: upload.begin.project_id.clone(),
            worktree: upload.begin.worktree.clone(),
            title: "task-title-git-send-receive".to_owned(),
            changes: Changes::None,
            askpass: false,
        };
        let (path, branch, machine, commit) = (
            upload.path.clone(),
            upload.begin.branch.clone(),
            upload.begin.source_machine.clone(),
            upload.commit,
        );
        let result = write
            .run(request, move |ctx| async move {
                fetch(&ctx, &path, &machine, &branch, commit).await
            })
            .await;
        drop(upload);
        result
    }
}

async fn fetch(
    ctx: &WriteCtx,
    file: &std::path::Path,
    machine: &str,
    branch: &str,
    commit: ObjectId,
) -> Result<Received, WriteError> {
    let pruned = prune(ctx).await?;
    let file_arg = file.as_os_str().to_owned();
    ctx.run(
        send_git(ctx.git())
            .args(["bundle", "verify", "--quiet"])
            .arg(file_arg.clone()),
    )
    .await?;
    let source_ref = format!("refs/heads/{branch}");
    let heads = ctx
        .run(
            send_git(ctx.git())
                .args(["bundle", "list-heads"])
                .arg(file_arg.clone())
                .arg(&source_ref),
        )
        .await?;
    let carried = String::from_utf8_lossy(&heads.stdout)
        .lines()
        .filter_map(|line| line.split_once(' '))
        .filter(|(_, name)| *name == source_ref)
        .map(|(oid, _)| oid.to_owned())
        .collect::<Vec<_>>();
    if carried != [commit.to_string()] {
        return Err(invalid(format!(
            "the bundle does not carry {source_ref} at {commit}"
        )));
    }
    let incoming = incoming_ref(machine, branch);
    let previous = {
        let incoming = incoming.clone();
        ctx.read(move |o| ref_target(&o.repo, &incoming)).await?
    };
    ctx.run(
        send_git(ctx.git())
            .args([
                "fetch",
                "--no-recurse-submodules",
                "--no-tags",
                "--no-write-fetch-head",
                "--no-auto-maintenance",
                "--quiet",
            ])
            .arg(file_arg)
            .arg(format!("+{source_ref}:{incoming}")),
    )
    .await?;
    let now = {
        let incoming = incoming.clone();
        ctx.read(move |o| ref_target(&o.repo, &incoming)).await?
    };
    if now != Some(commit) {
        return Err(WriteError::Internal(format!(
            "{incoming} is not at {commit} after the fetch"
        )));
    }
    Ok(Received {
        work_tree: crate::branch::work_tree(ctx),
        incoming_ref: incoming,
        commit: commit.to_string(),
        previous: previous.map(|id| id.to_string()),
        pruned,
    })
}

/// 다 쓴 전용 ref 를 지운다: `refs/pitwall/incoming/<머신>/<b>` 의 커밋이 `refs/heads/<b>` 에 들었으면(병합 · fast-forward 됨).
/// 지울 때도 CAS(`update-ref -d <ref> <본 커밋>`)라 그사이 바뀐 것은 남는다.
async fn prune(ctx: &WriteCtx) -> Result<Vec<String>, WriteError> {
    let done = ctx
        .read(|o| {
            let repo = &o.repo;
            let Ok(platform) = repo.references() else {
                return Vec::new();
            };
            let Ok(refs) = platform.prefixed(INCOMING_PREFIX.as_bytes().as_bstr()) else {
                return Vec::new();
            };
            refs.flatten()
                .filter_map(|mut reference| {
                    let name = reference.name().as_bstr().to_str().ok()?.to_owned();
                    let tip = reference.peel_to_id().ok()?.detach();
                    let branch = name.strip_prefix(INCOMING_PREFIX)?.split_once('/')?.1;
                    let head = ref_target(repo, &format!("refs/heads/{branch}"))?;
                    contains(repo, head, tip)?.then_some((name, tip))
                })
                .collect::<Vec<_>>()
        })
        .await?;
    let mut pruned = Vec::new();
    for (name, tip) in done {
        let output = ctx
            .output(send_git(ctx.git()).args(["update-ref", "-d", &name, &tip.to_string()]))
            .await?;
        if output.status.success() {
            pruned.push(name);
        }
    }
    Ok(pruned)
}

fn unknown(id: u64) -> WriteError {
    invalid(format!("no upload `{id}` on this connection"))
}

fn chunk_too_large() -> WriteError {
    invalid(format!("a chunk must not be larger than {MAX_CHUNK} bytes"))
}

fn invalid(message: String) -> WriteError {
    WriteError::Rpc(RpcError::invalid_params(message))
}
