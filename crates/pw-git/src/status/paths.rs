//! git RPC 의 경로 표기: 저장소 작업 트리 기준 상대 경로, 구분자 `/`(git porcelain 과 같다). 작업 트리 꼭대기는 `""` 이다.
//! 응답은 작업 트리의 절대 경로(`workTree`)를 함께 준다 — 화면은 둘을 이어 절대 경로를 만든다.

use std::path::{Component, Path};

use gix::bstr::{BStr, BString, ByteSlice};

/// `path`(절대 경로)를 작업 트리 `work_tree` 기준 상대 경로로. 작업 트리 밖이거나 `..` 을 지나면 `None`. 둘 다 같은 정규화를
/// 거친 경로여야 한다(`pw_core::registry::path::normalize`).
pub fn relative(work_tree: &Path, path: &Path) -> Option<String> {
    let rest = path.strip_prefix(work_tree).ok()?;
    join(rest)
}

/// 상대 경로의 이름들을 `/` 로 잇는다(Windows 의 `\` 도 `/` 가 된다). `.` 은 건너뛰고, `..` · 루트 · 드라이브가 있으면 `None`.
pub fn join(rest: &Path) -> Option<String> {
    let mut out = String::new();
    for component in rest.components() {
        match component {
            Component::Normal(name) => {
                if !out.is_empty() {
                    out.push('/');
                }
                out.push_str(&name.to_string_lossy());
            }
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    Some(out)
}

/// gix 가 준 저장소 상대 경로(바이트, 이미 `/` 구분)를 RPC 의 문자열로. UTF-8 이 아닌 이름은 대체 글자로 바뀐다 — 그 경로는
/// 다시 입력으로 받아도 같은 파일을 가리키지 않는다.
pub fn from_git(path: &BStr) -> String {
    path.to_str_lossy().into_owned()
}

/// RPC 입력 경로가 저장소 상대 경로의 꼴인가: `/` 로 시작하지 않고 `..` 이름이 없다. 빈 문자열은 작업 트리 전체다. `:` · `\` 는
/// 유닉스에서 이름의 글자라 막지 않는다 — pathspec 이 글자 그대로 꼭대기 기준이라([`pathspec`]) 어떤 값도 저장소 밖을 가리키지 않는다.
pub fn is_valid(path: &str) -> bool {
    !path.starts_with('/') && !path.split('/').any(|name| name == "..")
}

/// 경로 하나를 gix pathspec 으로: 저장소 꼭대기 기준(`top` — 프로세스의 현재 폴더와 상관없이), 글자 그대로(`literal` — `*` · `?` 를
/// 패턴으로 보지 않는다). 폴더면 그 아래 전체에 맞는다. 빈 문자열은 작업 트리 전체다.
pub fn pathspec(path: &str) -> BString {
    let path = path.trim_end_matches('/');
    if path.is_empty() {
        BString::from(":(top)")
    } else {
        BString::from(format!(":(top,literal){path}"))
    }
}

/// `path` 가 `under` 이거나 그 아래인가(`/` 단위). `under` 가 빈 문자열이면 모든 경로.
pub fn is_within(path: &str, under: &str) -> bool {
    let under = under.trim_end_matches('/');
    under.is_empty()
        || path == under
        || path
            .strip_prefix(under)
            .is_some_and(|rest| rest.starts_with('/'))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn relative_paths_use_slashes_and_stay_inside_the_work_tree() {
        let work_tree = PathBuf::from("/w/repo");
        assert_eq!(
            relative(&work_tree, Path::new("/w/repo/src/a.rs")).as_deref(),
            Some("src/a.rs")
        );
        assert_eq!(
            relative(&work_tree, Path::new("/w/repo")).as_deref(),
            Some("")
        );
        assert_eq!(relative(&work_tree, Path::new("/w/other/a.rs")), None);
        assert_eq!(relative(&work_tree, Path::new("/w/repository/a")), None);
    }

    /// Windows 의 절대 경로(`\` 구분 · 드라이브)도 `/` 구분 상대 경로가 된다.
    #[cfg(windows)]
    #[test]
    fn windows_paths_become_slash_separated() {
        let work_tree = PathBuf::from(r"C:\w\repo");
        assert_eq!(
            relative(&work_tree, Path::new(r"C:\w\repo\src\deep\a.rs")).as_deref(),
            Some("src/deep/a.rs")
        );
        assert_eq!(relative(&work_tree, Path::new(r"D:\w\repo\a.rs")), None);
        assert_eq!(join(Path::new(r"src\a.rs")).as_deref(), Some("src/a.rs"));
        assert_eq!(join(Path::new(r"C:\src")), None);
    }

    #[test]
    fn input_paths_must_be_repository_relative() {
        for ok in ["", "a", "src/a.rs", "dir/", "a b/c", "a:b", r"a\b"] {
            assert!(is_valid(ok), "{ok}");
        }
        for bad in ["/a", "../a", "a/../b", "a/.."] {
            assert!(!is_valid(bad), "{bad}");
        }
    }

    #[test]
    fn pathspecs_are_literal_and_anchored_at_the_top() {
        assert_eq!(pathspec("src/*.rs"), ":(top,literal)src/*.rs");
        assert_eq!(pathspec("dir/"), ":(top,literal)dir");
        assert_eq!(pathspec(""), ":(top)");
    }

    #[test]
    fn within_is_by_whole_names() {
        assert!(is_within("src/a.rs", "src"));
        assert!(is_within("src/a.rs", "src/"));
        assert!(is_within("src", "src"));
        assert!(is_within("anything", ""));
        assert!(!is_within("srcx/a.rs", "src"));
        assert!(!is_within("src", "src/a.rs"));
    }
}
