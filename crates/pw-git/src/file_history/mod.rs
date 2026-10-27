//! 파일 히스토리(`docs/design/13-m2-plan.md` M2-05 — F5-16): 이름 바뀜을 따라가는 그 파일의 커밋 목록, 커밋마다 그때의 경로.
//!
//! `git log --follow --name-status <rev> -- <경로>` 와 같은 커밋을 같은 순서 · 같은 상태로 낸다. git 의 동작을 그대로 옮겼다:
//!
//! - 순서: git log 의 기본 순서다 — 시작 커밋에서 커미터 시각이 늦은 커밋부터 꺼내고, 꺼낸 커밋의 부모를 넣는다(시각이 같으면 먼저
//!   넣은 것부터). `--follow` 는 이력 단순화를 하지 않으므로 머지의 부모를 모두 걷는다.
//! - 보이는 커밋: 머지가 아닌 커밋 중 첫 부모(첫 커밋은 빈 트리)와 견줘 지금 따라가는 경로가 바뀐 것. 머지는 보이지 않는다(git
//!   log 는 `-m` 없이 머지의 diff 를 내지 않는다).
//! - 이름 바뀜: 그 경로가 새로 생긴 커밋이면 이름 바뀜 · 복사의 원본을 찾아([`similar`] — 부모의 모든 파일이 원본 후보인
//!   `--find-copies-harder`, git 의 비슷함 계산으로 50% 이상), 그 경로로 온 것이 있으면 `R`/`C` 로 보이고 **그 뒤로 걷는 모든 커밋**에서 원래
//!   경로를 따라간다(git 이 diff 의 경로를 한 번 바꾸면 다른 갈래의 커밋에도 그 경로를 쓴다 — 갈래가 섞인 이력에서 git 과 같은
//!   결과를 내려고 그대로 따른다).
//!
//! git 과 다를 수 있는 것: 점수 · 기본 이름까지 같은 원본 후보가 여럿일 때 고르는 차례(git 은 정렬이 안정적이지 않다), 원본 후보가
//! `diff.renameLimit` 의 제곱을 넘는 큰 트리(git 은 바뀐 파일로만 좁힌다), 빈 파일의 이름 바뀜(견주지 않는다). 시작 리비전에서
//! 폴더인 경로는 받지 않는다(git 은 그 아래 파일들을 따로 보인다).
//!
//! 읽기는 gix 의 객체 읽기뿐이라 저장소의 프로그램이 돌지 않는다 — 미신뢰 프로젝트도 같은 경로다.

pub mod methods;
pub mod similar;

#[cfg(test)]
mod tests;

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use gix::ObjectId;
use gix::bstr::{BStr, BString, ByteSlice};
use gix::objs::FindExt;
use gix::objs::tree::EntryKind;
use pw_rpc::RpcError;
use serde::{Deserialize, Serialize};

use crate::log::commit::{self, MAX_IDENT, Mailmap, Text, clip};

/// 첫 청크의 행 수 상한(화면 한 쪽).
pub const FIRST_CHUNK: usize = 200;
/// 그 뒤 청크의 행 수 상한.
pub const CHUNK: usize = 500;
/// 맞는 커밋이 드물 때, 모인 행을 이만큼 기다린 뒤 보낸다.
const FLUSH_AFTER: Duration = Duration::from_millis(100);

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum FileChangeKind {
    /// `A`.
    Added,
    /// `M`(내용이나 실행 비트).
    Modified,
    /// `D`.
    Deleted,
    /// `R` — `oldPath` 에서 이름이 바뀌었다.
    Renamed,
    /// `C` — `oldPath` 를 복사했다(원본은 남았다).
    Copied,
    /// `T` — 파일 · 심볼릭 링크 · 서브모듈 사이에서 종류가 바뀌었다.
    TypeChanged,
}

/// 파일 히스토리의 한 행.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct FileHistoryRow {
    /// 전체 해시(16진).
    pub hash: String,
    /// 부모 해시(커밋에 적힌 순서. 얕은 클론의 경계 커밋은 비어 있다).
    pub parents: Vec<String>,
    /// 제목(메시지의 첫 문단을 한 줄로).
    pub subject: String,
    /// 작성자(mailmap 적용).
    pub author_name: String,
    pub author_email: String,
    /// 작성 시각(유닉스 초).
    #[specta(type = pw_rpc::JsNumber)]
    pub author_time: i64,
    /// 이 커밋에서의 경로(저장소 꼭대기 기준, `/` 구분). 지운 커밋이면 지운 경로.
    pub path: String,
    pub change: FileChangeKind,
    /// 이름 바뀜 · 복사의 원래 경로(부모에서의 경로).
    pub old_path: Option<String>,
}

/// `git.fileHistory` 스트림의 항목 하나.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct FileHistoryChunk {
    pub rows: Vec<FileHistoryRow>,
}

fn internal(e: impl std::fmt::Display) -> RpcError {
    RpcError::internal(e.to_string())
}

/// 경로를 시작 커밋에서 따라갈 수 있는지 본다: 폴더면 invalid params. 없으면(지운 파일) 그대로 따라간다.
pub fn check_start(repo: &gix::Repository, start: ObjectId, path: &BStr) -> Result<(), RpcError> {
    let tree = repo
        .find_commit(start)
        .map_err(internal)?
        .tree()
        .map_err(internal)?;
    let entry = tree
        .lookup_entry_by_path(path.to_path_lossy())
        .map_err(internal)?;
    if entry.is_some_and(|e| e.mode().is_tree()) {
        return Err(RpcError::invalid_params(format!(
            "`{path}` is a directory in {start}"
        )));
    }
    Ok(())
}

/// 걷는 중인 커밋 하나.
struct Node {
    tree: ObjectId,
    parents: Vec<ObjectId>,
    time: i64,
}

/// 트리 안의 항목 하나.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Entry {
    id: ObjectId,
    kind: EntryKind,
}

/// 경로 하나의 바뀜(첫 부모 → 커밋).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Change {
    Added,
    Modified,
    Deleted,
    TypeChanged,
}

/// `start` 에서 `path` 의 히스토리를 만들어 청크로 `emit` 한다(첫 청크 [`FIRST_CHUNK`] 행, 다음은 [`CHUNK`] 행, 모인 행이 있으면
/// [`FLUSH_AFTER`] 마다). `emit` 이 `false` 를 돌려주면 멈추고 `Ok(false)`. `stop` 이 켜지면 `CANCELLED`. `trusted` 는 연
/// 저장소의 신뢰다(mailmap 설정을 볼지).
pub fn run(
    repo: &gix::Repository,
    trusted: bool,
    start: ObjectId,
    path: &BStr,
    stop: &AtomicBool,
    emit: &mut dyn FnMut(FileHistoryChunk) -> bool,
) -> Result<bool, RpcError> {
    let shallow: HashSet<ObjectId> = repo
        .shallow_commits()
        .map_err(internal)?
        .map(|commits| commits.iter().copied().collect())
        .unwrap_or_default();
    let mut walk = Walk {
        repo,
        shallow,
        nodes: HashMap::new(),
        queue: BinaryHeap::new(),
        seen: HashSet::new(),
        seq: 0,
        buf: Vec::new(),
    };
    walk.push(start)?;
    let mut path: BString = path.to_owned();
    let mut mailmap: Option<Mailmap> = None;
    let mut out = Chunker {
        rows: Vec::new(),
        sent: 0,
        since_flush: Instant::now(),
    };
    let (mut a, mut b, mut buf) = (Vec::new(), Vec::new(), Vec::new());
    let mut n = 0usize;
    while let Some(id) = walk.pop() {
        n += 1;
        if n.is_multiple_of(256) {
            if stop.load(Ordering::Relaxed) {
                return Err(RpcError::cancelled());
            }
            // 맞는 커밋이 드물어도 모인 행을 오래 쥐고 있지 않는다.
            if !out.rows.is_empty() && out.since_flush.elapsed() >= FLUSH_AFTER && !out.flush(emit)
            {
                return Ok(false);
            }
        }
        let node = walk.nodes.remove(&id).expect("queued commits are loaded");
        for &parent in &node.parents {
            walk.push(parent)?;
        }
        if node.parents.len() > 1 {
            continue;
        }
        let parent_tree = match node.parents.first() {
            Some(parent) => Some(walk.tree_of(*parent)?),
            None => None,
        };
        let Some(change) = change_at(repo, parent_tree, node.tree, path.as_bstr(), &mut a, &mut b)?
        else {
            continue;
        };
        let (kind, old_path) = match (change, parent_tree) {
            (Change::Added, Some(parent_tree)) => {
                match similar::follow(repo, parent_tree, node.tree, path.as_bstr())? {
                    Some((source, copy)) => {
                        let kind = if copy {
                            FileChangeKind::Copied
                        } else {
                            FileChangeKind::Renamed
                        };
                        (kind, Some(source))
                    }
                    None => (FileChangeKind::Added, None),
                }
            }
            (Change::Added, None) => (FileChangeKind::Added, None),
            (Change::Modified, _) => (FileChangeKind::Modified, None),
            (Change::Deleted, _) => (FileChangeKind::Deleted, None),
            (Change::TypeChanged, _) => (FileChangeKind::TypeChanged, None),
        };
        let found = repo.objects.find_commit(&id, &mut buf).map_err(internal)?;
        let mailmap = mailmap.get_or_insert_with(|| Mailmap::of(repo, trusted));
        let text = Text::read(&found, mailmap)?;
        let row = FileHistoryRow {
            hash: id.to_string(),
            parents: node.parents.iter().map(ToString::to_string).collect(),
            subject: commit::subject(&found),
            author_name: clip(&text.author_name, MAX_IDENT),
            author_email: clip(&text.author_email, MAX_IDENT),
            author_time: text.author_time,
            path: path.to_str_lossy().into_owned(),
            change: kind,
            old_path: old_path.as_ref().map(|p| p.to_str_lossy().into_owned()),
        };
        // 그 뒤로 걷는 커밋은 모두 원래 경로를 따라간다(머리 주석).
        if let Some(source) = old_path {
            path = source;
        }
        if !out.push(row, emit) {
            return Ok(false);
        }
    }
    Ok(out.finish(emit))
}

/// git log 의 기본 순서로 커밋을 꺼낸다(머리 주석).
struct Walk<'r> {
    repo: &'r gix::Repository,
    shallow: HashSet<ObjectId>,
    /// 줄에 들어 있는 커밋(꺼낼 때 지운다).
    nodes: HashMap<ObjectId, Node>,
    /// (커미터 시각, 넣은 차례의 역순, 커밋) — 시각이 늦은 것, 같으면 먼저 넣은 것이 먼저 나온다.
    queue: BinaryHeap<(i64, Reverse<u64>, ObjectId)>,
    seen: HashSet<ObjectId>,
    seq: u64,
    buf: Vec<u8>,
}

impl Walk<'_> {
    fn push(&mut self, id: ObjectId) -> Result<(), RpcError> {
        if !self.seen.insert(id) {
            return Ok(());
        }
        let node = self.load(id)?;
        self.queue.push((node.time, Reverse(self.seq), id));
        self.seq += 1;
        self.nodes.insert(id, node);
        Ok(())
    }

    fn pop(&mut self) -> Option<ObjectId> {
        self.queue.pop().map(|(_, _, id)| id)
    }

    fn load(&mut self, id: ObjectId) -> Result<Node, RpcError> {
        let commit = self
            .repo
            .objects
            .find_commit(&id, &mut self.buf)
            .map_err(internal)?;
        let parents = if self.shallow.contains(&id) {
            Vec::new()
        } else {
            commit.parents().collect()
        };
        Ok(Node {
            tree: commit.tree(),
            parents,
            time: commit.committer().map_err(internal)?.seconds(),
        })
    }

    /// 부모의 트리. 부모는 [`Self::push`] 로 줄에 넣은 뒤라 아직 꺼내지 않았으면 들고 있다.
    fn tree_of(&mut self, id: ObjectId) -> Result<ObjectId, RpcError> {
        match self.nodes.get(&id) {
            Some(node) => Ok(node.tree),
            None => Ok(self.load(id)?.tree),
        }
    }
}

/// `path` 가 `old`(없으면 빈 트리) → `new` 에서 어떻게 바뀌었나. 경로를 따라 내려가다 두 쪽의 하위 트리가 같으면 바로 「안
/// 바뀜」 이다.
fn change_at(
    repo: &gix::Repository,
    old: Option<ObjectId>,
    new: ObjectId,
    path: &BStr,
    a: &mut Vec<u8>,
    b: &mut Vec<u8>,
) -> Result<Option<Change>, RpcError> {
    let parts: Vec<&[u8]> = path.split(|&c| c == b'/').collect();
    let (mut old, mut new) = (old, Some(new));
    let mut found = (None, None);
    for (i, name) in parts.iter().enumerate() {
        if old == new {
            return Ok(None);
        }
        let last = i + 1 == parts.len();
        let old_entry = match old {
            Some(tree) => lookup(repo, tree, name, a)?,
            None => None,
        };
        let new_entry = match new {
            Some(tree) => lookup(repo, tree, name, b)?,
            None => None,
        };
        if last {
            found = (old_entry, new_entry);
            break;
        }
        let subtree = |e: Option<Entry>| e.filter(|e| e.kind == EntryKind::Tree).map(|e| e.id);
        old = subtree(old_entry);
        new = subtree(new_entry);
        if old.is_none() && new.is_none() {
            return Ok(None);
        }
    }
    Ok(match found {
        (None, None) => None,
        (None, Some(_)) => Some(Change::Added),
        (Some(_), None) => Some(Change::Deleted),
        (Some(o), Some(n)) if o == n => None,
        (Some(o), Some(n)) if family(o.kind) != family(n.kind) => Some(Change::TypeChanged),
        (Some(_), Some(_)) => Some(Change::Modified),
    })
}

/// 종류가 바뀌었는지 볼 때의 묶음: 실행 비트만 다른 파일은 같은 종류다(`M`).
fn family(kind: EntryKind) -> u8 {
    match kind {
        EntryKind::Blob | EntryKind::BlobExecutable => 0,
        EntryKind::Link => 1,
        EntryKind::Commit => 2,
        EntryKind::Tree => 3,
    }
}

/// 트리 `tree` 의 바로 아래 항목 `name`.
fn lookup(
    repo: &gix::Repository,
    tree: ObjectId,
    name: &[u8],
    buf: &mut Vec<u8>,
) -> Result<Option<Entry>, RpcError> {
    let iter = repo.objects.find_tree_iter(&tree, buf).map_err(internal)?;
    for entry in iter {
        let entry = entry.map_err(internal)?;
        if entry.filename == name {
            return Ok(Some(Entry {
                id: entry.oid.to_owned(),
                kind: entry.mode.kind(),
            }));
        }
    }
    Ok(None)
}

/// 행을 청크로 묶는다.
struct Chunker {
    rows: Vec<FileHistoryRow>,
    sent: usize,
    since_flush: Instant,
}

impl Chunker {
    fn push(
        &mut self,
        row: FileHistoryRow,
        emit: &mut dyn FnMut(FileHistoryChunk) -> bool,
    ) -> bool {
        self.rows.push(row);
        let size = if self.sent == 0 { FIRST_CHUNK } else { CHUNK };
        if self.rows.len() >= size || self.since_flush.elapsed() >= FLUSH_AFTER {
            return self.flush(emit);
        }
        true
    }

    fn flush(&mut self, emit: &mut dyn FnMut(FileHistoryChunk) -> bool) -> bool {
        self.sent += 1;
        self.since_flush = Instant::now();
        emit(FileHistoryChunk {
            rows: std::mem::take(&mut self.rows),
        })
    }

    fn finish(&mut self, emit: &mut dyn FnMut(FileHistoryChunk) -> bool) -> bool {
        self.rows.is_empty() || self.flush(emit)
    }
}
