//! 대상 쪽 맞추기(`git.send.apply` — 모듈 머리 주석의 「대상 맞추기」). 멈춤이면 대상 브랜치 ref · HEAD · 작업 트리가 처음과 같다.

use std::sync::Arc;

use gix::ObjectId;
use gix::bstr::ByteSlice;
use pw_rpc::ConnectionScope;
use serde::{Deserialize, Serialize};

use super::{branch_name, commit_id, contains, incoming_ref, machine_id, ref_target, send_git};
use crate::write::{GitWrite, WriteCtx, WriteError, WriteRequest};

/// 맞추기가 받는 것.
#[derive(Debug, Clone)]
pub struct Apply {
    pub caller: ConnectionScope,
    pub project_id: String,
    pub worktree: Option<String>,
    pub source_machine: String,
    pub branch: String,
    /// 받은 커밋 — 전용 ref 가 이것이어야 한다.
    pub commit: String,
    /// 대상 브랜치의 지금 끝이어야 하는 것(CAS). `None` 은 「그 브랜치가 없다」.
    pub expected: Option<String>,
}

/// 멈춘 까닭. 어느 것이든 대상 브랜치 ref · HEAD · 작업 트리는 그대로다(전용 ref 만 남는다).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum GitSendStop {
    /// 대상 브랜치에 받은 커밋에 없는 커밋이 있다(fast-forward 가 아니다).
    Diverged,
    /// 대상 브랜치가 `expected` 가 아니다(그사이 움직였다).
    BranchMoved,
    /// 전용 ref 가 받은 커밋이 아니다(다른 보내기가 덮었거나 받지 않았다).
    IncomingMoved,
    /// git 이 전환 · 병합을 거절했다(작업 트리의 변경 등 — `message` 가 git 의 원문).
    Blocked,
    /// 그 브랜치를 이 머신의 다른 워크트리가 쓴다(체크아웃 · rebase 중).
    InOtherWorktree,
}

/// 맞추기의 끝.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum GitSendOutcome {
    /// 대상 체크아웃이 그 브랜치 · 그 커밋이 됐다.
    Applied {
        /// 브랜치의 이전 끝(되돌리기용, D42). 만들었으면 `null`.
        previous: Option<String>,
        current: String,
        /// 브랜치를 새로 만들었다.
        created: bool,
        /// 체크아웃을 그 브랜치로 바꿨다(이미 그 브랜치였으면 `false`).
        switched: bool,
        /// 바뀐 파일 가운데 LFS 포인터로 남은 수 — 그 머신에서 `git lfs pull` 이 필요하다.
        lfs_pointers: u32,
    },
    /// 아무것도 바꾸지 않았다.
    Stopped {
        reason: GitSendStop,
        /// git 의 거절 원문(있으면).
        message: Option<String>,
    },
}

/// 잠금 안에서 처음 본 대상의 상태.
struct Seen {
    /// 이 워크트리의 HEAD 가 그 브랜치인가.
    on_branch: bool,
    head: Option<ObjectId>,
    branch: Option<ObjectId>,
    incoming: Option<ObjectId>,
    /// `commit` 이 지금 브랜치 끝을 담는가(fast-forward). 브랜치가 없으면 참.
    fast_forward: bool,
    in_other_worktree: bool,
    null: ObjectId,
}

/// 맞춘다. 결과의 `workTree` 와 끝.
pub async fn apply(
    write: &Arc<GitWrite>,
    request: Apply,
) -> Result<(String, GitSendOutcome), WriteError> {
    machine_id(&request.source_machine)?;
    branch_name(&request.branch)?;
    let commit = commit_id("commit", &request.commit)?;
    let expected = request
        .expected
        .as_deref()
        .map(|e| commit_id("expected", e))
        .transpose()?;
    // 미저장 확인에 실을 경로: HEAD 와 받은 커밋의 트리가 다른 경로(체크아웃과 같은 계획 — 잠금 안에서 HEAD 가 그대로인지 본다).
    let plan = {
        let git = Arc::clone(write.git());
        let (id, tree, rev) = (
            request.project_id.clone(),
            request.worktree.clone(),
            commit.to_string(),
        );
        tokio::task::spawn_blocking(move || {
            git.open(&id, tree.as_deref())
                .map(|opened| opened.map(|o| crate::branch::plan(&o.repo, &rev)))
        })
        .await
        .map_err(|e| WriteError::Internal(e.to_string()))?
        .map_err(WriteError::Rpc)?
        .ok_or(WriteError::NoRepository)?
    };
    let write_request = WriteRequest {
        caller: request.caller,
        project_id: request.project_id,
        worktree: request.worktree,
        title: "task-title-git-send-apply".to_owned(),
        changes: plan.changes.clone(),
        askpass: false,
    };
    let (machine, branch) = (request.source_machine, request.branch);
    write
        .run(write_request, move |ctx| async move {
            let outcome = reconcile(&ctx, &machine, &branch, commit, expected, plan.head).await?;
            Ok((crate::branch::work_tree(&ctx), outcome))
        })
        .await
}

fn stopped(reason: GitSendStop, message: Option<String>) -> GitSendOutcome {
    GitSendOutcome::Stopped { reason, message }
}

async fn reconcile(
    ctx: &WriteCtx,
    machine: &str,
    branch: &str,
    commit: ObjectId,
    expected: Option<ObjectId>,
    planned_head: Option<ObjectId>,
) -> Result<GitSendOutcome, WriteError> {
    let full = format!("refs/heads/{branch}");
    let incoming = incoming_ref(machine, branch);
    let seen = {
        let (full, incoming, short) = (full.clone(), incoming.clone(), branch.to_owned());
        ctx.read(move |o| {
            let repo = &o.repo;
            let branch_tip = ref_target(repo, &full);
            let on_branch = repo
                .head_name()
                .ok()
                .flatten()
                .is_some_and(|name| name.as_bstr() == full.as_bytes().as_bstr());
            Seen {
                on_branch,
                head: crate::branch::head_id(repo),
                branch: branch_tip,
                incoming: ref_target(repo, &incoming),
                fast_forward: branch_tip
                    .is_none_or(|old| contains(repo, commit, old) == Some(true)),
                in_other_worktree: !on_branch && crate::branch::checked_out(repo, &short),
                null: repo.object_hash().null(),
            }
        })
        .await?
    };
    if seen.head != planned_head {
        // 미저장 확인에 실은 경로는 잠금 전의 HEAD 로 셈했다.
        return Err(crate::branch::moved(seen.head));
    }
    if seen.incoming != Some(commit) {
        return Ok(stopped(GitSendStop::IncomingMoved, None));
    }
    if seen.branch != expected {
        return Ok(stopped(GitSendStop::BranchMoved, None));
    }
    if !seen.fast_forward {
        return Ok(stopped(GitSendStop::Diverged, None));
    }
    let previous = seen.branch;
    if seen.on_branch {
        if previous != Some(commit) {
            // 작업 트리 · index 를 함께 옮긴다. 막는 변경이 있으면 git 이 거절하고 아무것도 바꾸지 않는다.
            let output = ctx
                .output(
                    send_git(ctx.git())
                        .args([
                            "merge",
                            "--ff-only",
                            "--no-edit",
                            "--quiet",
                            "--end-of-options",
                        ])
                        .arg(commit.to_string()),
                )
                .await?;
            if !output.status.success() {
                return Ok(stopped(GitSendStop::Blocked, Some(git_text(&output))));
            }
        }
        let lfs_pointers = lfs_pointers(ctx, seen.head, commit).await?;
        return Ok(GitSendOutcome::Applied {
            previous: previous.map(|id| id.to_string()),
            current: commit.to_string(),
            created: false,
            switched: false,
            lfs_pointers,
        });
    }
    if seen.in_other_worktree {
        return Ok(stopped(GitSendStop::InOtherWorktree, None));
    }
    let moved = previous != Some(commit);
    if moved {
        let old = previous.unwrap_or(seen.null).to_string();
        let output = ctx
            .output(send_git(ctx.git()).args([
                "update-ref",
                "-m",
                &format!("pitwall: commits sent from {machine}"),
                &full,
                &commit.to_string(),
                &old,
            ]))
            .await?;
        if !output.status.success() {
            return Ok(stopped(GitSendStop::BranchMoved, Some(git_text(&output))));
        }
    }
    let output = ctx
        .output(send_git(ctx.git()).args([
            "switch",
            "--no-recurse-submodules",
            "--no-guess",
            "--quiet",
            "--end-of-options",
            branch,
        ]))
        .await?;
    if !output.status.success() {
        let message = git_text(&output);
        if moved {
            undo(ctx, &full, commit, previous).await?;
        }
        return Ok(stopped(GitSendStop::Blocked, Some(message)));
    }
    let lfs_pointers = lfs_pointers(ctx, seen.head, commit).await?;
    Ok(GitSendOutcome::Applied {
        previous: previous.map(|id| id.to_string()),
        current: commit.to_string(),
        created: previous.is_none(),
        switched: true,
        lfs_pointers,
    })
}

/// `switch` 가 실패한 뒤 ref 를 CAS 로 되돌린다(만들었으면 지운다). 되돌리지 못하면 그 원문으로 실패한다.
async fn undo(
    ctx: &WriteCtx,
    full: &str,
    commit: ObjectId,
    previous: Option<ObjectId>,
) -> Result<(), WriteError> {
    let command = match previous {
        Some(old) => send_git(ctx.git()).args([
            "update-ref",
            "-m",
            "pitwall: undo a send that could not switch",
            full,
            &old.to_string(),
            &commit.to_string(),
        ]),
        None => send_git(ctx.git()).args(["update-ref", "-d", full, &commit.to_string()]),
    };
    ctx.run(command).await.map(|_| ())
}

/// git 의 거절 원문(stderr, 없으면 stdout).
fn git_text(output: &crate::cli::Output) -> String {
    let text = [&output.stderr, &output.stdout]
        .into_iter()
        .map(|bytes| String::from_utf8_lossy(bytes).trim_end().to_owned())
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    if text.is_empty() {
        format!("git exited with {}", output.status)
    } else {
        text
    }
}

/// `before` 에서 `after` 로 바뀐 파일 가운데 `after` 의 내용이 LFS 포인터인 것의 수.
async fn lfs_pointers(
    ctx: &WriteCtx,
    before: Option<ObjectId>,
    after: ObjectId,
) -> Result<u32, WriteError> {
    ctx.read(move |o| {
        let repo = &o.repo;
        let Ok(paths) = crate::branch::changed_paths(repo, before, after) else {
            return 0;
        };
        let Ok(tree) = repo.find_commit(after).and_then(|c| c.tree()) else {
            return 0;
        };
        let count = paths
            .iter()
            .filter(|path| {
                tree.lookup_entry_by_path(path.as_str())
                    .ok()
                    .flatten()
                    .and_then(|entry| entry.object().ok())
                    .is_some_and(|object| is_lfs_pointer(&object.data))
            })
            .count();
        u32::try_from(count).unwrap_or(u32::MAX)
    })
    .await
}

/// git-lfs 포인터 파일(1024 바이트 미만, 첫 줄이 spec 판).
pub(crate) fn is_lfs_pointer(data: &[u8]) -> bool {
    data.len() < 1024 && data.starts_with(b"version https://git-lfs.github.com/spec/v1\n")
}
