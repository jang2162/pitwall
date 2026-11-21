//! origin · 서브모듈 원격 · 로그에 닿지 않는다(① ⑦).

use std::sync::{Arc, Mutex, PoisonError};

use super::super::apply::GitSendOutcome;
use super::*;

/// ① origin 이 있는 저장소에서 보내도 원본 · 대상이 실행한 git 에 `push` · `fetch origin` · `remote` · `config` 가 없고, origin
/// 의 원격 도우미(덫)가 불리지 않는다.
#[tokio::test]
async fn sending_never_touches_origin_or_remote_config() {
    let pair = Pair::new(true);
    let src_trap = trap(&pair.src.bin, "git-remote-trap");
    let dst_trap = trap(&pair.dst.bin, "git-remote-trap");
    for repo in [&pair.src.repo, &pair.dst.repo] {
        repo.git(&["remote", "add", "origin", "trap::origin"]);
        repo.git(&["config", "branch.main.remote", "origin"]);
        repo.git(&["config", "branch.main.merge", "refs/heads/main"]);
        repo.git(&["config", "fetch.prune", "true"]);
    }
    let commit = pair.src.repo.commit_file("a.txt", "a\n", "a");
    let outcome = pair.send("main", &commit).await;
    assert!(
        matches!(outcome, GitSendOutcome::Applied { .. }),
        "{outcome:?}"
    );
    pair.src.repo.git(&["switch", "--quiet", "-c", "feat"]);
    let commit = pair.src.repo.commit_file("b.txt", "b\n", "b");
    let outcome = pair.send("feat", &commit).await;
    assert!(
        matches!(outcome, GitSendOutcome::Applied { switched: true, .. }),
        "{outcome:?}"
    );

    let all: Vec<Vec<String>> = pair
        .src_calls()
        .into_iter()
        .chain(pair.dst_calls())
        .collect();
    let mut fetches = 0;
    for argv in &all {
        let (sub, rest) = subcommand(argv);
        assert!(
            !["push", "remote", "config", "pull", "ls-remote"].contains(&sub.as_str()),
            "{argv:?}"
        );
        if sub == "fetch" {
            fetches += 1;
            assert!(
                rest.iter().all(|a| a != "origin"
                    && !a.starts_with("trap::")
                    && !a.starts_with("--recurse-submodules")),
                "{argv:?}"
            );
            assert!(rest.iter().any(|a| a.ends_with(".bundle")), "{argv:?}");
        }
    }
    assert_eq!(fetches, 2, "{all:?}");
    assert!(
        all.iter().any(|argv| subcommand(argv).0 == "bundle"),
        "{all:?}"
    );
    assert!(!src_trap.exists() && !dst_trap.exists());
}

/// ① `submodule.recurse=true` 인 대상에서도 받기 · 맞추기가 서브모듈의 원격으로 fetch 하지 않는다(서브모듈의 origin 은 덫).
#[tokio::test]
async fn submodule_recurse_does_not_reach_submodule_remotes() {
    let sub = TestRepo::with_commit();
    let src = TestRepo::with_commit();
    let sub_url = sub.root().to_str().unwrap().to_owned();
    src.git(&[
        "-c",
        "protocol.file.allow=always",
        "submodule",
        "add",
        "--quiet",
        &sub_url,
        "sub",
    ]);
    src.commit_all("add sub");
    let dst = clone_of(&src);
    dst.git(&[
        "-c",
        "protocol.file.allow=always",
        "submodule",
        "update",
        "--init",
        "--quiet",
    ]);
    dst.git_in(
        &dst.root().join("sub"),
        &["remote", "set-url", "origin", "trap::sub"],
    );
    dst.git(&["config", "submodule.recurse", "true"]);
    let pair = Pair::of(src, dst, true);
    let marker = trap(&pair.dst.bin, "git-remote-trap");
    // 원본에서 서브모듈을 한 커밋 올린다 — 대상의 서브모듈에는 그 커밋이 없다.
    let src_sub = pair.src.repo.root().join("sub");
    std::fs::write(src_sub.join("more.txt"), "more\n").unwrap();
    pair.src.repo.commit_all_in(&src_sub, "sub: more");
    let commit = pair.src.repo.commit_all("bump sub");

    let outcome = pair.send("main", &commit).await;

    assert!(
        matches!(outcome, GitSendOutcome::Applied { .. }),
        "{outcome:?}"
    );
    assert!(
        !marker.exists(),
        "a submodule remote was contacted: {:?}",
        std::fs::read_to_string(&marker)
    );
    assert_eq!(pair.dst.repo.head(), commit);
}

#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Captured {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// ⑦ 로그에 묶음 바이트가 없다: 받는 청크의 base64 도, 팩의 바이트도 trace 까지의 로그에 나오지 않는다. 잘못된 청크를
/// 거절하는 오류 문장에도 청크가 없다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn logs_never_carry_bundle_bytes() {
    use base64::Engine as _;
    let captured = Captured::default();
    let sink = captured.clone();
    tracing::subscriber::set_global_default(
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_ansi(false)
            .with_writer(move || sink.clone())
            .finish(),
    )
    .unwrap();
    let pair = Pair::new(true);
    pair.src
        .repo
        .write("big.txt", &"payload line that compresses\n".repeat(2000));
    let commit = pair.src.repo.commit_all("big");
    let prepared = pair.prepare("main", &commit).await;
    let bytes = std::fs::read(&prepared.file).unwrap();
    pair.receive_file(&prepared.file, "main", &commit)
        .await
        .unwrap();
    let tip = pair.theirs("main").await.branch_tip;
    pair.apply("main", &commit, tip).await.unwrap();
    // 잘못된 offset 의 청크.
    let uploads = Arc::new(Uploads::default());
    let owner = UploadOwner::test(7, Role::Owner);
    let id = uploads
        .begin(
            &pair.dst.write,
            owner,
            begin(&pair.dst, "main", &commit, bytes.len() as u64),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let encoded =
        base64::engine::general_purpose::STANDARD.encode(&bytes[..bytes.len().min(CHUNK)]);
    let error = uploads.chunk(owner, id, 5, &encoded).await.unwrap_err();

    let log = String::from_utf8_lossy(&captured.0.lock().unwrap()).into_owned();
    let pack = bytes
        .windows(4)
        .position(|w| w == b"PACK")
        .expect("a bundle has a pack");
    let pack_head = base64::engine::general_purpose::STANDARD.encode(&bytes[pack..pack + 48]);
    for chunk in bytes.chunks(CHUNK) {
        let data = base64::engine::general_purpose::STANDARD.encode(chunk);
        assert!(!log.contains(&data[..64]), "a chunk is in the log");
    }
    assert!(!log.contains(&pack_head[..32]));
    assert!(!log.contains("PACK"), "pack bytes are in the log");
    let text = format!("{error} {error:?}");
    assert!(!text.contains(&encoded[..64]), "{text}");
}
