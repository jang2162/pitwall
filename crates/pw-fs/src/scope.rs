//! 파일 접근 범위(`docs/design/12-m1-plan.md` 결정 M1-D8). Host RPC 는 MCP · 원격으로도 열리므로 경로 하나로 디스크 전체를 열지
//! 않게 한다.
//!
//! - 경로는 절대 경로만 받고, 프로젝트 레지스트리와 **같은 함수**([`pw_core::registry::path::normalize`])로 정규화한다. 루트도
//!   등록할 때 같은 함수를 지났으므로 구성 요소 단위 접두사로 비교한다.
//! - **읽기**(목록 · 상태 · 읽기): 정규화한 실제 경로가 루트 안이면 된다. 아니어도, 요청이 `..` 없이 글자 그대로 루트 아래를
//!   가리키면 그 자리의 심링크를 따라 읽는다 — pnpm 전역 저장소를 가리키는 `node_modules` 처럼 루트 안에 놓인 링크다. `..` 를
//!   허용하지 않아야 링크 대상의 부모로 올라가 그 밖을 읽지 못한다.
//! - **쓰기**: 정규화한 실제 경로(심링크를 모두 따라간 뒤)가 루트 안일 때만. 루트 밖을 가리키는 심링크로는 쓰지 못한다.

use std::path::{Component, Path, PathBuf};

use pw_core::registry::path::{PathRejected, normalize, simplify};

/// 요청 경로를 받지 않은 이유.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Denied {
    Relative(PathBuf),
    Outside(PathBuf),
}

impl std::fmt::Display for Denied {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Denied::Relative(p) => write!(f, "path `{}` must be an absolute path", p.display()),
            Denied::Outside(p) => write!(f, "`{}` is outside the project roots", p.display()),
        }
    }
}

/// 읽기로 받은 경로.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Readable {
    /// 정규화한 실제 경로. 결과에 싣는 경로(문서 모델의 키)이고 파일을 이 경로로 연다.
    pub real: PathBuf,
    /// 받은 루트 중 `inside` 를 담은 가장 깊은 것.
    pub root: PathBuf,
    /// 루트 아래에서 이 대상을 가리키는 경로. 실제 경로가 루트 안이면 그것, 아니면 요청 그대로(심링크를 지나는 경로).
    /// ignore 판정은 이 경로로 한다.
    pub inside: PathBuf,
}

/// 요청이 들어갈 수 있는 루트들. 모두 [`normalize`] 를 지난 경로다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Roots(Vec<PathBuf>);

impl Roots {
    /// `roots` 를 다시 정규화한다(등록된 루트는 이미 정규화돼 있어 그대로 나온다). 상대 경로는 버린다.
    pub fn new(roots: impl IntoIterator<Item = PathBuf>) -> Self {
        let mut out: Vec<PathBuf> = roots
            .into_iter()
            .filter_map(|r| normalize(&r).ok())
            .collect();
        out.dedup();
        Self(out)
    }

    pub fn as_slice(&self) -> &[PathBuf] {
        &self.0
    }

    /// `path` 를 담은 가장 깊은 루트. 메인 체크아웃 안의 워크트리(`.worktrees/x`)처럼 루트가 겹치면 안쪽 루트가 그 경로의
    /// ignore 기준이다.
    fn deepest(&self, path: &Path) -> Option<&PathBuf> {
        self.0
            .iter()
            .filter(|root| path.starts_with(root))
            .max_by_key(|root| root.components().count())
    }

    pub fn readable(&self, path: &Path) -> Result<Readable, Denied> {
        let real = absolute_normalized(path)?;
        if let Some(root) = self.deepest(&real) {
            return Ok(Readable {
                root: root.clone(),
                inside: real.clone(),
                real,
            });
        }
        let literal = simplify(path.to_path_buf());
        let climbs = literal.components().any(|c| c == Component::ParentDir);
        match self.deepest(&literal) {
            Some(root) if !climbs => Ok(Readable {
                root: root.clone(),
                inside: literal.clone(),
                real,
            }),
            _ => Err(Denied::Outside(path.to_path_buf())),
        }
    }

    /// 쓸 실제 경로. 심링크면 그 대상이다 — 링크를 rename 으로 덮지 않고 대상 파일을 바꾼다.
    pub fn writable(&self, path: &Path) -> Result<PathBuf, Denied> {
        let real = absolute_normalized(path)?;
        if self.deepest(&real).is_some() && !self.0.contains(&real) {
            Ok(real)
        } else {
            Err(Denied::Outside(path.to_path_buf()))
        }
    }
}

fn absolute_normalized(path: &Path) -> Result<PathBuf, Denied> {
    normalize(path).map_err(|PathRejected::Relative| Denied::Relative(path.to_path_buf()))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn real_tmp() -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let real = simplify(fs::canonicalize(tmp.path()).unwrap());
        (tmp, real)
    }

    #[test]
    fn relative_and_outside_paths_are_denied() {
        let (_tmp, base) = real_tmp();
        fs::create_dir_all(base.join("proj/src")).unwrap();
        fs::write(base.join("secret"), "s").unwrap();
        let roots = Roots::new([base.join("proj")]);

        assert_eq!(
            roots.readable(Path::new("src/main.rs")),
            Err(Denied::Relative(PathBuf::from("src/main.rs")))
        );
        assert!(matches!(
            roots.readable(&base.join("secret")),
            Err(Denied::Outside(_))
        ));
        // `..` 로 루트를 빠져나가면 글자로는 루트 아래에서 시작해도 거절한다.
        let escape = base.join("proj/src/../../secret");
        assert!(matches!(roots.readable(&escape), Err(Denied::Outside(_))));
        assert!(matches!(roots.writable(&escape), Err(Denied::Outside(_))));
        // 이름이 루트로 시작할 뿐인 형제 폴더(`proj-other`)는 루트 안이 아니다(구성 요소 단위 비교).
        fs::create_dir_all(base.join("proj-other")).unwrap();
        assert!(matches!(
            roots.readable(&base.join("proj-other/a")),
            Err(Denied::Outside(_))
        ));
        // 루트 안에서 오르내리는 `..` 는 정규화돼 받는다.
        let inside = roots.readable(&base.join("proj/src/../src/a.rs")).unwrap();
        assert_eq!(inside.real, base.join("proj/src/a.rs"));
        assert_eq!(inside.root, base.join("proj"));
        // 루트 자체는 파일로 쓸 수 없다.
        assert!(matches!(
            roots.writable(&base.join("proj")),
            Err(Denied::Outside(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_inside_a_root_are_read_through_but_not_written_outside() {
        let (_tmp, base) = real_tmp();
        fs::create_dir_all(base.join("proj/node_modules")).unwrap();
        fs::create_dir_all(base.join("store/pkg")).unwrap();
        fs::write(base.join("store/pkg/index.js"), "x").unwrap();
        fs::write(base.join("store/sibling.js"), "y").unwrap();
        std::os::unix::fs::symlink(base.join("store/pkg"), base.join("proj/node_modules/pkg"))
            .unwrap();
        let roots = Roots::new([base.join("proj")]);

        let via_link = base.join("proj/node_modules/pkg/index.js");
        let read = roots.readable(&via_link).unwrap();
        assert_eq!(read.real, base.join("store/pkg/index.js"));
        assert_eq!(read.inside, via_link);
        // 링크 대상의 부모로 오르는 `..` 는 받지 않는다.
        assert!(matches!(
            roots.readable(&base.join("proj/node_modules/pkg/../sibling.js")),
            Err(Denied::Outside(_))
        ));
        // 루트 밖을 가리키는 링크로는 쓰지 못한다.
        assert!(matches!(roots.writable(&via_link), Err(Denied::Outside(_))));

        // 루트 안을 가리키는 링크로 쓰면 대상 파일이 쓰기 경로다.
        fs::write(base.join("proj/real.txt"), "r").unwrap();
        std::os::unix::fs::symlink(base.join("proj/real.txt"), base.join("proj/link.txt")).unwrap();
        assert_eq!(
            roots.writable(&base.join("proj/link.txt")).unwrap(),
            base.join("proj/real.txt")
        );
    }

    #[test]
    fn nested_roots_pick_the_deepest() {
        let (_tmp, base) = real_tmp();
        fs::create_dir_all(base.join("main/.worktrees/wt/src")).unwrap();
        let roots = Roots::new([base.join("main"), base.join("main/.worktrees/wt")]);
        let read = roots
            .readable(&base.join("main/.worktrees/wt/src"))
            .unwrap();
        assert_eq!(read.root, base.join("main/.worktrees/wt"));
    }
}
