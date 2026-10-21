//! 목록을 걷고, 바뀐 경로를 디스크와 맞춘다. 규칙은 [`super`] 머리 주석.

use std::collections::BTreeMap;
use std::ops::Bound;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use ignore::{WalkBuilder, WalkState};

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

    /// 루트 아래 파일 전부.
    pub(crate) fn all(&self) -> Vec<(Arc<str>, Flags)> {
        let mut out = Vec::new();
        self.under(self.root, &mut out);
        out
    }

    /// 바뀐 경로들을 디스크와 맞출 일. 경로마다 그 자리와 그 아래를 지우고, 지금 디스크에 있고 목록에 들 것이면 다시 넣는다.
    pub(crate) fn ops(&self, paths: &[PathBuf]) -> Vec<Op> {
        let mut ops = Vec::new();
        for path in paths {
            let rule_file = path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| RULE_FILES.contains(&n));
            let target = match path.parent() {
                Some(parent) if rule_file => parent,
                _ => path.as_path(),
            };
            let Some(rel) = relative(target, self.root) else {
                continue;
            };
            ops.push(Op::Remove(rel));
            if !self.admitted(target) {
                continue;
            }
            let Ok(meta) = std::fs::symlink_metadata(target) else {
                continue;
            };
            let meta = if meta.is_symlink() {
                if !link_inside(self.scope, target) {
                    continue;
                }
                match std::fs::metadata(target) {
                    Ok(m) => m,
                    Err(_) => continue,
                }
            } else {
                meta
            };
            if meta.is_dir() {
                let mut found = Vec::new();
                self.under(target, &mut found);
                ops.extend(found.into_iter().map(|(rel, flags)| Op::Insert(rel, flags)));
            } else if meta.is_file()
                && let Some((rel, flags)) = self.item(target)
            {
                ops.push(Op::Insert(rel, flags));
            }
        }
        ops
    }

    /// [`Self::ops`] 를 바로 적용한다.
    pub(crate) fn refresh(&self, files: &mut BTreeMap<Arc<str>, Flags>, paths: &[PathBuf]) {
        apply(files, self.ops(paths));
    }

    /// 루트 안의 `path` 가 걷기로 닿는 자리인가: 루트에서 그 경로까지 `.git` 이 없고, 어느 단계도 ignore 되지 않았다. 감시는 실제
    /// 경로로 알리므로 중간의 링크는 보지 않는다.
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

    /// `dir` 아래 파일을 병렬로 걸어 `out` 에 더한다(순서는 정하지 않는다). 멈추라면 그때까지 찾은 것으로 끝낸다.
    fn under(&self, dir: &Path, out: &mut Vec<(Arc<str>, Flags)>) {
        let threads = std::thread::available_parallelism().map_or(1, |n| n.get().min(WALK_THREADS));
        let mut builder = WalkBuilder::new(dir);
        // ignore 파일은 `IgnoreRules` 가 안전하게 읽는다(그 모듈 머리 주석). 숨김 파일도 목록에 든다.
        skip_ignore_files(&mut builder)
            .threads(threads)
            .hidden(false)
            .follow_links(true);
        let rules = Arc::clone(&self.rules);
        let scope = self.scope.clone();
        builder.filter_entry(move |entry| {
            if entry.depth() == 0 {
                return true;
            }
            if entry.file_name() == ".git" {
                return false;
            }
            if entry.path_is_symlink() && !link_inside(&scope, entry.path()) {
                return false;
            }
            let is_dir = entry.file_type().is_some_and(|t| t.is_dir());
            !rules.ignored(entry.path(), is_dir)
        });
        let found = Mutex::new(std::mem::take(out));
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
        *out = found.into_inner().unwrap_or_else(PoisonError::into_inner);
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
