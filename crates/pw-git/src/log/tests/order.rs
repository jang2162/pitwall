//! 순서가 `git log --topo-order` · `--date-order` 와 같다 — 합성 이력(머지 · 문어발 · 고아 · 같은 시각)에서 commit-graph 있음 ·
//! 없음 둘 다. commit-graph 가 없으면 보조 스레드가 객체를 미리 읽는 길(`walk_prefetched`), 있으면 그래프 파일 길이다.

use super::*;

/// 기본(모든 ref)과 같은 `git log` 인자.
const ALL: [&str; 4] = ["--branches", "--remotes", "--tags", "HEAD"];

fn check_orders(s: &Synth) {
    for (order, flag) in [
        (LogOrder::Topo, "--topo-order"),
        (LogOrder::Date, "--date-order"),
    ] {
        let mut args = vec![flag];
        args.extend(ALL);
        let ours = hashes(&log(&s.dir, None, &LogFilter::default(), order));
        let theirs = s.git_log(&args);
        assert_eq!(ours.len(), theirs.len(), "{flag}: commit count");
        assert_eq!(ours, theirs, "{flag}");
    }
    // 브랜치 필터: 시각이 같은 끝은 준 순서를 따른다.
    let topics = s.topics();
    let refs = [
        topics[0].as_str(),
        "main",
        "origin/main",
        "v1",
        topics[1].as_str(),
    ];
    let ours = hashes(&log(
        &s.dir,
        Some(&refs),
        &LogFilter::default(),
        LogOrder::Topo,
    ));
    let mut args = vec!["--topo-order"];
    args.extend(refs);
    assert_eq!(ours, s.git_log(&args), "branch filter");
}

#[test]
fn the_order_is_gits_without_a_commit_graph() {
    let s = Synth::new(SYNTH_COMMITS);
    assert!(!s.commit_graph().exists());
    check_orders(&s);
}

#[test]
fn the_order_is_gits_with_a_commit_graph() {
    let s = Synth::new(SYNTH_COMMITS);
    s.write_commit_graph();
    check_orders(&s);
}

/// commit-graph 가 일부 커밋만 담아도(쓴 뒤에 커밋이 더 생겼다) 같다 — 나머지는 객체에서 읽는다.
#[test]
fn a_stale_commit_graph_still_gives_gits_order() {
    let s = Synth::new(SYNTH_COMMITS);
    s.write_commit_graph();
    let topic = &s.topics()[0];
    let late = s.git_out(&[
        "-c",
        "user.name=x",
        "-c",
        "user.email=x@pitwall.invalid",
        "commit-tree",
        "HEAD^{tree}",
        "-p",
        "HEAD",
        "-p",
        topic,
        "-m",
        "late",
    ]);
    s.git_out(&["update-ref", "refs/heads/main", late.trim()]);
    check_orders(&s);
}

/// 같은 커밋을 가리키는 ref 가 여럿이고 HEAD 가 분리돼 있어도 끝이 한 번만 나온다.
#[test]
fn shared_tips_and_a_detached_head_appear_once() {
    let r = crate::testing::TestRepo::with_commit();
    r.commit_file("a.txt", "a\n", "a");
    r.branch("same");
    r.git(&["tag", "-a", "-m", "annotated", "t1"]);
    r.detach();
    let rows = log(r.root(), None, &LogFilter::default(), LogOrder::Topo);
    assert_eq!(hashes(&rows), [r.head(), r.rev_parse("HEAD~1")]);
    let names: Vec<(&str, LogRefKind, bool)> = rows[0]
        .refs
        .iter()
        .map(|r| (r.name.as_str(), r.kind, r.current))
        .collect();
    assert_eq!(
        names,
        [
            ("HEAD", LogRefKind::Head, false),
            ("main", LogRefKind::Branch, false),
            ("same", LogRefKind::Branch, false),
            ("t1", LogRefKind::Tag, false),
        ]
    );
    // 브랜치에 있으면 그 브랜치가 앞에 오고 `current` 다.
    r.checkout("same");
    let rows = log(r.root(), None, &LogFilter::default(), LogOrder::Topo);
    assert_eq!(rows[0].refs[0].name, "same");
    assert!(rows[0].refs[0].current);
}

/// 빈 저장소(HEAD 가 태어나지 않음)는 행 없이 `total` 0 인 청크 하나다.
#[test]
fn an_empty_repository_has_one_empty_chunk() {
    let r = crate::testing::TestRepo::new();
    let (rows, chunks) = log_chunks(r.root(), None, &LogFilter::default(), LogOrder::Topo);
    assert!(rows.is_empty());
    assert_eq!(chunks, [(0, Some(0))]);
}

/// 행의 내용: 제목은 첫 문단을 한 줄로(git `%s`), 작성자 · 시각 · 부모.
#[test]
fn a_row_carries_gits_fields() {
    let s = Synth::new(300);
    let rows = log(&s.dir, None, &LogFilter::default(), LogOrder::Topo);
    let out = s
        .git()
        .args([
            "log",
            "--topo-order",
            "--format=%H%x00%P%x00%s%x00%an%x00%ae%x00%at",
        ])
        .args(ALL)
        .output()
        .unwrap();
    let theirs: Vec<Vec<String>> = String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(|l| l.split('\0').map(str::to_owned).collect())
        .collect();
    assert_eq!(rows.len(), theirs.len());
    for (row, t) in rows.iter().zip(&theirs) {
        let parents = if t[1].is_empty() {
            Vec::new()
        } else {
            t[1].split(' ').map(str::to_owned).collect()
        };
        assert_eq!(
            (
                &row.hash,
                &row.parents,
                &row.subject,
                &row.author_name,
                &row.author_email,
                row.author_time.to_string()
            ),
            (&t[0], &parents, &t[2], &t[3], &t[4], t[5].clone())
        );
    }
}

/// 장식이 `git log --decorate=full` 의 ref 와 같다(원격의 심볼릭 ref 는 뺀다).
#[test]
fn decorations_match_gits() {
    let s = Synth::new(SYNTH_COMMITS);
    let out = s
        .git()
        .args([
            "for-each-ref",
            "--format=%(objectname) %(*objectname) %(refname)",
            "refs/heads",
            "refs/remotes",
            "refs/tags",
        ])
        .output()
        .unwrap();
    let mut theirs: Vec<(String, String)> = String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(|l| {
            let mut parts = l.split(' ');
            let (id, peeled, name) = (
                parts.next().unwrap(),
                parts.next().unwrap(),
                parts.next().unwrap(),
            );
            let commit = if peeled.is_empty() { id } else { peeled };
            let short = name
                .strip_prefix("refs/heads/")
                .or_else(|| name.strip_prefix("refs/remotes/"))
                .or_else(|| name.strip_prefix("refs/tags/"))
                .unwrap();
            (commit.to_owned(), short.to_owned())
        })
        .collect();
    theirs.sort();
    let rows = log(&s.dir, None, &LogFilter::default(), LogOrder::Topo);
    let mut ours: Vec<(String, String)> = rows
        .iter()
        .flat_map(|r| r.refs.iter().map(|d| (r.hash.clone(), d.name.clone())))
        .collect();
    ours.sort();
    assert!(theirs.len() > 20, "{}", theirs.len());
    assert_eq!(ours, theirs);
}
