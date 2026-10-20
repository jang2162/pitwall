//! 폴더 한 단계 목록과 각 항목의 ignore 여부.
//!
//! ignore 판정은 `ignore` 크레이트(ripgrep 의 규칙: `.gitignore` · `.git/info/exclude` · 전역 gitignore · `.ignore`)에 맡긴다.
//! 그 크레이트는 무시된 항목을 **건너뛸** 뿐 표시하지 않으므로, 루트에서 이 폴더까지만 내려가는 걷기로 「보이는 항목」을 얻고
//! `read_dir` 의 전체 항목과 비교한다. 루트부터 걸어야 무시된 폴더 안(`target/debug`)의 항목도 무시된 것으로 나온다 — 그 폴더에서
//! 걷기 시작하면 걷는 시작점은 판정하지 않는다. 숨김 파일은 ignore 가 아니다(`hidden(false)`).

use std::collections::HashSet;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};

use crate::scope::Readable;

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
    /// 심링크면 가리키는 것의 종류. 깨진 링크면 `null`. 심링크가 아니면 `null`.
    pub target: Option<EntryKind>,
    /// 바이트. 심링크면 대상의 크기(깨졌으면 0).
    #[specta(type = pw_rpc::JsNumber)]
    pub size: u64,
    /// 수정 시각(유닉스 밀리초). 심링크면 대상의 것. 알 수 없으면 `null`.
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

/// 이보다 항목이 많으면 메타데이터 읽기를 스레드로 나눈다. 항목마다 `lstat` 이 한 번이라 1만 개 폴더에서 가장 큰 비용이다.
const PARALLEL_FROM: usize = 1024;

/// `dir.inside` 의 항목. 이름순.
///
/// ignore 판정(걷기)과 메타데이터 읽기는 서로 기다리지 않으므로 함께 돈다. 1만 항목 폴더의 `fs.list` 100 ms(M1-01 완료 기준 ⑤)를
/// 맞추려는 것이다 — 따로 재면 `lstat` 1만 번이 걷기보다 길다.
pub fn list(dir: &Readable) -> io::Result<Vec<FsEntry>> {
    let raw: Vec<fs::DirEntry> = fs::read_dir(&dir.inside)?.collect::<io::Result<_>>()?;
    let (visible, mut entries) = std::thread::scope(|s| {
        let walker = s.spawn(|| visible_names(dir));
        let entries = entries_of(&raw);
        // 걷기가 패닉하면 모두 무시되지 않은 것으로 보지 않고, 무시된 것으로 표시한다(숨김 쪽이 안전하다).
        (walker.join().unwrap_or(None), entries)
    });
    for entry in &mut entries {
        entry.0.ignored = visible
            .as_ref()
            .is_none_or(|names| !names.contains(&entry.1));
    }
    let mut entries: Vec<FsEntry> = entries.into_iter().map(|(entry, _)| entry).collect();
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(entries)
}

/// 항목과 그 원래 이름. `ignored` 는 아직 정하지 않았다.
fn entries_of(raw: &[fs::DirEntry]) -> Vec<(FsEntry, OsString)> {
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
fn entry(raw: &fs::DirEntry) -> Option<(FsEntry, OsString)> {
    let name = raw.file_name();
    let lmeta = raw.metadata().ok()?;
    let kind = EntryKind::of(lmeta.file_type());
    let (target, meta) = if kind == EntryKind::Symlink {
        match fs::metadata(raw.path()) {
            Ok(m) => (Some(EntryKind::of(m.file_type())), Some(m)),
            Err(_) => (None, None),
        }
    } else {
        (None, None)
    };
    let meta = meta.as_ref().unwrap_or(&lmeta);
    Some((
        FsEntry {
            name: name.to_string_lossy().into_owned(),
            kind,
            target,
            size: if kind == EntryKind::Symlink && target.is_none() {
                0
            } else {
                meta.len()
            },
            mtime_ms: mtime_ms(meta),
            ignored: false,
        },
        name,
    ))
}

/// `dir` 의 항목 중 무시되지 않는 것의 이름. `dir` 자체가 무시되는 곳(또는 그 안)이면 `None`.
fn visible_names(dir: &Readable) -> Option<HashSet<OsString>> {
    let depth = dir
        .inside
        .strip_prefix(&dir.root)
        .ok()?
        .components()
        .count();
    let target: PathBuf = dir.inside.clone();
    let walk = ignore::WalkBuilder::new(&dir.root)
        .hidden(false)
        // 루트 안 심링크 폴더를 지나는 요청(`node_modules/pkg`)이면 그 길로 내려간다. 링크를 따르면 걷기가 항목마다 stat 하므로
        // 그때만 켠다. 깊이를 막아 두므로 고리가 돌지 않는다.
        .follow_links(dir.inside != dir.real)
        .max_depth(Some(depth + 1))
        .filter_entry(move |e| target.starts_with(e.path()) || e.path().parent() == Some(&target))
        .build();
    let mut reached = false;
    let mut names = HashSet::new();
    for entry in walk.flatten() {
        if entry.depth() == depth && entry.path() == dir.inside {
            reached = true;
        } else if entry.depth() == depth + 1 {
            names.insert(entry.file_name().to_os_string());
        }
    }
    reached.then_some(names)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::scope::Roots;
    use pw_core::registry::path::simplify;

    fn listed(roots: &Roots, dir: &Path) -> Vec<(String, EntryKind, bool)> {
        list(&roots.readable(dir).unwrap())
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
        let entries = list(&roots.readable(&root).unwrap()).unwrap();
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
        let entries = list(&roots.readable(&root).unwrap()).unwrap();
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
}
