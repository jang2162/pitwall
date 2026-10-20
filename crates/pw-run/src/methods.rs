//! 터미널 RPC. UI 전용이라 MCP 툴이 아니다.
//!
//! `terminal.create` 는 스트림이다. 항목은 `started` 하나 → `output` 여럿 → `exited` 하나 순서로 오고, `exited` 뒤에 스트림이 끝난다.
//! 스트림이 끝나면(창이 닫힘 · 연결 끊김 · `stream.cancel`) 셸의 프로세스 트리도 끝난다 — 스트림 없이 사는 터미널이 없으므로 종료를
//! 따로 알리는 버스 이벤트가 없다.

use std::sync::Arc;

use pw_rpc::{Router, RpcError, rpc_method};
use serde::{Deserialize, Serialize};

use crate::Terminals;

/// 터미널 하나의 번호(1부터). 그 터미널을 연 RPC 세션 안에서만 유효하다 — 다른 세션이 부르면 없는 터미널이다.
pub type TerminalId = u32;

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct TerminalCreateParams {
    pub project_id: String,
    /// 셸의 작업 디렉터리(절대 경로). 프로젝트 루트나 그 루트를 담은 연결 워크트리 안이어야 한다 — 창은 활성 워크트리의 프로젝트
    /// 루트를 준다. 없으면 프로젝트의 주 루트.
    #[serde(default)]
    pub cwd: Option<String>,
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
    /// PTY 출력. `data` 는 바이트의 base64 — 청크 경계가 UTF-8 문자 가운데일 수 있다.
    Output { data: String },
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

rpc_method! {
    /// 프로젝트의 디렉터리에서 셸을 PTY 로 띄운다(결정 M1-D14: `$SHELL -l`, 전역 설정 `[terminal] shell` 로 덮기). 환경은 그
    /// 디렉터리의 런타임 환경 + `TERM=xterm-256color` · `COLORTERM=truecolor` · `TERM_PROGRAM=Pitwall`. 출력은 credit 스트림(청크는 16 KiB
    /// 이하)이고, 소비자가 읽지 않으면 Host 버퍼(512 KiB)가 찬 뒤 셸이 쓰기에서 멈춘다 — 기본 window(32)면 내보낸 청크와 합쳐
    /// 1 MiB 를 넘지 않는다. `terminalId` 는 이 세션(연결) 안에서만 유효하다. 없는 프로젝트 · 프로젝트 밖이거나 없는
    /// `cwd` · 0 인 크기면 스트림이 invalid params 오류로, 셸을 띄우지 못하면 internal 오류로 끝난다(`started` 없이).
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
    /// 터미널 · 다른 세션이 연 터미널이면 아무것도 하지 않는다.
    pub TerminalClose: "terminal.close" {
        params: TerminalCloseParams,
        result: (),
        tier: Exec,
        roles: [Owner],
    }
}

/// 핸들러를 붙인다. 입력은 핸들러의 동기 구간에서 큐에 넣는다 — 세션은 요청이 온 순서대로 그 구간을 부르므로 키 입력 순서가
/// 지켜진다(본문은 태스크마다 따로 돈다).
pub fn register(router: &mut Router, terminals: Arc<Terminals>) {
    let create = Arc::clone(&terminals);
    let input = Arc::clone(&terminals);
    let resize = Arc::clone(&terminals);
    router
        .stream(TerminalCreate, move |ctx, params, sink| {
            Arc::clone(&create).run(ctx.session(), params, sink)
        })
        .unary(TerminalInput, move |ctx, params: TerminalInputParams| {
            let result = input.input(ctx.session(), params.terminal_id, params.data.into_bytes());
            async move { result }
        })
        .unary(TerminalResize, move |ctx, params: TerminalResizeParams| {
            let result = resize.resize(ctx.session(), params.terminal_id, params.cols, params.rows);
            async move { result }
        })
        .unary(TerminalClose, move |ctx, params: TerminalCloseParams| {
            terminals.close(ctx.session(), params.terminal_id);
            async { Ok::<(), RpcError>(()) }
        });
}
