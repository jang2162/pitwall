//! 스테이징 · 되돌리기 · stash RPC(`docs/design/13-m2-plan.md` M2-07). UI 전용이라 MCP 툴이 아니다(결정 M2-D7 — git 쓰기는 M5
//! 에서도 툴이 아니다). 쓰기는 모두 owner 만, 그 프로젝트 창의 연결로만 부른다(실행기가 본다 — `crate::write` 머리 주석 0). 경로는 저장소 작업 트리 기준 상대 경로, 구분자 `/` 다(`git.diff` 와 같다).

use std::sync::Arc;

use pw_rpc::{Router, rpc_method};
use serde::{Deserialize, Serialize};

use crate::diff::methods::DiffHunk;
use crate::write::GitWrite;

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitPathsParams {
    pub project_id: String,
    /// 활성 워크트리의 꼭대기(`git.repo` 와 같다).
    #[serde(default)]
    pub worktree: Option<String>,
    /// 파일 · 폴더(그 아래 전부). 글롭 · pathspec 문법은 쓰지 않는다(글자 그대로). 비면 invalid params.
    pub paths: Vec<String>,
}

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitDiscardParams {
    pub project_id: String,
    #[serde(default)]
    pub worktree: Option<String>,
    pub paths: Vec<String>,
    /// 사람이 확인했다(결정 M2-D6 ① — 작업 트리 변경 버리기는 되돌릴 기록이 없다). 거짓이면 아무것도 하지 않는다(invalid params).
    pub confirmed: bool,
}

/// `git.diff` 의 hunk 하나와 그 안에서 고른 줄.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct HunkSelection {
    /// `git.diff` 가 준 hunk 그대로. Host 가 그 파일의 diff 를 다시 만들어 머리(범위 · 함수 줄)와 줄이 모두 같은 hunk 를 찾는다 —
    /// 없으면 그 사이 파일이 바뀐 것이라 아무것도 쓰지 않는다(conflict).
    pub hunk: DiffHunk,
    /// 고른 줄(`hunk.lines` 의 인덱스, 더한 줄 · 뺀 줄만). `null` 이면 hunk 의 모든 변경. 빈 목록 · 문맥 줄 · 범위 밖은 invalid
    /// params.
    #[serde(default)]
    pub lines: Option<Vec<u32>>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitLinesParams {
    pub project_id: String,
    #[serde(default)]
    pub worktree: Option<String>,
    /// 파일 하나(`git.diff` 의 `path`).
    pub path: String,
    /// 이름이 바뀐 파일의 옛 경로(`git.diff` 의 `oldPath`) — `git.unstageLines` 만 받는다(HEAD 쪽이 그 경로다).
    #[serde(default)]
    pub old_path: Option<String>,
    /// 고른 hunk. 비면 invalid params.
    pub hunks: Vec<HunkSelection>,
    /// hunk 를 공백 무시(`ignoreWhitespace`)로 받았다. 그 hunk 는 실제 바이트와 줄이 맞지 않으므로 줄 단위로 쓰지 않는다(invalid
    /// params) — 공백을 보는 diff 에서 고른다.
    #[serde(default)]
    pub ignore_whitespace: bool,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitDiscardLinesParams {
    pub project_id: String,
    #[serde(default)]
    pub worktree: Option<String>,
    pub path: String,
    pub hunks: Vec<HunkSelection>,
    #[serde(default)]
    pub ignore_whitespace: bool,
    /// [`GitDiscardParams::confirmed`] 와 같다.
    pub confirmed: bool,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitWorkTree {
    /// 저장소의 작업 트리 꼭대기(절대 경로, `git.repo` 의 `workTree` 와 같은 꼴).
    pub work_tree: String,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitDiscardResult {
    pub work_tree: String,
    /// 추적하지 않는 경로라 버리지 않은 것(요청 순서). 버리기는 작업 트리를 index 로 돌리는 것이라 index 에 없는 파일은 대상이
    /// 아니다 — 지우지 않는다. intent-to-add(`git add -N`) 파일도 여기에 든다(비우지 않는다 — 넘긴 폴더 아래의 것이면 그 파일
    /// 경로로). 나머지 경로는 버렸다.
    pub untracked: Vec<String>,
}

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitStashListParams {
    pub project_id: String,
    #[serde(default)]
    pub worktree: Option<String>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct StashEntry {
    /// `stash@{index}` 의 수. 0 이 가장 최근이다.
    pub index: u32,
    /// stash 커밋 해시.
    pub commit: String,
    /// reflog 의 메시지(`On main: 메시지` · `WIP on main: <해시> <제목>` — git 이 만든 그대로).
    pub message: String,
    /// 만든 시각(유닉스 초).
    #[specta(type = pw_rpc::JsNumber)]
    pub time: i64,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitStashList {
    pub work_tree: String,
    /// 최근 것부터(`git stash list` 의 순서).
    pub stashes: Vec<StashEntry>,
}

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitStashPushParams {
    pub project_id: String,
    #[serde(default)]
    pub worktree: Option<String>,
    /// 비었거나 없으면 git 의 기본 메시지(`WIP on <브랜치>: …`).
    #[serde(default)]
    pub message: Option<String>,
    /// untracked 파일도 담는다(`--include-untracked`). 무시된 파일은 담지 않는다.
    #[serde(default)]
    pub include_untracked: bool,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitStashPushResult {
    pub work_tree: String,
    /// 만든 항목(`stash@{0}`). 담을 변경이 없었으면 `null`(git 은 성공으로 끝난다).
    pub stash: Option<StashEntry>,
}

/// stash 항목 하나를 고르는 요청. `index` 와 `commit` 이 지금 목록과 맞지 않으면(그 사이 목록이 바뀜) 아무것도 하지 않는다(conflict).
#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitStashApplyParams {
    pub project_id: String,
    #[serde(default)]
    pub worktree: Option<String>,
    pub index: u32,
    pub commit: String,
    /// index 의 상태까지 되살린다(`--index`). index 쪽이 충돌하면 git 이 원문으로 거절한다.
    #[serde(default)]
    pub reinstate_index: bool,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitStashApplyResult {
    pub work_tree: String,
    /// 충돌로 끝났다 — 실패가 아니라 충돌 파일을 풀 차례다(충돌 목록은 `git.status`). pop 이면 git 이 항목을 지우지 않고 남겼다.
    pub conflicted: bool,
}

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitStashDropParams {
    pub project_id: String,
    #[serde(default)]
    pub worktree: Option<String>,
    pub index: u32,
    pub commit: String,
    /// 사람이 확인했다(결정 M2-D6 ① 의 stash 버리기). 거짓이면 아무것도 하지 않는다(invalid params).
    pub confirmed: bool,
}

rpc_method! {
    /// 경로를 스테이징한다(`git add --all`): 바뀐 파일 · untracked(무시된 파일은 git 이 거절) · 지운 파일(지움을 담는다). 충돌
    /// 중인 경로(폴더면 그 아래)가 있으면 아무것도 하지 않는다(invalid params, `data.conflicted` — 해결은 `git.conflict.resolve`).
    pub GitStageMethod: "git.stage" {
        params: GitPathsParams,
        result: GitWorkTree,
        tier: Write,
        roles: [Owner],
    }
}

rpc_method! {
    /// 경로의 스테이징을 푼다(`git reset`): index 를 HEAD 의 것으로(HEAD 에 없으면 index 에서 뺀다 — 커밋이 없는 저장소도 된다).
    /// 충돌 중인 경로는 `git.stage` 처럼 거절한다(HEAD 쪽을 담아 충돌을 조용히 풀지 않는다). 이름 바뀜은 옛 경로와 새 경로를 둘
    /// 다 넘긴다.
    pub GitUnstageMethod: "git.unstage" {
        params: GitPathsParams,
        result: GitWorkTree,
        tier: Write,
        roles: [Owner],
    }
}

rpc_method! {
    /// 작업 트리의 변경을 버린다(`git restore --worktree` — index 의 것으로). `confirmed: true` 가 있어야 한다(결정 M2-D6 ①).
    /// untracked · intent-to-add 경로는 지우지도 비우지도 않고 `untracked` 로 돌려준다. 작업 트리가 바뀌므로 미저장 문서를 먼저 확인한다(M2-06).
    pub GitDiscardMethod: "git.discard" {
        params: GitDiscardParams,
        result: GitDiscardResult,
        tier: Write,
        roles: [Owner],
    }
}

rpc_method! {
    /// 작업 트리 diff(index ↔ 작업 트리)의 hunk · 줄을 골라 스테이징한다(고른 줄로 만든 패치를 `git apply --cached`). 바이너리 ·
    /// 심링크 · 충돌 중 · index 에 없는 파일은 줄 단위로 할 수 없다(invalid params — 파일 단위로). 고른 hunk 가 지금 diff 에 없으면
    /// conflict.
    pub GitStageLinesMethod: "git.stageLines" {
        params: GitLinesParams,
        result: GitWorkTree,
        tier: Write,
        roles: [Owner],
    }
}

rpc_method! {
    /// 스테이징된 diff(HEAD ↔ index)의 hunk · 줄을 골라 스테이징을 푼다(`git apply --cached -R`). 이름이 바뀐 파일은 `oldPath` 를
    /// 함께 넘긴다.
    pub GitUnstageLinesMethod: "git.unstageLines" {
        params: GitLinesParams,
        result: GitWorkTree,
        tier: Write,
        roles: [Owner],
    }
}

rpc_method! {
    /// 작업 트리 diff 의 hunk · 줄을 골라 버린다(작업 트리에 `git apply -R`). `confirmed: true` 가 있어야 한다(결정 M2-D6 ①).
    /// 작업 트리에서 지운 파일은 파일 단위(`git.discard`)로 되살린다.
    pub GitDiscardLinesMethod: "git.discardLines" {
        params: GitDiscardLinesParams,
        result: GitWorkTree,
        tier: Write,
        roles: [Owner],
    }
}

rpc_method! {
    /// stash 목록(`refs/stash` 의 reflog, 최근 것부터). 저장소가 없으면 invalid params.
    pub GitStashListMethod: "git.stash.list" {
        params: GitStashListParams,
        result: GitStashList,
        tier: Read,
        roles: [Owner],
    }
}

rpc_method! {
    /// stash 를 만든다(`git stash push`) — 메시지 · untracked 포함을 고른다. 작업 트리 전체가 바뀌므로 미저장 문서를 먼저 확인한다.
    pub GitStashPushMethod: "git.stash.push" {
        params: GitStashPushParams,
        result: GitStashPushResult,
        tier: Write,
        roles: [Owner],
    }
}

rpc_method! {
    /// stash 를 적용한다(`git stash apply`, 항목은 남는다). 충돌로 끝나면 `conflicted`.
    pub GitStashApplyMethod: "git.stash.apply" {
        params: GitStashApplyParams,
        result: GitStashApplyResult,
        tier: Write,
        roles: [Owner],
    }
}

rpc_method! {
    /// stash 를 적용하고 지운다(`git stash apply <커밋>` 뒤 `git stash drop` — 지운 것이 그 커밋인지 본다). 충돌로 끝나면
    /// `conflicted` 이고 항목은 남는다. 이미 충돌 중이라 git 이 적용을 거절하면 git 의 원문으로 실패한다(apply 도). 버리는 사이
    /// 목록이 밀렸으면 적용은 된 채 항목을 남기고 conflict.
    pub GitStashPopMethod: "git.stash.pop" {
        params: GitStashApplyParams,
        result: GitStashApplyResult,
        tier: Write,
        roles: [Owner],
    }
}

rpc_method! {
    /// stash 항목을 버린다(`git stash drop`). `confirmed: true` 가 있어야 한다(결정 M2-D6 ①). git 이 지운 것이 고른 커밋이
    /// 아니면(확인과 실행 사이에 목록이 밀림) 그 항목을 `git stash store` 로 되돌려 넣고 conflict.
    pub GitStashDropMethod: "git.stash.drop" {
        params: GitStashDropParams,
        result: GitWorkTree,
        tier: Write,
        roles: [Owner],
    }
}

/// `$method` 를 [`GitWrite`] 의 `$call` 로 잇는다. 부른 연결의 범위를 넘긴다 — 실행기가 그 프로젝트 창의 연결인지 본다.
macro_rules! write_method {
    ($router:ident, $write:ident, $method:ident, $call:ident) => {{
        let w = Arc::clone(&$write);
        $router.unary($method, move |ctx, params| {
            let (w, caller) = (Arc::clone(&w), ctx.scope().clone());
            async move { Ok(w.$call(caller, params).await?) }
        });
    }};
}

/// 핸들러를 붙인다.
pub fn register(router: &mut Router, write: Arc<GitWrite>) {
    write_method!(router, write, GitStageMethod, stage);
    write_method!(router, write, GitUnstageMethod, unstage);
    write_method!(router, write, GitDiscardMethod, discard);
    write_method!(router, write, GitStageLinesMethod, stage_lines);
    write_method!(router, write, GitUnstageLinesMethod, unstage_lines);
    write_method!(router, write, GitDiscardLinesMethod, discard_lines);
    write_method!(router, write, GitStashPushMethod, stash_push);
    write_method!(router, write, GitStashApplyMethod, stash_apply);
    write_method!(router, write, GitStashPopMethod, stash_pop);
    write_method!(router, write, GitStashDropMethod, stash_drop);
    let w = Arc::clone(&write);
    router.unary(GitStashListMethod, move |_ctx, params| {
        let w = Arc::clone(&w);
        async move { Ok(w.stash_list(params).await?) }
    });
}
