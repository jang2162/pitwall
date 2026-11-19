//! git 조회 툴 메서드(16-m5-plan M5-05)의 시험. 저장소는 진짜 git 으로 임시 폴더에 만들고(`crate::testing`), 툴은 앱처럼
//! `status::register` 로 붙인 라우터에 MCP 세션의 호출자(`Caller::agent`)로 부른다. 이름 · 그룹 · 등급 · 스키마와 세션의 워크트리 ·
//! 응답 상한은 진짜 MCP 세션으로 데스크톱 크레이트의 `mcp_tests::git` 이 본다.

use std::sync::Arc;

use pw_rpc::{Caller, EventHub, Role, Router, RpcError, Session, code};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use super::RESPONSE_BUDGET;
use super::blame::GitBlameToolResult;
use super::branches::GitBranchListToolResult;
use super::diff::GitDiffToolResult;
use super::log::GitLogToolResult;
use super::status::GitStatusToolResult;
use crate::blame::BlameEngine;
use crate::blame::tests::rpc::{Fx, next};
use crate::methods::{RepoHead, RepoOperation};
use crate::status::{GitChange, GitTrack, Status};
use crate::testing::TestRepo;

/// 툴을 붙인 라우터를 부르는 쪽.
struct Tools {
    fx: Fx,
    router: Arc<Router>,
}

impl Tools {
    fn new(settings: &str) -> Self {
        let fx = Fx::new(settings);
        let mut router = Router::new();
        crate::status::register(&mut router, Arc::new(Status::new(Arc::clone(&fx.git))));
        Self {
            fx,
            router: Arc::new(router),
        }
    }

    /// MCP 세션의 호출자로 부른다.
    async fn call<R: DeserializeOwned>(&self, method: &str, params: Value) -> Result<R, RpcError> {
        self.call_as(Caller::agent(Role::Owner), method, &params)
            .await
    }

    async fn call_as<P: Serialize, R: DeserializeOwned>(
        &self,
        caller: Caller,
        method: &str,
        params: &P,
    ) -> Result<R, RpcError> {
        let (session, mut rx) = Session::new(Arc::clone(&self.router), EventHub::new(), caller);
        session
            .handle_value(json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}));
        next(&mut rx).await.into_response().unwrap().result_as()
    }
}

/// invalid params 의 까닭 글(`data`).
fn invalid(e: RpcError) -> String {
    assert_eq!(e.code, code::INVALID_PARAMS, "{e:?}");
    match e.data {
        Some(Value::String(detail)) => detail,
        other => panic!("{other:?}"),
    }
}

const TOOLS: [(&str, &str); 5] = [
    ("git.tool.status", "{}"),
    ("git.tool.log", "{}"),
    ("git.tool.diff", "{}"),
    (
        "git.tool.blame",
        r#"{"path": "README.md", "startLine": 1, "endLine": 1}"#,
    ),
    ("git.tool.branchList", "{}"),
];

fn with_project(extra: &str, project: &str) -> Value {
    let mut params: Value = serde_json::from_str(extra).unwrap();
    params["projectId"] = json!(project);
    params
}

/// 툴은 MCP 세션만 부른다(`agent_grant`) — 창 · Host 범위의 호출자는 거절, 없는 프로젝트는 그 관문의 invalid params 그대로.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tools_pass_the_agent_gate_first() {
    let r = TestRepo::with_commit();
    let t = Tools::new("");
    let id = t.fx.project(r.root());
    for (method, extra) in TOOLS {
        let params = with_project(extra, &id);
        for caller in [
            Caller::new(Role::Owner),
            Caller {
                scope: pw_rpc::ConnectionScope::Project(id.clone()),
                ..Caller::new(Role::Owner)
            },
        ] {
            let e = t
                .call_as::<_, Value>(caller, method, &params)
                .await
                .unwrap_err();
            assert_eq!(e.code, code::PERMISSION_DENIED, "{method}: {e:?}");
            assert_eq!(e.data.unwrap()["reason"], "notAgentSession", "{method}");
        }
        t.call::<Value>(method, params)
            .await
            .unwrap_or_else(|e| panic!("{method}: {e:?}"));
        let e = t
            .call::<Value>(method, with_project(extra, "nope"))
            .await
            .unwrap_err();
        assert_eq!(e.code, code::INVALID_PARAMS, "{method}: {e:?}");
    }
}

/// `worktree` 를 주면 메인 체크아웃이 아니라 그 워크트리의 상태다. 요약의 HEAD · upstream · ahead/behind · 수.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn git_status_reads_the_given_worktree_and_summarizes() {
    let r = TestRepo::with_commit();
    r.git(&["branch", "base"]);
    r.commit_file("a.txt", "a\n", "a");
    r.commit_file("b.txt", "b\n", "b");
    r.git(&["branch", "--set-upstream-to=base", "main"]);
    let wt = r.add_worktree("wt", "feat");
    std::fs::write(wt.join("only-in-wt.txt"), "x\n").unwrap();
    r.write("README.md", "changed\n");
    r.write("staged.txt", "s\n");
    r.git(&["add", "staged.txt"]);
    r.write("new.txt", "n\n");
    let t = Tools::new("");
    let id = t.fx.project(r.root());

    let main: GitStatusToolResult = t
        .call("git.tool.status", json!({"projectId": id}))
        .await
        .unwrap();
    assert_eq!(main.work_tree.as_deref(), r.root().to_str());
    let paths: Vec<_> = main.files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(paths, ["README.md", "new.txt", "staged.txt"]);
    let s = &main.summary;
    assert!(matches!(&s.head, Some(RepoHead::Branch { name, .. }) if name == "main"));
    let upstream = s.upstream.as_ref().expect("an upstream");
    assert_eq!(upstream.name, "base");
    assert_eq!(
        upstream.track,
        Some(GitTrack {
            ahead: 2,
            behind: 0
        })
    );
    assert_eq!(
        (s.staged, s.unstaged, s.untracked, s.conflicted, s.operation),
        (1, 1, 1, 0, None)
    );
    assert_eq!(main.next_cursor, None);

    let other: GitStatusToolResult = t
        .call(
            "git.tool.status",
            json!({"projectId": id, "worktree": wt.display().to_string()}),
        )
        .await
        .unwrap();
    assert_eq!(other.work_tree.as_deref(), wt.to_str());
    let paths: Vec<_> = other.files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(paths, ["only-in-wt.txt"]);
    assert_eq!(other.files[0].unstaged, Some(GitChange::Untracked));
    assert!(matches!(&other.summary.head, Some(RepoHead::Branch { name, .. }) if name == "feat"));
    assert_eq!(other.summary.upstream, None);
}

/// 충돌 중인 병합: 충돌 수와 진행 중 작업.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn git_status_counts_conflicts_and_the_operation_in_progress() {
    let r = TestRepo::with_commit();
    r.branch("side");
    r.commit_file("README.md", "main\n", "main edit");
    r.checkout("side");
    r.commit_file("README.md", "side\n", "side edit");
    r.checkout("main");
    assert!(!r.try_git_in(r.root(), &["merge", "side"]).status.success());
    let t = Tools::new("");
    let id = t.fx.project(r.root());
    let got: GitStatusToolResult = t
        .call("git.tool.status", json!({"projectId": id}))
        .await
        .unwrap();
    let s = got.summary;
    assert_eq!(
        (s.conflicted, s.staged, s.operation),
        (1, 0, Some(RepoOperation::Merge))
    );
}

/// 쪽: `limit` 개씩, 커서가 이어지고 겹치지 않는다. `limit` 의 범위는 invalid params.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn git_status_pages_files_by_path() {
    let r = TestRepo::with_commit();
    for name in ["e", "a", "d", "b", "c"] {
        r.write(&format!("{name}.txt"), "x\n");
    }
    let t = Tools::new("");
    let id = t.fx.project(r.root());
    let mut cursor = Value::Null;
    let mut pages = Vec::new();
    loop {
        let page: GitStatusToolResult = t
            .call(
                "git.tool.status",
                json!({"projectId": id, "limit": 2, "cursor": cursor}),
            )
            .await
            .unwrap();
        assert_eq!(page.summary.untracked, 5, "the summary counts every file");
        pages.push(
            page.files
                .iter()
                .map(|f| f.path.clone())
                .collect::<Vec<_>>(),
        );
        match page.next_cursor {
            Some(c) => cursor = json!(c),
            None => break,
        }
    }
    assert_eq!(
        pages,
        [
            vec!["a.txt", "b.txt"],
            vec!["c.txt", "d.txt"],
            vec!["e.txt"]
        ]
    );
    for limit in [0, 1001] {
        let e = t
            .call::<Value>("git.tool.status", json!({"projectId": id, "limit": limit}))
            .await
            .unwrap_err();
        invalid(e);
    }
}

/// 커밋 120개 · `limit` 50 → 세 쪽(50 · 50 · 20), 겹침 없이 HEAD 의 이력 전부. 기본 `limit` 은 50.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn git_log_pages_through_history_without_overlap() {
    let r = TestRepo::new();
    for i in 0..120 {
        r.git(&["commit", "--quiet", "--allow-empty", "-m", &format!("c{i}")]);
    }
    let t = Tools::new("");
    let id = t.fx.project(r.root());
    let mut cursor = Value::Null;
    let mut sizes = Vec::new();
    let mut seen = Vec::new();
    loop {
        let page: GitLogToolResult = t
            .call(
                "git.tool.log",
                json!({"projectId": id, "limit": 50, "cursor": cursor}),
            )
            .await
            .unwrap();
        sizes.push(page.commits.len());
        seen.extend(page.commits.iter().map(|c| c.subject.clone()));
        assert!(page.commits.iter().all(|c| c.body.is_none()));
        match page.next_cursor {
            Some(c) => cursor = json!(c),
            None => break,
        }
    }
    assert_eq!(sizes, [50, 50, 20]);
    let expected: Vec<String> = (0..120).rev().map(|i| format!("c{i}")).collect();
    assert_eq!(seen, expected);

    let first: GitLogToolResult = t
        .call("git.tool.log", json!({"projectId": id}))
        .await
        .unwrap();
    assert_eq!(first.commits.len(), 50);
    for limit in [0, 501] {
        let e = t
            .call::<Value>("git.tool.log", json!({"projectId": id, "limit": limit}))
            .await
            .unwrap_err();
        invalid(e);
    }
}

/// 필터(브랜치 · 작성자 · 메시지 · 기간 · 경로)와 `detail` 의 본문. HEAD 에 커밋이 없으면 빈 쪽.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn git_log_filters_and_details() {
    let r = TestRepo::new();
    let t = Tools::new("");
    let id = t.fx.project(r.root());
    let empty: GitLogToolResult = t
        .call("git.tool.log", json!({"projectId": id}))
        .await
        .unwrap();
    assert_eq!((empty.commits.len(), empty.next_cursor), (0, None));

    let first = r.commit_file("a.txt", "a\n", "add a\n\nwhy we add a");
    r.git(&[
        "commit",
        "--quiet",
        "--allow-empty",
        "--author=Other Person <other@pitwall.invalid>",
        "-m",
        "Fix the parser",
    ]);
    r.branch("side");
    r.checkout("side");
    r.commit_file("side.txt", "s\n", "side only");
    r.checkout("main");
    let last = r.commit_file("b.txt", "b\n", "add b");
    let time = |rev: &str| -> i64 {
        r.git(&["log", "-1", "--format=%ct", rev])
            .trim()
            .parse()
            .unwrap()
    };
    let subjects = |filters: Value| {
        let mut params = filters;
        params["projectId"] = json!(id);
        let t = &t;
        async move {
            let got: GitLogToolResult = t.call("git.tool.log", params).await.unwrap();
            got.commits
                .into_iter()
                .map(|c| c.subject)
                .collect::<Vec<_>>()
        }
    };
    assert_eq!(
        subjects(json!({})).await,
        ["add b", "Fix the parser", "add a"]
    );
    assert_eq!(
        subjects(json!({"branch": "side"})).await,
        ["side only", "Fix the parser", "add a"]
    );
    assert_eq!(
        subjects(json!({"author": "other PERSON"})).await,
        ["Fix the parser"]
    );
    assert_eq!(
        subjects(json!({"message": "PARSER"})).await,
        ["Fix the parser"]
    );
    assert_eq!(subjects(json!({"paths": ["a.txt"]})).await, ["add a"]);
    assert_eq!(
        subjects(json!({"since": format!("@{}", time(&last))})).await,
        ["add b"]
    );
    assert_eq!(
        subjects(json!({"until": format!("@{}", time(&first))})).await,
        ["add a"]
    );
    let e = t
        .call::<Value>(
            "git.tool.log",
            json!({"projectId": id, "since": "not a date"}),
        )
        .await
        .unwrap_err();
    let m = invalid(e);
    assert!(m.contains("since"), "{m}");

    let detailed: GitLogToolResult = t
        .call("git.tool.log", json!({"projectId": id, "detail": true}))
        .await
        .unwrap();
    let bodies: Vec<_> = detailed.commits.iter().map(|c| c.body.as_deref()).collect();
    assert_eq!(bodies, [Some(""), Some(""), Some("why we add a")]);
    let head = &detailed.commits[0];
    assert_eq!(head.hash, last);
    assert_eq!(head.refs, ["main"]);
    assert_eq!(head.author_name, "Pitwall Test");
    assert_eq!(head.parents.len(), 1);
}

/// 기본은 파일 목록과 줄 수만, `detail` · `path` 면 hunk(unified diff).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn git_diff_lists_files_and_adds_hunks_on_request() {
    let r = TestRepo::with_commit();
    r.commit_file("a.txt", "1\n2\n3\n", "a");
    r.write("a.txt", "1\nTWO\n3\n");
    r.write("README.md", "bye\n");
    let t = Tools::new("");
    let id = t.fx.project(r.root());

    let plain: GitDiffToolResult = t
        .call("git.tool.diff", json!({"projectId": id}))
        .await
        .unwrap();
    let summary: Vec<_> = plain
        .files
        .iter()
        .map(|f| (f.path.as_str(), f.added, f.removed, f.patch.is_some()))
        .collect();
    assert_eq!(
        summary,
        [("README.md", 1, 1, false), ("a.txt", 1, 1, false)]
    );
    assert!(!plain.truncated);

    let detail: GitDiffToolResult = t
        .call("git.tool.diff", json!({"projectId": id, "detail": true}))
        .await
        .unwrap();
    assert_eq!(
        detail.files[1].patch.as_deref(),
        Some("@@ -1,3 +1,3 @@\n 1\n-2\n+TWO\n 3\n")
    );
    assert_eq!(
        detail.files[0].patch.as_deref(),
        Some("@@ -1 +1 @@\n-hello\n+bye\n")
    );

    let one: GitDiffToolResult = t
        .call("git.tool.diff", json!({"projectId": id, "path": "a.txt"}))
        .await
        .unwrap();
    assert_eq!(one.files.len(), 1);
    assert!(one.files[0].patch.is_some());

    // 커밋 하나(첫 부모와 비교).
    let head = r.head();
    let commit: GitDiffToolResult = t
        .call(
            "git.tool.diff",
            json!({"projectId": id, "to": {"kind": "commit", "rev": "HEAD"}}),
        )
        .await
        .unwrap();
    assert_eq!(commit.new_commit.as_deref(), Some(head.as_str()));
    assert_eq!(commit.files[0].path, "a.txt");
}

/// 큰 diff: hunk 가 예산을 넘으면 그 파일부터는 목록만 싣고 `truncated` — 결과가 예산 안이다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn git_diff_truncates_hunks_at_the_budget() {
    let r = TestRepo::with_commit();
    let body = |tag: &str| -> String {
        (0..200)
            .map(|i| format!("{tag} line {i} with enough words to weigh something\n"))
            .collect()
    };
    for i in 0..40 {
        r.write(&format!("f{i:02}.txt"), &body("old"));
    }
    r.commit_all("files");
    for i in 0..40 {
        r.write(&format!("f{i:02}.txt"), &body("new"));
    }
    let t = Tools::new("");
    let id = t.fx.project(r.root());
    let got: Value = t
        .call("git.tool.diff", json!({"projectId": id, "detail": true}))
        .await
        .unwrap();
    let size = serde_json::to_vec(&got).unwrap().len();
    let got: GitDiffToolResult = serde_json::from_value(got).unwrap();
    assert!(got.truncated);
    assert_eq!(got.omitted_files, 0);
    assert_eq!(got.files.len(), 40, "every file is still listed");
    let with_patch = got.files.iter().filter(|f| f.patch.is_some()).count();
    assert!((1..40).contains(&with_patch), "{with_patch}");
    assert!(got.files.iter().all(|f| f.added == 200 && f.removed == 200));
    assert!(size <= RESPONSE_BUDGET + 1024, "{size}");
}

/// 줄 범위로 자른 blame. 범위 밖 · 500줄 넘음 · 없는 파일은 invalid params.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn git_blame_answers_a_line_range() {
    let r = TestRepo::new();
    let fx_repo = r.blame_fixture();
    let t = Tools::new("");
    let id = t.fx.project(r.root());
    let blame = |start: u32, end: u32| {
        let t = &t;
        let id = id.clone();
        async move {
            t.call::<GitBlameToolResult>(
                "git.tool.blame",
                json!({"projectId": id, "path": fx_repo.path, "startLine": start, "endLine": end}),
            )
            .await
        }
    };
    let got = blame(1, 31).await.unwrap();
    assert_eq!((got.lines, got.engine), (31, BlameEngine::Gix));
    assert_eq!(got.commit, r.head());
    assert_eq!(got.ranges.first().unwrap().start_line, 1);
    assert_eq!(got.ranges.last().unwrap().end_line, 31);
    for pair in got.ranges.windows(2) {
        assert_eq!(pair[0].end_line + 1, pair[1].start_line, "{pair:?}");
    }

    let two = blame(2, 2).await.unwrap();
    assert_eq!(two.ranges.len(), 1);
    let line = &two.ranges[0];
    assert_eq!((line.start_line, line.end_line), (2, 2));
    assert_eq!(line.subject, "main edit 2");
    assert_eq!(line.author_name, "Pitwall Test");
    assert_eq!(line.source_path, "b.txt");
    let five = blame(4, 6).await.unwrap();
    let subjects: Vec<_> = five
        .ranges
        .iter()
        .map(|r| (r.start_line, r.end_line, r.subject.as_str()))
        .collect();
    assert_eq!(
        subjects,
        [(4, 4, "add a"), (5, 5, "edit 5"), (6, 6, "add a")]
    );
    assert_eq!(blame(31, 31).await.unwrap().ranges[0].subject, "append");

    let m = invalid(blame(30, 32).await.unwrap_err());
    assert!(m.contains("31 lines"), "{m}");
    invalid(blame(0, 1).await.unwrap_err());
    invalid(blame(3, 2).await.unwrap_err());
    assert!(invalid(blame(1, 501).await.unwrap_err()).contains("500"));
    blame(1, 500).await.unwrap_err(); // 500줄은 묻을 수 있지만 이 파일은 31줄이다.
    let missing = t
        .call::<Value>(
            "git.tool.blame",
            json!({"projectId": id, "path": "nope.txt", "startLine": 1, "endLine": 1}),
        )
        .await
        .unwrap_err();
    assert!(invalid(missing).contains("nope.txt"));
}

/// 로컬 · 원격 브랜치와 `includeTags`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn git_branch_list_lists_branches_and_tags_on_request() {
    let r = TestRepo::with_commit();
    r.branch("feat");
    r.git(&["tag", "v1"]);
    r.git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
    let t = Tools::new("");
    let id = t.fx.project(r.root());
    let got: GitBranchListToolResult = t
        .call("git.tool.branchList", json!({"projectId": id}))
        .await
        .unwrap();
    let branches: Vec<_> = got
        .branches
        .iter()
        .map(|b| (b.name.as_str(), b.head, b.commit == r.head()))
        .collect();
    assert_eq!(branches, [("feat", false, true), ("main", true, true)]);
    let remotes: Vec<_> = got
        .remote_branches
        .iter()
        .map(|b| b.name.as_str())
        .collect();
    assert_eq!(remotes, ["origin/main"]);
    assert_eq!(got.tags, None);
    assert!(!got.truncated);

    let tagged: GitBranchListToolResult = t
        .call(
            "git.tool.branchList",
            json!({"projectId": id, "includeTags": true}),
        )
        .await
        .unwrap();
    let tags: Vec<_> = tagged.tags.unwrap().into_iter().map(|t| t.name).collect();
    assert_eq!(tags, ["v1"]);
}

/// 미신뢰 프로젝트에서 `git_status` · `git_blame` 은 git CLI 도 저장소가 심은 프로그램도 띄우지 않는다(03-architecture.md 「보안」).
/// 대조: 신뢰하면 `git_blame` 이 그 git 을 부른다(표식이 생긴다) — 위의 「없다」 가 시험의 무력함이 아니다.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn untrusted_projects_run_no_process_through_the_tools() {
    use crate::blame::tests::untrusted::{marks, plant};

    let r = TestRepo::new();
    let fx_repo = r.blame_fixture();
    let (marks_dir, fake) = plant(&r);
    // 내용은 그대로이고 stat(시각)만 바뀐 추적 파일 — status 가 내용을 견주려면 clean 필터를 거쳐야 한다(크기가 달라지면 내용을
    // 읽지 않고 바뀐 것으로 본다).
    let later = std::time::SystemTime::now() + std::time::Duration::from_secs(5);
    std::fs::File::options()
        .write(true)
        .open(r.root().join(fx_repo.path))
        .unwrap()
        .set_modified(later)
        .unwrap();
    let t = Tools::new(&format!("[git]\npath = \"{}\"\n", fake.display()));
    let id = t.fx.project(r.root());
    assert!(!t.fx.projects.is_trusted(&id));

    let status: GitStatusToolResult = t
        .call("git.tool.status", json!({"projectId": id}))
        .await
        .unwrap();
    assert!(status.work_tree.is_some());
    let blame = json!({"projectId": id, "path": fx_repo.path, "startLine": 1, "endLine": 3});
    let got: GitBlameToolResult = t.call("git.tool.blame", blame.clone()).await.unwrap();
    assert_eq!(got.engine, BlameEngine::Gix);
    assert_eq!(marks(&marks_dir), Vec::<String>::new(), "nothing ran");

    t.fx.trust(&id, true);
    let got: GitBlameToolResult = t.call("git.tool.blame", blame).await.unwrap();
    assert_eq!(got.engine, BlameEngine::Git);
    assert!(marks(&marks_dir).contains(&"git".to_owned()));
}

/// 예산으로 끊는 길(결정 M5-D34): 잘린 쪽은 예산 안이고, 끊은 자리의 다음 표지로 이어 부르면 빠짐 · 겹침 없이 전부가 온다.
fn within_budget(value: &impl Serialize) {
    let size = serde_json::to_vec(value).unwrap().len();
    assert!(size <= RESPONSE_BUDGET + 1024, "{size}");
}

/// `detail` 의 긴 본문으로 쪽이 예산에 닿으면 그 쪽의 마지막 커밋이 커서다 — 엔진의 커서(`limit` 개 뒤)를 그대로 내면 뒤 커밋이
/// 조용히 빠진다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn git_log_cut_at_the_budget_continues_from_the_last_commit_it_sent() {
    let r = TestRepo::new();
    let body = "b".repeat(15_000);
    for i in 0..30 {
        r.git(&[
            "commit",
            "--quiet",
            "--allow-empty",
            "-m",
            &format!("c{i}"),
            "-m",
            &body,
        ]);
    }
    let t = Tools::new("");
    let id = t.fx.project(r.root());
    let mut cursor = Value::Null;
    let mut sizes = Vec::new();
    let mut seen = Vec::new();
    loop {
        let page: Value = t
            .call(
                "git.tool.log",
                json!({"projectId": id, "detail": true, "limit": 50, "cursor": cursor}),
            )
            .await
            .unwrap();
        within_budget(&page);
        let page: GitLogToolResult = serde_json::from_value(page).unwrap();
        sizes.push(page.commits.len());
        seen.extend(page.commits.iter().map(|c| c.subject.clone()));
        assert!(
            page.commits
                .iter()
                .all(|c| c.body.as_deref() == Some(body.as_str()))
        );
        match page.next_cursor {
            Some(c) => {
                assert_eq!(Some(&c), page.commits.last().map(|c| &c.hash));
                cursor = json!(c);
            }
            None => break,
        }
    }
    assert!(
        sizes.len() > 1 && sizes[0] < 30,
        "the budget cut the page: {sizes:?}"
    );
    let expected: Vec<String> = (0..30).rev().map(|i| format!("c{i}")).collect();
    assert_eq!(seen, expected);
}

/// 긴 경로의 파일이 많으면 `limit` 전에 예산에서 끊기고, 커서로 이어 부르면 전부가 한 번씩 온다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn git_status_cut_at_the_budget_continues_by_cursor() {
    let r = TestRepo::with_commit();
    let dir = format!("{}/{}", "d".repeat(240), "e".repeat(240));
    let names: Vec<String> = (0..300)
        .map(|i| format!("{dir}/{i:03}{}", "f".repeat(240)))
        .collect();
    for name in &names {
        r.write(name, "x\n");
    }
    let t = Tools::new("");
    let id = t.fx.project(r.root());
    let mut cursor = Value::Null;
    let mut sizes = Vec::new();
    let mut seen = Vec::new();
    loop {
        let page: Value = t
            .call(
                "git.tool.status",
                json!({"projectId": id, "limit": 1000, "cursor": cursor}),
            )
            .await
            .unwrap();
        within_budget(&page);
        let page: GitStatusToolResult = serde_json::from_value(page).unwrap();
        assert_eq!(page.summary.untracked, 300);
        sizes.push(page.files.len());
        seen.extend(page.files.into_iter().map(|f| f.path));
        match page.next_cursor {
            Some(c) => {
                assert_eq!(Some(&c), seen.last());
                cursor = json!(c);
            }
            None => break,
        }
    }
    assert!(
        sizes.len() > 1 && sizes[0] < 300,
        "the budget cut the page: {sizes:?}"
    );
    assert_eq!(seen, names);
}

/// 브랜치가 많으면 예산 안의 앞쪽만 싣고 `truncated` — 뒤의 목록(태그)도 싣지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn git_branch_list_cut_at_the_budget_says_truncated() {
    use std::io::Write as _;

    let r = TestRepo::with_commit();
    let head = r.head();
    let mut update = r
        .command_in(r.root())
        .args(["update-ref", "--stdin"])
        .stdin(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = update.stdin.take().unwrap();
    for i in 0..600 {
        writeln!(stdin, "create refs/heads/{i:03}{} {head}", "n".repeat(180)).unwrap();
    }
    drop(stdin);
    assert!(update.wait().unwrap().success());
    r.git(&["tag", "v1"]);
    let t = Tools::new("");
    let id = t.fx.project(r.root());
    let got: Value = t
        .call(
            "git.tool.branchList",
            json!({"projectId": id, "includeTags": true}),
        )
        .await
        .unwrap();
    within_budget(&got);
    let got: GitBranchListToolResult = serde_json::from_value(got).unwrap();
    assert!(got.truncated);
    assert!(
        (1..600).contains(&got.branches.len()),
        "{}",
        got.branches.len()
    );
    assert_eq!(
        got.tags,
        Some(Vec::new()),
        "a cut list carries nothing after it"
    );
}

/// 줄마다 다른 커밋(긴 제목)이면 500줄 안에서도 예산에 닿는다 — 앞쪽만 싣고 `truncated` · `nextLine`, 그 줄부터 이어 부르면
/// 빈틈없이 덮는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn git_blame_cut_at_the_budget_says_where_to_continue() {
    const LINES: u32 = 200;
    let r = TestRepo::new();
    let mut lines: Vec<String> = (0..LINES).map(|i| format!("line {i}")).collect();
    let text = |lines: &[String]| lines.iter().map(|l| format!("{l}\n")).collect::<String>();
    r.commit_file("f.txt", &text(&lines), "add");
    for i in 0..LINES as usize {
        lines[i] = format!("changed {i}");
        r.write("f.txt", &text(&lines));
        r.git(&[
            "commit",
            "--quiet",
            "-am",
            &format!("{i:03} {}", "s".repeat(1000)),
        ]);
    }
    let t = Tools::new("");
    let id = t.fx.project(r.root());
    let mut start = 1;
    let mut covered = Vec::new();
    let mut calls = 0;
    loop {
        calls += 1;
        let got: Value = t
            .call(
                "git.tool.blame",
                json!({"projectId": id, "path": "f.txt", "startLine": start, "endLine": LINES}),
            )
            .await
            .unwrap();
        within_budget(&got);
        let got: GitBlameToolResult = serde_json::from_value(got).unwrap();
        covered.extend(got.ranges.iter().map(|r| (r.start_line, r.end_line)));
        assert_eq!(got.truncated, got.next_line.is_some());
        match got.next_line {
            Some(next) => {
                assert_eq!(next, got.ranges.last().unwrap().end_line + 1);
                start = next;
            }
            None => break,
        }
    }
    assert!(calls > 1, "the budget cut the answer");
    let expected: Vec<(u32, u32)> = (1..=LINES).map(|l| (l, l)).collect();
    assert_eq!(covered, expected);
}
