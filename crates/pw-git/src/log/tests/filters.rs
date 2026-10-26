//! 필터마다 커밋 집합 · 순서가 같은 옵션의 `git log` 와 같다(합성 이력, commit-graph 있음 · 없음). 필터가 켜지면 행에 그래프가
//! 없다(결정 M2-D2).
//!
//! `--until` 만 예외가 있다: git 은 commit-graph 가 없으면 `--until` 로 빠진 커밋을 순서 계산에서도 빼서, 머지 위를 자르면 남은
//! 커밋의 순서가 commit-graph 가 있을 때와 다르다. 우리는 언제나 commit-graph 가 있을 때의 git 과 같다(거르기 전 순서에서 거른다)
//! — 없음에서는 집합만 맞춘다. 그 갈림 자체를 [`git_orders_until_differently_without_a_commit_graph`] 가 보인다.

use super::*;

const SINCE: i64 = 1_600_000_000 + 25_000;
const UNTIL: i64 = 1_600_000_000 + 80_000;

/// 필터와 같은 `git log` 인자.
fn git_args(f: &LogFilter) -> Vec<String> {
    let mut args = vec!["--topo-order".to_owned()];
    if f.author.is_some() || f.message.is_some() {
        args.push(if f.regex { "-E" } else { "-F" }.to_owned());
        if f.ignore_case {
            args.push("-i".to_owned());
        }
    }
    if let Some(a) = &f.author {
        args.push(format!("--author={a}"));
    }
    if let Some(m) = &f.message {
        args.push(format!("--grep={m}"));
    }
    if let Some(s) = f.since {
        args.push(format!("--max-age={s}"));
    }
    if let Some(u) = f.until {
        args.push(format!("--min-age={u}"));
    }
    args.extend(["--branches", "--remotes", "--tags", "HEAD"].map(str::to_owned));
    if !f.paths.is_empty() {
        args.push("--".to_owned());
        args.extend(f.paths.iter().cloned());
    }
    args
}

fn filters(s: &Synth) -> Vec<(&'static str, LogFilter)> {
    let f = LogFilter::default;
    // `main` 의 마지막 머지 아닌 커밋이 바꾼 파일 — 기본 이력 단순화 뒤에도 남는 파일이다.
    let file = s.git_out(&[
        "log",
        "--no-merges",
        "-1",
        "--name-only",
        "--format=",
        "main",
    ]);
    vec![
        (
            "author",
            LogFilter {
                author: Some("Grace Hopper".into()),
                ..f()
            },
        ),
        (
            "author email",
            LogFilter {
                author: Some("linus@".into()),
                ..f()
            },
        ),
        (
            "author regex",
            LogFilter {
                author: Some("^(Ada|Ken) ".into()),
                regex: true,
                ..f()
            },
        ),
        (
            "author ignore case",
            LogFilter {
                author: Some("BARBARA".into()),
                ignore_case: true,
                ..f()
            },
        ),
        (
            "message",
            LogFilter {
                message: Some("cache change".into()),
                ..f()
            },
        ),
        (
            "message body line",
            LogFilter {
                message: Some("Touches the graph".into()),
                ..f()
            },
        ),
        (
            "message regex",
            LogFilter {
                message: Some("^fix: (lexer|theme) change 1[0-9]+$".into()),
                regex: true,
                ..f()
            },
        ),
        (
            "message fixed dot",
            LogFilter {
                message: Some("n.".into()),
                ..f()
            },
        ),
        (
            "message ignore case",
            LogFilter {
                message: Some("MERGE TOPIC".into()),
                ignore_case: true,
                ..f()
            },
        ),
        (
            "author and message",
            LogFilter {
                author: Some("Ada".into()),
                message: Some("feat".into()),
                ..f()
            },
        ),
        (
            "since",
            LogFilter {
                since: Some(SINCE),
                ..f()
            },
        ),
        (
            "since and until",
            LogFilter {
                since: Some(SINCE),
                until: Some(UNTIL),
                ..f()
            },
        ),
        (
            "until",
            LogFilter {
                until: Some(UNTIL),
                ..f()
            },
        ),
        (
            "file",
            LogFilter {
                paths: vec![file.trim().to_owned()],
                ..f()
            },
        ),
        (
            "folder",
            LogFilter {
                paths: vec!["d7".into()],
                ..f()
            },
        ),
        (
            "deep folder",
            LogFilter {
                paths: vec!["d2/s5".into()],
                ..f()
            },
        ),
        (
            "two paths",
            LogFilter {
                paths: vec!["d1/s1/f1.txt".into(), "d9/s0".into()],
                ..f()
            },
        ),
        (
            "missing path",
            LogFilter {
                paths: vec!["nowhere/at/all".into()],
                ..f()
            },
        ),
        (
            "path and author",
            LogFilter {
                paths: vec!["d4".into()],
                author: Some("Dennis".into()),
                ..f()
            },
        ),
    ]
}

/// `until` 이 들어 있으면 commit-graph 없는 git 과는 집합만 맞춘다(머리 주석).
fn check(s: &Synth, graph: bool) {
    for (name, filter) in filters(s) {
        let rows = log(&s.dir, None, &filter, LogOrder::Topo);
        assert!(rows.iter().all(|r| r.graph.is_none()), "{name}: no graph");
        let ours = hashes(&rows);
        let args = git_args(&filter);
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let theirs = s.git_log(&args);
        if name != "missing path" {
            assert!(
                !theirs.is_empty(),
                "{name}: the filter should match something"
            );
        }
        if filter.until.is_some() && !graph {
            let (mut a, mut b) = (ours.clone(), theirs.clone());
            a.sort();
            b.sort();
            assert_eq!(a, b, "{name}: same commits");
        } else {
            assert_eq!(ours, theirs, "{name} (commit-graph {graph})");
        }
    }
}

#[test]
fn every_filter_matches_git_without_a_commit_graph() {
    let s = Synth::new(SYNTH_COMMITS);
    check(&s, false);
}

#[test]
fn every_filter_matches_git_with_a_commit_graph() {
    let s = Synth::new(SYNTH_COMMITS);
    s.write_commit_graph();
    check(&s, true);
}

/// 해시 접두: git 에 같은 옵션이 없어 전체 목록을 접두로 거른 것과 맞춘다. 대문자도 받는다.
#[test]
fn the_hash_filter_keeps_the_order_of_the_full_log() {
    let s = Synth::new(SYNTH_COMMITS);
    let all = s.git_log(&["--topo-order", "--branches", "--remotes", "--tags", "HEAD"]);
    for prefix in ["a", "0f", &all[1234][..12]] {
        let filter = LogFilter {
            hash: Some(prefix.to_ascii_uppercase()),
            ..LogFilter::default()
        };
        let ours = hashes(&log(&s.dir, None, &filter, LogOrder::Topo));
        let theirs: Vec<String> = all
            .iter()
            .filter(|h| h.starts_with(prefix))
            .cloned()
            .collect();
        assert!(!theirs.is_empty());
        assert_eq!(ours, theirs, "prefix {prefix}");
    }
}

/// 브랜치 필터는 그래프를 유지하고, 다른 필터와 함께면 그 브랜치에서 닿는 것만 거른다.
#[test]
fn a_branch_filter_keeps_the_graph_and_combines_with_other_filters() {
    let s = Synth::new(SYNTH_COMMITS);
    let topic = s.topics()[1].clone();
    let rows = log(
        &s.dir,
        Some(&[topic.as_str()]),
        &LogFilter::default(),
        LogOrder::Topo,
    );
    assert!(rows.iter().all(|r| r.graph.is_some()));
    check_lanes(&rows);
    assert_eq!(hashes(&rows), s.git_log(&["--topo-order", &topic]));
    let filter = LogFilter {
        author: Some("Ken".into()),
        paths: vec!["d5".into()],
        ..LogFilter::default()
    };
    let ours = hashes(&log(
        &s.dir,
        Some(&[topic.as_str(), "v1"]),
        &filter,
        LogOrder::Topo,
    ));
    let theirs = s.git_log(&[
        "--topo-order",
        "-F",
        "--author=Ken",
        &topic,
        "v1",
        "--",
        "d5",
    ]);
    assert!(!theirs.is_empty());
    assert_eq!(ours, theirs);
}

/// 경로 필터는 git 의 기본 이력 단순화를 따른다: 머지가 어느 부모와 경로가 같으면 그 부모만 따라가서, 다른 쪽 줄의 커밋(경로를
/// 바꿨다가 되돌린 것)은 나오지 않는다. `--full-history` 와는 다르다.
#[test]
fn a_path_filter_simplifies_history_like_git() {
    let r = crate::testing::TestRepo::with_commit();
    r.commit_file("f.txt", "1\n", "add f");
    r.git(&["checkout", "--quiet", "-b", "side"]);
    r.commit_file("f.txt", "2\n", "side changes f");
    r.commit_file("f.txt", "1\n", "side reverts f");
    r.checkout("main");
    r.commit_file("g.txt", "x\n", "main touches g");
    r.git(&["merge", "--no-ff", "--quiet", "-m", "merge side", "side"]);
    let filter = LogFilter {
        paths: vec!["f.txt".into()],
        ..LogFilter::default()
    };
    let subjects: Vec<String> = log(r.root(), Some(&["main"]), &filter, LogOrder::Topo)
        .into_iter()
        .map(|row| row.subject)
        .collect();
    let git = |args: &[&str]| -> Vec<String> {
        r.git(&[&["log", "--format=%s"], args].concat())
            .lines()
            .map(str::to_owned)
            .collect()
    };
    assert_eq!(subjects, ["add f"]);
    assert_eq!(subjects, git(&["--topo-order", "main", "--", "f.txt"]));
    assert_ne!(
        subjects,
        git(&["--topo-order", "--full-history", "main", "--", "f.txt"])
    );
}

/// git 자신이 commit-graph 유무로 `--until` 순서를 다르게 낸다(머리 주석의 근거). 머지 M(150) 아래 두 줄 A(130 · 140) · B(110 ·
/// 120)에서 M 을 잘라 낸다.
#[test]
fn git_orders_until_differently_without_a_commit_graph() {
    let r = crate::testing::TestRepo::with_commit();
    r.git(&["checkout", "--quiet", "-b", "side"]);
    r.commit_file("b.txt", "1\n", "B1");
    r.commit_file("b.txt", "2\n", "B2");
    r.checkout("main");
    r.commit_file("a.txt", "1\n", "A1");
    r.commit_file("a.txt", "2\n", "A2");
    r.git(&["merge", "--no-ff", "--quiet", "-m", "M", "side"]);
    let until = r
        .git(&["log", "-1", "--format=%ct", "HEAD~1"])
        .trim()
        .parse::<i64>()
        .unwrap();
    let git = || -> Vec<String> {
        r.git(&[
            "log",
            "--topo-order",
            "--format=%s",
            &format!("--min-age={until}"),
            "main",
        ])
        .lines()
        .map(str::to_owned)
        .collect()
    };
    let without = git();
    r.git(&["commit-graph", "write", "--reachable"]);
    let with = git();
    assert_ne!(
        without, with,
        "git's own --until order depends on the commit-graph"
    );
    let ours: Vec<String> = log(
        r.root(),
        Some(&["main"]),
        &LogFilter {
            until: Some(until),
            ..LogFilter::default()
        },
        LogOrder::Topo,
    )
    .into_iter()
    .map(|row| row.subject)
    .collect();
    assert_eq!(ours, with);
}

/// 잘못된 입력은 걷기 전에 invalid params.
#[test]
fn bad_filters_are_invalid_params() {
    let bad = [
        LogFilter {
            author: Some("(".into()),
            regex: true,
            ..LogFilter::default()
        },
        LogFilter {
            message: Some(String::new()),
            ..LogFilter::default()
        },
        LogFilter {
            hash: Some("xyz".into()),
            ..LogFilter::default()
        },
        LogFilter {
            hash: Some(String::new()),
            ..LogFilter::default()
        },
        LogFilter {
            paths: vec!["/abs".into()],
            ..LogFilter::default()
        },
        LogFilter {
            paths: vec!["a/../b".into()],
            ..LogFilter::default()
        },
        LogFilter {
            paths: vec!["".into()],
            ..LogFilter::default()
        },
    ];
    for f in bad {
        let e = Query::new(None, &f, LogOrder::Topo).unwrap_err();
        assert_eq!(e.code, pw_rpc::code::INVALID_PARAMS, "{f:?}: {e:?}");
    }
    let e = Query::new(Some(Vec::new()), &LogFilter::default(), LogOrder::Topo).unwrap_err();
    assert_eq!(e.code, pw_rpc::code::INVALID_PARAMS);
    // 패턴은 글자 그대로가 기본이라 `(` 도 된다.
    Query::new(
        None,
        &LogFilter {
            author: Some("(".into()),
            ..LogFilter::default()
        },
        LogOrder::Topo,
    )
    .unwrap();
}

/// 경로는 저장소 작업 트리 기준 상대 경로, 구분자 `/`(git RPC 공통). Windows 에서는 `\` 도 `/` 로 바뀌고, 다른 OS 에서는 이름의
/// 일부다(그런 폴더가 없어 아무것도 안 맞는다). 겹친 `/` 와 끝의 `/` 는 접힌다.
#[test]
fn paths_are_relative_to_the_work_tree_with_forward_slashes() {
    use crate::log::filter::relative_path;
    assert_eq!(relative_path("d1//s1/").unwrap(), "d1/s1");
    let back = relative_path("d1\\s1").unwrap();
    assert_eq!(back, if cfg!(windows) { "d1/s1" } else { "d1\\s1" });
    let s = Synth::new(300);
    let with = |path: &str| {
        hashes(&log(
            &s.dir,
            None,
            &LogFilter {
                paths: vec![path.to_owned()],
                ..LogFilter::default()
            },
            LogOrder::Topo,
        ))
    };
    let slash = with("d1/s1");
    assert!(!slash.is_empty());
    assert_eq!(
        slash,
        s.git_log(&[
            "--topo-order",
            "--branches",
            "--remotes",
            "--tags",
            "HEAD",
            "--",
            "d1/s1"
        ])
    );
    if cfg!(windows) {
        assert_eq!(with("d1\\s1"), slash);
    } else {
        assert!(with("d1\\s1").is_empty());
    }
}
