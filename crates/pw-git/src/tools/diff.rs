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
    /// Top of the worktree to read (absolute path). Empty means the session's worktree, or else the project's main
    /// checkout.
    #[serde(default)]
    pub worktree: Option<String>,
    /// The old side. There are four pairings:
    /// - `to` is a commit (`{"kind":"commit","rev":"…"}`): without `from`, the commit's first parent (`git show`); with
    ///   it, that commit (comparing two commits).
    /// - `to` is the index (`{"kind":"index"}`): `from` is a commit, HEAD if missing — `git diff --cached`.
    /// - `to` is the working tree (`{"kind":"workTree"}`): `from` is the index (or missing) — `git diff`.
    #[serde(default)]
    pub from: Option<DiffSide>,
    /// The new side. Empty means the working tree (`git diff` — unstaged changes).
    #[serde(default)]
    pub to: Option<DiffSide>,
    /// Only this path (relative to the working tree, `/`-separated, a file or a folder) and what is under it, with
    /// hunks included.
    #[serde(default)]
    pub path: Option<String>,
    /// Include the hunks of every file.
    #[serde(default)]
    pub detail: bool,
    /// Ignore all whitespace (`git diff -w`).
    #[serde(default)]
    pub ignore_whitespace: bool,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitDiffToolResult {
    /// Top of the working tree that file paths are relative to (absolute path).
    pub work_tree: String,
    /// Hash of the old side's commit. `null` for the empty tree · the index.
    pub old_commit: Option<String>,
    /// Hash of the new side's commit. `null` for the index · the working tree.
    pub new_commit: Option<String>,
    /// Changed files (sorted by path).
    pub files: Vec<GitDiffToolFile>,
    /// The response size limit was reached, so the `patch` of later files (or the files themselves — `omittedFiles`) is
    /// not included. Narrow with `path` and call again.
    pub truncated: bool,
    /// Number of files left out of the list because the limit was reached.
    pub omitted_files: u32,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitDiffToolFile {
    pub status: FileStatus,
    /// New path (the old path if deleted).
    pub path: String,
    /// Old path, if renamed.
    pub old_path: Option<String>,
    /// Number of lines added · removed (`git diff --numstat`).
    pub added: u32,
    pub removed: u32,
    /// Binary, so no hunks.
    pub binary: bool,
    /// The file is too large, so no hunks were made.
    pub too_large: bool,
    /// A generated file (`linguist-generated`, etc.).
    pub generated: bool,
    /// Unified diff of the hunks (`@@ -a,b +c,d @@` and ` ` · `+` · `-` lines). `null` if hunks were not asked for,
    /// there are none (binary · too large · submodule), or the response size limit was reached.
    pub patch: Option<String>,
    /// Why this file's content could not be read.
    pub error: Option<String>,
}

rpc_method! {
    /// Diff of two sides (commit · index · working tree). By default the list of changed files (status · path · lines
    /// added · removed); with `detail` or `path`, the hunks as a unified diff. `truncated` when the response size limit
    /// is reached. invalid params if there is no repository.
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
