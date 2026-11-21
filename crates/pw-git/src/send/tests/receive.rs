//! 받는 쪽은 원본을 믿지 않는다(④ ⑤)와 받기의 수명(연결 · role · 임시 파일).

use base64::Engine as _;

use super::*;

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn message(e: &WriteError) -> String {
    match e {
        WriteError::Rpc(e) => match &e.data {
            Some(serde_json::Value::String(detail)) => format!("{}: {detail}", e.message),
            _ => e.message.clone(),
        },
        other => other.to_string(),
    }
}

fn state_dir(f: &WriteFixture) -> PathBuf {
    f.projects.paths().state.join(super::super::STATE_DIR)
}

/// ④ 묶음이 다른 ref(`refs/heads/main` · 태그)를 실어도 전용 ref 하나만 생긴다.
#[tokio::test]
async fn only_the_incoming_ref_is_written_whatever_the_bundle_carries() {
    let pair = Pair::new(true);
    let src = &pair.src.repo;
    let base = pair.dst.repo.head();
    src.commit_file("main.txt", "main\n", "main moves");
    src.git(&["tag", "v1"]);
    src.git(&["switch", "--quiet", "-c", "feat"]);
    let feat = src.commit_file("feat.txt", "feat\n", "feat");
    let file = src.base().join("evil.bundle");
    src.git(&[
        "bundle",
        "create",
        "--quiet",
        file.to_str().unwrap(),
        "refs/heads/main",
        "refs/heads/feat",
        "refs/tags/v1",
        "--not",
        &base,
    ]);
    let before = pair
        .dst
        .repo
        .git(&["for-each-ref", "--format=%(refname) %(objectname)"]);

    let received = pair.receive_file(&file, "feat", &feat).await.unwrap();

    let after = pair
        .dst
        .repo
        .git(&["for-each-ref", "--format=%(refname) %(objectname)"]);
    let added: Vec<&str> = after
        .lines()
        .filter(|l| !before.lines().any(|b| b == *l))
        .collect();
    assert_eq!(added, vec![format!("{} {feat}", received.incoming_ref)]);
    assert!(
        before.lines().all(|b| after.lines().any(|a| a == b)),
        "a ref changed"
    );
}

/// 묶음이 알린 커밋을 싣지 않았으면 받지 않는다(전용 ref 를 만들지 않는다).
#[tokio::test]
async fn a_bundle_that_does_not_carry_the_announced_commit_is_refused() {
    let pair = Pair::new(true);
    let first = pair.src.repo.commit_file("a.txt", "a\n", "a");
    let second = pair.src.repo.commit_file("b.txt", "b\n", "b");
    let prepared = pair.prepare("main", &second).await;

    let error = pair
        .receive_file(&prepared.file, "main", &first)
        .await
        .unwrap_err();

    assert!(message(&error).contains("does not carry"), "{error:?}");
    assert_eq!(incoming(&pair.dst.repo, "main"), None);
}

/// ⑤ 잘못된 브랜치 이름 · 원본 머신 id · 커밋 id 와 상한을 넘는 크기는 받지 않는다(임시 파일도 만들지 않는다).
#[tokio::test]
async fn bad_names_and_oversized_bundles_are_refused_up_front() {
    let pair = Pair::new(true);
    let commit = pair.dst.repo.head();
    let uploads = Arc::new(Uploads::default());
    let owner = UploadOwner::test(1, Role::Owner);
    let try_begin = |b: Begin| {
        let uploads = Arc::clone(&uploads);
        let write = Arc::clone(&pair.dst.write);
        async move {
            uploads
                .begin(&write, owner, b, CancellationToken::new())
                .await
        }
    };
    for branch in [
        "a..b",
        "-x",
        "a b",
        "x.lock",
        "@{-1}",
        "a:b",
        "a~1",
        "",
        "refs/heads/../x",
        "x/",
    ] {
        let error = try_begin(begin(&pair.dst, branch, &commit, 10))
            .await
            .unwrap_err();
        assert!(message(&error).contains("branch"), "{branch}: {error:?}");
    }
    for machine in ["", "../x", "a/b", ".hidden", "m.lock", "a b"] {
        let mut b = begin(&pair.dst, "feat", &commit, 10);
        b.source_machine = machine.to_owned();
        let error = try_begin(b).await.unwrap_err();
        assert!(
            message(&error).contains("source machine"),
            "{machine}: {error:?}"
        );
    }
    for bad in ["HEAD", &commit[..12], &commit.to_uppercase()] {
        let error = try_begin(begin(&pair.dst, "feat", bad, 10))
            .await
            .unwrap_err();
        assert!(message(&error).contains("commit id"), "{bad}: {error:?}");
    }
    let error = try_begin(begin(
        &pair.dst,
        "feat",
        &commit,
        super::super::MAX_BUNDLE + 1,
    ))
    .await
    .unwrap_err();
    assert!(message(&error).contains("larger than"), "{error:?}");
    let error = try_begin(begin(&pair.dst, "feat", &commit, 0))
        .await
        .unwrap_err();
    assert!(message(&error).contains("must not be 0"), "{error:?}");
    assert!(uploads.is_empty());
    assert!(
        !state_dir(&pair.dst).exists()
            || std::fs::read_dir(state_dir(&pair.dst)).unwrap().count() == 0
    );
}

/// ⑤ 알린 크기를 넘는 바이트 · 빠지거나 겹친 청크 · 상한을 넘는 청크는 거절하고 그 받기를 버린다. 다 받지 않으면 끝낼 수 없다.
#[tokio::test]
async fn chunks_must_line_up_and_stay_within_the_announced_size() {
    let pair = Pair::new(true);
    let commit = pair.dst.repo.head();
    let uploads = Arc::new(Uploads::default());
    let owner = UploadOwner::test(1, Role::Owner);
    let open = |size: u64| {
        let (uploads, write, b) = (
            Arc::clone(&uploads),
            Arc::clone(&pair.dst.write),
            begin(&pair.dst, "feat", &commit, size),
        );
        async move {
            uploads
                .begin(&write, owner, b, CancellationToken::new())
                .await
                .unwrap()
        }
    };

    let id = open(10).await;
    assert_eq!(
        uploads.chunk(owner, id, 0, &b64(b"12345")).await.unwrap(),
        5
    );
    let error = uploads
        .chunk(owner, id, 5, &b64(b"123456"))
        .await
        .unwrap_err();
    assert!(
        message(&error).contains("past the announced size"),
        "{error:?}"
    );
    let error = uploads.chunk(owner, id, 5, &b64(b"1")).await.unwrap_err();
    assert!(
        message(&error).contains("no upload"),
        "the upload was dropped: {error:?}"
    );

    let id = open(10).await;
    let error = uploads.chunk(owner, id, 3, &b64(b"123")).await.unwrap_err();
    assert!(message(&error).contains("starts at 3"), "{error:?}");

    let id = open(10).await;
    uploads.chunk(owner, id, 0, &b64(b"123")).await.unwrap();
    let error = uploads.chunk(owner, id, 0, &b64(b"123")).await.unwrap_err();
    assert!(
        message(&error).contains("starts at 0"),
        "overlap: {error:?}"
    );

    let id = open(super::super::MAX_BUNDLE).await;
    let big = vec![0u8; super::super::MAX_CHUNK + 1];
    let error = uploads.chunk(owner, id, 0, &b64(&big)).await.unwrap_err();
    assert!(message(&error).contains("larger than"), "{error:?}");

    let id = open(10).await;
    uploads.chunk(owner, id, 0, &b64(b"123")).await.unwrap();
    let error = uploads
        .finish(&pair.dst.write, owner, id)
        .await
        .unwrap_err();
    assert!(message(&error).contains("received 3 of 10"), "{error:?}");
    assert!(uploads.is_empty());
    assert_eq!(std::fs::read_dir(state_dir(&pair.dst)).unwrap().count(), 0);
}

/// `uploadId` 는 연 연결 · role 의 것이다 — 다른 연결이나 다른 role 이 쓰면 없는 id 와 같은 오류이고 받기는 그대로다.
#[tokio::test]
async fn an_upload_belongs_to_its_connection_and_role() {
    let pair = Pair::new(true);
    let commit = pair.dst.repo.head();
    let uploads = Arc::new(Uploads::default());
    let owner = UploadOwner::test(1, Role::Owner);
    let id = uploads
        .begin(
            &pair.dst.write,
            owner,
            begin(&pair.dst, "feat", &commit, 10),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let missing = uploads
        .chunk(owner, id + 100, 0, &b64(b"1"))
        .await
        .unwrap_err();
    for stranger in [
        UploadOwner::test(2, Role::Owner),
        UploadOwner::test(1, Role::Readonly),
    ] {
        let error = uploads
            .chunk(stranger, id, 0, &b64(b"1"))
            .await
            .unwrap_err();
        assert_eq!(
            message(&error),
            message(&missing).replace(&(id + 100).to_string(), &id.to_string())
        );
        uploads
            .finish(&pair.dst.write, stranger, id)
            .await
            .unwrap_err();
    }
    assert_eq!(uploads.chunk(owner, id, 0, &b64(b"1")).await.unwrap(), 1);
}

/// 연결이 닫히면 그 받기를 버리고 임시 파일을 지운다. 파일은 0600 이다.
#[tokio::test]
async fn closing_the_connection_drops_its_uploads() {
    let pair = Pair::new(true);
    let commit = pair.dst.repo.head();
    let uploads = Arc::new(Uploads::default());
    let owner = UploadOwner::test(1, Role::Owner);
    let closed = CancellationToken::new();
    let id = uploads
        .begin(
            &pair.dst.write,
            owner,
            begin(&pair.dst, "feat", &commit, 10),
            closed.clone(),
        )
        .await
        .unwrap();
    uploads.chunk(owner, id, 0, &b64(b"12")).await.unwrap();
    let files: Vec<PathBuf> = std::fs::read_dir(state_dir(&pair.dst))
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .collect();
    assert_eq!(files.len(), 1);
    assert_eq!(
        std::fs::metadata(&files[0]).unwrap().permissions().mode() & 0o777,
        0o600
    );

    closed.cancel();
    for _ in 0..100 {
        if uploads.is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(uploads.is_empty());
    assert!(!files[0].exists());
}

/// 그 프로젝트 창의 연결만 받기를 연다(다른 프로젝트 창 · Host 범위 · 에이전트는 거절).
#[tokio::test]
async fn only_the_project_window_may_receive() {
    let pair = Pair::new(true);
    let commit = pair.dst.repo.head();
    let uploads = Arc::new(Uploads::default());
    for caller in [
        pw_rpc::ConnectionScope::Host,
        pw_rpc::ConnectionScope::Agent,
        pw_rpc::ConnectionScope::Project("other".into()),
    ] {
        let mut b = begin(&pair.dst, "feat", &commit, 10);
        b.caller = caller.clone();
        let error = uploads
            .begin(
                &pair.dst.write,
                UploadOwner::test(1, Role::Owner),
                b,
                CancellationToken::new(),
            )
            .await
            .unwrap_err();
        let WriteError::Rpc(e) = error else {
            panic!("{caller:?}: {error:?}")
        };
        assert_eq!(e.code, pw_rpc::code::PERMISSION_DENIED, "{caller:?}");
    }
}

/// Host 가 처음 `git-send` 폴더를 쓸 때, 끝난 프로세스가 남긴 임시 파일을 지운다(살아 있는 프로세스의 것은 둔다).
#[tokio::test]
async fn files_left_by_dead_processes_are_swept() {
    let pair = Pair::new(true);
    let dir = state_dir(&pair.dst);
    std::fs::create_dir_all(&dir).unwrap();
    // pid 의 상한(macOS 99998, Linux 기본 4194304)을 넘는 수 — 살아 있는 프로세스가 아니다.
    let dead = dir.join("2147483646-u1.bundle");
    let dead_repo = dir.join("2147483646-3-repo");
    let junk = dir.join("junk.bundle");
    let live = dir.join(format!("{}-u99.bundle", std::process::id()));
    for file in [&dead, &junk, &live] {
        std::fs::write(file, b"x").unwrap();
    }
    std::fs::create_dir_all(dead_repo.join("objects")).unwrap();
    let commit = pair.dst.repo.head();

    let uploads = Arc::new(Uploads::default());
    uploads
        .begin(
            &pair.dst.write,
            UploadOwner::test(1, Role::Owner),
            begin(&pair.dst, "feat", &commit, 10),
            CancellationToken::new(),
        )
        .await
        .unwrap();

    assert!(!dead.exists() && !dead_repo.exists() && !junk.exists());
    assert!(live.exists());
}
