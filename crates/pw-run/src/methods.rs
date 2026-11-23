//! 터미널 RPC. UI 전용이라 MCP 툴이 아니다.
//!
//! `terminal.create` 는 스트림이다. 항목은 `started` 하나 → `output` 여럿 → `exited` 하나 순서로 오고, `exited` 뒤에 스트림이 끝난다.
//!
//! - **로컬 Host**: 스트림이 끝나면(창이 닫힘 · 연결 끊김 · `stream.cancel`) 셸의 프로세스 트리도 끝난다 — 스트림 없이 사는 터미널이
//!   없으므로 종료를 따로 알리는 버스 이벤트가 없다. `terminal.detach` 는 invalid params, `terminal.list` 는 늘 비었다.
//! - **원격 Host(Relay)**: 터미널은 Host 의 것이다(18-m7-plan M7-09 ② — `crate::hosted`). 스트림이 끝나도 셸은 떼어진 터미널로 남고
//!   `terminal.attach { terminalId, since }` 로 다시 붙는다. `output` 에 오프셋(`offset`)이 실린다. `terminalId` 를 받는 메서드는 그
//!   프로젝트의 창만 받는다 — 다른 범위는 permission denied, 다른 프로젝트의 번호는 없는 터미널이다. 미신뢰 프로젝트의 터미널은
//!   연결 수명이다(떼기 · 붙기는 permission denied `untrusted`, 연결이 닫히면 끝남). 신뢰를 거두면 그 프로젝트의 터미널은 모두 끝난다.

use std::sync::Arc;

use pw_rpc::{Router, rpc_method};
use serde::{Deserialize, Serialize};

use crate::Terminals;

/// 터미널 하나의 번호(1부터). 로컬 Host 에서는 그 터미널을 연 RPC 세션 안에서만 유효하다 — 다른 세션이 부르면 없는 터미널이다.
/// 원격 Host 에서는 Host 안에서 유일하고 그 프로젝트의 창이면 어느 연결이든 쓴다.
pub type TerminalId = u32;

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct TerminalCreateParams {
    pub project_id: String,
    /// 셸의 작업 디렉터리(절대 경로). 프로젝트 루트나 그 루트를 담은 연결 워크트리 안이어야 한다. 없으면 `worktree` 안의 프로젝트
    /// 루트, 그것도 없으면 프로젝트의 주 루트.
    #[serde(default)]
    pub cwd: Option<String>,
    /// 창의 활성 워크트리 꼭대기(`ActiveWorktree.top`, `fs.*` · `openWith.run` 의 `worktree` 와 같다). 그 프로젝트의 연결 워크트리가
    /// 아니면(위조한 `.git` 파일 포함) `cwd` 가 있어도 invalid params 오류다. 창은 이것만 주고 `cwd` 는 비운다 — 창은 워크트리 안의
    /// 프로젝트 루트(모노레포의 하위 폴더일 수 있다)를 모른다.
    #[serde(default)]
    pub worktree: Option<String>,
    pub cols: u16,
    pub rows: u16,
}

/// `terminal.create` 스트림의 항목.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum TerminalEvent {
    /// 첫 항목. `terminalId` 로 `terminal.input` · `resize` · `close` 를 부른다.
    Started {
        terminal_id: TerminalId,
        /// 셸의 pid. OS 가 알려 주지 않으면 `null`.
        pid: Option<u32>,
        /// 띄운 셸의 argv.
        shell: Vec<String>,
        /// 셸의 작업 디렉터리(정규화한 절대 경로).
        cwd: String,
    },
    /// PTY 출력. `data` 는 바이트의 base64 — 청크 경계가 UTF-8 문자 가운데일 수 있다. 원격 Host 의 터미널이면 `offset` 은 그
    /// 청크의 첫 바이트의 오프셋(셸이 시작된 뒤 낸 바이트 수) — 앞 청크의 끝보다 크면 그 사이는 링 버퍼에서 버려졌다. 다시 붙을 때
    /// 받은 마지막 끝을 `terminal.attach` 의 `since` 로 준다. 로컬 Host 는 `null`.
    Output {
        data: String,
        #[serde(default)]
        #[specta(type = Option<pw_rpc::JsNumber>)]
        offset: Option<u64>,
    },
    /// 마지막 항목(12-m1-plan.md M1-08 의 `terminal.exited { code }`). 출력을 모두 보낸 뒤에 온다. 신호로 끝났으면(`terminal.close`
    /// 포함) `code` 가 `null`.
    Exited { code: Option<i32> },
}

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct TerminalInputParams {
    pub terminal_id: TerminalId,
    /// 셸에 보낼 글자(키 입력 · 붙여넣기). 받은 순서대로 쓰인다.
    pub data: String,
}

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct TerminalResizeParams {
    pub terminal_id: TerminalId,
    pub cols: u16,
    pub rows: u16,
}

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct TerminalCloseParams {
    pub terminal_id: TerminalId,
}

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct TerminalDetachParams {
    pub terminal_id: TerminalId,
    /// 떼어진 터미널이 상한을 넘으면 가장 오래 떼어진 것부터 끝낸다(사람이 확인했다). 거짓이면 넘을 때 아무것도 하지 않고 conflict
    /// 오류(`data.wouldEnd` — 끝낼 번호)를 준다.
    #[serde(default)]
    pub end_oldest: bool,
}

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct TerminalDetached {
    /// 상한 때문에 끝낸 떼어진 터미널.
    pub ended: Vec<TerminalId>,
}

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct TerminalAttachParams {
    pub terminal_id: TerminalId,
    /// 이 오프셋부터 보낸다(받은 마지막 `output` 의 `offset` + 바이트 수). 없으면 0 — 남은 처음부터.
    #[serde(default)]
    #[specta(type = Option<pw_rpc::JsNumber>)]
    pub since: Option<u64>,
}

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct TerminalListParams {
    pub project_id: String,
}

/// Host 수명 터미널 하나(`terminal.list` · `terminal.attach` 의 `attached`).
#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct TerminalInfo {
    pub terminal_id: TerminalId,
    /// 띄울 때 준 워크트리(`terminal.create` 의 `worktree`) — 창이 체크아웃마다 다시 연다.
    pub worktree: Option<String>,
    pub cwd: String,
    pub shell: Vec<String>,
    pub pid: Option<u32>,
    /// 붙은 스트림이 있는가. 없으면 떼어진 터미널이다.
    pub attached: bool,
    /// 셸이 끝났다(남은 출력과 `exited` 는 다음 붙기가 받는다).
    pub exited: bool,
    /// 끝 코드(신호로 끝났거나 아직 돌면 `null`).
    pub code: Option<i32>,
}

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct TerminalList {
    pub terminals: Vec<TerminalInfo>,
}

/// `terminal.attach` 스트림의 항목.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum TerminalAttachEvent {
    /// 첫 항목: 지금의 터미널. `from` 은 처음 보낼 오프셋 — `since` 앞이 링 버퍼에서 버려졌으면 `since` 보다 크고, 그 차이가
    /// `dropped`(잃은 바이트)다.
    Attached {
        terminal: Box<TerminalInfo>,
        #[specta(type = pw_rpc::JsNumber)]
        from: u64,
        #[specta(type = pw_rpc::JsNumber)]
        dropped: u64,
    },
    /// 출력(`terminal.create` 의 `output` 과 같다 — `offset` 이 늘 실린다).
    Output {
        #[specta(type = pw_rpc::JsNumber)]
        offset: u64,
        data: String,
    },
    /// 마지막 항목 — 셸이 끝났고 남은 출력을 다 보냈다.
    Exited { code: Option<i32> },
}

rpc_method! {
    /// 프로젝트의 디렉터리에서 셸을 PTY 로 띄운다(결정 M1-D14: `$SHELL -l`, 전역 설정 `[terminal] shell` 로 덮기). 환경은 그
    /// 디렉터리의 런타임 환경 + `TERM=xterm-256color` · `COLORTERM=truecolor` · `TERM_PROGRAM=Pitwall`. 출력은 credit 스트림(청크는 16 KiB
    /// 이하)이고, 소비자가 읽지 않으면 Host 버퍼(512 KiB)가 찬 뒤 셸이 쓰기에서 멈춘다 — 기본 window(32)면 내보낸 청크와 합쳐
    /// 1 MiB 를 넘지 않는다. `terminalId` 는 이 세션(연결) 안에서만 유효하다. 없는 프로젝트 · 프로젝트 밖이거나 없는
    /// `cwd` · 0 인 크기면 스트림이 invalid params 오류로, 그 프로젝트의 창이 아닌 연결(웰컴 · 설정 · 다른 프로젝트 창)이면 permission
    /// denied 오류로, 셸을 띄우지 못하면 internal 오류로 끝난다(`started` 없이).
    pub TerminalCreate: "terminal.create" {
        params: TerminalCreateParams,
        stream: TerminalEvent,
        tier: Exec,
        roles: [Owner],
    }

    /// 셸에 입력을 보낸다. 끝난 터미널 · 다른 세션이 연 터미널이면 invalid params 오류.
    pub TerminalInput: "terminal.input" {
        params: TerminalInputParams,
        result: (),
        tier: Exec,
        roles: [Owner],
    }

    /// PTY 크기를 바꾼다(셸은 SIGWINCH 를 받는다). 끝난 터미널 · 다른 세션이 연 터미널 · 0 인 크기는
    /// invalid params 오류.
    pub TerminalResize: "terminal.resize" {
        params: TerminalResizeParams,
        result: (),
        tier: Write,
        roles: [Owner],
    }

    /// 셸과 그 세션의 프로세스를 끝낸다(SIGHUP, 남으면 SIGKILL). 스트림은 남은 출력과 `exited` 를 보내고 끝난다. 이미 끝난
    /// 터미널 · 다른 세션이 연 터미널이면 아무것도 하지 않는다. 원격 Host 에서는 그 프로젝트의 창이면 떼어진 터미널도 끝낸다(다른
    /// 범위는 permission denied).
    pub TerminalClose: "terminal.close" {
        params: TerminalCloseParams,
        result: (),
        tier: Exec,
        roles: [Owner],
    }

    /// 원격 Host 의 터미널에서 이 연결의 스트림을 떼고 셸은 둔다([두고 닫기] — 18-m7-plan M7-09 ②). 떼어진 터미널은 머신당 16개까지
    /// — 넘으면 `endOldest` 가 아닐 때 conflict 오류(`data.wouldEnd`)로 아무것도 하지 않고, `endOldest` 면 가장 오래 떼어진 것부터
    /// 끝낸다. 로컬 Host 는 invalid params(창이 닫히면 끝난다), 프로젝트 창이 아니면 permission denied, 다른 프로젝트의 번호는 없는
    /// 터미널(invalid params).
    pub TerminalDetach: "terminal.detach" {
        params: TerminalDetachParams,
        result: TerminalDetached,
        tier: Exec,
        roles: [Owner],
    }

    /// 원격 Host 의 터미널에 다시 붙어 `since` 오프셋부터 출력을 받는다(`attached` 하나 → `output` 여럿 → 끝났으면 `exited`). 스트림을
    /// 끊어도 셸은 돈다. 그 프로젝트의 창 · owner 만 — 다른 범위는 permission denied, 다른 프로젝트의 번호 · 없는 터미널은 invalid
    /// params.
    pub TerminalAttach: "terminal.attach" {
        params: TerminalAttachParams,
        stream: TerminalAttachEvent,
        tier: Exec,
        roles: [Owner],
    }

    /// 그 프로젝트의 원격 Host 터미널(붙음 · 떼어짐 · 떼어진 채 끝남). 다시 붙은 창이 읽어 체크아웃마다 다시 연다. 그 프로젝트의
    /// 창만(아니면 permission denied). 로컬 Host 는 늘 비었다.
    pub TerminalListMethod: "terminal.list" {
        params: TerminalListParams,
        result: TerminalList,
        tier: Read,
        roles: [Owner],
    }
}

/// 핸들러를 붙인다. 입력은 핸들러의 동기 구간에서 큐에 넣는다 — 세션은 요청이 온 순서대로 그 구간을 부르므로 키 입력 순서가
/// 지켜진다(본문은 태스크마다 따로 돈다).
pub fn register(router: &mut Router, terminals: Arc<Terminals>) {
    let create = Arc::clone(&terminals);
    let input = Arc::clone(&terminals);
    let resize = Arc::clone(&terminals);
    let detach = Arc::clone(&terminals);
    let attach = Arc::clone(&terminals);
    let list = Arc::clone(&terminals);
    let close = terminals;
    router
        .stream(TerminalCreate, move |ctx, params, sink| {
            let (create, scope) = (Arc::clone(&create), ctx.scope().clone());
            async move { create.run(ctx.session(), &scope, params, sink).await }
        })
        .unary(TerminalInput, move |ctx, params: TerminalInputParams| {
            let result = input.input_from(
                ctx.session(),
                ctx.scope(),
                params.terminal_id,
                params.data.into_bytes(),
            );
            async move { result }
        })
        .unary(TerminalResize, move |ctx, params: TerminalResizeParams| {
            let result = resize.resize_from(
                ctx.session(),
                ctx.scope(),
                params.terminal_id,
                params.cols,
                params.rows,
            );
            async move { result }
        })
        .unary(TerminalClose, move |ctx, params: TerminalCloseParams| {
            let result = close.close_from(ctx.session(), ctx.scope(), params.terminal_id);
            async move { result }
        })
        .unary(TerminalDetach, move |ctx, params: TerminalDetachParams| {
            let result = detach.detach(
                ctx.session(),
                ctx.scope(),
                params.terminal_id,
                params.end_oldest,
            );
            async move { result }
        })
        .stream(
            TerminalAttach,
            move |ctx, params: TerminalAttachParams, sink| {
                let (attach, scope) = (Arc::clone(&attach), ctx.scope().clone());
                async move {
                    attach
                        .attach_stream(ctx.session(), &scope, params, sink)
                        .await
                }
            },
        )
        .unary(
            TerminalListMethod,
            move |ctx, params: TerminalListParams| {
                let result = list
                    .list(ctx.scope(), &params.project_id)
                    .map(|terminals| TerminalList { terminals });
                async move { result }
            },
        );
}
