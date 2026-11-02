//! 작업 공간 심볼(15-m4-plan.md M4-16): `lsp.workspaceSymbol` — 심볼로 이동(`⌘⌥O`) · 클래스로 이동(`⌘O`) · Search Everywhere 의
//! 「심볼」 탭이 심볼 인덱스(M4-15 `index.symbols.search`)에 합치는 서버 쪽 결과.
//!
//! **물을 서버**: 그 창의 프로젝트(활성 워크트리) 아래 떠 있는 인스턴스 중 `workspace/symbol` 을 다루고, 설정에서 끄지 않았고, **이
//! 연결이 문서를 연** 것([`LspManager::session_instances`] — 서버를 띄우지 않는다). 문서 없이 인스턴스에 묻는 길
//! ([`LspManager::request_instance`])이라 허가 · 세대 표지를 부를 때마다 다시 본다. 미신뢰면 `lsp_grant` 가 거절한다(untrusted) — 그때는
//! 서버도 없다(결정 M4-D8). 웹뷰는 이 메서드의 실패를 조용히 넘기고 인덱스 결과만 보인다.
//!
//! **결과**는 서버의 정의 순서, 한 서버 안은 서버가 준 순서다(서버마다 자기 기준으로 고른다 — 다시 점수 매기지 않는다). 위치는
//! 내비게이션과 같은 꼴([`LspLocation`] — 절대 경로, 1 부터 센 줄 · UTF-16 열)이고, 그 창의 루트 밖(라이브러리 심볼 · `jdt://`)이면
//! 내비게이션의 토큰 표로 만든 **읽기 토큰**을 싣는다([`Navigation::place`] — 결정 M4-D13, 그 창 세션 · 인스턴스 · 세대 표지에 묶임).
//! 루트 안이면 루트 기준 상대 경로(`relative`)도 준다 — 웹뷰가 인덱스 결과(루트 기준 상대 경로를 준다)와 같은 (파일, 줄, 이름)을
//! 하나로 합친다(루트가 심링크를 지나도 같은 열쇠).
//!
//! **상한**(망가지거나 악의 있는 서버): 결과 [`MAX_LIMIT`] 개(기본 [`DEFAULT_LIMIT`] — 넘으면 `truncated`), 이름 · 컨테이너
//! [`MAX_TEXT`] 바이트(글자 경계에서 자른다), 질의 [`MAX_QUERY_CHARS`] 글자, 서버마다 [`SERVER_TIMEOUT`]. 서버가 준 이름은 글자
//! 그대로다 — 웹뷰가 글로만 그린다. 읽을 수 없는 항목(이름 · URI 가 없음)은 버린다. 종류는 LSP `SymbolKind` 숫자(1 ~ 26, 밖이면
//! 0)이고 `kinds` 로 거른다 — 심볼 인덱스의 종류와 맞추는 것은 웹뷰다(pw-lsp 는 pw-index 에 의존하지 않는다 — M4-10 과 같은 메인 판정).

use std::path::Path;
use std::time::Duration;

use pw_ext::LspFeature;
use pw_rpc::{Ctx, Router, RpcError, rpc_method};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::task::JoinSet;

use crate::manager::{LspManager, SessionInstance};
use crate::navigation::{LspLocation, Navigation, Placed};

/// 결과 수의 기본값.
pub const DEFAULT_LIMIT: u32 = 200;
/// `limit` 의 상한.
pub const MAX_LIMIT: u32 = 1_000;
/// 이름 · 컨테이너의 길이 상한(UTF-8 바이트). 심볼 인덱스의 이름 상한(M4-15 ⓕ)과 같다.
pub const MAX_TEXT: usize = 256;
/// 질의의 최대 길이(글자). 심볼 인덱스(`index.symbols.search`)와 같다.
pub const MAX_QUERY_CHARS: usize = 256;
/// 서버 하나의 시한. 넘으면 그 서버는 `failed` 에 들고 나머지 결과만 준다(웹뷰는 인덱스 결과를 먼저 보인다 — 200 ms 규칙).
pub const SERVER_TIMEOUT: Duration = Duration::from_secs(5);

const METHOD: &str = "workspace/symbol";

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct LspWorkspaceSymbolParams {
    pub project_id: String,
    /// 창의 활성 워크트리 꼭대기(`ActiveWorktree.top`). 없으면 메인 체크아웃.
    #[serde(default)]
    pub worktree: Option<String>,
    /// 찾을 것(서버에 그대로). 비어 있으면 묻지 않는다. 글자 수 상한 [`MAX_QUERY_CHARS`].
    pub query: String,
    /// 이 LSP `SymbolKind` 만(클래스로 이동 — 타입 종류). 없으면 모두.
    #[serde(default)]
    pub kinds: Option<Vec<u32>>,
    /// 결과 수. 기본 [`DEFAULT_LIMIT`], 1 ~ [`MAX_LIMIT`].
    #[serde(default)]
    pub limit: Option<u32>,
}

/// 서버가 준 심볼 하나.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct LspWorkspaceSymbol {
    /// 서버가 준 이름(글로만 그린다). [`MAX_TEXT`] 바이트까지.
    pub name: String,
    /// LSP `SymbolKind`(1 `File` ~ 26 `TypeParameter`). 범위 밖이면 0.
    pub kind: u32,
    /// 서버가 준 컨테이너 이름(흐린 글자로). 없으면 `None`.
    pub container: Option<String>,
    /// 답한 서버 id.
    pub server: String,
    /// 그 서버에 이 창이 연 문서의 언어(Linguist 이름 — 루트 밖 위치를 여는 외부 소스 탭의 언어).
    pub language: String,
    /// 위치(이름 자리 — 서버가 범위를 주지 않았으면 파일의 처음). 루트 밖이면 읽기 토큰과 함께.
    pub location: LspLocation,
    /// 루트 안이면 루트 기준 상대 경로(`/` 구분) — 인덱스 결과와 합칠 열쇠.
    pub relative: Option<String>,
}

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct LspWorkspaceSymbols {
    /// 서버의 정의 순서, 한 서버 안은 서버가 준 순서. `limit` 개까지.
    pub symbols: Vec<LspWorkspaceSymbol>,
    /// 답한 서버 id(정의 순서).
    pub servers: Vec<String>,
    /// 답하지 못한 서버 id(시한 · 오류 · 서버를 다시 띄움).
    pub failed: Vec<String>,
    /// 상한(`limit`)에 걸려 일부를 버렸다.
    pub truncated: bool,
}

rpc_method! {
    /// 프로젝트의 언어 서버들에 `workspace/symbol` 을 묻는다(심볼로 이동 · 클래스로 이동 · SE 「심볼」 탭 — M4-16). 이 창이 문서를 연
    /// 떠 있는 서버에만 묻고 서버를 띄우지 않는다. 루트 밖 위치는 읽기 토큰과 함께(결정 M4-D13). 미신뢰면 untrusted.
    pub LspWorkspaceSymbolMethod: "lsp.workspaceSymbol" {
        params: LspWorkspaceSymbolParams,
        result: LspWorkspaceSymbols,
        tier: Read,
        roles: [Owner],
    }
}

/// 서버 하나의 답.
struct Answer {
    order: usize,
    instance: SessionInstance,
    result: Result<Value, RpcError>,
}

/// `lsp.workspaceSymbol`.
pub async fn workspace_symbols(
    manager: &LspManager,
    nav: &Navigation,
    ctx: &Ctx,
    params: LspWorkspaceSymbolParams,
) -> Result<LspWorkspaceSymbols, RpcError> {
    if params.query.chars().count() > MAX_QUERY_CHARS {
        return Err(RpcError::invalid_params(format!(
            "the query is longer than {MAX_QUERY_CHARS} characters"
        )));
    }
    let limit = params.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT) as usize;
    let (root, instances) = manager.session_instances(
        ctx,
        &params.project_id,
        params.worktree.as_deref(),
        LspFeature::WorkspaceSymbol,
        METHOD,
    )?;
    if params.query.trim().is_empty() || instances.is_empty() {
        return Ok(LspWorkspaceSymbols::default());
    }
    let mut asks = JoinSet::new();
    for (order, instance) in instances.into_iter().enumerate() {
        let (manager, ctx, query) = (manager.clone(), ctx.clone(), params.query.clone());
        asks.spawn(async move {
            let result = manager
                .request_instance(
                    &ctx,
                    &instance.key,
                    Some(instance.epoch),
                    METHOD,
                    json!({ "query": query }),
                    Some(SERVER_TIMEOUT),
                )
                .await;
            Answer {
                order,
                instance,
                result,
            }
        });
    }
    let mut answers = Vec::new();
    while let Some(joined) = asks.join_next().await {
        if let Ok(answer) = joined {
            answers.push(answer);
        }
    }
    answers.sort_by_key(|a| a.order);
    let mut out = LspWorkspaceSymbols::default();
    for answer in answers {
        let server = answer.instance.key.server.clone();
        let result = match answer.result {
            Ok(result) => result,
            Err(e) => {
                tracing::debug!(target: "pw_lsp", server = %server, error = %e.message, "workspace/symbol failed");
                out.failed.push(server);
                continue;
            }
        };
        out.servers.push(server.clone());
        for item in result.as_array().map(Vec::as_slice).unwrap_or_default() {
            let Some(raw) = read_item(item) else {
                continue;
            };
            if params
                .kinds
                .as_ref()
                .is_some_and(|k| !k.contains(&raw.kind))
            {
                continue;
            }
            if out.symbols.len() == limit {
                out.truncated = true;
                break;
            }
            let Placed { real, path, token } = nav.place(
                ctx,
                (&answer.instance.key, answer.instance.epoch),
                &root,
                raw.uri,
            );
            let relative = token
                .is_none()
                .then(|| real.as_deref().and_then(|r| relative_to(&root, r)))
                .flatten();
            out.symbols.push(LspWorkspaceSymbol {
                name: clip(raw.name),
                kind: raw.kind,
                container: raw.container.map(clip),
                server: server.clone(),
                language: answer.instance.language.clone(),
                location: LspLocation {
                    path,
                    token,
                    line: raw.start.0 + 1,
                    column: raw.start.1 + 1,
                    end_line: raw.end.0 + 1,
                    end_column: raw.end.1 + 1,
                    preview: None,
                },
                relative,
            });
        }
    }
    Ok(out)
}

/// `SymbolInformation` · `WorkspaceSymbol` 하나에서 읽은 것.
struct RawSymbol<'a> {
    name: &'a str,
    kind: u32,
    container: Option<&'a str>,
    uri: &'a str,
    /// (줄, 열) — 0 부터, 열은 UTF-16.
    start: (u32, u32),
    end: (u32, u32),
}

fn read_item(item: &Value) -> Option<RawSymbol<'_>> {
    let name = item.get("name")?.as_str()?;
    let location = item.get("location")?;
    let uri = location.get("uri")?.as_str()?;
    let kind = item
        .get("kind")
        .and_then(Value::as_u64)
        .and_then(|k| u32::try_from(k).ok())
        .filter(|k| (1..=26).contains(k))
        .unwrap_or(0);
    let container = item
        .get("containerName")
        .and_then(Value::as_str)
        .filter(|c| !c.is_empty());
    // `WorkspaceSymbol` 은 `location` 에 범위가 없을 수 있다(클라이언트가 resolve 를 알린 때만 — 우리는 알리지 않지만 받아 둔다).
    let point = |key: &str| -> (u32, u32) {
        location
            .get("range")
            .and_then(|r| r.get(key))
            .and_then(|p| {
                Some((
                    u32::try_from(p.get("line")?.as_u64()?).ok()?,
                    u32::try_from(p.get("character")?.as_u64()?).ok()?,
                ))
            })
            .unwrap_or((0, 0))
    };
    Some(RawSymbol {
        name,
        kind,
        container,
        uri,
        start: point("start"),
        end: point("end"),
    })
}

/// [`MAX_TEXT`] 바이트까지(글자 경계).
fn clip(text: &str) -> String {
    if text.len() <= MAX_TEXT {
        return text.to_owned();
    }
    let mut cut = MAX_TEXT;
    while !text.is_char_boundary(cut) {
        cut -= 1;
    }
    text[..cut].to_owned()
}

/// `root` 아래면 상대 경로(`/` 구분).
fn relative_to(root: &Path, path: &Path) -> Option<String> {
    let rest = path.strip_prefix(root).ok()?;
    Some(
        rest.components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/"),
    )
}

/// 핸들러를 붙인다(`crate::register` 가 부른다). 루트 밖 위치의 토큰은 내비게이션의 표(`nav`)로 만든다.
pub fn register(router: &mut Router, manager: &LspManager, nav: &Navigation) {
    let (m, n) = (manager.clone(), nav.clone());
    router.unary(LspWorkspaceSymbolMethod, move |ctx, params| {
        let (m, n) = (m.clone(), n.clone());
        async move { workspace_symbols(&m, &n, &ctx, params).await }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_clipped_on_a_character_boundary() {
        let long = "가".repeat(200);
        let cut = clip(&long);
        assert!(cut.len() <= MAX_TEXT);
        assert_eq!(cut, "가".repeat(MAX_TEXT / 3));
        assert_eq!(clip("short"), "short");
    }

    #[test]
    fn items_without_a_name_or_uri_are_dropped_and_odd_kinds_become_zero() {
        assert!(read_item(&json!({ "location": { "uri": "file:///a" } })).is_none());
        assert!(read_item(&json!({ "name": "x", "location": {} })).is_none());
        let item = json!({ "name": "x", "kind": 99, "containerName": "", "location": { "uri": "file:///a" } });
        let raw = read_item(&item).unwrap();
        assert_eq!((raw.kind, raw.container, raw.start), (0, None, (0, 0)));
    }
}
