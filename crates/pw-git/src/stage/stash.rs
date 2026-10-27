//! stash(F5-8). 목록은 gix 로 `refs/stash` 의 reflog 를 읽고(미신뢰 프로젝트도 프로세스를 띄우지 않는다), 만들기 · 적용 · pop ·
//! 버리기는 실행기로 git CLI 를 돈다.
//!
//! 적용 · pop · 버리기는 `{ index, commit }` 으로 항목을 고른다 — 잠금 안에서 `stash@{index}` 가 그 커밋인지 보고 아니면(그 사이
//! 목록이 바뀜) 아무것도 하지 않는다(conflict). Pitwall 의 쓰기끼리는 잠금으로 한 줄이지만 `refs/stash` 는 모든 워크트리 · 터미널이
//! 함께 쓴다 — 확인과 실행 사이에 다른 프로세스가 stash 를 만들 수 있다. 그래서 적용(pop 도)은 커밋 해시로 하고(`git stash apply
//! <해시>`), 버리기는 `stash@{n}` 으로만 부를 수 있으므로 git 이 지웠다고 알린 커밋([`dropped`])을 보고 다르면 되돌려 넣는다.

use std::collections::BTreeMap;

use gix::ObjectId;
use gix::bstr::{BString, ByteSlice};
use gix::index::entry::Mode as IndexMode;
use pw_rpc::{RpcError, code as rpc_code};

use super::methods::StashEntry;

/// 최근 것부터의 stash 목록. `refs/stash` 가 없으면 빈 목록.
pub fn list(repo: &gix::Repository) -> Result<Vec<StashEntry>, RpcError> {
    let Some(reference) = repo.try_find_reference("refs/stash").map_err(internal)? else {
        return Ok(Vec::new());
    };
    let mut log = reference.log_iter();
    let Some(lines) = log.all().map_err(internal)? else {
        return Ok(Vec::new());
    };
    let mut entries = Vec::new();
    for line in lines {
        let line = line.map_err(internal)?;
        let time = line.signature.trim().time().map_err(internal)?;
        entries.push((
            line.new_oid().to_string(),
            line.message.to_str_lossy().into_owned(),
            time.seconds,
        ));
    }
    Ok(entries
        .into_iter()
        .rev()
        .enumerate()
        .map(|(index, (commit, message, time))| StashEntry {
            index: index as u32,
            commit,
            message,
            time,
        })
        .collect())
}

/// `stash@{index}` 가 `commit` 이 아니면 conflict.
pub fn check(entries: &[StashEntry], index: u32, commit: &str) -> Result<(), RpcError> {
    match entries.get(index as usize) {
        Some(entry) if entry.commit == commit => Ok(()),
        _ => Err(RpcError::new(
            rpc_code::CONFLICT,
            format!("stash@{{{index}}} is no longer {commit}; read git.stash.list again"),
        )),
    }
}

/// stash 를 적용하면 바뀔 작업 트리 경로: 만들 때의 기준(첫 부모)과 작업 트리 상태(stash 커밋의 트리)가 다른 경로 + untracked
/// 커밋(셋째 부모)의 파일. stash 커밋이 아니면(부모가 둘 미만) invalid params.
pub fn paths(repo: &gix::Repository, commit: &str) -> Result<Vec<String>, RpcError> {
    let id = ObjectId::from_hex(commit.as_bytes())
        .map_err(|e| RpcError::invalid_params(format!("`{commit}` is not a commit hash: {e}")))?;
    let stash = repo
        .find_commit(id)
        .map_err(|e| RpcError::invalid_params(format!("cannot find the stash `{commit}`: {e}")))?;
    let parents: Vec<ObjectId> = stash.parent_ids().map(|p| p.detach()).collect();
    if parents.len() < 2 {
        return Err(RpcError::invalid_params(format!(
            "`{commit}` is not a stash entry"
        )));
    }
    let tree_of = |id: ObjectId| -> Result<BTreeMap<BString, (ObjectId, IndexMode)>, RpcError> {
        let tree = repo
            .find_commit(id)
            .map_err(internal)?
            .tree_id()
            .map_err(internal)?;
        let index = repo.index_from_tree(&tree).map_err(internal)?;
        Ok(index
            .entries()
            .iter()
            .map(|e| (e.path(&index).to_owned(), (e.id, e.mode)))
            .collect())
    };
    let base = tree_of(parents[0])?;
    let worktree = tree_of(stash.id)?;
    let mut out: Vec<String> = worktree
        .iter()
        .filter(|(path, entry)| base.get(*path) != Some(entry))
        .map(|(path, _)| path.to_str_lossy().into_owned())
        .collect();
    out.extend(
        base.keys()
            .filter(|path| !worktree.contains_key(*path))
            .map(|path| path.to_str_lossy().into_owned()),
    );
    if let Some(&untracked) = parents.get(2) {
        out.extend(
            tree_of(untracked)?
                .keys()
                .map(|path| path.to_str_lossy().into_owned()),
        );
    }
    out.sort();
    out.dedup();
    Ok(out)
}

/// `git stash drop` 의 stdout(`Dropped refs/stash@{0} (<해시>)` — 앞말은 번역되므로 보지 않는다)에서 지운 커밋 해시.
pub fn dropped(stdout: &[u8]) -> Option<String> {
    let text = stdout.to_str_lossy();
    let line = text
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())?
        .trim_end();
    let inner = line.strip_suffix(')')?;
    let hash = &inner[inner.rfind('(')? + 1..];
    let ok = matches!(hash.len(), 40 | 64) && hash.bytes().all(|b| b.is_ascii_hexdigit());
    ok.then(|| hash.to_ascii_lowercase())
}

/// stash 커밋의 메시지 첫 줄 — git 이 stash 를 만들 때 reflog 에도 같은 것을 적는다(되돌려 넣을 때 `git stash store -m`).
pub fn message(repo: &gix::Repository, commit: &str) -> Result<String, RpcError> {
    let id = ObjectId::from_hex(commit.as_bytes()).map_err(internal)?;
    let commit = repo.find_commit(id).map_err(internal)?;
    let message = commit.message_raw_sloppy();
    Ok(message
        .lines()
        .next()
        .unwrap_or_default()
        .to_str_lossy()
        .into_owned())
}

/// index 에 충돌(단계 1~3)이 있는가.
pub fn conflicted(repo: &gix::Repository) -> Result<bool, RpcError> {
    let index = repo.index_or_empty().map_err(internal)?;
    Ok(index.entries().iter().any(|e| e.stage_raw() != 0))
}

fn internal(e: impl std::fmt::Display) -> RpcError {
    RpcError::internal(e.to_string())
}
