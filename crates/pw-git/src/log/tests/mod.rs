//! 로그 시험. 저장소는 임시 폴더에 진짜 git 으로 만들고(`crate::testing`), 순서 · 필터는 같은 옵션의 `git log` 출력과 맞춘다.
//! 레인 배치는 스냅숏(insta)과 연속성 검사([`check_lanes`])로 본다.

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

/// 로그 전체(청크를 이어 붙인 행)와 청크마다의 (행 수, `total`).
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
        &query,
        Window::default(),
        &AtomicBool::new(false),
        &mut |chunk| {
            chunks.push((chunk.rows.len(), chunk.total));
            rows.extend(chunk.rows);
            true
        },
    )
    .expect("the log");
    (rows, chunks)
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
/// 레인 · 색까지 같고, 레인이 기다리는 커밋이 실제 부모다 — 노드로 모이는 레인은 정확히 그 커밋을 기다리던 레인들이다. 뿌리가
/// 아닌 커밋의 부모는 모두 행에 있어야 한다(그래프 목록은 조상을 모두 담는다).
fn check_lanes(rows: &[LogRow]) {
    use std::collections::BTreeMap;
    // 레인 → (기다리는 커밋, 색).
    let mut lanes: BTreeMap<u32, (String, u32)> = BTreeMap::new();
    for (n, row) in rows.iter().enumerate() {
        let g = row.graph.as_ref().expect("a graph row");
        let waiting: Vec<u32> = lanes
            .iter()
            .filter(|(_, (target, _))| *target == row.hash)
            .map(|(&lane, _)| lane)
            .collect();
        let up: Vec<u32> = g.up.iter().map(|e| e.0).collect();
        assert_eq!(
            up, waiting,
            "row {n} ({}): lanes converging on the node",
            row.subject
        );
        for &(lane, color) in &g.up {
            assert_eq!(lanes[&lane].1, color, "row {n}: color of up lane {lane}");
        }
        if let Some(&(first, _)) = g.up.first() {
            assert_eq!(
                first, g.lane,
                "row {n}: the node sits on the leftmost waiting lane"
            );
        }
        let passing: Vec<(u32, u32)> = lanes
            .iter()
            .filter(|(_, (target, _))| *target != row.hash)
            .map(|(&lane, &(_, color))| (lane, color))
            .collect();
        assert_eq!(g.pass, passing, "row {n}: passing lanes");
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
            if let Some((target, c)) = lanes.get(&lane) {
                assert_eq!(
                    (target, *c),
                    (parent, color),
                    "row {n}: edge {k} joins a lane waiting for its parent"
                );
            }
            lanes.insert(lane, (parent.clone(), color));
        }
    }
    assert!(
        lanes.is_empty(),
        "lanes still waiting after the last row: {lanes:?}"
    );
}
