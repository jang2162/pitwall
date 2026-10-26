//! 미신뢰 프로젝트의 diff · 커밋 상세 · blob 읽기는 저장소 설정의 프로그램을 돌리지 않는다(결정 M2-D8, 03-architecture.md
//! 「보안」). 압축으로 받은 저장소처럼 `.git/config` 에 `core.fsmonitor` · `filter.<x>.clean` · `diff.external` ·
//! `diff.<x>.textconv` 를 심고(모든 파일에 그 필터 · diff 드라이버를 거는 `info/attributes`), 네 짝의 `git.diff` 와 `git.show` ·
//! `git.containedIn` · `git.blob` 이 표식을 남기지 않는지 본다. 신뢰하면 사람의 설정대로 clean 필터가 돈다 — 위의 「안 남긴다」 가
//! 신뢰로 갈린다.

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use super::*;

/// 부를 때마다 `marks/<이름>` 에 한 줄을 남기는 프로그램을 심은 저장소. 커밋 둘, 스테이징한 변경, 작업 트리 변경이 있다.
fn planted() -> (Repo, PathBuf) {
    let r = TestRepo::with_commit();
    r.commit_file("README.md", "hello\nagain\n", "second");
    let marks = r.base().join("marks");
    let bin = r.base().join("planted");
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
        ("diff.mark.command", script("command", "exit 0")),
    ] {
        r.git(&["config", key, &value]);
    }
    fs::write(
        r.root().join(".git/info/attributes"),
        "* filter=mark diff=mark\n",
    )
    .unwrap();
    r.write("staged.txt", "staged\n");
    r.git(&["add", "staged.txt"]);
    // 내용과 stat 을 바꿔 작업 트리 diff 가 그 파일을 읽게 한다.
    r.write("README.md", "hello\nchanged\n");
    let later = std::time::SystemTime::now() + Duration::from_secs(5);
    fs::File::options()
        .write(true)
        .open(r.root().join("README.md"))
        .unwrap()
        .set_modified(later)
        .unwrap();
    // 심기 전에 남은 표식(설정을 쓰는 git 이 부른 것)은 지운다.
    for e in fs::read_dir(&marks).unwrap() {
        fs::remove_file(e.unwrap().path()).unwrap();
    }
    (Repo::new(r), marks)
}

fn marks(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// 네 짝의 diff, 커밋 상세, 담은 ref, blob(리비전 · index).
fn read_everything(repo: &Repo) {
    for (from, to) in [
        (None, commit("HEAD")),
        (Some(commit("HEAD~1")), commit("HEAD")),
        (None, DiffSide::Index),
        (None, DiffSide::WorkTree),
    ] {
        let diff = repo.diff(from, to);
        assert!(!diff.files.is_empty());
    }
    repo.f.git.show(&repo.rev("HEAD")).unwrap();
    repo.f.git.contained_in(&repo.rev("HEAD~1")).unwrap();
    repo.blob(Some("HEAD"), "README.md");
    repo.blob(None, "staged.txt");
}

#[test]
fn an_untrusted_repository_runs_none_of_its_programs_on_diff_show_and_blob_reads() {
    let (repo, dir) = planted();
    assert!(!repo.f.projects.is_trusted(&repo.id));
    read_everything(&repo);
    // 작업 트리 diff 가 그 파일을 실제로 읽었다(필터를 거를 자리가 있었다).
    let worktree = repo.diff(None, DiffSide::WorkTree);
    assert!(
        worktree
            .files
            .iter()
            .any(|f| f.path == "README.md" && !f.hunks.is_empty())
    );
    assert_eq!(marks(&dir), Vec::<String>::new());

    // 신뢰하면 작업 트리 파일을 저장소 꼴로 바꾸는 clean 필터가 git 처럼 돈다. textconv · 외부 diff(`diff.external` ·
    // `diff.<x>.command`)는 신뢰해도 쓰지 않는다 — hunk 는 저장소에 담긴 내용 그대로다(M2-07 의 줄 단위 스테이징이 그 줄을 쓴다).
    repo.f.trust(&repo.id, true);
    read_everything(&repo);
    assert_eq!(marks(&dir), ["clean"]);
}
