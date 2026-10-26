//! 레인 배치: 픽스처(머지 · 문어발 머지 · 고아 브랜치)의 스냅숏과 합성 이력 전체의 연속성.
//!
//! 스냅숏은 행마다 `*`(노드) · `|`(지나가는 간선) 그림과 간선 목록(`up` 레인 → 노드 → `down` 레인, 레인:색)이다. 커밋 시각은
//! 시험 저장소의 시계로 정해져 순서가 실행마다 같다.

use super::*;
use crate::testing::TestRepo;

fn render(rows: &[LogRow]) -> String {
    let edges = |list: &[(u32, u32)]| -> String {
        list.iter()
            .map(|(lane, color)| format!("{lane}:{color}"))
            .collect::<Vec<_>>()
            .join(",")
    };
    let mut out = String::new();
    for row in rows {
        let g = row.graph.as_ref().expect("a graph row");
        let width =
            g.up.iter()
                .chain(&g.down)
                .chain(&g.pass)
                .map(|e| e.0 + 1)
                .chain([g.lane + 1])
                .max()
                .unwrap_or(1);
        let mut art = String::new();
        for lane in 0..width {
            art.push(if lane == g.lane {
                '*'
            } else if g.pass.iter().any(|e| e.0 == lane) {
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
            let widest = rows
                .iter()
                .filter_map(|r| r.graph.as_ref())
                .flat_map(|g| g.pass.iter().chain(&g.down).map(|e| e.0))
                .max()
                .unwrap();
            assert!(widest >= 4, "the history should be wide: {widest}");
        }
    }
    s.remove_commit_graph();
}
