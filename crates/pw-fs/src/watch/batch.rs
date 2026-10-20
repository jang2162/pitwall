//! 디바운스 창 하나 동안 받은 OS 이벤트를 경로별 변경으로 모은다.
//!
//! 종류는 창이 끝날 때 디스크를 보고 정한다. OS 가 주는 종류는 플랫폼마다 다르고(FSEvents 는 이름 바꾸기의 옛 이름 · 새 이름을 같은
//! 종류로 준다) 같은 창 안에서 여러 번 바뀌기 때문이다.
//!
//! - 지금 없으면 `removed`. 단 이 창 안에서 처음 생긴 경로면 빼낸다 — 에이전트식 쓰기(임시 파일 → rename)의 임시 파일은 생겼다
//!   사라진 것이라 알리지 않는다. 그래서 그 쓰기는 대상 경로의 변경 하나로 보인다. OS 의 「생성」 이 정확할 때만 뺀다
//!   ([`super::engine::CREATIONS_ARE_EXACT`]) — macOS 에서는 임시 파일 경로가 `removed` 로 함께 온다(대상 경로는 여전히 하나다).
//! - 지금 있고 이 창의 첫 이벤트가 생성이면 `created`, 아니면 `modified`. rename 으로 들어온 경로(원자 쓰기의 대상 · 이름 바꾸기의
//!   새 이름)는 전에 있었는지 알 수 없어 `modified` 다 — `created` · `modified` 는 힌트이고, 확실한 것은 「있다 / 없다」 다.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

use notify::EventKind;

use super::{FsChangeKind, FsChanged};

/// 창 하나에 담는 경로의 상한. 넘으면 경로를 버리고 `rescan` 으로 알린다(브랜치 전환 등 — 하나씩 반영하는 것보다 다시 읽는 편이
/// 싸고, 이벤트 하나가 수 MB 가 되지 않게).
pub const MAX_PATHS: usize = 4096;

pub struct Batch<'a> {
    root: &'a Path,
    /// 경로 → 이 창에서 처음 본 이벤트가 생성이었다.
    seen: HashMap<PathBuf, bool>,
    rescan: bool,
    max: usize,
    /// 「창 안에서 생겼다 사라진 경로」 를 뺀다.
    drop_transient: bool,
}

impl<'a> Batch<'a> {
    pub fn new(root: &'a Path, max: usize, drop_transient: bool) -> Self {
        Self {
            root,
            seen: HashMap::new(),
            rescan: false,
            max,
            drop_transient,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.seen.is_empty() && !self.rescan
    }

    /// 루트 안 · `.git/` 밖의 경로만 받는다. 읽기(`Access`)는 바뀐 것이 아니라 버린다 — 창이 파일을 읽을 때마다(inotify 의
    /// `IN_CLOSE_NOWRITE` 등) 다시 읽게 하는 고리가 생긴다. 다시 읽으라는 플래그(FSEvents 의 `MustScanSubDirs` 등)도 경로가
    /// 루트(자신 포함) 안 · `.git/` 밖일 때만 따르고, 경로가 없으면(큐 넘침) 늘 따른다 — `git gc` 가 `.git/objects` 에 붙인 플래그로
    /// 루트 전체를 다시 읽지 않게.
    pub fn add(&mut self, event: &notify::Event) {
        if event.need_rescan()
            && (event.paths.is_empty() || event.paths.iter().any(|p| self.covers(p)))
        {
            self.rescan = true;
        }
        if matches!(event.kind, EventKind::Access(_)) {
            return;
        }
        let born = matches!(event.kind, EventKind::Create(_));
        self.insert(event.paths.iter().cloned(), born);
    }

    /// OS 이벤트 없이 안 경로. `born` 이면 이 창에서 생긴 것으로 본다(새 폴더에 감시가 붙기 전에 생긴 항목), 아니면 바뀐 것이다
    /// (ignore 된 폴더 안의 변경을 접은 그 폴더).
    pub fn insert(&mut self, paths: impl IntoIterator<Item = PathBuf>, born: bool) {
        for path in paths {
            if !self.inside(&path) {
                continue;
            }
            if self.seen.len() >= self.max && !self.seen.contains_key(&path) {
                self.rescan = true;
                continue;
            }
            self.seen.entry(path).or_insert(born);
        }
    }

    /// 감시 자체의 오류(큐 넘침 등). 놓친 변경이 있을 수 있다.
    pub fn lost(&mut self) {
        self.rescan = true;
    }

    /// `.git/` 안(M2 의 몫)과 루트 밖(루트 밖을 가리키는 링크를 따라간 경로)은 알리지 않는다. 루트 자체도 — 그 안의 항목이 바뀌면
    /// FSEvents 가 루트를 함께 알리는데, 그것은 바뀐 경로가 아니다.
    fn inside(&self, path: &Path) -> bool {
        path != self.root && self.covers(path)
    }

    /// 루트(자신 포함) 아래이고 `.git/` 밖이다.
    fn covers(&self, path: &Path) -> bool {
        path.strip_prefix(self.root).is_ok_and(|rel| {
            !rel.components()
                .any(|c| c == Component::Normal(".git".as_ref()))
        })
    }

    /// 창을 닫는다. 알릴 것이 없으면 `None`. 디스크를 경로마다 한 번 본다.
    pub fn finish(self) -> Option<(Vec<PathBuf>, Vec<FsChangeKind>, bool)> {
        if self.rescan {
            return Some((Vec::new(), Vec::new(), true));
        }
        let mut seen: Vec<(PathBuf, bool)> = self.seen.into_iter().collect();
        seen.sort();
        let mut paths = Vec::with_capacity(seen.len());
        let mut kinds = Vec::with_capacity(seen.len());
        for (path, born) in seen {
            let kind = match (std::fs::symlink_metadata(&path).is_ok(), born) {
                (false, true) if self.drop_transient => continue,
                (false, _) => FsChangeKind::Removed,
                (true, true) => FsChangeKind::Created,
                (true, false) => FsChangeKind::Modified,
            };
            paths.push(path);
            kinds.push(kind);
        }
        (!paths.is_empty()).then_some((paths, kinds, false))
    }
}

impl FsChanged {
    pub(crate) fn from_parts(
        root: &Path,
        worktree: Option<&str>,
        (paths, kinds, rescan): (Vec<PathBuf>, Vec<FsChangeKind>, bool),
    ) -> Self {
        Self {
            root: root.display().to_string(),
            worktree: worktree.map(str::to_owned),
            paths: paths.iter().map(|p| p.display().to_string()).collect(),
            kinds,
            rescan,
        }
    }
}

#[cfg(test)]
mod tests {
    use notify::event::{CreateKind, DataChange, ModifyKind, RemoveKind, RenameMode};
    use notify::{Event, EventKind};

    use super::*;

    fn event(kind: EventKind, paths: &[&Path]) -> Event {
        let mut e = Event::new(kind);
        for p in paths {
            e = e.add_path(p.to_path_buf());
        }
        e
    }

    fn real_tmp() -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let real = std::fs::canonicalize(tmp.path()).unwrap();
        (tmp, real)
    }

    #[test]
    fn kinds_come_from_the_disk_when_the_window_closes() {
        let (_tmp, root) = real_tmp();
        let (new, kept, gone, tmp) = (
            root.join("new.txt"),
            root.join("kept.txt"),
            root.join("gone.txt"),
            root.join(".kept.txt.tmp"),
        );
        std::fs::write(&new, "n").unwrap();
        std::fs::write(&kept, "k").unwrap();
        let mut batch = Batch::new(&root, MAX_PATHS, true);
        batch.add(&event(EventKind::Create(CreateKind::File), &[&new]));
        // 원자 쓰기: 임시 파일이 생기고 → 대상으로 rename. 임시 파일은 이 창에서 생겼다 사라졌다.
        batch.add(&event(EventKind::Create(CreateKind::File), &[&tmp]));
        batch.add(&event(
            EventKind::Modify(ModifyKind::Name(RenameMode::Both)),
            &[&tmp, &kept],
        ));
        batch.add(&event(EventKind::Remove(RemoveKind::File), &[&gone]));
        // 같은 경로가 여러 번 와도 하나다. 처음 본 이벤트가 종류를 정한다.
        batch.add(&event(
            EventKind::Modify(ModifyKind::Data(DataChange::Content)),
            &[&new],
        ));
        let (paths, kinds, rescan) = batch.finish().unwrap();
        assert!(!rescan);
        assert_eq!(paths, vec![gone, kept, new]);
        assert_eq!(
            kinds,
            vec![
                FsChangeKind::Removed,
                FsChangeKind::Modified,
                FsChangeKind::Created
            ]
        );
    }

    /// 「생성」 을 믿을 수 없는 OS(FSEvents)에서는 창 안에서 생겼다 사라진 경로도 `removed` 로 남긴다 — 방금 만든 파일의 진짜
    /// 삭제를 잃지 않게.
    #[test]
    fn transient_paths_stay_when_creations_are_not_exact() {
        let (_tmp, root) = real_tmp();
        let gone = root.join("recent.txt");
        let mut batch = Batch::new(&root, MAX_PATHS, false);
        batch.add(&event(EventKind::Create(CreateKind::File), &[&gone]));
        batch.add(&event(EventKind::Remove(RemoveKind::File), &[&gone]));
        assert_eq!(
            batch.finish(),
            Some((vec![gone], vec![FsChangeKind::Removed], false))
        );
    }

    #[test]
    fn git_internals_reads_the_root_and_outside_paths_are_dropped() {
        let (_tmp, root) = real_tmp();
        let outside = root.parent().unwrap().join("elsewhere.txt");
        let mut batch = Batch::new(&root, MAX_PATHS, true);
        let modify = EventKind::Modify(ModifyKind::Data(DataChange::Content));
        batch.add(&event(modify, &[&root.join(".git/index")]));
        batch.add(&event(modify, &[&root.join("sub/.git/HEAD")]));
        batch.add(&event(modify, &[&root]));
        batch.add(&event(modify, &[&outside]));
        batch.add(&event(
            EventKind::Access(notify::event::AccessKind::Read),
            &[&root.join("read.txt")],
        ));
        assert!(batch.is_empty());
        assert!(batch.finish().is_none());
        // `.gitignore` · `.github` 은 `.git` 이 아니다.
        let mut batch = Batch::new(&root, MAX_PATHS, true);
        batch.add(&event(
            EventKind::Remove(RemoveKind::File),
            &[&root.join(".gitignore")],
        ));
        assert_eq!(batch.finish().unwrap().0, vec![root.join(".gitignore")]);
    }

    #[test]
    fn overflow_and_too_many_paths_become_a_rescan() {
        let (_tmp, root) = real_tmp();
        let mut batch = Batch::new(&root, 2, true);
        for name in ["a", "b", "c"] {
            batch.add(&event(
                EventKind::Remove(RemoveKind::File),
                &[&root.join(name)],
            ));
        }
        assert_eq!(batch.finish(), Some((Vec::new(), Vec::new(), true)));

        let mut batch = Batch::new(&root, MAX_PATHS, true);
        batch.add(&Event::new(EventKind::Other).set_flag(notify::event::Flag::Rescan));
        assert_eq!(batch.finish(), Some((Vec::new(), Vec::new(), true)));

        let mut batch = Batch::new(&root, MAX_PATHS, true);
        batch.lost();
        assert_eq!(batch.finish(), Some((Vec::new(), Vec::new(), true)));
    }

    /// 다시 읽으라는 플래그는 루트(자신 포함) 안 · `.git/` 밖 경로에 붙었을 때만 따른다 — `git gc` 가 `.git/objects` 에 붙인
    /// 플래그로 루트 전체를 다시 읽지 않는다.
    #[test]
    fn rescan_flags_on_git_internals_and_outside_paths_are_ignored() {
        let (_tmp, root) = real_tmp();
        let flagged =
            |p: &Path| event(EventKind::Other, &[p]).set_flag(notify::event::Flag::Rescan);
        for ignored in [
            root.join(".git/objects"),
            root.join("sub/.git"),
            root.parent().unwrap().join("elsewhere"),
        ] {
            let mut batch = Batch::new(&root, MAX_PATHS, true);
            batch.add(&flagged(&ignored));
            assert_eq!(batch.finish(), None, "{}", ignored.display());
        }
        for followed in [root.clone(), root.join("src")] {
            let mut batch = Batch::new(&root, MAX_PATHS, true);
            batch.add(&flagged(&followed));
            assert_eq!(
                batch.finish(),
                Some((Vec::new(), Vec::new(), true)),
                "{}",
                followed.display()
            );
        }
    }
}
