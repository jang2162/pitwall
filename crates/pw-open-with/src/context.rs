//! 템플릿 변수의 값: 프로젝트 루트(활성 워크트리 기준), 파일 · 줄 · 열, 브랜치.

use std::path::{Path, PathBuf};

use pw_ext::template::Vars;

/// 명령을 띄울 문맥.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenContext {
    /// `{project_root}` 이자 작업 디렉터리. 활성 워크트리가 있으면 그 안의 프로젝트 루트(`ActiveWorktree::root`, F2-5), 없으면
    /// 등록된 주 루트. 절대 경로다.
    pub project_root: PathBuf,
    /// 절대 경로.
    pub file: Option<PathBuf>,
    pub line: Option<u32>,
    pub column: Option<u32>,
}

impl OpenContext {
    /// 템플릿 값. 파일이 있고 줄 · 열이 없으면 1 이다. `{branch}` 는 git 을 실행하지 않고 읽는다([`read_branch`]).
    pub fn vars(&self) -> Vars {
        let file = self.file.as_ref();
        Vars {
            project_root: Some(self.project_root.display().to_string()),
            file: file.map(|f| f.display().to_string()),
            line: file.map(|_| self.line.unwrap_or(1).to_string()),
            column: file.map(|_| self.column.unwrap_or(1).to_string()),
            branch: read_branch(&self.project_root),
        }
    }
}

/// `start` 를 담은 git 체크아웃의 현재 브랜치. git 을 실행하지 않는다 — 가장 가까운 `.git`(폴더, 또는 연결 워크트리 · 서브모듈의
/// `gitdir:` 파일)의 `HEAD` 가 `ref: refs/heads/<이름>` 이면 그 이름. 분리된 HEAD · 저장소 밖 · 읽기 실패는 `None`.
pub fn read_branch(start: &Path) -> Option<String> {
    let dot_git = start
        .ancestors()
        .map(|d| d.join(".git"))
        .find(|p| p.exists())?;
    let gitdir = if dot_git.is_dir() {
        dot_git
    } else {
        let text = std::fs::read_to_string(&dot_git).ok()?;
        let pointer = text.lines().next()?.strip_prefix("gitdir:")?.trim();
        let pointer = Path::new(pointer);
        if pointer.is_absolute() {
            pointer.to_path_buf()
        } else {
            dot_git.parent()?.join(pointer)
        }
    };
    let head = std::fs::read_to_string(gitdir.join("HEAD")).ok()?;
    let name = head.trim().strip_prefix("ref: refs/heads/")?;
    (!name.is_empty()).then(|| name.to_owned())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn reads_the_branch_of_a_checkout_and_a_linked_worktree() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("main");
        fs::create_dir_all(main.join(".git/worktrees/wt")).unwrap();
        fs::create_dir_all(main.join("sub/deeper")).unwrap();
        fs::write(main.join(".git/HEAD"), "ref: refs/heads/feat/a b\n").unwrap();
        assert_eq!(
            read_branch(&main.join("sub/deeper")).as_deref(),
            Some("feat/a b")
        );

        let wt = dir.path().join("wt");
        fs::create_dir_all(&wt).unwrap();
        fs::write(wt.join(".git"), "gitdir: ../main/.git/worktrees/wt\n").unwrap();
        fs::write(
            main.join(".git/worktrees/wt/HEAD"),
            "ref: refs/heads/$(x);y\n",
        )
        .unwrap();
        assert_eq!(read_branch(&wt).as_deref(), Some("$(x);y"));
    }

    #[test]
    fn a_detached_head_or_no_repository_has_no_branch() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read_branch(dir.path()), None);
        fs::create_dir_all(dir.path().join(".git")).unwrap();
        fs::write(
            dir.path().join(".git/HEAD"),
            "0123456789abcdef0123456789abcdef01234567\n",
        )
        .unwrap();
        assert_eq!(read_branch(dir.path()), None);
    }

    #[test]
    fn file_positions_default_to_one() {
        let ctx = OpenContext {
            project_root: PathBuf::from("/p"),
            file: Some(PathBuf::from("/p/a.rs")),
            line: Some(3),
            column: None,
        };
        let vars = ctx.vars();
        assert_eq!(vars.line.as_deref(), Some("3"));
        assert_eq!(vars.column.as_deref(), Some("1"));
        let no_file = OpenContext { file: None, ..ctx };
        assert_eq!(no_file.vars().line, None);
    }
}
