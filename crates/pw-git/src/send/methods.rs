//! `git.send.*` RPC(18-m7-plan.md M7-16 — 「공유 부품의 주인」: M7-17 · 19 는 부르기만, M7-19 가 에이전트 갈래의 `git.send.plan` ·
//! `git.send.compare` 와 승인 표를 더했다 — 메인 판정). MCP 툴이 아니다 — MCP 세션의 세션 툴 `checkout_send` 가 부른다.
//!
//! 한 번의 보내기: Desktop 이 대상에서 `git.send.theirs` 로 대상이 가진 커밋 · 브랜치 끝을 읽고 → 원본의 `git.send.prepare` 스트림을
//! 대상의 `git.send.receive.begin` · `chunk` · `finish` 로 흘리고(바이트를 디스크에 두지 않는다) → 대상의 `git.send.apply`.

use std::sync::Arc;

use base64::Engine as _;
use gix::bstr::ByteSlice;
use pw_rpc::{Router, RpcError, rpc_method};
use serde::{Deserialize, Serialize};

use pw_core::registry::plan::Planned;

use super::agent::SendTarget;
use super::apply::{Apply, GitSendOutcome};
use super::compare::{GitSendCompare, GitSendCompareParams};
use super::receive::{Begin, UploadOwner, Uploads};
use super::{CHUNK, INCOMING_PREFIX, branch_name, ref_target};
use crate::write::{GitWrite, WriteError};

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitSendTheirsParams {
    pub project_id: String,
    #[serde(default)]
    pub worktree: Option<String>,
    /// 보낼 브랜치(짧은 이름). 그 끝이 결과의 `branchTip` 이다.
    pub branch: String,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitSendTheirs {
    /// 이 저장소가 가진 커밋: 로컬 브랜치 · 전용 ref(`refs/pitwall/incoming/…`)의 끝(겹치지 않게, 정렬). 원본 `prepare` 의
    /// `theirs` 로 넘긴다.
    pub theirs: Vec<String>,
    /// 그 브랜치의 지금 끝. 없으면 `null`. `apply` 의 `expected` 로 넘긴다.
    pub branch_tip: Option<String>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitSendPrepareParams {
    pub project_id: String,
    #[serde(default)]
    pub worktree: Option<String>,
    /// 묶음 안의 ref 이름(`refs/heads/<branch>`) — 대상은 이 이름으로만 가져온다.
    pub branch: String,
    /// 보낼 커밋(전체 id). 그 커밋과 조상만 묶음에 든다 — 그사이 브랜치에 더한 커밋은 가지 않는다.
    pub commit: String,
    /// 대상이 가진 커밋(`git.send.theirs`). 원본에 없는 것은 무시한다.
    #[serde(default)]
    pub theirs: Vec<String>,
}

/// `git.send.prepare` 의 항목: `info` 하나 → `data` …(offset 순서).
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum GitSendPrepareItem {
    Info {
        /// 묶음 전체 바이트 수(`receive.begin` 의 `size`).
        #[specta(type = pw_rpc::JsNumber)]
        size: u64,
        /// 원본에 커밋 안 한 변경이 있다 — 그 변경은 가지 않는다(막지 않는다).
        uncommitted: bool,
        /// 묶음에서 뺀 커밋(대상이 가졌다고 본 것).
        excluded: Vec<String>,
    },
    Data {
        #[specta(type = pw_rpc::JsNumber)]
        offset: u64,
        /// base64. 디코드하면 512 KiB 이하.
        data: String,
    },
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitSendReceiveBeginParams {
    pub project_id: String,
    #[serde(default)]
    pub worktree: Option<String>,
    /// 원본 머신 id(`[A-Za-z0-9._-]`, 128자 이하) — 전용 ref 의 한 마디.
    pub source_machine: String,
    pub branch: String,
    /// 묶음이 실은 `refs/heads/<branch>` 의 커밋이어야 한다.
    pub commit: String,
    /// 보낼 바이트 수(1 GiB 이하). 이것을 넘는 바이트는 받지 않는다.
    #[specta(type = pw_rpc::JsNumber)]
    pub size: u64,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitSendUpload {
    /// 이 연결 안에서만 유효하다.
    #[specta(type = pw_rpc::JsNumber)]
    pub upload_id: u64,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitSendReceiveChunkParams {
    #[specta(type = pw_rpc::JsNumber)]
    pub upload_id: u64,
    /// 이 청크의 시작 — 지금까지 받은 바이트 수와 같아야 한다.
    #[specta(type = pw_rpc::JsNumber)]
    pub offset: u64,
    /// base64. 디코드하면 1 MiB 이하.
    pub data: String,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitSendReceived {
    /// 지금까지 받은 바이트 수.
    #[specta(type = pw_rpc::JsNumber)]
    pub received: u64,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitSendReceiveFinishResult {
    pub work_tree: String,
    /// 받은 커밋을 둔 전용 ref(`refs/pitwall/incoming/<원본 머신>/<브랜치>`).
    pub incoming_ref: String,
    pub commit: String,
    /// 받기 전 그 전용 ref 의 커밋.
    pub previous: Option<String>,
    /// 이번에 치운 전용 ref(커밋이 같은 이름의 브랜치에 든 것).
    pub pruned: Vec<String>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitSendApplyParams {
    pub project_id: String,
    #[serde(default)]
    pub worktree: Option<String>,
    pub source_machine: String,
    pub branch: String,
    /// 받은 커밋(`receive.finish` 의 `commit`).
    pub commit: String,
    /// 그 브랜치의 끝이어야 하는 것(`git.send.theirs` 의 `branchTip`). `null` 은 「없어야 한다」(만든다).
    pub expected: Option<String>,
}

/// `git.send.plan` 의 입력: 대상이 바꿀 것(`super::agent::SendTarget`).
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitSendPlanParams {
    pub project_id: String,
    #[serde(default)]
    pub worktree: Option<String>,
    pub source_machine: String,
    pub branch: String,
    /// 받을 커밋(원본 끝 — 계획 때 읽은 것).
    pub commit: String,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitSendPlan {
    /// 대상 계획의 해시(`sha256:<hex>`). MCP 세션의 받기 · 맞추기는 이것을 승인 표에 싣는다.
    pub hash: String,
    /// 해시에 든 그 브랜치의 지금 끝. 없으면 `null`.
    pub branch_tip: Option<String>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitSendApplyResult {
    pub work_tree: String,
    /// 받은 커밋이 있는 전용 ref(멈췄으면 사람이 거기서 병합한다).
    pub incoming_ref: String,
    pub outcome: GitSendOutcome,
}

rpc_method! {
    /// 커밋 보내기의 대상 쪽 읽기: 이 저장소가 가진 커밋(로컬 브랜치 · 전용 ref 끝)과 그 브랜치의 지금 끝.
    pub GitSendTheirsMethod: "git.send.theirs" {
        params: GitSendTheirsParams,
        result: GitSendTheirs,
        tier: Read,
        roles: [Owner],
    }
}

rpc_method! {
    /// 에이전트 갈래(MCP 세션의 `checkout_send`)의 대상 계획: 대상이 바꿀 것과 지금 읽은 브랜치 끝으로 만든 계획 해시. 세션이 사람의
    /// 승인을 받은 뒤 그 해시를 승인 표로 `receive.begin` · `apply` 에 싣는다 — MCP 세션 범위의 두 메서드는 계획을 다시 만들어 표가
    /// 맞을 때만 돈다(`super::agent`).
    pub GitSendPlanMethod: "git.send.plan" {
        params: GitSendPlanParams,
        result: GitSendPlan,
        tier: Read,
        roles: [Owner],
    }
}

rpc_method! {
    /// 커밋 보내기의 원본 쪽 읽기(에이전트 갈래의 계획): 그 브랜치의 지금 끝, 대상이 가진 커밋에 없는 커밋 수, 대상 브랜치 끝이 그
    /// 조상인가(fast-forward), 커밋 안 한 변경이 있는가. 원본에 쓰지 않는다.
    pub GitSendCompareMethod: "git.send.compare" {
        params: GitSendCompareParams,
        result: GitSendCompare,
        tier: Read,
        roles: [Owner],
    }
}

rpc_method! {
    /// 커밋 보내기의 원본 쪽: `commit` 과 그 조상 가운데 `theirs` 에 없는 것의 `git bundle`(ref 는 `refs/heads/<branch>` 하나)을
    /// 흘린다. 원본 저장소에 쓰지 않는다. 1 GiB 를 넘으면 실패한다.
    pub GitSendPrepareMethod: "git.send.prepare" {
        params: GitSendPrepareParams,
        stream: GitSendPrepareItem,
        tier: Read,
        roles: [Owner],
    }
}

rpc_method! {
    /// 커밋 보내기의 대상 쪽 받기를 연다. 그 프로젝트 창의 연결, 또는 `git.send.plan` 의 해시를 승인 표로 실은 MCP 세션(쓰기).
    /// 연결이 닫히면 받기를 버린다.
    pub GitSendReceiveBeginMethod: "git.send.receive.begin" {
        params: Planned<GitSendReceiveBeginParams>,
        result: GitSendUpload,
        tier: Write,
        roles: [Owner],
    }
}

rpc_method! {
    /// 받기에 청크 하나를 잇는다. 빠지거나 겹치거나 크기를 넘는 청크는 거절하고 그 받기를 버린다.
    pub GitSendReceiveChunkMethod: "git.send.receive.chunk" {
        params: GitSendReceiveChunkParams,
        result: GitSendReceived,
        tier: Write,
        roles: [Owner],
    }
}

rpc_method! {
    /// 다 받은 묶음을 확인하고(`git bundle verify`) 그 브랜치만 전용 ref 로 가져온다. 다 쓴 전용 ref 를 먼저 치운다.
    pub GitSendReceiveFinishMethod: "git.send.receive.finish" {
        params: GitSendUpload,
        result: GitSendReceiveFinishResult,
        tier: Write,
        roles: [Owner],
    }
}

rpc_method! {
    /// 대상 맞추기: 받은 커밋으로 그 브랜치를 fast-forward(없으면 만들기)하고 체크아웃을 그 브랜치로 바꾼다. 갈라짐 · git 의 거절 ·
    /// 다른 워크트리가 쓰는 브랜치 · `expected` 와 다른 브랜치면 아무것도 바꾸지 않고 `stopped`. 받기와 같이 그 프로젝트 창의 연결,
    /// 또는 `git.send.plan` 의 해시를 승인 표로 실은 MCP 세션.
    pub GitSendApplyMethod: "git.send.apply" {
        params: Planned<GitSendApplyParams>,
        result: GitSendApplyResult,
        tier: Write,
        roles: [Owner],
    }
}

/// 핸들러를 붙인다(`crate::write::register` 가 부른다).
pub fn register(router: &mut Router, write: Arc<GitWrite>) {
    let uploads = Arc::new(Uploads::default());
    let w = Arc::clone(&write);
    router.unary(GitSendTheirsMethod, move |_ctx, p: GitSendTheirsParams| {
        let w = Arc::clone(&w);
        async move { Ok(theirs(&w, p).await?) }
    });
    let w = Arc::clone(&write);
    router.unary(GitSendPlanMethod, move |_ctx, p: GitSendPlanParams| {
        let w = Arc::clone(&w);
        async move {
            let target = SendTarget {
                project_id: p.project_id,
                worktree: p.worktree,
                source_machine: p.source_machine,
                branch: p.branch,
                commit: p.commit,
            };
            let (plan, branch_tip) = super::agent::planned(&w, &target).await?;
            Ok(GitSendPlan {
                hash: plan.hash,
                branch_tip,
            })
        }
    });
    let w = Arc::clone(&write);
    router.unary(
        GitSendCompareMethod,
        move |_ctx, p: GitSendCompareParams| {
            let w = Arc::clone(&w);
            async move { Ok(super::compare::compare(w.git(), p).await?) }
        },
    );
    let w = Arc::clone(&write);
    router.stream(
        GitSendPrepareMethod,
        move |ctx, p: GitSendPrepareParams, sink| {
            let w = Arc::clone(&w);
            async move {
                let cancel = ctx.cancellation().clone();
                let prepared = super::prepare::prepare(
                    w.git(),
                    &p.project_id,
                    p.worktree.as_deref(),
                    &p.branch,
                    &p.commit,
                    &p.theirs,
                    &cancel,
                )
                .await?;
                sink.send(GitSendPrepareItem::Info {
                    size: prepared.size,
                    uncommitted: prepared.uncommitted,
                    excluded: prepared.excluded.clone(),
                })
                .await?;
                let mut offset = 0u64;
                while offset < prepared.size {
                    let (path, at) = (prepared.file.clone(), offset);
                    let bytes = tokio::task::spawn_blocking(move || read_at(&path, at))
                        .await
                        .map_err(|e| RpcError::internal(e.to_string()))?
                        .map_err(|e| RpcError::internal(format!("cannot read the bundle: {e}")))?;
                    if bytes.is_empty() {
                        return Err(RpcError::internal("the bundle ended early"));
                    }
                    let len = bytes.len() as u64;
                    sink.send(GitSendPrepareItem::Data {
                        offset,
                        data: base64::engine::general_purpose::STANDARD.encode(&bytes),
                    })
                    .await?;
                    offset += len;
                }
                Ok(())
            }
        },
    );
    let (w, u) = (Arc::clone(&write), Arc::clone(&uploads));
    router.unary(
        GitSendReceiveBeginMethod,
        move |ctx, planned: Planned<GitSendReceiveBeginParams>| {
            let (w, u) = (Arc::clone(&w), Arc::clone(&u));
            async move {
                let Planned { params: p, ticket } = planned;
                let approved = super::agent::authorize(
                    &w,
                    ctx.scope(),
                    ticket.as_ref(),
                    &SendTarget {
                        project_id: p.project_id.clone(),
                        worktree: p.worktree.clone(),
                        source_machine: p.source_machine.clone(),
                        branch: p.branch.clone(),
                        commit: p.commit.clone(),
                    },
                )
                .await?;
                let begin = Begin {
                    caller: ctx.scope().clone(),
                    approved,
                    project_id: p.project_id,
                    worktree: p.worktree,
                    source_machine: p.source_machine,
                    branch: p.branch,
                    commit: p.commit,
                    size: p.size,
                };
                let upload_id = u
                    .begin(&w, UploadOwner::of(&ctx), begin, ctx.connection_closed())
                    .await?;
                Ok(GitSendUpload { upload_id })
            }
        },
    );
    let u = Arc::clone(&uploads);
    router.unary(
        GitSendReceiveChunkMethod,
        move |ctx, p: GitSendReceiveChunkParams| {
            let u = Arc::clone(&u);
            async move {
                let received = u
                    .chunk(UploadOwner::of(&ctx), p.upload_id, p.offset, &p.data)
                    .await?;
                Ok(GitSendReceived { received })
            }
        },
    );
    let (w, u) = (Arc::clone(&write), Arc::clone(&uploads));
    router.unary(GitSendReceiveFinishMethod, move |ctx, p: GitSendUpload| {
        let (w, u) = (Arc::clone(&w), Arc::clone(&u));
        async move {
            let r = u.finish(&w, UploadOwner::of(&ctx), p.upload_id).await?;
            Ok(GitSendReceiveFinishResult {
                work_tree: r.work_tree,
                incoming_ref: r.incoming_ref,
                commit: r.commit,
                previous: r.previous,
                pruned: r.pruned,
            })
        }
    });
    let w = Arc::clone(&write);
    router.unary(
        GitSendApplyMethod,
        move |ctx, planned: Planned<GitSendApplyParams>| {
            let w = Arc::clone(&w);
            async move {
                let Planned { params: p, ticket } = planned;
                let approved = super::agent::authorize(
                    &w,
                    ctx.scope(),
                    ticket.as_ref(),
                    &SendTarget {
                        project_id: p.project_id.clone(),
                        worktree: p.worktree.clone(),
                        source_machine: p.source_machine.clone(),
                        branch: p.branch.clone(),
                        commit: p.commit.clone(),
                    },
                )
                .await?;
                let incoming_ref = super::incoming_ref(&p.source_machine, &p.branch);
                let (work_tree, outcome) = super::apply::apply(
                    &w,
                    Apply {
                        caller: ctx.scope().clone(),
                        approved,
                        project_id: p.project_id,
                        worktree: p.worktree,
                        source_machine: p.source_machine,
                        branch: p.branch,
                        commit: p.commit,
                        expected: p.expected,
                    },
                )
                .await?;
                Ok(GitSendApplyResult {
                    work_tree,
                    incoming_ref,
                    outcome,
                })
            }
        },
    );
}

fn read_at(path: &std::path::Path, offset: u64) -> std::io::Result<Vec<u8>> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path)?;
    file.seek(SeekFrom::Start(offset))?;
    let mut bytes = Vec::with_capacity(CHUNK);
    file.take(CHUNK as u64).read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// `git.send.theirs`(gix 로 읽는다 — 미신뢰도 git CLI 를 띄우지 않는다).
pub async fn theirs(
    write: &Arc<GitWrite>,
    p: GitSendTheirsParams,
) -> Result<GitSendTheirs, WriteError> {
    branch_name(&p.branch)?;
    let git = Arc::clone(write.git());
    tokio::task::spawn_blocking(move || {
        let opened = git
            .open(&p.project_id, p.worktree.as_deref())
            .map_err(WriteError::Rpc)?
            .ok_or(WriteError::NoRepository)?;
        let repo = &opened.repo;
        let mut theirs = Vec::new();
        if let Ok(platform) = repo.references() {
            for prefix in ["refs/heads/", INCOMING_PREFIX] {
                if let Ok(refs) = platform.prefixed(prefix.as_bytes().as_bstr()) {
                    theirs.extend(
                        refs.flatten()
                            .filter_map(|mut r| r.peel_to_id().ok().map(|id| id.to_string())),
                    );
                }
            }
        }
        theirs.sort();
        theirs.dedup();
        Ok(GitSendTheirs {
            theirs,
            branch_tip: ref_target(repo, &format!("refs/heads/{}", p.branch))
                .map(|id| id.to_string()),
        })
    })
    .await
    .map_err(|e| WriteError::Internal(e.to_string()))?
}
