//! 로그 시험. 저장소는 임시 폴더에 진짜 git 으로 만들고(`crate::testing`), 순서 · 필터는 같은 옵션의 `git log` 출력과 맞춘다.
//! 레인 배치는 스냅숏(insta)과 연속성 검사([`check_lanes`])로 본다.

mod commits;
mod filters;
mod graph;
mod order;
mod perf;
mod rpc;
#[cfg(unix)]
mod untrusted;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::AtomicBool;

use super::*;
use crate::testing::repo::git_command;
use crate::testing::synth;

/// 미신뢰로 연다(RPC 와 같은 열기 경로).
fn open(dir: &Path) -> gix::Repository {
    crate::repo::discover(dir, false)
        .expect("open the repository")
        .expect("a repository")
}

/// 로그 전체(청크를 이어 붙인 행)와 청크마다의 (행 수, `total`). 그래프 목록이면 청크의 첫 행에만 `top` 이 있다.
fn log_chunks(
    dir: &Path,
    refs: Option<&[&str]>,
    filter: &LogFilter,
    order: LogOrder,
) -> (Vec<LogRow>, Vec<(usize, Option<u32>)>) {
    let query = Query::new(
        refs.map(|r| r.iter().map(|s| (*s).to_owned()).collect()),
        filter,
        order,
    )
    .expect("a valid query");
    let mut rows = Vec::new();
    let mut chunks = Vec::new();
    run(
        &open(dir),
        false,
        &query,
        Window::default(),
        &AtomicBool::new(false),
        &mut |chunk| {
            for (k, row) in chunk.rows.iter().enumerate() {
                if let Some(g) = &row.graph {
                    assert_eq!(
                        g.top.is_some(),
                        k == 0,
                        "`top` only on the first row of a chunk"
                    );
                }
            }
            chunks.push((chunk.rows.len(), chunk.total));
            rows.extend(chunk.rows);
            true
        },
    )
    .expect("the log");
    (rows, chunks)
}

/// `top` 을 지운 행(청크 경계가 다른 두 목록을 견준다).
fn without_tops(rows: &[LogRow]) -> Vec<LogRow> {
    rows.iter()
        .cloned()
        .map(|mut r| {
            if let Some(g) = &mut r.graph {
                g.top = None;
            }
            r
        })
        .collect()
}

/// 행마다 지나가는 간선 — `graph` 머리 주석의 규칙대로 앞 행에서 이어 계산한다(UI 가 하는 일). `top` 이 실린 행은 이어 계산한
/// 윗변과 같아야 한다.
fn passing(rows: &[LogRow]) -> Vec<Vec<(u32, u32)>> {
    use std::collections::BTreeMap;
    let mut open: BTreeMap<u32, u32> = BTreeMap::new();
    let mut out = Vec::with_capacity(rows.len());
    for (n, row) in rows.iter().enumerate() {
        let g = row.graph.as_ref().expect("a graph row");
        if let Some(top) = &g.top {
            let expected: Vec<(u32, u32)> = open.iter().map(|(&l, &c)| (l, c)).collect();
            assert_eq!(top, &expected, "row {n}: `top` is the open lanes");
        }
        let ends: Vec<u32> =
            g.up.iter()
                .map(|e| e.0)
                .filter(|&lane| lane != crate::log::graph::FOLDED || g.fold_closes)
                .collect();
        let pass: Vec<(u32, u32)> = open
            .iter()
            .filter(|(lane, _)| !ends.contains(lane))
            .map(|(&l, &c)| (l, c))
            .collect();
        open = pass.iter().copied().collect();
        open.extend(g.down.iter().copied());
        out.push(pass);
    }
    out
}

fn log(dir: &Path, refs: Option<&[&str]>, filter: &LogFilter, order: LogOrder) -> Vec<LogRow> {
    log_chunks(dir, refs, filter, order).0
}

fn hashes(rows: &[LogRow]) -> Vec<String> {
    rows.iter().map(|r| r.hash.clone()).collect()
}

/// 합성 저장소 하나(임시 폴더). `ceiling` 은 위로 찾기를 멈추는 폴더다.
struct Synth {
    _tmp: tempfile::TempDir,
    dir: PathBuf,
    ceiling: PathBuf,
}

/// 시험용 합성 이력의 커밋 수. 브랜치 · 머지 · 문어발 · 고아가 모두 섞이는 크기다(시험에서 확인한다).
const SYNTH_COMMITS: usize = 2500;

impl Synth {
    fn new(commits: usize) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let ceiling =
            pw_core::registry::path::normalize(&std::fs::canonicalize(tmp.path()).unwrap())
                .unwrap();
        let dir = ceiling.join("synth");
        let summary =
            synth::create(&dir, &ceiling, commits, synth::SEED).expect("a synthetic repository");
        if commits >= SYNTH_COMMITS {
            assert!(
                summary.merges > 0 && summary.octopus > 0 && summary.orphans > 0,
                "{summary:?}"
            );
        }
        Self {
            _tmp: tmp,
            dir,
            ceiling,
        }
    }

    fn git(&self) -> Command {
        git_command(&self.dir, &self.ceiling)
    }

    /// `git log --format=%H <args>`.
    fn git_log(&self, args: &[&str]) -> Vec<String> {
        let out = self
            .git()
            .args(["log", "--format=%H"])
            .args(args)
            .output()
            .expect("run git");
        assert!(
            out.status.success(),
            "git log {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout)
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    /// git 을 돌려 stdout 을 돌려준다. 실패하면 패닉한다.
    fn git_out(&self, args: &[&str]) -> String {
        let out = self.git().args(args).output().expect("run git");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    }

    /// 남은 `topic/*` 브랜치(이름순).
    fn topics(&self) -> Vec<String> {
        let topics: Vec<String> = self
            .git_out(&[
                "for-each-ref",
                "--format=%(refname:short)",
                "refs/heads/topic",
            ])
            .lines()
            .map(str::to_owned)
            .collect();
        assert!(topics.len() >= 2, "{topics:?}");
        topics
    }

    fn write_commit_graph(&self) {
        synth::write_commit_graph(&self.dir, &self.ceiling).unwrap();
        assert!(self.commit_graph().exists());
    }

    fn remove_commit_graph(&self) {
        let _ = std::fs::remove_file(self.commit_graph());
        let _ = std::fs::remove_dir_all(self.dir.join(".git/objects/info/commit-graphs"));
    }

    fn commit_graph(&self) -> PathBuf {
        self.dir.join(".git/objects/info/commit-graph")
    }
}

/// 레인 배치의 연속성: 행의 아랫변(지나가는 간선 + 노드에서 나간 간선)이 다음 행의 윗변(지나가는 간선 + 노드로 모이는 간선)과
/// 레인 · 색까지 같고, 레인이 기다리는 커밋이 실제 부모다 — 노드로 모이는 레인은 정확히 그 커밋을 기다리던 레인들이다. 지나가는
/// 간선은 [`passing`] 으로 이어 계산한 것이다. 보통 레인은 [`graph::MAX_LANES`] 개 안이고, 넘는 갈래는 접힌 레인이 함께 기다린다.
/// 뿌리가 아닌 커밋의 부모는 모두 행에 있어야 한다(그래프 목록은 조상을 모두 담는다). HEAD 의 줄(장식으로 찾은 HEAD 커밋에서
/// 행의 첫 부모를 따라간 사슬)의 노드는 레인 0 이고, 사슬이 끝나기 전에는 다른 갈래가 레인 0 을 새로 열지 않는다.
fn check_lanes(rows: &[LogRow]) {
    use crate::log::graph::{FOLDED, FOLDED_COLOR, MAX_LANES};
    use std::collections::{BTreeMap, BTreeSet, HashMap};
    let passes = passing(rows);
    let by_hash: HashMap<&str, &LogRow> = rows.iter().map(|r| (r.hash.as_str(), r)).collect();
    let mut chain: BTreeSet<String> = BTreeSet::new();
    let mut next = rows
        .iter()
        .find(|r| {
            r.refs
                .iter()
                .any(|d| d.current || d.kind == LogRefKind::Head)
        })
        .map(|r| r.hash.as_str());
    while let Some(hash) = next.filter(|h| chain.insert((*h).to_owned())) {
        next = by_hash
            .get(hash)
            .and_then(|r| r.parents.first())
            .map(String::as_str);
    }
    // 보통 레인 → (기다리는 커밋, 색), 접힌 레인이 기다리는 커밋들.
    let mut lanes: BTreeMap<u32, (String, u32)> = BTreeMap::new();
    let mut folded: BTreeSet<String> = BTreeSet::new();
    for (n, row) in rows.iter().enumerate() {
        let g = row.graph.as_ref().expect("a graph row");
        let mut waiting: Vec<u32> = lanes
            .iter()
            .filter(|(_, (target, _))| *target == row.hash)
            .map(|(&lane, _)| lane)
            .collect();
        if folded.contains(&row.hash) {
            waiting.push(FOLDED);
        }
        let up: Vec<u32> = g.up.iter().map(|e| e.0).collect();
        assert_eq!(
            up, waiting,
            "row {n} ({}): lanes converging on the node",
            row.subject
        );
        for &(lane, color) in &g.up {
            let expected = if lane == FOLDED {
                FOLDED_COLOR
            } else {
                lanes[&lane].1
            };
            assert_eq!(expected, color, "row {n}: color of up lane {lane}");
        }
        let on_chain = chain.remove(&row.hash);
        let reserved = !chain.is_empty();
        if on_chain {
            assert_eq!(g.lane, 0, "row {n}: HEAD's line sits on lane 0");
            if let Some(&(first, color)) = g.up.first().filter(|e| e.0 == 0) {
                assert_eq!((first, color), (g.lane, g.color), "row {n}: lane 0 goes on");
            }
        } else {
            match g.up.first() {
                Some(&(first, _)) => assert_eq!(
                    first, g.lane,
                    "row {n}: the node sits on the leftmost waiting lane"
                ),
                None => assert!(
                    g.lane == FOLDED || !lanes.contains_key(&g.lane),
                    "row {n}: a new line starts on a free lane"
                ),
            }
            assert!(
                !(reserved && g.up.is_empty() && g.lane == 0),
                "row {n}: lane 0 is kept for HEAD's line"
            );
        }
        assert!(g.lane <= FOLDED, "row {n}: lane {} over the cap", g.lane);
        if g.lane == FOLDED {
            assert_eq!(g.color, FOLDED_COLOR, "row {n}: a folded node");
        }
        let was_folded = !folded.is_empty();
        folded.remove(&row.hash);
        let mut passing: Vec<(u32, u32)> = lanes
            .iter()
            .filter(|(_, (target, _))| *target != row.hash)
            .map(|(&lane, &(_, color))| (lane, color))
            .collect();
        if !folded.is_empty() {
            passing.push((FOLDED, FOLDED_COLOR));
        }
        assert_eq!(passes[n], passing, "row {n}: passing lanes");
        for lane in &up {
            lanes.remove(lane);
        }
        assert_eq!(
            g.down.len(),
            row.parents.len(),
            "row {n}: one edge per parent"
        );
        for (k, (&(lane, color), parent)) in g.down.iter().zip(&row.parents).enumerate() {
            if k == 0 {
                assert_eq!(
                    (lane, color),
                    (g.lane, g.color),
                    "row {n}: first parent keeps the node lane"
                );
            }
            if lane == FOLDED {
                assert_eq!(
                    color, FOLDED_COLOR,
                    "row {n}: edge {k} into the folded lane"
                );
                folded.insert(parent.clone());
                continue;
            }
            assert!(lane < MAX_LANES, "row {n}: lane {lane} over the cap");
            if let Some((target, c)) = lanes.get(&lane) {
                assert_eq!(
                    (target, *c),
                    (parent, color),
                    "row {n}: edge {k} joins a lane waiting for its parent"
                );
            } else if k > 0 {
                assert!(
                    !(reserved && lane == 0),
                    "row {n}: edge {k} opens lane 0 while it is kept for HEAD's line"
                );
            }
            lanes.insert(lane, (parent.clone(), color));
        }
        assert_eq!(
            g.fold_closes,
            was_folded && folded.is_empty(),
            "row {n}: the folded lane closes"
        );
    }
    assert!(
        lanes.is_empty() && folded.is_empty(),
        "lanes still waiting after the last row: {lanes:?} {folded:?}"
    );
}
