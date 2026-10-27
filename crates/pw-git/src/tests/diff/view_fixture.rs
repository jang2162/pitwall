//! M2-15 의 창 시험 픽스처: 커밋 하나의 `git.show` · `git.diff` · 파일 내용(`git.blob`)과 그 커밋의 `git show` patch 를 JSON 으로
//! 남긴다(`apps/desktop/src/git/diff/fixtures/commit.json`). 창 시험(`apps/desktop/src/git/documents/commit-window.test.tsx`)이 이
//! JSON 을 가짜 Host 의 답으로 쓰고, 칠한 줄을 같은 JSON 의 patch 와 견준다 — 이 시험이 그 JSON 이 지금 pw-git · git 의 출력과
//! 같은지를 본다(다르면 실패). 다시 만들 때는 `PW_UPDATE_DIFF_FIXTURE=1` 로 이 시험을 돌린다.
//!
//! 커밋 시각은 `TestRepo` 의 시계라 해시가 늘 같다. 작업 트리 경로는 `/repo` 로 바꿔 적는다. 견주는 것은 JSON 값이다 — 저장소의
//! 포매터(oxfmt)가 그 파일의 줄바꿈 · 들여쓰기를 바꿔도 같다.

use std::collections::BTreeMap;
use std::path::PathBuf;

use super::*;

const FIXTURE: &str = "../../apps/desktop/src/git/diff/fixtures/commit.json";

fn lib_rs(variant: u8) -> String {
    let mut s = String::new();
    s.push_str(if variant == 0 {
        "fn one() {\n"
    } else {
        "fn uno() {\n"
    });
    s.push('\n');
    s.push_str(if variant == 0 { "    1\n" } else { "    2\n" });
    s.push_str("}\n");
    for i in 0..40 {
        s.push_str(&format!("const C{i}: u32 = {i};\n"));
    }
    s.push_str(if variant == 0 {
        "fn tail() {}\n"
    } else {
        "fn tail() { changed() }\n"
    });
    if variant == 1 {
        s.push_str("fn added() {}\n");
    }
    s
}

fn build() -> (Repo, String) {
    let r = TestRepo::new();
    r.write("src/lib.rs", &lib_rs(0));
    r.write("src/removed.rs", "pub fn gone() {}\n");
    r.write(
        "src/old_name.rs",
        "pub fn a() {}\npub fn b() {}\npub fn c() {}\npub fn d() {}\npub fn e() {}\n",
    );
    r.write("crlf.txt", "one\r\ntwo\r\nthree\r\n");
    r.write("noeol.txt", "first\nlast");
    r.write(
        "Cargo.lock",
        "version = 3\n\n[[package]]\nname = \"a\"\nversion = \"0.1.0\"\n",
    );
    r.write("script.sh", "#!/bin/sh\necho hi\n");
    std::fs::write(r.root().join("bin.dat"), b"\0\x01\x02bin").unwrap();
    r.commit_all("base");

    r.write("src/lib.rs", &lib_rs(1));
    std::fs::remove_file(r.root().join("src/removed.rs")).unwrap();
    std::fs::remove_file(r.root().join("src/old_name.rs")).unwrap();
    r.write(
        "src/new_name.rs",
        "pub fn a() {}\npub fn b() {}\npub fn c() {}\npub fn d() {}\npub fn e2() {}\n",
    );
    r.write("src/added.rs", "pub fn new() {}\n");
    r.write("crlf.txt", "one\r\n2\r\nthree\r\n");
    r.write("noeol.txt", "first\nlast changed");
    r.write(
        "Cargo.lock",
        "version = 3\n\n[[package]]\nname = \"a\"\nversion = \"0.2.0\"\n",
    );
    std::fs::write(r.root().join("bin.dat"), b"\0\x01\x03bin").unwrap();
    r.git(&["add", "--all"]);
    // 모드는 index 로 바꾼다 — 파일 시스템의 실행 비트를 쓰지 않아 Windows 에서도 같은 커밋이다.
    r.git(&["update-index", "--chmod=+x", "script.sh"]);
    r.git(&[
        "commit",
        "--quiet",
        "-m",
        "change things",
        "-m",
        "Body of the commit.",
    ]);
    let commit = r.head();
    (Repo::new(r), commit)
}

fn fixture_value() -> serde_json::Value {
    let (repo, commit) = build();
    let show = repo.f.git.show(&repo.rev(&commit)).unwrap();
    let mut diff = repo.diff(
        None,
        DiffSide::Commit {
            rev: commit.clone(),
        },
    );
    let mut blobs = BTreeMap::new();
    for file in &diff.files {
        let sides = [
            (
                diff.old_commit.clone(),
                file.old_path.clone().unwrap_or(file.path.clone()),
                file.status != FileStatus::Added,
            ),
            (
                diff.new_commit.clone(),
                file.path.clone(),
                file.status != FileStatus::Deleted,
            ),
        ];
        for (rev, path, exists) in sides {
            let Some(rev) = rev.filter(|_| exists) else {
                continue;
            };
            // 텍스트로 읽힌 쪽만 싣는다(바이너리는 창이 읽지 않는다).
            let blob = serde_json::to_value(repo.blob(Some(&rev), &path)).unwrap();
            if blob["content"]["kind"] == "text" {
                blobs.insert(format!("{rev}:{path}"), blob);
            }
        }
    }
    diff.work_tree = "/repo".to_owned();
    let patch = repo.r.git(&[
        "show",
        "--format=",
        "--find-renames",
        "-U3",
        "--no-color",
        "--no-ext-diff",
        &commit,
    ]);
    serde_json::json!({
        "commit": commit,
        "show": show,
        "diff": diff,
        "blobs": blobs,
        "patch": patch,
    })
}

#[test]
fn commit_view_fixture_matches_pw_git_and_git() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(FIXTURE);
    let got = fixture_value();
    if std::env::var_os("PW_UPDATE_DIFF_FIXTURE").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let text = serde_json::to_string_pretty(&got).unwrap();
        std::fs::write(&path, format!("{text}\n")).unwrap();
        return;
    }
    let want: serde_json::Value = std::fs::read_to_string(&path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();
    assert!(
        got == want,
        "{} 이 지금 pw-git · git 의 출력과 다르다 — PW_UPDATE_DIFF_FIXTURE=1 로 이 시험을 돌려 다시 만든다",
        path.display()
    );
}
