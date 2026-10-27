//! 템플릿 변수의 값: 프로젝트 루트(활성 워크트리 기준), 파일 · 줄 · 열, 브랜치.

use std::path::PathBuf;

use pw_ext::template::Vars;
use pw_git::worktree::read_branch;

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
    /// 템플릿 값. 파일이 있고 줄 · 열이 없으면 1 이다. `{branch}` 는 git 을 실행하지 않고 읽는다([`read_branch`] — 웰컴 창의
    /// 브랜치와 같은 읽기).
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

#[cfg(test)]
mod tests {
    use super::*;

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
