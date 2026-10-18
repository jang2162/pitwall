//! 경로 비교 규칙. 레지스트리에 넣는 루트와 해석하려는 경로를 같은 방법으로 정규화한 뒤 **구성 요소 단위 접두사**로 비교한다.
//!
//! - 상대 경로는 받지 않는다. Host 의 작업 디렉터리는 호출자(CLI · Orca)의 것과 다르다 — CLI 가 자기 위치에서 절대 경로로 바꿔 넘긴다.
//! - 있는 부분은 `fs::canonicalize` 로 실제 경로가 된다: 심링크를 따라가고 `.` · `..` 를 푼다. macOS(APFS 기본)와 Windows 는
//!   여기서 디스크에 적힌 대소문자로 돌아오므로 `/Users/me/Proj` 와 `/users/ME/proj` 가 같은 경로가 된다.
//!   대소문자를 무시하는 Linux 파일 시스템(ext4 casefold 등)은 `realpath` 가 대소문자를 고치지 않아 다른 경로로 본다.
//! - 없는 부분(아직 만들지 않은 파일 등)은 가장 깊은 있는 조상까지만 실제 경로로 바꾸고 나머지는 글자 그대로 잇는다.
//!   그 나머지의 `..` 는 글자로 푼다 — 없는 구성 요소는 심링크일 수 없으므로 글자로 푸는 것이 맞다.
//! - Windows 에서 `canonicalize` 가 붙이는 verbatim 접두(`\\?\C:\…`)는 걷는다([`simplify`]). 저장 · 표시 · 다른 프로세스
//!   (터미널 cwd, 외부 도구 인자)로 넘기는 경로가 모두 이것이고, `cmd.exe` 등은 그 접두를 받지 못한다.

use std::fs;
use std::path::{Component, Path, PathBuf};

use crate::config::gitdir::{WorktreeLink, linked_worktree};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathRejected {
    Relative,
}

/// 비교에 쓰는 경로. 위 규칙을 따른다.
pub fn normalize(path: &Path) -> Result<PathBuf, PathRejected> {
    if !path.is_absolute() {
        return Err(PathRejected::Relative);
    }
    let components: Vec<Component<'_>> = path.components().collect();
    for split in (1..=components.len()).rev() {
        let head: PathBuf = components[..split].iter().collect();
        // 없음 · 권한 없음 · 디렉터리가 아닌 것 아래 등 어떤 실패든 한 칸 위로 올라간다.
        let Ok(mut real) = fs::canonicalize(&head) else {
            continue;
        };
        for component in &components[split..] {
            match component {
                Component::Normal(name) => real.push(name),
                Component::ParentDir => {
                    real.pop();
                }
                Component::CurDir | Component::RootDir | Component::Prefix(_) => {}
            }
        }
        return Ok(simplify(real));
    }
    // 루트조차 canonicalize 되지 않는 경우(없는 드라이브 등): 글자로만 정리한다.
    Ok(simplify(lexical(path)))
}

/// [`linked_worktree`] 의 결과를 이 모듈의 비교 규칙에 맞춘다(`main_top` 은 `canonicalize` 에서 와서 verbatim 접두가 있을 수 있다).
pub fn worktree_link(path: &Path) -> Option<WorktreeLink> {
    linked_worktree(path).map(|link| WorktreeLink {
        worktree_top: simplify(link.worktree_top),
        main_top: simplify(link.main_top),
    })
}

/// Windows verbatim 경로를 보통 경로로 바꾼다. 다른 OS 에서는 그대로 돌려준다.
pub fn simplify(path: PathBuf) -> PathBuf {
    #[cfg(windows)]
    if let Some(plain) = path.to_str().and_then(strip_verbatim) {
        return PathBuf::from(plain);
    }
    path
}

/// `\\?\C:\a` → `C:\a`, `\\?\UNC\server\share\a` → `\\server\share\a`. 접두를 걷으면 뜻이 바뀌는 경로는 `None` 이라
/// verbatim 그대로 둔다: 260자(MAX_PATH) 이상, 장치 이름(`con` 등)인 구성 요소, `.` · 공백으로 끝나는 구성 요소, Win32 경로에
/// 못 쓰는 문자. 문자열만 보는 함수라 모든 OS 에서 시험한다.
#[cfg_attr(not(windows), allow(dead_code))]
fn strip_verbatim(path: &str) -> Option<String> {
    const MAX_PATH: usize = 260;
    let rest = path.strip_prefix(r"\\?\")?;
    let (plain, body) = if let Some(unc) = rest.strip_prefix(r"UNC\") {
        (format!(r"\\{unc}"), unc)
    } else {
        let drive = rest.as_bytes();
        if drive.len() < 3 || !drive[0].is_ascii_alphabetic() || &drive[1..3] != b":\\" {
            return None;
        }
        (rest.to_owned(), &rest[3..])
    };
    let safe = plain.len() < MAX_PATH
        && body.split('\\').filter(|c| !c.is_empty()).all(|component| {
            let stem = component
                .split('.')
                .next()
                .unwrap_or_default()
                .to_ascii_lowercase();
            !component.ends_with(['.', ' '])
                && !component.contains(['<', '>', ':', '"', '/', '|', '?', '*'])
                && !super::slug::WINDOWS_DEVICES.contains(&stem.as_str())
        });
    safe.then_some(plain)
}

fn lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verbatim_prefixes_are_stripped_only_when_the_meaning_stays() {
        assert_eq!(
            strip_verbatim(r"\\?\C:\Users\me\proj").as_deref(),
            Some(r"C:\Users\me\proj")
        );
        assert_eq!(strip_verbatim(r"\\?\d:\").as_deref(), Some(r"d:\"));
        assert_eq!(
            strip_verbatim(r"\\?\UNC\server\share\repo").as_deref(),
            Some(r"\\server\share\repo")
        );
        // 보통 경로 · 다른 verbatim 형식은 건드리지 않는다.
        assert_eq!(strip_verbatim(r"C:\Users"), None);
        assert_eq!(strip_verbatim(r"\\?\Volume{0000}\x"), None);
        // 걷으면 뜻이 바뀌는 경로는 verbatim 으로 둔다.
        assert_eq!(strip_verbatim(r"\\?\C:\dir\con"), None);
        assert_eq!(strip_verbatim(r"\\?\C:\dir\NUL.txt"), None);
        assert_eq!(strip_verbatim(r"\\?\C:\trailing.\x"), None);
        assert_eq!(strip_verbatim(r"\\?\C:\space \x"), None);
        assert_eq!(strip_verbatim(r"\\?\C:\a|b"), None);
        let long = format!(r"\\?\C:\{}", "x".repeat(300));
        assert_eq!(strip_verbatim(&long), None);
    }

    /// Windows 에서는 정규화 결과에 verbatim 접두가 없다.
    #[cfg(windows)]
    #[test]
    fn normalized_paths_have_no_verbatim_prefix_on_windows() {
        let tmp = tempfile::tempdir().unwrap();
        let normalized = normalize(tmp.path()).unwrap();
        assert!(
            !normalized.to_str().unwrap().starts_with(r"\\?\"),
            "{normalized:?}"
        );
        assert!(normalized.is_dir());
    }

    #[test]
    fn relative_paths_are_rejected() {
        assert_eq!(
            normalize(Path::new("src/main.rs")),
            Err(PathRejected::Relative)
        );
        assert_eq!(normalize(Path::new("../x")), Err(PathRejected::Relative));
    }

    #[test]
    fn dot_segments_resolve_and_missing_tail_is_kept() {
        let tmp = tempfile::tempdir().unwrap();
        // Windows 의 canonicalize 는 verbatim 접두를 붙이고 normalize 는 걷는다.
        let real = simplify(fs::canonicalize(tmp.path()).unwrap());
        fs::create_dir_all(real.join("a/b")).unwrap();
        assert_eq!(
            normalize(&tmp.path().join("a/./b/../b")).unwrap(),
            real.join("a/b")
        );
        // `missing` 은 없다. 그 뒤의 `..` 는 글자로 풀린다.
        assert_eq!(
            normalize(&tmp.path().join("a/missing/new.rs")).unwrap(),
            real.join("a/missing/new.rs")
        );
        assert_eq!(
            normalize(&tmp.path().join("a/missing/../b")).unwrap(),
            real.join("a/b")
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_are_followed_before_parent_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let real = fs::canonicalize(tmp.path()).unwrap();
        fs::create_dir_all(real.join("target/inner")).unwrap();
        fs::create_dir_all(real.join("elsewhere")).unwrap();
        std::os::unix::fs::symlink(real.join("target/inner"), real.join("elsewhere/link")).unwrap();
        // 글자로 풀면 `elsewhere` 가 되지만, 링크를 따라간 뒤의 `..` 는 `target` 이다.
        assert_eq!(
            normalize(&real.join("elsewhere/link/..")).unwrap(),
            real.join("target")
        );
        assert_eq!(
            normalize(&real.join("elsewhere/link/file.rs")).unwrap(),
            real.join("target/inner/file.rs")
        );
    }

    /// 대소문자를 무시하는 파일 시스템(macOS 기본 APFS, Windows NTFS)에서는 디스크의 대소문자로 맞춰진다.
    /// 구분하는 파일 시스템이면 다른 대소문자 경로가 없으므로 이 시험은 할 것이 없다.
    #[test]
    fn case_insensitive_file_systems_compare_by_disk_case() {
        let tmp = tempfile::tempdir().unwrap();
        let real = simplify(fs::canonicalize(tmp.path()).unwrap());
        fs::create_dir_all(real.join("CaseDir/Inner")).unwrap();
        let other_case = real.join("casedir/inner");
        if !other_case.exists() {
            return;
        }
        assert_eq!(normalize(&other_case).unwrap(), real.join("CaseDir/Inner"));
        assert_eq!(
            normalize(&real.join("casedir/inner/New.rs")).unwrap(),
            real.join("CaseDir/Inner/New.rs")
        );
    }
}
