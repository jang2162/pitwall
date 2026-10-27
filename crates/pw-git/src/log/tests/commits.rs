//! 커밋 글자(mailmap · `encoding` 헤더 · 길이 상한)와 걷기의 특수한 저장소(얕은 클론 · 대체 객체)가 git log 와 같다.

use super::*;
use crate::testing::TestRepo;

/// `trusted` 로 열어 로그 전체.
fn log_as(dir: &Path, trusted: bool, refs: Option<&[&str]>, filter: &LogFilter) -> Vec<LogRow> {
    let repo = crate::repo::discover(dir, trusted).unwrap().unwrap();
    let query = Query::new(
        refs.map(|r| r.iter().map(|s| (*s).to_owned()).collect()),
        filter,
        LogOrder::Topo,
    )
    .unwrap();
    let mut rows = Vec::new();
    run(
        &repo,
        trusted,
        &query,
        Window::default(),
        &AtomicBool::new(false),
        &mut |chunk| {
            rows.extend(chunk.rows);
            true
        },
    )
    .unwrap();
    rows
}

fn author(name: &str) -> LogFilter {
    LogFilter {
        author: Some(name.to_owned()),
        ..LogFilter::default()
    }
}

/// 작성자가 `name <email>` 인 커밋.
fn commit_by(r: &TestRepo, name: &str, email: &str, message: &str) {
    let out = r
        .command_in(r.root())
        .env("GIT_AUTHOR_NAME", name)
        .env("GIT_AUTHOR_EMAIL", email)
        .args(["commit", "--quiet", "--allow-empty", "-m", message])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// `(작성자 이름 · 메일, 제목)` 을 git 으로. `%aN` · `%aE` 는 mailmap 을 따른다(`log.mailmap` 이 켜졌을 때 기본 형식의 `Author:`
/// 줄과 같다 — `%an` 은 언제나 원래 이름이다).
fn git_idents(r: &TestRepo, args: &[&str]) -> Vec<String> {
    r.git(&[&["log", "--topo-order", "--format=%aN <%aE> %s"], args].concat())
        .lines()
        .map(str::to_owned)
        .collect()
}

fn idents(rows: &[LogRow]) -> Vec<String> {
    rows.iter()
        .map(|r| format!("{} <{}> {}", r.author_name, r.author_email, r.subject))
        .collect()
}

/// mailmap: 작업 트리의 `.mailmap` 은 미신뢰에서도 따르고(표시 이름 · `--author`), 설정의 `mailmap.file` 은 신뢰할 때만 따른다.
/// `log.mailmap = false` 면 따르지 않는다. 모두 git log 와 같다.
#[test]
fn the_mailmap_rewrites_authors_like_git() {
    let r = TestRepo::with_commit();
    commit_by(&r, "Old Name", "old@pitwall.invalid", "by old");
    commit_by(&r, "Other", "other@pitwall.invalid", "by other");
    r.write(
        ".mailmap",
        "New Name <new@pitwall.invalid> Old Name <old@pitwall.invalid>\n",
    );
    let extra = r.base().join("extra.mailmap");
    std::fs::write(
        &extra,
        "Mapped Other <mapped@pitwall.invalid> <other@pitwall.invalid>\n",
    )
    .unwrap();
    r.git(&["config", "mailmap.file", extra.to_str().unwrap()]);

    let git = git_idents(&r, &["main"]);
    assert!(
        git.iter()
            .any(|l| l.starts_with("New Name <new@pitwall.invalid> by old")),
        "{git:?}"
    );
    assert!(
        git.iter()
            .any(|l| l.starts_with("Mapped Other <mapped@pitwall.invalid>")),
        "{git:?}"
    );
    assert_eq!(
        idents(&log_as(
            r.root(),
            true,
            Some(&["main"]),
            &LogFilter::default()
        )),
        git
    );
    for (who, count) in [("New", 1), ("Old", 0), ("Mapped", 1)] {
        let theirs = git_idents(&r, &[&format!("--author={who}"), "main"]);
        assert_eq!(theirs.len(), count, "git --author={who}");
        assert_eq!(
            idents(&log_as(r.root(), true, Some(&["main"]), &author(who))),
            theirs,
            "--author={who}"
        );
    }

    // 미신뢰: `.mailmap` 파일만.
    let untrusted = idents(&log_as(
        r.root(),
        false,
        Some(&["main"]),
        &LogFilter::default(),
    ));
    assert!(
        untrusted
            .iter()
            .any(|l| l.starts_with("New Name <new@pitwall.invalid> by old")),
        "{untrusted:?}"
    );
    assert!(
        untrusted
            .iter()
            .any(|l| l.starts_with("Other <other@pitwall.invalid> by other")),
        "{untrusted:?}"
    );
    assert_eq!(
        log_as(r.root(), false, Some(&["main"]), &author("New")).len(),
        1
    );
    assert_eq!(
        log_as(r.root(), false, Some(&["main"]), &author("Old")).len(),
        0
    );
    assert_eq!(
        log_as(r.root(), false, Some(&["main"]), &author("Mapped")).len(),
        0
    );

    // `log.mailmap = false`: 기본 형식의 `Author:` 줄은 원래 이름(`%an`)이고 `--author` 도 원래 이름에 맞춘다.
    r.git(&["config", "log.mailmap", "false"]);
    let plain: Vec<String> = r
        .git(&["log", "--topo-order", "--format=%an <%ae> %s", "main"])
        .lines()
        .map(str::to_owned)
        .collect();
    assert!(
        plain
            .iter()
            .any(|l| l.starts_with("Old Name <old@pitwall.invalid> by old")),
        "{plain:?}"
    );
    assert_eq!(
        r.git(&["log", "--format=%s", "--author=Old", "main"])
            .trim(),
        "by old"
    );
    for trusted in [false, true] {
        assert_eq!(
            idents(&log_as(
                r.root(),
                trusted,
                Some(&["main"]),
                &LogFilter::default()
            )),
            plain
        );
        assert_eq!(
            log_as(r.root(), trusted, Some(&["main"]), &author("Old")).len(),
            1
        );
    }
}

/// 설정의 `mailmap.blob` 은 신뢰할 때만 따른다. 미신뢰 열기는 `mailmap.file`(경로)은 이미 버리지만 `mailmap.blob` 은 보통 값이라
/// gix 가 그대로 읽는다 — 미신뢰에서 설정을 보지 않는 것은 이 시험만 가른다.
#[test]
fn the_mailmap_blob_is_followed_only_when_trusted() {
    let r = TestRepo::with_commit();
    commit_by(&r, "Blob Old", "blob@pitwall.invalid", "by blob");
    let out = r
        .command_in(r.root())
        .args(["hash-object", "-w", "--stdin"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write as _;
            child
                .stdin
                .take()
                .unwrap()
                .write_all(b"Blob New <blob-new@pitwall.invalid> <blob@pitwall.invalid>\n")?;
            child.wait_with_output()
        })
        .unwrap();
    assert!(out.status.success());
    let blob = String::from_utf8(out.stdout).unwrap();
    r.git(&["config", "mailmap.blob", blob.trim()]);

    let git = git_idents(&r, &["main"]);
    assert!(
        git.iter()
            .any(|l| l.starts_with("Blob New <blob-new@pitwall.invalid> by blob")),
        "{git:?}"
    );
    assert_eq!(
        idents(&log_as(
            r.root(),
            true,
            Some(&["main"]),
            &LogFilter::default()
        )),
        git
    );
    assert_eq!(
        log_as(r.root(), true, Some(&["main"]), &author("Blob New")).len(),
        1
    );

    let untrusted = idents(&log_as(
        r.root(),
        false,
        Some(&["main"]),
        &LogFilter::default(),
    ));
    assert!(
        untrusted
            .iter()
            .any(|l| l.starts_with("Blob Old <blob@pitwall.invalid> by blob")),
        "{untrusted:?}"
    );
    assert_eq!(
        log_as(r.root(), false, Some(&["main"]), &author("Blob New")).len(),
        0
    );
}

/// 커밋 객체를 그대로 쓴다(`encoding` 헤더 · 바이트를 손으로 정한다). 부모는 `parent`.
fn raw_commit(r: &TestRepo, parent: &str, ident: &[u8], encoding: &str, message: &[u8]) -> String {
    let tree = r.git(&["rev-parse", "HEAD^{tree}"]);
    let mut body = format!("tree {}\nparent {parent}\n", tree.trim()).into_bytes();
    for role in ["author", "committer"] {
        body.extend_from_slice(role.as_bytes());
        body.push(b' ');
        body.extend_from_slice(ident);
        body.extend_from_slice(b" 1700000000 +0000\n");
    }
    body.extend_from_slice(format!("encoding {encoding}\n\n").as_bytes());
    body.extend_from_slice(message);
    let path = r.base().join("raw-commit");
    std::fs::write(&path, &body).unwrap();
    let id = r.git(&["hash-object", "-t", "commit", "-w", path.to_str().unwrap()]);
    let id = id.trim().to_owned();
    r.git(&["update-ref", "refs/heads/main", &id]);
    id
}

/// `encoding` 헤더의 인코딩(ISO-8859-1 · EUC-KR)에서 UTF-8 로 바꿔 보이고 거른다 — git log 와 같다. ISO-8859-1 의 0x93 은
/// git(iconv)처럼 U+0093 이다(windows-1252 로 읽으면 U+201C).
#[test]
fn encoded_commits_are_shown_and_matched_in_utf8_like_git() {
    let r = TestRepo::with_commit();
    let base = r.head();
    let latin = raw_commit(
        &r,
        &base,
        b"Jos\xe9 <jose@pitwall.invalid>",
        "ISO-8859-1",
        b"caf\xe9 au lait \x93\n",
    );
    // 「한글」 의 EUC-KR.
    raw_commit(
        &r,
        &latin,
        b"Kim <kim@pitwall.invalid>",
        "EUC-KR",
        b"\xc7\xd1\xb1\xdb commit\n",
    );
    let git = git_idents(&r, &["main"]);
    assert_eq!(git[0], "Kim <kim@pitwall.invalid> 한글 commit");
    assert_eq!(git[1], "José <jose@pitwall.invalid> café au lait \u{93}");
    assert_eq!(
        idents(&log(
            r.root(),
            Some(&["main"]),
            &LogFilter::default(),
            LogOrder::Topo
        )),
        git
    );
    for filter in [
        author("José"),
        LogFilter {
            message: Some("café".into()),
            ..LogFilter::default()
        },
        LogFilter {
            message: Some("한글".into()),
            ..LogFilter::default()
        },
    ] {
        let mut args = vec!["-F".to_owned()];
        if let Some(a) = &filter.author {
            args.push(format!("--author={a}"));
        }
        if let Some(m) = &filter.message {
            args.push(format!("--grep={m}"));
        }
        args.push("main".to_owned());
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let theirs = git_idents(&r, &args);
        assert_eq!(theirs.len(), 1, "{args:?}");
        assert_eq!(
            idents(&log(r.root(), Some(&["main"]), &filter, LogOrder::Topo)),
            theirs,
            "{args:?}"
        );
    }
}

/// 행의 제목은 [`commit::MAX_SUBJECT`] 글자에서 자르고, 메시지 거르기는 자르기 전 글자에 맞춘다.
#[test]
fn long_subjects_are_clipped_on_rows_but_not_for_filters() {
    let r = TestRepo::with_commit();
    let subject = format!("{}tail", "가".repeat(3000));
    r.commit_file("f.txt", "1\n", &subject);
    let rows = log(
        r.root(),
        Some(&["main"]),
        &LogFilter::default(),
        LogOrder::Topo,
    );
    assert_eq!(rows[0].subject.chars().count(), commit::MAX_SUBJECT + 1);
    assert!(rows[0].subject.ends_with('…'));
    let filter = LogFilter {
        message: Some("tail".into()),
        ..LogFilter::default()
    };
    assert_eq!(
        log(r.root(), Some(&["main"]), &filter, LogOrder::Topo).len(),
        1
    );
}

/// git 의 `--topo-order` 해시 목록(기본 ref).
fn git_hashes(dir: &Path, ceiling: &Path) -> Vec<String> {
    let out = crate::testing::repo::git_command(dir, ceiling)
        .args([
            "log",
            "--topo-order",
            "--format=%H %P",
            "--branches",
            "--remotes",
            "--tags",
            "HEAD",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(|l| l.trim_end().to_owned())
        .collect()
}

fn hashes_and_parents(rows: &[LogRow]) -> Vec<String> {
    rows.iter()
        .map(|r| {
            let mut line = r.hash.clone();
            line.push(' ');
            line.push_str(&r.parents.join(" "));
            line.trim_end().to_owned()
        })
        .collect()
}

/// 얕은 클론: 경계 커밋은 부모가 없는 것으로 걷고 행의 부모도 비어 있다(git `%P`). commit-graph 있음 · 없음.
#[test]
fn a_shallow_clone_logs_like_git() {
    let r = TestRepo::with_commit();
    r.git(&["checkout", "--quiet", "-b", "side"]);
    for n in 0..4 {
        r.commit_file("s.txt", &format!("{n}\n"), &format!("side {n}"));
    }
    r.checkout("main");
    for n in 0..4 {
        r.commit_file("m.txt", &format!("{n}\n"), &format!("main {n}"));
    }
    r.git(&["merge", "--no-ff", "--quiet", "-m", "merge side", "side"]);
    let clone = r.base().join("shallow");
    let url = format!("file://{}", r.root().display());
    r.git_in(
        r.base(),
        &[
            "clone",
            "--quiet",
            "--no-local",
            "--depth",
            "3",
            "--no-single-branch",
            &url,
            clone.to_str().unwrap(),
        ],
    );
    assert!(clone.join(".git/shallow").exists());
    for graph in [false, true] {
        if graph {
            r.git_in(&clone, &["commit-graph", "write", "--reachable"]);
        }
        let rows = log(&clone, None, &LogFilter::default(), LogOrder::Topo);
        check_lanes(&rows);
        let theirs = git_hashes(&clone, r.base());
        assert_eq!(hashes_and_parents(&rows), theirs, "commit-graph {graph}");
        assert!(
            rows.iter().any(|r| r.parents.is_empty()),
            "a boundary commit"
        );
        assert!(rows.len() < 10, "the clone is shallow: {}", rows.len());
    }
}

/// 대체 객체(`git replace --graft`): 바꾼 부모로 걷는다. commit-graph 있음 · 없음.
#[test]
fn replaced_commits_are_walked_like_git() {
    let r = TestRepo::with_commit();
    let a = r.commit_file("f.txt", "a\n", "A");
    r.commit_file("f.txt", "b\n", "B");
    r.commit_file("f.txt", "c\n", "C");
    r.git(&["replace", "--graft", "HEAD", &a]);
    for graph in [false, true] {
        if graph {
            r.git(&["commit-graph", "write", "--reachable"]);
        }
        let rows = log(r.root(), None, &LogFilter::default(), LogOrder::Topo);
        let subjects: Vec<&str> = rows.iter().map(|r| r.subject.as_str()).collect();
        assert_eq!(subjects[..2], ["C", "A"], "commit-graph {graph}");
        assert_eq!(
            hashes_and_parents(&rows),
            git_hashes(r.root(), r.base()),
            "commit-graph {graph}"
        );
    }
}
