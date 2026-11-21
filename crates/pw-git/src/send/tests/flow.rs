//! 보내기가 되는 갈래: fast-forward(체크아웃된 브랜치), 새 브랜치 만들기 + 전환, LFS 포인터, 전용 ref 치우기.

use super::super::apply::GitSendOutcome;
use super::*;

/// ⑧ 대상이 그 브랜치에 있을 때: 작업 트리 · index 가 받은 커밋과 같고 `git status` 가 깨끗하다(`update-ref` 만 하면 실패).
/// 원본 · 대상 모두 미신뢰여도 사람이 누른 보내기는 돈다(D43).
#[tokio::test]
async fn sending_to_the_checked_out_branch_moves_the_work_tree_too() {
    let pair = Pair::new(false);
    let before = pair.dst.repo.head();
    pair.src
        .repo
        .commit_file("README.md", "hello\nfrom the laptop\n", "one");
    let commit = pair.src.repo.commit_file("src/new.txt", "new\n", "two");

    let outcome = pair.send("main", &commit).await;

    assert_eq!(
        outcome,
        GitSendOutcome::Applied {
            previous: Some(before),
            current: commit.clone(),
            created: false,
            switched: false,
            lfs_pointers: 0,
        }
    );
    let dst = &pair.dst.repo;
    assert_eq!(dst.head(), commit);
    assert_eq!(
        dst.git(&["status", "--porcelain=v1", "--untracked-files=all"]),
        ""
    );
    assert_eq!(
        std::fs::read_to_string(dst.root().join("README.md")).unwrap(),
        "hello\nfrom the laptop\n"
    );
    assert_eq!(
        std::fs::read_to_string(dst.root().join("src/new.txt")).unwrap(),
        "new\n"
    );
    assert_eq!(incoming(dst, "main"), Some(commit));
}

/// 대상에 없는 브랜치: 만들고 체크아웃을 그리로 바꾼다. 결과의 `previous` 는 없다(만들었다).
#[tokio::test]
async fn a_new_branch_is_created_and_checked_out() {
    let pair = Pair::new(true);
    pair.src.repo.git(&["switch", "--quiet", "-c", "feat"]);
    let commit = pair.src.repo.commit_file("feat.txt", "feat\n", "feat");

    let outcome = pair.send("feat", &commit).await;

    assert_eq!(
        outcome,
        GitSendOutcome::Applied {
            previous: None,
            current: commit.clone(),
            created: true,
            switched: true,
            lfs_pointers: 0,
        }
    );
    let dst = &pair.dst.repo;
    assert_eq!(dst.git(&["symbolic-ref", "HEAD"]).trim(), "refs/heads/feat");
    assert_eq!(dst.head(), commit);
    assert_eq!(
        dst.git(&["status", "--porcelain=v1", "--untracked-files=all"]),
        ""
    );
}

/// 있는 브랜치를 fast-forward 하고 그리로 바꾼다. 이전 해시가 결과에 있다(되돌리기용).
#[tokio::test]
async fn an_existing_branch_is_fast_forwarded_and_checked_out() {
    let pair = Pair::new(true);
    pair.dst.repo.branch("feat");
    let old = pair.dst.repo.head();
    pair.src.repo.git(&["switch", "--quiet", "-c", "feat"]);
    let commit = pair.src.repo.commit_file("feat.txt", "feat\n", "feat");

    let outcome = pair.send("feat", &commit).await;

    assert_eq!(
        outcome,
        GitSendOutcome::Applied {
            previous: Some(old),
            current: commit.clone(),
            created: false,
            switched: true,
            lfs_pointers: 0,
        }
    );
    assert_eq!(pair.dst.repo.head(), commit);
}

/// ① LFS: 대상에 `filter=lfs` 와 사람의 LFS 필터 설정이 있어도 맞추기가 LFS 서버로 가지 않는다 — 가짜 `git-lfs` 는
/// `GIT_LFS_SKIP_SMUDGE=1` 없이 불리면 「서버에 닿음」 을 적고 실패한다(진짜 git-lfs 가 그 변수로 하는 일). 작업 트리에는
/// 포인터가 남고 그 수가 결과에 있다. 두 갈래(체크아웃된 브랜치 · 새 브랜치) 모두.
#[tokio::test]
async fn lfs_files_stay_pointers_and_never_reach_the_lfs_server() {
    let pair = Pair::new(true);
    let bin = &pair.dst.bin;
    let reached = bin.join("git-lfs.server");
    let lfs = bin.join("git-lfs");
    std::fs::write(
        &lfs,
        format!(
            "#!/bin/sh\nif [ \"$1\" = smudge ] && [ \"$GIT_LFS_SKIP_SMUDGE\" != 1 ]; then echo smudge >> '{}'; exit 1; fi\nexec cat\n",
            reached.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&lfs, std::fs::Permissions::from_mode(0o755)).unwrap();
    let dst = &pair.dst.repo;
    dst.git(&["config", "filter.lfs.smudge", "git-lfs smudge -- %f"]);
    dst.git(&["config", "filter.lfs.clean", "git-lfs clean -- %f"]);
    dst.git(&["config", "filter.lfs.required", "true"]);
    let pointer = "version https://git-lfs.github.com/spec/v1\noid sha256:4d7a214614ab2935c943f9e0ff69d22eadbb8f32b1258daaa5e2ca24d17e2393\nsize 12345\n";
    pair.src.repo.write(
        ".gitattributes",
        "*.bin filter=lfs diff=lfs merge=lfs -text\n",
    );
    pair.src.repo.write("data/a.bin", pointer);
    let first = pair.src.repo.commit_all("lfs");

    let outcome = pair.send("main", &first).await;
    let GitSendOutcome::Applied { lfs_pointers, .. } = outcome else {
        panic!("expected applied, got {outcome:?}");
    };
    assert_eq!(lfs_pointers, 1);
    assert_eq!(
        std::fs::read_to_string(dst.root().join("data/a.bin")).unwrap(),
        pointer
    );

    pair.src.repo.git(&["switch", "--quiet", "-c", "feat"]);
    pair.src
        .repo
        .write("data/b.bin", &pointer.replace("12345", "54321"));
    let second = pair.src.repo.commit_all("lfs two");
    let outcome = pair.send("feat", &second).await;
    let GitSendOutcome::Applied {
        lfs_pointers,
        switched,
        ..
    } = outcome
    else {
        panic!("expected applied, got {outcome:?}");
    };
    assert!(switched);
    assert_eq!(lfs_pointers, 1);
    assert!(
        !reached.exists(),
        "git-lfs ran without GIT_LFS_SKIP_SMUDGE: {:?}",
        std::fs::read_to_string(&reached)
    );
}

/// ⑥ 전용 ref 치우기: 그 커밋이 같은 이름의 브랜치에 들었으면 다음 받기 때 지운다. 들지 않은 것은 남는다.
#[tokio::test]
async fn incoming_refs_already_in_their_branch_are_pruned_on_the_next_receive() {
    let pair = Pair::new(true);
    let merged = pair.src.repo.commit_file("one.txt", "1\n", "one");
    pair.send("main", &merged).await;
    pair.src.repo.git(&["switch", "--quiet", "-c", "side"]);
    let side = pair.src.repo.commit_file("side.txt", "s\n", "side");
    let prepared = pair.prepare("side", &side).await;
    // 받기만 하고 맞추지 않는다 — side 의 전용 ref 는 브랜치에 들지 않았다.
    let received = pair
        .receive_file(&prepared.file, "side", &side)
        .await
        .unwrap();
    assert_eq!(
        received.pruned,
        vec![super::super::incoming_ref(MACHINE, "main")]
    );
    assert_eq!(incoming(&pair.dst.repo, "main"), None);

    pair.src.repo.git(&["switch", "--quiet", "main"]);
    let next = pair.src.repo.commit_file("two.txt", "2\n", "two");
    let prepared = pair.prepare("main", &next).await;
    let received = pair
        .receive_file(&prepared.file, "main", &next)
        .await
        .unwrap();
    assert!(received.pruned.is_empty(), "{:?}", received.pruned);
    assert_eq!(incoming(&pair.dst.repo, "side"), Some(side));
}
