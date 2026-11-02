//! 서버 관리자의 RPC(15-m4-plan.md M4-05): 문서 동기화 `lsp.document.*`, 상태 `lsp.status.get` · 이벤트 [`TOPIC_STATUS`], 제어
//! `lsp.server.restart` · `lsp.server.stop`. UI 전용이라 MCP 툴이 아니다(결정 M4-D21).
//!
//! **등급**(완료 기준 ⑪): 서버를 띄울 수 있는 것(`lsp.document.open` · `lsp.server.restart` · `lsp.server.stop`)은 tier exec,
//! 이미 뜬 서버에 보내기만 하는 것(`change` · `save` · `close` · 기능 RPC)과 상태 읽기는 tier read. 띄우는 것은 그 프로젝트 창의
//! 연결만이다 — `LspGrant`(`crate::grant`)가 판정한다.
//!
//! 문서는 연 연결(창)에 속한다 — 다른 연결이 그 `docId` 로 부르면 not found 다. 연결이 닫히면 그 연결의 문서가 모두 닫힌다.

use std::sync::Arc;

use pw_core::registry::exec::{Launch, exec_grant};
use pw_rpc::{Router, rpc_method};
use serde::{Deserialize, Serialize};

use crate::document::LspTextChange;
use crate::manager::LspManager;

/// 프로젝트의 서버 인스턴스 상태가 바뀌었다. 범위는 그 프로젝트, 내용은 [`LspStatus`](그 프로젝트의 인스턴스 전부).
pub const TOPIC_STATUS: &str = "lsp.status";

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct LspDocumentOpenParams {
    pub project_id: String,
    /// 활성 워크트리의 꼭대기(`ActiveWorktree.top`, 절대 경로).
    #[serde(default)]
    pub worktree: Option<String>,
    /// 문서 파일의 절대 경로. 프로젝트 루트(활성 워크트리면 그 안의 루트) 안이어야 한다.
    pub path: String,
    /// 문서 언어 — `lang.detect` 의 `language`(Linguist 이름, `Rust`) 또는 확장의 언어 id(`rust`).
    pub language: String,
    /// 웹뷰 문서의 버전. 다음 `lsp.document.change` 는 이것 + 1 이다.
    pub version: u32,
    pub text: String,
}

/// 서버에 보내지 않는 문서의 까닭. 이 `docId` 는 열리지 않았다(바꾸기 · 닫기는 not found) — 웹뷰는 그 문서의 동기화를 멈춘다.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum LspDocumentOff {
    /// 5 MiB 를 넘는다(편집 등급 밖 — 결정 M1-D9).
    TooLarge,
    /// 1만 자를 넘는 줄이 있다(편집 등급 밖).
    LongLine,
    /// 그 언어를 맡는 언어 서버 정의가 없다.
    NoServer,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LspDocumentOpened {
    /// 이 문서의 id(이 연결 안에서만). `off` 면 쓰지 않는다.
    pub doc_id: String,
    pub off: Option<LspDocumentOff>,
    /// 이 문서를 맡는 인스턴스들(정의 순서 — 결정 M4-D12). 미신뢰 프로젝트면 상태가 `untrusted` 다.
    pub instances: Vec<LspInstanceStatus>,
}

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct LspDocumentChangeParams {
    pub doc_id: String,
    /// 이 편집을 적용한 뒤의 버전 = 앞 버전 + 1.
    pub version: u32,
    /// 차례로 적용한다 — 각 바꾸기의 위치는 앞 바꾸기를 적용한 뒤의 텍스트 기준이다(`syntax.edit` 와 같은 꼴, UTF-16).
    pub changes: Vec<LspTextChange>,
}

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct LspDocumentChangeResult {
    /// 이 편집으로 편집 등급을 벗어나 서버에서 닫았다(이 `docId` 는 이제 없다).
    pub off: Option<LspDocumentOff>,
}

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct LspDocumentRef {
    pub doc_id: String,
}

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct LspStatusGetParams {
    pub project_id: String,
}

/// [`TOPIC_STATUS`] 의 내용이자 `lsp.status.get` 의 결과.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LspStatus {
    pub project_id: String,
    /// (워크트리, 서버, 루트) 순.
    pub instances: Vec<LspInstanceStatus>,
}

/// 서버 인스턴스 하나 — (프로젝트, 워크트리, 서버, 작업 공간 루트)마다 하나(결정 M4-D11).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LspInstanceStatus {
    /// 서버 id(`[contributes.language_servers.<id>]`).
    pub server: String,
    /// 작업 공간 루트(실제 경로 — 워크트리마다 다르다). `lsp.server.restart` · `stop` 은 이것과 그 창의 활성 워크트리로 고른다.
    pub root: String,
    pub state: LspInstanceState,
    /// 서버가 알린 진행(`$/progress`) 중 가장 최근 것. 같은 진행이 `Task` 로도 나간다.
    pub progress: Option<LspProgress>,
}

/// 인스턴스의 상태.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum LspInstanceState {
    /// 띄우는 중(`initialize` 를 기다림 · 죽은 뒤 물러남 재기동을 기다림).
    Starting,
    /// 답한다.
    Ready,
    /// 설정 · 정의가 바뀌었다 — 다시 시작해야 새 값이 든다. 서버는 그대로 답한다.
    RestartNeeded,
    /// 런타임 환경이 바뀌었다(`runtime.changed`) — 다시 시작을 권한다. 서버는 그대로 답한다(저절로 다시 띄우지 않는다).
    RestartSuggested,
    /// 3분 안에 5번 죽어 멈췄다 — `lsp.server.restart` 로만 다시 띄운다.
    Crashed,
    /// 사람이 멈췄다(`lsp.server.stop`).
    Stopped,
    /// 프로젝트를 신뢰하지 않아 띄우지 않는다(결정 M4-D8).
    Untrusted,
    /// 실행 파일을 찾지 못했다 — 설치를 제안한다(M4-17).
    NotInstalled { mason: Option<String> },
    /// 서버를 돌릴 런타임(`requires`)을 찾지 못했다(결정 M4-D6). `tool` 은 `node` · `python` · `java`, `needed` 는 판 범위.
    RuntimeMissing { tool: String, needed: String },
    /// 설정에서 껐다(`[lsp.<id>] enabled = false`).
    Disabled,
    /// 띄우지 못했다(설정 경로가 틀림 · 실행 실패 등 — 까닭).
    Failed { message: String },
}

impl LspInstanceState {
    /// 서버 프로세스가 떠서 답하는 상태인가.
    pub fn is_running(&self) -> bool {
        matches!(
            self,
            Self::Ready | Self::RestartNeeded | Self::RestartSuggested
        )
    }
}

/// 서버의 진행 하나(`$/progress` 의 `WorkDoneProgress`).
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct LspProgress {
    pub title: String,
    pub message: Option<String>,
    /// 0 ~ 100. 서버가 모르면 없다.
    pub percentage: Option<u32>,
}

/// `lsp.server.restart` · `lsp.server.stop` 의 대상 — 상태 이벤트의 인스턴스 하나.
#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct LspServerControlParams {
    pub project_id: String,
    #[serde(default)]
    pub worktree: Option<String>,
    pub server: String,
    /// [`LspInstanceStatus::root`] 그대로.
    pub root: String,
}

rpc_method! {
    /// 문서를 연다: Host 가 텍스트를 쥐고, 그 언어의 서버 인스턴스마다(없으면 띄운다 — 지연 기동) `didOpen` 을 보낸다. 그 프로젝트
    /// 창의 연결만 부를 수 있다(다른 창 · 웰컴 · 설정 · 파일 창은 permission denied — 서버를 띄우지 않는다). 미신뢰 프로젝트는 띄우지
    /// 않고 인스턴스 상태 `untrusted` 로 답한다. 편집 등급 밖 · 서버 없는 언어는 `off`. 경로가 프로젝트 루트 밖이면 invalid params.
    pub LspDocumentOpen: "lsp.document.open" {
        params: LspDocumentOpenParams,
        result: LspDocumentOpened,
        tier: Exec,
        roles: [Owner],
    }

    /// 문서에 편집을 적용하고 떠 있는 서버에 `didChange` 를 보낸다(서버가 `Incremental` 이면 증분, 아니면 전체). 서버를 띄우지
    /// 않는다. 없는 문서 · 다른 연결의 문서는 not found, 지난 버전 · 빠진 편집은 conflict(`data.version` 에 지금 버전 — 다시 연다),
    /// 범위가 텍스트 밖이면 invalid params(아무것도 바꾸지 않는다).
    pub LspDocumentChange: "lsp.document.change" {
        params: LspDocumentChangeParams,
        result: LspDocumentChangeResult,
        tier: Read,
        roles: [Owner],
    }

    /// 저장을 알린다(`didSave` — 서버가 원하면 텍스트 포함). 없는 문서는 not found.
    pub LspDocumentSave: "lsp.document.save" {
        params: LspDocumentRef,
        result: (),
        tier: Read,
        roles: [Owner],
    }

    /// 문서를 닫는다(`didClose`). 인스턴스의 마지막 문서면 유휴 시한(`[lsp] idle_shutdown_minutes`, 기본 10분) 뒤 서버를 끝낸다.
    /// 없는 id 도 성공이다.
    pub LspDocumentClose: "lsp.document.close" {
        params: LspDocumentRef,
        result: (),
        tier: Read,
        roles: [Owner],
    }

    /// 그 프로젝트의 서버 인스턴스 상태. 바뀔 때마다 `lsp.status` 이벤트로도 나간다. 그 프로젝트 창의 연결만(다른 연결은
    /// permission denied).
    pub LspStatusGet: "lsp.status.get" {
        params: LspStatusGetParams,
        result: LspStatus,
        tier: Read,
        roles: [Owner],
    }

    /// 인스턴스를 다시 띄운다(떠 있으면 끝내고). 죽음 기록을 비운다 — `crashed` · `stopped` 에서 다시 띄우는 길이다. 허가를 다시
    /// 받는다(미신뢰면 `untrusted`). 그 프로젝트 창의 연결만. 없는 인스턴스는 not found.
    pub LspServerRestart: "lsp.server.restart" {
        params: LspServerControlParams,
        result: LspInstanceStatus,
        tier: Exec,
        roles: [Owner],
    }

    /// 인스턴스를 끝내고 `stopped` 로 둔다 — 문서를 다시 열어도 띄우지 않는다(`lsp.server.restart` 로 다시). 그 프로젝트 창의
    /// 연결만. 없는 인스턴스는 not found.
    pub LspServerStop: "lsp.server.stop" {
        params: LspServerControlParams,
        result: LspInstanceStatus,
        tier: Exec,
        roles: [Owner],
    }
}

/// 핸들러를 붙인다(`crate::register` 가 부른다).
pub fn register(router: &mut Router, manager: &LspManager) {
    let m = manager.clone();
    router.unary(LspDocumentOpen, move |ctx, params| {
        let m = m.clone();
        async move { m.open(&ctx, params).await }
    });
    let m = manager.clone();
    router.unary(LspDocumentChange, move |ctx, params| {
        let m = m.clone();
        async move { m.change(&ctx, params) }
    });
    let m = manager.clone();
    router.unary(LspDocumentSave, move |ctx, params: LspDocumentRef| {
        let m = m.clone();
        async move { m.save(&ctx, &params.doc_id) }
    });
    let m = manager.clone();
    router.unary(LspDocumentClose, move |ctx, params: LspDocumentRef| {
        let m = m.clone();
        async move {
            m.close(&ctx, &params.doc_id);
            Ok(())
        }
    });
    let m = manager.clone();
    router.unary(LspStatusGet, move |ctx, params: LspStatusGetParams| {
        let m = m.clone();
        async move {
            // 그 프로젝트 창의 연결만 읽는다 — 상태에는 작업 공간 루트(실제 경로)가 든다. 이벤트 `lsp.status` 도 프로젝트 범위로만
            // 나간다(M4-05 리뷰 L2). 미신뢰여도 읽는다(`untrusted` 를 보여야 한다).
            exec_grant(ctx.scope(), &params.project_id, &Launch::Process)?;
            Ok(m.status(&params.project_id))
        }
    });
    let m = manager.clone();
    router.unary(LspServerRestart, move |ctx, params| {
        let m = m.clone();
        async move { m.restart(&ctx, params).await }
    });
    let m = Arc::new(manager.clone());
    router.unary(LspServerStop, move |ctx, params| {
        let m = Arc::clone(&m);
        async move { m.stop(&ctx, params).await }
    });
}

pw_rpc::rpc_type!(LspStatus);

#[cfg(test)]
mod tests {
    use pw_rpc::{MethodInfo, Tier};

    /// 완료 기준 ⑪: 서버를 띄울 수 있는 메서드는 tier exec, 보내기 · 읽기만 하는 메서드는 tier read. 어느 것도 MCP 툴 · 파일 창
    /// 메서드가 아니다(결정 M4-D21).
    #[test]
    fn the_launching_methods_are_declared_exec() {
        let tiers: Vec<(&str, Tier, bool, bool)> = pw_rpc::methods()
            .into_iter()
            .map(|m: &MethodInfo| &m.meta)
            .filter(|m| m.name.starts_with("lsp.") && m.name != "lsp.servers.list")
            .map(|m| (m.name, m.tier, m.mcp, m.file_scope))
            .collect();
        assert_eq!(
            tiers,
            [
                ("lsp.applyEdit.answer", Tier::Write, false, false),
                ("lsp.codeAction.list", Tier::Read, false, false),
                ("lsp.codeAction.resolve", Tier::Read, false, false),
                ("lsp.command.execute", Tier::Write, false, false),
                ("lsp.completion", Tier::Read, false, false),
                ("lsp.completion.execute", Tier::Write, false, false),
                ("lsp.completion.resolve", Tier::Read, false, false),
                ("lsp.declaration", Tier::Read, false, false),
                ("lsp.definition", Tier::Read, false, false),
                ("lsp.diagnostics.counts", Tier::Read, false, false),
                ("lsp.diagnostics.list", Tier::Read, false, false),
                ("lsp.document.capabilities", Tier::Read, false, false),
                ("lsp.document.change", Tier::Read, false, false),
                ("lsp.document.close", Tier::Read, false, false),
                ("lsp.document.open", Tier::Exec, false, false),
                ("lsp.document.read", Tier::Read, false, false),
                ("lsp.document.save", Tier::Read, false, false),
                ("lsp.format", Tier::Read, false, false),
                ("lsp.hover", Tier::Read, false, false),
                ("lsp.implementation", Tier::Read, false, false),
                ("lsp.references", Tier::Read, false, false),
                ("lsp.rename", Tier::Read, false, false),
                ("lsp.rename.prepare", Tier::Read, false, false),
                ("lsp.server.log", Tier::Read, false, false),
                ("lsp.server.restart", Tier::Exec, false, false),
                ("lsp.server.setLocal", Tier::Write, false, false),
                ("lsp.server.stop", Tier::Exec, false, false),
                ("lsp.signatureHelp", Tier::Read, false, false),
                ("lsp.status.document", Tier::Read, false, false),
                ("lsp.status.get", Tier::Read, false, false),
                ("lsp.symbols.document", Tier::Read, false, false),
                ("lsp.typeDefinition", Tier::Read, false, false),
                ("lsp.workspaceSymbol", Tier::Read, false, false),
            ]
        );
    }
}
