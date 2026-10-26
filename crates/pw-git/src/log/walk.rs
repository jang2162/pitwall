//! 리비전 걷기와 순서. git 의 `--topo-order` · `--date-order` 와 같은 순서를 낸다(git `revision.c` 의
//! `sort_in_topological_order` — commit-graph 가 없을 때 git 이 쓰는 경로).
//!
//! 1. 시작 커밋(ref 끝)에서 닿는 커밋을 모두 모은다(부모 · 커미터 시각). commit-graph 가 있고 쓸 수 있으면 그것으로 읽는다(gix
//!    `revision_graph`). 얕은 클론의 경계 커밋은 부모가 없는 것으로 본다(git 과 같다).
//! 2. 순서: 자식 수(진입 차수)를 세고, 자식이 없는 시작 커밋을 「끝」 으로 놓는다. 끝의 순서는 커미터 시각이 늦은 것부터, 같으면
//!    시작 커밋을 받은 순서다. topo 는 스택(마지막에 넣은 것을 먼저 — 머지 다음에는 마지막 부모 쪽 줄이 먼저 나온다), date 는 시각이
//!    늦은 것부터(같으면 먼저 넣은 것). 커밋은 자식이 모두 나온 뒤에 나온다.
//!
//! 경로 필터가 있으면 git 의 기본 이력 단순화를 그대로 한다(`try_to_simplify_commit`): 경로의 항목이 부모와 같은(TREESAME) 커밋은
//! 숨기고, 머지가 어느 부모와 같으면 그 부모만 따라간다(다른 부모 쪽 이력은 걷지 않는다). 순서는 단순화한 부모로 정한다 — 그래서
//! 숨긴 커밋도 순서 계산에는 남는다.

use std::collections::{BinaryHeap, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};

use gix::ObjectId;
use gix::bstr::{BStr, BString, ByteSlice};
use gix::objs::FindExt;
use pw_rpc::RpcError;

use super::LogOrder;

/// 이 수만큼 커밋을 볼 때마다 멈춤 신호를 본다.
const STOP_EVERY: usize = 1024;

/// 모은 커밋 그래프. 번호는 처음 만난 차례다(0 부터 시작 커밋).
pub struct Dag {
    pub ids: Vec<ObjectId>,
    /// 커밋 `i` 의 부모는 `parents[start[i]..start[i + 1]]`.
    start: Vec<u32>,
    parents: Vec<u32>,
    /// 커미터 시각(초).
    pub time: Vec<i64>,
    /// 경로 필터에서 숨기는 커밋(TREESAME). 경로 필터가 없으면 비어 있다.
    hidden: Vec<bool>,
    /// 시작 커밋의 번호. 시각이 늦은 것부터, 같으면 받은 순서.
    pub starts: Vec<u32>,
}

impl Dag {
    pub fn len(&self) -> usize {
        self.ids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    pub fn parents(&self, i: u32) -> &[u32] {
        let i = i as usize;
        &self.parents[self.start[i] as usize..self.start[i + 1] as usize]
    }

    /// 다음 커밋(번호 `time.len()`)의 시각과 부모를 적는다. 처음 보는 부모는 `add` 가 번호를 준다.
    fn push(
        &mut self,
        time: i64,
        parents: &[ObjectId],
        add: &mut impl FnMut(&mut Dag, ObjectId) -> u32,
    ) {
        self.time.push(time);
        for &p in parents {
            let p = add(self, p);
            self.parents.push(p);
        }
        self.start.push(self.parents.len() as u32);
    }

    /// 경로 필터가 숨기는 커밋인가.
    pub fn hidden(&self, i: u32) -> bool {
        self.hidden.get(i as usize).copied().unwrap_or(false)
    }
}

fn internal(e: impl std::fmt::Display) -> RpcError {
    RpcError::internal(e.to_string())
}

fn stopped() -> RpcError {
    RpcError::cancelled()
}

/// 경로 필터의 항목: 경로마다 (모드, 객체) — 없으면 `None`. 커밋 둘의 항목이 같으면 그 경로들에서 TREESAME 이다.
type Entries = Vec<Option<(u16, ObjectId)>>;

/// `tips`(받은 순서, 겹쳐도 된다)에서 닿는 커밋을 모은다. `paths` 가 있으면 그 경로로 이력을 단순화한다.
pub fn collect(
    repo: &gix::Repository,
    tips: &[ObjectId],
    paths: &[BString],
    stop: &AtomicBool,
) -> Result<Dag, RpcError> {
    let shallow: Vec<ObjectId> = repo
        .shallow_commits()
        .map_err(internal)?
        .map(|c| c.iter().copied().collect())
        .unwrap_or_default();
    let mut dag = Dag {
        ids: Vec::new(),
        start: vec![0],
        parents: Vec::new(),
        time: Vec::new(),
        hidden: Vec::new(),
        starts: Vec::new(),
    };
    let mut index: gix::hashtable::HashMap<ObjectId, u32> = gix::hashtable::HashMap::default();
    let mut add = |dag: &mut Dag, id: ObjectId| -> u32 {
        *index.entry(id).or_insert_with(|| {
            dag.ids.push(id);
            (dag.ids.len() - 1) as u32
        })
    };
    let mut starts = Vec::new();
    for &tip in tips {
        let i = add(&mut dag, tip);
        if !starts.contains(&i) {
            starts.push(i);
        }
    }
    if paths.is_empty() {
        let cache = repo.commit_graph_if_enabled().map_err(internal)?;
        let helpers = helpers();
        // 대체 객체(`refs/replace`)가 있으면 저장소 핸들(그것을 따른다)로만 읽는다 — 보조 스레드는 객체 저장소만 연다.
        let replaced = repo.objects.store_ref().replacements().next().is_some();
        if cache.is_none() && helpers > 0 && !replaced {
            walk_prefetched(repo, &mut dag, &mut add, &shallow, helpers, stop)?;
        } else {
            let mut graph = repo.revision_graph::<()>(cache.as_ref());
            let mut parents = Vec::new();
            let mut i = 0;
            while i < dag.ids.len() {
                if i % STOP_EVERY == 0 && stop.load(Ordering::Relaxed) {
                    return Err(stopped());
                }
                let id = dag.ids[i];
                parents.clear();
                let time = {
                    let commit = graph.lookup(&id).map_err(internal)?;
                    if !shallow.contains(&id) {
                        for p in commit.iter_parents() {
                            parents.push(p.map_err(internal)?);
                        }
                    }
                    commit.committer_timestamp().map_err(internal)?
                };
                dag.push(time, &parents, &mut add);
                i += 1;
            }
        }
    } else {
        let mut entries: HashMap<ObjectId, Entries> = HashMap::new();
        let mut buf = Vec::new();
        let mut i = 0;
        while i < dag.ids.len() {
            if i % STOP_EVERY == 0 && stop.load(Ordering::Relaxed) {
                return Err(stopped());
            }
            let id = dag.ids[i];
            let (time, mut parents) = {
                let commit = repo.objects.find_commit(&id, &mut buf).map_err(internal)?;
                let time = commit.committer().map_err(internal)?.seconds();
                let parents: Vec<ObjectId> = if shallow.contains(&id) {
                    Vec::new()
                } else {
                    commit.parents().collect()
                };
                (time, parents)
            };
            let own = entries_of(repo, &mut entries, id, paths)?;
            let hidden = if parents.is_empty() {
                own.iter().all(Option::is_none)
            } else {
                let mut same = None;
                for &p in &parents {
                    if entries_of(repo, &mut entries, p, paths)? == own {
                        same = Some(p);
                        break;
                    }
                }
                if let Some(p) = same {
                    parents = vec![p];
                }
                same.is_some()
            };
            dag.hidden.push(hidden);
            dag.push(time, &parents, &mut add);
            i += 1;
        }
    }
    starts.sort_by_key(|&i| std::cmp::Reverse(dag.time[i as usize]));
    dag.starts = starts;
    Ok(dag)
}

/// commit-graph 가 없을 때 커밋 객체를 미리 읽을 보조 스레드 수. 코어가 하나면 0(혼자 읽는다).
fn helpers() -> usize {
    std::thread::available_parallelism()
        .map_or(1, std::num::NonZero::get)
        .saturating_sub(1)
        .min(MAX_HELPERS)
}

/// 보조 스레드 상한. 본 스레드가 행 차례대로 부모를 잇는 일이 병목이 되는 수다(합성 10만 커밋에서 잰다).
const MAX_HELPERS: usize = 7;

/// 보조 스레드가 한 번에 가져가는 커밋 수 상한.
const PREFETCH_BATCH: usize = 16;

/// 커밋 하나: 부모(얕은 경계면 비어 있다)와 커미터 시각.
struct Decoded {
    parents: Vec<ObjectId>,
    time: i64,
}

fn decode(
    objects: &impl gix::objs::Find,
    id: &ObjectId,
    shallow: &[ObjectId],
    buf: &mut Vec<u8>,
) -> Result<Decoded, String> {
    let commit = objects
        .find_commit_iter(id, buf)
        .map_err(|e| e.to_string())?;
    let parents = if shallow.contains(id) {
        Vec::new()
    } else {
        commit.parent_ids().collect()
    };
    let time = commit.committer().map_err(|e| e.to_string())?.seconds();
    Ok(Decoded { parents, time })
}

/// 보조 스레드에 넘길 커밋(번호, 해시)과 닫힘.
#[derive(Default)]
struct Pending {
    items: std::collections::VecDeque<(u32, ObjectId)>,
    closed: bool,
}

struct Prefetch {
    pending: std::sync::Mutex<Pending>,
    wake: std::sync::Condvar,
}

impl Prefetch {
    fn lock(&self) -> std::sync::MutexGuard<'_, Pending> {
        self.pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn close(&self) {
        self.lock().closed = true;
        self.wake.notify_all();
    }
}

/// 본 스레드가 닫지 못하고 나가도(오류 · 멈춤) 보조 스레드가 끝나게 한다.
struct CloseOnDrop<'a>(&'a Prefetch);

impl Drop for CloseOnDrop<'_> {
    fn drop(&mut self) {
        self.0.close();
    }
}

/// commit-graph 없이 걷는다. 커밋 객체 읽기(압축 해제)가 대부분의 시간이라, 번호가 붙는 대로 보조 스레드들이 미리 읽고 본
/// 스레드는 번호 차례대로 결과를 받아 부모를 잇는다 — 순서 · 번호는 혼자 걸을 때와 같다.
///
/// 보조 스레드는 저장소가 아니라 그 객체 폴더만 연다(`gix::odb::at` — 설정을 읽지 않는다). 이 크레이트가 켠 gix 기능으로는 저장소
/// 핸들을 스레드 사이에 넘길 수 없다.
fn walk_prefetched(
    repo: &gix::Repository,
    dag: &mut Dag,
    add: &mut impl FnMut(&mut Dag, ObjectId) -> u32,
    shallow: &[ObjectId],
    helpers: usize,
    stop: &AtomicBool,
) -> Result<(), RpcError> {
    let objects_dir = repo.objects.store_ref().path().to_path_buf();
    let hash = repo.object_hash();
    let prefetch = Prefetch {
        pending: std::sync::Mutex::new(Pending {
            items: dag
                .ids
                .iter()
                .enumerate()
                .map(|(i, id)| (i as u32, *id))
                .collect(),
            closed: false,
        }),
        wake: std::sync::Condvar::new(),
    };
    type Batch = Vec<(u32, Result<Decoded, String>)>;
    let (tx, rx) = std::sync::mpsc::channel::<Batch>();
    std::thread::scope(|scope| {
        for _ in 0..helpers {
            let (tx, objects_dir, prefetch) = (tx.clone(), objects_dir.clone(), &prefetch);
            scope.spawn(move || {
                let objects = match gix::odb::at(objects_dir, hash) {
                    Ok(objects) => objects,
                    Err(e) => {
                        // 본 스레드가 기다리지 않게 받을 것을 모두 실패로 돌려준다.
                        let _ = tx.send(vec![(u32::MAX, Err(e.to_string()))]);
                        return;
                    }
                };
                let mut buf = Vec::new();
                loop {
                    let take: Vec<(u32, ObjectId)> = {
                        let mut pending = prefetch.lock();
                        while pending.items.is_empty() && !pending.closed {
                            pending = prefetch
                                .wake
                                .wait(pending)
                                .unwrap_or_else(std::sync::PoisonError::into_inner);
                        }
                        if pending.items.is_empty() {
                            return;
                        }
                        let n = pending
                            .items
                            .len()
                            .div_ceil(helpers)
                            .clamp(1, PREFETCH_BATCH);
                        let n = n.min(pending.items.len());
                        pending.items.drain(..n).collect()
                    };
                    let batch: Batch = take
                        .into_iter()
                        .map(|(i, id)| (i, decode(&objects, &id, shallow, &mut buf)))
                        .collect();
                    if tx.send(batch).is_err() {
                        return;
                    }
                }
            });
        }
        drop(tx);
        let _close = CloseOnDrop(&prefetch);
        let mut ready: Vec<Option<Decoded>> = Vec::new();
        let mut i = 0;
        while i < dag.ids.len() {
            if i % STOP_EVERY == 0 && stop.load(Ordering::Relaxed) {
                return Err(stopped());
            }
            while ready.get(i).is_none_or(Option::is_none) {
                let batch = rx
                    .recv()
                    .map_err(|_| RpcError::internal("the commit readers stopped"))?;
                for (j, decoded) in batch {
                    let decoded = decoded.map_err(RpcError::internal)?;
                    let j = j as usize;
                    if ready.len() <= j {
                        ready.resize_with(j + 1, || None);
                    }
                    ready[j] = Some(decoded);
                }
            }
            let decoded = ready[i].take().expect("decoded above");
            let known = dag.ids.len();
            dag.push(decoded.time, &decoded.parents, add);
            if dag.ids.len() > known {
                let mut pending = prefetch.lock();
                pending
                    .items
                    .extend((known..dag.ids.len()).map(|j| (j as u32, dag.ids[j])));
                drop(pending);
                // 새 커밋 수만큼만 깨운다(모두 깨우면 쉬던 스레드가 빈 손으로 잠금을 다툰다).
                for _ in known..dag.ids.len() {
                    prefetch.wake.notify_one();
                }
            }
            i += 1;
        }
        Ok(())
    })
}

/// 커밋 `id` 의 경로 항목(캐시).
fn entries_of(
    repo: &gix::Repository,
    cache: &mut HashMap<ObjectId, Entries>,
    id: ObjectId,
    paths: &[BString],
) -> Result<Entries, RpcError> {
    if let Some(e) = cache.get(&id) {
        return Ok(e.clone());
    }
    let tree = repo
        .find_commit(id)
        .map_err(internal)?
        .tree()
        .map_err(internal)?;
    let mut entries = Vec::with_capacity(paths.len());
    for path in paths {
        let entry = tree
            .lookup_entry(path_components(path.as_bstr()))
            .map_err(internal)?;
        entries.push(entry.map(|e| (e.mode().value(), e.object_id())));
    }
    cache.insert(id, entries.clone());
    Ok(entries)
}

fn path_components(path: &BStr) -> impl Iterator<Item = &[u8]> {
    path.split(|&b| b == b'/')
}

/// 행 순서(번호 목록). 숨긴 커밋도 들어 있다 — 부른 쪽이 거른다.
pub fn order(dag: &Dag, order: LogOrder, stop: &AtomicBool) -> Result<Vec<u32>, RpcError> {
    let n = dag.len();
    let mut indegree = vec![0u32; n];
    for i in 0..n as u32 {
        for &p in dag.parents(i) {
            indegree[p as usize] += 1;
        }
    }
    let tips = dag
        .starts
        .iter()
        .copied()
        .filter(|&i| indegree[i as usize] == 0)
        .collect::<Vec<_>>();
    let mut out = Vec::with_capacity(n);
    // 부모의 남은 자식 수를 줄이고, 0 이 되면 `ready` 에 넣는다.
    let mut release = |i: u32, ready: &mut dyn FnMut(u32)| {
        for &p in dag.parents(i) {
            let d = &mut indegree[p as usize];
            *d -= 1;
            if *d == 0 {
                ready(p);
            }
        }
    };
    match order {
        LogOrder::Topo => {
            let mut stack = tips;
            stack.reverse();
            while let Some(i) = stack.pop() {
                if out.len() % STOP_EVERY == 0 && stop.load(Ordering::Relaxed) {
                    return Err(stopped());
                }
                out.push(i);
                release(i, &mut |p| stack.push(p));
            }
        }
        LogOrder::Date => {
            // (시각, 넣은 차례의 역순, 번호) — 시각이 늦은 것, 같으면 먼저 넣은 것이 먼저 나온다.
            let mut heap = BinaryHeap::new();
            let mut counter = 0u64;
            for i in tips {
                heap.push((dag.time[i as usize], std::cmp::Reverse(counter), i));
                counter += 1;
            }
            while let Some((_, _, i)) = heap.pop() {
                if out.len() % STOP_EVERY == 0 && stop.load(Ordering::Relaxed) {
                    return Err(stopped());
                }
                out.push(i);
                release(i, &mut |p| {
                    heap.push((dag.time[p as usize], std::cmp::Reverse(counter), p));
                    counter += 1;
                });
            }
        }
    }
    Ok(out)
}
