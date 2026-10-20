//! 외부 도구 RPC. UI 전용이라 MCP 툴이 아니다(`docs/design/05-mcp.md` 의 `open_with_*` 툴은 이후).
//!
//! 웹뷰는 도구 id 와 문맥(프로젝트, 활성 워크트리, 있으면 파일 · 줄 · 열)만 넘긴다. 명령을 만들고 띄우는 것은 이 Host 다.

use std::sync::Arc;

use pw_rpc::{Router, RpcError, rpc_method};
use serde::{Deserialize, Serialize};

use crate::OpenWith;

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct ExternalToolInfo {
    pub id: String,
    pub label: String,
    /// 기여한 확장의 id. 전역 설정에서만 정의한 도구면 `null`.
    pub extension: Option<String>,
    /// 전역 설정(`[open_with.tools.<id>]`)이 고쳤거나 더했는가.
    pub from_settings: bool,
    /// 파일까지 여는 명령(`file_command`)이 있는가.
    pub opens_files: bool,
    /// 설정 형식 오류. 있으면 실행하지 않는다(실행하면 `invalidTool` 로 끝난다).
    pub problem: Option<String>,
}

/// 기본 도구.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum DefaultToolInfo {
    Tool {
        id: String,
    },
    /// 프로젝트 · 전역 설정 어디에도 `open_with.default` 가 없다.
    Unset,
    /// 고른 도구가 이 OS 의 목록에 없다.
    Unknown {
        id: String,
    },
}

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct OpenWithListParams {
    /// 있으면 그 프로젝트의 설정(`[open_with] default`)까지 보고 기본 도구를 고른다.
    #[serde(default)]
    pub project_id: Option<String>,
    /// 활성 워크트리의 꼭대기(`ActiveWorktree.top`, 절대 경로). 그 워크트리의 `.local.toml` 을 읽는다.
    #[serde(default)]
    pub worktree: Option<String>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct OpenWithList {
    /// 이 OS 에서 쓸 수 있는 도구(`platforms` 로 거른 것). 확장 순서, 그 뒤에 설정에서만 정의한 도구.
    pub tools: Vec<ExternalToolInfo>,
    pub default: DefaultToolInfo,
}

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct OpenWithRunParams {
    pub project_id: String,
    /// 없으면 기본 도구. 기본 도구가 없으면 `noDefault` 로 끝난다(창은 「다음에서 열기」 팝업을 연다).
    #[serde(default)]
    pub tool: Option<String>,
    /// 활성 워크트리의 꼭대기(`ActiveWorktree.top`, 절대 경로). `{project_root}` 와 작업 디렉터리가 그 워크트리 안의 프로젝트 루트가 된다.
    #[serde(default)]
    pub worktree: Option<String>,
    /// 열 파일의 절대 경로. 있으면 도구의 `file_command` 를 쓴다(없으면 `command`).
    #[serde(default)]
    pub file: Option<String>,
    #[serde(default)]
    pub line: Option<u32>,
    #[serde(default)]
    pub column: Option<u32>,
}

/// 실행하지 못한 이유. 화면은 `kind` 로 번역된 문장을 고른다.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum LaunchProblem {
    /// 이 OS 의 목록에 없는 도구 id.
    UnknownTool,
    /// 설정 형식 오류(`ExternalToolInfo.problem`).
    InvalidTool { message: String },
    /// 명령이 쓰는 변수에 이 문맥의 값이 없다(중괄호 없는 이름, 예: `branch`).
    MissingVariable { variable: String },
    /// 프로젝트 루트(작업 디렉터리)가 없다.
    ProjectRootMissing { path: String },
    /// 실행 파일을 찾지 못했다. `loginShellPath` 는 찾아본 PATH 가 로그인 셸의 것인가(아니면 앱 프로세스의 것).
    NotFound {
        program: String,
        login_shell_path: bool,
    },
    /// 실행 파일을 띄우지 못했다(권한 등).
    SpawnFailed { program: String, message: String },
    /// `open`(macOS LaunchServices)에 번들 폴더(`*.app` 등)를 넘기려 했다 — `open` 은 번들을 열지 않고 실행하므로 띄우지 않았다.
    BundlePath { path: String },
    /// 띄운 런처가 곧바로 0 이 아닌 코드로 끝났다. 신호로 끝났으면 `code` 가 없다.
    Exited { code: Option<i32> },
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum OpenWithOutcome {
    Launched {
        tool: String,
        label: String,
        /// 띄운 argv(첫 요소는 찾은 실행 파일의 절대 경로).
        argv: Vec<String>,
        pid: u32,
    },
    Failed {
        tool: String,
        /// 목록에 없는 도구면 id.
        label: String,
        /// 치환까지 마친 argv. 치환 전에 실패했으면 비어 있다.
        argv: Vec<String>,
        problem: LaunchProblem,
    },
    /// 도구를 고르지 않았고 기본 도구도 없다. `configured` 는 설정이 고른, 이 OS 에 없는 도구.
    NoDefault { configured: Option<String> },
}

rpc_method! {
    /// 이 OS 에서 쓸 수 있는 외부 도구와 기본 도구(04 F8-1 · F8-4). 「다음에서 열기」 팝업이 쓴다.
    pub OpenWithListMethod: "openWith.list" {
        params: OpenWithListParams,
        result: OpenWithList,
        tier: Read,
        roles: [Owner],
    }

    /// 프로젝트를 외부 도구로 연다. 셸 없이 argv 를 띄우고(Pitwall 과 따로 산다) 곧바로 실패하는지 잠깐 본다. 실행하지 못한 이유는
    /// 오류가 아니라 `failed` 결과로 온다. 없는 프로젝트 · 상대 경로 파일 · 상대 경로이거나 그 프로젝트의 것이 아닌 워크트리는 invalid params 오류.
    pub OpenWithRun: "openWith.run" {
        params: OpenWithRunParams,
        result: OpenWithOutcome,
        tier: Exec,
        roles: [Owner],
    }
}

/// 핸들러를 붙인다. 레지스트리 · 설정 읽기와 실행(로그인 셸 환경을 기다릴 수 있다)은 블로킹 스레드에서 한다.
pub fn register(router: &mut Router, open_with: Arc<OpenWith>) {
    async fn blocking<T: Send + 'static>(
        f: impl FnOnce() -> Result<T, RpcError> + Send + 'static,
    ) -> Result<T, RpcError> {
        tokio::task::spawn_blocking(f)
            .await
            .map_err(|e| RpcError::internal(e.to_string()))?
    }
    let list = Arc::clone(&open_with);
    router
        .unary(
            OpenWithListMethod,
            move |_ctx, params: OpenWithListParams| {
                let s = Arc::clone(&list);
                // 팝업을 여는 사람은 곧 실행한다 — 로그인 셸 환경을 미리 가져오기 시작한다.
                s.prefetch_env();
                blocking(move || s.list(&params))
            },
        )
        .unary(OpenWithRun, move |_ctx, params: OpenWithRunParams| {
            let s = Arc::clone(&open_with);
            blocking(move || s.run(&params))
        });
}
