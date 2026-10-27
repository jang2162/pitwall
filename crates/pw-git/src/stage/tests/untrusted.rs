//! 미신뢰 프로젝트(결정 M2-D8, 03-architecture.md 「보안」): stash 목록과 줄 단위 쓰기가 diff 를 다시 만드는 읽기는 저장소 설정의
//! 프로그램을 돌리지 않는다. 압축으로 받은 저장소처럼 `.git/config` 에 `core.fsmonitor` · `filter.<x>.clean` · `diff.external` ·
//! `diff.<x>.textconv` 를 심고 표식이 남지 않는지 본다. 사람이 누른 쓰기(`git.stage`)는 터미널의 git 과 같이 그 설정으로 돈다(D43).

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use pw_rpc::code as rpc_code;

use super::{diff, select};
use crate::diff::methods::{DiffSide, LineKind};
use crate::stage::methods::{GitLinesParams, GitPathsParams, GitStashListParams};
use crate::testing::TestRepo;
use crate::testing::write::WriteFixture;
use crate::write::{Askpass, WriteError};

struct Planted {
    f: WriteFixture,
    marks: PathBuf,
}

impl Planted {
    /// stash 하나와 커밋된 `f.txt` 가 있는 미신뢰 저장소에 프로그램을 심는다(만든 뒤에 심는다 — 만드는 git 이 표식을 남기지 않게).
    fn new() -> Self {
        let repo = TestRepo::with_commit();
        repo.commit_file("f.txt", "1\n2\n3\n", "f");
        repo.write("README.md", "stashed\n");
        repo.git(&["stash", "push", "--quiet", "-m", "kept"]);
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
        let planted = [
            ("core.fsmonitor", script("fsmonitor", "exit 1")),
            ("filter.mark.clean", script("clean", "cat")),
            ("diff.external", script("external", "exit 0")),
            ("diff.mark.textconv", script("textconv", "cat \"$1\"")),
        ];
        for (key, value) in &planted {
            repo.git(&["config", key, value]);
        }
        fs::write(
            repo.root().join(".git/info/attributes"),
            "* filter=mark diff=mark\n",
        )
        .unwrap();
        let f = WriteFixture::with_repo(repo, false, Askpass::default());
        Self { f, marks }
    }

    /// 사람의 전역 설정에 필터 드라이버를 둔다(git lfs 가 `~/.gitconfig` 에 쓰는 것처럼). 저장소의 속성(`* filter=mark`)이 그
    /// 드라이버를 고른다 — 미신뢰 저장소는 저장소 설정의 `filter.mark` 를 버리지만 전역 설정의 것은 남는다. 시험 프로세스의 gix 만
    /// 이 전역 설정을 읽는다(실행기의 git 은 가짜 로그인 셸의 빈 전역 설정으로 돈다).
    fn person_filter(&self) {
        let person = self.f.repo.base().join("person");
        fs::create_dir_all(&person).unwrap();
        let clean = person.join("global-clean");
        fs::write(
            &clean,
            format!(
                "#!/bin/sh\necho \"$*\" >> '{}'\ncat\n",
                self.marks.join("global-clean").display()
            ),
        )
        .unwrap();
        fs::set_permissions(&clean, fs::Permissions::from_mode(0o755)).unwrap();
        let config = person.join(".gitconfig");
        fs::write(
            &config,
            format!("[filter \"mark\"]\n\tclean = {}\n", clean.display()),
        )
        .unwrap();
        // SAFETY: nextest 는 시험마다 프로세스가 따로이고, gix 는 요청마다 저장소를 새로 열며 그때 환경을 읽는다.
        unsafe {
            std::env::set_var("HOME", &person);
            std::env::set_var("GIT_CONFIG_GLOBAL", &config);
        }
    }

    fn marks(&self) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(&self.marks)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_reads_of_an_untrusted_repository_run_none_of_its_programs() {
    let p = Planted::new();
    let f = &p.f;
    assert!(!f.projects.is_trusted(&f.project));
    p.person_filter();

    let stashes = f
        .write
        .stash_list(GitStashListParams {
            project_id: f.project.clone(),
            worktree: None,
        })
        .await
        .unwrap()
        .stashes;
    assert_eq!(stashes.len(), 1);
    assert_eq!(stashes[0].message, "On main: kept");

    // 줄 단위 쓰기는 잠금 안에서 diff 를 다시 만든다 — 그 사이 파일이 바뀌어 conflict 로 끝나도 읽기는 돌았다.
    f.repo.write("f.txt", "1\ntwo\n3\n");
    let hunk = diff(f, DiffSide::WorkTree, "f.txt").hunks[0].clone();
    f.repo.write("f.txt", "1\nTWO\n3\n");
    let request = |hunk| GitLinesParams {
        project_id: f.project.clone(),
        worktree: None,
        path: "f.txt".into(),
        old_path: None,
        hunks: vec![select(hunk, |l| l.kind != LineKind::Context)],
        ignore_whitespace: false,
    };
    let stale = f
        .write
        .stage_lines(f.caller(), request(&hunk))
        .await
        .unwrap_err();
    assert!(
        matches!(&stale, WriteError::Rpc(e) if e.code == rpc_code::CONFLICT),
        "{stale:?}"
    );
    assert_eq!(p.marks(), Vec::<String>::new());

    // 같은 설정이 살아 있다: 사람이 누른 쓰기는 저장소의 필터를 돌린다(D43).
    f.write
        .stage(
            f.caller(),
            GitPathsParams {
                project_id: f.project.clone(),
                worktree: None,
                paths: vec!["f.txt".into()],
            },
        )
        .await
        .unwrap();
    assert!(
        p.marks().contains(&"clean".to_owned()),
        "the planted clean filter is live: {:?}",
        p.marks()
    );
}
