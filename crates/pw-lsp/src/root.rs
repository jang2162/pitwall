//! 작업 공간 루트 고르기(15-m4-plan.md M4-05 「인스턴스」, 메인 판정 2026-11-02).
//!
//! 문서 폴더에서 프로젝트 루트까지 올라가며 정의의 `root_markers`(파일 이름 글롭) 중 하나가 있는 폴더를 보고, **그중 가장 위**
//! 폴더를 고른다. 없으면 프로젝트 루트다. 프로젝트 루트 위로는 가지 않는다 — 신뢰는 그 프로젝트를 허락한 것이다.
//!
//! 가장 가까운 폴더가 아니라 가장 위인 까닭: Cargo 작업 공간의 멤버 크레이트 · npm 작업 공간의 패키지마다 자기 `Cargo.toml` ·
//! `package.json` 이 있어, 가장 가까운 것을 고르면 크레이트 · 패키지마다 서버가 따로 뜬다(각자 작업 공간 전체를 읽는다). 가장 위를
//! 고르면 한 작업 공간은 서버 하나이고, 형제 폴더로 나뉜 작업 공간 둘(루트에 마커가 없는 모노레포의 하위 앱 둘)은 여전히 따로 뜬다.
//!
//! **알려진 한계**: 위 작업 공간이 제외한 중첩 작업 공간(`fuzz/` 의 독립 `Cargo.toml` 등)은 위 루트의 서버가 맡아 그 크레이트가
//! 잡히지 않을 수 있다 — 서버별 설정(rust-analyzer 의 `linkedProjects` 등)으로 푼다.

use std::path::{Path, PathBuf};

use globset::{Glob, GlobMatcher};

/// 마커 하나: 이름 그대로이거나 글롭.
enum Marker {
    Name(String),
    Glob(GlobMatcher),
}

impl Marker {
    fn new(pattern: &str) -> Option<Self> {
        if pattern.contains(['*', '?', '[', '{']) {
            Some(Self::Glob(Glob::new(pattern).ok()?.compile_matcher()))
        } else {
            Some(Self::Name(pattern.to_owned()))
        }
    }

    fn found_in(&self, dir: &Path) -> bool {
        match self {
            Self::Name(name) => dir.join(name).exists(),
            Self::Glob(glob) => std::fs::read_dir(dir).is_ok_and(|entries| {
                entries
                    .filter_map(Result::ok)
                    .any(|entry| glob.is_match(entry.file_name()))
            }),
        }
    }
}

/// `dir`(문서가 든 폴더)의 작업 공간 루트. `root` 는 프로젝트 루트(실제 경로)다. `dir` 이 루트 밖이거나 없으면 루트다. 돌려주는
/// 경로는 실제 경로(심볼릭 링크를 푼 것)다.
pub fn workspace_root(root: &Path, dir: &Path, markers: &[String]) -> PathBuf {
    let Ok(root) = std::fs::canonicalize(root) else {
        return root.to_path_buf();
    };
    let markers: Vec<Marker> = markers.iter().filter_map(|m| Marker::new(m)).collect();
    if markers.is_empty() {
        return root;
    }
    let Some(start) = std::fs::canonicalize(dir)
        .ok()
        .filter(|d| d.starts_with(&root))
    else {
        return root;
    };
    start
        .ancestors()
        .take_while(|d| d.starts_with(&root))
        .filter(|d| markers.iter().any(|m| m.found_in(d)))
        .last()
        .map_or(root.clone(), Path::to_path_buf)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn touch(path: &Path) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "").unwrap();
    }

    fn markers(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| (*s).to_owned()).collect()
    }

    /// 메인 판정 ①: 루트 Cargo 작업 공간의 멤버 크레이트 둘은 루트 하나로 묶인다.
    #[test]
    fn member_crates_of_a_workspace_share_its_root() {
        let tmp = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(tmp.path()).unwrap();
        touch(&root.join("Cargo.toml"));
        touch(&root.join("crates/a/Cargo.toml"));
        touch(&root.join("crates/b/Cargo.toml"));
        fs::create_dir_all(root.join("crates/a/src/deep")).unwrap();
        fs::create_dir_all(root.join("crates/b/src")).unwrap();
        let m = markers(&["Cargo.toml", "rust-project.json"]);
        assert_eq!(
            workspace_root(&root, &root.join("crates/a/src/deep"), &m),
            root
        );
        assert_eq!(workspace_root(&root, &root.join("crates/b/src"), &m), root);
    }

    /// 메인 판정 ②: 형제 작업 공간 둘 · 루트에 마커가 없는 모노레포의 하위 앱 둘은 따로다.
    #[test]
    fn sibling_workspaces_get_their_own_roots() {
        let tmp = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(tmp.path()).unwrap();
        touch(&root.join("apps/web/package.json"));
        touch(&root.join("apps/web/packages/ui/package.json"));
        touch(&root.join("apps/api/tsconfig.json"));
        fs::create_dir_all(root.join("apps/web/packages/ui/src")).unwrap();
        fs::create_dir_all(root.join("apps/api/src")).unwrap();
        fs::create_dir_all(root.join("docs")).unwrap();
        let m = markers(&["tsconfig.json", "jsconfig.json", "package.json"]);
        assert_eq!(
            workspace_root(&root, &root.join("apps/web/packages/ui/src"), &m),
            root.join("apps/web")
        );
        assert_eq!(
            workspace_root(&root, &root.join("apps/api/src"), &m),
            root.join("apps/api")
        );
        // 마커가 없는 곳은 프로젝트 루트.
        assert_eq!(workspace_root(&root, &root.join("docs"), &m), root);
    }

    #[test]
    fn markers_above_the_project_root_and_globs() {
        let tmp = tempfile::tempdir().unwrap();
        let base = fs::canonicalize(tmp.path()).unwrap();
        touch(&base.join("Cargo.toml"));
        let root = base.join("project");
        fs::create_dir_all(root.join("src")).unwrap();
        // 프로젝트 루트 위의 마커는 보지 않는다.
        assert_eq!(
            workspace_root(&root, &root.join("src"), &markers(&["Cargo.toml"])),
            root
        );
        touch(&root.join("lib/App.csproj"));
        assert_eq!(
            workspace_root(&root, &root.join("lib"), &markers(&["*.csproj"])),
            root.join("lib")
        );
        // 루트 밖 폴더는 루트로.
        assert_eq!(
            workspace_root(&root, &base, &markers(&["Cargo.toml"])),
            root
        );
        // 마커가 없는 정의는 루트.
        assert_eq!(workspace_root(&root, &root.join("lib"), &[]), root);
    }
}
