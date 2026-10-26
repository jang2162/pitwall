//! 미신뢰 프로젝트에서 충돌 읽기(`git.conflicts` · `git.conflict.get`)는 저장소 설정의 프로그램을 돌리지 않는다(결정 M2-D8,
//! 03-architecture.md 「보안」). 압축으로 받은 저장소처럼 `.git/config` 에 `core.fsmonitor` · `filter.<x>.clean` · `smudge` ·
//! `diff.external` · `textconv` · `merge.<x>.driver` 를 심고(모든 파일에 그 드라이버를 거는 `info/attributes`) 표식이 없는지 본다.
//! 그 설정이 살아 있다는 것은 사람이 누른 해결이 표식을 남기는 것으로 보인다 — 해결은 터미널의 git 과 같이 돈다(D43).

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use super::*;
use crate::conflict::methods::{
    ConflictChoice, ConflictPick, GitConflictParams, GitConflictResolveParams, GitConflictsParams,
};
use crate::testing::write::{Window, WriteFixture};
use crate::write::{Askpass, GitPromptAnswer};

fn marks(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// 갈래 충돌 중인 저장소에 프로그램을 심는다(충돌을 만든 뒤에 — 만드는 git 이 표식을 남기지 않게). 표식 폴더를 돌려준다.
fn planted(repo: &TestRepo) -> PathBuf {
    let marks = repo.base().join("marks");
    let bin = repo.base().join("planted");
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
        ("merge.mark.driver", script("merge", "exit 1")),
    ] {
        repo.git(&["config", key, &value]);
    }
    fs::write(
        repo.root().join(".git/info/attributes"),
        "* filter=mark diff=mark merge=mark\n",
    )
    .unwrap();
    marks
}

#[test]
fn reading_conflicts_of_an_untrusted_repository_runs_none_of_its_programs() {
    let repo = TestRepo::with_commit();
    repo.start_conflict_kinds();
    let marks_dir = planted(&repo);
    isolate(repo.base());
    block_on(async {
        let f = WriteFixture::with_repo(repo, false, Askpass::default());
        assert!(!f.projects.is_trusted(&f.project));
        let git = f.write.git();
        let list = git
            .conflicts(&GitConflictsParams {
                project_id: f.project.clone(),
                worktree: None,
            })
            .unwrap();
        assert_eq!(list.files.len(), crate::testing::conflict::KINDS.len());
        for file in &list.files {
            git.conflict(&GitConflictParams {
                project_id: f.project.clone(),
                worktree: None,
                path: file.path.clone(),
            })
            .unwrap();
        }
        assert_eq!(marks(&marks_dir), Vec::<String>::new());

        // 같은 설정이 살아 있다: 사람이 누른 해결은 터미널의 git 과 같이 저장소의 필터를 돌린다(D43).
        let _window = Window::open(&f.write, &f.project, |_| Some(GitPromptAnswer::Proceed));
        let expected_version = git
            .conflict(&GitConflictParams {
                project_id: f.project.clone(),
                worktree: None,
                path: "both.txt".to_owned(),
            })
            .unwrap()
            .version;
        f.write
            .resolve(GitConflictResolveParams {
                project_id: f.project.clone(),
                path: "both.txt".to_owned(),
                side: Some(ConflictChoice {
                    pick: ConflictPick::Theirs,
                    expected_version,
                }),
                ..GitConflictResolveParams::default()
            })
            .await
            .unwrap();
        let seen = marks(&marks_dir);
        assert!(
            seen.contains(&"smudge".to_owned()) && seen.contains(&"clean".to_owned()),
            "the planted filters are live for the person's write: {seen:?}"
        );
    });
}
