//! 리뷰 반영: 받기의 상한 · 유휴(F1), chunk 와 연결 닫힘의 경쟁(F2), 받은 객체의 fsck(권고 1), 잠금 전후 · `update-ref` 직전의
//! 경쟁과 `bundle verify`(권고 2), sha256 원본(권고 4).

use std::io::Write as _;
use std::process::Stdio;
use std::time::Duration;

use base64::Engine as _;

use super::super::apply::{BEFORE_UPDATE_REF, GitSendStop};
use super::super::receive::{
    IDLE_TIMEOUT, MAX_OPEN_BYTES, MAX_UPLOADS, MAX_UPLOADS_PER_CONNECTION,
};
use super::super::{MAX_BUNDLE, STATE_DIR};
use super::*;

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn detail(e: &WriteError) -> String {
    match e {
        WriteError::Rpc(e) => format!("{} {:?}", e.message, e.data),
        other => format!("{other:?}"),
    }
}

fn code(e: &WriteError) -> i32 {
    match e {
        WriteError::Rpc(e) => e.code,
        other => panic!("expected an rpc error, got {other:?}"),
    }
}

fn files(f: &WriteFixture) -> usize {
    std::fs::read_dir(f.projects.paths().state.join(STATE_DIR)).map_or(0, |d| d.count())
}

async fn open(
    uploads: &Arc<Uploads>,
    f: &WriteFixture,
    owner: UploadOwner,
    size: u64,
) -> Result<u64, WriteError> {
    let commit = f.repo.head();
    uploads
        .begin(
            &f.write,
            owner,
            begin(f, "feat", &commit, size),
            CancellationToken::new(),
        )
        .await
}

#[test]
fn the_limits_are_what_the_report_says() {
    assert_eq!((MAX_UPLOADS_PER_CONNECTION, MAX_UPLOADS), (2, 8));
    assert_eq!(MAX_OPEN_BYTES, 2 * MAX_BUNDLE);
    assert_eq!(IDLE_TIMEOUT, Duration::from_secs(300));
}

/// F1: 연결 하나는 받기를 둘까지 연다. 셋째는 conflict 이고 임시 파일을 남기지 않는다. 다른 연결은 열 수 있다.
#[tokio::test]
async fn a_connection_may_hold_at_most_two_uploads() {
    let pair = Pair::new(true);
    let uploads = Arc::new(Uploads::default());
    let me = UploadOwner::test(1, Role::Owner);
    open(&uploads, &pair.dst, me, 10).await.unwrap();
    open(&uploads, &pair.dst, me, 10).await.unwrap();
    let error = open(&uploads, &pair.dst, me, 10).await.unwrap_err();
    assert_eq!(code(&error), pw_rpc::code::CONFLICT, "{error:?}");
    assert!(detail(&error).contains("already has 2"), "{error:?}");
    assert_eq!(files(&pair.dst), 2);
    open(&uploads, &pair.dst, UploadOwner::test(2, Role::Owner), 10)
        .await
        .unwrap();
}

/// F1: Host 전체로 받기는 여덟, 알린 크기의 합은 2 GiB 까지다.
#[tokio::test]
async fn the_host_caps_open_uploads_and_their_bytes() {
    let pair = Pair::new(true);
    let uploads = Arc::new(Uploads::default());
    for n in 0..8 {
        open(&uploads, &pair.dst, UploadOwner::test(n, Role::Owner), 10)
            .await
            .unwrap();
    }
    let error = open(&uploads, &pair.dst, UploadOwner::test(99, Role::Owner), 10)
        .await
        .unwrap_err();
    assert!(detail(&error).contains("8 uploads"), "{error:?}");

    let uploads = Arc::new(Uploads::default());
    open(
        &uploads,
        &pair.dst,
        UploadOwner::test(1, Role::Owner),
        MAX_BUNDLE,
    )
    .await
    .unwrap();
    open(
        &uploads,
        &pair.dst,
        UploadOwner::test(2, Role::Owner),
        MAX_BUNDLE,
    )
    .await
    .unwrap();
    let error = open(&uploads, &pair.dst, UploadOwner::test(3, Role::Owner), 1)
        .await
        .unwrap_err();
    assert_eq!(code(&error), pw_rpc::code::CONFLICT);
    assert!(detail(&error).contains("exceed"), "{error:?}");
    assert_eq!(uploads.len(), 2);
}

/// F1: 마지막 begin · chunk 뒤 유휴 시간이 지난 받기는 버린다(파일도). 청크가 오는 받기는 남는다.
#[tokio::test]
async fn an_idle_upload_is_dropped_while_a_busy_one_stays() {
    let pair = Pair::new(true);
    let uploads = Arc::new(Uploads::with_idle(Duration::from_millis(300)));
    let (idle, busy) = (
        UploadOwner::test(1, Role::Owner),
        UploadOwner::test(2, Role::Owner),
    );
    let a = open(&uploads, &pair.dst, idle, 10).await.unwrap();
    let b = open(&uploads, &pair.dst, busy, 10).await.unwrap();
    for n in 0..8u64 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        uploads.chunk(busy, b, n, &b64(b"x")).await.unwrap();
    }
    let error = uploads.chunk(idle, a, 0, &b64(b"x")).await.unwrap_err();
    assert!(detail(&error).contains("no upload"), "{error:?}");
    assert_eq!(uploads.len(), 1);
    assert_eq!(files(&pair.dst), 1);
}

/// F2: chunk 가 받기를 꺼내 쓰는 사이 연결이 닫히면, 다시 넣지 않고 버린다(주인 없는 받기 · 파일이 남지 않는다).
#[tokio::test(flavor = "current_thread")]
async fn a_connection_closing_during_a_chunk_leaves_no_upload() {
    let pair = Pair::new(true);
    let uploads = Arc::new(Uploads::default());
    let owner = UploadOwner::test(1, Role::Owner);
    let closed = CancellationToken::new();
    let commit = pair.dst.repo.head();
    let id = uploads
        .begin(
            &pair.dst.write,
            owner,
            begin(&pair.dst, "feat", &commit, 10),
            closed.clone(),
        )
        .await
        .unwrap();
    let cancel = closed.clone();
    *uploads.before_reinsert.lock().unwrap() = Some(Box::new(move || {
        cancel.cancel();
        // 감시 태스크가 먼저 돌아 (꺼내져 있어) 아무것도 못 지우고 끝나게 한다.
        std::thread::sleep(Duration::from_millis(200));
    }));

    let result = uploads.chunk(owner, id, 0, &b64(b"12")).await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    assert!(matches!(result, Err(WriteError::Cancelled)), "{result:?}");
    assert!(uploads.is_empty());
    assert_eq!(files(&pair.dst), 0);
}

/// 권고 1: 받은 객체를 fsck 로 본다 — `.git` 이라는 트리 항목을 실은 커밋은 받지 않는다(전용 ref 를 만들지 않는다).
#[tokio::test]
async fn received_objects_are_checked_by_fsck() {
    let pair = Pair::new(true);
    let src = &pair.src.repo;
    let blob = src.commit_file("x.txt", "x\n", "x");
    let blob = src
        .git(&["rev-parse", &format!("{blob}:x.txt")])
        .trim()
        .to_owned();
    let mut mktree = src.command_in(src.root());
    let mut child = mktree
        .args(["mktree"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(format!("100644 blob {blob}\t.git\n").as_bytes())
        .unwrap();
    let tree = String::from_utf8(child.wait_with_output().unwrap().stdout).unwrap();
    let evil = src
        .git(&["commit-tree", tree.trim(), "-p", "HEAD", "-m", "evil"])
        .trim()
        .to_owned();
    src.git(&["update-ref", "refs/heads/evil", &evil]);
    let prepared = pair.prepare("evil", &evil).await;

    let error = pair
        .receive_file(&prepared.file, "evil", &evil)
        .await
        .unwrap_err();

    assert!(
        matches!(&error, WriteError::Failed { command, .. } if command == "fetch"),
        "{error:?}"
    );
    assert_eq!(incoming(&pair.dst.repo, "evil"), None);
}

/// 권고 2(M8): 대상에 없는 전제 커밋을 요구하는 묶음은 `git bundle verify` 가 먼저 거절한다 — 가져오기(`fetch`)까지 가지 않는다.
#[tokio::test]
async fn a_bundle_missing_prerequisites_stops_at_verify() {
    let pair = Pair::new(true);
    let first = pair.src.repo.commit_file("a.txt", "a\n", "a");
    let commit = pair.src.repo.commit_file("b.txt", "b\n", "b");
    // 대상은 `first` 를 갖지 않았는데 그것을 전제로 묶는다.
    let prepared = make_bundle(
        pair.src.write.git(),
        &pair.src.project,
        None,
        "main",
        &commit,
        &[first],
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    let bytes = std::fs::read(&prepared.file).unwrap();

    let error = receive_bytes(&pair.dst, &bytes, "main", &commit)
        .await
        .unwrap_err();

    assert!(
        matches!(&error, WriteError::Failed { command, .. } if command == "bundle"),
        "{error:?}"
    );
    assert!(
        pair.dst_calls()
            .iter()
            .all(|argv| subcommand(argv).0 != "fetch"),
        "{:?}",
        pair.dst_calls()
    );
}

/// 권고 2(M2): 잠금을 기다리는 사이 대상 HEAD 가 움직였으면(미저장 확인에 실은 경로가 맞지 않는다) 아무것도 하지 않고 conflict.
#[tokio::test]
async fn a_head_that_moves_while_apply_waits_for_the_lock_is_a_conflict() {
    let pair = Pair::new(true);
    let commit = pair.src.repo.commit_file("a.txt", "a\n", "a");
    let prepared = pair.prepare("main", &commit).await;
    pair.receive_file(&prepared.file, "main", &commit)
        .await
        .unwrap();
    let expected = pair.theirs("main").await.branch_tip;
    let (release, held) = tokio::sync::oneshot::channel::<()>();
    let write = Arc::clone(&pair.dst.write);
    let request = pair.dst.request("task-title-git-send-apply");
    let blocker = tokio::spawn(async move {
        write
            .run(request, move |_ctx| async move {
                let _ = held.await;
                Ok(())
            })
            .await
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
    let apply = {
        let dst = Apply {
            caller: pair.dst.caller(),
            approved: None,
            project_id: pair.dst.project.clone(),
            worktree: None,
            source_machine: MACHINE.to_owned(),
            branch: "main".to_owned(),
            commit: commit.clone(),
            expected,
        };
        let write = Arc::clone(&pair.dst.write);
        tokio::spawn(async move { super::super::apply::apply(&write, dst).await })
    };
    tokio::time::sleep(Duration::from_millis(300)).await;
    pair.dst
        .repo
        .commit_file("theirs.txt", "t\n", "moved meanwhile");
    let moved = pair.dst.repo.head();
    release.send(()).unwrap();
    blocker.await.unwrap().unwrap();

    let error = apply.await.unwrap().unwrap_err();

    assert_eq!(code(&error), pw_rpc::code::CONFLICT, "{error:?}");
    assert_eq!(pair.dst.repo.head(), moved);
}

/// 권고 2(M3): 기대 값을 본 뒤 `update-ref` 직전에 바깥 git 이 브랜치를 움직이면 `update-ref` 의 CAS 가 거절해 멈춘다.
#[tokio::test]
async fn a_branch_moved_right_before_update_ref_is_left_alone() {
    let pair = Pair::new(true);
    let dst = &pair.dst.repo;
    dst.branch("feat");
    dst.git(&["switch", "--quiet", "-c", "side"]);
    let side = dst.commit_file("side.txt", "s\n", "side");
    dst.git(&["switch", "--quiet", "main"]);
    pair.src.repo.git(&["switch", "--quiet", "-c", "feat"]);
    let commit = pair.src.repo.commit_file("feat.txt", "f\n", "feat");
    let prepared = pair.prepare("feat", &commit).await;
    pair.receive_file(&prepared.file, "feat", &commit)
        .await
        .unwrap();
    let expected = pair.theirs("feat").await.branch_tip;
    let (root, base, to) = (
        dst.root().to_path_buf(),
        dst.base().to_path_buf(),
        side.clone(),
    );
    *BEFORE_UPDATE_REF.lock().unwrap() = Some(Box::new(move || {
        let out = crate::testing::repo::git_command(&root, &base)
            .args(["update-ref", "refs/heads/feat", &to])
            .output()
            .unwrap();
        assert!(out.status.success());
    }));

    let outcome = pair.apply("feat", &commit, expected).await.unwrap();

    assert!(
        matches!(
            outcome,
            GitSendOutcome::Stopped {
                reason: GitSendStop::BranchMoved,
                message: Some(_)
            }
        ),
        "{outcome:?}"
    );
    assert_eq!(ref_of(dst, "refs/heads/feat"), Some(side));
    assert_eq!(dst.git(&["symbolic-ref", "HEAD"]).trim(), "refs/heads/main");
}

/// 권고 4: sha256 원본은 분명한 오류로 거절한다(사본 저장소는 sha1 만 만든다).
#[tokio::test]
async fn a_sha256_source_is_refused_clearly() {
    let repo = TestRepo::new();
    std::fs::remove_dir_all(repo.root().join(".git")).unwrap();
    repo.git(&["init", "--quiet", "--object-format=sha256"]);
    let commit = repo.commit_file("a.txt", "a\n", "a");
    let f = WriteFixture::with_repo(repo, true, Askpass::default());

    let error = make_bundle(
        f.write.git(),
        &f.project,
        None,
        "main",
        &commit,
        &[],
        &CancellationToken::new(),
    )
    .await
    .unwrap_err();

    assert!(detail(&error).contains("not supported"), "{error:?}");
}
