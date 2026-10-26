//! 원격: fetch · pull · push(13-m2-plan.md M2-09 — F5-11, 결정 M2-D6 · M2-D8). 셋 다 원격에 닿아 인증을 물을 수 있으므로
//! askpass 를 켠다([`WriteRequest::askpass`]). git 이 거절하는 것(앞선 원격으로의 일반 push, upstream 없는 pull)은 그 원문으로
//! 실패한다(결정 M2-D6 ④).
//!
//! - fetch: 원격 하나 또는 전부(`--all`), prune 선택. 작업 트리를 바꾸지 않는다.
//! - pull: 지금 브랜치의 upstream(또는 준 원격 · 브랜치)을 merge(`--no-rebase`)나 rebase(`--rebase`)로. 작업 트리 전체를 미저장
//!   확인에 싣는다. 충돌로 멈추면 실패가 아니라 진행 중 작업이 남은 `stopped` 다 — 계속 · 중단 · 건너뛰기는 M2-06 의
//!   `git.operation.*`(결정 M2-D5).
//! - push: 로컬 브랜치 하나를 `refs/heads/<브랜치>:<대상>` 으로 보낸다 — 설정의 push refspec · `push.default` 를 쓰지 않고 무엇이
//!   어디로 가는지를 여기서 정한다([`push_plan`]). 그래야 lease 의 ref 가 실제로 바뀌는 ref 와 같다. upstream 설정은 선택이다.
//!   **force 는 `--force-with-lease=<대상>:<마지막으로 본 원격 해시>` + `--force-if-includes` 한 꼴뿐이다**(결정 M2-D6 ②, [`push_args`])
//!   — 입력에 `force` 가 없고(모르는 필드는 거절한다), refspec 은 언제나 `refs/heads/` 로 시작해 `+` 가 붙을 자리가 없으며, 기대
//!   해시는 전체 16진 해시만 받는다(빈 기대값 = 「원격에 없어야 한다」 도 받지 않는다).

pub mod methods;
#[cfg(all(test, unix))]
mod tests;

use std::sync::Arc;

use gix::bstr::ByteSlice;
use pw_rpc::RpcError;
use serde::{Deserialize, Serialize};

pub use methods::register;

use crate::branch::{plain, work_tree};
use crate::write::{Changes, GitWrite, OperationInfo, WriteError, WriteRequest};

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
        plain("branch", branch)?;
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

/// push 할 것: 로컬 브랜치, 원격, 원격에서 바뀔 ref.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushPlan {
    /// 로컬 브랜치의 짧은 이름.
    pub branch: String,
    pub remote: String,
    /// 원격의 전체 ref 이름(`refs/heads/…`).
    pub remote_ref: String,
}

/// 보낼 곳을 정한다. 브랜치는 `branch`(없으면 HEAD 의 브랜치 — 분리된 HEAD 면 오류). 원격은 `remote`, 없으면 git 의 push 원격
/// 차례(`branch.<b>.pushRemote` → `remote.pushDefault` → `branch.<b>.remote` → `origin`). 대상은 그 원격이 그 브랜치의 upstream
/// 원격이고 `branch.<b>.merge` 가 브랜치면 그것, 아니면 같은 이름이다. 설정 값은 이름일 뿐 아무것도 띄우지 않으므로 신뢰와 상관없이
/// 저장소 설정에서 읽는다(`crate::status::refs` 와 같다).
pub fn push_plan(
    repo: &gix::Repository,
    branch: Option<&str>,
    remote: Option<&str>,
) -> Result<PushPlan, String> {
    let branch = match branch {
        Some(branch) => branch.to_owned(),
        None => {
            let head = repo
                .head_name()
                .map_err(|e| e.to_string())?
                .ok_or("HEAD is detached; choose a branch to push")?;
            head.as_bstr()
                .strip_prefix(b"refs/heads/")
                .ok_or("HEAD is not on a local branch; choose a branch to push")?
                .to_str_lossy()
                .into_owned()
        }
    };
    let config = repo.config_snapshot();
    let plumbing = config.plumbing();
    let value = |section: &str, subsection: Option<&str>, key: &str| {
        plumbing
            .string_by(section, subsection.map(|s| s.as_bytes().as_bstr()), key)
            .map(|v| v.to_str_lossy().into_owned())
            .filter(|v| !v.is_empty())
    };
    let upstream_remote = value("branch", Some(&branch), "remote");
    let remote = remote
        .map(str::to_owned)
        .or_else(|| value("branch", Some(&branch), "pushRemote"))
        .or_else(|| value("remote", None, "pushDefault"))
        .or_else(|| upstream_remote.clone())
        .unwrap_or_else(|| "origin".to_owned());
    let merge = value("branch", Some(&branch), "merge")
        .map(|merge| {
            if merge.starts_with("refs/") {
                merge
            } else {
                format!("refs/heads/{merge}")
            }
        })
        .filter(|merge| merge.starts_with("refs/heads/"));
    let remote_ref = match merge {
        Some(merge) if upstream_remote.as_deref() == Some(remote.as_str()) => merge,
        _ => format!("refs/heads/{branch}"),
    };
    Ok(PushPlan {
        branch,
        remote,
        remote_ref,
    })
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

/// 설정의 push refspec(`remote.<원격>.push`)이 이 push 와 다른 곳을 가리킨다 — 사람이 터미널에서 인자 없이 `git push` 하면 갈
/// 곳과 이 push 가 간 곳이 다르다. 푸시 다이얼로그(M2-20)가 경고로 보인다.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitPushConfigured {
    /// 설정에 적힌 push refspec 원문(차례대로).
    pub refspecs: Vec<String>,
    /// 그 refspec 들이 이 브랜치를 보낼 원격 ref. `null` 이면 이 브랜치를 보내지 않는다.
    pub destination: Option<String>,
}

/// `plan` 의 원격에 push refspec 이 설정돼 있고 그것이 이 브랜치를 `plan.remote_ref` 가 아닌 곳으로 보내거나 보내지 않으면 그
/// 내용. 설정이 없거나 같은 곳이면 `None`. `:`(matching) 처럼 원본이 없는 refspec 은 같은 이름으로 본다.
pub fn configured_push(repo: &gix::Repository, plan: &PushPlan) -> Option<GitPushConfigured> {
    let config = repo.config_snapshot();
    let refspecs: Vec<String> = config
        .plumbing()
        .strings_by("remote", Some(plan.remote.as_bytes().as_bstr()), "push")?
        .iter()
        .map(|spec| spec.to_str_lossy().into_owned())
        .collect();
    if refspecs.is_empty() {
        return None;
    }
    let local = format!("refs/heads/{}", plan.branch);
    let parsed: Vec<gix::refspec::RefSpec> = refspecs
        .iter()
        .filter_map(|spec| {
            gix::refspec::parse(
                spec.as_bytes().as_bstr(),
                gix::refspec::parse::Operation::Push,
            )
            .ok()
            .map(|s| s.to_owned())
        })
        .collect();
    let matching = parsed.iter().any(|spec| spec.to_ref().source().is_none());
    let group = gix::refspec::MatchGroup {
        specs: parsed
            .iter()
            .map(gix::refspec::RefSpec::to_ref)
            .filter(|spec| spec.source().is_some())
            .collect(),
    };
    let null = repo.object_hash().null();
    let out = group.match_lhs(std::iter::once(gix::refspec::match_group::Item {
        full_ref_name: local.as_bytes().as_bstr(),
        target: &null,
        object: None,
    }));
    let destination = out
        .mappings
        .into_iter()
        .next()
        .map(|m| match m.rhs {
            Some(rhs) => rhs.to_str_lossy().into_owned(),
            None => m.lhs.to_string(),
        })
        .or_else(|| matching.then(|| local.clone()));
    (destination.as_deref() != Some(plan.remote_ref.as_str())).then_some(GitPushConfigured {
        refspecs,
        destination,
    })
}

/// push 의 결과.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pushed {
    pub work_tree: String,
    pub plan: PushPlan,
    /// 설정의 push refspec 이 다른 곳을 가리키면 그 내용([`configured_push`]).
    pub configured: Option<GitPushConfigured>,
}

impl PushPlan {
    /// 보낸 refspec(`refs/heads/<브랜치>:<대상>`, [`push_args`] 의 마지막 인자).
    pub fn refspec(&self) -> String {
        format!("refs/heads/{}:{}", self.branch, self.remote_ref)
    }
}

/// `git.push`. `lease` 가 있으면 force push 다(창이 경고한 뒤에만, 결정 M2-D6 ①).
pub async fn push(
    write: &Arc<GitWrite>,
    project_id: String,
    worktree: Option<String>,
    branch: Option<String>,
    remote: Option<String>,
    set_upstream: bool,
    lease: Option<String>,
) -> Result<Pushed, WriteError> {
    if let Some(branch) = &branch {
        plain("branch", branch)?;
    }
    if let Some(remote) = &remote {
        plain("remote", remote)?;
    }
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
            let plan = ctx
                .read(move |o| {
                    push_plan(&o.repo, branch.as_deref(), remote.as_deref())
                        .map(|plan| (configured_push(&o.repo, &plan), plan))
                })
                .await?
                .map_err(|e| WriteError::Rpc(RpcError::invalid_params(e)))?;
            let (configured, plan) = plan;
            ctx.run(
                ctx.git()
                    .args(push_args(&plan, set_upstream, lease.as_deref())),
            )
            .await?;
            Ok(Pushed {
                work_tree: work_tree(&ctx),
                plan,
                configured,
            })
        })
        .await
}
