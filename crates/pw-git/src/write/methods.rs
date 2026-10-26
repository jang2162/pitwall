//! 진행 중 작업과 창 질의 RPC. UI 전용이라 MCP 툴이 아니다(결정 M2-D7 — git 쓰기는 M5 에서도 툴이 아니다).
//!
//! - `git.operation`: 진행 중 작업(읽기, gix · 상태 파일 — 미신뢰 프로젝트도 프로세스를 띄우지 않는다).
//! - `git.operation.continue` · `abort` · `skip`: 쓰기 실행기로 git CLI.
//! - `git.prompts`: 프로젝트 창이 떠 있는 동안 열어 두는 질문 스트림(askpass · 미저장 확인), `git.prompt.answer`: 그 답.

use std::sync::Arc;

use pw_rpc::{Router, RpcError, code as rpc_code, rpc_method};
use serde::{Deserialize, Serialize};

use super::{
    AnswerError, GitPromptAnswer, GitPromptEvent, GitWrite, OperationAction, OperationInfo,
};

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitOperationParams {
    pub project_id: String,
    /// 활성 워크트리의 꼭대기(`git.repo` 와 같다).
    #[serde(default)]
    pub worktree: Option<String>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitOperation {
    /// `null` 이면 진행 중 작업이 없다(저장소가 없을 때도).
    pub operation: Option<OperationInfo>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitOperationResult {
    /// 끝난 뒤의 진행 중 작업. `null` 이면 작업이 끝났다.
    pub operation: Option<OperationInfo>,
    /// git 이 다음 단계에서 다시 멈췄다(충돌) — 실패가 아니라 다음 충돌을 풀 차례다.
    pub stopped: bool,
}

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitPromptsParams {
    pub project_id: String,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GitPromptAnswerParams {
    pub project_id: String,
    #[specta(type = pw_rpc::JsNumber)]
    pub prompt_id: u64,
    pub answer: GitPromptAnswer,
}

rpc_method! {
    /// 진행 중 작업(merge · rebase · cherry-pick · revert · am · bisect): 종류 · 대상 · 남은 단계 · 할 수 있는 동작. 없으면
    /// `operation: null`.
    pub GitOperationMethod: "git.operation" {
        params: GitOperationParams,
        result: GitOperation,
        tier: Read,
        roles: [Owner],
    }
}

rpc_method! {
    /// 진행 중 작업을 계속한다(`git <작업> --continue`). 충돌을 다 풀지 않았으면 git 의 원문으로 실패한다.
    pub GitOperationContinueMethod: "git.operation.continue" {
        params: GitOperationParams,
        result: GitOperationResult,
        tier: Write,
        roles: [Owner],
    }
}

rpc_method! {
    /// 진행 중 작업을 그만두고 시작 전으로 돌린다(`git <작업> --abort`). 확인은 창이 먼저 한다(결정 M2-D6 ①).
    pub GitOperationAbortMethod: "git.operation.abort" {
        params: GitOperationParams,
        result: GitOperationResult,
        tier: Write,
        roles: [Owner],
    }
}

rpc_method! {
    /// 진행 중 작업의 지금 단계를 건너뛴다(`git <작업> --skip`, merge 는 없다).
    pub GitOperationSkipMethod: "git.operation.skip" {
        params: GitOperationParams,
        result: GitOperationResult,
        tier: Write,
        roles: [Owner],
    }
}

rpc_method! {
    /// 프로젝트 창이 떠 있는 동안 열어 두는 질문 스트림: askpass 입력과 쓰기 전 미저장 확인. 열 때 아직 열린 질문이 먼저 온다.
    /// 듣는 창이 없으면 askpass 는 바로 실패하고 미저장 확인은 묻지 않고 진행한다.
    pub GitPromptsMethod: "git.prompts" {
        params: GitPromptsParams,
        stream: GitPromptEvent,
        tier: Read,
        roles: [Owner],
    }
}

rpc_method! {
    /// 질문의 답. 처음 온 답만 받는다 — 이미 끝난 질문이면 not found, 종류에 맞지 않는 답이면 invalid params.
    pub GitPromptAnswerMethod: "git.prompt.answer" {
        params: GitPromptAnswerParams,
        result: (),
        tier: Write,
        roles: [Owner],
    }
}

/// 핸들러를 붙인다.
pub fn register(router: &mut Router, write: Arc<GitWrite>) {
    let w = Arc::clone(&write);
    router.unary(
        GitOperationMethod,
        move |_ctx, params: GitOperationParams| {
            let w = Arc::clone(&w);
            async move {
                let operation = w
                    .operation(&params.project_id, params.worktree.as_deref())
                    .await?;
                Ok(GitOperation { operation })
            }
        },
    );
    let w = Arc::clone(&write);
    router.unary(GitOperationContinueMethod, move |_ctx, params| {
        act(Arc::clone(&w), params, OperationAction::Continue)
    });
    let w = Arc::clone(&write);
    router.unary(GitOperationAbortMethod, move |_ctx, params| {
        act(Arc::clone(&w), params, OperationAction::Abort)
    });
    let w = Arc::clone(&write);
    router.unary(GitOperationSkipMethod, move |_ctx, params| {
        act(Arc::clone(&w), params, OperationAction::Skip)
    });
    let w = Arc::clone(&write);
    router.stream(
        GitPromptsMethod,
        move |_ctx, params: GitPromptsParams, sink| {
            let w = Arc::clone(&w);
            async move {
                let git = Arc::clone(w.git());
                let project = params.project_id.clone();
                tokio::task::spawn_blocking(move || {
                    git.host()?
                        .projects
                        .get(&project)
                        .map(drop)
                        .map_err(crate::registry_error)
                })
                .await
                .map_err(|e| RpcError::internal(e.to_string()))??;
                let mut listener = w.prompts().listen(&params.project_id);
                while let Some(event) = listener.recv().await {
                    sink.send(event).await?;
                }
                Ok(())
            }
        },
    );
    let w = Arc::clone(&write);
    router.unary(
        GitPromptAnswerMethod,
        move |_ctx, params: GitPromptAnswerParams| {
            let w = Arc::clone(&w);
            async move {
                w.prompts()
                    .answer(&params.project_id, params.prompt_id, params.answer)
                    .map_err(|e| match e {
                        AnswerError::NotFound => RpcError::new(
                            rpc_code::NOT_FOUND,
                            "no such open prompt (already answered or closed)",
                        ),
                        AnswerError::Mismatch => {
                            RpcError::invalid_params("the answer does not fit the prompt")
                        }
                    })
            }
        },
    );
}

async fn act(
    write: Arc<GitWrite>,
    params: GitOperationParams,
    action: OperationAction,
) -> Result<GitOperationResult, RpcError> {
    Ok(write
        .act(&params.project_id, params.worktree.as_deref(), action)
        .await?)
}
