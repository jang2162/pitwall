//! `git.show`(커밋 상세) · `git.containedIn`(그 커밋을 담은 ref).

use std::collections::HashMap;

use gix::ObjectId;
use gix::bstr::ByteSlice;
use gix::objs::FindExt;
use pw_rpc::RpcError;

use super::methods::{GitContainedIn, GitShow, RefKind, RefLabel, Signature};

pub fn show(repo: &gix::Repository, rev: &str) -> Result<GitShow, RpcError> {
    let commit = super::commit(repo, rev)?;
    let signature = |s: gix::actor::SignatureRef<'_>| -> Result<Signature, RpcError> {
        let s = s.trim();
        let time = s.time().map_err(internal)?;
        Ok(Signature {
            name: s.name.to_str_lossy().into_owned(),
            email: s.email.to_str_lossy().into_owned(),
            time: time.seconds,
            offset: time.offset,
        })
    };
    let refs = refs(repo)?
        .into_iter()
        .filter(|(_, id)| *id == commit.id)
        .map(|(label, _)| label)
        .collect();
    Ok(GitShow {
        commit: commit.id.to_string(),
        tree: commit.tree_id().map_err(internal)?.to_string(),
        parents: super::parent_ids(repo, &commit)?
            .iter()
            .map(ObjectId::to_string)
            .collect(),
        shallow: super::shallow(repo)?.contains(&commit.id),
        author: signature(commit.author().map_err(internal)?)?,
        committer: signature(commit.committer().map_err(internal)?)?,
        message: commit
            .message_raw()
            .map_err(internal)?
            .to_str_lossy()
            .into_owned(),
        head: repo.head_id().is_ok_and(|id| id == commit.id),
        refs,
    })
}

/// 모든 브랜치 · 원격 브랜치 · 태그 끝에서 이력을 거슬러 그 커밋에 닿는지 본다. 걸은 커밋마다 답을 기억하므로 ref 가 많아도 이력을
/// 한 번 걷는다. 커밋 시각으로 자르지 않는다 — 시계가 틀린 커밋이 있어도 답이 맞다. 얕은 클론의 경계 커밋에서 멈춘다(git 처럼
/// 부모가 없다).
pub fn contained_in(repo: &gix::Repository, rev: &str) -> Result<GitContainedIn, RpcError> {
    let target = super::commit(repo, rev)?.id;
    let shallow = super::shallow(repo)?;
    let mut memo: HashMap<ObjectId, bool> = HashMap::new();
    let mut parents: HashMap<ObjectId, Vec<ObjectId>> = HashMap::new();
    let mut buf = Vec::new();
    let mut out = Vec::new();
    for (label, tip) in refs(repo)? {
        let mut stack = vec![(tip, false)];
        while let Some((id, expanded)) = stack.pop() {
            if memo.contains_key(&id) {
                continue;
            }
            if id == target {
                memo.insert(id, true);
                continue;
            }
            if expanded {
                let of = parents.remove(&id).unwrap_or_default();
                let found = of.iter().any(|p| memo.get(p).copied().unwrap_or(false));
                memo.insert(id, found);
                continue;
            }
            let commit = repo
                .objects
                .find_commit_iter(&id, &mut buf)
                .map_err(internal)?;
            let of: Vec<ObjectId> = if shallow.contains(&id) {
                Vec::new()
            } else {
                commit.parent_ids().collect()
            };
            stack.push((id, true));
            stack.extend(
                of.iter()
                    .filter(|p| !memo.contains_key(*p))
                    .map(|p| (*p, false)),
            );
            parents.insert(id, of);
        }
        if memo.get(&tip).copied().unwrap_or(false) {
            out.push(label);
        }
    }
    Ok(GitContainedIn { refs: out })
}

/// 브랜치 · 원격 브랜치 · 태그와 그 끝 커밋(주석 태그는 벗긴 것). 커밋이 아닌 것을 가리키는 태그 · 심볼릭 원격 HEAD
/// (`origin/HEAD`)는 뺀다. 종류 순, 같은 종류는 이름순.
fn refs(repo: &gix::Repository) -> Result<Vec<(RefLabel, ObjectId)>, RpcError> {
    let platform = repo.references().map_err(internal)?;
    let mut out = Vec::new();
    for (kind, iter) in [
        (RefKind::LocalBranch, platform.local_branches()),
        (RefKind::RemoteBranch, platform.remote_branches()),
        (RefKind::Tag, platform.tags()),
    ] {
        for reference in iter.map_err(internal)? {
            let Ok(mut reference) = reference else {
                continue;
            };
            if reference.target().try_name().is_some() {
                continue;
            }
            let Ok(commit) = reference.peel_to_kind(gix::object::Kind::Commit) else {
                continue;
            };
            let name = reference.name();
            out.push((
                RefLabel {
                    name: name.shorten().to_str_lossy().into_owned(),
                    full_name: name.as_bstr().to_str_lossy().into_owned(),
                    kind,
                },
                commit.id,
            ));
        }
    }
    out.sort_by(|(a, _), (b, _)| (a.kind, &a.name).cmp(&(b.kind, &b.name)));
    Ok(out)
}

fn internal(e: impl std::fmt::Display) -> RpcError {
    RpcError::internal(e.to_string())
}
