//! 파일 접근 범위(`docs/design/12-m1-plan.md` 결정 M1-D8). Host RPC 는 MCP · 원격으로도 열리므로 경로 하나로 디스크 전체를 열지
//! 않게 한다.
//!
//! - 경로는 절대 경로만 받고, 프로젝트 레지스트리와 **같은 함수**([`pw_core::registry::path::normalize`])로 정규화한다. 루트도
//!   등록할 때 같은 함수를 지났으므로 구성 요소 단위 접두사로 비교한다.
//! - **읽기**(목록 · 상태 · 읽기): 심링크를 끝까지 푼 실제 경로가 루트 안일 때만. 루트 안 링크(pnpm 의 `node_modules` →
//!   `node_modules/.pnpm/…`)는 따라 읽고, 실제 대상이 루트 밖인 링크는 따르지 않는다 — 클론한 저장소에 든 링크 하나
//!   (`docs -> ../../..`, `sys -> /etc`)로 디스크를 열지 않게. 중간 구성 요소가 링크여도(`root/up/secret`, `up -> ..`) 같은
//!   판정이다. 목록은 그런 링크를 항목으로는 보이되 「루트 밖을 가리킴」 으로 표시하고 대상을 알리지 않는다([`crate::list`]).
//! - **쓰기**: 정규화한 실제 경로(심링크를 모두 따라간 뒤)가 루트 안일 때만. 루트 밖을 가리키는 심링크로는 쓰지 못한다. 대상이
//!   없거나 고리인 심링크에도 쓰지 않는다 — 실제 경로로 풀리지 않아 rename 이 링크 자체를 일반 파일로 바꾼다.
//! - **한계(쓰기)**: 판정은 경로 글자로 하고 쓰기는 그 뒤에 경로로 연다. 판정과 rename 사이에 다른 로컬 프로세스가 경로의 폴더
//!   하나를 루트 밖 링크로 바꾸면 임시 파일 · rename 이 루트 밖에 떨어진다. 막으려면 폴더 핸들 기준 `openat` · `renameat` 가
//!   필요하다(M1 에서는 하지 않는다). 루트 안에 동시에 쓸 수 있는 로컬 프로세스가 있어야 생기는 경합이다.

use std::fs;
use std::path::{Component, Path, PathBuf};

use pw_core::registry::path::{PathRejected, normalize, simplify};

/// 요청 경로를 받지 않은 이유.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Denied {
    Relative(PathBuf),
    Outside(PathBuf),
    /// 요청은 루트 아래를 가리키지만 심링크를 풀면 루트 밖이다.
    LinkOutside(PathBuf),
    /// 쓰려는 경로가 실제 경로로 풀리지 않는 심링크(대상이 없거나 고리)다.
    UnresolvedLink(PathBuf),
}

impl std::fmt::Display for Denied {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Denied::Relative(p) => write!(f, "path `{}` must be an absolute path", p.display()),
            Denied::Outside(p) => write!(f, "`{}` is outside the project roots", p.display()),
            Denied::LinkOutside(p) => write!(
                f,
                "`{}` goes through a symlink that points outside the project roots",
                p.display()
            ),
            Denied::UnresolvedLink(p) => write!(
                f,
                "`{}` is a symlink whose target is missing or loops",
                p.display()
            ),
        }
    }
}

/// 읽기로 받은 경로.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Readable {
    /// 정규화한 실제 경로. 결과에 싣는 경로(문서 모델의 키)이고 파일을 이 경로로 연다.
    pub real: PathBuf,
    /// 받은 루트 중 `real` 을 담은 가장 깊은 것. ignore 판정의 기준이다.
    pub root: PathBuf,
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

    /// 정규화한 실제 경로가 루트 안인가(목록이 링크 항목을 판정할 때 쓴다).
    pub fn contains(&self, real: &Path) -> bool {
        self.deepest(real).is_some()
    }

    pub fn readable(&self, path: &Path) -> Result<Readable, Denied> {
        let real = absolute_normalized(path)?;
        if let Some(root) = self.deepest(&real) {
            return Ok(Readable {
                root: root.clone(),
                real,
            });
        }
        // 거절 이유만 가른다: `..` 없이 글자로는 루트 아래면 링크가 밖으로 데려간 것이다.
        let literal = simplify(path.to_path_buf());
        let climbs = literal.components().any(|c| c == Component::ParentDir);
        if !climbs && self.deepest(&literal).is_some() {
            Err(Denied::LinkOutside(path.to_path_buf()))
        } else {
            Err(Denied::Outside(path.to_path_buf()))
        }
    }

    /// 쓸 실제 경로. 심링크면 그 대상이다 — 링크를 rename 으로 덮지 않고 대상 파일을 바꾼다.
    pub fn writable(&self, path: &Path) -> Result<PathBuf, Denied> {
        let real = absolute_normalized(path)?;
        if self.deepest(&real).is_none() || self.0.contains(&real) {
            return Err(Denied::Outside(path.to_path_buf()));
        }
        // 정규화는 풀리는 링크를 대상으로 바꾼다 — 그래도 링크로 남았으면 대상이 없거나 고리다.
        if fs::symlink_metadata(&real).is_ok_and(|m| m.file_type().is_symlink()) {
            Err(Denied::UnresolvedLink(path.to_path_buf()))
        } else {
            Ok(real)
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
    fn symlinks_are_followed_only_when_the_real_target_is_inside() {
        let (_tmp, base) = real_tmp();
        let proj = base.join("proj");
        fs::create_dir_all(proj.join("node_modules/.pnpm/pkg@1/node_modules/pkg")).unwrap();
        fs::create_dir_all(base.join("store/pkg")).unwrap();
        fs::write(base.join("secret.txt"), "s").unwrap();
        // pnpm 기본 구성: 링크가 루트 안(`.pnpm`)을 가리킨다 — 따라 읽는다.
        std::os::unix::fs::symlink(
            proj.join("node_modules/.pnpm/pkg@1/node_modules/pkg"),
            proj.join("node_modules/pkg"),
        )
        .unwrap();
        // 루트 밖을 가리키는 링크(전역 저장소) · 부모를 가리키는 링크 — 따르지 않는다.
        std::os::unix::fs::symlink(base.join("store/pkg"), proj.join("node_modules/global"))
            .unwrap();
        std::os::unix::fs::symlink("..", proj.join("up")).unwrap();
        let roots = Roots::new([proj.clone()]);

        let inside = roots
            .readable(&proj.join("node_modules/pkg/index.js"))
            .unwrap();
        assert_eq!(
            inside.real,
            proj.join("node_modules/.pnpm/pkg@1/node_modules/pkg/index.js")
        );
        for denied in [
            proj.join("node_modules/global/index.js"),
            proj.join("node_modules/global"),
            proj.join("up"),
            proj.join("up/secret.txt"),
        ] {
            assert_eq!(
                roots.readable(&denied),
                Err(Denied::LinkOutside(denied.clone()))
            );
            assert!(matches!(roots.writable(&denied), Err(Denied::Outside(_))));
        }
        // 링크 대상의 부모로 오르는 `..` 도 실제 경로로 판정한다.
        assert!(roots.readable(&proj.join("up/proj/../secret.txt")).is_err());

        // 루트 안을 가리키는 링크로 쓰면 대상 파일이 쓰기 경로다.
        fs::write(proj.join("real.txt"), "r").unwrap();
        std::os::unix::fs::symlink(proj.join("real.txt"), proj.join("link.txt")).unwrap();
        assert_eq!(
            roots.writable(&proj.join("link.txt")).unwrap(),
            proj.join("real.txt")
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
