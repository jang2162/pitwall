//! ref 목록 · upstream · ahead/behind 를 gix 로 읽는다 — `git for-each-ref refs/heads refs/remotes refs/tags` 와
//! `git rev-list --left-right --count <브랜치>...<upstream>` 과 같은 내용.
//!
//! upstream 은 git 처럼 `branch.<이름>.remote` · `branch.<이름>.merge` 와 그 원격의 `remote.<원격>.fetch` 로 정한다. 미신뢰
//! 프로젝트는 저장소 설정을 프로그램 · 원격을 고르는 데 쓰지 않고 열지만(`crate::repo::open_options`), 이 값들은 이름일 뿐 아무것도
//! 띄우지 않으므로 신뢰와 상관없이 저장소 설정에서 그대로 읽는다(gix 의 원격 고르기는 쓰지 않는다).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, PoisonError};

use gix::bstr::{BStr, ByteSlice};
use gix::refs::FullNameRef;

use super::{GitBranch, GitBrokenRef, GitRefs, GitRemoteBranch, GitTag, GitTrack, GitUpstream};
use crate::repo;

/// 저장소의 ref 들. 심볼릭 ref(`refs/remotes/origin/HEAD` 등)는 넣지 않는다. 읽지 못한 ref(깨진 loose ref, 없는 객체를 가리키는
/// 태그, upstream 과 셀 수 없는 브랜치)는 git 처럼 건너뛰고 `broken` 에 적는다 — 나머지는 그대로 보인다.
/// `generations` 가 있으면 센 세대 번호를 거기서 받고 돌려준다([`Generations`]).
pub fn refs(repo: &gix::Repository, generations: Option<&Generations>) -> Result<GitRefs, String> {
    let head = repo::info(repo)?.head;
    let current = repo
        .head_name()
        .map_err(|e| e.to_string())?
        .map(|name| name.as_bstr().to_owned());
    let remotes = remote_names(repo);
    // commit-graph 를 읽지 못하면(손상) 없이 센다 — 느릴 뿐 결과는 같다.
    let cache = repo.commit_graph_if_enabled().ok().flatten();
    let taken = generations.and_then(|g| g.take(repo));
    let (key, known) = taken.unzip();
    let mut counter = Counter::with_known(repo, cache.as_ref(), known.unwrap_or_default());
    let platform = repo.references().map_err(|e| e.to_string())?;
    let mut refs = GitRefs {
        work_tree: None,
        head: Some(head),
        branches: Vec::new(),
        remote_branches: Vec::new(),
        tags: Vec::new(),
        unsupported: None,
        broken: Vec::new(),
    };
    for reference in platform.all().map_err(|e| e.to_string())? {
        let mut reference = match reference {
            Ok(reference) => reference,
            Err(e) => {
                // 내용을 읽지 못한 loose ref 는 git 디렉터리 기준 경로가 곧 이름이다.
                let ref_name = e
                    .downcast_any_ref::<gix::refs::file::find::ReferenceDecode>()
                    .map(|d| d.relative_path.to_string_lossy().replace('\\', "/"));
                refs.broken.push(GitBrokenRef {
                    ref_name,
                    error: e.to_string(),
                });
                continue;
            }
        };
        let full = reference.name().as_bstr().to_owned();
        if let Err(error) = add(
            repo,
            &mut counter,
            &mut reference,
            &current,
            &remotes,
            &mut refs,
        ) {
            tracing::debug!(target: "pw_git", reference = %full, error, "skipping an unreadable ref");
            refs.broken.push(GitBrokenRef {
                ref_name: Some(lossy(full.as_bstr())),
                error,
            });
        }
    }
    if let (Some(generations), Some(key)) = (generations, key) {
        generations.put(key, counter.known);
    }
    let by_ref = |a: &String, b: &String| a.as_bytes().cmp(b.as_bytes());
    refs.branches
        .sort_by(|a, b| by_ref(&a.ref_name, &b.ref_name));
    refs.remote_branches
        .sort_by(|a, b| by_ref(&a.ref_name, &b.ref_name));
    refs.tags.sort_by(|a, b| by_ref(&a.ref_name, &b.ref_name));
    Ok(refs)
}

/// ref 하나를 제 목록에 넣는다. `current` 는 HEAD 가 가리키는 브랜치의 전체 이름.
fn add(
    repo: &gix::Repository,
    counter: &mut Counter<'_, '_>,
    reference: &mut gix::Reference<'_>,
    current: &Option<gix::bstr::BString>,
    remotes: &[String],
    refs: &mut GitRefs,
) -> Result<(), String> {
    let Some(target) = reference.target().try_id().map(ToOwned::to_owned) else {
        return Ok(());
    };
    let full = reference.name().as_bstr().to_owned();
    if let Some(short) = full.strip_prefix(b"refs/heads/") {
        let upstream = upstream(repo, counter, reference.name(), target)?;
        refs.branches.push(GitBranch {
            name: lossy(short.as_bstr()),
            ref_name: lossy(full.as_bstr()),
            commit: target.to_string(),
            head: current.as_ref() == Some(&full),
            upstream,
        });
    } else if let Some(short) = full.strip_prefix(b"refs/remotes/") {
        let name = lossy(short.as_bstr());
        let remote = remote_of(&name, remotes);
        refs.remote_branches.push(GitRemoteBranch {
            remote,
            name,
            ref_name: lossy(full.as_bstr()),
            commit: target.to_string(),
        });
    } else if let Some(short) = full.strip_prefix(b"refs/tags/") {
        let peeled = reference.peel_to_id().map_err(|e| e.to_string())?.detach();
        refs.tags.push(GitTag {
            name: lossy(short.as_bstr()),
            ref_name: lossy(full.as_bstr()),
            target: target.to_string(),
            peeled: (peeled != target).then(|| peeled.to_string()),
        });
    }
    Ok(())
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
    counter: &mut Counter<'_, '_>,
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
            Some(counter.ahead_behind(tip, theirs)?)
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

/// 저장소(공유 git 디렉터리)마다 센 세대 번호([`Counter`]). 커밋은 바뀌지 않으므로 한 번 센 것은 다음 `git.refs` 에서도 맞다 —
/// commit-graph 가 없는 큰 저장소에서 부를 때마다 이력 전체를 다시 읽지 않는다(10만 커밋에서 수백 ms). 객체 바꿔치기
/// (`refs/replace/`)가 있는 저장소는 그것이 바뀌면 부모가 바뀌므로 담지 않는다.
#[derive(Default)]
pub struct Generations {
    repos: Mutex<HashMap<PathBuf, Known>>,
}

/// 커밋 → 세대 번호.
type Known = gix::hashtable::HashMap<gix::ObjectId, u32>;

/// 담아 두는 저장소 수. 넘으면 비운다.
const GENERATION_REPOSITORIES: usize = 16;

impl Generations {
    /// `repo` 의 것을 꺼낸다(그동안 같은 저장소의 다른 호출은 처음부터 센다). 담지 않는 저장소면 `None`.
    fn take(&self, repo: &gix::Repository) -> Option<(PathBuf, Known)> {
        let replaced = repo
            .references()
            .ok()?
            .prefixed("refs/replace/")
            .ok()?
            .next()
            .is_some();
        if replaced {
            return None;
        }
        let key = repo.common_dir().to_path_buf();
        let known = self
            .repos
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&key)
            .unwrap_or_default();
        Some((key, known))
    }

    fn put(&self, key: PathBuf, known: Known) {
        let mut repos = self.repos.lock().unwrap_or_else(PoisonError::into_inner);
        if repos.len() >= GENERATION_REPOSITORIES {
            repos.clear();
        }
        repos.insert(key, known);
    }
}

/// ahead/behind 를 세는 커밋 그래프(`git rev-list --left-right --count ours...theirs`). 한 번의 `git.refs` 동안 브랜치마다 다시
/// 쓴다 — 센 세대 번호를 나눈다.
///
/// 커밋 시각을 믿지 않는다: 시각 순으로 걸으면 조상이 자손보다 늦은 시각인 이력(틀린 시계, 고친 커밋)에서 한쪽에만 있다고 센
/// 커밋에 다른 쪽이 나중에 닿는다. 대신 세대 번호(뿌리 1, 그 밖은 부모 중 가장 큰 것 + 1 — 자손이 언제나 크다) 순으로 칠한다.
/// commit-graph 에 있는 커밋은 그 세대 번호를 쓰고, 없는 커밋(commit-graph 뒤에 만든 것, commit-graph 가 없는 저장소의 전부)은
/// 조상을 따라 직접 센다 — commit-graph 가 없으면 처음 한 번 이력 전체를 읽는다([`Generations`] 가 다음 호출에 넘긴다).
pub struct Counter<'repo, 'cache> {
    graph: gix::revwalk::Graph<'repo, 'cache, ()>,
    known: Known,
    /// 객체가 없는 커밋(얕은 클론의 경계 너머). 없는 부모는 git 처럼 없는 것으로 본다.
    missing: gix::hashtable::HashSet<gix::ObjectId>,
}

/// 칠하는 표시: `ours` · `theirs` 에서 닿는다.
const OURS: u8 = 1;
const THEIRS: u8 = 2;

impl<'repo, 'cache> Counter<'repo, 'cache> {
    /// `cache` 는 `repo.commit_graph_if_enabled()` 로 연 commit-graph(없으면 `None`).
    pub fn new(
        repo: &'repo gix::Repository,
        cache: Option<&'cache gix::commitgraph::Graph>,
    ) -> Self {
        Self::with_known(repo, cache, Known::default())
    }

    fn with_known(
        repo: &'repo gix::Repository,
        cache: Option<&'cache gix::commitgraph::Graph>,
        known: Known,
    ) -> Self {
        Self {
            graph: repo.revision_graph(cache),
            known,
            missing: Default::default(),
        }
    }

    /// `ours` 에만 있는 커밋 수와 `theirs` 에만 있는 커밋 수.
    pub fn ahead_behind(
        &mut self,
        ours: gix::ObjectId,
        theirs: gix::ObjectId,
    ) -> Result<GitTrack, String> {
        if ours == theirs {
            return Ok(GitTrack {
                ahead: 0,
                behind: 0,
            });
        }
        let mut flags: gix::hashtable::HashMap<gix::ObjectId, u8> = Default::default();
        let mut queue = std::collections::BinaryHeap::new();
        // 큐에 든 커밋 중 한쪽에서만 닿는 것의 수. 0 이 되면 남은 것은 모두 양쪽의 조상이라 더 걸을 일이 없다.
        let mut one_sided = 0usize;
        for (tip, flag) in [(ours, OURS), (theirs, THEIRS)] {
            let generation = self
                .generation(tip)?
                .ok_or_else(|| format!("the commit {tip} is missing"))?;
            flags.insert(tip, flag);
            queue.push((generation, tip));
            one_sided += 1;
        }
        // 세대가 큰 것부터 꺼낸다 — 꺼낼 때 그 커밋에 닿는 자손은 모두 이미 꺼냈으므로 표시가 끝난 것이다.
        while one_sided > 0 {
            let Some((_, id)) = queue.pop() else { break };
            let flag = flags[&id];
            if flag != OURS | THEIRS {
                one_sided -= 1;
            }
            let Some(parents) = self.parents(id)? else {
                continue;
            };
            for parent in parents {
                let Some(generation) = self.generation(parent)? else {
                    continue;
                };
                match flags.get_mut(&parent) {
                    Some(old) => {
                        if *old != OURS | THEIRS && *old | flag == OURS | THEIRS {
                            one_sided -= 1;
                        }
                        *old |= flag;
                    }
                    None => {
                        flags.insert(parent, flag);
                        queue.push((generation, parent));
                        if flag != OURS | THEIRS {
                            one_sided += 1;
                        }
                    }
                }
            }
        }
        let side = |want: u8| {
            let n = flags.values().filter(|f| **f == want).count();
            u32::try_from(n).unwrap_or(u32::MAX)
        };
        Ok(GitTrack {
            ahead: side(OURS),
            behind: side(THEIRS),
        })
    }

    /// 커밋의 부모와 commit-graph 의 세대 번호(오래된 git 이 쓴 commit-graph 는 0 을 적는다 — 모르는 것으로 본다). 객체가 없으면
    /// `None`. 읽은 커밋을 그래프에 담아 두지 않는다 — 이력 전체를 읽을 때 메모리가 이력 크기만큼 늘지 않게.
    #[allow(clippy::type_complexity)]
    fn read(
        &mut self,
        id: gix::ObjectId,
    ) -> Result<Option<(Vec<gix::ObjectId>, Option<u32>)>, String> {
        if self.missing.contains(&id) {
            return Ok(None);
        }
        let Some(commit) = self.graph.try_lookup(&id).map_err(|e| e.to_string())? else {
            self.missing.insert(id);
            return Ok(None);
        };
        let parents = commit
            .iter_parents()
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        Ok(Some((parents, commit.generation().filter(|g| *g != 0))))
    }

    fn parents(&mut self, id: gix::ObjectId) -> Result<Option<Vec<gix::ObjectId>>, String> {
        Ok(self.read(id)?.map(|(parents, _)| parents))
    }

    /// `tip` 의 세대 번호(커밋이 없으면 `None`). 모르는 조상의 세대를 깊이 우선으로 채운다(재귀하지 않는다 — 이력이 길다).
    fn generation(&mut self, tip: gix::ObjectId) -> Result<Option<u32>, String> {
        if let Some(generation) = self.known.get(&tip) {
            return Ok(Some(*generation));
        }
        // (커밋, 부모를 다 센 뒤 돌아왔으면 그 부모들).
        let mut stack: Vec<(gix::ObjectId, Option<Vec<gix::ObjectId>>)> = vec![(tip, None)];
        while let Some((id, parents)) = stack.pop() {
            if self.known.contains_key(&id) {
                continue;
            }
            if let Some(parents) = parents {
                let highest = parents
                    .iter()
                    .filter_map(|p| self.known.get(p).copied())
                    .max()
                    .unwrap_or(0);
                self.known.insert(id, highest.saturating_add(1));
                continue;
            }
            let Some((parents, generation)) = self.read(id)? else {
                continue;
            };
            if let Some(generation) = generation {
                self.known.insert(id, generation);
                continue;
            }
            let unknown: Vec<gix::ObjectId> = parents
                .iter()
                .filter(|p| !self.known.contains_key(*p))
                .copied()
                .collect();
            stack.push((id, Some(parents)));
            stack.extend(unknown.into_iter().map(|p| (p, None)));
        }
        Ok(self.known.get(&tip).copied())
    }
}
