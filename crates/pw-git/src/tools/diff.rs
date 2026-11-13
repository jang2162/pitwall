//! `git_diff` — `git.diff` 의 함수([`crate::Git::diff`]) 위: 기본은 바뀐 파일 목록과 파일별 더한 · 뺀 줄 수, `detail` 이나 `path`
//! 를 주면 hunk 를 unified diff 글로 싣는다(예산 안에서 — 넘으면 `truncated`).

use std::fmt::Write as _;
use std::sync::Arc;

use pw_rpc::{ConnectionScope, RpcError, rpc_method};
use serde::{Deserialize, Serialize};

use super::{Budget, blocking, grant};
use crate::Git;
use crate::diff::methods::{DiffFile, DiffHunk, DiffSide, FileStatus, GitDiffParams, LineKind};

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitDiffToolParams {
    pub project_id: String,
    /// 볼 워크트리의 꼭대기(절대 경로). 비면 세션의 워크트리, 그것도 없으면 프로젝트의 메인 체크아웃.
    #[serde(default)]
    pub worktree: Option<String>,
    /// 옛 쪽. 짝은 넷이다:
    /// - `to` 가 커밋(`{"kind":"commit","rev":"…"}`): `from` 이 없으면 그 커밋의 첫 부모(`git show`), 있으면 그 커밋(두 커밋 비교).
    /// - `to` 가 index(`{"kind":"index"}`): `from` 은 커밋, 없으면 HEAD — `git diff --cached`.
    /// - `to` 가 작업 트리(`{"kind":"workTree"}`): `from` 은 index(없어도) — `git diff`.
    #[serde(default)]
    pub from: Option<DiffSide>,
    /// 새 쪽. 비면 작업 트리(`git diff` — 스테이징하지 않은 변경).
    #[serde(default)]
    pub to: Option<DiffSide>,
    /// 이 경로(작업 트리 기준, `/` 구분, 파일이나 폴더)와 그 아래만 보고 hunk 를 싣는다.
    #[serde(default)]
    pub path: Option<String>,
    /// 모든 파일의 hunk 를 싣는다.
    #[serde(default)]
    pub detail: bool,
    /// 공백을 모두 무시한다(`git diff -w`).
    #[serde(default)]
    pub ignore_whitespace: bool,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitDiffToolResult {
    /// 파일 경로의 기준인 작업 트리 꼭대기(절대 경로).
    pub work_tree: String,
    /// 옛 쪽 커밋의 해시. 빈 트리 · index 면 `null`.
    pub old_commit: Option<String>,
    /// 새 쪽 커밋의 해시. index · 작업 트리면 `null`.
    pub new_commit: Option<String>,
    /// 바뀐 파일(경로순).
    pub files: Vec<GitDiffToolFile>,
    /// 응답 크기 상한에 닿아 뒤쪽 파일의 `patch`(또는 파일 자체 — `omittedFiles`)를 싣지 못했다. `path` 로 좁혀 다시 부른다.
    pub truncated: bool,
    /// 상한에 닿아 목록에서 뺀 파일 수.
    pub omitted_files: u32,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitDiffToolFile {
    pub status: FileStatus,
    /// 새 경로(지워졌으면 옛 경로).
    pub path: String,
    /// 이름이 바뀌었으면 옛 경로.
    pub old_path: Option<String>,
    /// 더한 줄 · 뺀 줄 수(`git diff --numstat`).
    pub added: u32,
    pub removed: u32,
    /// 바이너리라 hunk 가 없다.
    pub binary: bool,
    /// 파일이 너무 커서 hunk 를 만들지 않았다.
    pub too_large: bool,
    /// 생성 파일이다(`linguist-generated` 등).
    pub generated: bool,
    /// hunk 의 unified diff(`@@ -a,b +c,d @@` 와 ` ` · `+` · `-` 줄). hunk 를 묻지 않았거나 없거나(바이너리 · 너무 큼 · 서브모듈)
    /// 응답 크기 상한에 닿았으면 `null`.
    pub patch: Option<String>,
    /// 이 파일의 내용을 읽지 못한 까닭.
    pub error: Option<String>,
}

rpc_method! {
    /// 두 쪽(커밋 · index · 작업 트리)의 diff. 기본은 바뀐 파일 목록(상태 · 경로 · 더한 · 뺀 줄 수), `detail` 이나 `path` 를
    /// 주면 hunk 를 unified diff 로 싣는다. 응답 크기 상한에 닿으면 `truncated`. 저장소가 없으면 invalid params.
    pub GitDiffTool: "git.tool.diff" {
        params: GitDiffToolParams,
        result: GitDiffToolResult,
        tier: Read,
        roles: [Owner],
        mcp: { group: "git", name: "git_diff" },
    }
}

pub(crate) async fn call(
    git: &Arc<Git>,
    scope: ConnectionScope,
    params: GitDiffToolParams,
) -> Result<GitDiffToolResult, RpcError> {
    grant(git, scope, &params.project_id).await?;
    let hunks = params.detail || params.path.is_some();
    let diff_params = GitDiffParams {
        project_id: params.project_id,
        worktree: params.worktree,
        from: params.from,
        to: params.to.unwrap_or(DiffSide::WorkTree),
        paths: params.path.map(|p| vec![p]),
        ignore_whitespace: params.ignore_whitespace,
    };
    let git = Arc::clone(git);
    let diff = blocking(move || git.diff(&diff_params)).await?;
    let mut budget = Budget::new();
    let mut truncated = false;
    let mut files = Vec::with_capacity(diff.files.len());
    let mut omitted_files = 0u32;
    // 목록 한 줄도 들지 않으면 그 뒤는 모두 뺀다 — 작은 파일만 골라 실어 빈자리를 숨기지 않게.
    let mut full = false;
    for file in diff.files {
        if full {
            omitted_files += 1;
            continue;
        }
        let mut entry = summary(&file);
        if hunks && !truncated {
            entry.patch = patch(&file);
            if entry.patch.is_some() {
                if budget.take(&entry) {
                    files.push(entry);
                    continue;
                }
                // hunk 가 예산을 넘는다 — 이 파일부터는 목록만.
                truncated = true;
                entry.patch = None;
            }
        }
        if budget.take(&entry) {
            files.push(entry);
        } else {
            truncated = true;
            full = true;
            omitted_files += 1;
        }
    }
    Ok(GitDiffToolResult {
        work_tree: diff.work_tree,
        old_commit: diff.old_commit,
        new_commit: diff.new_commit,
        files,
        truncated,
        omitted_files,
    })
}

fn summary(file: &DiffFile) -> GitDiffToolFile {
    GitDiffToolFile {
        status: file.status,
        path: file.path.clone(),
        old_path: file.old_path.clone(),
        added: file.added,
        removed: file.removed,
        binary: file.binary,
        too_large: file.too_large,
        generated: file.generated,
        patch: None,
        error: file.error.clone(),
    }
}

/// hunk 를 unified diff 글로. hunk 가 없으면 `None`.
fn patch(file: &DiffFile) -> Option<String> {
    if file.hunks.is_empty() {
        return None;
    }
    let mut out = String::new();
    for hunk in &file.hunks {
        hunk_header(&mut out, hunk);
        for line in &hunk.lines {
            out.push(match line.kind {
                LineKind::Context => ' ',
                LineKind::Added => '+',
                LineKind::Removed => '-',
            });
            out.push_str(&line.text);
            out.push('\n');
            if line.no_newline {
                out.push_str("\\ No newline at end of file\n");
            }
        }
    }
    Some(out)
}

/// git 과 같이 줄 수가 1 이면 생략한다(`@@ -3 +3,2 @@`).
fn hunk_header(out: &mut String, hunk: &DiffHunk) {
    let side = |start: u32, lines: u32| {
        if lines == 1 {
            start.to_string()
        } else {
            format!("{start},{lines}")
        }
    };
    let _ = write!(
        out,
        "@@ -{} +{} @@",
        side(hunk.old_start, hunk.old_lines),
        side(hunk.new_start, hunk.new_lines)
    );
    if !hunk.header.is_empty() {
        out.push(' ');
        out.push_str(&hunk.header);
    }
    out.push('\n');
}
