//! 원격: fetch · pull · push(13-m2-plan.md M2-09 — F5-11, 결정 M2-D6 · M2-D8). 셋 다 원격에 닿아 인증을 물을 수 있으므로
//! askpass 를 켠다([`WriteRequest::askpass`]). git 이 거절하는 것(앞선 원격으로의 일반 push, upstream 없는 pull)은 그 원문으로
//! 실패한다(결정 M2-D6 ④).
//!
//! - fetch: 원격 하나 또는 전부(`--all`), prune 선택. 작업 트리를 바꾸지 않는다.
//! - pull: 지금 브랜치의 upstream(또는 준 원격 · 브랜치)을 merge(`--no-rebase`)나 rebase(`--rebase`)로. 작업 트리 전체를 미저장
//!   확인에 싣는다. 충돌로 멈추면 실패가 아니라 진행 중 작업이 남은 `stopped` 다 — 계속 · 중단 · 건너뛰기는 M2-06 의
//!   `git.operation.*`(결정 M2-D5).
//! - push: 로컬 브랜치 하나를 `refs/heads/<브랜치>:<대상>` 으로 보낸다. 대상은 터미널의 `git push` 가 보낼 곳을 git 의 규칙대로 셈한
//!   것이고(설정의 push refspec · `push.default`), git 이 거절할 상황이면 아무것도 보내지 않고 [`code::PUSH_REFUSED`] 로 거절한다
//!   ([`target`]). 그 대상을 명시해 보내므로 lease 의 ref 가 실제로 바뀌는 ref 와 같다. 창은 `git.push.plan` 으로 같은 계산을 먼저
//!   본다 — 창이 그 계획의 원격 · 대상을 `expectedTarget` 로 주면, 계획 뒤에 설정이 바뀌어 다시 센 대상이 다를 때 아무것도
//!   보내지 않고 [`pw_rpc::code::CONFLICT`] 로 끝낸다. upstream 설정은 선택이다. **force 는 `--force-with-lease=<대상>:<마지막으로 본 원격 해시>` + `--force-if-includes` 한 꼴뿐이다**(결정 M2-D6 ②, [`push_args`])
//!   — 입력에 `force` 가 없고(모르는 필드는 거절한다), refspec 은 언제나 `refs/heads/` 로 시작해 `+` 가 붙을 자리가 없으며, 기대
//!   해시는 전체 16진 해시만 받는다(빈 기대값 = 「원격에 없어야 한다」 도 받지 않는다).

pub mod methods;
pub mod target;
#[cfg(all(test, unix))]
mod tests;

use std::sync::Arc;

use pw_rpc::RpcError;
use serde::{Deserialize, Serialize};

pub use methods::register;
pub use target::{
    GitPushLease, GitPushRefusal, GitPushRefusalReason, GitPushTargetSource, GitPushTerminal,
    Planned,
};

use crate::branch::{branch_name, plain, work_tree};
use crate::write::{Changes, GitWrite, OperationInfo, WriteError, WriteRequest};
use crate::{Git, Opened};

/// 이 모듈의 애플리케이션 오류 코드(`crate::write::code` · `crate::branch::code` 와 겹치지 않는 범위). `data` 는 객체다.
pub mod code {
    /// git 이 거절할 push 라 아무것도 보내지 않았다(`data` 는 [`super::GitPushRefusal`] — 까닭 · 설명 · 고를 만한 대상). 창은 대상을
    /// 골라 `remoteRef` 로 명시해 다시 부른다.
    pub const PUSH_REFUSED: i32 = 2102;
}

/// 응답 · 알림에 싣는 원격: 주소(`<scheme>://`)면 사용자 정보(`user:token@`)를 지운다. 이름 · scp 꼴(`git@host:path`)은 그대로다.
pub fn redact(remote: &str) -> String {
    let Some((scheme, rest)) = remote.split_once("://") else {
        return remote.to_owned();
    };
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    match rest[..authority_end].rfind('@') {
        Some(at) => format!("{scheme}://{}", &rest[at + 1..]),
        None => remote.to_owned(),
    }
}

/// pull 이 upstream 을 들이는 방법.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum GitPullMode {
    /// `git pull --no-rebase`.
    Merge,
    /// `git pull --rebase`.
    Rebase,
}

/// `git.fetch`: `remote` 가 없으면 원격 전부(`--all`).
pub async fn fetch(
    write: &Arc<GitWrite>,
    project_id: String,
    worktree: Option<String>,
    remote: Option<String>,
    prune: bool,
) -> Result<String, WriteError> {
    if let Some(remote) = &remote {
        plain("remote", remote)?;
    }
    let request = WriteRequest {
        project_id,
        worktree,
        title: "task-title-git-fetch".to_owned(),
        changes: Changes::None,
        askpass: true,
    };
    write
        .run(request, move |ctx| async move {
            let mut command = ctx.git().arg("fetch");
            if prune {
                command = command.arg("--prune");
            }
            command = match remote {
                Some(remote) => command.args(["--end-of-options".to_owned(), remote]),
                None => command.arg("--all"),
            };
            ctx.run(command).await?;
            Ok(work_tree(&ctx))
        })
        .await
}

/// pull 의 결과.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pulled {
    pub work_tree: String,
    /// 끝난 뒤의 진행 중 작업.
    pub operation: Option<OperationInfo>,
    /// 충돌로 멈췄다(`operation` 이 그 작업이다).
    pub stopped: bool,
}

/// `git.pull`. `branch` 는 `remote` 와 함께만 준다.
pub async fn pull(
    write: &Arc<GitWrite>,
    project_id: String,
    worktree: Option<String>,
    mode: GitPullMode,
    remote: Option<String>,
    branch: Option<String>,
) -> Result<Pulled, WriteError> {
    if let Some(remote) = &remote {
        plain("remote", remote)?;
    }
    if let Some(branch) = &branch {
        if remote.is_none() {
            return Err(WriteError::Rpc(RpcError::invalid_params(
                "a pull branch needs its remote",
            )));
        }
        branch_name("branch", branch)?;
    }
    let request = WriteRequest {
        project_id,
        worktree,
        title: "task-title-git-pull".to_owned(),
        changes: Changes::WorkTree,
        askpass: true,
    };
    write
        .run(request, move |ctx| async move {
            let before = ctx.read(|o| crate::write::operation::read(&o.repo)).await?;
            let mut command = ctx.git().args([
                "pull",
                match mode {
                    GitPullMode::Merge => "--no-rebase",
                    GitPullMode::Rebase => "--rebase",
                },
            ]);
            if let Some(remote) = remote {
                command = command.args(["--end-of-options".to_owned(), remote]);
                command = command.args(branch);
            }
            let output = ctx.output(command).await?;
            let operation = ctx.read(|o| crate::write::operation::read(&o.repo)).await?;
            if output.status.success() {
                return Ok(Pulled {
                    work_tree: work_tree(&ctx),
                    operation,
                    stopped: false,
                });
            }
            // 시작 전에 없던 진행 중 작업이 남았으면 충돌로 멈춘 것이다 — 다음은 충돌을 풀고 계속할 차례다.
            if before.is_none() && operation.is_some() {
                return Ok(Pulled {
                    work_tree: work_tree(&ctx),
                    operation,
                    stopped: true,
                });
            }
            Err(WriteError::failed("pull", &output))
        })
        .await
}

/// 창이 기대하는 push 대상: `git.push.plan` 의 `remote`(주소면 사용자 정보를 지운 것) · `remoteRef` 그대로.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GitPushExpectedTarget {
    pub remote: String,
    pub remote_ref: String,
}

/// 보낼 것: 로컬 브랜치, 원격, 원격에서 바뀔 ref([`Planned::ready`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushPlan {
    /// 로컬 브랜치의 짧은 이름.
    pub branch: String,
    /// 원격 이름(또는 주소 원문 — git 에 그대로 넘긴다).
    pub remote: String,
    /// 원격의 전체 ref 이름.
    pub remote_ref: String,
}

impl PushPlan {
    /// 보내는 refspec(`refs/heads/<브랜치>:<대상>`, [`push_args`] 의 마지막 인자).
    pub fn refspec(&self) -> String {
        format!("refs/heads/{}:{}", self.branch, self.remote_ref)
    }
}

/// 기대 해시: 전체 16진 해시(SHA-1 40자 · SHA-256 64자)만.
fn full_hash(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|b| b.is_ascii_hexdigit())
}

/// push 의 인자(하위 명령부터). force 는 `lease`(마지막으로 본 원격 해시)가 있을 때의 `--force-with-lease=<대상>:<해시>` +
/// `--force-if-includes` 한 꼴뿐이다 — 이 함수 말고 push 인자를 만드는 곳은 없다.
pub fn push_args(plan: &PushPlan, set_upstream: bool, lease: Option<&str>) -> Vec<String> {
    let mut args = vec!["push".to_owned()];
    if set_upstream {
        args.push("--set-upstream".into());
    }
    if let Some(expected) = lease {
        args.push(format!("--force-with-lease={}:{expected}", plan.remote_ref));
        args.push("--force-if-includes".into());
    }
    args.extend([
        "--end-of-options".into(),
        plan.remote.clone(),
        plan.refspec(),
    ]);
    args
}

/// [`code::PUSH_REFUSED`] 오류.
pub fn refused(refusal: &GitPushRefusal) -> RpcError {
    RpcError::new(code::PUSH_REFUSED, refusal.message.clone())
        .with_data(serde_json::to_value(refusal).unwrap_or_default())
}

/// push 의 입력(`git.push` · `git.push.plan` 이 같다): 이름 꼴을 git 을 띄우기 전에 거절한다.
fn check_push_input(
    branch: Option<&str>,
    remote: Option<&str>,
    remote_ref: Option<&str>,
) -> Result<(), RpcError> {
    if let Some(branch) = branch {
        branch_name("branch", branch).map_err(RpcError::from)?;
    }
    if let Some(remote) = remote {
        plain("remote", remote).map_err(RpcError::from)?;
    }
    if let Some(remote_ref) = remote_ref {
        target::remote_ref_input(remote_ref).map_err(RpcError::invalid_params)?;
    }
    Ok(())
}

/// `git.push.plan`: 보낼 곳을 셈한다(gix 만 — 미신뢰 프로젝트에서도 프로세스를 띄우지 않는다). 작업 트리와 계획.
pub async fn plan(
    git: &Arc<Git>,
    project_id: String,
    worktree: Option<String>,
    branch: Option<String>,
    remote: Option<String>,
    remote_ref: Option<String>,
) -> Result<(String, Planned), RpcError> {
    check_push_input(branch.as_deref(), remote.as_deref(), remote_ref.as_deref())?;
    let git = Arc::clone(git);
    tokio::task::spawn_blocking(move || {
        let opened: Opened = git
            .open(&project_id, worktree.as_deref())?
            .ok_or_else(|| RpcError::from(WriteError::NoRepository))?;
        let planned = target::plan(
            &opened.repo,
            branch.as_deref(),
            remote.as_deref(),
            remote_ref.as_deref(),
        )
        .map_err(RpcError::invalid_params)?;
        let work_tree = pw_core::registry::path::normalize(&opened.target.work_tree)
            .unwrap_or_else(|_| opened.target.work_tree.clone())
            .display()
            .to_string();
        Ok((work_tree, planned))
    })
    .await
    .map_err(|e| RpcError::internal(e.to_string()))?
}

/// 계획 뒤에 대상이 바뀌었다(설정이 바뀌었다) — 창이 본 곳이 아니라 아무것도 보내지 않았다. `data` 는 지금 셈한 대상.
fn target_moved(actual: &GitPushExpectedTarget) -> WriteError {
    WriteError::Rpc(
        RpcError::new(
            pw_rpc::code::CONFLICT,
            format!(
                "the push target is now `{}` on `{}`, not the planned one (the git config changed); nothing was pushed",
                actual.remote_ref, actual.remote
            ),
        )
        .with_data(serde_json::to_value(actual).unwrap_or_default()),
    )
}

/// push 의 결과.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pushed {
    pub work_tree: String,
    pub plan: PushPlan,
    /// 터미널의 `git push` 와 다르면 그 내용([`Planned::terminal_difference`]).
    pub terminal: Option<GitPushTerminal>,
}

/// `git.push`. 대상은 [`target::plan`] 이 셈한다 — `remote_ref` 가 있으면 그것. `expected` 가 있으면 셈한 원격 · 대상이 그것과
/// 같을 때만 보낸다(아니면 [`pw_rpc::code::CONFLICT`]). `lease` 가 있으면 force push 다(창이 경고한 뒤에만, 결정 M2-D6 ①).
#[allow(clippy::too_many_arguments)] // RPC 입력의 칸을 그대로 받는다.
pub async fn push(
    write: &Arc<GitWrite>,
    project_id: String,
    worktree: Option<String>,
    branch: Option<String>,
    remote: Option<String>,
    remote_ref: Option<String>,
    set_upstream: bool,
    lease: Option<String>,
    expected: Option<GitPushExpectedTarget>,
) -> Result<Pushed, WriteError> {
    check_push_input(branch.as_deref(), remote.as_deref(), remote_ref.as_deref())
        .map_err(WriteError::Rpc)?;
    if let Some(expected) = &lease
        && !full_hash(expected)
    {
        return Err(WriteError::Rpc(RpcError::invalid_params(format!(
            "the force-with-lease hash `{expected}` must be a full hexadecimal object id"
        ))));
    }
    let request = WriteRequest {
        project_id,
        worktree,
        title: "task-title-git-push".to_owned(),
        changes: Changes::None,
        askpass: true,
    };
    write
        .run(request, move |ctx| async move {
            // 잠금 안에서 셈한다 — 셈한 뒤 다른 쓰기가 설정 · ref 를 바꾸지 못한다.
            let planned = ctx
                .read(move |o| {
                    target::plan(
                        &o.repo,
                        branch.as_deref(),
                        remote.as_deref(),
                        remote_ref.as_deref(),
                    )
                })
                .await?
                .map_err(|e| WriteError::Rpc(RpcError::invalid_params(e)))?;
            let plan = planned
                .ready()
                .map_err(|refusal| WriteError::Rpc(refused(&refusal)))?;
            if let Some(expected) = &expected {
                let actual = GitPushExpectedTarget {
                    remote: redact(&plan.remote),
                    remote_ref: plan.remote_ref.clone(),
                };
                if actual != *expected {
                    return Err(target_moved(&actual));
                }
            }
            ctx.run(
                ctx.git()
                    .args(push_args(&plan, set_upstream, lease.as_deref())),
            )
            .await?;
            Ok(Pushed {
                work_tree: work_tree(&ctx),
                terminal: planned.terminal_difference(),
                plan,
            })
        })
        .await
}
