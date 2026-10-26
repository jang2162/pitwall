//! 진행 중 작업(13-m2-plan.md M2-06, 결정 M2-D5): 종류 · 대상 · 남은 단계를 읽고, 계속 · 중단 · 건너뛰기를 git CLI 로 한다.
//!
//! 읽기는 gix 의 상태 판정(`Repository::state`, git 의 `wt_status_get_state` 와 같다)과 git 디렉터리의 상태 파일이다 — 프로세스를
//! 띄우지 않으므로 미신뢰 프로젝트도 같다. 상태 파일은 일반 파일 · [`MAX_STATE_FILE`] 이하만 읽는다(받은 저장소의 `MERGE_HEAD` 가
//! `/dev/zero` 를 가리키는 링크여도 멈추지 않게).
//!
//! | 종류 | 대상(`heads`) | 단계 | 동작 |
//! | --- | --- | --- | --- |
//! | merge | `MERGE_HEAD`(octopus 면 여럿) | — | 계속 · 중단 |
//! | rebase | `REBASE_HEAD`, 브랜치 `head-name`, `onto` | `msgnum`/`end`(apply 방식은 `next`/`last`) | 계속 · 중단 · 건너뛰기 |
//! | cherry-pick · revert | `CHERRY_PICK_HEAD` · `REVERT_HEAD` | 남은 수 = `sequencer/todo` 의 명령 줄 − 1(첫 줄이 지금 것) | 계속 · 중단 · 건너뛰기 |
//! | am | — | `next`/`last` | 계속 · 중단 · 건너뛰기 |
//! | bisect | — | — | 없음(M2 밖, 상태만 보인다) |

use std::path::Path;

use gix::state::InProgress;
use serde::{Deserialize, Serialize};

use super::Changes;
use crate::methods::RepoOperation;

/// 읽는 상태 파일의 최대 크기. `sequencer/todo` 의 한 줄이 100바이트 남짓이라 수천 단계까지 읽는다.
pub const MAX_STATE_FILE: u64 = 1024 * 1024;

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum OperationAction {
    Continue,
    Abort,
    Skip,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct OperationStep {
    /// 지금 단계(1부터).
    pub current: u32,
    pub total: u32,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct OperationInfo {
    pub kind: RepoOperation,
    /// 들여오는 쪽 커밋(전체 해시): merge 의 `MERGE_HEAD`, cherry-pick · revert 의 그 커밋, rebase 가 멈춘 커밋. 모르면 빈 목록.
    pub heads: Vec<String>,
    /// rebase 하는 브랜치의 짧은 이름. 분리된 HEAD 를 rebase 하거나 rebase 가 아니면 `null`.
    pub branch: Option<String>,
    /// rebase 의 대상 커밋.
    pub onto: Option<String>,
    /// 지금 단계와 전체 단계(rebase · am). 모르면 `null`.
    pub step: Option<OperationStep>,
    /// 지금 단계 뒤에 남은 단계 수. 모르면 `null`.
    pub remaining: Option<u32>,
    /// 할 수 있는 동작(`git.operation.continue` · `abort` · `skip`).
    pub actions: Vec<OperationAction>,
}

/// 저장소의 진행 중 작업. 없으면 `None`.
pub(crate) fn read(repo: &gix::Repository) -> Option<OperationInfo> {
    // `InProgress` 는 복제할 수 없어 두 번 읽는다(상태 파일 몇 개의 존재 확인이다).
    let kind = crate::repo::operation(repo.state()?);
    let state = repo.state()?;
    let dir = repo.git_dir();
    let mut info = OperationInfo {
        kind,
        heads: Vec::new(),
        branch: None,
        onto: None,
        step: None,
        remaining: None,
        actions: actions(kind).to_vec(),
    };
    match state {
        InProgress::Merge => info.heads = hashes(&dir.join("MERGE_HEAD")),
        InProgress::CherryPick | InProgress::CherryPickSequence => {
            info.heads = hashes(&dir.join("CHERRY_PICK_HEAD"));
            info.remaining = Some(sequence_left(dir));
        }
        InProgress::Revert | InProgress::RevertSequence => {
            info.heads = hashes(&dir.join("REVERT_HEAD"));
            info.remaining = Some(sequence_left(dir));
        }
        InProgress::Rebase | InProgress::RebaseInteractive | InProgress::ApplyMailboxRebase => {
            info.heads = hashes(&dir.join("REBASE_HEAD"));
            let (state_dir, numbers) = if dir.join("rebase-merge").is_dir() {
                (dir.join("rebase-merge"), ("msgnum", "end"))
            } else {
                (dir.join("rebase-apply"), ("next", "last"))
            };
            info.branch = read_text(&state_dir.join("head-name"))
                .and_then(|name| name.trim().strip_prefix("refs/heads/").map(str::to_owned));
            info.onto = hashes(&state_dir.join("onto")).into_iter().next();
            info.step = step(&state_dir, numbers);
        }
        InProgress::ApplyMailbox => {
            info.step = step(&dir.join("rebase-apply"), ("next", "last"));
        }
        InProgress::Bisect => {}
    }
    if let Some(step) = info.step {
        info.remaining = Some(step.total.saturating_sub(step.current));
    }
    Some(info)
}

/// 종류마다 할 수 있는 동작.
pub(crate) fn actions(kind: RepoOperation) -> &'static [OperationAction] {
    use OperationAction::{Abort, Continue, Skip};
    match kind {
        RepoOperation::Merge => &[Continue, Abort],
        RepoOperation::Rebase
        | RepoOperation::CherryPick
        | RepoOperation::Revert
        | RepoOperation::Am => &[Continue, Abort, Skip],
        RepoOperation::Bisect => &[],
    }
}

/// 동작의 git 인자. 할 수 없는 동작이면 `None`.
pub(crate) fn args(kind: RepoOperation, action: OperationAction) -> Option<[&'static str; 2]> {
    if !actions(kind).contains(&action) {
        return None;
    }
    let command = match kind {
        RepoOperation::Merge => "merge",
        RepoOperation::Rebase => "rebase",
        RepoOperation::CherryPick => "cherry-pick",
        RepoOperation::Revert => "revert",
        RepoOperation::Am => "am",
        RepoOperation::Bisect => return None,
    };
    let flag = match action {
        OperationAction::Continue => "--continue",
        OperationAction::Abort => "--abort",
        OperationAction::Skip => "--skip",
    };
    Some([command, flag])
}

/// 동작이 작업 트리를 어떻게 바꾸는가(미저장 확인의 범위). merge 를 계속하는 것은 커밋만 하므로 바꾸지 않는다. 그 밖은 다음 단계를
/// 적용하거나 되돌리므로 바뀔 경로를 미리 모른다 — 작업 트리 전체로 본다.
pub(crate) fn changes(kind: RepoOperation, action: OperationAction) -> Changes {
    match (kind, action) {
        (RepoOperation::Merge, OperationAction::Continue) => Changes::None,
        _ => Changes::WorkTree,
    }
}

/// 일반 파일이고 [`MAX_STATE_FILE`] 이하면 그 내용(UTF-8 이 아니면 `None`).
pub(crate) fn read_text(path: &Path) -> Option<String> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    if !meta.file_type().is_file() || meta.len() > MAX_STATE_FILE {
        return None;
    }
    String::from_utf8(std::fs::read(path).ok()?).ok()
}

/// 줄마다 해시 하나인 파일(`MERGE_HEAD` 등)의 해시들. 해시가 아닌 줄은 건너뛴다.
pub(crate) fn hashes(path: &Path) -> Vec<String> {
    read_text(path)
        .map(|text| {
            text.lines()
                .map(str::trim)
                .filter(|l| matches!(l.len(), 40 | 64) && l.bytes().all(|b| b.is_ascii_hexdigit()))
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn number(path: &Path) -> Option<u32> {
    read_text(path)?.trim().parse().ok()
}

fn step(dir: &Path, (current, total): (&str, &str)) -> Option<OperationStep> {
    let current = number(&dir.join(current))?;
    let total = number(&dir.join(total))?;
    (current >= 1 && current <= total).then_some(OperationStep { current, total })
}

/// 순차 cherry-pick · revert 에서 지금 것 뒤에 남은 단계 수. `sequencer/todo` 의 첫 명령이 지금 멈춘 커밋이다. 순차가 아니면 0.
fn sequence_left(dir: &Path) -> u32 {
    let commands = read_text(&dir.join("sequencer").join("todo")).map_or(0, |todo| {
        todo.lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .count()
    });
    u32::try_from(commands.saturating_sub(1)).unwrap_or(u32::MAX)
}
