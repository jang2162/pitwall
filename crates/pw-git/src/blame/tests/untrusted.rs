//! 미신뢰 프로젝트에서 `git.blame` · `git.fileHistory` 는 git CLI 를 띄우지 않고 저장소가 심은 프로그램도 돌리지 않는다(결정
//! M2-D8, 03-architecture.md 「보안」). `.git/config` 에 `core.fsmonitor` · `filter.<x>.clean`/`smudge` · `diff.external` ·
//! `diff.<x>.textconv` 를 심고(부를 때마다 `marks/<이름>` 에 표식), `[git] path` 를 부를 때마다 표식을 남기는 가짜 git 으로 둔다.
//! 표식이 없어야 한다. 대조: 신뢰하면 blame 이 그 git 을 부르고(표식이 생긴다), 같은 저장소에서 진짜 git 으로 읽으면 심은
//! 프로그램이 돈다 — 위의 「없다」 가 시험의 무력함이 아님을 보인다.

use std::os::unix::fs::PermissionsExt;

use pw_rpc::protocol::StreamOutcome;

use super::rpc::Fx;
use super::*;
use crate::blame::methods::GitBlameParams;
use crate::file_history::FileHistoryChunk;
use crate::file_history::methods::GitFileHistoryParams;
use crate::testing::TestRepo;

/// 심은 프로그램과 가짜 git. (표식 폴더, 가짜 git 경로).
fn plant(r: &TestRepo) -> (std::path::PathBuf, std::path::PathBuf) {
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
        path
    };
    for (key, name, body) in [
        ("core.fsmonitor", "fsmonitor", "exit 1"),
        ("filter.mark.clean", "clean", "cat"),
        ("filter.mark.smudge", "smudge", "cat"),
        ("diff.external", "external", "exit 0"),
        ("diff.mark.textconv", "textconv", "cat \"$1\""),
    ] {
        let path = script(name, body);
        r.git(&["config", key, path.to_str().unwrap()]);
    }
    std::fs::write(
        r.root().join(".git/info/attributes"),
        "* filter=mark diff=mark\n",
    )
    .unwrap();
    // 받은 인자를 적고 진짜 git 으로 넘기는 git.
    let real = crate::testing::write::which_git();
    let fake = script("git", &format!("exec '{}' \"$@\"", real.display()));
    (marks, fake)
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
async fn an_untrusted_repository_runs_no_git_and_none_of_its_programs_on_blame_reads() {
    let r = TestRepo::new();
    let fx_repo = r.blame_fixture();
    let (marks_dir, fake) = plant(&r);
    r.write(fx_repo.path, "dirty\n");
    let fx = Fx::new(&format!("[git]\npath = \"{}\"\n", fake.display()));
    let id = fx.project(r.root());
    assert!(!fx.projects.is_trusted(&id));

    let blame = GitBlameParams {
        project_id: id.clone(),
        worktree: None,
        path: fx_repo.path.into(),
        rev: None,
    };
    let (chunks, outcome) = fx.stream::<_, BlameChunk>("git.blame", &blame).await;
    assert_eq!(outcome, StreamOutcome::Completed);
    assert_eq!(expand(&chunks).0.engine, BlameEngine::Gix);
    let history = GitFileHistoryParams {
        project_id: id.clone(),
        worktree: None,
        path: fx_repo.path.into(),
        rev: None,
    };
    let (chunks, outcome) = fx
        .stream::<_, FileHistoryChunk>("git.fileHistory", &history)
        .await;
    assert_eq!(outcome, StreamOutcome::Completed);
    assert_eq!(chunks.iter().map(|c| c.rows.len()).sum::<usize>(), 8);
    assert_eq!(marks(&marks_dir), Vec::<String>::new(), "nothing ran");

    // 대조 ①: 신뢰하면 blame 은 그 git 을 부른다.
    fx.trust(&id, true);
    let (chunks, outcome) = fx.stream::<_, BlameChunk>("git.blame", &blame).await;
    assert_eq!(outcome, StreamOutcome::Completed);
    assert_eq!(expand(&chunks).0.engine, BlameEngine::Git);
    assert!(marks(&marks_dir).contains(&"git".to_owned()));
    // 대조 ②: 진짜 git 으로 읽으면 심은 프로그램이 돈다.
    let _ = r.try_git_in(r.root(), &["status"]);
    let _ = r.try_git_in(r.root(), &["diff"]);
    let seen = marks(&marks_dir);
    assert!(seen.iter().any(|m| m != "git"), "{seen:?}");
}
