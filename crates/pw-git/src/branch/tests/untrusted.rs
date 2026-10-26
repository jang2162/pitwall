//! 미신뢰 프로젝트: 체크아웃의 자동 경로(잠금 전에 gix 로 바뀔 경로를 셈하기, 창이 취소한 확인)는 저장소 설정의 프로그램을
//! 돌리지 않는다(결정 M2-D8, 03-architecture.md 「보안」). 압축으로 받은 저장소처럼 `.git/config` 에 `core.fsmonitor` ·
//! `filter.<x>.clean` · `diff.external` · `diff.<x>.textconv` 를 심는다. 사람이 누른 체크아웃은 터미널의 git 과 같이 돈다(D43).

use std::fs;
use std::os::unix::fs::PermissionsExt;

use super::super::*;
use super::unsaved;
use crate::testing::TestRepo;
use crate::testing::write::{Window, WriteFixture};
use crate::write::{Askpass, GitPromptAnswer};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_untrusted_checkout_plans_with_gix_and_runs_no_planted_program() {
    let repo = TestRepo::with_commit();
    repo.commit_file("a.txt", "a\n", "a");
    repo.git(&["checkout", "--quiet", "-b", "feat"]);
    repo.commit_file("a.txt", "a2\n", "feat");
    repo.checkout("main");
    let marks = repo.base().join("marks");
    let bin = repo.base().join("planted");
    fs::create_dir_all(&marks).unwrap();
    fs::create_dir_all(&bin).unwrap();
    let script = |name: &str, body: &str| {
        let path = bin.join(name);
        fs::write(
            &path,
            format!(
                "#!/bin/sh\necho \"$*\" >> '{}'\n{body}\n",
                marks.join(name).display()
            ),
        )
        .unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path.display().to_string()
    };
    for (key, value) in [
        ("core.fsmonitor", script("fsmonitor", "exit 1")),
        ("filter.mark.clean", script("clean", "cat")),
        ("filter.mark.smudge", script("smudge", "cat")),
        ("diff.external", script("external", "exit 0")),
        ("diff.mark.textconv", script("textconv", "cat \"$1\"")),
    ] {
        repo.git(&["config", key, &value]);
    }
    fs::write(
        repo.root().join(".git/info/attributes"),
        "* filter=mark diff=mark\n",
    )
    .unwrap();
    let f = WriteFixture::with_repo(repo, false, Askpass::default());
    assert!(!f.projects.is_trusted(&f.project));
    let listed = |f: &WriteFixture| -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(f.repo.base().join("marks"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    };

    let window = Window::open(&f.write, &f.project, |_| Some(GitPromptAnswer::Cancel));
    let target = GitCheckoutTarget::Branch {
        name: "feat".into(),
    };
    assert_eq!(
        checkout(&f.write, f.project.clone(), None, target.clone()).await,
        Err(crate::write::WriteError::Cancelled),
        "the window cancelled — git did not run"
    );
    assert_eq!(unsaved(&window), [Some(vec!["a.txt".to_owned()])]);
    assert_eq!(listed(&f), Vec::<String>::new());
    drop(window);

    // 사람이 누른 체크아웃은 돈다 — 터미널의 git 과 같이 저장소의 smudge 필터가 돈다(D43).
    let _window = Window::open(&f.write, &f.project, |_| Some(GitPromptAnswer::Proceed));
    let (_, head) = checkout(&f.write, f.project.clone(), None, target)
        .await
        .unwrap();
    assert!(matches!(head, RepoHead::Branch { name, .. } if name == "feat"));
    assert!(
        listed(&f).contains(&"smudge".to_owned()),
        "the planted smudge filter is live: {:?}",
        listed(&f)
    );
}
