//! 이력을 바꾸는 쓰기(13-m2-plan.md M2-10 — F5-12, 결정 M2-D5 · M2-D6): merge · cherry-pick · revert · rebase(비대화형) · reset.
//! 모두 쓰기 실행기([`crate::write::GitWrite::run`])로 git CLI 를 돌린다 — 잠금 · `index.lock` 거절 · 미저장 확인 · Task 의 stderr
//! 원문은 실행기의 것이다. 미신뢰 프로젝트에서도 돈다(사람이 누른 쓰기, 결정 M2-D8).
//!
//! - 충돌로 멈추면 실패가 아니라 `stopped` 다 — 결과의 `operation` 이 M2-06 의 진행 중 작업이고, 계속 · 중단 · 건너뛰기는
//!   `git.operation.*`, 충돌 해결은 M2-11 · M2-19 다. 시작 전부터 진행 중 작업이 있었으면 git 이 거절한 원문 그대로 실패다.
//! - ref 를 움직였으면 결과의 `moved`([`GitRefMove`])에 그 ref 와 이전 · 지금 해시를 싣는다(결정 M2-D6 ③ — 창의 [되돌리기]).
//!   HEAD 의 ref 를 되돌리는 것은 `git.reset { commit: previous, mode, expected: current, expectedRef: refName }` 다 — mode 는
//!   soft reset 을 되돌리면 soft, mixed 면 mixed, 그 밖(hard · merge · rebase · cherry-pick · revert)은 keep(그사이 편집이 덮이면
//!   git 이 거절한다). 충돌로 멈췄다가 `git.operation.continue` · `skip` 으로 끝나면 그 결과의 `moved` 가 시작점([`start`])부터다.
//! - reset hard · keep 이 잃을 변경은 `git.resetPreview` 로 미리 읽는다(확인 다이얼로그는 M2-20). 읽기라 gix 로만 한다([`lost`]).
//!   hard 는 늘, keep(되돌리기)은 잃을 것(ignore 된 파일 · 디스크와 다른 스테이징 내용 등 git 이 묻지 않고 덮는 것)이 있을 때
//!   미리보기의 지문을 받아 잠금 안에서 다시 읽은 것과 다르면 아무것도 하지 않는다. 잃을 것이 없는 keep 은 지문 없이 한다.
//! - 리비전 입력은 git 의 인자로 그대로 넘긴다(브랜치 이름이면 merge 메시지가 그 이름을 쓴다). `-` 로 시작하면 옵션으로 읽히므로
//!   invalid params 로 거절한다.
//! - git 이 스스로 거절하는 것(로컬 변경이 덮어써지는 merge · rebase 등)은 덮지 않고 원문을 보인다(결정 M2-D6 ④).
//!
//! RPC 는 [`methods`].

pub mod lost;
pub mod methods;
#[cfg(test)]
mod tests;

use std::ffi::OsString;

use pw_rpc::{RpcError, code as rpc_code};
use serde::{Deserialize, Serialize};

pub use methods::register;

use crate::Opened;
use crate::write::{
    Changes, GitWrite, OperationInfo, WriteCtx, WriteError, WriteRequest, operation,
};

/// ref 하나를 움직인 기록(결정 M2-D6 ③). 창은 이것으로 알림에 이전 해시와 [되돌리기] 를 단다. reset · rebase · merge ·
/// cherry-pick · revert(이 모듈), 브랜치 만들기 · 지우기(M2-09), amend(M2-08)가 같은 꼴로 싣는다. `null` 은 「그때 없었다」 다.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitRefMove {
    /// 움직인 ref 의 전체 이름(`refs/heads/main`). 분리된 HEAD 를 움직였으면 `HEAD`.
    pub ref_name: String,
    /// 움직이기 전 커밋(전체 해시) — [되돌리기] 가 돌아갈 곳. ref 를 새로 만들었으면(브랜치 만들기, 태어나지 않은 브랜치에 처음
    /// 커밋을 둔 reset · merge) `null`.
    pub previous: Option<String>,
    /// 움직인 뒤 커밋. ref 를 지웠으면(브랜치 삭제) `null`.
    pub current: Option<String>,
}

/// 쓰기가 어떻게 끝났는가.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum HistoryOutcome {
    /// 바뀐 것이 없다(이미 들어 있는 브랜치의 merge, 이미 위에 있는 rebase).
    UpToDate,
    /// merge 가 fast-forward 로 끝났다(새 커밋 없이 ref 만 옮겼다).
    FastForward,
    /// 새 커밋을 만들었다(merge 커밋 · cherry-pick · revert · rebase 로 다시 쓴 커밋).
    Done,
    /// 충돌로 멈췄다 — `operation` 이 진행 중 작업이다.
    Stopped,
}

/// merge · cherry-pick · revert · rebase 의 결과.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitHistoryResult {
    /// 저장소 작업 트리 꼭대기(절대 경로).
    pub work_tree: String,
    pub outcome: HistoryOutcome,
    /// 끝난 뒤 HEAD 의 커밋(전체 해시). 태어나지 않은 브랜치면 `null`.
    pub head: Option<String>,
    /// 움직인 ref. 멈췄거나(`stopped` — 되돌리기는 중단이다) 바뀐 것이 없으면 `null`.
    pub moved: Option<GitRefMove>,
    /// `stopped` 일 때 진행 중 작업(M2-06 `git.operation` 과 같다).
    pub operation: Option<OperationInfo>,
}

/// 리비전 입력 검사: 비었거나 `-` 로 시작하면(옵션으로 읽힌다) invalid params.
pub(crate) fn check_rev(what: &str, rev: &str) -> Result<(), WriteError> {
    if rev.is_empty() || rev.starts_with('-') {
        return Err(WriteError::Rpc(RpcError::invalid_params(format!(
            "`{what}` must be a revision, not empty or starting with `-`: `{rev}`"
        ))));
    }
    Ok(())
}

/// 쓰기 전후로 보는 저장소 상태.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Snapshot {
    /// 지켜보는 ref 의 커밋(없으면 `None`).
    pub commit: Option<String>,
    pub head: Option<String>,
    /// HEAD 커밋의 부모(첫째가 첫 부모).
    pub head_parents: Vec<String>,
    pub operation: Option<OperationInfo>,
}

/// 지켜볼 ref: HEAD 가 가리키는 브랜치(분리돼 있으면 `HEAD`) 또는 이름을 준 ref.
#[derive(Debug, Clone)]
pub(crate) enum Watched {
    Head,
    Ref(String),
}

/// HEAD 가 가리키는 ref 의 전체 이름. 분리된 HEAD 면 `HEAD`.
pub(crate) fn head_ref_name(repo: &gix::Repository) -> Result<String, String> {
    Ok(repo
        .head_name()
        .map_err(|e| e.to_string())?
        .map_or_else(|| "HEAD".to_owned(), |name| name.as_bstr().to_string()))
}

/// ref 의 커밋(전체 해시). 없거나 태어나지 않았으면 `None`.
pub(crate) fn ref_commit(repo: &gix::Repository, name: &str) -> Option<String> {
    if name == "HEAD" {
        return repo.head_id().ok().map(|id| id.to_string());
    }
    let mut reference = repo.try_find_reference(name).ok()??;
    reference.peel_to_id().ok().map(|id| id.to_string())
}

pub(crate) fn snapshot(opened: &Opened, watched: &str) -> Snapshot {
    let repo = &opened.repo;
    let head_id = repo.head_id().ok();
    let head_parents = head_id
        .and_then(|id| id.object().ok()?.try_into_commit().ok())
        .map(|commit| commit.parent_ids().map(|p| p.to_string()).collect())
        .unwrap_or_default();
    Snapshot {
        commit: ref_commit(repo, watched),
        head: head_id.map(|id| id.to_string()),
        head_parents,
        operation: operation::read(repo),
    }
}

/// [`run`] 이 할 쓰기 하나.
pub(crate) struct Job {
    pub request: WriteRequest,
    pub watched: Watched,
    /// git 인자(하위 명령부터).
    pub args: Vec<OsString>,
    /// 성공했을 때 결과의 종류를 가른다(전, 후).
    pub classify: fn(&Snapshot, &Snapshot) -> HistoryOutcome,
}

/// 쓰기 하나를 돌려 결과를 만든다: 잠금 안에서 전 상태를 읽고 git 을 돌린 뒤 후 상태를 읽는다.
pub(crate) async fn run(write: &GitWrite, job: Job) -> Result<GitHistoryResult, WriteError> {
    let Job {
        request,
        watched,
        args,
        classify,
    } = job;
    write
        .run(request, move |ctx| async move {
            let name = watched_name(&ctx, &watched).await?;
            let before = read_snapshot(&ctx, &name).await?;
            let output = ctx.output(ctx.git().args(&args)).await?;
            let after = read_snapshot(&ctx, &name).await?;
            let work_tree = display(ctx.work_tree());
            if !output.status.success() {
                // 시작 전에는 없던 진행 중 작업이 생겼으면 충돌로 멈춘 것이다.
                if before.operation.is_none()
                    && let Some(operation) = after.operation
                {
                    return Ok(GitHistoryResult {
                        work_tree,
                        outcome: HistoryOutcome::Stopped,
                        head: after.head,
                        moved: None,
                        operation: Some(operation),
                    });
                }
                let command = args[0].to_string_lossy().into_owned();
                return Err(WriteError::failed(&command, &output));
            }
            Ok(GitHistoryResult {
                work_tree,
                outcome: classify(&before, &after),
                moved: moved(&name, &before, &after),
                head: after.head,
                operation: after.operation,
            })
        })
        .await
}

/// 지켜볼 ref 의 전체 이름(잠금 안에서 읽는다).
async fn watched_name(ctx: &WriteCtx, watched: &Watched) -> Result<String, WriteError> {
    match watched {
        Watched::Ref(name) => Ok(name.clone()),
        Watched::Head => ctx
            .read(|o| head_ref_name(&o.repo))
            .await?
            .map_err(WriteError::Internal),
    }
}

async fn read_snapshot(ctx: &WriteCtx, name: &str) -> Result<Snapshot, WriteError> {
    let name = name.to_owned();
    ctx.read(move |o| snapshot(o, &name)).await
}

/// 지켜본 ref 가 움직였으면 그 기록.
pub(crate) fn moved(name: &str, before: &Snapshot, after: &Snapshot) -> Option<GitRefMove> {
    (after.commit != before.commit).then(|| GitRefMove {
        ref_name: name.to_owned(),
        previous: before.commit.clone(),
        current: after.commit.clone(),
    })
}

/// 충돌로 멈춘 작업이 시작한 곳: 움직이는 ref 의 전체 이름과 시작 전 커밋. 계속 · 건너뛰기로 작업이 끝나면 이 ref 의 기록을
/// 만든다(`git.operation.*` 결과의 `moved`). 진행 중 작업이 없거나 am · bisect 면 `None`.
///
/// - rebase: `rebase-merge/` · `rebase-apply/` 의 `orig-head` 와 `head-name`(분리된 HEAD 면 `HEAD`).
/// - 여러 커밋의 cherry-pick · revert: `sequencer/head`(첫 커밋을 적용하기 전 HEAD). cherry-pick 은 `ORIG_HEAD` 를 남기지 않는다.
/// - merge · 커밋 하나의 cherry-pick · revert: 아직 커밋하지 않았으니 지금 HEAD.
pub(crate) fn start(repo: &gix::Repository) -> Option<(String, Option<String>)> {
    use crate::methods::RepoOperation;
    let info = operation::read(repo)?;
    let dir = repo.git_dir();
    match info.kind {
        RepoOperation::Rebase => {
            let state = ["rebase-merge", "rebase-apply"]
                .into_iter()
                .map(|d| dir.join(d))
                .find(|d| d.is_dir())?;
            let name = operation::read_text(&state.join("head-name"))?;
            let name = name.trim();
            let name = if name.starts_with("refs/") {
                name.to_owned()
            } else {
                "HEAD".to_owned()
            };
            let previous = operation::hashes(&state.join("orig-head"))
                .into_iter()
                .next()?;
            Some((name, Some(previous)))
        }
        RepoOperation::Merge | RepoOperation::CherryPick | RepoOperation::Revert => {
            let name = head_ref_name(repo).ok()?;
            let previous = operation::hashes(&dir.join("sequencer").join("head"))
                .into_iter()
                .next()
                .or_else(|| ref_commit(repo, &name));
            Some((name, previous))
        }
        RepoOperation::Am | RepoOperation::Bisect => None,
    }
}

/// merge: 그대로면 up to date, HEAD 가 전 HEAD 를 첫 부모로 둔 merge 커밋이면 done, 그 밖에 옮겨졌으면 fast-forward.
fn merge_outcome(before: &Snapshot, after: &Snapshot) -> HistoryOutcome {
    if after.head == before.head {
        HistoryOutcome::UpToDate
    } else if after.head_parents.len() > 1 && after.head_parents.first() == before.head.as_ref() {
        HistoryOutcome::Done
    } else {
        HistoryOutcome::FastForward
    }
}

/// cherry-pick · revert · rebase: 지켜본 ref 가 그대로면 up to date, 아니면 done.
fn rewrite_outcome(before: &Snapshot, after: &Snapshot) -> HistoryOutcome {
    if after.commit == before.commit {
        HistoryOutcome::UpToDate
    } else {
        HistoryOutcome::Done
    }
}

/// `git.merge` 의 fast-forward 선택.
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    specta::Type,
    schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum FastForward {
    /// 할 수 있으면 fast-forward, 아니면 merge 커밋(git 기본, `merge.ff` 설정을 따른다).
    #[default]
    Allow,
    /// fast-forward 만(`--ff-only`). 못 하면 git 의 원문으로 실패한다.
    Only,
    /// 늘 merge 커밋(`--no-ff`).
    Never,
}

/// `git.reset` 의 방식.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum ResetMode {
    /// ref 만 옮긴다 — index · 작업 트리는 그대로.
    Soft,
    /// ref 와 index — 작업 트리는 그대로(git 기본).
    Mixed,
    /// ref · index · 작업 트리 — 저장하지 않은 변경을 잃는다(결정 M2-D6 ① 의 확인 대상). `fingerprint` 가 있어야 한다.
    Hard,
    /// ref · index · 작업 트리를 옮기되 로컬 변경은 지킨다(`--keep`) — 옮길 파일에 로컬 변경이 있으면 git 이 거절한다. [되돌리기] 가
    /// 쓴다. git 이 묻지 않고 덮는 것(ignore 된 파일, 디스크와 다른 스테이징 내용)이 있으면 hard 처럼 `fingerprint` 가 있어야 한다.
    Keep,
}

impl ResetMode {
    fn flag(self) -> &'static str {
        match self {
            Self::Soft => "--soft",
            Self::Mixed => "--mixed",
            Self::Hard => "--hard",
            Self::Keep => "--keep",
        }
    }
}

/// `git.reset` 의 결과.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitResetResult {
    pub work_tree: String,
    /// 끝난 뒤 HEAD 의 커밋.
    pub head: Option<String>,
    /// 움직인 ref. 그대로였거나 전이 없었으면(태어나지 않은 브랜치) `null`.
    pub moved: Option<GitRefMove>,
    /// hard · keep 일 때 잃은 변경(잠금 안에서 reset 바로 전에 읽은 것). soft · mixed 면 `null`.
    pub lost: Option<lost::GitLostChanges>,
}

/// reset 이 잠금 안에서 확인할 것. 하나라도 어긋나면 아무것도 하지 않고 conflict(`-32004`)다.
#[derive(Debug, Clone, Default)]
pub(crate) struct ResetGuard {
    /// HEAD 가 이 커밋이어야 한다.
    pub expected: Option<String>,
    /// HEAD 가 가리키는 ref(분리돼 있으면 `HEAD`)가 이 이름이어야 한다.
    pub expected_ref: Option<String>,
    /// 잃을 변경의 지문(`git.resetPreview`)이 이것이어야 한다. hard 는 늘, keep 은 잃을 것이 있으면 있어야 한다.
    pub fingerprint: Option<String>,
}

fn conflict(message: &str, data: serde_json::Value) -> WriteError {
    WriteError::Rpc(RpcError::new(rpc_code::CONFLICT, message).with_data(data))
}

/// reset 하나. 잠금 안에서 [`ResetGuard`] 를 본다. 서브모듈 작업 트리는 건드리지 않는다(`submodule.recurse` 설정이어도 — 잃을
/// 변경 목록이 서브모듈 안을 세지 않는다).
pub(crate) async fn reset(
    write: &GitWrite,
    request: WriteRequest,
    commit: String,
    mode: ResetMode,
    guard: ResetGuard,
) -> Result<GitResetResult, WriteError> {
    write
        .run(request, move |ctx| async move {
            let name = watched_name(&ctx, &Watched::Head).await?;
            let before = read_snapshot(&ctx, &name).await?;
            if let Some(expected) = &guard.expected
                && before.head.as_deref() != Some(expected.as_str())
            {
                return Err(conflict(
                    "HEAD is no longer at the expected commit; nothing was reset",
                    serde_json::json!({ "head": before.head, "refName": name }),
                ));
            }
            if let Some(expected) = &guard.expected_ref
                && *expected != name
            {
                return Err(conflict(
                    "HEAD no longer points at the expected ref; nothing was reset",
                    serde_json::json!({ "head": before.head, "refName": name }),
                ));
            }
            let lost = if matches!(mode, ResetMode::Hard | ResetMode::Keep) {
                let rev = commit.clone();
                let read = ctx
                    .read(move |o| lost::lost(o, &rev, mode, None))
                    .await?
                    .map_err(WriteError::Rpc)?;
                // 지문을 받았거나 받아야 하면 같아야 한다. 지문을 셀 수 없으면(`dir/` 아래가 너무 많다) 확인할 수 없다.
                let needed =
                    guard.fingerprint.is_some() || mode == ResetMode::Hard || !read.lost.is_empty();
                if needed
                    && (read.fingerprint.is_none() || guard.fingerprint != read.fingerprint)
                {
                    let message = if mode == ResetMode::Hard {
                        "the changes a hard reset loses differ from the confirmed ones; nothing was reset"
                    } else {
                        "the reset would overwrite changes that were not confirmed; nothing was reset"
                    };
                    return Err(conflict(
                        message,
                        serde_json::json!({
                            "head": read.head,
                            "lost": read.lost,
                            "fingerprint": read.fingerprint,
                        }),
                    ));
                }
                Some(read.lost)
            } else {
                None
            };
            // `--` 뒤가 비어 있으면 앞의 인자는 늘 리비전이다(같은 이름의 파일이 있어도).
            ctx.run(ctx.git().args([
                "reset",
                mode.flag(),
                "--no-recurse-submodules",
                commit.as_str(),
                "--",
            ]))
            .await?;
            let after = read_snapshot(&ctx, &name).await?;
            Ok(GitResetResult {
                work_tree: display(ctx.work_tree()),
                moved: moved(&name, &before, &after),
                head: after.head,
                lost,
            })
        })
        .await
}

/// 쓰기 요청의 공통 부분.
pub(crate) fn request(
    project_id: &str,
    worktree: Option<&str>,
    title: &str,
    changes: Changes,
) -> WriteRequest {
    WriteRequest {
        project_id: project_id.to_owned(),
        worktree: worktree.map(str::to_owned),
        title: title.to_owned(),
        changes,
        askpass: false,
    }
}

/// 레지스트리 · 파일 RPC 와 같은 정규화를 거친 경로 문자열.
pub(crate) fn display(path: &std::path::Path) -> String {
    pw_core::registry::path::normalize(path)
        .unwrap_or_else(|_| path.to_path_buf())
        .display()
        .to_string()
}
