//! 목록을 걷고, 바뀐 경로를 디스크와 맞춘다. 규칙은 [`super`] 머리 주석.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::ffi::OsString;
use std::ops::Bound;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use ignore::{WalkBuilder, WalkState};
use pw_core::registry::path::normalize;

use crate::ignore_rules::{IgnoreRules, skip_ignore_files};
use crate::scope::Roots;
pub(crate) use crate::search::Exclude as Flags;
use crate::search::{Labels, link_inside, relative};

/// 걷기 스레드 수 상한(코어 수가 적으면 그만큼). `search.text` 와 같다.
const WALK_THREADS: usize = 8;

/// 바뀌면 그 폴더 아래의 판정이 달라지는 파일.
const RULE_FILES: [&str; 3] = [".gitignore", ".ignore", ".gitattributes"];

/// 목록에 할 일 하나.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Op {
    /// 그 상대 경로와 그 아래를 모두 지운다(빈 문자열이면 전부).
    Remove(String),
    Insert(Arc<str>, Flags),
}

/// 걷기 한 번(처음부터 걷기 · 변경 한 묶음)의 판정 재료. ignore 파일 · 속성 파일은 이 안에서 폴더마다 한 번 읽는다 — 묶음마다 새로
/// 만드므로 바뀐 규칙 파일을 다시 읽는다.
pub(crate) struct Walk<'a> {
    root: &'a Path,
    scope: &'a Roots,
    stop: &'a AtomicBool,
    rules: Arc<IgnoreRules>,
    labels: Labels,
}

impl<'a> Walk<'a> {
    pub(crate) fn new(root: &'a Path, scope: &'a Roots, stop: &'a AtomicBool) -> Self {
        Self {
            root,
            scope,
            stop,
            rules: Arc::new(IgnoreRules::new(scope.clone())),
            labels: Labels::default(),
        }
    }

    /// 루트 아래 파일 전부. 루트 자체가 연 때와 다른 실제 경로로 풀리면(루트를 지우고 같은 이름의 링크를 둠) 비어 있다 — 루트는
    /// 정규화한 실제 경로로 받는다([`crate::scope`]).
    pub(crate) fn all(&self) -> Vec<(Arc<str>, Flags)> {
        let mut out = Vec::new();
        if normalize(self.root).is_ok_and(|real| real == self.root) {
            self.under(&[self.root.to_path_buf()], &mut out);
        }
        out
    }

    /// 바뀐 경로들을 디스크와 맞출 일. 경로마다 그 자리와 그 아래를 지우고, 지금 디스크에 있고 목록에 들 것이면 다시 넣는다.
    /// 지우기를 모두 낸 뒤 넣기를 낸다.
    ///
    /// - 감시가 알린 경로라도 지금 디스크를 다시 보는 사이 중간 구성 요소가 링크로 바뀌었을 수 있다(`rm -rf d; ln -s <밖> d`). 넣기
    ///   전에 정규화한 실제 경로가 범위 안이고 `.git` 밖인지 다시 판정한다(결정 M1-D8) — 링크가 아닌 대상에도. 알림 순서와
    ///   상관없다(`d` 보다 `d/x` 가 늦게 와도 `d/x` 가 밖으로 풀린다).
    /// - 대소문자를 가리지 않는 볼륨(APFS 기본)에서는 옛 이름(`Foo.rs` → `foo.rs` 의 `Foo.rs`)도 열린다. 루트에서 그 경로까지 이름이
    ///   부모 폴더에 글자 그대로 있어야 넣는다.
    /// - 한 묶음에 폴더와 그 아래 경로가 함께 오면(폴더를 만들면 `d` · `d/e` · `d/e/f.rs`) 가장 바깥 것만 다시 걷는다. 다시 걸을
    ///   폴더들은 걷기 한 번으로 걷는다.
    pub(crate) fn ops(&self, paths: &[PathBuf]) -> Vec<Op> {
        let mut targets: Vec<&Path> = paths
            .iter()
            .map(|path| {
                let rule_file = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| RULE_FILES.contains(&n));
                match path.parent() {
                    Some(parent) if rule_file => parent,
                    _ => path.as_path(),
                }
            })
            .collect();
        // 구성 요소 단위 순서라 한 폴더 아래 경로는 그 폴더 바로 뒤에 모인다.
        targets.sort_unstable();
        targets.dedup();
        let mut outermost: Vec<&Path> = Vec::with_capacity(targets.len());
        for target in targets {
            if outermost.last().is_none_or(|top| !target.starts_with(top)) {
                outermost.push(target);
            }
        }

        let mut ops = Vec::new();
        let mut inserts = Vec::new();
        let mut dirs = Vec::new();
        let mut names = Names::default();
        for target in outermost {
            let Some(rel) = relative(target, self.root) else {
                continue;
            };
            ops.push(Op::Remove(rel));
            if !self.admitted(target)
                || !names.spelled(self.root, target)
                || !link_inside(self.scope, target)
            {
                continue;
            }
            // 링크면 대상(범위 안인 것은 위에서 봤다)의 종류다.
            let Ok(meta) = std::fs::metadata(target) else {
                continue;
            };
            if meta.is_dir() {
                dirs.push(target.to_path_buf());
            } else if meta.is_file()
                && let Some((rel, flags)) = self.item(target)
            {
                inserts.push(Op::Insert(rel, flags));
            }
        }
        let mut found = Vec::new();
        self.under(&dirs, &mut found);
        ops.extend(inserts);
        ops.extend(found.into_iter().map(|(rel, flags)| Op::Insert(rel, flags)));
        ops
    }

    /// [`Self::ops`] 를 바로 적용한다.
    pub(crate) fn refresh(&self, files: &mut BTreeMap<Arc<str>, Flags>, paths: &[PathBuf]) {
        apply(files, self.ops(paths));
    }

    /// 루트 안의 `path` 가 걷기로 닿는 자리인가: 루트에서 그 경로까지 `.git` 이 없고, 어느 단계도 ignore 되지 않았다. 링크는 보지
    /// 않는다([`Self::ops`] 가 따로 판정한다).
    fn admitted(&self, path: &Path) -> bool {
        let Ok(rel) = path.strip_prefix(self.root) else {
            return false;
        };
        let parts: Vec<_> = rel.components().collect();
        let mut at = self.root.to_path_buf();
        for (i, part) in parts.iter().enumerate() {
            if part.as_os_str() == ".git" {
                return false;
            }
            at.push(part);
            let is_dir = i + 1 < parts.len() || std::fs::metadata(&at).is_ok_and(|m| m.is_dir());
            if self.rules.ignored(&at, is_dir) {
                return false;
            }
        }
        true
    }

    /// 파일 하나의 항목. 루트 밖이면 `None`.
    fn item(&self, path: &Path) -> Option<(Arc<str>, Flags)> {
        let rel = relative(path, self.root)?;
        let flags = self.labels.classify(
            path,
            self.root,
            Flags {
                vendored: true,
                generated: true,
            },
        );
        Some((Arc::from(rel), flags))
    }

    /// `dirs` 아래 파일을 병렬로 걸어 `out` 에 더한다(순서는 정하지 않는다). 멈추라면 그때까지 찾은 것으로 끝낸다.
    ///
    /// 걷기는 시작 폴더를 거르지 않으므로(`filter_entry` 는 그 아래만 본다) 시작 폴더의 범위는 부르는 쪽이 판정한다([`Self::all`] ·
    /// [`Self::ops`]). 걷는 동안 바뀌었으면 그 폴더에서 찾은 것을 버린다. 그 아래가 걷는 사이 밖 링크로 바뀐 것은 그 바뀜의
    /// `fs.changed` 가 지운다.
    fn under(&self, dirs: &[PathBuf], out: &mut Vec<(Arc<str>, Flags)>) {
        let Some((first, rest)) = dirs.split_first() else {
            return;
        };
        let threads = std::thread::available_parallelism().map_or(1, |n| n.get().min(WALK_THREADS));
        let mut builder = WalkBuilder::new(first);
        for dir in rest {
            builder.add(dir);
        }
        // ignore 파일은 `IgnoreRules` 가 안전하게 읽는다(그 모듈 머리 주석). 숨김 파일도 목록에 든다.
        skip_ignore_files(&mut builder)
            .threads(threads)
            .hidden(false)
            .follow_links(true);
        let rules = Arc::clone(&self.rules);
        let scope = self.scope.clone();
        builder.filter_entry(move |entry| {
            if entry.file_name() == ".git" {
                return false;
            }
            if entry.path_is_symlink() && !link_inside(&scope, entry.path()) {
                return false;
            }
            let is_dir = entry.file_type().is_some_and(|t| t.is_dir());
            !rules.ignored(entry.path(), is_dir)
        });
        let found = Mutex::new(Vec::new());
        builder.build_parallel().run(|| {
            let mut batch = Batch {
                items: Vec::new(),
                found: &found,
            };
            Box::new(move |entry| {
                if self.stop.load(Ordering::Relaxed) {
                    return WalkState::Quit;
                }
                let Ok(entry) = entry else {
                    return WalkState::Continue;
                };
                if entry.file_type().is_some_and(|t| t.is_file())
                    && let Some(item) = self.item(entry.path())
                {
                    batch.items.push(item);
                }
                WalkState::Continue
            })
        });
        let mut found = found.into_inner().unwrap_or_else(PoisonError::into_inner);
        let moved: Vec<String> = dirs
            .iter()
            .filter(|dir| !link_inside(self.scope, dir))
            .filter_map(|dir| relative(dir, self.root))
            .collect();
        if !moved.is_empty() {
            found.retain(|(rel, _)| !moved.iter().any(|m| under_rel(rel, m)));
        }
        out.append(&mut found);
    }
}

/// 상대 경로 `rel` 이 `dir` 이거나 그 아래다(빈 `dir` 은 루트).
fn under_rel(rel: &str, dir: &str) -> bool {
    dir.is_empty()
        || rel
            .strip_prefix(dir)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

/// 폴더마다 한 번 읽은 이름들(바뀐 경로 한 묶음 동안).
#[derive(Default)]
struct Names(HashMap<PathBuf, Option<HashSet<OsString>>>);

impl Names {
    /// 루트에서 `path` 까지 구성 요소마다 그 이름이 부모 폴더에 글자 그대로 있다.
    fn spelled(&mut self, root: &Path, path: &Path) -> bool {
        let Ok(rel) = path.strip_prefix(root) else {
            return false;
        };
        let mut parent = root.to_path_buf();
        for part in rel.components() {
            let listed = self.0.entry(parent.clone()).or_insert_with(|| {
                std::fs::read_dir(&parent)
                    .ok()
                    .map(|dir| dir.filter_map(|e| e.ok().map(|e| e.file_name())).collect())
            });
            if !listed
                .as_ref()
                .is_some_and(|names| names.contains(part.as_os_str()))
            {
                return false;
            }
            parent.push(part);
        }
        true
    }
}

/// 걷기 스레드 하나가 모은 것. 스레드가 끝나 버려질 때 한 번에 넘긴다(항목마다 잠그지 않게).
struct Batch<'f> {
    items: Vec<(Arc<str>, Flags)>,
    found: &'f Mutex<Vec<(Arc<str>, Flags)>>,
}

impl Drop for Batch<'_> {
    fn drop(&mut self) {
        self.found
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .append(&mut self.items);
    }
}

/// 할 일을 차례로 적용한다.
pub(crate) fn apply(files: &mut BTreeMap<Arc<str>, Flags>, ops: Vec<Op>) {
    for op in ops {
        match op {
            Op::Remove(rel) if rel.is_empty() => files.clear(),
            Op::Remove(rel) => {
                files.remove(rel.as_str());
                // `rel/` 로 시작하는 것: `'0'` 은 `'/'` 바로 다음 바이트다.
                let (lo, hi) = (format!("{rel}/"), format!("{rel}0"));
                let under: Vec<Arc<str>> = files
                    .range::<str, _>((Bound::Included(lo.as_str()), Bound::Excluded(hi.as_str())))
                    .map(|(k, _)| Arc::clone(k))
                    .collect();
                for k in under {
                    files.remove(&k);
                }
            }
            Op::Insert(rel, flags) => {
                files.insert(rel, flags);
            }
        }
    }
}
