//! git 디렉터리 안의 경로 하나가 무엇을 바꿨는지 가른다. 내용은 읽지 않는다(HEAD 가 가리키는 브랜치만 [`head_target`] 이 읽는다).
//!
//! 작업 트리마다 따로인 것(`HEAD` · `index` · 진행 중 작업의 표지)은 그 작업 트리의 git 디렉터리에, 함께 쓰는 것(`refs/` ·
//! `packed-refs` · `config` · `info/`)은 공유 git 디렉터리에 있다. 연결 워크트리의 git 디렉터리는 공유 git 디렉터리의
//! `worktrees/<이름>` 이고, 그 밖의 `worktrees/` 아래(다른 워크트리의 것)와 메인 체크아웃의 `HEAD` · `index` 는 이 작업 트리와
//! 상관없다.

use std::path::{Path, PathBuf};

use crate::status::paths;

/// 감시하는 저장소의 두 git 디렉터리(같은 정규화를 거친 경로).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dirs {
    pub git_dir: PathBuf,
    pub common_dir: PathBuf,
}

/// 경로 하나의 결과.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// 상관없다(객체 · reflog · 훅 · 잠금 파일 · 다른 워크트리).
    None,
    /// 작업 트리 상태 전체가 바뀌었을 수 있다(`index` · `info/exclude`).
    Status,
    /// ref · HEAD · 진행 중 작업이 바뀌었다.
    Refs,
    /// 둘 다(`HEAD` · 설정).
    Both,
    /// ref 하나(`refs/heads/main` 처럼 공유 git 디렉터리 기준 이름). HEAD 가 그 브랜치면 상태도 바뀐다.
    Ref(String),
    /// `packed-refs` — 어느 ref 든 바뀌었을 수 있다.
    PackedRefs,
}

/// 진행 중 작업의 표지(git 의 `wt_status_get_state` 가 보는 것). 나타나고 사라지는 것이 `git.repo` 의 `operation` 을 바꾼다.
const OPERATION: &[&str] = &[
    "MERGE_HEAD",
    "CHERRY_PICK_HEAD",
    "REVERT_HEAD",
    "REBASE_HEAD",
    "BISECT_LOG",
    "BISECT_START",
    "rebase-merge",
    "rebase-apply",
    "sequencer",
];

pub fn classify(dirs: &Dirs, path: &Path) -> Effect {
    let in_git_dir = path.strip_prefix(&dirs.git_dir).ok().and_then(paths::join);
    let in_common = path
        .strip_prefix(&dirs.common_dir)
        .ok()
        .and_then(paths::join);
    if in_git_dir
        .as_deref()
        .or(in_common.as_deref())
        .is_some_and(|p| p.ends_with(".lock"))
    {
        return Effect::None;
    }
    let linked = dirs.git_dir != dirs.common_dir;
    if let Some(rel) = &in_git_dir {
        let own = per_worktree(rel, linked);
        if linked || own != Effect::None {
            return own;
        }
    }
    match in_common {
        Some(rel) if !rel.starts_with("worktrees/") && rel != "worktrees" => shared(&rel),
        _ => Effect::None,
    }
}

/// 작업 트리의 git 디렉터리 기준 경로. `linked` 가 아니면(메인 체크아웃) `refs/` 는 공유 쪽([`shared`])이 가른다.
fn per_worktree(rel: &str, linked: bool) -> Effect {
    let first = rel.split('/').next().unwrap_or(rel);
    match rel {
        "index" | "info/exclude" | "info/attributes" => Effect::Status,
        "HEAD" | "config.worktree" => Effect::Both,
        _ if OPERATION.contains(&first) => Effect::Refs,
        // 워크트리마다 따로인 ref(`refs/bisect` · `refs/worktree` · `refs/rewritten`).
        _ if linked && first == "refs" => Effect::Refs,
        _ => Effect::None,
    }
}

/// 공유 git 디렉터리 기준 경로.
fn shared(rel: &str) -> Effect {
    match rel {
        "config" => Effect::Both,
        "packed-refs" => Effect::PackedRefs,
        "info/exclude" | "info/attributes" => Effect::Status,
        "refs" => Effect::PackedRefs,
        _ if rel.starts_with("refs/") => Effect::Ref(rel.to_owned()),
        _ if rel.starts_with("modules/") => submodule(rel),
        _ => Effect::None,
    }
}

/// 서브모듈의 git 디렉터리(`modules/<이름>/…`, 이름에 `/` 가 들 수 있다) 안. 서브모듈 안의 `git commit` · `checkout` 은 그 HEAD 나
/// HEAD 가 가리키는 브랜치를 옮기고, 그러면 이 작업 트리의 상태(서브모듈이 가리키는 커밋, `.M sub`)가 바뀐다 — 어느 서브모듈인지
/// 가르지 않고 상태 전체로 낸다. 서브모듈 안의 index · 객체 · reflog 는 상관없다(서브모듈 안의 변경은 보지 않는다, 결정 M2-D3).
fn submodule(rel: &str) -> Effect {
    if rel.contains("/objects/") || rel.contains("/logs/") {
        return Effect::None;
    }
    if rel.ends_with("/HEAD") || rel.ends_with("/packed-refs") || rel.contains("/refs/") {
        Effect::Status
    } else {
        Effect::None
    }
}

/// `git_dir/HEAD` 가 가리키는 ref 이름(`ref: refs/heads/main` 의 뒤). 분리된 HEAD · 읽지 못하면 `None`.
pub fn head_target(git_dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(git_dir.join("HEAD")).ok()?;
    text.trim_end()
        .strip_prefix("ref:")
        .map(|r| r.trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn main_checkout() -> Dirs {
        Dirs {
            git_dir: PathBuf::from("/r/.git"),
            common_dir: PathBuf::from("/r/.git"),
        }
    }

    fn linked() -> Dirs {
        Dirs {
            git_dir: PathBuf::from("/r/.git/worktrees/wt"),
            common_dir: PathBuf::from("/r/.git"),
        }
    }

    fn of(dirs: &Dirs, rel: &str) -> Effect {
        classify(dirs, &dirs.common_dir.join(rel))
    }

    #[test]
    fn the_main_checkout_sees_its_own_files_and_the_shared_ones() {
        let d = main_checkout();
        assert_eq!(of(&d, "index"), Effect::Status);
        assert_eq!(of(&d, "HEAD"), Effect::Both);
        assert_eq!(of(&d, "MERGE_HEAD"), Effect::Refs);
        assert_eq!(of(&d, "rebase-merge/msgnum"), Effect::Refs);
        assert_eq!(
            of(&d, "refs/heads/main"),
            Effect::Ref("refs/heads/main".into())
        );
        assert_eq!(of(&d, "packed-refs"), Effect::PackedRefs);
        assert_eq!(of(&d, "config"), Effect::Both);
        assert_eq!(of(&d, "info/exclude"), Effect::Status);
        for ignored in [
            "objects/ab/cdef",
            "logs/HEAD",
            "index.lock",
            "refs/heads/main.lock",
            "ORIG_HEAD",
            "FETCH_HEAD",
            "COMMIT_EDITMSG",
            "hooks/pre-commit",
            "worktrees/wt/HEAD",
            "worktrees/wt/index",
        ] {
            assert_eq!(of(&d, ignored), Effect::None, "{ignored}");
        }
    }

    #[test]
    fn a_commit_inside_a_submodule_changes_the_status() {
        for d in [main_checkout(), linked()] {
            for moved in [
                "modules/sub/HEAD",
                "modules/sub/refs/heads/main",
                "modules/sub/packed-refs",
                "modules/libs/a/HEAD",
                "modules/outer/modules/inner/refs/heads/x",
            ] {
                assert_eq!(of(&d, moved), Effect::Status, "{moved}");
            }
            for ignored in [
                "modules/sub/index",
                "modules/sub/objects/ab/cdef",
                "modules/sub/logs/refs/heads/main",
                "modules/sub/logs/HEAD",
                "modules/sub/HEAD.lock",
                "modules/sub/refs/heads/main.lock",
                "modules/sub/config",
                "modules/sub/FETCH_HEAD",
            ] {
                assert_eq!(of(&d, ignored), Effect::None, "{ignored}");
            }
        }
    }

    #[test]
    fn a_linked_worktree_ignores_the_main_checkouts_head_and_index() {
        let d = linked();
        assert_eq!(of(&d, "worktrees/wt/index"), Effect::Status);
        assert_eq!(of(&d, "worktrees/wt/HEAD"), Effect::Both);
        assert_eq!(of(&d, "worktrees/wt/MERGE_HEAD"), Effect::Refs);
        assert_eq!(of(&d, "worktrees/wt/logs/HEAD"), Effect::None);
        assert_eq!(of(&d, "refs/heads/x"), Effect::Ref("refs/heads/x".into()));
        assert_eq!(of(&d, "packed-refs"), Effect::PackedRefs);
        assert_eq!(of(&d, "config"), Effect::Both);
        assert_eq!(of(&d, "HEAD"), Effect::None);
        assert_eq!(of(&d, "index"), Effect::None);
        assert_eq!(of(&d, "MERGE_HEAD"), Effect::None);
        assert_eq!(of(&d, "worktrees/other/HEAD"), Effect::None);
    }
}
