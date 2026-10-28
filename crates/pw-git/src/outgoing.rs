//! 푸시 다이얼로그(13-m2-plan.md M2-20)의 「보낼 커밋」 — `git.push.commits`. gix 로만 읽는다(미신뢰 프로젝트에서도 프로세스 0,
//! 연 저장소의 열기 옵션 그대로 — [`crate::repo::open_options`]).
//!
//! 보낼 커밋은 `git log <base>..refs/heads/<branch>` 다: 창이 `git.push.plan` 의 `lease.commit`(대상의 원격 추적 ref 가 가리키는 커밋 —
//! 원격에 있다고 아는 마지막 것)을 `base` 로 넘긴다. 원격 추적 ref 가 없으면(원격에 아직 없는 브랜치) `base` 없이 부르고, 그
//! 원격의 원격 추적 ref 전부(`refs/remotes/<remote>/*`)에서 닿는 커밋을 뺀다 — 원격에 이미 있다고 아는 커밋은 다시 보내지
//! 않는다(git 도 협상에서 빼므로 보내는 커밋 수와 같다). 원격이 그 사이 움직였는지는 모른다 — 네트워크를 보지 않는다.

use std::sync::Arc;

use pw_rpc::{Router, RpcError, rpc_method};
use serde::{Deserialize, Serialize};

use crate::Git;
use crate::log::commit::{Mailmap, Text, subject};

/// 한 번에 돌려주는 커밋의 상한.
pub const MAX_COMMITS: u32 = 500;

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitPushCommitsParams {
    pub project_id: String,
    #[serde(default)]
    pub worktree: Option<String>,
    /// 보낼 로컬 브랜치의 짧은 이름(`git.push.plan` 의 `branch`).
    pub branch: String,
    /// 원격 이름(`git.push.plan` 의 `remote`). `base` 가 없을 때 뺄 원격 추적 ref 의 원격이다.
    pub remote: String,
    /// 원격에 있다고 아는 커밋(전체 해시 — `git.push.plan` 의 `lease.commit`). 없으면 그 원격의 원격 추적 ref 전부를 뺀다.
    #[serde(default)]
    pub base: Option<String>,
    /// 1 ~ [`MAX_COMMITS`].
    pub limit: u32,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct OutgoingCommit {
    /// 전체 해시.
    pub hash: String,
    /// 메시지 첫 줄(git `%s`).
    pub subject: String,
    /// 작성자 이름(mailmap 적용).
    pub author: String,
    /// 작성 시각(유닉스 초).
    #[specta(type = pw_rpc::JsNumber)]
    pub author_time: i64,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitPushCommits {
    /// 저장소의 작업 트리 꼭대기. 저장소가 없으면 `null` 이고 목록은 비었다.
    pub work_tree: Option<String>,
    /// 보낼 커밋, 커밋 시각이 새것부터(`git log` 의 기본 차례).
    pub commits: Vec<OutgoingCommit>,
    /// `limit` 개에서 잘랐다.
    pub truncated: bool,
}

rpc_method! {
    /// 푸시 다이얼로그의 보낼 커밋: `base..refs/heads/<branch>`(`base` 가 없으면 그 원격의 원격 추적 ref 전부를 뺀다). gix 로만
    /// 읽는다(미신뢰 프로젝트에서도 프로세스 0). 없는 브랜치 · 커밋이면 invalid params.
    pub GitPushCommitsMethod: "git.push.commits" {
        params: GitPushCommitsParams,
        result: GitPushCommits,
        tier: Read,
        roles: [Owner],
    }
}

fn invalid(e: impl std::fmt::Display) -> RpcError {
    RpcError::invalid_params(e.to_string())
}

fn internal(e: impl std::fmt::Display) -> RpcError {
    RpcError::internal(e.to_string())
}

impl Git {
    /// `git.push.commits`.
    pub fn push_commits(&self, params: &GitPushCommitsParams) -> Result<GitPushCommits, RpcError> {
        if params.limit == 0 || params.limit > MAX_COMMITS {
            return Err(invalid(format!("limit must be 1 ~ {MAX_COMMITS}")));
        }
        // `git.push` 와 같은 이름 검사(브랜치는 `check-ref-format --branch`, 원격은 평범한 이름) — ref 이름에 그대로 붙이므로.
        crate::remote::check_push_input(Some(&params.branch), Some(&params.remote), None)?;
        // 원격 이름의 ref 규칙(`..` 등)은 아래 `prefixed` 가 거절한다(invalid params).
        let Some(opened) = self.open(&params.project_id, params.worktree.as_deref())? else {
            return Ok(GitPushCommits {
                work_tree: None,
                commits: Vec::new(),
                truncated: false,
            });
        };
        let repo = &opened.repo;
        let branch = format!("refs/heads/{}", params.branch);
        let tip = repo
            .try_find_reference(branch.as_str())
            .map_err(invalid)?
            .ok_or_else(|| invalid(format!("no local branch `{}`", params.branch)))?
            .into_fully_peeled_id()
            .map_err(internal)?
            .detach();
        let hidden: Vec<gix::ObjectId> = match &params.base {
            Some(base) => {
                let id = gix::ObjectId::from_hex(base.as_bytes()).map_err(invalid)?;
                if !repo.has_object(id) {
                    return Err(invalid(format!("no commit {base}")));
                }
                vec![id]
            }
            None => {
                let prefix = format!("refs/remotes/{}/", params.remote);
                let refs = repo.references().map_err(internal)?;
                let mut ids = Vec::new();
                for reference in refs.prefixed(prefix.as_str()).map_err(invalid)? {
                    let Ok(reference) = reference else { continue };
                    // 원격의 `HEAD` 는 다른 원격 추적 ref 를 가리키는 기호 ref 다 — 그 끝도 같은 커밋이라 겹쳐도 된다.
                    if let Ok(id) = reference.into_fully_peeled_id() {
                        ids.push(id.detach());
                    }
                }
                ids
            }
        };
        let mailmap = Mailmap::of(repo, opened.trusted);
        let walk = repo
            .rev_walk([tip])
            .with_hidden(hidden)
            .sorting(gix::revision::walk::Sorting::ByCommitTime(
                gix::traverse::commit::simple::CommitTimeOrder::NewestFirst,
            ))
            .all()
            .map_err(internal)?;
        let mut commits = Vec::new();
        let mut truncated = false;
        for info in walk {
            let info = info.map_err(internal)?;
            if commits.len() == params.limit as usize {
                truncated = true;
                break;
            }
            let commit = repo.find_commit(info.id).map_err(internal)?;
            let decoded = commit.decode().map_err(internal)?;
            let text = Text::read(&decoded, &mailmap)?;
            commits.push(OutgoingCommit {
                hash: info.id.to_string(),
                subject: subject(&decoded),
                author: text.author_name,
                author_time: text.author_time,
            });
        }
        let work_tree = crate::repo::display(&opened.target.work_tree);
        Ok(GitPushCommits {
            work_tree: Some(work_tree),
            commits,
            truncated,
        })
    }
}

pub fn register(router: &mut Router, git: Arc<Git>) {
    router.unary(
        GitPushCommitsMethod,
        move |_ctx, params: GitPushCommitsParams| {
            let git = Arc::clone(&git);
            async move {
                tokio::task::spawn_blocking(move || git.push_commits(&params))
                    .await
                    .map_err(|e| RpcError::internal(e.to_string()))?
            }
        },
    );
}

#[cfg(all(test, unix))]
mod tests;
