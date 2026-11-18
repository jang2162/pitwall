//! 사람의 「태스크 읽기」(17-m6-plan M6-05 ④ — 결정 M6-D8 · D70): `run.tasks.refresh { projectId, worktree?, detector }`.
//!
//! - **누구**: 그 프로젝트 창의 연결만(`exec_grant` — Host · 다른 프로젝트 창 · 파일 창 · 에이전트 범위는 `PERMISSION_DENIED`).
//!   MCP 툴로는 내지 않는다. 미신뢰 프로젝트면 아무것도 띄우지 않고 M6-02 의 실행 거절
//!   (`pw_run::runs::untrusted` — `PERMISSION_DENIED`, `data.reason = "untrusted"`) 그대로다. 도는 동안 신뢰를 거두면 멈춘다.
//! - **무엇**: `gradle` · `maven` 은 빌드 스크립트를 돌려 목록을 읽고([`super::gradle::read`] · [`super::maven::read`]) 캐시에 쓴다.
//!   `mise` 는 캐시하지 않으므로 감지만 다시 한다.
//! - **보이게**: 프로젝트 범위 Host 작업(상태바의 진행 · `task.cancel`)으로 돈다. 취소하거나 요청이 버려지면 프로세스를 세션째
//!   끝낸다. 시한 · 출력 상한은 [`super::exec::PRESSED`].
//! - **결과**: 그 감지기의 감지(태스크 · 까닭 · 목록 상태)와, 실패했으면 그 까닭(`error` — 캐시는 그대로). 창은 이 결과나
//!   `run.config.list` 로 목록을 다시 그린다(프로젝트 창은 하나다).

use serde::{Deserialize, Serialize};

use super::{DetectProblem, TaskList};
use crate::config::methods::DetectedRunTask;
use pw_rpc::rpc_method;

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct RunTasksRefreshParams {
    pub project_id: String,
    /// `run.config.list` 의 `worktree` 와 같다.
    #[serde(default)]
    pub worktree: Option<String>,
    /// 다시 읽을 감지기: `gradle` · `maven` · `mise`.
    pub detector: String,
}

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct RunTasksRefreshed {
    /// 그 감지기의 감지된 태스크(읽은 뒤의 것).
    pub detected: Vec<DetectedRunTask>,
    /// 그 감지기의 감지 까닭.
    pub detect_problems: Vec<DetectProblem>,
    /// 그 감지기의 목록 상태(gradle · maven).
    pub task_lists: Vec<TaskList>,
    /// 읽지 못한 까닭(도구 없음 · 시한 · 실패 끝 코드 · 형식 다름). 있으면 캐시는 그대로다.
    pub error: Option<String>,
}

rpc_method! {
    /// 사람이 「태스크 읽기」 를 눌렀다 — gradle · maven 은 빌드 스크립트를 돌려 태스크 목록을 읽고 캐시한다(Host 작업으로 진행 ·
    /// 취소가 보인다). 그 프로젝트 창의 연결만(아니면 permission denied), 미신뢰 프로젝트면 permission denied
    /// (`data.reason = "untrusted"`), 모르는 감지기는 invalid params, 취소되면 cancelled.
    pub RunTasksRefresh: "run.tasks.refresh" {
        params: RunTasksRefreshParams,
        result: RunTasksRefreshed,
        tier: Exec,
        roles: [Owner],
    }
}
