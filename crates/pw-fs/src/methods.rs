//! 파일 시스템 RPC. UI 전용이라 MCP 툴이 아니다.
//!
//! 모든 요청은 프로젝트(`projectId`)와, 창이 활성 워크트리로 열려 있으면 그 꼭대기(`worktree`)를 함께 받는다. 경로는 그 프로젝트의
//! 루트와 활성 워크트리 안의 루트 안이어야 한다([`crate::scope`]). 오류 코드: 루트 밖 · 상대 경로 · 형식 오류는 invalid params,
//! 없는 대상은 `NOT_FOUND`, 기대와 다른 디스크 상태의 쓰기는 `CONFLICT`.

use std::sync::Arc;

use pw_rpc::{Router, RpcError, rpc_method};
use serde::{Deserialize, Serialize};

use crate::Fs;
use crate::judge::{Judgment, SizeClass};
use crate::list::{EntryKind, FsEntry};

/// 경로 하나를 받는 요청(`fs.list` · `fs.stat`).
#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct FsPathParams {
    pub project_id: String,
    /// 활성 워크트리의 꼭대기(`ActiveWorktree.top`, 절대 경로). 그 워크트리 안의 프로젝트 루트도 접근 범위가 된다.
    #[serde(default)]
    pub worktree: Option<String>,
    /// 절대 경로.
    pub path: String,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct FsList {
    /// 정규화한 실제 경로.
    pub path: String,
    /// 이름순.
    pub entries: Vec<FsEntry>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct FsStat {
    /// 정규화한 실제 경로(심링크를 모두 따라간 것). 문서 모델의 키다.
    pub path: String,
    /// 요청 경로 자리의 종류(심링크면 `symlink`).
    pub kind: EntryKind,
    /// 심링크면 가리키는 것의 종류. 깨진 링크 · 심링크가 아니면 `null`.
    pub target: Option<EntryKind>,
    /// 바이트(심링크면 대상의 것).
    #[specta(type = pw_rpc::JsNumber)]
    pub size: u64,
    #[specta(type = Option<pw_rpc::JsNumber>)]
    pub mtime_ms: Option<i64>,
    /// 쓰기 권한이 없다(`fs.write` 가 거절한다).
    pub readonly: bool,
    /// 파일이면 크기 등급(결정 M1-D9). 읽지 않고 정하므로 「열지 않음」 판단에 바이트를 읽지 않는다.
    pub size_class: Option<SizeClass>,
}

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct FsReadParams {
    pub project_id: String,
    #[serde(default)]
    pub worktree: Option<String>,
    /// 절대 경로.
    pub path: String,
    /// 범위 읽기의 시작 바이트. `offset` · `length` 가 모두 없으면 파일 전체를 읽는다(50 MiB 까지).
    #[serde(default)]
    #[specta(type = Option<pw_rpc::JsNumber>)]
    pub offset: Option<u64>,
    /// 범위 읽기의 최대 바이트(50 MiB 까지). 없으면 끝까지(최대 50 MiB).
    #[serde(default)]
    #[specta(type = Option<pw_rpc::JsNumber>)]
    pub length: Option<u64>,
    /// 텍스트여도 바이트(base64)로 받는다(hex 뷰어 · 이미지).
    #[serde(default)]
    pub bytes: bool,
}

/// 읽거나 쓸 내용.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum FsContent {
    /// UTF-8 텍스트. 읽기에서는 파일 앞의 UTF-8 BOM 을 뺀 것이다(`judgment.bom` 이 알린다).
    Text { text: String },
    /// 바이트 그대로(BOM 포함), 표준 base64.
    Base64 { data: String },
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct FsRead {
    /// 정규화한 실제 경로.
    pub path: String,
    /// 파일 전체 크기.
    #[specta(type = pw_rpc::JsNumber)]
    pub size: u64,
    #[specta(type = Option<pw_rpc::JsNumber>)]
    pub mtime_ms: Option<i64>,
    /// 읽은 범위의 시작.
    #[specta(type = pw_rpc::JsNumber)]
    pub offset: u64,
    /// 읽은 범위가 파일 끝까지 닿았다.
    pub eof: bool,
    /// 파일 전체를 읽었을 때만 있다. `fs.write` 의 `expectedVersion` 으로 그대로 돌려준다.
    pub version: Option<String>,
    pub content: FsContent,
    /// 판정. 텍스트/바이너리 · BOM 은 파일 앞부분으로, 인코딩 · 줄바꿈 · 긴 줄은 읽은 범위로 정한다.
    pub judgment: Judgment,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct FsWriteParams {
    pub project_id: String,
    #[serde(default)]
    pub worktree: Option<String>,
    /// 절대 경로. 심링크면 대상 파일을 바꾼다(정규화한 실제 경로가 루트 안이어야 한다). 폴더는 있어야 한다.
    pub path: String,
    pub content: FsContent,
    /// 텍스트 앞에 UTF-8 BOM 을 붙인다. 읽을 때 `judgment.bom` 이 `utf8` 이었으면 켠다. `base64` 내용에는 쓸 수 없다.
    #[serde(default)]
    pub bom: bool,
    /// 읽을 때 받은 `version`. 지금 파일과 다르면 쓰지 않고 `CONFLICT`. 없으면 새 파일을 만든다 — 파일이 이미 있으면 `CONFLICT`.
    #[serde(default)]
    pub expected_version: Option<String>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct FsWritten {
    /// 쓴 실제 경로.
    pub path: String,
    #[specta(type = pw_rpc::JsNumber)]
    pub size: u64,
    #[specta(type = Option<pw_rpc::JsNumber>)]
    pub mtime_ms: Option<i64>,
    /// 쓴 뒤의 `version`. 다음 쓰기의 `expectedVersion`.
    pub version: String,
}

rpc_method! {
    /// 폴더 한 단계의 항목(이름 · 종류 · 크기 · mtime · ignore 여부). 파일 트리가 펼칠 때 부른다.
    pub FsListMethod: "fs.list" {
        params: FsPathParams,
        result: FsList,
        tier: Read,
    }

    /// 파일 · 폴더 하나의 상태와 파일의 크기 등급. 내용은 읽지 않는다.
    pub FsStatMethod: "fs.stat" {
        params: FsPathParams,
        result: FsStat,
        tier: Read,
    }

    /// 파일 전체 또는 범위를 읽고 판정(텍스트/바이너리 · 인코딩 · 줄바꿈 · BOM · 크기 등급)을 함께 준다. 전체 읽기는 50 MiB 까지,
    /// 그보다 큰 파일은 범위로 읽는다.
    pub FsReadMethod: "fs.read" {
        params: FsReadParams,
        result: FsRead,
        tier: Read,
    }

    /// 같은 폴더의 임시 파일에 쓰고 rename 으로 바꿔 끼운다(권한 · 심링크 대상 보존). `expectedVersion` 이 지금 파일과 다르면 아무것도
    /// 바꾸지 않고 `CONFLICT`(`data.current` 에 지금 `version`, 파일이 없으면 `null`). 읽기 전용 파일은 거절한다.
    pub FsWriteMethod: "fs.write" {
        params: FsWriteParams,
        result: FsWritten,
        tier: Write,
    }
}

/// 핸들러를 붙인다. 디스크 IO 는 블로킹 스레드에서 한다.
pub fn register(router: &mut Router, fs: Arc<Fs>) {
    async fn blocking<T: Send + 'static>(
        f: impl FnOnce() -> Result<T, RpcError> + Send + 'static,
    ) -> Result<T, RpcError> {
        tokio::task::spawn_blocking(f)
            .await
            .map_err(|e| RpcError::internal(e.to_string()))?
    }
    let (list, stat, read) = (Arc::clone(&fs), Arc::clone(&fs), Arc::clone(&fs));
    crate::search::register(router, Arc::clone(&fs));
    router
        .unary(FsListMethod, move |_ctx, params: FsPathParams| {
            let s = Arc::clone(&list);
            blocking(move || s.list(&params))
        })
        .unary(FsStatMethod, move |_ctx, params: FsPathParams| {
            let s = Arc::clone(&stat);
            blocking(move || s.stat(&params))
        })
        .unary(FsReadMethod, move |_ctx, params: FsReadParams| {
            let s = Arc::clone(&read);
            blocking(move || s.read(&params))
        })
        .unary(FsWriteMethod, move |_ctx, params: FsWriteParams| {
            let s = Arc::clone(&fs);
            blocking(move || s.write(&params))
        });
}
