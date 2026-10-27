//! 레인 배치: 픽스처(머지 · 문어발 머지 · 고아 브랜치)의 스냅숏, 합성 이력 전체의 연속성, 레인 상한(접힌 레인)과 청크 크기.
//!
//! 스냅숏은 행마다 `*`(노드) · `|`(지나가는 간선) 그림과 간선 목록(`up` 레인 → 노드 → `down` 레인, 레인:색)이다. 커밋 시각은
//! 시험 저장소의 시계로 정해져 순서가 실행마다 같다.

use super::*;
use crate::testing::TestRepo;
use crate::testing::repo::git_command;

fn render(rows: &[LogRow]) -> String {
    let edges = |list: &[(u32, u32)]| -> String {
        list.iter()
            .map(|(lane, color)| format!("{lane}:{color}"))
            .collect::<Vec<_>>()
            .join(",")
    };
    let passes = passing(rows);
    let mut out = String::new();
    for (row, pass) in rows.iter().zip(&passes) {
        let g = row.graph.as_ref().expect("a graph row");
        let width =
            g.up.iter()
                .chain(&g.down)
                .chain(pass)
                .map(|e| e.0 + 1)
                .chain([g.lane + 1])
                .max()
                .unwrap_or(1);
        let mut art = String::new();
        for lane in 0..width {
            art.push(if lane == g.lane {
                '*'
            } else if pass.iter().any(|e| e.0 == lane) {
                '|'
            } else {
                ' '
            });
        }
        out.push_str(&format!(
            "{art:<5} {:<16} up [{}] node {}:{} down [{}]\n",
            row.subject,
            edges(&g.up),
            g.lane,
            g.color,
            edges(&g.down)
        ));
    }
    out
}

fn graph_of(r: &TestRepo) -> Vec<LogRow> {
    let rows = log(r.root(), None, &LogFilter::default(), LogOrder::Topo);
    check_lanes(&rows);
    rows
}

/// 머지 둘: 갈라진 브랜치가 다시 모이고, 머지된 쪽 줄이 먼저 나온다(topo).
#[test]
fn merges() {
    let r = TestRepo::with_commit();
    r.git(&["checkout", "--quiet", "-b", "feature"]);
    r.commit_file("f.txt", "1\n", "feature 1");
    r.commit_file("f.txt", "2\n", "feature 2");
    r.checkout("main");
    r.commit_file("m.txt", "1\n", "main 1");
    r.git(&[
        "merge",
        "--no-ff",
        "--quiet",
        "-m",
        "merge feature",
        "feature",
    ]);
    r.git(&["checkout", "--quiet", "-b", "fix", "HEAD~1"]);
    r.commit_file("x.txt", "1\n", "fix 1");
    r.checkout("main");
    r.commit_file("m.txt", "2\n", "main 2");
    r.git(&["merge", "--no-ff", "--quiet", "-m", "merge fix", "fix"]);
    insta::assert_snapshot!(render(&graph_of(&r)));
}

/// 문어발 머지(부모 넷): 부모마다 레인이 열리고 각 줄이 공통 조상에서 모인다.
#[test]
fn octopus_merge() {
    let r = TestRepo::with_commit();
    for name in ["a", "b", "c"] {
        r.git(&["checkout", "--quiet", "-b", name, "main"]);
        r.commit_file(&format!("{name}.txt"), "1\n", &format!("{name} 1"));
    }
    r.checkout("main");
    r.commit_file("m.txt", "1\n", "main 1");
    r.git(&["merge", "--quiet", "-m", "octopus", "a", "b", "c"]);
    let rows = graph_of(&r);
    assert_eq!(rows[0].parents.len(), 4);
    insta::assert_snapshot!(render(&rows));
}

/// 고아 브랜치: 뿌리가 둘 — 고아 줄은 끝에서 레인이 닫히고, 그 뒤 행이 빈 레인을 다시 쓴다.
#[test]
fn orphan_branch() {
    let r = TestRepo::with_commit();
    r.commit_file("m.txt", "1\n", "main 1");
    r.git(&["checkout", "--quiet", "--orphan", "pages"]);
    r.git(&["rm", "-r", "--quiet", "-f", "."]);
    r.commit_file("index.html", "1\n", "pages 1");
    r.commit_file("index.html", "2\n", "pages 2");
    r.checkout("main");
    r.commit_file("m.txt", "2\n", "main 2");
    r.git(&[
        "merge",
        "--quiet",
        "--no-ff",
        "--allow-unrelated-histories",
        "-m",
        "adopt pages",
        "pages",
    ]);
    r.commit_file("m.txt", "3\n", "main 3");
    insta::assert_snapshot!(render(&graph_of(&r)));
}

/// 합성 이력 전체(topo · date, commit-graph 있음 · 없음)에서 레인이 이어진다. 첫 청크는 200행이고 `total` 이 실린다.
#[test]
fn lanes_stay_connected_across_the_synthetic_history() {
    let s = Synth::new(SYNTH_COMMITS);
    for graph in [false, true] {
        if graph {
            s.write_commit_graph();
        }
        for order in [LogOrder::Topo, LogOrder::Date] {
            let (rows, chunks) = log_chunks(&s.dir, None, &LogFilter::default(), order);
            check_lanes(&rows);
            assert_eq!(chunks[0], (FIRST_CHUNK, Some(rows.len() as u32)));
            assert!(
                chunks[1..]
                    .iter()
                    .all(|&(n, total)| n <= CHUNK && total.is_none())
            );
            assert_eq!(chunks.iter().map(|c| c.0).sum::<usize>(), rows.len());
            let widest = passing(&rows)
                .iter()
                .zip(&rows)
                .flat_map(|(pass, r)| {
                    let down = &r.graph.as_ref().unwrap().down;
                    pass.iter().chain(down).map(|e| e.0)
                })
                .max()
                .unwrap();
            assert!(widest >= 4, "the history should be wide: {widest}");
        }
    }
    s.remove_commit_graph();
}

/// 머지의 둘째 부모는 노드 오른쪽의 빈 레인에 연다 — 왼쪽에 빈 레인이 있어도.
#[test]
fn a_second_parent_opens_right_of_the_node() {
    let mut l = Layout::new();
    // 0 → 레인 0(부모 2), 1 → 레인 1(부모 3), 2 는 뿌리(레인 0 이 빈다), 3 은 머지(부모 4 · 5).
    assert_eq!(l.row(0, &[2]).lane, 0);
    assert_eq!(l.row(1, &[3]).lane, 1);
    assert_eq!(l.row(2, &[]).down, []);
    let merge = l.row(3, &[4, 5]);
    assert_eq!(merge.lane, 1);
    assert_eq!(
        merge.down.iter().map(|e| e.0).collect::<Vec<_>>(),
        [1, 2],
        "{merge:?}"
    );
}

/// 보통 레인이 다 차면 새 갈래는 접힌 레인으로 간다. 접힌 레인은 기다리는 커밋이 모두 나온 행에서 닫힌다.
#[test]
fn lanes_past_the_cap_fold_into_one() {
    use crate::log::graph::{FOLDED, FOLDED_COLOR, MAX_LANES};
    let mut l = Layout::new();
    let tips = MAX_LANES + 6;
    for tip in 0..tips {
        let row = l.row(tip, &[1000]);
        if tip < MAX_LANES {
            assert_eq!(row.lane, tip);
        } else {
            assert_eq!((row.lane, row.color), (FOLDED, FOLDED_COLOR), "tip {tip}");
            assert_eq!(row.down, [(FOLDED, FOLDED_COLOR)]);
        }
        assert!(!row.fold_closes);
    }
    let top = l.top();
    assert_eq!(top.len(), MAX_LANES as usize + 1);
    assert_eq!(top.last(), Some(&(FOLDED, FOLDED_COLOR)));
    let root = l.row(1000, &[]);
    assert_eq!(root.lane, 0);
    assert_eq!(root.up.len(), MAX_LANES as usize + 1);
    assert!(root.fold_closes);
    assert!(l.top().is_empty());
}

/// 원격 브랜치가 수천 개 쌓인 저장소(곧은 `main` 곳곳에서 커밋 하나씩 갈라짐): 레인이 상한을 넘지 않고(넘는 갈래는 접힌 레인),
/// 행이 지나가는 레인을 싣지 않아 청크 크기가 레인 수와 상관없이 작다.
#[test]
fn thousands_of_remote_branches_stay_within_the_lane_cap() {
    use crate::log::graph::FOLDED;
    let tmp = tempfile::tempdir().unwrap();
    let ceiling =
        pw_core::registry::path::normalize(&std::fs::canonicalize(tmp.path()).unwrap()).unwrap();
    let dir = ceiling.join("fanout");
    synth::fanout(&dir, &ceiling, 2000, 1200, synth::SEED).unwrap();
    let query = Query::new(None, &LogFilter::default(), LogOrder::Topo).unwrap();
    let mut rows = Vec::new();
    let mut largest = 0usize;
    run(
        &open(&dir),
        false,
        &query,
        Window::default(),
        &AtomicBool::new(false),
        &mut |chunk| {
            largest = largest.max(serde_json::to_vec(&chunk).unwrap().len() / chunk.rows.len());
            rows.extend(chunk.rows);
            true
        },
    )
    .unwrap();
    assert_eq!(rows.len(), 3200);
    check_lanes(&rows);
    let graphs: Vec<&GraphRow> = rows.iter().map(|r| r.graph.as_ref().unwrap()).collect();
    assert!(
        graphs.iter().any(|g| g.lane == FOLDED),
        "some branches fold"
    );
    assert!(
        graphs
            .iter()
            .flat_map(|g| g.top.iter().flatten().chain(&g.up).chain(&g.down))
            .all(|e| e.0 <= FOLDED)
    );
    // 한 행(해시 · 부모 · 제목 · 작성자 · 장식 · 그래프)의 평균 JSON 크기. 지나가는 레인을 행마다 실으면 수 KB 다.
    assert!(largest < 600, "{largest} bytes per row");
}

/// HEAD 의 줄은 레인 0 이다: HEAD(`main`)보다 늦은 갈래 둘이 먼저 나와 레인을 차지해도 `main` 의 노드는 레인 0 에 놓이고, 그
/// 갈래들은 레인 0 을 새로 열지 않는다(M2-14 에서 바꾼 배정 — 바꾸기 전에는 `main` 줄이 레인 2 였다).
#[test]
fn head_line_stays_on_lane_zero() {
    let r = TestRepo::with_commit();
    r.commit_file("m.txt", "1\n", "main 1");
    for name in ["late-a", "late-b"] {
        r.git(&["checkout", "--quiet", "-b", name, "main"]);
        r.commit_file(&format!("{name}.txt"), "1\n", &format!("{name} 1"));
        r.commit_file(&format!("{name}.txt"), "2\n", &format!("{name} 2"));
    }
    r.checkout("main");
    let rows = graph_of(&r);
    let head = rows
        .iter()
        .find(|r| r.refs.iter().any(|d| d.current))
        .unwrap();
    assert_eq!(head.graph.as_ref().unwrap().lane, 0);
    insta::assert_snapshot!(render(&rows));
}

/// `main` 2천 커밋 + 원격 갈래 300 개(갈래가 모두 HEAD 보다 늦다): 갈래가 보통 레인을 다 차지해도 HEAD 행은 레인 0 이다.
/// M2-03 재리뷰에서 HEAD 행이 접힌 레인(64)에 놓였던 모양이다.
#[test]
fn head_stays_on_lane_zero_past_hundreds_of_remote_branches() {
    use crate::log::graph::{FOLDED, MAX_LANES};
    let tmp = tempfile::tempdir().unwrap();
    let ceiling =
        pw_core::registry::path::normalize(&std::fs::canonicalize(tmp.path()).unwrap()).unwrap();
    let dir = ceiling.join("fanout");
    synth::fanout(&dir, &ceiling, 2000, 300, synth::SEED).unwrap();
    let rows = log(&dir, None, &LogFilter::default(), LogOrder::Topo);
    check_lanes(&rows);
    let at = rows
        .iter()
        .position(|r| r.refs.iter().any(|d| d.current))
        .expect("the HEAD row");
    assert!(
        at > MAX_LANES as usize,
        "the remote lines come first: HEAD at row {at}"
    );
    assert_eq!(rows[at].graph.as_ref().unwrap().lane, 0);
    assert!(
        rows[..at]
            .iter()
            .any(|r| r.graph.as_ref().unwrap().lane == FOLDED),
        "the remote lines fill the ordinary lanes"
    );
}

/// 쪽(`Window`)으로 이어 받은 행이 한 번에 배치한 행과 같다 — HEAD 의 줄 예약이 커서를 넘어 이어진다. 무작위 이력(시드 셋, HEAD 를
/// 토픽 브랜치로 옮긴 것 포함) · 얕은 클론 · 여러 ref(HEAD 를 담은 것 · 뺀 것)마다, `limit` 1 · 7 · 200 · 500 으로. 쪽의 첫 행
/// `top` 은 앞 쪽에서 이어 계산한 열린 레인과 같아야 한다([`passing`]).
#[test]
fn pages_lay_out_lanes_like_one_pass() {
    let compare = |dir: &Path, refs: Option<&[&str]>, what: &str| {
        let all = log(dir, refs, &LogFilter::default(), LogOrder::Topo);
        check_lanes(&all);
        let query = Query::new(
            refs.map(|r| r.iter().map(|s| (*s).to_owned()).collect()),
            &LogFilter::default(),
            LogOrder::Topo,
        )
        .unwrap();
        let repo = open(dir);
        for limit in [1usize, 7, 200, 500] {
            let mut paged = Vec::new();
            let mut after = None;
            loop {
                let mut page = Vec::new();
                let outcome = run(
                    &repo,
                    false,
                    &query,
                    Window {
                        after,
                        limit: Some(limit),
                    },
                    &AtomicBool::new(false),
                    &mut |chunk| {
                        page.extend(chunk.rows);
                        true
                    },
                )
                .unwrap();
                assert!(
                    page.first()
                        .is_none_or(|r| r.graph.as_ref().unwrap().top.is_some()),
                    "{what}: a page starts with its open lanes"
                );
                after = page
                    .last()
                    .map(|r| gix::ObjectId::from_hex(r.hash.as_bytes()).unwrap());
                paged.extend(page);
                if !outcome.more {
                    break;
                }
            }
            // 쪽마다 실린 `top` 이 이어 계산한 것과 같은지 본다(`passing` 이 단언한다).
            passing(&paged);
            assert_eq!(
                without_tops(&paged),
                without_tops(&all),
                "{what}: limit {limit}"
            );
        }
    };
    for seed in [synth::SEED, 0x1234_5678, 0xdead_beef] {
        let tmp = tempfile::tempdir().unwrap();
        let ceiling =
            pw_core::registry::path::normalize(&std::fs::canonicalize(tmp.path()).unwrap())
                .unwrap();
        let dir = ceiling.join("synth");
        synth::create(&dir, &ceiling, 150, seed).unwrap();
        let git = |args: &[&str]| {
            let out = git_command(&dir, &ceiling).args(args).output().unwrap();
            assert!(out.status.success(), "git {args:?}");
            String::from_utf8(out.stdout).unwrap()
        };
        let topics = git(&[
            "for-each-ref",
            "--format=%(refname:short)",
            "refs/heads/topic",
        ]);
        let topic = topics.lines().next().expect("a topic branch").to_owned();
        compare(&dir, None, &format!("seed {seed:x}, HEAD main"));
        compare(
            &dir,
            Some(&["main", topic.as_str()]),
            &format!("seed {seed:x}, main + {topic}"),
        );
        compare(
            &dir,
            Some(&[topic.as_str()]),
            &format!("seed {seed:x}, {topic} only"),
        );
        git(&["symbolic-ref", "HEAD", &format!("refs/heads/{topic}")]);
        compare(&dir, None, &format!("seed {seed:x}, HEAD {topic}"));
        let clone = ceiling.join("shallow");
        let url = format!("file://{}", dir.display());
        git(&[
            "clone",
            "--quiet",
            "--no-local",
            "--depth",
            "20",
            "--no-single-branch",
            &url,
            clone.to_str().unwrap(),
        ]);
        assert!(clone.join(".git/shallow").exists());
        compare(&clone, None, &format!("seed {seed:x}, shallow"));
    }
}
