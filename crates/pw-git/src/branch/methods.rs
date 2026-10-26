//! 브랜치 · 체크아웃 · 태그 RPC. UI 전용이라 MCP 툴이 아니다(결정 M2-D7 — git 쓰기는 M5 에서도 툴이 아니다). 모두 쓰기
//! 실행기로 돈다 — 오류 코드는 `crate::write::code`(git 의 원문은 `2004` 의 `data.stderr`)와 [`super::code`].

use std::sync::Arc;

use pw_rpc::{Router, RpcError, rpc_method};
use serde::{Deserialize, Serialize};

use super::{Deleted, GitCheckoutTarget, GitRefMove};
use crate::methods::RepoHead;
use crate::write::GitWrite;

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitCheckoutParams {
    pub project_id: String,
    /// 활성 워크트리의 꼭대기(`git.repo` 와 같다).
    #[serde(default)]
    pub worktree: Option<String>,
    pub target: GitCheckoutTarget,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitCheckoutResult {
    /// 저장소의 작업 트리 꼭대기(절대 경로).
    pub work_tree: String,
    /// 체크아웃한 뒤의 HEAD.
    pub head: RepoHead,
}

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitBranchCreateParams {
    pub project_id: String,
    #[serde(default)]
    pub worktree: Option<String>,
    /// 새 브랜치의 짧은 이름.
    pub name: String,
    /// 시작 리비전(해시 · 브랜치 · 태그). 없으면 HEAD.
    #[serde(default)]
    pub start: Option<String>,
    /// 만든 뒤 그 브랜치로 간다(`git switch --create`).
    #[serde(default)]
    pub checkout: bool,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitBranchResult {
    pub work_tree: String,
    /// 전체 이름(`refs/heads/…`).
    pub ref_name: String,
    /// 그 브랜치의 끝 커밋. 태어나지 않은 브랜치면 `null`.
    pub commit: Option<String>,
    /// `checkout` 으로 옮겨 간 뒤의 HEAD. 옮기지 않았으면 `null`.
    pub head: Option<RepoHead>,
}

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitBranchRenameParams {
    pub project_id: String,
    #[serde(default)]
    pub worktree: Option<String>,
    pub name: String,
    pub new_name: String,
}

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitBranchDeleteParams {
    pub project_id: String,
    #[serde(default)]
    pub worktree: Option<String>,
    pub name: String,
    /// 병합 안 된 브랜치도 지운다(`--force`). 창이 확인한 뒤에만 준다(결정 M2-D6 ①).
    #[serde(default)]
    pub force: bool,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitBranchDeleteResult {
    pub work_tree: String,
    /// 지운 브랜치와 그 이전 해시(`current: null`).
    pub deleted: GitRefMove,
}

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitTagCreateParams {
    pub project_id: String,
    #[serde(default)]
    pub worktree: Option<String>,
    pub name: String,
    /// 태그를 달 리비전. 없으면 HEAD.
    #[serde(default)]
    pub rev: Option<String>,
    /// 있으면 주석 태그의 메시지. 없으면 가벼운 태그.
    #[serde(default)]
    pub message: Option<String>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitTagResult {
    pub work_tree: String,
    /// 전체 이름(`refs/tags/…`).
    pub ref_name: String,
    /// 태그가 가리키는 커밋(주석 태그면 벗긴 것).
    pub commit: Option<String>,
}

rpc_method! {
    /// 체크아웃: 로컬 브랜치, 원격 브랜치(추적 브랜치를 만든다), 커밋(분리된 HEAD). 바뀔 경로가 미저장 문서와 겹치면 창이 먼저
    /// 확인한다. 로컬 변경 때문에 못 하는 등 git 이 거절하면 그 원문으로 실패한다(`2004`). 잠금을 기다리는 사이 HEAD · 대상이
    /// 움직였으면 아무것도 하지 않고 conflict.
    pub GitCheckoutMethod: "git.checkout" {
        params: GitCheckoutParams,
        result: GitCheckoutResult,
        tier: Write,
        roles: [Owner],
    }
}

rpc_method! {
    /// 브랜치를 만든다(`git branch`, `checkout` 이면 `git switch --create`).
    pub GitBranchCreateMethod: "git.branch.create" {
        params: GitBranchCreateParams,
        result: GitBranchResult,
        tier: Write,
        roles: [Owner],
    }
}

rpc_method! {
    /// 브랜치 이름을 바꾼다(`git branch --move`). 새 이름이 이미 있으면 git 의 원문으로 실패한다.
    pub GitBranchRenameMethod: "git.branch.rename" {
        params: GitBranchRenameParams,
        result: GitBranchResult,
        tier: Write,
        roles: [Owner],
    }
}

rpc_method! {
    /// 브랜치를 지운다(`git branch --delete`). 병합 안 된 브랜치는 `force` 없이 지우지 않는다 — `2101`(git 의 원문을 실어서).
    /// 결과의 이전 해시로 창이 [되돌리기] 를 단다.
    pub GitBranchDeleteMethod: "git.branch.delete" {
        params: GitBranchDeleteParams,
        result: GitBranchDeleteResult,
        tier: Write,
        roles: [Owner],
    }
}

rpc_method! {
    /// 태그를 만든다(`git tag`, 메시지가 있으면 주석 태그).
    pub GitTagCreateMethod: "git.tag.create" {
        params: GitTagCreateParams,
        result: GitTagResult,
        tier: Write,
        roles: [Owner],
    }
}

/// 핸들러를 붙인다.
pub fn register(router: &mut Router, write: Arc<GitWrite>) {
    let w = Arc::clone(&write);
    router.unary(GitCheckoutMethod, move |_ctx, p: GitCheckoutParams| {
        let w = Arc::clone(&w);
        async move {
            let (work_tree, head) = super::checkout(&w, p.project_id, p.worktree, p.target).await?;
            Ok(GitCheckoutResult { work_tree, head })
        }
    });
    let w = Arc::clone(&write);
    router.unary(
        GitBranchCreateMethod,
        move |_ctx, p: GitBranchCreateParams| {
            let w = Arc::clone(&w);
            async move {
                let (branch, head) =
                    super::create(&w, p.project_id, p.worktree, p.name, p.start, p.checkout)
                        .await?;
                Ok(GitBranchResult {
                    work_tree: branch.work_tree,
                    ref_name: branch.ref_name,
                    commit: branch.commit,
                    head,
                })
            }
        },
    );
    let w = Arc::clone(&write);
    router.unary(
        GitBranchRenameMethod,
        move |_ctx, p: GitBranchRenameParams| {
            let w = Arc::clone(&w);
            async move {
                let branch =
                    super::rename(&w, p.project_id, p.worktree, p.name, p.new_name).await?;
                Ok(GitBranchResult {
                    work_tree: branch.work_tree,
                    ref_name: branch.ref_name,
                    commit: branch.commit,
                    head: None,
                })
            }
        },
    );
    let w = Arc::clone(&write);
    router.unary(
        GitBranchDeleteMethod,
        move |_ctx, p: GitBranchDeleteParams| {
            let w = Arc::clone(&w);
            async move {
                match super::delete(&w, p.project_id, p.worktree, p.name, p.force).await? {
                    Deleted::Deleted { work_tree, moved } => Ok(GitBranchDeleteResult {
                        work_tree,
                        deleted: moved,
                    }),
                    Deleted::NotMerged { stderr, commit } => {
                        Err::<_, RpcError>(Deleted::not_merged(stderr, commit))
                    }
                }
            }
        },
    );
    let w = Arc::clone(&write);
    router.unary(GitTagCreateMethod, move |_ctx, p: GitTagCreateParams| {
        let w = Arc::clone(&w);
        async move {
            let tag = super::tag(&w, p.project_id, p.worktree, p.name, p.rev, p.message).await?;
            Ok(GitTagResult {
                work_tree: tag.work_tree,
                ref_name: tag.ref_name,
                commit: tag.commit,
            })
        }
    });
}
