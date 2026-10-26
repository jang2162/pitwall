//! diff · 커밋 상세 데이터(`docs/design/13-m2-plan.md` M2-04 — 결정 M2-D5 · M2-D9).
//!
//! - [`changes`]: `git.diff` — 커밋 ↔ 커밋(커밋 ↔ 첫 부모) · 커밋 ↔ index · index ↔ 작업 트리의 바뀐 파일 목록과 hunk.
//! - [`hunks`]: 줄 diff 를 `git diff -U3` 과 같은 hunk 로.
//! - [`show`]: `git.show` · `git.containedIn`.
//! - [`blob`]: `git.blob` — 리비전 · index 의 파일 내용(전체 · 범위).
//! - [`methods`]: RPC 선언과 등록.
//!
//! 모두 gix 로 읽는다 — 미신뢰 프로젝트에서도 같은 길이다(git CLI 를 띄우지 않고, 저장소는 `crate::repo::open_options` 로 열려
//! 저장소 설정의 프로그램을 고르지 않는다). 요청마다 저장소를 새로 열고 핸들을 들고 있지 않는다.

pub mod blob;
pub mod changes;
pub mod hunks;
pub mod methods;
pub mod show;

pub use methods::register;

use gix::bstr::ByteSlice;
use pw_rpc::RpcError;

use crate::{Git, Opened};
use methods::{
    GitBlob, GitBlobParams, GitContainedIn, GitDiff, GitDiffParams, GitRevParams, GitShow,
};

impl Git {
    /// `git.show`.
    pub fn show(&self, params: &GitRevParams) -> Result<GitShow, RpcError> {
        let opened = self.opened(&params.project_id, params.worktree.as_deref())?;
        show::show(&opened.repo, &params.rev)
    }

    /// `git.containedIn`.
    pub fn contained_in(&self, params: &GitRevParams) -> Result<GitContainedIn, RpcError> {
        let opened = self.opened(&params.project_id, params.worktree.as_deref())?;
        show::contained_in(&opened.repo, &params.rev)
    }

    /// `git.diff`.
    pub fn diff(&self, params: &GitDiffParams) -> Result<GitDiff, RpcError> {
        let opened = self.opened(&params.project_id, params.worktree.as_deref())?;
        changes::diff(&opened, params, &changes::Stats::default())
    }

    /// `git.blob`.
    pub fn blob(&self, params: &GitBlobParams) -> Result<GitBlob, RpcError> {
        let opened = self.opened(&params.project_id, params.worktree.as_deref())?;
        blob::blob(&opened.repo, params)
    }

    /// 저장소가 없으면 invalid params.
    fn opened(&self, project_id: &str, worktree: Option<&str>) -> Result<Opened, RpcError> {
        self.open(project_id, worktree)?.ok_or_else(|| {
            RpcError::invalid_params(format!("project `{project_id}` has no git repository"))
        })
    }
}

/// 리비전을 커밋으로 푼다(태그는 벗긴다). 못 찾거나 커밋이 아니면 invalid params.
fn commit<'r>(repo: &'r gix::Repository, rev: &str) -> Result<gix::Commit<'r>, RpcError> {
    let id = repo
        .rev_parse_single(rev.as_bytes().as_bstr())
        .map_err(|e| RpcError::invalid_params(format!("cannot resolve `{rev}`: {e}")))?;
    id.object()
        .map_err(|e| RpcError::internal(e.to_string()))?
        .peel_to_commit()
        .map_err(|e| RpcError::invalid_params(format!("`{rev}` is not a commit: {e}")))
}
