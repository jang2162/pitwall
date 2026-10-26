//! git RPC 의 경로 꼴: 작업 트리 기준 상대 경로, 구분자 `/`. Windows 에서도 `/` 로 나온다(그 OS 의 `Path::join` 은 `\` 를 쓴다).

use std::path::{Path, PathBuf};

use super::super::repo_path;

fn root() -> PathBuf {
    if cfg!(windows) {
        PathBuf::from(r"C:\work\repo")
    } else {
        PathBuf::from("/work/repo")
    }
}

#[test]
fn paths_are_relative_to_the_work_tree_with_forward_slashes() {
    let root = root();
    let nested = root.join("src").join("lib.rs");
    assert_eq!(repo_path(&root, &nested).as_deref(), Some("src/lib.rs"));
    assert_eq!(
        repo_path(&root, &Path::new("docs").join("a b.md")).as_deref(),
        Some("docs/a b.md"),
        "상대 경로는 그대로(구분자만 `/`)"
    );
    assert_eq!(
        repo_path(&root, &Path::new(".").join("x").join(".").join("y")).as_deref(),
        Some("x/y")
    );
    #[cfg(windows)]
    assert_eq!(
        repo_path(&root, Path::new(r"src\main.rs")).as_deref(),
        Some("src/main.rs"),
        "Windows 의 `\\` 는 `/` 가 된다"
    );
}

#[test]
fn paths_outside_the_work_tree_or_empty_are_refused() {
    let root = root();
    let outside = root.parent().unwrap().join("other").join("f");
    assert_eq!(repo_path(&root, &outside), None);
    assert_eq!(repo_path(&root, &Path::new("..").join("f")), None);
    assert_eq!(repo_path(&root, &Path::new("a").join("..").join("b")), None);
    assert_eq!(
        repo_path(&root, &root),
        None,
        "작업 트리 자체는 경로가 아니다"
    );
    assert_eq!(repo_path(&root, Path::new("")), None);
}
