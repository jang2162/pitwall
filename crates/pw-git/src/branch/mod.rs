//! 브랜치 · 체크아웃 · 태그(13-m2-plan.md M2-09 — F5-10, 결정 M2-D6 · M2-D8). 모두 쓰기 실행기([`crate::write`])로 git CLI 를
//! 돌린다 — 사람이 누른 쓰기라 미신뢰 프로젝트에서도 돈다(D43). git 이 스스로 거절하는 것(로컬 변경으로 체크아웃할 수 없음, 이미
//! 있는 이름, 다른 워크트리가 쓰는 브랜치)은 덮지 않고 그 원문으로 실패한다(결정 M2-D6 ④).
//!
//! - 체크아웃(`git switch`): 로컬 브랜치, 원격 브랜치 → 추적 브랜치 생성, 커밋 → 분리된 HEAD. 미저장 확인에는 바뀔 경로(HEAD 의
//!   트리와 대상 트리가 다른 경로 — git 은 그 밖의 작업 트리 파일을 건드리지 않는다)를 싣는다. 경로는 잠금 전에 gix 로 셈하므로,
//!   잠금을 기다리는 사이 HEAD 나 대상이 움직였으면 확인한 경로가 맞지 않는다 — 그때는 아무것도 하지 않고
//!   [`pw_rpc::code::CONFLICT`] 로 끝낸다.
//! - 브랜치 생성 · 이름 바꾸기 · 삭제(`git branch`). 삭제는 병합 안 된 브랜치를 `force` 없이 지우지 않는다 — `git branch -d` 가
//!   거절했고 그 까닭이 git 의 「병합 안 됨」 판정([`merged`])이면 [`code::NOT_MERGED`] 로 알린다(git 의 원문을 실어서). 창은 확인한
//!   뒤 `force` 로 다시 부른다(결정 M2-D6 ①). 지운 결과는 이전 해시다([`GitRefMove`] — 창의 [되돌리기], 결정 M2-D6 ③).
//! - 태그 생성(`git tag`, 메시지가 있으면 주석 태그).
//!
//! 사람이 준 이름 · 리비전은 `--end-of-options` 뒤에 둔다 — `-` 로 시작하는 값이 옵션으로 읽히지 않게. 옵션의 값으로 넘기는
//! 이름(`switch --create <이름>`)은 [`plain`] 이 `-` 로 시작하는 것을 거절한다.

pub mod methods;
#[cfg(all(test, unix))]
mod tests;

use std::collections::BTreeMap;
use std::sync::Arc;

use gix::ObjectId;
use gix::bstr::ByteSlice;
use pw_rpc::{RpcError, code as rpc_code};
use serde::{Deserialize, Serialize};

pub use methods::register;

use crate::history::GitRefMove;
use crate::methods::RepoHead;
use crate::write::{Changes, GitWrite, WriteCtx, WriteError, WriteRequest};

/// 이 모듈의 애플리케이션 오류 코드(`crate::write::code` 와 겹치지 않는 범위). `data` 는 객체다.
pub mod code {
    /// 병합 안 된 브랜치라 `force` 없이 지우지 않았다(`data.stderr` 는 git 의 거절 원문, `data.commit` 은 그 브랜치의 끝 커밋).
    /// 창은 확인한 뒤 `force: true` 로 다시 부른다(결정 M2-D6 ①).
    pub const NOT_MERGED: i32 = 2101;
}

/// 체크아웃할 것.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum GitCheckoutTarget {
    /// 로컬 브랜치(짧은 이름). 없으면 git 의 원문으로 실패한다 — 같은 이름의 원격 브랜치로 추적 브랜치를 몰래 만들지 않는다.
    Branch { name: String },
    /// 원격 브랜치 `refs/remotes/<remote>/<branch>` 를 upstream 으로 하는 로컬 브랜치 `local`(없으면 `branch`)을 만들고 그리로
    /// 간다. 그 이름의 로컬 브랜치가 이미 있으면 git 의 원문으로 실패한다.
    Remote {
        remote: String,
        branch: String,
        #[serde(default)]
        local: Option<String>,
    },
    /// 커밋(해시 · 태그 등 리비전)으로 간다 — 분리된 HEAD.
    Commit { rev: String },
}

/// 사람이 준 이름 · 리비전: 비지 않고 `-` 로 시작하지 않아야 한다(옵션으로 읽히지 않게). 그 밖의 이름 규칙은 git 이 판정한다.
pub(crate) fn plain(what: &str, value: &str) -> Result<(), WriteError> {
    if value.is_empty() || value.starts_with('-') || value.contains('\0') {
        return Err(WriteError::Rpc(RpcError::invalid_params(format!(
            "{what} `{value}` must be non-empty and must not start with `-`"
        ))));
    }
    Ok(())
}

/// 응답의 `workTree`(절대 경로, 레지스트리와 같은 정규화).
pub(crate) fn work_tree(ctx: &WriteCtx) -> String {
    pw_core::registry::path::normalize(ctx.work_tree())
        .unwrap_or_else(|_| ctx.work_tree().to_path_buf())
        .display()
        .to_string()
}

/// `rev` 가 가리키는 커밋(태그는 벗긴다). 없거나 커밋이 아니면 `None`.
fn commit_of(repo: &gix::Repository, rev: &str) -> Option<ObjectId> {
    let id = repo.rev_parse_single(rev.as_bytes().as_bstr()).ok()?;
    Some(id.object().ok()?.peel_to_commit().ok()?.id)
}

fn head_id(repo: &gix::Repository) -> Option<ObjectId> {
    repo.head_id().ok().map(gix::Id::detach)
}

/// 체크아웃 계획: 잠금 전에 본 HEAD · 대상 커밋과, 미저장 확인에 실을 바뀔 경로.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Plan {
    head: Option<ObjectId>,
    target: Option<ObjectId>,
    changes: Changes,
}

/// `rev` 로 가는 체크아웃의 계획. 대상을 찾지 못하거나 트리를 읽지 못하면 작업 트리 전체로 묻는다(git 이 그 까닭으로 실패한다).
fn plan(repo: &gix::Repository, rev: &str) -> Plan {
    let head = head_id(repo);
    let target = commit_of(repo, rev);
    let changes = match target.map(|target| changed_paths(repo, head, target)) {
        Some(Ok(paths)) => Changes::Paths(paths),
        Some(Err(e)) => {
            tracing::debug!(target: "pw_git", rev, error = %e, "cannot list the paths a checkout changes");
            Changes::WorkTree
        }
        None => Changes::WorkTree,
    };
    Plan {
        head,
        target,
        changes,
    }
}

/// HEAD 의 트리와 `target` 의 트리가 다른 경로(내용 · 모드가 다르거나 한쪽에만 있다). HEAD 가 없으면(태어나지 않은 브랜치)
/// `target` 의 경로 전부.
fn changed_paths(
    repo: &gix::Repository,
    head: Option<ObjectId>,
    target: ObjectId,
) -> Result<Vec<String>, String> {
    type Entries = BTreeMap<gix::bstr::BString, (ObjectId, gix::index::entry::Mode)>;
    let entries = |commit: ObjectId| -> Result<Entries, String> {
        let tree = repo
            .find_commit(commit)
            .map_err(|e| e.to_string())?
            .tree_id()
            .map_err(|e| e.to_string())?;
        let index = repo.index_from_tree(&tree).map_err(|e| e.to_string())?;
        Ok(index
            .entries()
            .iter()
            .map(|e| (e.path(&index).to_owned(), (e.id, e.mode)))
            .collect())
    };
    let before = match head {
        Some(head) => entries(head)?,
        None => Entries::new(),
    };
    let after = entries(target)?;
    let mut paths: Vec<String> = before
        .iter()
        .filter(|(path, entry)| after.get(*path) != Some(*entry))
        .map(|(path, _)| path)
        .chain(after.keys().filter(|path| !before.contains_key(*path)))
        .map(|path| path.to_str_lossy().into_owned())
        .collect();
    paths.sort();
    paths.dedup();
    Ok(paths)
}

/// git 의 `branch -d` 판정(`builtin/branch.c` 의 `branch_merged`): 브랜치 `name` 의 끝 커밋이 기준에 들어 있는가. 기준은 그
/// 브랜치의 upstream 이 있으면 그것, 없으면 HEAD 다. HEAD 도 없으면 병합 안 됨. 브랜치가 없거나 읽지 못하면 `None`.
pub(crate) fn merged(repo: &gix::Repository, name: &str) -> Option<bool> {
    let full = gix::refs::FullName::try_from(format!("refs/heads/{name}")).ok()?;
    let tip = repo
        .find_reference(full.as_ref())
        .ok()?
        .peel_to_id()
        .ok()?
        .detach();
    let upstream = crate::status::refs::tracking_ref(repo, full.as_ref())
        .and_then(|tracking| repo.find_reference(tracking.as_bstr()).ok())
        .and_then(|mut reference| reference.peel_to_id().ok().map(gix::Id::detach));
    let Some(reference) = upstream.or_else(|| head_id(repo)) else {
        return Some(false);
    };
    if tip == reference {
        return Some(true);
    }
    let base = repo.merge_base(tip, reference).ok()?;
    Some(base.map(gix::Id::detach) == Some(tip))
}

/// 체크아웃이나 체크아웃하는 브랜치 생성: 계획(바뀔 경로)을 세워 쓰기를 돌리고, 끝난 HEAD 를 돌려준다.
async fn switch(
    write: &Arc<GitWrite>,
    project_id: String,
    worktree: Option<String>,
    title: &str,
    rev: String,
    args: Vec<String>,
) -> Result<(String, RepoHead), WriteError> {
    let git = Arc::clone(write.git());
    let (id, tree, plan_rev) = (project_id.clone(), worktree.clone(), rev.clone());
    let plan = tokio::task::spawn_blocking(move || {
        git.open(&id, tree.as_deref())
            .map(|opened| opened.map(|o| plan(&o.repo, &plan_rev)))
    })
    .await
    .map_err(|e| WriteError::Internal(e.to_string()))?
    .map_err(WriteError::Rpc)?
    .ok_or(WriteError::NoRepository)?;
    let request = WriteRequest {
        project_id,
        worktree,
        title: title.to_owned(),
        changes: plan.changes.clone(),
        askpass: false,
    };
    write
        .run(request, move |ctx| async move {
            if matches!(plan.changes, Changes::Paths(_)) {
                // 확인한 경로는 잠금 전의 HEAD · 대상에서 셈했다 — 그 사이 움직였으면 확인이 맞지 않는다.
                let now = ctx
                    .read(move |o| (head_id(&o.repo), commit_of(&o.repo, &rev)))
                    .await?;
                if now != (plan.head, plan.target) {
                    return Err(moved(now.0));
                }
            }
            ctx.run(ctx.git().args(&args)).await?;
            let head = ctx
                .read(|o| crate::repo::info(&o.repo).map(|info| info.head))
                .await?
                .map_err(WriteError::Internal)?;
            Ok((work_tree(&ctx), head))
        })
        .await
}

/// 잠금을 기다리는 사이 HEAD 나 대상이 움직였다 — 확인한 경로가 맞지 않아 아무것도 하지 않았다.
fn moved(head: Option<ObjectId>) -> WriteError {
    WriteError::Rpc(
        RpcError::new(
            rpc_code::CONFLICT,
            "HEAD or the checkout target moved while the checkout waited for another git write; nothing was changed",
        )
        .with_data(serde_json::json!({ "head": head.map(|id| id.to_string()) })),
    )
}

/// `git.checkout`.
pub async fn checkout(
    write: &Arc<GitWrite>,
    project_id: String,
    worktree: Option<String>,
    target: GitCheckoutTarget,
) -> Result<(String, RepoHead), WriteError> {
    let (rev, args) = match target {
        GitCheckoutTarget::Branch { name } => {
            plain("branch", &name)?;
            (
                format!("refs/heads/{name}"),
                vec![
                    "switch".into(),
                    "--no-guess".into(),
                    "--end-of-options".into(),
                    name,
                ],
            )
        }
        GitCheckoutTarget::Remote {
            remote,
            branch,
            local,
        } => {
            plain("remote", &remote)?;
            plain("branch", &branch)?;
            let local = local.unwrap_or_else(|| branch.clone());
            plain("local branch", &local)?;
            let tracked = format!("refs/remotes/{remote}/{branch}");
            (
                tracked.clone(),
                vec![
                    "switch".into(),
                    "--create".into(),
                    local,
                    "--track".into(),
                    "--end-of-options".into(),
                    tracked,
                ],
            )
        }
        GitCheckoutTarget::Commit { rev } => {
            plain("revision", &rev)?;
            (
                rev.clone(),
                vec![
                    "switch".into(),
                    "--detach".into(),
                    "--end-of-options".into(),
                    rev,
                ],
            )
        }
    };
    switch(
        write,
        project_id,
        worktree,
        "task-title-git-checkout",
        rev,
        args,
    )
    .await
}

/// 이름이 `name` 인 로컬 브랜치의 끝 커밋.
async fn branch_tip(ctx: &WriteCtx, name: &str) -> Result<Option<String>, WriteError> {
    let full = format!("refs/heads/{name}");
    ctx.read(move |o| {
        o.repo
            .try_find_reference(full.as_str())
            .ok()
            .flatten()
            .and_then(|mut r| r.peel_to_id().ok().map(|id| id.to_string()))
    })
    .await
}

/// 브랜치 하나(만들거나 이름을 바꾼 결과).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Branch {
    pub work_tree: String,
    pub ref_name: String,
    pub commit: Option<String>,
}

/// `git.branch.create`: `start`(없으면 HEAD)에서 브랜치를 만든다. `checkout` 이면 그리로 간다(`git switch --create`).
pub async fn create(
    write: &Arc<GitWrite>,
    project_id: String,
    worktree: Option<String>,
    name: String,
    start: Option<String>,
    checkout: bool,
) -> Result<(Branch, Option<RepoHead>), WriteError> {
    plain("branch", &name)?;
    if let Some(start) = &start {
        plain("start point", start)?;
    }
    let ref_name = format!("refs/heads/{name}");
    if checkout {
        let mut args: Vec<String> = vec!["switch".into(), "--create".into(), name];
        if let Some(start) = &start {
            args.extend(["--end-of-options".into(), start.clone()]);
        }
        let rev = start.unwrap_or_else(|| "HEAD".to_owned());
        let (work_tree, head) = switch(
            write,
            project_id,
            worktree,
            "task-title-git-branch-create",
            rev,
            args,
        )
        .await?;
        let commit = match &head {
            RepoHead::Branch { commit, .. } => Some(commit.clone()),
            RepoHead::Unborn { .. } | RepoHead::Detached { .. } => None,
        };
        return Ok((
            Branch {
                work_tree,
                ref_name,
                commit,
            },
            Some(head),
        ));
    }
    let request = WriteRequest {
        project_id,
        worktree,
        title: "task-title-git-branch-create".to_owned(),
        changes: Changes::None,
        askpass: false,
    };
    let branch = write
        .run(request, move |ctx| async move {
            let mut args = vec!["branch".to_owned(), "--end-of-options".into(), name.clone()];
            args.extend(start);
            ctx.run(ctx.git().args(&args)).await?;
            Ok(Branch {
                work_tree: work_tree(&ctx),
                ref_name,
                commit: branch_tip(&ctx, &name).await?,
            })
        })
        .await?;
    Ok((branch, None))
}

/// `git.branch.rename`: `git branch --move <name> <newName>`(이미 있는 이름이면 git 이 거절한다 — 덮어쓰는 `-M` 은 없다).
pub async fn rename(
    write: &Arc<GitWrite>,
    project_id: String,
    worktree: Option<String>,
    name: String,
    new_name: String,
) -> Result<Branch, WriteError> {
    plain("branch", &name)?;
    plain("new branch name", &new_name)?;
    let request = WriteRequest {
        project_id,
        worktree,
        title: "task-title-git-branch-rename".to_owned(),
        changes: Changes::None,
        askpass: false,
    };
    write
        .run(request, move |ctx| async move {
            ctx.run(
                ctx.git()
                    .args(["branch", "--move", "--end-of-options", &name, &new_name]),
            )
            .await?;
            Ok(Branch {
                work_tree: work_tree(&ctx),
                ref_name: format!("refs/heads/{new_name}"),
                commit: branch_tip(&ctx, &new_name).await?,
            })
        })
        .await
}

/// 브랜치 삭제의 결과.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Deleted {
    Deleted {
        work_tree: String,
        moved: GitRefMove,
    },
    /// 병합 안 된 브랜치라 git 이 `-d` 를 거절했다.
    NotMerged {
        stderr: String,
        commit: Option<String>,
    },
}

impl Deleted {
    /// [`Deleted::NotMerged`] 의 RPC 오류([`code::NOT_MERGED`]).
    pub fn not_merged(stderr: String, commit: Option<String>) -> RpcError {
        RpcError::new(
            code::NOT_MERGED,
            "the branch is not fully merged; delete it with force after confirming",
        )
        .with_data(serde_json::json!({ "stderr": stderr, "commit": commit }))
    }
}

/// `git.branch.delete`: `git branch --delete`(`force` 면 `--force` 를 더한다 — 창이 확인한 뒤에만).
pub async fn delete(
    write: &Arc<GitWrite>,
    project_id: String,
    worktree: Option<String>,
    name: String,
    force: bool,
) -> Result<Deleted, WriteError> {
    plain("branch", &name)?;
    let request = WriteRequest {
        project_id,
        worktree,
        title: "task-title-git-branch-delete".to_owned(),
        changes: Changes::None,
        askpass: false,
    };
    write
        .run(request, move |ctx| async move {
            let previous = branch_tip(&ctx, &name).await?;
            let mut command = ctx.git().args(["branch", "--delete"]);
            if force {
                command = command.arg("--force");
            }
            let output = ctx
                .output(command.args(["--end-of-options", &name]))
                .await?;
            if output.status.success() {
                return Ok(Deleted::Deleted {
                    work_tree: work_tree(&ctx),
                    moved: GitRefMove {
                        ref_name: format!("refs/heads/{name}"),
                        previous,
                        current: None,
                    },
                });
            }
            // git 의 거절이 「병합 안 됨」 인지는 git 과 같은 판정으로 가린다(사람용 출력을 해석하지 않는다).
            let lookup = name.clone();
            let merged = ctx.read(move |o| merged(&o.repo, &lookup)).await?;
            if !force && merged == Some(false) {
                return Ok(Deleted::NotMerged {
                    stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
                    commit: previous,
                });
            }
            Err(WriteError::failed("branch", &output))
        })
        .await
}

/// `git.tag.create`: `rev`(없으면 HEAD)에 태그. `message` 가 있으면 주석 태그(`--annotate`, 메시지는 stdin 으로)다.
pub async fn tag(
    write: &Arc<GitWrite>,
    project_id: String,
    worktree: Option<String>,
    name: String,
    rev: Option<String>,
    message: Option<String>,
) -> Result<Branch, WriteError> {
    plain("tag", &name)?;
    if let Some(rev) = &rev {
        plain("revision", rev)?;
    }
    let request = WriteRequest {
        project_id,
        worktree,
        title: "task-title-git-tag-create".to_owned(),
        changes: Changes::None,
        askpass: false,
    };
    write
        .run(request, move |ctx| async move {
            let mut command = ctx.git().arg("tag");
            if let Some(message) = message {
                command = command.args(["--annotate", "--file=-"]).stdin(message);
            }
            let mut args = vec!["--end-of-options".to_owned(), name.clone()];
            args.extend(rev);
            ctx.run(command.args(&args)).await?;
            let full = format!("refs/tags/{name}");
            let lookup = full.clone();
            let commit = ctx
                .read(move |o| commit_of(&o.repo, &lookup).map(|id| id.to_string()))
                .await?;
            Ok(Branch {
                work_tree: work_tree(&ctx),
                ref_name: full,
                commit,
            })
        })
        .await
}
