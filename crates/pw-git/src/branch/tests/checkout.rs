//! 체크아웃: 로컬 브랜치(미저장 확인에 바뀔 경로만), 원격 브랜치 → 추적 브랜치, 커밋 → 분리된 HEAD, 바뀔 경로가 없으면 묻지
//! 않음, git 의 거절 원문, 잠금을 기다리는 사이 HEAD 가 움직이면 하지 않음.

use std::fs;

use super::super::*;
use super::{proceed, stderr_of, unsaved};
use crate::testing::write::WriteFixture;
use crate::write::code as write_code;

/// `main` 에서 갈라진 `feat`: `a.txt` 를 바꾸고 `b.txt` 를 더한다. `README.md` 는 그대로다. 끝나면 `main` 에 있다.
fn feature(f: &WriteFixture) -> String {
    f.repo.commit_file("a.txt", "a\n", "a");
    f.repo.git(&["checkout", "--quiet", "-b", "feat"]);
    f.repo.write("a.txt", "a2\n");
    f.repo.write("b.txt", "b\n");
    let tip = f.repo.commit_all("feat");
    f.repo.checkout("main");
    tip
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_local_branch_checkout_asks_about_exactly_the_paths_it_changes() {
    let f = WriteFixture::new(true);
    let tip = feature(&f);
    let window = proceed(&f);
    let (work_tree, head) = checkout(
        &f.write,
        f.project.clone(),
        None,
        GitCheckoutTarget::Branch {
            name: "feat".into(),
        },
    )
    .await
    .unwrap();
    assert_eq!(work_tree, f.repo.root().display().to_string());
    assert_eq!(
        head,
        RepoHead::Branch {
            name: "feat".into(),
            commit: tip.clone()
        }
    );
    assert_eq!(
        f.repo.git(&["symbolic-ref", "--short", "HEAD"]).trim(),
        "feat"
    );
    assert_eq!(
        unsaved(&window),
        [Some(vec!["a.txt".to_owned(), "b.txt".to_owned()])],
        "the paths that differ between the two trees — not README.md"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_checkout_that_changes_no_path_needs_no_window() {
    let f = WriteFixture::new(true);
    f.repo.branch("same");
    // 듣는 창이 없다 — 바뀔 경로가 없으니 묻지 않고 간다.
    let (_, head) = checkout(
        &f.write,
        f.project.clone(),
        None,
        GitCheckoutTarget::Branch {
            name: "same".into(),
        },
    )
    .await
    .unwrap();
    assert!(matches!(head, RepoHead::Branch { name, .. } if name == "same"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_remote_branch_checkout_creates_a_tracking_branch() {
    let f = WriteFixture::new(true);
    let remote = f.repo.add_bare_remote("origin");
    let other = f.repo.clone_remote(&remote, "other");
    f.repo
        .git_in(&other, &["checkout", "--quiet", "-b", "topic/x"]);
    let tip = f.repo.push_from(&other, "topic/x", "x.txt", "x\n");
    f.repo.git(&["fetch", "--quiet", "origin"]);
    assert_eq!(f.repo.upstream_of("topic/x"), None);
    // 사람이 자동 추적을 꺼 두었어도 원격 브랜치 체크아웃은 추적 브랜치를 만든다(`--track`).
    f.repo.git(&["config", "branch.autoSetupMerge", "false"]);

    let window = proceed(&f);
    let (_, head) = checkout(
        &f.write,
        f.project.clone(),
        None,
        GitCheckoutTarget::Remote {
            remote: "origin".into(),
            branch: "topic/x".into(),
            local: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(
        head,
        RepoHead::Branch {
            name: "topic/x".into(),
            commit: tip
        }
    );
    assert_eq!(
        f.repo.upstream_of("topic/x").as_deref(),
        Some("origin/topic/x")
    );
    assert_eq!(unsaved(&window), [Some(vec!["x.txt".to_owned()])]);

    // 다른 이름으로도: 같은 원격 브랜치를 upstream 으로 하는 `mine`.
    checkout(
        &f.write,
        f.project.clone(),
        None,
        GitCheckoutTarget::Remote {
            remote: "origin".into(),
            branch: "topic/x".into(),
            local: Some("mine".into()),
        },
    )
    .await
    .unwrap();
    assert_eq!(
        f.repo.upstream_of("mine").as_deref(),
        Some("origin/topic/x")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_commit_checkout_detaches_head() {
    let f = WriteFixture::new(true);
    let first = f.repo.head();
    f.repo.commit_file("a.txt", "a\n", "second");
    let _window = proceed(&f);
    let (_, head) = checkout(
        &f.write,
        f.project.clone(),
        None,
        GitCheckoutTarget::Commit { rev: first.clone() },
    )
    .await
    .unwrap();
    assert_eq!(head, RepoHead::Detached { commit: first });
    assert!(!f.repo.root().join("a.txt").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_checkout_git_refuses_shows_the_git_text_and_changes_nothing() {
    let f = WriteFixture::new(true);
    feature(&f);
    let before = f.repo.head();
    // `a.txt` 는 두 트리에서 다르다 — 로컬 변경이 있으면 git 이 체크아웃을 거절한다(자동 stash 같은 우회는 없다, 결정 M2-D6 ④).
    f.repo.write("a.txt", "local edit\n");
    let _window = proceed(&f);
    let e = checkout(
        &f.write,
        f.project.clone(),
        None,
        GitCheckoutTarget::Branch {
            name: "feat".into(),
        },
    )
    .await
    .unwrap_err();
    let stderr = stderr_of(&e);
    assert!(stderr.contains("a.txt"), "{stderr}");
    assert_eq!(RpcError::from(e).code, write_code::GIT_FAILED);
    assert_eq!(f.repo.head(), before);
    assert_eq!(
        fs::read_to_string(f.repo.root().join("a.txt")).unwrap(),
        "local edit\n"
    );

    // 없는 로컬 브랜치도 git 의 원문이다 — 같은 이름의 원격 브랜치(`origin/only-remote`)로 추적 브랜치를 몰래 만들지 않는다
    // (`--no-guess`).
    f.repo.write("a.txt", "a\n");
    let remote = f.repo.add_bare_remote("origin");
    let other = f.repo.clone_remote(&remote, "other");
    f.repo.push_from(&other, "only-remote", "r.txt", "r\n");
    f.repo.git(&["fetch", "--quiet", "origin"]);
    let e = checkout(
        &f.write,
        f.project.clone(),
        None,
        GitCheckoutTarget::Branch {
            name: "only-remote".into(),
        },
    )
    .await
    .unwrap_err();
    assert!(stderr_of(&e).contains("only-remote"), "{e:?}");
    assert_eq!(
        f.repo.git(&["symbolic-ref", "--short", "HEAD"]).trim(),
        "main"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_checkout_does_nothing_when_head_moved_while_it_waited() {
    let f = WriteFixture::new(true);
    feature(&f);
    let _window = proceed(&f);
    // 앞 쓰기가 잠금을 쥔 채 멈춰 있는 동안 체크아웃을 부른다 — 그 계획(바뀔 경로)은 지금 HEAD 로 셈한다.
    let (hold_tx, hold_rx) = tokio::sync::oneshot::channel::<()>();
    let (held_tx, held_rx) = tokio::sync::oneshot::channel::<()>();
    let write = Arc::clone(&f.write);
    let first = tokio::spawn({
        let request = f.request("hold the lock");
        async move {
            write
                .run(request, move |_ctx| async move {
                    held_tx.send(()).unwrap();
                    hold_rx.await.unwrap();
                    Ok(())
                })
                .await
        }
    });
    held_rx.await.unwrap();
    let write = Arc::clone(&f.write);
    let project = f.project.clone();
    let second = tokio::spawn(async move {
        checkout(
            &write,
            project,
            None,
            GitCheckoutTarget::Branch {
                name: "feat".into(),
            },
        )
        .await
    });
    // 체크아웃의 Task 가 뜨면 계획을 셈한 뒤 잠금을 기다리는 중이다.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    while f.tasks.running(Some(&f.project)).len() < 2 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the checkout did not queue"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    // 그 사이 터미널에서 커밋한다 — 확인할 경로가 달라진다.
    let moved = f.repo.commit_file("README.md", "moved\n", "moved");
    hold_tx.send(()).unwrap();
    first.await.unwrap().unwrap();
    let e = second.await.unwrap().unwrap_err();
    let e = RpcError::from(e);
    assert_eq!(e.code, pw_rpc::code::CONFLICT, "{e:?}");
    assert_eq!(f.repo.head(), moved, "the checkout did not run");
    assert_eq!(
        f.repo.git(&["symbolic-ref", "--short", "HEAD"]).trim(),
        "main"
    );
}
