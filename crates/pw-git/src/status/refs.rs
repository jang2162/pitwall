//! ref 목록 · upstream · ahead/behind 를 gix 로 읽는다 — `git for-each-ref refs/heads refs/remotes refs/tags` 와
//! `git rev-list --left-right --count <브랜치>...<upstream>` 과 같은 내용.
//!
//! upstream 은 git 처럼 `branch.<이름>.remote` · `branch.<이름>.merge` 와 그 원격의 `remote.<원격>.fetch` 로 정한다. 미신뢰
//! 프로젝트는 저장소 설정을 프로그램 · 원격을 고르는 데 쓰지 않고 열지만(`crate::repo::open_options`), 이 값들은 이름일 뿐 아무것도
//! 띄우지 않으므로 신뢰와 상관없이 저장소 설정에서 그대로 읽는다(gix 의 원격 고르기는 쓰지 않는다).

use gix::bstr::{BStr, ByteSlice};
use gix::refs::FullNameRef;

use super::{GitBranch, GitRefs, GitRemoteBranch, GitTag, GitTrack, GitUpstream};
use crate::repo;

/// 저장소의 ref 들. 심볼릭 ref(`refs/remotes/origin/HEAD` 등)는 넣지 않는다.
pub fn refs(repo: &gix::Repository) -> Result<GitRefs, String> {
    let head = repo::info(repo)?.head;
    let current = repo
        .head_name()
        .map_err(|e| e.to_string())?
        .map(|name| name.as_bstr().to_owned());
    let remotes = remote_names(repo);
    let platform = repo.references().map_err(|e| e.to_string())?;
    let mut branches = Vec::new();
    let mut remote_branches = Vec::new();
    let mut tags = Vec::new();
    for reference in platform.all().map_err(|e| e.to_string())? {
        let mut reference = reference.map_err(|e| e.to_string())?;
        let Some(target) = reference.target().try_id().map(ToOwned::to_owned) else {
            continue;
        };
        let full = reference.name().as_bstr().to_owned();
        if let Some(short) = full.strip_prefix(b"refs/heads/") {
            let upstream = upstream(repo, reference.name(), target)?;
            branches.push(GitBranch {
                name: lossy(short.as_bstr()),
                ref_name: lossy(full.as_bstr()),
                commit: target.to_string(),
                head: current.as_ref() == Some(&full),
                upstream,
            });
        } else if let Some(short) = full.strip_prefix(b"refs/remotes/") {
            let name = lossy(short.as_bstr());
            let remote = remote_of(&name, &remotes);
            remote_branches.push(GitRemoteBranch {
                remote,
                name,
                ref_name: lossy(full.as_bstr()),
                commit: target.to_string(),
            });
        } else if let Some(short) = full.strip_prefix(b"refs/tags/") {
            let peeled = reference.peel_to_id().map_err(|e| e.to_string())?.detach();
            tags.push(GitTag {
                name: lossy(short.as_bstr()),
                ref_name: lossy(full.as_bstr()),
                target: target.to_string(),
                peeled: (peeled != target).then(|| peeled.to_string()),
            });
        }
    }
    let by_ref = |a: &String, b: &String| a.as_bytes().cmp(b.as_bytes());
    branches.sort_by(|a, b| by_ref(&a.ref_name, &b.ref_name));
    remote_branches.sort_by(|a, b| by_ref(&a.ref_name, &b.ref_name));
    tags.sort_by(|a, b| by_ref(&a.ref_name, &b.ref_name));
    Ok(GitRefs {
        work_tree: None,
        head: Some(head),
        branches,
        remote_branches,
        tags,
    })
}

fn lossy(name: &BStr) -> String {
    name.to_str_lossy().into_owned()
}

/// 설정에 있는 원격 이름. 이름에 `/` 가 들 수 있어 원격 브랜치의 원격은 이 목록에서 가장 긴 접두로 고른다.
fn remote_names(repo: &gix::Repository) -> Vec<String> {
    let config = repo.config_snapshot();
    let mut names: Vec<String> = config
        .plumbing()
        .sections_by_name("remote")
        .into_iter()
        .flatten()
        .filter_map(|section| section.header().subsection_name().map(lossy))
        .collect();
    names.sort();
    names.dedup();
    names
}

/// `origin/feat/x` 의 원격. 설정에 맞는 원격이 없으면 첫 `/` 앞.
fn remote_of(name: &str, remotes: &[String]) -> String {
    remotes
        .iter()
        .filter(|r| {
            name.strip_prefix(r.as_str())
                .is_some_and(|rest| rest.starts_with('/'))
        })
        .max_by_key(|r| r.len())
        .cloned()
        .unwrap_or_else(|| name.split('/').next().unwrap_or(name).to_owned())
}

/// 로컬 브랜치 `branch`(끝 커밋 `tip`)의 upstream. 설정이 없으면 `None`.
fn upstream(
    repo: &gix::Repository,
    branch: &FullNameRef,
    tip: gix::ObjectId,
) -> Result<Option<GitUpstream>, String> {
    let Some(tracking) = tracking_ref(repo, branch) else {
        return Ok(None);
    };
    let track = match repo
        .try_find_reference(tracking.as_bstr())
        .map_err(|e| e.to_string())?
    {
        Some(mut reference) => {
            let theirs = reference.peel_to_id().map_err(|e| e.to_string())?.detach();
            Some(ahead_behind(repo, tip, theirs)?)
        }
        None => None,
    };
    let short = tracking
        .strip_prefix(b"refs/remotes/")
        .or_else(|| tracking.strip_prefix(b"refs/heads/"))
        .unwrap_or(tracking.as_bytes());
    Ok(Some(GitUpstream {
        name: lossy(short.as_bstr()),
        ref_name: lossy(tracking.as_bstr()),
        track,
    }))
}

/// git 의 `branch_get_upstream`: `branch.<b>.merge` 를 `branch.<b>.remote` 의 fetch refspec 으로 옮긴 원격 추적 ref. 원격이 `.` 이면
/// 그 로컬 ref 그대로다.
fn tracking_ref(repo: &gix::Repository, branch: &FullNameRef) -> Option<gix::bstr::BString> {
    let config = repo.config_snapshot();
    let plumbing = config.plumbing();
    let short = branch.shorten();
    let remote = plumbing.string_by("branch", Some(short), "remote")?;
    let merge = plumbing.string_by("branch", Some(short), "merge")?;
    let merge: gix::bstr::BString = if merge.starts_with(b"refs/") {
        merge.to_vec().into()
    } else {
        format!("refs/heads/{}", merge.to_str_lossy()).into()
    };
    if remote.as_bytes() == b"." {
        return Some(merge);
    }
    let specs: Vec<gix::refspec::RefSpec> = plumbing
        .strings_by("remote", Some(remote.as_ref()), "fetch")?
        .iter()
        .filter_map(|spec| {
            gix::refspec::parse(spec.as_ref(), gix::refspec::parse::Operation::Fetch)
                .ok()
                .map(|s| s.to_owned())
        })
        .collect();
    let group = gix::refspec::MatchGroup {
        specs: specs
            .iter()
            .map(gix::refspec::RefSpec::to_ref)
            .filter(|s| s.source().is_some() && s.destination().is_some())
            .collect(),
    };
    let null = repo.object_hash().null();
    let out = group.match_lhs(std::iter::once(gix::refspec::match_group::Item {
        full_ref_name: merge.as_ref(),
        target: &null,
        object: None,
    }));
    out.mappings
        .into_iter()
        .next()
        .and_then(|m| m.rhs.map(std::borrow::Cow::into_owned))
}

/// `ours` 에만 있는 커밋 수와 `theirs` 에만 있는 커밋 수(`git rev-list --left-right --count ours...theirs`).
pub fn ahead_behind(
    repo: &gix::Repository,
    ours: gix::ObjectId,
    theirs: gix::ObjectId,
) -> Result<GitTrack, String> {
    if ours == theirs {
        return Ok(GitTrack {
            ahead: 0,
            behind: 0,
        });
    }
    let count = |tip: gix::ObjectId, hidden: gix::ObjectId| -> Result<u32, String> {
        let walk = repo
            .rev_walk([tip])
            .with_hidden([hidden])
            .all()
            .map_err(|e| e.to_string())?;
        let mut n = 0u32;
        for info in walk {
            info.map_err(|e| e.to_string())?;
            n = n.saturating_add(1);
        }
        Ok(n)
    };
    Ok(GitTrack {
        ahead: count(ours, theirs)?,
        behind: count(theirs, ours)?,
    })
}
