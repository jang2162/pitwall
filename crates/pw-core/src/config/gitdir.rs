//! 경로가 `git worktree add` 로 만든 연결 워크트리 안에 있는지, 그렇다면 메인 체크아웃의 어디에 대응하는지.
//!
//! `git` 프로세스를 띄우지 않고 파일만 읽는다. 연결 워크트리의 꼭대기에는 디렉터리가 아니라 **파일** `.git` 이 있고
//! (`gitdir: <메인>/.git/worktrees/<이름>`), 그 gitdir 안의 `commondir` 이 메인 저장소의 `.git` 을 가리킨다.
//! 서브모듈도 `.git` 이 파일이지만 gitdir 에 `commondir` 이 없으므로 워크트리로 보지 않는다.
//! gitdir 의 `gitdir` 파일(git 이 워크트리마다 적는 되돌림 포인터)이 그 `.git` 파일을 가리켜야 한다 — 다른 워크트리의 관리 폴더를
//! 가리키는 `.git` 파일 하나로 아무 폴더나 워크트리(파일 RPC 의 접근 루트)가 되지 않게.
//! gitdir 은 실제 경로로 **`<commondir>/worktrees/<이름>` 바로 아래**여야 한다 — git 은 관리 폴더를 언제나 그 자리에 만든다.
//! 아니면 gitdir · `commondir` · 되돌림 포인터를 모두 제 폴더 안에 둔 위조 `.git` 하나로 아무 폴더나 남의 저장소의 워크트리가
//! 되어 그 프로젝트의 신뢰(런타임 제공자 실행, `registry::trust`)와 파일 루트를 물려받는다. 이 검사가 있으면 위조하려면 그
//! 저장소의 `.git` 안에 써야 하고, 그럴 수 있으면 이미 그 저장소를 가진 것이다.

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
        if !points_back(&gitdir, &dot_git) {
            return None;
        }
        let common = fs::read_to_string(gitdir.join("commondir")).ok()?;
        let common = fs::canonicalize(resolve_from(&gitdir, common.trim())).ok()?;
        if fs::canonicalize(&gitdir).ok()?.parent()? != common.join("worktrees") {
            return None;
        }
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

/// `<gitdir>/gitdir` 이 `dot_git` 을 가리키는가. 내용은 그 워크트리의 `.git` 파일 경로다(`worktree.useRelativePaths` 면 gitdir 기준
/// 상대 경로). 실제 경로로 비교한다.
fn points_back(gitdir: &Path, dot_git: &Path) -> bool {
    let Ok(text) = fs::read_to_string(gitdir.join("gitdir")) else {
        return false;
    };
    let back = resolve_from(gitdir, text.trim());
    match (fs::canonicalize(back), fs::canonicalize(dot_git)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
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
        fs::write(
            admin.join("gitdir"),
            format!("{}\n", wt.join(".git").display()),
        )
        .unwrap();
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

    /// bare 저장소(`repo.git`)에 붙은 워크트리에는 메인 체크아웃이 없다.
    #[test]
    fn worktree_of_a_bare_repository_has_no_main_checkout() {
        let tmp = tempfile::tempdir().unwrap();
        let admin = tmp.path().join("repo.git/worktrees/wt");
        fs::create_dir_all(&admin).unwrap();
        fs::write(admin.join("commondir"), "../..\n").unwrap();
        let wt = tmp.path().join("wt");
        fs::create_dir_all(&wt).unwrap();
        fs::write(wt.join(".git"), format!("gitdir: {}\n", admin.display())).unwrap();
        fs::write(
            admin.join("gitdir"),
            format!("{}\n", wt.join(".git").display()),
        )
        .unwrap();
        assert_eq!(linked_worktree(&wt), None);
    }

    /// 다른 워크트리의 관리 폴더를 가리키는 `.git` 파일은 워크트리가 아니다(되돌림 포인터가 그 파일을 가리키지 않는다).
    #[test]
    fn a_dot_git_file_must_be_pointed_back_to() {
        let tmp = tempfile::tempdir().unwrap();
        let (main, wt) = fake_repo(tmp.path());
        let impostor = tmp.path().join("impostor");
        fs::create_dir_all(&impostor).unwrap();
        fs::write(
            impostor.join(".git"),
            format!("gitdir: {}\n", main.join(".git/worktrees/wt").display()),
        )
        .unwrap();
        assert_eq!(linked_worktree(&impostor), None);
        assert!(linked_worktree(&wt).is_some());
        // 상대 경로 되돌림 포인터(`worktree.useRelativePaths`)도 받는다.
        fs::write(
            main.join(".git/worktrees/wt/gitdir"),
            "../../../../wt/.git\n",
        )
        .unwrap();
        assert!(linked_worktree(&wt).is_some());
        // 되돌림 포인터가 없으면 워크트리로 보지 않는다.
        fs::remove_file(main.join(".git/worktrees/wt/gitdir")).unwrap();
        assert_eq!(linked_worktree(&wt), None);
    }

    /// gitdir · `commondir` · 되돌림 포인터를 모두 제 폴더 안에 둔 위조 `.git` 은 워크트리가 아니다 — 관리 폴더가 메인 저장소의
    /// `.git/worktrees/` 바로 아래가 아니다. `commondir` 이 절대 경로든 상대 경로든 같다.
    #[test]
    fn a_gitdir_outside_the_common_worktrees_folder_is_not_a_worktree() {
        let tmp = tempfile::tempdir().unwrap();
        let (main, wt) = fake_repo(tmp.path());
        let evil = tmp.path().join("downloads/evil");
        fs::create_dir_all(evil.join(".admin")).unwrap();
        fs::write(evil.join(".git"), "gitdir: .admin\n").unwrap();
        fs::write(evil.join(".admin/gitdir"), "../.git\n").unwrap();
        for common in [
            main.join(".git").display().to_string(),
            "../../../main/.git".to_owned(),
        ] {
            fs::write(evil.join(".admin/commondir"), format!("{common}\n")).unwrap();
            assert_eq!(linked_worktree(&evil), None, "commondir {common}");
        }
        // `worktrees/` 의 더 깊은 곳 · 다른 이름의 폴더도 git 이 만드는 자리가 아니다.
        for admin in ["worktrees/a/b", "worktrees-x/wt", "modules/wt"] {
            let dir = main.join(".git").join(admin);
            fs::create_dir_all(&dir).unwrap();
            let back = "../".repeat(admin.matches('/').count() + 3);
            fs::write(
                dir.join("commondir"),
                format!("{}..\n", "../".repeat(admin.matches('/').count())),
            )
            .unwrap();
            fs::write(dir.join("gitdir"), format!("{back}downloads/evil/.git\n")).unwrap();
            fs::write(evil.join(".git"), format!("gitdir: {}\n", dir.display())).unwrap();
            assert_eq!(linked_worktree(&evil), None, "{admin}");
        }
        assert!(linked_worktree(&wt).is_some());
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
