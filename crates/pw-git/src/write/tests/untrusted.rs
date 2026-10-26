//! 미신뢰 프로젝트에서 이 행의 자동 경로는 저장소 설정의 프로그램을 돌리지 않는다(결정 M2-D8, 03-architecture.md 「보안」).
//! 압축으로 받은 저장소처럼 `.git/config` 에 `core.fsmonitor` · `filter.<x>.clean` · `diff.external` · `diff.<x>.textconv` 를 심고,
//! 진행 중 작업 읽기(`git.operation`) · 쓰기 전 확인(창이 취소) · `index.lock` 거절 · 질문 표가 표식을 남기지 않는지 본다. 그 설정이
//! 살아 있다는 것은 사람이 누른 쓰기(`git add`)가 표식을 남기는 것으로 보인다 — 쓰기는 터미널의 git 과 같이 돈다(D43).

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use super::super::*;
use crate::testing::TestRepo;
use crate::testing::write::{Window, WriteFixture};

struct Planted {
    f: WriteFixture,
    marks: PathBuf,
}

impl Planted {
    /// merge 충돌 중인 미신뢰 저장소에 프로그램을 심는다(충돌을 만든 뒤에 심는다 — 만드는 git 이 표식을 남기지 않게).
    fn new() -> Self {
        let repo = TestRepo::with_commit();
        repo.start_conflicting_merge();
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
async fn the_automatic_paths_of_an_untrusted_repository_run_none_of_its_programs() {
    let p = Planted::new();
    let f = &p.f;
    assert!(!f.projects.is_trusted(&f.project));
    // 파일을 바꿔 둔다 — status 가 clean 필터를, diff 가 외부 diff 를 부를 일이 생긴다.
    f.repo.write("README.md", "changed\n");

    let info = f.write.operation(&f.project, None).await.unwrap().unwrap();
    assert_eq!(info.kind, crate::methods::RepoOperation::Merge);

    let window = Window::open(&f.write, &f.project, |_| Some(GitPromptAnswer::Cancel));
    assert_eq!(
        f.write.act(&f.project, None, OperationAction::Abort).await,
        Err(WriteError::Cancelled),
        "the window cancelled the unsaved check — git did not run"
    );
    assert_eq!(window.seen().len(), 1);
    drop(window);

    let _window = Window::open(&f.write, &f.project, |_| Some(GitPromptAnswer::Proceed));
    let lock = f.repo.root().join(".git/index.lock");
    fs::write(&lock, "").unwrap();
    assert!(matches!(
        f.write.act(&f.project, None, OperationAction::Abort).await,
        Err(WriteError::IndexLocked { .. })
    ));
    fs::remove_file(&lock).unwrap();
    assert_eq!(p.marks(), Vec::<String>::new());

    // 같은 설정이 살아 있다: 사람이 누른 쓰기는 터미널의 git 과 같이 저장소의 필터를 돌린다(D43).
    f.write
        .run(f.request("test git write"), |ctx| async move {
            ctx.run(ctx.git().args(["add", "README.md"]))
                .await
                .map(drop)
        })
        .await
        .unwrap();
    assert!(
        p.marks().contains(&"clean".to_owned()),
        "the planted clean filter is live: {:?}",
        p.marks()
    );
}
