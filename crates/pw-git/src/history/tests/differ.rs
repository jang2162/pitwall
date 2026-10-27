//! 차등 판정: 저장소 상태 하나에서 미리보기(hard · keep · mixed)를 읽고, 저장소의 복사본에서 진짜 `git reset` 을 돌려 사람의 내용 중 무엇이
//! 사라지거나 바뀌었는지 미리보기 목록과 대조한다. 하나라도 목록에 없으면 놓친 것이다.
//!
//! 「사람의 내용」 은 작업 트리(꼭대기 `.git` 밖 — 중첩 저장소 · 서브모듈 안도)의 파일 · 링크와 index 에만 있는(디스크와 다른)
//! 스테이징 내용이다. 리셋 전 HEAD 의 같은 경로에 같은 내용으로 있는 것은 되찾을 수 있어(되돌리기의 `previous`) 빼고, 빈 blob 의
//! 스테이징(`git add -N`)도 뺀다. 실행 비트는 내용으로 보지 않는다. mixed 는 작업 트리를 건드리지 않으므로 index 에만 있던 내용
//! 중 reset 뒤 index(대상의 같은 경로)에도 디스크에도 없는 것이 잃은 것이다. 대소문자를 무시하는 파일 시스템이면 경로를 ASCII 소문자로 견준다.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::super::ResetMode;
use super::super::lost::{self, GitLostChanges};
use crate::testing::repo::TestRepo;

#[derive(Debug, Clone, PartialEq, Eq)]
enum Node {
    File { exec: bool, data: Vec<u8> },
    Link(Vec<u8>),
}

impl Node {
    fn blob(&self) -> gix::ObjectId {
        let data = match self {
            Self::File { data, .. } | Self::Link(data) => data,
        };
        gix::objs::compute_hash(gix::hash::Kind::Sha1, gix::objs::Kind::Blob, data).unwrap()
    }

    fn is_link(&self) -> bool {
        matches!(self, Self::Link(_))
    }
}

/// 한 방식의 판정. git 은 두 번 돈다 — 복사한 그대로(index 의 stat 이 모두 낡았다)와 `update-index --refresh` 뒤(사람이
/// `git status` 를 돌린 뒤처럼). 둘이 덮는 것이 다를 수 있다(sparse 의 콘 밖 파일, keep 의 거절).
#[derive(Debug)]
pub(super) struct Side {
    pub mode: ResetMode,
    pub preview: GitLostChanges,
    /// 미리보기가 오류였다(그러면 reset 도 하지 않는다 — hard 는 지문이 없고 keep 은 잠금 안의 계산이 같은 오류다).
    pub refused: Option<String>,
    /// git 이 성공했는가(keep 은 거절할 수 있다). 두 번 중 하나라도.
    pub succeeded: bool,
    /// git 이 실제로 잃은 사람의 내용(경로, 두 번의 합).
    pub lost: Vec<String>,
    /// 그중 목록에 없는 것.
    pub missed: Vec<String>,
}

impl Side {
    /// 목록에 있지만 잃지 않은 항목 수(넉넉히 센 것 — 실패는 아니다).
    pub fn extra(&self) -> usize {
        let fold = Fold::detect_cached();
        let lost: Vec<String> = self.lost.iter().map(|q| fold.key(q)).collect();
        let covers = |entry: &str| {
            let e = fold.key(entry);
            lost.iter()
                .any(|q| *q == e || (e.ends_with('/') && q.starts_with(&e)))
        };
        self.preview
            .changes
            .iter()
            .map(|c| c.path.as_str())
            .chain(self.preview.overwritten.iter().map(String::as_str))
            .filter(|entry| !covers(entry))
            .count()
    }
}

#[derive(Debug)]
pub(super) struct Verdict {
    pub hard: Side,
    pub keep: Side,
    pub mixed: Side,
}

impl Verdict {
    /// 놓친 것이 없으면 `Ok`, 있으면 사람이 읽을 설명.
    pub fn complete(&self) -> Result<(), String> {
        let mut out = String::new();
        for side in self.sides() {
            if !side.missed.is_empty() {
                out.push_str(&format!(
                    "{:?} reset (git {}) lost {:?} but the preview misses {:?}; preview = {:?}\n",
                    side.mode,
                    if side.succeeded { "ran" } else { "refused" },
                    side.lost,
                    side.missed,
                    side.preview,
                ));
            }
        }
        if out.is_empty() { Ok(()) } else { Err(out) }
    }

    pub fn sides(&self) -> [&Side; 3] {
        [&self.hard, &self.keep, &self.mixed]
    }

    /// [`Self::complete`] 가 아니면 패닉.
    pub fn assert_complete(&self) {
        if let Err(e) = self.complete() {
            panic!("{e}");
        }
    }
}

/// `repo` 의 지금 상태에서 `target` 으로 reset hard · keep · mixed 하면 잃는 것과 미리보기를 대조한다. `repo` 는 그대로다(복사본에서 돈다).
pub(super) fn check(repo: &TestRepo, target: &str) -> Verdict {
    let opened = open(repo.root());
    let side = |mode: ResetMode, name: &str| {
        let read = match lost::lost(&opened, target, mode, None) {
            Ok(read) => read,
            Err(e) => {
                return Side {
                    mode,
                    preview: GitLostChanges::default(),
                    refused: Some(format!("{e:?}")),
                    succeeded: false,
                    lost: Vec::new(),
                    missed: Vec::new(),
                };
            }
        };
        let flag = match mode {
            ResetMode::Hard => "--hard",
            ResetMode::Mixed => "--mixed",
            _ => "--keep",
        };
        let mut lost = Vec::new();
        let mut succeeded = false;
        for refresh in [false, true] {
            let copy = repo.base().join(format!("{name}-{refresh}"));
            let _ = std::fs::remove_dir_all(&copy);
            let out = std::process::Command::new("cp")
                .arg("-Rp")
                .arg(repo.root())
                .arg(&copy)
                .output()
                .unwrap();
            assert!(out.status.success(), "cp: {out:?}");
            if refresh {
                repo.try_git_in(&copy, &["update-index", "-q", "--refresh"]);
            }
            let before = State::read(repo, &copy);
            let run = repo.try_git_in(
                &copy,
                &["reset", "-q", flag, "--no-recurse-submodules", target, "--"],
            );
            succeeded |= run.status.success();
            let after = State::read(repo, &copy);
            let _ = std::fs::remove_dir_all(&copy);
            lost.extend(before.lost_in(&after));
        }
        lost.sort();
        lost.dedup();
        let fold = Fold::detect_cached();
        let missed = lost
            .iter()
            .filter(|q| !covered(&fold, &read.lost, q))
            .cloned()
            .collect();
        Side {
            mode,
            preview: read.lost,
            refused: None,
            succeeded,
            lost,
            missed,
        }
    };
    Verdict {
        hard: side(ResetMode::Hard, "copy-hard"),
        keep: side(ResetMode::Keep, "copy-keep"),
        mixed: side(ResetMode::Mixed, "copy-mixed"),
    }
}

/// 신뢰한 프로젝트로 연 저장소(`crate::GitRepo::open` 과 같은 열기).
pub(super) fn open(root: &Path) -> crate::Opened {
    let repo = crate::repo::discover(root, true).unwrap().unwrap();
    crate::Opened {
        target: crate::Target {
            project_id: "differ".into(),
            project_root: root.to_path_buf(),
            work_tree: root.to_path_buf(),
        },
        repo,
        trusted: true,
    }
}

fn covered(fold: &Fold, preview: &GitLostChanges, q: &str) -> bool {
    let q = fold.key(q);
    preview.changes.iter().any(|c| fold.key(&c.path) == q)
        || preview.overwritten.iter().any(|o| {
            let o = fold.key(o);
            o == q || (o.ends_with('/') && q.starts_with(&o))
        })
}

/// 대소문자를 무시하는 파일 시스템이면 ASCII 소문자로 견준다.
#[derive(Clone, Copy)]
struct Fold(bool);

impl Fold {
    fn detect_cached() -> Self {
        static FOLD: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        Self(*FOLD.get_or_init(|| {
            let dir = tempfile::tempdir().unwrap();
            std::fs::write(dir.path().join("CaseProbe"), "").unwrap();
            dir.path().join("caseprobe").exists()
        }))
    }

    fn key(self, path: &str) -> String {
        if self.0 {
            path.to_ascii_lowercase()
        } else {
            path.to_owned()
        }
    }
}

/// 복사본 하나의 작업 트리 · index · HEAD 트리.
struct State {
    disk: BTreeMap<String, (String, Node)>,
    /// 단계 0 의 index 항목: 경로(접은 키) → (mode, 객체).
    index: BTreeMap<String, (String, gix::ObjectId)>,
    head: BTreeMap<String, (String, gix::ObjectId)>,
}

impl State {
    fn read(repo: &TestRepo, root: &Path) -> Self {
        let fold = Fold::detect_cached();
        let mut disk = BTreeMap::new();
        walk(root, root, &mut |rel, node| {
            disk.insert(fold.key(&rel), (rel, node));
        });
        let parse = |out: &[u8], tree: bool| {
            let mut map = BTreeMap::new();
            for rec in out.split(|b| *b == 0).filter(|r| !r.is_empty()) {
                let rec = String::from_utf8_lossy(rec);
                let (meta, path) = rec.split_once('\t').unwrap();
                let parts: Vec<&str> = meta.split(' ').collect();
                let (mode, oid, stage) = if tree {
                    (parts[0], parts[2], "0")
                } else {
                    (parts[0], parts[1], parts[2])
                };
                if stage == "0" {
                    map.insert(
                        fold.key(path),
                        (
                            mode.to_owned(),
                            gix::ObjectId::from_hex(oid.as_bytes()).unwrap(),
                        ),
                    );
                }
            }
            map
        };
        let index = parse(
            &repo.try_git_in(root, &["ls-files", "-s", "-z"]).stdout,
            false,
        );
        let head_out = repo.try_git_in(root, &["ls-tree", "-r", "-z", "--full-tree", "HEAD"]);
        let head = if head_out.status.success() {
            parse(&head_out.stdout, true)
        } else {
            BTreeMap::new()
        };
        Self { disk, index, head }
    }

    fn recoverable(&self, key: &str, node: &Node) -> bool {
        self.head
            .get(key)
            .is_some_and(|(mode, oid)| *oid == node.blob() && (mode == "120000") == node.is_link())
    }

    /// `self`(전) → `after`(후)에서 잃은 사람의 내용.
    fn lost_in(&self, after: &State) -> Vec<String> {
        let empty = gix::ObjectId::empty_blob(gix::hash::Kind::Sha1);
        let mut lost = Vec::new();
        for (key, (rel, node)) in &self.disk {
            let same = after.disk.get(key).is_some_and(|(_, n)| {
                n == node
                    || matches!((n, node), (Node::File { data: a, .. }, Node::File { data: b, .. }) if a == b)
            });
            if !same && !self.recoverable(key, node) {
                lost.push(rel.clone());
            }
        }
        for (key, (mode, oid)) in &self.index {
            if mode == "160000" || *oid == empty {
                continue;
            }
            if self.head.get(key).is_some_and(|(_, h)| h == oid) {
                continue;
            }
            // 디스크에 같은 내용이 있으면 위에서 본다.
            if self.disk.get(key).is_some_and(|(_, n)| n.blob() == *oid) {
                continue;
            }
            let kept = after.index.get(key).is_some_and(|(_, o)| o == oid)
                || after.disk.get(key).is_some_and(|(_, n)| n.blob() == *oid);
            if !kept {
                let rel = self
                    .disk
                    .get(key)
                    .map_or_else(|| key.clone(), |(rel, _)| rel.clone());
                lost.push(rel);
            }
        }
        lost.sort();
        lost.dedup();
        lost
    }
}

fn walk(root: &Path, dir: &Path, out: &mut dyn FnMut(String, Node)) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path: PathBuf = entry.path();
        if dir == root && entry.file_name() == ".git" {
            continue;
        }
        let rel = path
            .strip_prefix(root)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.file_type().is_symlink() {
            let to = std::fs::read_link(&path).unwrap();
            out(rel, Node::Link(to.as_os_str().as_encoded_bytes().to_vec()));
        } else if meta.is_dir() {
            walk(root, &path, out);
        } else {
            use std::os::unix::fs::PermissionsExt;
            out(
                rel,
                Node::File {
                    exec: meta.permissions().mode() & 0o111 != 0,
                    data: std::fs::read(&path).unwrap_or_default(),
                },
            );
        }
    }
}
