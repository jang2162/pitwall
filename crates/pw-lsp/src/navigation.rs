//! 내비게이션 기능(15-m4-plan.md M4-08): hover · 정의 · 선언 · 타입 정의 · 구현 · 사용처, 루트 밖 문서의 읽기 토큰(결정 M4-D13),
//! 서버의 `window/showDocument` 처리기.
//!
//! **요청** — 문서(`docId`)의 위치는 관리자의 요청 길([`LspManager::request`] — 결정 M4-D12 의 서버 고르기 · `staleVersion`)로 묻는다.
//! 루트 밖 문서(`token`) 안의 위치는 그 토큰을 만든 인스턴스에 직접 묻는다([`LspManager::request_instance`]) — 「그 문서 안의 hover ·
//! 정의는 같은 서버로」. 위치는 웹뷰의 UTF-16 오프셋이다.
//!
//! **결과 위치**는 절대 경로와 1 부터 센 줄 · 열(열은 UTF-16 — 에디터와 같다)이다. 그 창의 루트(`lsp_grant` 의 루트 — 프로젝트 또는
//! 워크트리) 안의 실제 경로면 경로만, 루트 밖(`~/.cargo/registry` · rustup 표준 라이브러리 · JDK)이나 파일이 아닌 URI(`jdt://` ·
//! 가상 문서)면 **읽기 토큰**을 함께 싣는다. 줄 미리보기(`preview`)는 Host 가 그 파일을 한 번씩 읽어 단다(편집 등급 상한 안의 일반
//! 파일만 — 위치 수 · 파일 수 상한).
//!
//! **읽기 토큰**(결정 M4-D13 · 메인 판정 2026-11-02): 서버가 이 연결(창 세션)의 응답에서 돌려준 루트 밖 URI 만 토큰이 된다 — 토큰은
//! (연결, 인스턴스, URI)에 묶이고, 그 연결이 닫히면 지워진다. 읽기(`lsp.document.read`)와 토큰 문서 안의 요청은 부를 때마다
//! 인스턴스의 허가를 다시 받는다(`LspManager::instance_grant` — 그 프로젝트 창의 연결 · 지금 신뢰 · 인스턴스가 떠 있음 · 그 연결이
//! 그 인스턴스에 문서를 연 적이 있음). 그래서 다른 창 · 다른 서버 · 신뢰 해제 뒤의 토큰은 읽지 못한다. 파일은 실제 경로를 다시 풀어
//! 토큰을 만들 때와 같은지 보고, 막히지 않게 연 핸들이 일반 파일일 때만(FIFO · 장치 · 폴더 거절) [`READ_LIMIT`] 까지 읽는다.
//! `jdt://` 는 디스크로 풀지 않고 그 인스턴스의 `java/classFileContents` 에만 묻는다. 그 밖의 스킴은 읽지 않는다. 쓰기 RPC 는 없다.
//! `fs.*` 의 루트 판정(결정 M1-D8)은 그대로다 — 토큰은 이 모듈의 RPC 로만 읽힌다.
//!
//! **`window/showDocument`**: `file://` 이고 실제 경로가 인스턴스의 작업 공간 루트 안이며 프로젝트를 지금 신뢰할 때만 받아
//! (`success: true`) 그 프로젝트 창에 [`TOPIC_SHOW_DOCUMENT`] 이벤트를 낸다. `external: true` · 다른 스킴 · 루트 밖(`..` · 링크로
//! 벗어난 것 포함) · 미신뢰는 `success: false` 이고 아무것도 열지 않는다(브라우저 · 외부 앱도) — 응답에 없던 루트 밖 경로를 서버가
//! 열게 하지 못한다.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::hash::{BuildHasher as _, Hasher as _};
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use lsp_types::Position;
use pw_core::bus::Scope;
use pw_ext::LspFeature;
use pw_rpc::{Ctx, Router, RpcError, SessionId, code as rpc_code, rpc_method};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::document::DocText;
use crate::grant::real_path;
use crate::handlers::HandlerContext;
use crate::manager::{FeatureRequest, InstanceKey, LspManager};
use crate::message::ResponseError;
use crate::server::Handlers;
use crate::uri::uri_path;

/// 서버가 루트 안 문서를 열라고 했다(`window/showDocument`). 프로젝트 범위 — 내용은 [`LspShowDocument`].
pub const TOPIC_SHOW_DOCUMENT: &str = "lsp.showDocument";
/// 토큰 문서를 읽는 상한(편집 등급 — 결정 M1-D9 의 5 MiB).
pub const READ_LIMIT: u64 = pw_fs::limits::EDITABLE_MAX;
/// 한 답에 싣는 위치의 상한. 넘으면 `truncated`.
pub const LOCATIONS_LIMIT: usize = 2000;
/// 줄 미리보기를 읽는 파일 수의 상한(앞에서부터).
pub const PREVIEW_FILES_LIMIT: usize = 200;
/// 줄 미리보기 한 줄의 상한(UTF-16 이 아니라 글자 수).
pub const PREVIEW_CHARS: usize = 240;
/// hover 본문의 상한(바이트). 넘으면 잘라 낸다.
pub const HOVER_LIMIT: usize = 64 * 1024;

// ---- RPC 꼴 -------------------------------------------------------------------------------------------------------------------

/// 위치 하나. 열린 문서(`docId`)나 루트 밖 문서(`token`) 중 하나를 준다.
#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct LspPositionParams {
    /// `lsp.document.open` 의 문서.
    #[serde(default)]
    pub doc_id: Option<String>,
    /// 루트 밖 문서의 읽기 토큰(이 연결이 받은 것).
    #[serde(default)]
    pub token: Option<String>,
    /// 웹뷰가 위치를 계산한 문서 버전(`docId` 일 때). 다르면 `staleVersion`.
    #[serde(default)]
    pub version: Option<u32>,
    /// UTF-16 오프셋.
    pub offset: u32,
}

/// `lsp.references` 의 매개변수.
#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct LspReferencesParams {
    #[serde(default)]
    pub doc_id: Option<String>,
    #[serde(default)]
    pub token: Option<String>,
    #[serde(default)]
    pub version: Option<u32>,
    pub offset: u32,
    /// 선언 자리도 넣는가(LSP `context.includeDeclaration`).
    pub include_declaration: bool,
}

/// hover 본문의 꼴.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum LspMarkupKind {
    Markdown,
    Plaintext,
}

/// hover 본문. 서버가 보낸 글 그대로다 — 웹뷰는 HTML 을 끈 렌더로만 그린다(서버의 글을 믿지 않는다).
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct LspHover {
    pub kind: LspMarkupKind,
    pub value: String,
    /// 서버가 준 범위(UTF-16 오프셋). 없으면 웹뷰가 단어로 잡는다.
    pub from: Option<u32>,
    pub to: Option<u32>,
}

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct LspHoverResult {
    /// 요청 사이에 문서가 바뀌었다 — 버린다.
    pub stale_version: bool,
    pub hover: Option<LspHover>,
}

/// 결과 위치 하나.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct LspLocation {
    /// 절대 경로(루트 안이면 정규화한 실제 경로). 파일이 아닌 URI 면 URI 그대로.
    pub path: String,
    /// 루트 밖이거나 파일이 아니면 읽기 토큰 — `lsp.document.read` 와 토큰 문서 안의 요청에 쓴다.
    pub token: Option<String>,
    /// 1 부터.
    pub line: u32,
    /// 1 부터, UTF-16.
    pub column: u32,
    pub end_line: u32,
    pub end_column: u32,
    /// 그 줄의 글(앞뒤 공백 그대로, [`PREVIEW_CHARS`] 까지). 읽지 못했으면 없다.
    pub preview: Option<String>,
}

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct LspLocationsResult {
    pub stale_version: bool,
    /// 답한 서버 id.
    pub server: Option<String>,
    pub locations: Vec<LspLocation>,
    /// [`LOCATIONS_LIMIT`] 를 넘어 잘랐다.
    pub truncated: bool,
}

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct LspDocumentReadParams {
    pub token: String,
}

/// 루트 밖 문서(읽기 전용).
#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct LspDocumentReadResult {
    pub text: String,
    pub uri: String,
    /// 보일 경로(파일이면 절대 경로, 아니면 URI).
    pub path: String,
    /// 이 위치를 돌려준 서버 id(탭 머리의 「출처」).
    pub server: String,
}

/// [`TOPIC_SHOW_DOCUMENT`] 의 내용.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct LspShowDocument {
    /// 정규화한 실제 경로(루트 안).
    pub path: String,
    pub worktree: Option<String>,
    /// 1 부터. 없으면 파일만 연다.
    pub line: Option<u32>,
    pub column: Option<u32>,
    pub take_focus: bool,
    pub server: String,
}

// 이벤트의 내용도 TS 로 내보낸다(웹뷰가 구독한다).
pw_rpc::rpc_type!(LspShowDocument);

rpc_method! {
    /// 위치의 hover(결정 M4-D12 의 서버). 문서 또는 루트 밖 문서의 토큰.
    pub LspHoverMethod: "lsp.hover" {
        params: LspPositionParams,
        result: LspHoverResult,
        tier: Read,
        roles: [Owner],
    }
}

rpc_method! {
    /// 정의로(`textDocument/definition`). 위치는 절대 경로, 루트 밖이면 읽기 토큰과 함께.
    pub LspDefinitionMethod: "lsp.definition" {
        params: LspPositionParams,
        result: LspLocationsResult,
        tier: Read,
        roles: [Owner],
    }
}

rpc_method! {
    /// 선언으로(`textDocument/declaration`).
    pub LspDeclarationMethod: "lsp.declaration" {
        params: LspPositionParams,
        result: LspLocationsResult,
        tier: Read,
        roles: [Owner],
    }
}

rpc_method! {
    /// 타입 정의로(`textDocument/typeDefinition`).
    pub LspTypeDefinitionMethod: "lsp.typeDefinition" {
        params: LspPositionParams,
        result: LspLocationsResult,
        tier: Read,
        roles: [Owner],
    }
}

rpc_method! {
    /// 구현으로(`textDocument/implementation`).
    pub LspImplementationMethod: "lsp.implementation" {
        params: LspPositionParams,
        result: LspLocationsResult,
        tier: Read,
        roles: [Owner],
    }
}

rpc_method! {
    /// 사용처(`textDocument/references`).
    pub LspReferencesMethod: "lsp.references" {
        params: LspReferencesParams,
        result: LspLocationsResult,
        tier: Read,
        roles: [Owner],
    }
}

rpc_method! {
    /// 루트 밖 문서를 읽기 토큰으로 읽는다(결정 M4-D13 — 이 연결이 받은 토큰만, 읽기 전용). 원격 role 로는 열지 않는다.
    pub LspDocumentReadMethod: "lsp.document.read" {
        params: LspDocumentReadParams,
        result: LspDocumentReadResult,
        tier: Read,
        roles: [Owner],
    }
}

// ---- 읽기 토큰 -----------------------------------------------------------------------------------------------------------------

#[derive(Clone)]
struct TokenEntry {
    session: SessionId,
    uri: String,
    /// 파일이면 토큰을 만들 때 푼 실제 경로(없는 파일이면 `None` — 읽기가 거절한다).
    path: Option<PathBuf>,
    instance: InstanceKey,
    /// 한 번 읽은 글(바뀌지 않는 문서로 다룬다 — 탭이 연 때의 글).
    text: Option<Arc<DocText>>,
}

#[derive(Default)]
struct Tokens {
    by_token: HashMap<String, TokenEntry>,
    /// (연결, 인스턴스, URI) → 토큰. 같은 위치를 다시 받으면 같은 토큰이다.
    by_place: HashMap<(SessionId, InstanceKey, String), String>,
    /// 닫힘을 지켜보는 연결.
    watched: HashSet<SessionId>,
}

impl Tokens {
    fn forget(&mut self, session: SessionId) {
        self.by_token.retain(|_, e| e.session != session);
        self.by_place.retain(|(s, _, _), _| *s != session);
        self.watched.remove(&session);
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// 토큰 글 하나(128비트). 연결마다 묶이므로 맞혀도 다른 연결에서는 쓸 수 없다 — 글은 프로세스마다 OS 난수로 씨를 받은 해시로 만든다.
fn new_token() -> String {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let state = std::collections::hash_map::RandomState::new();
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let mut a = state.build_hasher();
    a.write_u64(n);
    let mut b = state.build_hasher();
    b.write_u64(!n);
    format!("{:016x}{:016x}", a.finish(), b.finish())
}

/// 내비게이션 기능의 Host 쪽. 복제는 같은 토큰 표를 본다.
#[derive(Clone)]
pub struct Navigation {
    manager: LspManager,
    tokens: Arc<Mutex<Tokens>>,
}

/// 요청 하나의 답과 그 문서.
struct Answered {
    result: Value,
    server: String,
    instance: InstanceKey,
    stale: bool,
    uri: String,
    text: Arc<DocText>,
}

impl Navigation {
    pub fn new(manager: LspManager) -> Self {
        Self {
            manager,
            tokens: Arc::default(),
        }
    }

    /// 루트 밖 URI 의 토큰(이 연결 · 인스턴스에 묶는다). 처음 받은 연결이면 닫힘을 지켜본다.
    fn mint(&self, ctx: &Ctx, instance: &InstanceKey, uri: &str, path: Option<PathBuf>) -> String {
        let session = ctx.session();
        let mut tokens = lock(&self.tokens);
        let place = (session, instance.clone(), uri.to_owned());
        if let Some(token) = tokens.by_place.get(&place) {
            return token.clone();
        }
        let token = new_token();
        tokens.by_token.insert(
            token.clone(),
            TokenEntry {
                session,
                uri: uri.to_owned(),
                path,
                instance: instance.clone(),
                text: None,
            },
        );
        tokens.by_place.insert(place, token.clone());
        if tokens.watched.insert(session) {
            let closed = ctx.connection_closed();
            let table = Arc::clone(&self.tokens);
            tokio::spawn(async move {
                closed.cancelled().await;
                lock(&table).forget(session);
            });
        }
        token
    }

    /// 이 연결의 토큰. 없거나 다른 연결의 것이면 not found(있는지도 알리지 않는다).
    fn entry(&self, ctx: &Ctx, token: &str) -> Result<TokenEntry, RpcError> {
        lock(&self.tokens)
            .by_token
            .get(token)
            .filter(|e| e.session == ctx.session())
            .cloned()
            .ok_or_else(|| RpcError::new(rpc_code::NOT_FOUND, "no such document token"))
    }

    /// 토큰 문서의 글. 부를 때마다 허가를 다시 받는다.
    async fn text_of(
        &self,
        ctx: &Ctx,
        token: &str,
    ) -> Result<(TokenEntry, Arc<DocText>), RpcError> {
        let entry = self.entry(ctx, token)?;
        drop(self.manager.instance_grant(ctx, &entry.instance)?);
        if let Some(text) = &entry.text {
            return Ok((entry.clone(), Arc::clone(text)));
        }
        let text = if entry.uri.starts_with("file:") {
            let (uri, minted) = (entry.uri.clone(), entry.path.clone());
            tokio::task::spawn_blocking(move || read_file(&uri, minted.as_deref()))
                .await
                .map_err(|e| RpcError::internal(e.to_string()))??
        } else if entry.uri.starts_with("jdt:") {
            let answer = self
                .manager
                .request_instance(
                    ctx,
                    &entry.instance,
                    "java/classFileContents",
                    json!({ "uri": entry.uri }),
                    None,
                )
                .await?;
            let text = answer.as_str().unwrap_or_default();
            if text.len() as u64 > READ_LIMIT {
                return Err(too_large(&entry.uri));
            }
            text.to_owned()
        } else {
            return Err(RpcError::invalid_params(format!(
                "documents like {} cannot be read",
                entry.uri.split(':').next().unwrap_or_default()
            )));
        };
        let text = Arc::new(DocText::new(text));
        if let Some(e) = lock(&self.tokens).by_token.get_mut(token) {
            e.text = Some(Arc::clone(&text));
        }
        Ok((entry, text))
    }

    /// `lsp.document.read`.
    pub async fn read(&self, ctx: &Ctx, token: &str) -> Result<LspDocumentReadResult, RpcError> {
        let (entry, text) = self.text_of(ctx, token).await?;
        Ok(LspDocumentReadResult {
            text: text.as_str().to_owned(),
            path: display_path(&entry.uri),
            uri: entry.uri,
            server: entry.instance.server.clone(),
        })
    }

    /// 위치 하나로 묻는다: 문서면 요청 길, 토큰이면 그 인스턴스.
    async fn ask(
        &self,
        ctx: &Ctx,
        at: (&Option<String>, &Option<String>, Option<u32>, u32),
        feature: LspFeature,
        method: &str,
        extra: Option<Value>,
    ) -> Result<Answered, RpcError> {
        let (doc_id, token, version, offset) = at;
        let merge = |mut params: Value| {
            if let (Some(extra), Value::Object(map)) = (&extra, &mut params)
                && let Value::Object(more) = extra
            {
                map.extend(more.clone());
            }
            params
        };
        match (doc_id, token) {
            (Some(doc_id), None) => {
                let answer = self
                    .manager
                    .request(
                        ctx,
                        FeatureRequest {
                            doc_id,
                            feature,
                            method,
                            expected_version: version,
                            timeout: None,
                        },
                        |view| {
                            Ok(merge(json!({
                                "textDocument": view.text_document(),
                                "position": view.position(offset)?,
                            })))
                        },
                    )
                    .await?;
                Ok(Answered {
                    result: answer.result,
                    server: answer.server,
                    instance: answer.key,
                    stale: answer.stale_version,
                    uri: answer.doc.uri,
                    text: answer.doc.text,
                })
            }
            (None, Some(token)) => {
                let (entry, text) = self.text_of(ctx, token).await?;
                let position = text.position(offset as usize).ok_or_else(|| {
                    RpcError::invalid_params(format!("offset {offset} is outside the document"))
                })?;
                let params = merge(json!({
                    "textDocument": { "uri": entry.uri },
                    "position": position,
                }));
                let result = self
                    .manager
                    .request_instance(ctx, &entry.instance, method, params, None)
                    .await?;
                Ok(Answered {
                    result,
                    server: entry.instance.server.clone(),
                    instance: entry.instance,
                    stale: false,
                    uri: entry.uri,
                    text,
                })
            }
            _ => Err(RpcError::invalid_params(
                "give exactly one of docId and token",
            )),
        }
    }

    /// `lsp.hover`.
    pub async fn hover(
        &self,
        ctx: &Ctx,
        params: &LspPositionParams,
    ) -> Result<LspHoverResult, RpcError> {
        let answer = self
            .ask(
                ctx,
                (&params.doc_id, &params.token, params.version, params.offset),
                LspFeature::Hover,
                "textDocument/hover",
                None,
            )
            .await?;
        if answer.stale {
            return Ok(LspHoverResult {
                stale_version: true,
                hover: None,
            });
        }
        Ok(LspHoverResult {
            stale_version: false,
            hover: hover_of(&answer.result, &answer.text),
        })
    }

    /// 위치 목록을 돌려주는 요청(정의 · 선언 · 타입 정의 · 구현 · 사용처).
    pub async fn locations(
        &self,
        ctx: &Ctx,
        at: (&Option<String>, &Option<String>, Option<u32>, u32),
        feature: LspFeature,
        method: &str,
        extra: Option<Value>,
    ) -> Result<LspLocationsResult, RpcError> {
        let answer = self.ask(ctx, at, feature, method, extra).await?;
        if answer.stale {
            return Ok(LspLocationsResult {
                stale_version: true,
                server: Some(answer.server),
                ..LspLocationsResult::default()
            });
        }
        let grant = self.manager.instance_grant(ctx, &answer.instance)?;
        let root = grant.root().to_path_buf();
        drop(grant);
        let mut raw = raw_locations(&answer.result);
        let truncated = raw.len() > LOCATIONS_LIMIT;
        raw.truncate(LOCATIONS_LIMIT);
        let mut locations = Vec::with_capacity(raw.len());
        // 미리보기를 읽을 파일(경로 → 줄 번호들). 요청한 문서 자신은 Host 의 글을 쓴다.
        let mut wanted: BTreeMap<PathBuf, Vec<usize>> = BTreeMap::new();
        for (uri, start, end) in raw {
            let at_file = uri.starts_with("file:").then(|| uri_path(&uri)).flatten();
            let real = at_file.as_deref().and_then(real_path);
            let inside = real.as_ref().is_some_and(|r| r.starts_with(&root));
            let (path, token) = match (&real, inside) {
                (Some(real), true) => (real.display().to_string(), None),
                _ => (
                    at_file
                        .as_ref()
                        .map_or_else(|| uri.clone(), |p| p.display().to_string()),
                    Some(self.mint(ctx, &answer.instance, &uri, real.clone())),
                ),
            };
            let preview = if uri == answer.uri {
                line_text(answer.text.as_str(), start.line as usize)
            } else {
                if let Some(real) = &real
                    && (wanted.len() < PREVIEW_FILES_LIMIT || wanted.contains_key(real))
                {
                    wanted
                        .entry(real.clone())
                        .or_default()
                        .push(start.line as usize);
                }
                None
            };
            locations.push((
                real,
                LspLocation {
                    path,
                    token,
                    line: start.line + 1,
                    column: start.character + 1,
                    end_line: end.line + 1,
                    end_column: end.character + 1,
                    preview,
                },
            ));
        }
        let previews = tokio::task::spawn_blocking(move || read_previews(wanted))
            .await
            .unwrap_or_default();
        Ok(LspLocationsResult {
            stale_version: false,
            server: Some(answer.server),
            truncated,
            locations: locations
                .into_iter()
                .map(|(real, mut loc)| {
                    if loc.preview.is_none()
                        && let Some(real) = real
                    {
                        loc.preview = previews.get(&(real, loc.line as usize - 1)).cloned();
                    }
                    loc
                })
                .collect(),
        })
    }
}

fn too_large(uri: &str) -> RpcError {
    RpcError::invalid_params(format!("{uri} is larger than {READ_LIMIT} bytes"))
        .with_data(json!({ "reason": "tooLarge" }))
}

/// 보일 경로: 파일이면 절대 경로, 아니면 URI.
fn display_path(uri: &str) -> String {
    uri.starts_with("file:")
        .then(|| uri_path(uri))
        .flatten()
        .map_or_else(|| uri.to_owned(), |p| p.display().to_string())
}

/// 토큰의 파일을 읽는다: 실제 경로를 다시 풀어 토큰을 만들 때와 같은지 보고, 막히지 않게 연 핸들이 일반 파일이고 그 실제 경로가
/// 같을 때만(`pw_fs` 의 `open_inside` — FIFO · 장치 · 폴더 거절), 상한까지.
fn read_file(uri: &str, minted: Option<&Path>) -> Result<String, RpcError> {
    let gone = || RpcError::new(rpc_code::NOT_FOUND, format!("{uri} cannot be read"));
    let path = uri_path(uri).ok_or_else(gone)?;
    let real = real_path(&path).ok_or_else(gone)?;
    if minted != Some(real.as_path()) {
        return Err(RpcError::new(
            rpc_code::PERMISSION_DENIED,
            format!("{uri} no longer points at the file the language server returned"),
        ));
    }
    let file = pw_fs::scope::Roots::file(real.clone())
        .open_inside(&real)
        .map_err(|e| RpcError::invalid_params(format!("{uri} cannot be read: {e}")))?;
    let mut bytes = Vec::new();
    file.take(READ_LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| RpcError::invalid_params(format!("{uri} cannot be read: {e}")))?;
    if bytes.len() as u64 > READ_LIMIT {
        return Err(too_large(uri));
    }
    Ok(String::from_utf8(bytes)
        .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned()))
}

/// 미리보기 줄들((실제 경로, 0 부터 센 줄) → 글). 편집 등급 안의 일반 파일만 읽는다.
fn read_previews(wanted: BTreeMap<PathBuf, Vec<usize>>) -> HashMap<(PathBuf, usize), String> {
    let mut out = HashMap::new();
    for (path, lines) in wanted {
        let Ok(file) = pw_fs::scope::Roots::file(path.clone()).open_inside(&path) else {
            continue;
        };
        let mut bytes = Vec::new();
        if file.take(READ_LIMIT + 1).read_to_end(&mut bytes).is_err()
            || bytes.len() as u64 > READ_LIMIT
        {
            continue;
        }
        let text = String::from_utf8_lossy(&bytes);
        for line in lines {
            if let Some(t) = line_text(&text, line) {
                out.insert((path.clone(), line), t);
            }
        }
    }
    out
}

/// 0 부터 센 `line` 의 글(줄바꿈 제외 — LSP 의 `\r\n` · `\r` · `\n`), [`PREVIEW_CHARS`] 까지.
fn line_text(text: &str, line: usize) -> Option<String> {
    let mut rest = text;
    for _ in 0..line {
        let at = rest.find(['\n', '\r'])?;
        let skip = if rest[at..].starts_with("\r\n") { 2 } else { 1 };
        rest = &rest[at + skip..];
    }
    let end = rest.find(['\n', '\r']).unwrap_or(rest.len());
    Some(rest[..end].chars().take(PREVIEW_CHARS).collect())
}

fn position_of(value: &Value) -> Option<Position> {
    Some(Position {
        line: u32::try_from(value.get("line")?.as_u64()?).ok()?,
        character: u32::try_from(value.get("character")?.as_u64()?).ok()?,
    })
}

/// `Location | Location[] | LocationLink[] | null` → (URI, 시작, 끝). 링크는 `targetSelectionRange`(없으면 `targetRange`).
fn raw_locations(result: &Value) -> Vec<(String, Position, Position)> {
    let items: Vec<&Value> = match result {
        Value::Array(items) => items.iter().collect(),
        Value::Object(_) => vec![result],
        _ => Vec::new(),
    };
    items
        .into_iter()
        .filter_map(|item| {
            let (uri, range) = match item.get("targetUri") {
                Some(uri) => (
                    uri.as_str()?,
                    item.get("targetSelectionRange")
                        .or_else(|| item.get("targetRange"))?,
                ),
                None => (item.get("uri")?.as_str()?, item.get("range")?),
            };
            Some((
                uri.to_owned(),
                position_of(range.get("start")?)?,
                position_of(range.get("end")?)?,
            ))
        })
        .collect()
}

/// `MarkedString` 하나 → Markdown.
fn marked(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(s.clone()),
        Value::Object(o) => {
            let code = o.get("value")?.as_str()?;
            let language = o.get("language").and_then(Value::as_str).unwrap_or("");
            Some(format!("```{language}\n{code}\n```"))
        }
        _ => None,
    }
}

/// hover 답 → 본문. 비었으면 `None`.
fn hover_of(result: &Value, text: &DocText) -> Option<LspHover> {
    let contents = result.get("contents")?;
    let (kind, mut value) = match contents {
        Value::Object(o) if o.contains_key("kind") => (
            if o.get("kind").and_then(Value::as_str) == Some("plaintext") {
                LspMarkupKind::Plaintext
            } else {
                LspMarkupKind::Markdown
            },
            o.get("value")?.as_str()?.to_owned(),
        ),
        Value::Array(items) => (
            LspMarkupKind::Markdown,
            items
                .iter()
                .filter_map(marked)
                .collect::<Vec<_>>()
                .join("\n\n"),
        ),
        other => (LspMarkupKind::Markdown, marked(other)?),
    };
    if value.trim().is_empty() {
        return None;
    }
    if value.len() > HOVER_LIMIT {
        let mut cut = HOVER_LIMIT;
        while !value.is_char_boundary(cut) {
            cut -= 1;
        }
        value.truncate(cut);
    }
    let range = result.get("range");
    let offset = |key: &str| {
        range
            .and_then(|r| r.get(key))
            .and_then(position_of)
            .and_then(|p| u32::try_from(text.offset(p)).ok())
    };
    Some(LspHover {
        kind,
        value,
        from: offset("start"),
        to: offset("end"),
    })
}

// ---- window/showDocument -------------------------------------------------------------------------------------------------------

/// `window/showDocument` 를 받을지(머리 주석). 받으면 그 이벤트를 낸다.
fn show_document_answer(cx: &HandlerContext, params: &Value) -> bool {
    if params.get("external").and_then(Value::as_bool) == Some(true) {
        return false;
    }
    let Some(uri) = params.get("uri").and_then(Value::as_str) else {
        return false;
    };
    if !uri.starts_with("file:") {
        return false;
    }
    let Some(real) = uri_path(uri).as_deref().and_then(real_path) else {
        return false;
    };
    if !real.starts_with(&cx.root) {
        return false;
    }
    let (Some(bus), Some(projects)) = (&cx.bus, &cx.projects) else {
        return false;
    };
    if !projects.is_trusted(&cx.project_id) {
        return false;
    }
    let start = params
        .get("selection")
        .and_then(|s| s.get("start"))
        .and_then(position_of);
    let event = LspShowDocument {
        path: real.display().to_string(),
        worktree: cx.worktree.clone(),
        line: start.map(|p| p.line + 1),
        column: start.map(|p| p.character + 1),
        take_focus: params
            .get("takeFocus")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        server: cx.server.clone(),
    };
    bus.publish(
        Scope::Project(cx.project_id.clone()),
        TOPIC_SHOW_DOCUMENT,
        &event,
    )
    .is_ok()
}

/// `window/showDocument` 처리기(`crate::handlers` 의 등록부가 부른다).
pub(crate) fn show_document(handlers: &mut Handlers, cx: &HandlerContext) {
    let cx = cx.clone();
    handlers.on("window/showDocument", move |params: Value| {
        let cx = cx.clone();
        async move {
            let success = tokio::task::spawn_blocking(move || show_document_answer(&cx, &params))
                .await
                .unwrap_or(false);
            Ok::<_, ResponseError>(json!({ "success": success }))
        }
    });
}

// ---- 등록 ----------------------------------------------------------------------------------------------------------------------

/// 위치 하나로 위치 목록을 묻는 RPC 하나를 붙인다.
fn locations_method<M>(
    router: &mut Router,
    nav: &Navigation,
    method: M,
    feature: LspFeature,
    lsp: &'static str,
) where
    M: pw_rpc::UnaryMethod<Params = LspPositionParams, Result = LspLocationsResult>,
{
    let n = nav.clone();
    router.unary(method, move |ctx, p: LspPositionParams| {
        let n = n.clone();
        async move {
            n.locations(
                &ctx,
                (&p.doc_id, &p.token, p.version, p.offset),
                feature,
                lsp,
                None,
            )
            .await
        }
    });
}

/// 핸들러를 붙인다(`crate::register` 가 부른다).
pub fn register(router: &mut Router, manager: &LspManager) {
    let nav = Navigation::new(manager.clone());
    let n = nav.clone();
    router.unary(LspHoverMethod, move |ctx, p: LspPositionParams| {
        let n = n.clone();
        async move { n.hover(&ctx, &p).await }
    });
    locations_method(
        router,
        &nav,
        LspDefinitionMethod,
        LspFeature::Definition,
        "textDocument/definition",
    );
    locations_method(
        router,
        &nav,
        LspDeclarationMethod,
        LspFeature::Definition,
        "textDocument/declaration",
    );
    locations_method(
        router,
        &nav,
        LspTypeDefinitionMethod,
        LspFeature::Definition,
        "textDocument/typeDefinition",
    );
    locations_method(
        router,
        &nav,
        LspImplementationMethod,
        LspFeature::Implementation,
        "textDocument/implementation",
    );
    let n = nav.clone();
    router.unary(LspReferencesMethod, move |ctx, p: LspReferencesParams| {
        let n = n.clone();
        async move {
            n.locations(
                &ctx,
                (&p.doc_id, &p.token, p.version, p.offset),
                LspFeature::References,
                "textDocument/references",
                Some(json!({ "context": { "includeDeclaration": p.include_declaration } })),
            )
            .await
        }
    });
    let n = nav;
    router.unary(
        LspDocumentReadMethod,
        move |ctx, p: LspDocumentReadParams| {
            let n = n.clone();
            async move { n.read(&ctx, &p.token).await }
        },
    );
}
