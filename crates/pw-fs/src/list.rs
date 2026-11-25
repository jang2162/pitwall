//! 폴더 한 단계 목록과 각 항목의 ignore 여부.
//!
//! ignore 판정은 ripgrep 의 규칙(`.gitignore` · `.git/info/exclude` · 전역 gitignore · `.ignore`)이다. ignore 파일은
//! [`crate::ignore_rules`] 가 안전하게 읽는다 — `ignore` 크레이트의 걷기는 링크를 따라 끝까지 읽어 `.gitignore -> /dev/zero` 하나로
//! 메모리가 끝없이 는다. 판정은 루트부터 걷는 것과 같다 — 무시된 폴더 안(`target/debug`)의 항목도 무시된 것으로 나오고, 루트
//! 자신은 판정하지 않는다([`list`]). 숨김 파일은 ignore 가 아니다.
//!
//! 실제 대상이 루트 밖인 심링크는 항목으로 보이되 `outside` 로 표시하고, 대상의 종류 · 크기 · 시각을 알리지 않는다(결정 M1-D8).

use std::fs;
use std::io;
use std::panic::AssertUnwindSafe;
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};

use pw_core::registry::path::normalize;

use crate::ignore_rules::{Folder, IgnoreRules};
use crate::scope::{Readable, Roots};

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum EntryKind {
    File,
    Dir,
    Symlink,
    /// 소켓 · 장치 · FIFO 등.
    Other,
}

impl EntryKind {
    pub fn of(file_type: fs::FileType) -> Self {
        if file_type.is_symlink() {
            EntryKind::Symlink
        } else if file_type.is_dir() {
            EntryKind::Dir
        } else if file_type.is_file() {
            EntryKind::File
        } else {
            EntryKind::Other
        }
    }
}

/// 폴더의 항목 하나.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct FsEntry {
    /// UTF-8 이 아닌 이름은 대체 문자로 바뀌어 온다.
    pub name: String,
    pub kind: EntryKind,
    /// 심링크면 가리키는 것의 종류. 깨진 링크 · 루트 밖을 가리키는 링크 · 심링크가 아니면 `null`.
    pub target: Option<EntryKind>,
    /// 심링크의 실제 대상이 프로젝트 루트 밖이다. 따라 읽거나 펼칠 수 없다(`fs.*` 가 거절한다).
    pub outside: bool,
    /// 바이트. 심링크면 대상의 크기(깨졌거나 루트 밖이면 0).
    #[specta(type = pw_rpc::JsNumber)]
    pub size: u64,
    /// 수정 시각(유닉스 밀리초). 심링크면 대상의 것(루트 밖이면 링크 자신의 것). 알 수 없으면 `null`.
    #[specta(type = Option<pw_rpc::JsNumber>)]
    pub mtime_ms: Option<i64>,
    /// `.gitignore` 등으로 무시되는 항목(무시된 폴더 안의 항목 포함).
    pub ignored: bool,
}

pub fn mtime_ms(meta: &fs::Metadata) -> Option<i64> {
    let modified = meta.modified().ok()?;
    match modified.duration_since(UNIX_EPOCH) {
        Ok(d) => i64::try_from(d.as_millis()).ok(),
        Err(e) => i64::try_from(e.duration().as_millis()).ok().map(|ms| -ms),
    }
}

/// 이보다 항목이 많으면 항목 읽기(메타데이터 · ignore 판정)를 스레드로 나눈다. 항목마다 `lstat` 이 한 번이라 1만 개 폴더에서
/// 가장 큰 비용이다.
const PARALLEL_FROM: usize = 1024;

/// `dir.real` 의 항목. 이름순. `roots` 로 심링크 항목의 실제 대상이 루트 안인지 본다.
///
/// ignore 판정은 `read_dir` 의 항목을 [`IgnoreRules`] 로 직접 한다 — 루트에서 이 폴더까지 내려가는 걷기로 「보이는 항목」을 얻던
/// 때(걷기가 같은 폴더를 한 번 더 읽고 항목마다 경로를 비교했다)는 걷기가 1만 항목 폴더의 `fs.list` 시간의 대부분이었다(M1-01 완료
/// 기준 ⑤ 100 ms). 판정은 걷기와 같다: 루트 아래 조상 폴더(이 폴더 포함) 하나라도 무시되면 모든 항목이 무시된 것이고(걷기가 그
/// 폴더에 내려가지 않는다), 아니면 항목마다 그 종류(링크는 따르지 않는다)로 판정한다. 루트 자신은 판정하지 않는다(걷기의 시작점).
pub fn list(dir: &Readable, roots: &Roots) -> io::Result<Vec<FsEntry>> {
    let raw: Vec<fs::DirEntry> = fs::read_dir(&dir.real)?.collect::<io::Result<_>>()?;
    let rules = IgnoreRules::new(roots.clone());
    // 판정이 패닉하면 모두 무시되지 않은 것으로 보지 않고, 무시된 것으로 표시한다(숨김 쪽이 안전하다).
    let folder = std::panic::catch_unwind(AssertUnwindSafe(|| {
        (!inside_ignored(dir, &rules)).then(|| rules.folder(&dir.real))
    }))
    .unwrap_or(None);
    let mut entries = entries_of(&raw, roots, folder.as_ref());
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(entries)
}

/// 루트 아래에서 `dir.real` 까지의 폴더(루트 자신은 빼고 `dir.real` 은 넣는다) 중 무시되는 것이 있다.
fn inside_ignored(dir: &Readable, rules: &IgnoreRules) -> bool {
    let Ok(below) = dir.real.strip_prefix(&dir.root) else {
        return true;
    };
    let mut at = dir.root.clone();
    below.components().any(|part| {
        at.push(part);
        rules.ignored(&at, true)
    })
}

/// 항목들. `folder` 가 `None` 이면 모두 무시된 것이다.
fn entries_of(raw: &[fs::DirEntry], roots: &Roots, folder: Option<&Folder>) -> Vec<FsEntry> {
    let entry = |raw: &fs::DirEntry| entry(raw, roots, folder);
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get().min(8));
    if raw.len() < PARALLEL_FROM || threads == 1 {
        return raw.iter().filter_map(entry).collect();
    }
    let chunk = raw.len().div_ceil(threads);
    std::thread::scope(|s| {
        let parts: Vec<_> = raw
            .chunks(chunk)
            .map(|part| s.spawn(move || part.iter().filter_map(entry).collect::<Vec<_>>()))
            .collect();
        parts
            .into_iter()
            .flat_map(|p| p.join().unwrap_or_default())
            .collect()
    })
}

/// 항목을 읽는 사이 지워졌으면 `None`.
fn entry(raw: &fs::DirEntry, roots: &Roots, folder: Option<&Folder>) -> Option<FsEntry> {
    let lmeta = raw.metadata().ok()?;
    let kind = EntryKind::of(lmeta.file_type());
    let path = raw.path();
    let ignored = folder.is_none_or(|folder| {
        std::panic::catch_unwind(AssertUnwindSafe(|| {
            folder.ignored(&path, kind == EntryKind::Dir)
        }))
        .unwrap_or(true)
    });
    let mut outside = false;
    let (target, meta) = if kind == EntryKind::Symlink {
        // 풀리지 않는 링크(대상 없음 · 고리)는 링크 경로 그대로 돌아와 루트 안으로 판정되고, 아래 metadata 가 실패한다.
        outside = normalize(&path).is_ok_and(|real| !roots.contains(&real));
        match fs::metadata(&path) {
            Ok(m) if !outside => (Some(EntryKind::of(m.file_type())), Some(m)),
            _ => (None, None),
        }
    } else {
        (None, None)
    };
    let meta = meta.as_ref().unwrap_or(&lmeta);
    Some(FsEntry {
        name: raw.file_name().to_string_lossy().into_owned(),
        kind,
        target,
        outside,
        size: if kind == EntryKind::Symlink && target.is_none() {
            0
        } else {
            meta.len()
        },
        mtime_ms: mtime_ms(meta),
        ignored,
    })
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::*;
    use crate::scope::Roots;
    use pw_core::registry::path::simplify;

    fn listed(roots: &Roots, dir: &Path) -> Vec<(String, EntryKind, bool)> {
        list(&roots.readable(dir).unwrap(), roots)
            .unwrap()
            .into_iter()
            .map(|e| (e.name, e.kind, e.ignored))
            .collect()
    }

    /// 저장소처럼 `.git` 이 있어야 `.gitignore` 가 적용된다(ripgrep 과 같다).
    fn repo() -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let root = simplify(fs::canonicalize(tmp.path()).unwrap()).join("repo");
        fs::create_dir_all(root.join(".git")).unwrap();
        (tmp, root)
    }

    #[test]
    fn marks_ignored_entries_including_those_inside_an_ignored_folder() {
        let (_tmp, root) = repo();
        fs::write(root.join(".gitignore"), "target/\n*.log\n").unwrap();
        fs::create_dir_all(root.join("target/debug")).unwrap();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/main.rs"), "fn main() {}").unwrap();
        fs::write(root.join("src/run.log"), "x").unwrap();
        fs::write(root.join(".env"), "x").unwrap();
        let roots = Roots::new([root.clone()]);

        assert_eq!(
            listed(&roots, &root),
            [
                (".env".to_owned(), EntryKind::File, false),
                (".git".to_owned(), EntryKind::Dir, false),
                (".gitignore".to_owned(), EntryKind::File, false),
                ("src".to_owned(), EntryKind::Dir, false),
                ("target".to_owned(), EntryKind::Dir, true),
            ]
        );
        assert_eq!(
            listed(&roots, &root.join("src")),
            [
                ("main.rs".to_owned(), EntryKind::File, false),
                ("run.log".to_owned(), EntryKind::File, true),
            ]
        );
        // 무시된 폴더를 펼치면 그 안의 항목도 무시된 것이다.
        assert_eq!(
            listed(&roots, &root.join("target")),
            [("debug".to_owned(), EntryKind::Dir, true)]
        );
    }

    #[test]
    fn reports_size_and_mtime() {
        let (_tmp, root) = repo();
        fs::write(root.join("a.txt"), "12345").unwrap();
        let roots = Roots::new([root.clone()]);
        let entries = list(&roots.readable(&root).unwrap(), &roots).unwrap();
        let a = entries.iter().find(|e| e.name == "a.txt").unwrap();
        assert_eq!(a.size, 5);
        assert!(a.mtime_ms.unwrap() > 0);
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_report_their_target_kind() {
        let (_tmp, root) = repo();
        fs::create_dir_all(root.join("real")).unwrap();
        std::os::unix::fs::symlink(root.join("real"), root.join("to-dir")).unwrap();
        std::os::unix::fs::symlink(root.join("missing"), root.join("broken")).unwrap();
        let roots = Roots::new([root.clone()]);
        let entries = list(&roots.readable(&root).unwrap(), &roots).unwrap();
        let get = |n: &str| entries.iter().find(|e| e.name == n).unwrap().clone();
        assert_eq!(get("to-dir").kind, EntryKind::Symlink);
        assert_eq!(get("to-dir").target, Some(EntryKind::Dir));
        assert_eq!(get("broken").target, None);
    }

    /// 메인 체크아웃이 워크트리 폴더(`.worktrees/`)를 무시해도, 워크트리 루트 아래 항목은 워크트리의 규칙으로 판정한다.
    #[test]
    fn a_worktree_nested_in_an_ignored_folder_uses_its_own_rules() {
        let (_tmp, main) = repo();
        fs::write(main.join(".gitignore"), ".worktrees/\n").unwrap();
        let wt = main.join(".worktrees/wt");
        fs::create_dir_all(wt.join("src")).unwrap();
        fs::write(wt.join(".git"), "gitdir: elsewhere\n").unwrap();
        fs::write(wt.join(".gitignore"), "*.tmp\n").unwrap();
        fs::write(wt.join("src/a.rs"), "").unwrap();
        fs::write(wt.join("src/b.tmp"), "").unwrap();
        let roots = Roots::new([main.clone(), wt.clone()]);
        assert_eq!(
            listed(&roots, &wt.join("src")),
            [
                ("a.rs".to_owned(), EntryKind::File, false),
                ("b.tmp".to_owned(), EntryKind::File, true),
            ]
        );
    }

    /// 루트 자신은 판정하지 않는다 — 바깥 저장소가 루트 폴더를 그대로 무시해도(`/wt/`) 루트 아래 항목은 무시되지 않는다.
    #[test]
    fn the_root_itself_is_not_judged_by_an_outer_repository() {
        let (_tmp, main) = repo();
        fs::write(main.join(".gitignore"), "/wt/\n").unwrap();
        let wt = main.join("wt");
        fs::create_dir_all(wt.join("src")).unwrap();
        fs::write(wt.join(".git"), "gitdir: elsewhere\n").unwrap();
        fs::write(wt.join("src/a.rs"), "").unwrap();
        let roots = Roots::new([main.clone(), wt.clone()]);
        assert_eq!(
            listed(&roots, &wt),
            [
                (".git".to_owned(), EntryKind::File, false),
                ("src".to_owned(), EntryKind::Dir, false),
            ]
        );
        assert_eq!(
            listed(&roots, &wt.join("src")),
            [("a.rs".to_owned(), EntryKind::File, false)]
        );
        // 바깥 루트에서 보면 그 폴더는 무시된 것이다.
        assert_eq!(
            listed(&roots, &main)
                .into_iter()
                .find(|(name, ..)| name == "wt"),
            Some(("wt".to_owned(), EntryKind::Dir, true))
        );
    }

    /// `.gitignore -> /dev/zero` 링크는 읽지 않는다(없는 것으로 본다) — 끝까지 읽으면 메모리가 끝없이 는다. 같은 폴더의 `.ignore`
    /// 는 그대로 적용된다.
    #[cfg(unix)]
    #[test]
    fn an_ignore_file_linked_to_dev_zero_is_not_read() {
        use crate::search::tests::{MIB, bounded};

        let (_tmp, root) = repo();
        std::os::unix::fs::symlink("/dev/zero", root.join(".gitignore")).unwrap();
        fs::write(root.join(".ignore"), "*.log\n").unwrap();
        fs::write(root.join("a.log"), "x").unwrap();
        fs::write(root.join("a.rs"), "x").unwrap();
        let roots = Roots::new([root.clone()]);
        let dir = root.clone();
        let got = bounded(64 * MIB, std::time::Duration::from_secs(10), move || {
            listed(&roots, &dir)
        });
        assert_eq!(
            got,
            [
                (".git".to_owned(), EntryKind::Dir, false),
                (".gitignore".to_owned(), EntryKind::Symlink, false),
                (".ignore".to_owned(), EntryKind::File, false),
                ("a.log".to_owned(), EntryKind::File, true),
                ("a.rs".to_owned(), EntryKind::File, false),
            ]
        );
    }

    /// FIFO 를 가리키는 `.gitignore` · `.ignore` 링크는 열지 않는다 — 열면 쓰는 쪽이 없어 영영 막힌다. 위 폴더의 규칙은 그대로다.
    #[cfg(unix)]
    #[test]
    fn ignore_files_linked_to_a_fifo_do_not_block_the_list() {
        use crate::search::tests::{MIB, bounded};

        let (tmp, root) = repo();
        let fifo = simplify(fs::canonicalize(tmp.path()).unwrap()).join("fifo");
        let c = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
        // SAFETY: `c` 는 NUL 로 끝나는 살아 있는 경로 문자열이다.
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o644) }, 0);
        fs::write(root.join(".gitignore"), "*.log\n").unwrap();
        let sub = root.join("sub");
        fs::create_dir_all(&sub).unwrap();
        std::os::unix::fs::symlink(&fifo, sub.join(".gitignore")).unwrap();
        std::os::unix::fs::symlink(&fifo, sub.join(".ignore")).unwrap();
        fs::write(sub.join("a.log"), "x").unwrap();
        fs::write(sub.join("a.rs"), "x").unwrap();
        let roots = Roots::new([root.clone()]);
        let got = bounded(64 * MIB, std::time::Duration::from_secs(10), move || {
            listed(&roots, &sub)
        });
        assert_eq!(
            got,
            [
                (".gitignore".to_owned(), EntryKind::Symlink, false),
                (".ignore".to_owned(), EntryKind::Symlink, false),
                ("a.log".to_owned(), EntryKind::File, true),
                ("a.rs".to_owned(), EntryKind::File, false),
            ]
        );
    }
}
