//! reset hard 가 잃을 변경(결정 M2-D6 ① — 확인 다이얼로그는 M2-20). 읽기라 gix 로만 한다(`git.status` 와 같은 계산,
//! [`crate::status::compute`]) — 미신뢰 프로젝트면 어느 설정의 필터 드라이버도 떼고 연다(결정 M2-D8).
//!
//! - `changes`: 커밋하지 않은 추적 파일의 변경(스테이징한 것 · 작업 트리 · 충돌). reset hard 가 index 와 작업 트리를 대상 커밋으로
//!   맞추므로 모두 사라진다.
//! - `overwritten`: untracked 파일 중 대상 커밋의 트리에 같은 경로가 있는 것 — git 은 묻지 않고 덮어쓴다. 나머지 untracked 는 남는다.
//!   `changes` 에 이미 있는 경로(`git rm --cached` 한 파일)는 싣지 않는다.
//!
//! ignore 된 파일이 대상 트리의 경로와 겹쳐 덮어써지는 것은 세지 않는다(`git.status` 가 ignore 된 파일을 보지 않는다).

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use pw_rpc::RpcError;
use serde::{Deserialize, Serialize};

use crate::Opened;
use crate::status::compute::{self, Stopped};
use crate::status::{GitChange, GitStatusEntry};

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitLostChanges {
    /// 사라지는 변경(`git.status` 의 항목 그대로 — untracked 는 빠진다). 경로순.
    pub changes: Vec<GitStatusEntry>,
    /// 덮어써지는 untracked 파일(작업 트리 기준 상대 경로, `/`). 경로순.
    pub overwritten: Vec<String>,
}

impl GitLostChanges {
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty() && self.overwritten.is_empty()
    }
}

/// `rev` 로 reset hard 하면 잃을 변경과 `rev` 의 커밋(전체 해시). `interrupt` 가 서면 취소 오류다.
pub fn lost(
    opened: &Opened,
    rev: &str,
    interrupt: Option<&Arc<AtomicBool>>,
) -> Result<(String, GitLostChanges), RpcError> {
    let mut repo = opened.repo.clone();
    if !opened.trusted {
        crate::diff::changes::without_filter_drivers(&mut repo)?;
    }
    let target = repo
        .rev_parse_single(rev)
        .map_err(|e| RpcError::invalid_params(format!("cannot resolve `{rev}`: {e}")))?
        .object()
        .map_err(|e| RpcError::internal(e.to_string()))?
        .peel_to_commit()
        .map_err(|e| RpcError::invalid_params(format!("`{rev}` is not a commit: {e}")))?;
    let tree = target
        .tree()
        .map_err(|e| RpcError::internal(e.to_string()))?;
    let interrupt = interrupt.map_or_else(|| Arc::new(AtomicBool::new(false)), Arc::clone);
    let entries = compute::status(&repo, None, &interrupt, None).map_err(|e| match e {
        Stopped::Interrupted => RpcError::cancelled(),
        Stopped::Failed(e) => RpcError::internal(format!("cannot read the status: {e}")),
    })?;
    let mut lost = GitLostChanges::default();
    for entry in entries {
        let untracked = entry.unstaged == Some(GitChange::Untracked);
        // `git rm --cached` 한 파일은 스테이징한 삭제이면서 untracked 다 — 경로마다 한 번만, 변경 쪽에 싣는다.
        if entry.staged.is_some()
            || entry.conflict.is_some()
            || (entry.unstaged.is_some() && !untracked)
        {
            lost.changes.push(entry);
        } else if untracked
            // 중첩 저장소(`dir/`)는 폴더째 하나다 — 트리의 파일과 겹치지 않는다.
            && !entry.path.ends_with('/')
            && tree
                .lookup_entry_by_path(&entry.path)
                .map_err(|e| RpcError::internal(e.to_string()))?
                .is_some()
        {
            lost.overwritten.push(entry.path);
        }
    }
    Ok((target.id.to_string(), lost))
}
