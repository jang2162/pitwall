//! 미신뢰 프로젝트에서 로그의 어느 경로도 저장소가 심은 프로그램을 돌리지 않는다(결정 M2-D8, 03-architecture.md 「보안」).
//! 압축으로 받은 저장소처럼 `.git/config` 에 `core.fsmonitor` · `filter.<x>.clean` · `diff.external` · `diff.<x>.textconv` 를 심고
//! (부를 때마다 `marks/<이름>` 에 표식을 남긴다) 작업 트리를 더럽힌 뒤, `git.log`(그래프) · 경로 필터(트리 읽기) · 메시지 ·
//! 작성자 필터(커밋 읽기) · `git.logPage` 를 부른다. 표식이 없어야 한다. 대조: 같은 저장소에서 git CLI 로 읽으면 표식이 생긴다 —
//! 심은 프로그램이 정말 돈다는 것을 보여, 위의 「없다」 가 시험의 무력함이 아님을 보인다.

use std::os::unix::fs::PermissionsExt;

use pw_rpc::protocol::StreamOutcome;

use super::rpc::Fx;
use super::*;
use crate::log::methods::{GitLogPageParams, GitLogParams};
use crate::testing::TestRepo;

fn plant(r: &TestRepo) -> PathBuf {
    let marks = r.base().join("marks");
    let bin = r.base().join("planted");
    std::fs::create_dir_all(&marks).unwrap();
    std::fs::create_dir_all(&bin).unwrap();
    let script = |name: &str, body: &str| {
        let path = bin.join(name);
        std::fs::write(
            &path,
            format!(
                "#!/bin/sh\necho \"$*\" >> '{}'\n{body}\n",
                marks.join(name).display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path.display().to_string()
    };
    let fsmonitor = script("fsmonitor", "exit 1");
    let clean = script("clean", "cat");
    let external = script("external", "exit 0");
    let textconv = script("textconv", "cat \"$1\"");
    for (key, value) in [
        ("core.fsmonitor", fsmonitor.as_str()),
        ("filter.mark.clean", clean.as_str()),
        ("diff.external", external.as_str()),
        ("diff.mark.textconv", textconv.as_str()),
    ] {
        r.git(&["config", key, value]);
    }
    std::fs::write(
        r.root().join(".git/info/attributes"),
        "* filter=mark diff=mark\n",
    )
    .unwrap();
    marks
}

fn marks(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_untrusted_repository_runs_none_of_its_programs_on_log_reads() {
    let r = TestRepo::with_commit();
    r.commit_file("src/a.txt", "1\n", "feat: a");
    r.git(&["checkout", "--quiet", "-b", "side"]);
    r.commit_file("src/b.txt", "1\n", "fix: b");
    r.checkout("main");
    r.git(&["merge", "--no-ff", "--quiet", "-m", "merge side", "side"]);
    let marks_dir = plant(&r);
    r.write("src/a.txt", "dirty\n");
    let fx = Fx::new();
    let id = fx.project(r.root());
    assert!(!fx.projects.is_trusted(&id));

    let base = GitLogParams {
        project_id: id.clone(),
        ..GitLogParams::default()
    };
    let requests = [
        base.clone(),
        GitLogParams {
            filter: LogFilter {
                paths: vec!["src/a.txt".into(), "src".into()],
                ..LogFilter::default()
            },
            ..base.clone()
        },
        GitLogParams {
            filter: LogFilter {
                message: Some("^fix".into()),
                author: Some("Pitwall".into()),
                regex: true,
                ..LogFilter::default()
            },
            refs: Some(vec!["side".into(), "HEAD".into()]),
            ..base.clone()
        },
    ];
    for p in &requests {
        let (chunks, outcome) = fx.stream(p).await;
        assert_eq!(outcome, StreamOutcome::Completed, "{p:?}");
        assert!(
            chunks.iter().map(|c| c.rows.len()).sum::<usize>() > 0,
            "{p:?}"
        );
        let page = fx
            .page(&GitLogPageParams {
                project_id: id.clone(),
                refs: p.refs.clone(),
                filter: p.filter.clone(),
                limit: 2,
                ..GitLogPageParams::default()
            })
            .await
            .unwrap();
        assert!(!page.rows.is_empty());
    }
    assert_eq!(marks(&marks_dir), Vec::<String>::new());

    // 대조: git CLI 로 같은 저장소를 읽으면 심은 프로그램이 돈다.
    r.git(&["status", "--porcelain"]);
    r.git(&["diff"]);
    assert_eq!(marks(&marks_dir), ["clean", "external", "fsmonitor"]);
}
