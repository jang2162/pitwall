//! 픽스처(이름 바뀜 · 바이너리 · CRLF · 파일 끝 개행 없음 · 모드 변경 · 서브모듈 포인터 · 머지 커밋)마다 파일 목록과 hunk 가
//! `git diff --find-renames -U3` 과 같다(M2-04 완료 기준 ①). 네 짝(커밋 ↔ 부모 · 두 커밋 · HEAD ↔ index · index ↔ 작업 트리)을
//! 모두 본다.

use super::*;

fn numbered(range: std::ops::RangeInclusive<u32>) -> String {
    range.map(|i| format!("line {i}\n")).collect()
}

#[test]
fn renames_with_and_without_edits_match_git() {
    let r = TestRepo::with_commit();
    r.write("old/name.txt", &numbered(1..=20));
    r.write("moved.rs", &numbered(1..=30));
    r.write("gone.txt", "bye\n");
    r.commit_all("base");
    fs::create_dir_all(r.root().join("new")).unwrap();
    fs::create_dir_all(r.root().join("src")).unwrap();
    r.git(&["mv", "old/name.txt", "new/name.txt"]);
    r.git(&["mv", "moved.rs", "src/moved.rs"]);
    r.write(
        "src/moved.rs",
        &numbered(1..=30).replace("line 15\n", "line fifteen\n"),
    );
    r.git(&["rm", "-q", "gone.txt"]);
    r.write("fresh.txt", "new\n");
    r.commit_all("move");
    let repo = Repo::new(r);
    assert_same_as_git(&repo, None, commit("HEAD"), &["HEAD~1", "HEAD"]);
    let statuses: Vec<_> = repo
        .diff(None, commit("HEAD"))
        .files
        .iter()
        .map(|f| (f.status, f.path.clone(), f.old_path.clone()))
        .collect();
    assert!(statuses.contains(&(
        FileStatus::Renamed,
        "src/moved.rs".into(),
        Some("moved.rs".into())
    )));
}

#[test]
fn binary_files_match_git() {
    let r = TestRepo::with_commit();
    r.write("bin.dat", "abc\0def\n");
    r.write("text.txt", "a\nb\n");
    r.commit_all("base");
    r.write("bin.dat", "abc\0xyz\n");
    r.write("added.bin", "\0\0\0");
    // 텍스트였던 파일이 바이너리가 된다.
    r.write("text.txt", "a\0b\n");
    r.commit_all("binary");
    let repo = Repo::new(r);
    assert_same_as_git(&repo, None, commit("HEAD"), &["HEAD~1", "HEAD"]);
    let diff = repo.diff(None, commit("HEAD"));
    assert!(diff.files.iter().all(|f| f.binary && f.hunks.is_empty()));
}

#[test]
fn crlf_lines_match_git() {
    let r = TestRepo::with_commit();
    r.write("crlf.txt", "one\r\ntwo\r\nthree\r\nfour\r\n");
    r.write("mixed.txt", "a\nb\r\nc\n");
    r.commit_all("base");
    r.write("crlf.txt", "one\r\nTWO\r\nthree\r\nfour\r\nfive\r\n");
    r.write("mixed.txt", "a\r\nb\r\nc\n");
    r.commit_all("crlf");
    let repo = Repo::new(r);
    assert_same_as_git(&repo, None, commit("HEAD"), &["HEAD~1", "HEAD"]);
    let diff = repo.diff(None, commit("HEAD"));
    let crlf = diff.files.iter().find(|f| f.path == "crlf.txt").unwrap();
    assert!(crlf.hunks[0].lines.iter().all(|l| l.text.ends_with('\r')));
}

#[test]
fn a_missing_newline_at_the_end_matches_git() {
    let r = TestRepo::with_commit();
    r.write("grow.txt", "x\ny");
    r.write("cut.txt", "1\n2\n");
    r.write("both.txt", "p\nq");
    r.write("same-end.txt", "s\nt\nu\nv\nw");
    r.commit_all("base");
    r.write("grow.txt", "x\ny\nz\n");
    r.write("cut.txt", "1\n2");
    r.write("both.txt", "p\nr");
    r.write("same-end.txt", "S\nt\nu\nv\nw");
    r.commit_all("eol");
    let repo = Repo::new(r);
    assert_same_as_git(&repo, None, commit("HEAD"), &["HEAD~1", "HEAD"]);
}

#[test]
fn a_mode_change_matches_git() {
    let r = TestRepo::with_commit();
    r.write("run.sh", "#!/bin/sh\necho run\n");
    r.write("both.sh", "#!/bin/sh\necho both\n");
    r.commit_all("base");
    r.write("both.sh", "#!/bin/sh\necho both again\n");
    r.git(&["add", "both.sh"]);
    r.git(&["update-index", "--chmod=+x", "run.sh", "both.sh"]);
    r.git(&["commit", "-q", "-m", "chmod"]);
    let repo = Repo::new(r);
    assert_same_as_git(&repo, None, commit("HEAD"), &["HEAD~1", "HEAD"]);
    let diff = repo.diff(None, commit("HEAD"));
    let run = diff.files.iter().find(|f| f.path == "run.sh").unwrap();
    assert_eq!(
        (
            run.old_mode.as_deref(),
            run.new_mode.as_deref(),
            run.hunks.len()
        ),
        (Some("100644"), Some("100755"), 0)
    );
}

/// 작업 트리 안의 중첩 저장소 `sub` 를 서브모듈(gitlink)로 담는다. 서브모듈의 커밋 둘을 돌려준다.
fn with_submodule(r: &TestRepo) -> (String, String) {
    let sub = r.root().join("sub");
    fs::create_dir_all(&sub).unwrap();
    r.git_in(&sub, &["init", "-q"]);
    fs::write(sub.join("s.txt"), "one\n").unwrap();
    let first = r.commit_all_in(&sub, "s1");
    r.commit_all("add sub");
    fs::write(sub.join("s.txt"), "two\n").unwrap();
    let second = r.commit_all_in(&sub, "s2");
    (first, second)
}

#[test]
fn a_submodule_pointer_matches_git() {
    let r = TestRepo::with_commit();
    let (first, second) = with_submodule(&r);
    r.commit_all("bump sub");
    let repo = Repo::new(r);
    // 더한 커밋 · 옮긴 커밋.
    assert_same_as_git(&repo, None, commit("HEAD~1"), &["HEAD~2", "HEAD~1"]);
    assert_same_as_git(&repo, None, commit("HEAD"), &["HEAD~1", "HEAD"]);
    let diff = repo.diff(None, commit("HEAD"));
    let sub = diff.files[0].submodule.clone().unwrap();
    assert_eq!((sub.old, sub.new), (Some(first), Some(second)));

    // 서브모듈 안에서 커밋하면 index ↔ 작업 트리에 포인터 변경으로 보인다.
    let s = repo.r.root().join("sub");
    fs::write(s.join("s.txt"), "three\n").unwrap();
    let third = repo.r.commit_all_in(&s, "s3");
    let diff = repo.diff(None, DiffSide::WorkTree);
    assert_eq!(diff.files.len(), 1);
    assert_eq!(
        diff.files[0].submodule.as_ref().unwrap().new.as_deref(),
        Some(third.as_str())
    );
    assert_eq!(
        our_patches(&diff)[0].hunks,
        git_patches(&repo.r, &[])[0].hunks
    );
}

#[test]
fn a_merge_commit_is_diffed_against_its_first_parent_like_git() {
    let r = TestRepo::with_commit();
    r.write("f1.txt", &numbered(1..=10));
    r.write("f2.txt", &numbered(1..=10));
    r.commit_all("base");
    r.git(&["checkout", "-q", "-b", "feat"]);
    r.write("f1.txt", &numbered(1..=10).replace("line 3\n", "feat 3\n"));
    r.write("f3.txt", "from feat\n");
    r.commit_all("feat");
    r.checkout("main");
    r.write("f2.txt", &numbered(1..=10).replace("line 8\n", "main 8\n"));
    r.commit_all("main");
    r.git(&["merge", "-q", "--no-ff", "-m", "merge feat", "feat"]);
    let repo = Repo::new(r);
    assert_eq!(
        repo.r
            .git(&["rev-list", "--parents", "-n1", "HEAD"])
            .split(' ')
            .count(),
        3
    );
    assert_same_as_git(&repo, None, commit("HEAD"), &["HEAD^1", "HEAD"]);
    let diff = repo.diff(None, commit("HEAD"));
    assert_eq!(diff.old_commit, Some(repo.r.rev_parse("HEAD^1")));
    let paths: Vec<_> = diff.files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(paths, ["f1.txt", "f3.txt"]);
}

#[test]
fn the_root_commit_is_diffed_against_the_empty_tree_and_two_commits_against_each_other() {
    let r = TestRepo::with_commit();
    r.commit_file("a.txt", "a\n", "a");
    r.commit_file("a.txt", "a\nb\n", "b");
    let repo = Repo::new(r);
    let root = repo.r.git(&["rev-list", "--max-parents=0", "HEAD"]);
    let root = root.trim();
    assert_same_as_git(&repo, None, commit(root), &[EMPTY_TREE, root]);
    assert_eq!(repo.diff(None, commit(root)).old_commit, None);
    assert_same_as_git(&repo, Some(commit(root)), commit("HEAD"), &[root, "HEAD"]);
    // 거꾸로도.
    assert_same_as_git(&repo, Some(commit("HEAD")), commit(root), &["HEAD", root]);
}

#[test]
fn head_to_index_and_index_to_work_tree_match_git() {
    let r = TestRepo::with_commit();
    r.write("staged.txt", &numbered(1..=12));
    r.write("unstaged.txt", &numbered(1..=12));
    r.write("deleted.txt", "x\n");
    r.write("both.txt", "1\n2\n3\n");
    r.commit_all("base");
    r.write("staged.txt", &numbered(1..=12).replace("line 6\n", "six\n"));
    r.write("new.txt", "brand new\n");
    r.write("both.txt", "1\nTWO\n3\n");
    r.git(&["add", "staged.txt", "new.txt", "both.txt"]);
    r.git(&["rm", "-q", "deleted.txt"]);
    r.write("unstaged.txt", &numbered(2..=13));
    r.write("both.txt", "1\nTWO\n3\nfour\n");
    let repo = Repo::new(r);
    assert_same_as_git(&repo, None, DiffSide::Index, &["--cached"]);
    assert_same_as_git(
        &repo,
        Some(commit("HEAD~1")),
        DiffSide::Index,
        &["--cached", "HEAD~1"],
    );
    assert_same_as_git(&repo, None, DiffSide::WorkTree, &[]);
    assert_same_as_git(&repo, Some(DiffSide::Index), DiffSide::WorkTree, &[]);
    // 작업 트리에서 지운 파일, 실행 비트만 바꾼 파일(unix).
    fs::remove_file(repo.r.root().join("unstaged.txt")).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let script = repo.r.root().join("README.md");
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    }
    assert_same_as_git(&repo, None, DiffSide::WorkTree, &[]);
}

#[test]
fn a_repository_without_commits_diffs_the_index_against_the_empty_tree() {
    let r = TestRepo::new();
    r.write("first.txt", "first\n");
    r.git(&["add", "first.txt"]);
    let repo = Repo::new(r);
    assert_same_as_git(&repo, None, DiffSide::Index, &["--cached"]);
}

#[test]
fn function_headers_and_hunk_merging_match_git() {
    let mut text = String::new();
    for f in 0..6 {
        text.push_str(&format!("fn function_{f}() {{\n"));
        for i in 0..12 {
            text.push_str(&format!("    let v{i} = {i};\n"));
        }
        text.push_str("}\n\n");
    }
    // 80바이트를 넘는 함수 줄은 잘린다.
    text.push_str(&format!(
        "fn {}() {{\n    body();\n}}\n",
        "long_name_".repeat(10)
    ));
    // 함수 줄 하나 아래의 긴 본문: 둘째 hunk 와 첫째 hunk 사이에는 함수 줄이 없어 git 은 앞 hunk 의 것을 이어 쓴다.
    let long: String = std::iter::once("fn long() {\n".to_owned())
        .chain((0..40).map(|i| format!("    step({i});\n")))
        .chain(std::iter::once("}\n".to_owned()))
        .collect();
    let r = TestRepo::with_commit();
    r.write("code.rs", &text);
    r.write("long.rs", &long);
    r.commit_all("base");
    r.write(
        "long.rs",
        &long
            .replace("    step(4);\n", "    step(-4);\n")
            .replace("    step(30);\n", "    step(-30);\n"),
    );
    let changed = text
        .replace("    let v2 = 2;\n", "    let v2 = 20;\n")
        // 6줄 떨어진 변경은 한 hunk, 7줄이면 따로.
        .replacen("    let v9 = 9;\n", "    let v9 = 90;\n", 1)
        .replace("    body();\n", "    body2();\n");
    let changed = {
        let mut lines: Vec<&str> = changed.split_inclusive('\n').collect();
        // function_3 안: 6줄 사이를 둔 두 변경 · function_4 안: 7줄 사이를 둔 두 변경.
        let at = |lines: &[&str], name: &str| lines.iter().position(|l| l.contains(name)).unwrap();
        let f3 = at(&lines, "fn function_3");
        lines[f3 + 2] = "    changed_a();\n";
        lines[f3 + 9] = "    changed_b();\n";
        let f4 = at(&lines, "fn function_4");
        lines[f4 + 1] = "    changed_c();\n";
        lines[f4 + 9] = "    changed_d();\n";
        lines.concat()
    };
    r.write("code.rs", &changed);
    r.commit_all("edit");
    let repo = Repo::new(r);
    assert_same_as_git(&repo, None, commit("HEAD"), &["HEAD~1", "HEAD"]);
    let diff = repo.diff(None, commit("HEAD"));
    assert!(
        diff.files[0]
            .hunks
            .iter()
            .any(|h| h.header.starts_with("fn function_"))
    );
    let long: Vec<&str> = diff.files[1]
        .hunks
        .iter()
        .map(|h| h.header.as_str())
        .collect();
    assert_eq!(long, ["fn long() {", "fn long() {"]);
}

#[test]
fn ignoring_whitespace_matches_git_dash_w() {
    let r = TestRepo::with_commit();
    r.write("ws-only.txt", "a b\nc\n");
    r.write("mixed.txt", "keep  this\nalpha\nbeta\n");
    r.write("chmod-ws.txt", "x\n");
    r.commit_all("base");
    r.write("ws-only.txt", "a  b \nc\r\n");
    r.write("mixed.txt", "keep this\nalpha\nBETA\n");
    r.write("chmod-ws.txt", "x \n");
    r.git(&["add", "-A"]);
    r.git(&["update-index", "--chmod=+x", "chmod-ws.txt"]);
    r.git(&["commit", "-q", "-m", "ws"]);
    let repo = Repo::new(r);
    let mut params = repo.params(None, commit("HEAD"));
    params.ignore_whitespace = true;
    let ours = our_patches(&repo.f.git.diff(&params).unwrap());
    let theirs = git_patches(&repo.r, &["-w", "HEAD~1", "HEAD"]);
    assert_eq!(ours, theirs);
    let paths: Vec<_> = ours.iter().map(|p| p.path.as_str()).collect();
    // 공백만 바뀐 파일은 빠지고, 공백 + 모드는 남는다.
    assert_eq!(paths, ["chmod-ws.txt", "mixed.txt"]);
}

#[test]
fn a_path_filter_matches_git_with_a_pathspec() {
    let r = TestRepo::with_commit();
    r.write("dir/a.txt", "a\n");
    r.write("dir/sub/b.txt", "b\n");
    r.write("dirx/c.txt", "c\n");
    r.write("other.txt", "o\n");
    r.commit_all("base");
    for p in ["dir/a.txt", "dir/sub/b.txt", "dirx/c.txt", "other.txt"] {
        r.write(p, "changed\n");
    }
    r.git(&["mv", "other.txt", "dir/other.txt"]);
    r.commit_all("edit");
    let repo = Repo::new(r);
    let mut params = repo.params(None, commit("HEAD"));
    params.paths = Some(vec!["dir".into()]);
    let ours = our_patches(&repo.f.git.diff(&params).unwrap());
    assert_eq!(ours, git_patches(&repo.r, &["HEAD~1", "HEAD", "--", "dir"]));
    // `dir` 는 `dirx` 를 담지 않고, 거른 밖에서 온 이름 바뀜은 더하기로 보인다(git 과 같다).
    let paths: Vec<_> = ours.iter().map(|p| (p.status, p.path.as_str())).collect();
    assert_eq!(
        paths,
        [
            (FileStatus::Modified, "dir/a.txt"),
            (FileStatus::Added, "dir/other.txt"),
            (FileStatus::Modified, "dir/sub/b.txt"),
        ]
    );
    params.paths = Some(vec!["/abs".into()]);
    assert_eq!(
        repo.f.git.diff(&params).unwrap_err().code,
        pw_rpc::code::INVALID_PARAMS
    );
}

/// 시드를 고정한 무작위 텍스트 쌍 200개(들여쓰기 · 빈 줄 · 괄호처럼 변경 구간이 미끄러지는 줄이 많다)의 hunk 가 git 과 같다 —
/// 알고리즘 · 들여쓰기 휴리스틱 · 문맥 묶기 · 함수 줄 · 파일 끝 개행이 git 과 같은지 넓게 본다. 공백 무시도.
#[test]
fn random_edits_match_git() {
    const POOL: [&str; 12] = [
        "a\n",
        "b\n",
        "c\n",
        "\n",
        "{\n",
        "}\n",
        "    if x {\n",
        "    }\n",
        "fn f() {\n",
        "\tfoo();\n",
        "  return 0;\n",
        "x  y\n",
    ];
    let mut seed: u64 = 0x5eed_1234_abcd_0042;
    let mut next = move |n: usize| -> usize {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((seed >> 33) as usize) % n
    };
    let r = TestRepo::with_commit();
    let mut after = Vec::new();
    for i in 0..200 {
        let mut lines: Vec<String> = (0..next(40))
            .map(|_| POOL[next(POOL.len())].to_owned())
            .collect();
        let mut text: String = lines.concat();
        if next(8) == 0 {
            text = text.trim_end_matches('\n').to_owned();
        }
        r.write(&format!("f{i:03}.txt"), &text);
        for _ in 0..1 + next(6) {
            let at = if lines.is_empty() {
                0
            } else {
                next(lines.len() + 1)
            };
            match next(3) {
                0 => lines.insert(at, POOL[next(POOL.len())].to_owned()),
                1 if at < lines.len() => {
                    lines.remove(at);
                }
                _ if at < lines.len() => lines[at] = POOL[next(POOL.len())].to_owned(),
                _ => lines.push(POOL[next(POOL.len())].to_owned()),
            }
        }
        let mut changed = lines.concat();
        if next(8) == 0 {
            changed = changed.trim_end_matches('\n').to_owned();
        }
        after.push(changed);
    }
    r.commit_all("before");
    for (i, text) in after.iter().enumerate() {
        r.write(&format!("f{i:03}.txt"), text);
    }
    r.commit_all("after");
    let repo = Repo::new(r);
    assert_same_as_git(&repo, None, commit("HEAD"), &["HEAD~1", "HEAD"]);
    let mut params = repo.params(None, commit("HEAD"));
    params.ignore_whitespace = true;
    let ours = our_patches(&repo.f.git.diff(&params).unwrap());
    assert_patches_eq(
        &ours,
        &git_patches(&repo.r, &["-w", "HEAD~1", "HEAD"]),
        &["-w"],
    );
}

/// 고를 수 없는 짝 · 없는 리비전 · 커밋이 아닌 리비전은 invalid params.
#[test]
fn unsupported_sides_and_bad_revisions_are_invalid_params() {
    let repo = Repo::new(TestRepo::with_commit());
    let tree = repo.r.rev_parse("HEAD^{tree}");
    for (from, to) in [
        (Some(DiffSide::WorkTree), commit("HEAD")),
        (Some(DiffSide::Index), commit("HEAD")),
        (Some(commit("HEAD")), DiffSide::WorkTree),
        (Some(DiffSide::WorkTree), DiffSide::Index),
        (None, commit("no-such-branch")),
        (None, commit(&tree)),
    ] {
        let err = repo
            .f
            .git
            .diff(&repo.params(from.clone(), to.clone()))
            .unwrap_err();
        assert_eq!(
            err.code,
            pw_rpc::code::INVALID_PARAMS,
            "{from:?} → {to:?}: {err:?}"
        );
    }
}

/// 저장소가 없는 프로젝트는 invalid params.
#[test]
fn a_project_without_a_repository_is_invalid_params() {
    let dir = tempfile::tempdir().unwrap();
    let f = Fixture::new("", None);
    let id = f.project(&dir.path().join("plain"));
    let err = f
        .git
        .diff(&GitDiffParams {
            project_id: id,
            worktree: None,
            from: None,
            to: DiffSide::WorkTree,
            paths: None,
            ignore_whitespace: false,
        })
        .unwrap_err();
    assert_eq!(err.code, pw_rpc::code::INVALID_PARAMS);
}

/// git RPC 의 경로 꼴(공통 규칙): 작업 트리 꼭대기 기준 상대 경로, 구분자 `/` — Windows 에서도. 응답의 `workTree` 는
/// `git.repo` 의 것과 같다. 입력 경로도 같은 꼴이라 역슬래시 구분은 받지 않는다.
#[test]
fn paths_are_relative_to_the_work_tree_and_separated_by_slashes() {
    let r = TestRepo::with_commit();
    r.write("dir/sub/deep.txt", "a\n");
    r.commit_all("base");
    r.write("dir/sub/deep.txt", "b\n");
    r.commit_all("edit");
    let repo = Repo::new(r);
    let diff = repo.diff(None, commit("HEAD"));
    assert_eq!(diff.files[0].path, "dir/sub/deep.txt");
    let info = repo.f.repo(&repo.id, None).unwrap().repo.unwrap();
    assert_eq!(diff.work_tree, info.work_tree);
    assert_eq!(repo.blob(Some("HEAD"), "dir/sub/deep.txt").size, 2);
    let mut params = repo.params(None, commit("HEAD"));
    params.paths = Some(vec![r"dir\sub".into()]);
    assert_eq!(
        repo.f.git.diff(&params).unwrap_err().code,
        pw_rpc::code::INVALID_PARAMS
    );
    let backslashed = repo.blob_params(Some("HEAD"), r"dir\sub\deep.txt");
    assert_eq!(
        repo.f.git.blob(&backslashed).unwrap_err().code,
        pw_rpc::code::INVALID_PARAMS
    );
}

/// 시험의 gix 는 사람의 전역 설정을 읽지 않는다(`isolate_global_config`): 사람의 HOME 에 모든 파일을 `-diff` 로 만드는
/// `core.attributesFile` 이 있어도 diff 가 빈 전역 설정의 git 과 같다.
#[test]
fn tests_do_not_read_the_persons_global_git_config() {
    let r = TestRepo::with_commit();
    r.commit_file("README.md", "hello\nagain\n", "second");
    let person = r.base().join("person");
    fs::create_dir_all(&person).unwrap();
    fs::write(person.join("attributes"), "* -diff\n").unwrap();
    fs::write(
        person.join(".gitconfig"),
        format!(
            "[core]\n\tattributesFile = {}\n",
            person.join("attributes").display()
        ),
    )
    .unwrap();
    // SAFETY: nextest 는 시험마다 프로세스가 따로이고, 이 시험은 아직 다른 스레드를 띄우지 않았다.
    unsafe {
        std::env::set_var("HOME", &person);
        std::env::remove_var("GIT_CONFIG_GLOBAL");
        std::env::remove_var("XDG_CONFIG_HOME");
    }
    let repo = Repo::new(r);
    assert_same_as_git(&repo, None, commit("HEAD"), &["HEAD~1", "HEAD"]);
}
