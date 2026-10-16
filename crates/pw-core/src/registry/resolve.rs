//! 경로 → 등록 프로젝트(F2 흐름 첫 갈래, F2-5).
//!
//! 1. 경로를 [`super::path::normalize`] 로 정규화하고, 그 경로를 포함하는 등록 루트 중 **가장 깊은 것**을 고른다.
//! 2. 경로가 연결 워크트리(`git worktree add`) 안이고, 1 에서 고른 루트가 그 워크트리 **밖**(워크트리를 감싸는 조상이거나
//!    없음)이면 경로를 메인 체크아웃 쪽으로 옮겨 다시 1 을 한다. 이때는 **메인 체크아웃 안(또는 그 꼭대기)의 루트만** 본다 —
//!    메인 체크아웃을 감싸기만 하는 폴더(`~/code` 같은 여러 저장소 묶음)는 그 저장소의 프로젝트가 아니어서 활성 워크트리를 둘 수 없다.
//!    찾으면 그 프로젝트를 고르고 워크트리를 「활성 워크트리」로 함께 돌려준다. 메인 쪽에서 못 찾으면 1 의 결과를 쓴다.
//! 3. 워크트리 자체(또는 그 안의 폴더)가 등록돼 있으면 1 에서 그것이 골라지고 2 는 하지 않는다 — 그 프로젝트로 그냥 연다.
//!
//! 워크트리 판정은 [`crate::config::gitdir`] 가 한다(`git` 프로세스를 띄우지 않는다).

use std::path::{Path, PathBuf};

use crate::config::gitdir::linked_worktree;

/// 비교할 등록 루트 하나.
pub struct RootEntry<'a, P> {
    pub project: &'a P,
    pub root: &'a Path,
}

pub struct Matched<'a, P> {
    pub project: &'a P,
    pub root: &'a Path,
    pub worktree: Option<Worktree>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Worktree {
    /// 워크트리 꼭대기(`.git` 파일이 있는 폴더).
    pub top: PathBuf,
    /// 프로젝트 루트에 대응하는 워크트리 안의 폴더(루트가 메인 꼭대기면 워크트리 꼭대기).
    pub root: PathBuf,
}

/// `path` 는 정규화된 경로여야 한다.
pub fn resolve<'a, P>(path: &Path, roots: &'a [RootEntry<'a, P>]) -> Option<Matched<'a, P>> {
    let direct = deepest(path, roots);
    let Some(link) = linked_worktree(path) else {
        return direct.map(plain);
    };
    if let Some(entry) = direct
        && entry.root.starts_with(&link.worktree_top)
    {
        return Some(plain(entry));
    }
    let via_main = link.map_to_main(path).and_then(|main| {
        roots
            .iter()
            .filter(|entry| entry.root.starts_with(&link.main_top) && main.starts_with(entry.root))
            .max_by_key(|entry| entry.root.components().count())
    });
    match via_main {
        Some(entry) => {
            // 빈 상대 경로를 `join` 하면 끝에 구분자가 붙는다(`/wt/`). 루트가 메인 꼭대기면 워크트리 꼭대기 그대로.
            let root = match entry.root.strip_prefix(&link.main_top) {
                Ok(rel) if !rel.as_os_str().is_empty() => link.worktree_top.join(rel),
                _ => link.worktree_top.clone(),
            };
            Some(Matched {
                project: entry.project,
                root: entry.root,
                worktree: Some(Worktree {
                    top: link.worktree_top,
                    root,
                }),
            })
        }
        None => direct.map(plain),
    }
}

fn deepest<'a, 'b, P>(path: &Path, roots: &'b [RootEntry<'a, P>]) -> Option<&'b RootEntry<'a, P>> {
    roots
        .iter()
        .filter(|entry| path.starts_with(entry.root))
        .max_by_key(|entry| entry.root.components().count())
}

fn plain<'a, P>(entry: &RootEntry<'a, P>) -> Matched<'a, P> {
    Matched {
        project: entry.project,
        root: entry.root,
        worktree: None,
    }
}
