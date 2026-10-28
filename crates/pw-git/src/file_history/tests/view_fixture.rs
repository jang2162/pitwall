//! M2-18 의 창 시험 픽스처: 이름이 두 번 바뀐 파일(`TestRepo::blame_fixture` — `a.txt` → `b.txt` → `dir/c.txt`)의
//! `git.fileHistory` 와, 행마다 창이 그 커밋에서의 경로로 부르는 `git.diff`(`to` = 그 커밋, `from` 없음 = 첫 부모, `paths` = 그때 경로
//! · 이름 바뀜의 원래 경로)와 그 파일 내용(`git.blob`), 같은 커밋 · 경로의 `git show` patch, 그리고 `git log --follow --name-status`
//! 를 JSON 으로 남긴다(`apps/desktop/src/git/history/fixtures/history.json`). 창 시험
//! (`apps/desktop/src/git/history/history-window.test.tsx`)이 이 JSON 을 가짜 Host 의 답으로 쓰고 목록을 `follow` 와, 칠한 줄을
//! patch 와 견준다 — 이 시험은 그 JSON 이 지금 pw-git · git 의 출력과 같은지 본다(다르면 실패). 다시 만들 때는
//! `PW_UPDATE_HISTORY_FIXTURE=1` 로 이 시험을 돌린다.
//!
//! 커밋 시각은 `TestRepo` 의 시계라 해시가 늘 같다. 작업 트리 경로는 `/repo` 로 바꿔 적는다.

use std::collections::BTreeMap;
use std::path::PathBuf;

use super::*;
use crate::diff::methods::{DiffSide, FileStatus, GitBlobParams, GitDiffParams};

const FIXTURE: &str = "../../apps/desktop/src/git/history/fixtures/history.json";

fn fixture_value() -> serde_json::Value {
    let repo = TestRepo::new();
    let fx = repo.blame_fixture();
    let host = Fx::new("");
    let id = host.project(repo.root());
    let rows = assert_matches(repo.root(), repo.base(), "HEAD", fx.path);
    let follow = git_follow(repo.root(), repo.base(), "HEAD", fx.path);

    let mut diffs = BTreeMap::new();
    let mut blobs = BTreeMap::new();
    let mut patches = BTreeMap::new();
    for row in &rows {
        let paths: Vec<String> = std::iter::once(row.path.clone())
            .chain(row.old_path.clone())
            .collect();
        let mut diff = host
            .git
            .diff(&GitDiffParams {
                project_id: id.clone(),
                worktree: None,
                from: None,
                to: DiffSide::Commit {
                    rev: row.hash.clone(),
                },
                paths: Some(paths.clone()),
                ignore_whitespace: false,
            })
            .unwrap();
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
                let blob = host
                    .git
                    .blob(&GitBlobParams {
                        project_id: id.clone(),
                        rev: Some(rev.clone()),
                        path: path.clone(),
                        ..GitBlobParams::default()
                    })
                    .unwrap();
                blobs.insert(format!("{rev}:{path}"), serde_json::to_value(blob).unwrap());
            }
        }
        diff.work_tree = "/repo".to_owned();
        let mut show = vec![
            "show",
            "--format=",
            "--find-renames",
            "-U3",
            "--no-color",
            "--no-ext-diff",
            &row.hash,
            "--",
        ];
        show.extend(paths.iter().map(String::as_str));
        patches.insert(row.hash.clone(), repo.git(&show));
        diffs.insert(row.hash.clone(), diff);
    }
    serde_json::json!({
        "path": fx.path,
        "rows": rows,
        "follow": follow,
        "diffs": diffs,
        "blobs": blobs,
        "patches": patches,
    })
}

#[test]
fn history_view_fixture_matches_pw_git_and_git() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(FIXTURE);
    let got = fixture_value();
    if std::env::var_os("PW_UPDATE_HISTORY_FIXTURE").is_some() {
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
        "{} 이 지금 pw-git · git 의 출력과 다르다 — PW_UPDATE_HISTORY_FIXTURE=1 로 이 시험을 돌려 다시 만든다",
        path.display()
    );
}
