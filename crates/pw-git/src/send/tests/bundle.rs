//! 원본 쪽 묶음(⑥ 과 원본을 건드리지 않음).

use super::*;

/// 같은 입력에 같은 결과를 내는 압축되지 않는 글(LCG 의 16진수).
fn noise(len: usize, seed: u64) -> String {
    let mut x = seed;
    let mut out = String::with_capacity(len);
    while out.len() < len {
        x = x
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        out.push_str(&format!("{:016x}", x >> 1));
    }
    out
}

/// ⑥ 대상이 가진 커밋은 묶음에 들지 않는다 — 큰 파일이 든 커밋을 대상이 가졌으면 묶음은 작다.
#[tokio::test]
async fn commits_the_target_has_stay_out_of_the_bundle() {
    let src = TestRepo::with_commit();
    src.commit_file("big.txt", &noise(400_000, 7), "big");
    let dst = clone_of(&src);
    let pair = Pair::of(src, dst, true);
    let commit = pair.src.repo.commit_file("small.txt", "small\n", "small");

    let with_theirs = pair.prepare("main", &commit).await;
    let without = make_bundle(
        pair.src.write.git(),
        &pair.src.project,
        None,
        "main",
        &commit,
        &[],
        &CancellationToken::new(),
    )
    .await
    .unwrap();

    assert!(with_theirs.size < 20_000, "{}", with_theirs.size);
    assert!(without.size > 150_000, "{}", without.size);
    assert_eq!(with_theirs.excluded, vec![pair.dst.repo.head()]);
    let received = pair
        .receive_file(&with_theirs.file, "main", &commit)
        .await
        .unwrap();
    assert_eq!(received.commit, commit);
}

/// 보낼 커밋과 그 조상만 든다 — 그 뒤에 더한 커밋은 가지 않는다.
#[tokio::test]
async fn only_the_named_commit_and_its_ancestors_go() {
    let pair = Pair::new(true);
    let first = pair.src.repo.commit_file("a.txt", "a\n", "a");
    let later = pair.src.repo.commit_file("b.txt", "b\n", "b");

    let outcome = pair.send("main", &first).await;

    assert!(matches!(
        outcome,
        super::super::apply::GitSendOutcome::Applied { .. }
    ));
    assert_eq!(pair.dst.repo.head(), first);
    let out = pair
        .dst
        .repo
        .try_git_in(pair.dst.repo.root(), &["cat-file", "-e", &later]);
    assert!(!out.status.success(), "the later commit reached the target");
}

/// 대상이 이미 그 커밋을 가졌어도(다른 ref 로) 묶음은 비지 않는다 — 다시 보내도 된다.
#[tokio::test]
async fn a_commit_the_target_already_has_still_makes_a_bundle() {
    let src = TestRepo::with_commit();
    src.commit_file("big.txt", &noise(400_000, 11), "big");
    let dst = clone_of(&src);
    let pair = Pair::of(src, dst, true);
    let parent = pair.src.repo.head();
    let commit = pair.src.repo.commit_file("a.txt", "a\n", "a");
    pair.send("main", &commit).await;

    let prepared = pair.prepare("main", &commit).await;

    // 그 커밋 하나만 든다 — 부모(대상이 가진 것)를 뺐다. 빼지 않으면 큰 파일이 든 전체 이력이 묶인다.
    assert!(prepared.size > 0);
    assert!(prepared.size < 20_000, "{}", prepared.size);
    assert_eq!(prepared.excluded, vec![parent]);
    pair.receive_file(&prepared.file, "main", &commit)
        .await
        .unwrap();
}

/// 원본에 커밋 안 한 변경이 있으면 표지만 단다(막지 않는다).
#[tokio::test]
async fn uncommitted_changes_on_the_source_are_flagged_not_blocked() {
    let pair = Pair::new(true);
    let commit = pair.src.repo.commit_file("a.txt", "a\n", "a");
    assert!(!pair.prepare("main", &commit).await.uncommitted);
    pair.src.repo.write("a.txt", "changed\n");
    let prepared = pair.prepare("main", &commit).await;
    assert!(prepared.uncommitted);
    assert!(prepared.size > 0);
}

/// 원본 저장소의 설정을 읽지 않고(설정이 git 을 죽이는 값이어도), 원본에 쓰지 않는다(ref 가 바뀌면 부르는 훅이 불리지 않고 git
/// 디렉터리의 파일이 그대로다). 임시 저장소 · 묶음 파일은 끝나면 지운다.
#[tokio::test]
async fn prepare_neither_reads_the_source_config_nor_writes_to_the_source() {
    let pair = Pair::new(true);
    let src = &pair.src.repo;
    let commit = src.commit_file("a.txt", "a\n", "a");
    let git_dir = src.root().join(".git");
    let mut config = std::fs::read_to_string(git_dir.join("config")).unwrap();
    config.push_str("[core]\n\tbigFileThreshold = notanumber\n");
    std::fs::write(git_dir.join("config"), config).unwrap();
    let marker = git_dir.join("hook-ran");
    for hook in ["reference-transaction", "post-checkout", "post-merge"] {
        let path = git_dir.join("hooks").join(hook);
        std::fs::write(
            &path,
            format!("#!/bin/sh\necho {hook} >> '{}'\n", marker.display()),
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let before = listing(&git_dir);

    let prepared = pair.prepare("main", &commit).await;
    let file = prepared.file.clone();

    assert!(prepared.size > 0);
    assert!(!marker.exists(), "{:?}", std::fs::read_to_string(&marker));
    assert_eq!(listing(&git_dir), before);
    let state = pair
        .src
        .projects
        .paths()
        .state
        .join(super::super::STATE_DIR);
    let mode = std::fs::metadata(&file).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    assert_eq!(
        std::fs::metadata(&state).unwrap().permissions().mode() & 0o777,
        0o700
    );
    drop(prepared);
    assert!(!file.exists());
    assert_eq!(
        std::fs::read_dir(&state).unwrap().count(),
        0,
        "the scratch repository is left"
    );
}

/// git 디렉터리의 파일(경로 · 내용). 원본 git 이 쓰는 index 등 잠금 없는 파일까지 본다.
fn listing(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                out.push((path.display().to_string(), std::fs::read(&path).unwrap()));
            }
        }
    }
    out.sort();
    out
}
