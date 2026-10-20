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
//! - **읽기를 연 뒤 다시 판정**: 판정과 open 사이에 경로의 구성 요소가 루트 밖 링크로 바뀌면(깨진 링크의 대상이 그사이 생기는
//!   등) 다른 파일이 열린다. 파일 읽기는 연 핸들의 실제 경로([`crate::handle::path_of`])를 다시 판정하고 밖이면 버린다
//!   ([`Roots::open_inside`]). 핸들 경로를 얻지 못하는 플랫폼은 판정만 한다. 폴더 목록(`read_dir`)은 핸들 경로를 보지 않는다.
//! - **한계(쓰기)**: 판정은 경로 글자로 하고 쓰기는 그 뒤에 경로로 연다. 판정과 rename 사이에 다른 로컬 프로세스가 경로의 폴더
//!   하나를 루트 밖 링크로 바꾸면 임시 파일 · rename 이 루트 밖에 떨어진다. 막으려면 폴더 핸들 기준 `openat` · `renameat` 가
//!   필요하다(M1 에서는 하지 않는다). 루트 안에 동시에 쓸 수 있는 로컬 프로세스가 있어야 생기는 경합이다.

use std::fs::{self, File};
use std::io;
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

/// 판정 뒤에 연 파일이 루트 밖이었다([`Roots::open_inside`]). io 오류 안에 실려 RPC 오류로 바뀐다.
#[derive(Debug)]
pub struct Escaped(pub PathBuf);

impl std::fmt::Display for Escaped {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "`{}` was opened outside the project roots (a symlink changed while it was read)",
            self.0.display()
        )
    }
}

impl std::error::Error for Escaped {}

impl Roots {
    /// `real`(이미 판정한 경로)을 읽기로 열고, 연 핸들의 실제 경로가 루트 안인지 다시 본다.
    pub fn open_inside(&self, real: &Path) -> io::Result<File> {
        let file = File::open(real)?;
        if let Some(opened) = crate::handle::path_of(&file) {
            let opened = simplify(opened);
            if !self.contains(&opened) {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    Escaped(opened),
                ));
            }
        }
        Ok(file)
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

    /// 판정과 open 사이에 경로가 루트 밖 링크로 바뀌면(읽기 TOCTOU) 연 핸들의 실제 경로로 다시 판정해 버린다. 핸들 경로를
    /// 얻는 플랫폼(macOS · Linux)에서 본다.
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn a_path_swapped_to_an_outside_link_after_the_check_is_not_read() {
        let (_tmp, base) = real_tmp();
        let proj = base.join("proj");
        fs::create_dir_all(&proj).unwrap();
        fs::write(base.join("secret.txt"), "secret").unwrap();
        let path = proj.join("a.txt");
        fs::write(&path, "inside").unwrap();
        let roots = Roots::new([proj.clone()]);
        let checked = roots.readable(&path).unwrap();
        assert_eq!(
            crate::handle::path_of(&roots.open_inside(&checked.real).unwrap()),
            Some(path.clone())
        );

        // 판정은 끝났다. 그 사이 다른 프로세스가 같은 이름을 루트 밖 링크로 바꾼다.
        fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink(base.join("secret.txt"), &path).unwrap();
        let err = roots.open_inside(&checked.real).unwrap_err();
        let escaped = err
            .get_ref()
            .and_then(|e| e.downcast_ref::<Escaped>())
            .expect("escaped");
        assert_eq!(escaped.0, base.join("secret.txt"));
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
