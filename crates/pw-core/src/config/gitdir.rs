//! 경로가 `git worktree add` 로 만든 연결 워크트리 안에 있는지, 그렇다면 메인 체크아웃의 어디에 대응하는지.
//!
//! `git` 프로세스를 띄우지 않고 파일만 읽는다. 연결 워크트리의 꼭대기에는 디렉터리가 아니라 **파일** `.git` 이 있고
//! (`gitdir: <메인>/.git/worktrees/<이름>`), 그 gitdir 안의 `commondir` 이 메인 저장소의 `.git` 을 가리킨다.
//! 서브모듈도 `.git` 이 파일이지만 gitdir 에 `commondir` 이 없으므로 워크트리로 보지 않는다.

use std::fs;
use std::path::{Path, PathBuf};

/// 연결 워크트리와 메인 체크아웃의 대응.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeLink {
    /// `.git` 파일이 있는 워크트리 꼭대기.
    pub worktree_top: PathBuf,
    /// 메인 체크아웃 꼭대기(`commondir` 이 가리키는 `.git` 의 부모).
    pub main_top: PathBuf,
}

impl WorktreeLink {
    /// 워크트리 안의 `path` 에 대응하는 메인 체크아웃 쪽 경로. `path` 가 워크트리 밖이면 `None`.
    pub fn map_to_main(&self, path: &Path) -> Option<PathBuf> {
        let rel = path.strip_prefix(&self.worktree_top).ok()?;
        Some(self.main_top.join(rel))
    }
}

/// `start` 와 그 조상에서 가장 가까운 `.git` 을 찾는다. 그것이 연결 워크트리의 `.git` 파일이면 대응을 돌려준다.
/// 일반 체크아웃(`.git` 이 디렉터리), 서브모듈, bare 저장소의 워크트리, 저장소 밖이면 `None`.
pub fn linked_worktree(start: &Path) -> Option<WorktreeLink> {
    for dir in start.ancestors() {
        let dot_git = dir.join(".git");
        let Ok(meta) = fs::metadata(&dot_git) else {
            continue;
        };
        if !meta.is_file() {
            return None;
        }
        let gitdir = resolve_from(dir, read_pointer(&dot_git, "gitdir:")?);
        let common = fs::read_to_string(gitdir.join("commondir")).ok()?;
        let common = fs::canonicalize(resolve_from(&gitdir, common.trim())).ok()?;
        if common.file_name()? != ".git" {
            // bare 저장소에 붙은 워크트리 — 메인 체크아웃이 없다.
            return None;
        }
        return Some(WorktreeLink {
            worktree_top: dir.to_path_buf(),
            main_top: common.parent()?.to_path_buf(),
        });
    }
    None
}

/// `gitdir: <경로>` 형식 파일에서 경로를 읽는다.
fn read_pointer(file: &Path, prefix: &str) -> Option<String> {
    let text = fs::read_to_string(file).ok()?;
    let line = text.lines().next()?;
    Some(line.strip_prefix(prefix)?.trim().to_owned())
}

fn resolve_from(base: &Path, path: impl AsRef<Path>) -> PathBuf {
    let path = path.as_ref();
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `git worktree add` 가 만드는 것과 같은 모양을 손으로 만든다(테스트가 git 실행 파일에 의존하지 않게).
    fn fake_repo(root: &Path) -> (PathBuf, PathBuf) {
        let main = root.join("main");
        let wt = root.join("wt");
        let admin = main.join(".git/worktrees/wt");
        fs::create_dir_all(&admin).unwrap();
        fs::create_dir_all(wt.join("sub/dir")).unwrap();
        fs::write(admin.join("commondir"), "../..\n").unwrap();
        fs::write(wt.join(".git"), format!("gitdir: {}\n", admin.display())).unwrap();
        (fs::canonicalize(main).unwrap(), wt)
    }

    #[test]
    fn linked_worktree_maps_to_main_checkout() {
        let tmp = tempfile::tempdir().unwrap();
        let (main, wt) = fake_repo(tmp.path());
        let link = linked_worktree(&wt.join("sub/dir")).expect("worktree");
        assert_eq!(link.worktree_top, wt);
        assert_eq!(link.main_top, main);
        assert_eq!(link.map_to_main(&wt.join("sub")).unwrap(), main.join("sub"));
        // 메인 체크아웃 자신은 워크트리가 아니다.
        assert_eq!(linked_worktree(&main), None);
    }

    #[test]
    fn relative_gitdir_is_resolved_from_the_worktree() {
        let tmp = tempfile::tempdir().unwrap();
        let (main, wt) = fake_repo(tmp.path());
        fs::write(wt.join(".git"), "gitdir: ../main/.git/worktrees/wt\n").unwrap();
        assert_eq!(linked_worktree(&wt).unwrap().main_top, main);
    }

    #[test]
    fn submodule_and_plain_dirs_are_not_worktrees() {
        let tmp = tempfile::tempdir().unwrap();
        let sub_admin = tmp.path().join("super/.git/modules/m");
        fs::create_dir_all(&sub_admin).unwrap();
        let module = tmp.path().join("super/m");
        fs::create_dir_all(&module).unwrap();
        fs::write(
            module.join(".git"),
            format!("gitdir: {}\n", sub_admin.display()),
        )
        .unwrap();
        assert_eq!(linked_worktree(&module), None);
        assert_eq!(linked_worktree(tmp.path()), None);
    }
}
