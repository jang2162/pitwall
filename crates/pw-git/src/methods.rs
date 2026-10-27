//! git RPC. UI 전용이라 MCP 툴이 아니다(결정 M2-D7 — git 툴은 M5 가 켠다).
//!
//! 입력은 `{ projectId, worktree? }` 로 둔다(결정 M2-D3). 한 프로젝트의 여러 저장소는 나중에 선택 입력 `repo` 를 더해 하위 호환으로
//! 연다.

use std::sync::Arc;

use pw_rpc::{Router, RpcError, rpc_method};
use serde::{Deserialize, Serialize};

use crate::Git;

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitRepoParams {
    pub project_id: String,
    /// 활성 워크트리의 꼭대기(`ActiveWorktree.top`, 절대 경로). 있으면 그 워크트리 안의 프로젝트 루트를 담은 저장소를 본다.
    #[serde(default)]
    pub worktree: Option<String>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitRepo {
    /// `null` 이면 「저장소 없음」 — 저장소 밖, bare 저장소, 위조된 `.git` 파일, `core.worktree` 가 다른 폴더를 가리키는 저장소.
    /// git 기능 전체가 빈 상태다.
    pub repo: Option<RepoInfo>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct RepoInfo {
    /// 작업 트리 꼭대기(절대 경로). 프로젝트 루트가 저장소의 하위 폴더면 그 위다.
    pub work_tree: String,
    /// 이 작업 트리의 git 디렉터리. 연결 워크트리면 `<commonDir>/worktrees/<이름>` 이다.
    pub git_dir: String,
    /// 공유 git 디렉터리(refs · objects). 연결 워크트리가 아니면 `gitDir` 과 같다.
    pub common_dir: String,
    /// `git worktree add` 로 만든 연결 워크트리인가.
    pub linked_worktree: bool,
    pub head: RepoHead,
    /// 진행 중인 작업. 없으면 `null`.
    pub operation: Option<RepoOperation>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum RepoHead {
    /// 브랜치에 있다. `name` 은 짧은 이름(`main`, `feat/x`), `commit` 은 그 끝 커밋의 해시.
    Branch { name: String, commit: String },
    /// 아직 커밋이 없는 브랜치(새 저장소, `git checkout --orphan`).
    Unborn { name: String },
    /// 분리된 HEAD 가 가리키는 커밋(태그를 체크아웃해도 git 은 HEAD 에 커밋을 적는다).
    Detached { commit: String },
}

/// 진행 중인 작업(git 의 `wt_status_get_state` 와 같은 판정). 순차 cherry-pick · revert 와 대화형 rebase 도 각각 같은 값이다.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum RepoOperation {
    Merge,
    Rebase,
    CherryPick,
    Revert,
    Bisect,
    /// `git am`(메일 패치 적용).
    Am,
}

rpc_method! {
    /// 프로젝트의 git 저장소(결정 M2-D3): 작업 트리 · git 디렉터리 · HEAD · 진행 중 작업. 저장소가 없으면 `repo: null`.
    /// 없는 프로젝트, 상대 경로이거나 그 프로젝트의 것이 아닌 워크트리는 invalid params 오류.
    pub GitRepoMethod: "git.repo" {
        params: GitRepoParams,
        result: GitRepo,
        tier: Read,
        roles: [Owner],
    }
}

/// 핸들러를 붙인다. 저장소 읽기(디스크 IO)는 블로킹 스레드에서 한다.
pub fn register(router: &mut Router, git: Arc<Git>) {
    crate::diff::register(router, Arc::clone(&git));
    crate::log::methods::register(router, Arc::clone(&git));
    crate::worktree::register(router, Arc::clone(&git));
    router.unary(GitRepoMethod, move |_ctx, params: GitRepoParams| {
        let git = Arc::clone(&git);
        async move {
            tokio::task::spawn_blocking(move || git.repo(&params))
                .await
                .map_err(|e| RpcError::internal(e.to_string()))?
        }
    });
}
