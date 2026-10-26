//! 미신뢰 프로젝트: reset hard 의 사전 조회(자동 경로 — 창이 확인 다이얼로그를 열 때 부른다)는 저장소 설정의 프로그램도, 어느 설정의
//! 필터 드라이버도 돌리지 않는다(결정 M2-D8). 사람이 누른 쓰기(merge · reset)는 터미널의 git 과 같이 돈다(D43).

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use super::super::methods::{GitMergeParams, GitResetPreviewParams, merge, reset_preview};
use super::super::{FastForward, HistoryOutcome};
use super::window;
use crate::testing::TestRepo;
use crate::testing::write::WriteFixture;
use crate::write::Askpass;

/// 압축으로 받은 저장소처럼 `.git/config` 에 프로그램을 심은 미신뢰 저장소(`feature` 가 `main` 보다 커밋 하나 앞선다). 프로그램은
/// 부르면 `marks/<이름>` 에 적는다.
fn planted() -> (WriteFixture, PathBuf) {
    let repo = TestRepo::with_commit();
    repo.git(&["branch", "feature"]);
    repo.checkout("feature");
    repo.commit_file("f.txt", "feature\n", "feature");
    repo.checkout("main");
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
        ("diff.external", script("external", "exit 0")),
        ("diff.mark.textconv", script("textconv", "cat \"$1\"")),
    ] {
        repo.git(&["config", key, &value]);
    }
    fs::write(
        repo.root().join(".git/info/attributes"),
        "* filter=mark diff=mark\n",
    )
    .unwrap();
    let f = WriteFixture::with_repo(repo, false, Askpass::default());
    (f, marks)
}

fn marks_in(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_reset_preview_of_an_untrusted_repository_runs_none_of_its_programs() {
    let (f, dir) = planted();
    assert!(!f.projects.is_trusted(&f.project));
    // 내용을 바꾸면(크기가 같아 시각 · 내용을 견줘야 한다) 작업 트리 판정이 clean 필터를 부를 일이 생긴다.
    f.repo.write("README.md", "HELLO\n");
    let preview = reset_preview(
        f.write.clone(),
        GitResetPreviewParams {
            project_id: f.project.clone(),
            worktree: None,
            commit: "HEAD".into(),
        },
    )
    .await
    .unwrap();
    assert_eq!(
        preview
            .lost
            .changes
            .iter()
            .map(|e| e.path.as_str())
            .collect::<Vec<_>>(),
        ["README.md"]
    );
    assert_eq!(marks_in(&dir), Vec::<String>::new());

    // 사람이 누른 쓰기는 미신뢰여도 돈다 — 잠금 안에서 다시 읽는 잃을 변경도 gix 로만 읽는다.
    let _window = window(&f);
    let first = f.repo.head();
    let merged = merge(
        f.write.clone(),
        GitMergeParams {
            project_id: f.project.clone(),
            worktree: None,
            rev: "feature".into(),
            fast_forward: FastForward::Allow,
        },
    )
    .await;
    // README.md 를 고친 채라도 feature 는 그 파일을 건드리지 않아 git 이 merge 한다.
    assert_eq!(merged.unwrap().outcome, HistoryOutcome::FastForward);
    let (preview, done) = super::reset::confirmed_hard(&f, &first).await;
    let done = done.unwrap();
    assert_eq!(done.lost.as_ref(), Some(&preview.lost));
    assert_eq!(f.repo.head(), first);
    assert_eq!(done.lost.unwrap().changes.len(), 1);
}

/// 사람의 전역 설정(`~/.gitconfig`)에 정의된 필터 드라이버도 미신뢰 저장소의 `.gitattributes` 가 고르면 돌리지 않는다 — 저장소
/// 설정을 버리고 여는 것만으로는 막히지 않는다(결정 M2-D8, 2026-10-26 M2-04 리뷰). 신뢰하면 git 처럼 clean 필터가 돈다.
#[test]
fn a_global_filter_driver_runs_in_the_reset_preview_only_when_trusted() {
    let repo = TestRepo::with_commit();
    let home = repo.base().join("process-home");
    fs::create_dir_all(&home).unwrap();
    let config = home.join(".gitconfig");
    let marks = repo.base().join("marks");
    fs::create_dir_all(&marks).unwrap();
    let clean = repo.base().join("clean");
    fs::write(
        &clean,
        format!(
            "#!/bin/sh\necho \"$*\" >> '{}'\ncat\n",
            marks.join("clean").display()
        ),
    )
    .unwrap();
    fs::set_permissions(&clean, fs::Permissions::from_mode(0o755)).unwrap();
    fs::write(
        &config,
        format!(
            "[filter \"lfs\"]\n\tclean = {}\n\trequired = true\n",
            clean.display()
        ),
    )
    .unwrap();
    // SAFETY: nextest 는 시험마다 프로세스가 따로이고, 런타임(다른 스레드)을 띄우기 전에 바꾼다. gix 가 이 프로세스의 전역 설정
    // 자리로 읽는다(시험 저장소의 git CLI 는 따로 빈 전역 설정을 쓴다).
    unsafe {
        std::env::set_var("HOME", &home);
        std::env::set_var("XDG_CONFIG_HOME", home.join(".config"));
        std::env::set_var("GIT_CONFIG_GLOBAL", &config);
        std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
    }
    repo.write(".gitattributes", "* filter=lfs\n");
    repo.commit_all("attributes");
    repo.write("README.md", "HELLO\n");
    // 크기가 같으니 시각을 미래로 옮겨 내용을 견주게 한다(그때 clean 필터를 고른다).
    fs::File::options()
        .write(true)
        .open(repo.root().join("README.md"))
        .unwrap()
        .set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(5))
        .unwrap();

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let f = WriteFixture::with_repo(repo, false, Askpass::default());
        let preview = || {
            reset_preview(
                f.write.clone(),
                GitResetPreviewParams {
                    project_id: f.project.clone(),
                    worktree: None,
                    commit: "HEAD".into(),
                },
            )
        };
        let lost = preview().await.unwrap().lost;
        assert_eq!(lost.changes.len(), 1, "{lost:?}");
        assert_eq!(marks_in(&marks), Vec::<String>::new());

        f.projects.set_trust(&f.project, true).unwrap();
        preview().await.unwrap();
        assert_eq!(marks_in(&marks), ["clean"]);
    });
}
