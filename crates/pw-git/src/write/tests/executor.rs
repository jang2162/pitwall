//! 실행기: 같은 저장소의 쓰기는 차례로 돌고 둘 다 끝난다(①), 그동안 읽기는 막히지 않는다, `index.lock` 이 있으면 그 오류로 끝나고
//! 파일은 남는다(②), git 의 실패는 stderr 원문이 Task 의 오류다, stdin 이 git 에 닿는다, Task 취소는 git 이 잠금 파일을 지우고 끝난
//! 뒤에 잠금을 놓는다, 진행에는 하위 명령만 보인다.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use pw_core::task::{TOPIC_FINISHED, TOPIC_PROGRESS};
use pw_rpc::RpcError;
use tokio::sync::oneshot;

use super::super::*;
use super::{last, subscribe_all, until, until_finished};
use crate::testing::write::WriteFixture;

const TITLE: &str = "test git write";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_writes_to_one_repository_run_one_after_the_other_and_both_finish() {
    let f = WriteFixture::new(true);
    let order = Arc::new(Mutex::new(Vec::<&str>::new()));
    let (started_tx, started_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel::<()>();
    let a = tokio::spawn({
        let (write, order) = (Arc::clone(&f.write), Arc::clone(&order));
        let request = f.request(TITLE);
        async move {
            write
                .run(request, move |ctx| async move {
                    order.lock().unwrap().push("a start");
                    let _ = started_tx.send(());
                    let _ = release_rx.await;
                    ctx.run(ctx.git().args(["commit", "--allow-empty", "-q", "-m", "a"]))
                        .await?;
                    order.lock().unwrap().push("a end");
                    Ok(())
                })
                .await
        }
    });
    started_rx.await.unwrap();
    let b_started = Arc::new(AtomicBool::new(false));
    let b = tokio::spawn({
        let (write, order, b_started) = (
            Arc::clone(&f.write),
            Arc::clone(&order),
            Arc::clone(&b_started),
        );
        let request = f.request(TITLE);
        async move {
            write
                .run(request, move |ctx| async move {
                    b_started.store(true, Ordering::SeqCst);
                    order.lock().unwrap().push("b start");
                    ctx.run(ctx.git().args(["commit", "--allow-empty", "-q", "-m", "b"]))
                        .await?;
                    order.lock().unwrap().push("b end");
                    Ok(())
                })
                .await
        }
    });
    // b 의 Task 가 떴다(잠금을 기다리는 중). 잠금이 없다면 b 는 곧바로 본문을 시작한다.
    until("b's task starts", || {
        f.tasks.running(Some(&f.project)).len() == 2
    })
    .await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        !b_started.load(Ordering::SeqCst),
        "b ran while a held the lock"
    );

    // 읽기는 쓰기 잠금을 기다리지 않는다.
    let read = tokio::time::timeout(Duration::from_secs(5), f.write.operation(&f.project, None))
        .await
        .expect("a read waited for the write lock");
    assert_eq!(read, Ok(None));

    release_tx.send(()).unwrap();
    a.await.unwrap().unwrap();
    b.await.unwrap().unwrap();
    assert_eq!(
        *order.lock().unwrap(),
        ["a start", "a end", "b start", "b end"]
    );
    assert_eq!(
        f.repo.git(&["log", "--format=%s", "-2"]),
        "b\na\n",
        "두 커밋 모두 들어갔다"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_index_lock_ends_the_write_with_that_error_and_stays() {
    let f = WriteFixture::new(true);
    let lock = f.repo.root().join(".git").join("index.lock");
    fs::write(&lock, "held by someone else").unwrap();
    let mut events = subscribe_all(&f.bus);
    let ran = Arc::new(AtomicBool::new(false));
    let result = f
        .write
        .run(f.request(TITLE), {
            let ran = Arc::clone(&ran);
            move |_ctx| async move {
                ran.store(true, Ordering::SeqCst);
                Ok(())
            }
        })
        .await;
    let Err(WriteError::IndexLocked { path }) = &result else {
        panic!("expected IndexLocked, got {result:?}");
    };
    assert_eq!(
        fs::canonicalize(path).unwrap(),
        fs::canonicalize(&lock).unwrap()
    );
    assert!(!ran.load(Ordering::SeqCst), "the job must not run");
    assert_eq!(
        fs::read_to_string(&lock).unwrap(),
        "held by someone else",
        "the lock file is left alone"
    );
    let rpc = RpcError::from(result.unwrap_err());
    assert_eq!(rpc.code, code::INDEX_LOCKED);
    let finished = last(&until_finished(&mut events).await, TOPIC_FINISHED).expect("task.finished");
    assert_eq!(finished["status"], "failed");
    assert!(
        finished["error"].as_str().unwrap().contains("index.lock"),
        "{finished}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failing_git_fails_the_task_with_its_stderr_as_written() {
    let f = WriteFixture::new(true);
    let mut events = subscribe_all(&f.bus);
    let result = f
        .write
        .run(f.request(TITLE), |ctx| async move {
            ctx.run(ctx.git().args(["checkout", "no-such-branch"]))
                .await
                .map(drop)
        })
        .await;
    let Err(WriteError::Failed {
        command, stderr, ..
    }) = &result
    else {
        panic!("expected Failed, got {result:?}");
    };
    assert_eq!(command, "checkout");
    assert!(stderr.contains("no-such-branch"), "{stderr}");
    let finished = last(&until_finished(&mut events).await, TOPIC_FINISHED).expect("task.finished");
    assert_eq!(finished["title"], TITLE);
    assert_eq!(finished["status"], "failed");
    assert_eq!(
        finished["error"].as_str().unwrap(),
        stderr.trim_end(),
        "the notification body is git's stderr as written"
    );
    let stderr = stderr.clone();
    let rpc = RpcError::from(result.unwrap_err());
    assert_eq!(rpc.code, code::GIT_FAILED);
    assert_eq!(rpc.data.as_ref().unwrap()["stderr"], stderr);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stdin_reaches_git() {
    let f = WriteFixture::new(true);
    let out = f
        .write
        .run(f.request(TITLE), |ctx| async move {
            ctx.run(
                ctx.git()
                    .args(["hash-object", "--stdin"])
                    .stdin(b"hello\n".to_vec()),
            )
            .await
        })
        .await
        .unwrap();
    assert_eq!(
        String::from_utf8(out.stdout).unwrap().trim(),
        "ce013625030ba8dba906f756967f9e9ca394464a",
        "the blob id of `hello\\n`"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn progress_shows_the_subcommand_only() {
    let f = WriteFixture::new(true);
    let mut events = subscribe_all(&f.bus);
    f.write
        .run(f.request(TITLE), |ctx| async move {
            ctx.run(ctx.git().args([
                "-c",
                "user.name=Someone",
                "commit",
                "--allow-empty",
                "-q",
                "-m",
                "secret message",
            ]))
            .await
            .map(drop)
        })
        .await
        .unwrap();
    let progress = last(&until_finished(&mut events).await, TOPIC_PROGRESS).expect("task.progress");
    assert_eq!(progress["progress"]["message"], "git commit");
    assert!(!progress.to_string().contains("secret message"));
}

/// Task 를 취소하면 git(과 그 훅)이 SIGTERM 으로 끝나고, git 이 지운 `index.lock` 을 다음 쓰기가 보지 않는다 — 잠금은 git 이 끝난
/// 뒤에 풀린다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelling_the_task_ends_git_and_its_hook_before_the_lock_is_released() {
    let f = WriteFixture::new(true);
    let marks = f.repo.base().join("hook.pid");
    let hook = f.repo.root().join(".git/hooks/pre-commit");
    fs::write(
        &hook,
        format!(
            "#!/bin/sh\necho $$ > '{}'\nexec sleep 30\n",
            marks.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    // `commit -a` 는 새 index 를 `index.lock` 에 쓰고 그것으로 훅을 부른다 — 훅이 도는 동안 잠금 파일이 있다.
    f.repo.write("README.md", "changed\n");

    let first = tokio::spawn({
        let write = Arc::clone(&f.write);
        let request = f.request(TITLE);
        async move {
            write
                .run(request, |ctx| async move {
                    ctx.run(ctx.git().args(["commit", "-a", "-q", "-m", "x"]))
                        .await
                        .map(drop)
                })
                .await
        }
    });
    until("the hook runs", || {
        marks.exists() && fs::read_to_string(&marks).is_ok_and(|p| p.ends_with('\n'))
    })
    .await;
    let pid: i32 = fs::read_to_string(&marks).unwrap().trim().parse().unwrap();
    assert!(
        f.repo.root().join(".git/index.lock").exists(),
        "git commit -a holds index.lock while the hook runs"
    );
    let second_ran = Arc::new(AtomicBool::new(false));
    let second = tokio::spawn({
        let write = Arc::clone(&f.write);
        let request = f.request(TITLE);
        let ran = Arc::clone(&second_ran);
        async move {
            write
                .run(request, move |_ctx| async move {
                    ran.store(true, Ordering::SeqCst);
                    Ok(())
                })
                .await
        }
    });
    until("both tasks run", || {
        f.tasks.running(Some(&f.project)).len() == 2
    })
    .await;
    let id = f.tasks.running(Some(&f.project))[0].task_id;
    assert!(f.tasks.cancel(id));

    assert_eq!(first.await.unwrap(), Err(WriteError::Cancelled));
    // SAFETY: kill(2) 에 신호 0 은 프로세스가 있는지만 본다.
    until("the hook is gone", || unsafe { libc::kill(pid, 0) } != 0).await;
    assert_eq!(
        second.await.unwrap(),
        Ok(()),
        "the queued write did not see a leftover index.lock"
    );
    assert!(second_ran.load(Ordering::SeqCst));
    assert!(!f.repo.root().join(".git/index.lock").exists());
    assert_eq!(
        f.repo.git(&["log", "--format=%s", "-1"]),
        "initial\n",
        "the cancelled commit did not happen"
    );
}

/// 잠금을 기다리는 쓰기의 Task 를 취소하면 그 쓰기는 앞 쓰기가 잠금을 쥔 채로 곧 끝나고, 본문은 돌지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelling_a_write_that_waits_for_the_lock_never_runs_it() {
    let f = WriteFixture::new(true);
    let (holding_tx, holding_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel::<()>();
    let first = tokio::spawn({
        let write = Arc::clone(&f.write);
        let request = f.request("test git first");
        async move {
            write
                .run(request, move |_ctx| async move {
                    let _ = holding_tx.send(());
                    let _ = release_rx.await;
                    Ok(())
                })
                .await
        }
    });
    holding_rx.await.unwrap();
    let second_ran = Arc::new(AtomicBool::new(false));
    let second = tokio::spawn({
        let write = Arc::clone(&f.write);
        let request = f.request("test git second");
        let ran = Arc::clone(&second_ran);
        async move {
            write
                .run(request, move |_ctx| async move {
                    ran.store(true, Ordering::SeqCst);
                    Ok(())
                })
                .await
        }
    });
    until("the second task waits", || {
        f.tasks.running(Some(&f.project)).len() == 2
    })
    .await;
    let waiting = f
        .tasks
        .running(Some(&f.project))
        .into_iter()
        .find(|task| task.title == "test git second")
        .unwrap();
    assert!(f.tasks.cancel(waiting.task_id));
    // 앞 쓰기가 잠금을 쥔 채로 끝나야 한다 — 잠금이 풀리기를 기다렸다가 끝나는 것이 아니다(10초는 멈춤을 잡는 상한).
    let cancelled = tokio::time::timeout(Duration::from_secs(10), second)
        .await
        .expect("the cancelled write kept waiting for the lock")
        .unwrap();
    assert_eq!(cancelled, Err(WriteError::Cancelled));
    release_tx.send(()).unwrap();
    assert_eq!(first.await.unwrap(), Ok(()));
    assert!(
        !second_ran.load(Ordering::SeqCst),
        "the cancelled write ran its body"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_project_outside_any_repository_has_nothing_to_write() {
    let f = WriteFixture::new(true);
    let outside = f.repo.base().join("outside");
    fs::create_dir_all(&outside).unwrap();
    let project = f
        .projects
        .create(pw_core::registry::NewProject {
            root: outside,
            ..Default::default()
        })
        .unwrap()
        .id;
    let mut request = f.request(TITLE);
    request.project_id = project.clone();
    let result = f.write.run(request, |_ctx| async { Ok(()) }).await;
    assert_eq!(result, Err(WriteError::NoRepository));
    assert_eq!(
        RpcError::from(WriteError::NoRepository).code,
        code::NO_REPOSITORY
    );
    assert_eq!(f.write.operation(&project, None).await, Ok(None));
}

#[test]
fn the_subcommand_skips_options_and_config_values() {
    let base = GitCommand::new(
        "git".into(),
        ".".into(),
        Vec::new(),
        crate::cli::Access::Write,
    );
    assert_eq!(
        subcommand(&base.clone().args([
            "-c",
            "a=b",
            "-C",
            "dir",
            "--no-pager",
            "push",
            "https://u:t@h/x"
        ])),
        "push"
    );
    assert_eq!(subcommand(&base), "");
}
