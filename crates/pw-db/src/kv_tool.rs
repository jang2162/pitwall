//! Redis 의 MCP 툴 메서드(16-m5-plan.md M5-19, 결정 M5-D7, `docs/design/05-mcp.md` 「DB」 표). 그룹 `db`, 둘 다 `read`.
//!
//! - `db_kv_scan` ← `db.tool.kvScan`: 키 한 쪽(키마다 타입 · TTL · 크기)과 다음 `cursor`. 스트림 `db.kv.scan` 은 툴이 될 수 없어
//!   `SCAN` 커서를 한 쪽씩 도는 unary 다.
//! - `db_kv_get` ← `db.tool.kvGet`: 값 한 쪽(타입별). 키가 없으면 `missing`.
//!
//! **쪽 단위 키 스캔과 값 읽기만이고 명령 실행 툴은 없다** — 입력에 명령 칸이 없다. 키 이름 · `match` 패턴은 드라이버가 RESP 의
//! 인자(길이를 앞에 붙인 bulk string)로만 보낸다 — 줄바꿈 · 따옴표 · 공백이 든 키도 명령이 되지 못한다(`kv_live.rs` 의 MONITOR
//! 시험).
//!
//! - **관문**: `agent_grant` → M5-06 의 입구 [`db_grant_for_agent`](사람이 연 연결만 · 프로덕션 표지 — Redis 의 표지 열쇠는 DB
//!   번호를 뺀다, `DbTarget::mark_key`). `database` 입력(DB 번호)도 관문에 넘긴다. 관문을 따로 두지 않는다.
//! - **연결**: 세션 id 를 받지 않고 호출마다 자기 연결을 연다([`crate::connection::connect`] — 창의 세션 · 키 브라우저의 연결을
//!   빌리지 않는다). DB 번호마다 연결 하나다(결정 M3-D15 — `SELECT` 를 보내지 않는다). 응답 타임아웃은 `[db]
//!   redis_response_timeout`(연결을 풀 때 `crate::trust` 가 싣는다). 호출이 버려지면(취소 · 세션 끊김) 연결도 버려진다 — Redis
//!   드라이버는 그때 진행 중 요청과 연결을 버린다.
//! - **보내는 명령**: 드라이버의 [`KvConnection::scan`] · [`KvConnection::key_info`] · [`KvConnection::read`] 뿐이다 — `SCAN`
//!   (`COUNT` ≤ 1,000) · `TYPE` · `PTTL` · `MEMORY USAGE` · 타입별 길이 · `EXISTS` · `GETRANGE` · `HSCAN` · `SSCAN` · `LRANGE` ·
//!   `ZRANGE` · `XRANGE` · `JSON.GET`. 모두 분류기(`pw_db_sql::kv_command`)가 확인 없는 조회(`Allowed`)로 매긴다(`kv_live.rs` 가
//!   서버의 MONITOR 로 본다). 연결을 열 때의 설정 명령(`AUTH` · `SELECT <번호>` · `CLIENT SETINFO` · `INFO server`)은 창의 연결과
//!   같은 드라이버 열기 길이라 셈에 넣지 않는다(메인 판정 2026-11-17).
//! - **응답 상한 · `limit`**(결정 M5-D34): 결과 JSON 이 [`RESPONSE_BUDGET`](192 KiB) 안에 든다. 스캔은 드라이버 묶음을 통째로
//!   싣거나 싣지 않는다 — 응답 예산이나 `limit` 을 넘칠 묶음은 다음 쪽으로 넘기고(그 묶음 앞의 커서를 `nextCursor` 로), 쪽의 첫
//!   묶음은 늘 싣는다(그래서 커서가 나아간다 — `limit` 을 넘는 것은 `SCAN` 한 걸음이 혼자 더 준 때뿐이다). 첫 묶음이 응답 예산을
//!   넘치면 묶음 크기를 반씩 줄여 다시 묻는다.
//!   값 읽기는 쪽의 원소 수(string 은 바이트 창)를 반씩 줄여 다시 묻고, 더 줄일 수 없으면 [`cut_to_fit`]: 긴 값을 앞부분으로
//!   자르고(`truncated` — 감싸서 커지는 짧은 값은 두지 않는다), 그래도 넘치면 뒤쪽 필드(스트림 항목 하나) · 원소(한 걸음에 온 hash ·
//!   set 쪽)를 덜어 `omitted` 에 남긴다(리뷰 F1). 그래도 넘치면 거절한다 — 응답은 늘 예산 안이다. string 은 자르지 않고 창만
//!   줄인다 — 자르면 다음 쪽 커서와 사이가 빈다.
//! - **`SCAN` 의 중복**(이 행이 정한 것): Redis `SCAN` 은 같은 키를 두 번 줄 수 있다(재해시 중 · 다음 쪽이 넘긴 묶음을 다시 읽을
//!   때). **한 쪽 안에서는 거르고, 쪽 사이의 중복은 그대로 준다** — 툴 설명에 적는다. 끝까지 있던 키는 빠지지 않는다(`SCAN` 의
//!   보장 — 커서는 실은 묶음 뒤로만 나아간다).

use std::collections::HashSet;
use std::sync::Arc;

use pw_core::registry::agent::agent_grant;
use pw_db_driver::budget::within;
use pw_db_driver::{Budget, DbConnection, KvConnection};
use pw_db_types::{
    DbError, DbErrorKind, DbTruncated, DbValue, KvKey, KvKeyInfo, KvRange, KvScanRequest, KvType,
    KvValue, KvValuePage, TruncatedKind,
};
use pw_rpc::{Ctx, DEFAULT_LIST_LIMIT, Router, RpcError, rpc_method};
use serde::{Deserialize, Serialize};

use crate::gate::{DbGrant, db_grant_for_agent};
use crate::kv::{MAX_PATTERN_BYTES, MAX_REQUEST_BYTES};
use crate::schema::{check_limit, check_text};
use crate::script::RESPONSE_BUDGET;
use crate::session::{DbSessions, blocking, db_error};

/// 값 읽기에서 string · JSON 한 쪽의 바이트 창(응답 상한 안 — 글자를 이스케이프해 늘어나도 반씩 줄여 맞춘다).
pub const TOOL_STRING_WINDOW: u32 = 64 * 1024;
/// 툴 쪽 하나가 드라이버 스캔 묶음을 몇 번까지 묻는가(패턴이 드문 키 공간에서 빈 쪽이 이어지지 않게, 서버 일은 묶는다).
const MAX_SCAN_PAGES: usize = 16;

/// 드라이버에 물을 묶음 크기: 남은 몫의 절반. 드라이버는 묶음을 채우려고 `SCAN` 을 한 번 더 돌 수 있어(`COUNT` 만큼 — 그래서 묶음이
/// 물은 수의 두 배 가까이 된다, 실측 키 1,200개에 `limit` 100 이면 쪽마다 170개 남짓) 절반을 물으면 쪽이 `limit` 안에 든다.
fn scan_count(limit: u32, have: u32) -> u32 {
    (limit.saturating_sub(have) / 2).max(1)
}
/// 결과의 틀(커서 · 표지)에 남겨 두는 몫.
const FRAME_ROOM: usize = 1024;

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DbToolKvScanParams {
    pub project_id: String,
    /// Top of the worktree (absolute path). Connection settings are read from that worktree. Empty means the session's
    /// worktree, or else the main checkout.
    #[serde(default)]
    pub worktree: Option<String>,
    /// Connection id from `db_connection_list` (a Redis connection).
    pub connection_id: String,
    /// Redis database number (`"0"`, `"1"`, …). Empty means the connection settings' database.
    #[serde(default)]
    pub database: Option<String>,
    /// Glob pattern for key names (Redis `MATCH` — `*`, `?`, `[abc]`; `\` escapes). Empty means all keys. Sent as one
    /// argument, never as command text.
    #[serde(default, rename = "match")]
    pub pattern: Option<String>,
    /// `nextCursor` from the previous page. Empty for the first page.
    #[serde(default)]
    pub cursor: Option<String>,
    /// Keys per page (1 – 500, default 100) — at most this many, unless one Redis `SCAN` step returns more by itself. A
    /// page can hold fewer, even none, while `nextCursor` is set.
    #[serde(default)]
    pub limit: Option<u32>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DbToolKvScan {
    /// Keys of this page: name (text, or `{ "hex": "0x…" }` when not UTF-8 — pass it back as is to `db_kv_get`), type,
    /// remaining TTL in milliseconds (`null` = no expiry) and size (bytes from `MEMORY USAGE`, or the type's length when
    /// the server has no `MEMORY`).
    pub keys: Vec<KvKeyInfo>,
    /// Cursor of the next page (pass it back as `cursor`). Missing means the scan is complete.
    pub next_cursor: Option<String>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DbToolKvGetParams {
    pub project_id: String,
    /// Top of the worktree (absolute path). Connection settings are read from that worktree. Empty means the session's
    /// worktree, or else the main checkout.
    #[serde(default)]
    pub worktree: Option<String>,
    /// Connection id from `db_connection_list` (a Redis connection).
    pub connection_id: String,
    /// Redis database number (`"0"`, `"1"`, …). Empty means the connection settings' database.
    #[serde(default)]
    pub database: Option<String>,
    /// Key name as `db_kv_scan` returned it: text, or `{ "hex": "0x…" }` for bytes that are not UTF-8.
    pub key: KvKey,
    /// `nextCursor` from the previous page of this key's value. Empty for the first page.
    #[serde(default)]
    pub cursor: Option<String>,
    /// Elements per page for hash · list · set · zset · stream (1 – 500, default 100). A string is read in windows of
    /// up to 64 KiB. A page can hold fewer elements so that the response fits.
    #[serde(default)]
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DbToolKvGet {
    /// The key's type (`null` when the key is missing).
    pub kind: Option<KvType>,
    /// Remaining TTL in milliseconds (`null` = no expiry or missing).
    #[specta(type = Option<pw_rpc::JsNumber>)]
    pub ttl_ms: Option<u64>,
    /// One page of the value by type (`"missing"` when the key does not exist): `string` (text, or bytes as hex) ·
    /// `hash` (field · value pairs) · `list` · `set` · `zset` (member · score) · `stream` (entry id · fields) · `json`.
    pub value: KvValue,
    /// Whole length (elements; bytes for a string). `null` if unknown.
    #[specta(type = Option<pw_rpc::JsNumber>)]
    pub total: Option<u64>,
    /// Cursor of the next page (pass it back as `cursor`). Missing means this is the last page.
    pub next_cursor: Option<String>,
    /// Something was left out to fit the response: some element values were cut (those values are marked `truncated`)
    /// and/or trailing elements or fields were left out (`omitted`).
    pub truncated: bool,
    /// Trailing parts left out of this page to fit the response (`null` when nothing was left out).
    pub omitted: Option<DbToolKvOmitted>,
}

/// What a page left out to fit the response.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum DbToolKvOmittedKind {
    /// The last elements of this page (a hash · set page that Redis returned in one step). `nextCursor` continues after
    /// them, so those elements are not shown by any page.
    Elements,
    /// The last fields of this page's one stream entry. Redis reads an entry whole, so the left-out fields cannot be
    /// read through this tool; `nextCursor` continues with the next entry.
    Fields,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DbToolKvOmitted {
    pub of: DbToolKvOmittedKind,
    /// How many are shown (the first ones, in order).
    pub shown: u32,
    /// How many there were on this page (elements) or in the entry (fields).
    pub total: u32,
}

rpc_method! {
    /// Find Redis keys, one page at a time (Redis `SCAN` with an optional glob `match`): each key with its type · TTL ·
    /// size, and `nextCursor` for the next page — call again with it until `nextCursor` is missing. A page may hold
    /// fewer keys than `limit`, even none, while more pages remain. Keys are not repeated within one page, but **a key
    /// can appear again on a later page** (Redis `SCAN` may return a key twice); keys that exist for the whole scan are
    /// never missed. There is no command input — this tool only reads. Uses only connections the person opened in
    /// Pitwall's DB explorer during this app run (otherwise `notOpenedByUser` — try again after the person has opened
    /// it once) and never uses a Redis server marked production — any database number on the same server · user
    /// (`productionBlocked`). Redis connections only — for SQL databases use `db_schema` · `db_query`.
    pub DbToolKvScanMethod: "db.tool.kvScan" {
        params: DbToolKvScanParams,
        result: DbToolKvScan,
        tier: Read,
        roles: [Owner],
        mcp: { group: "db", name: "db_kv_scan" },
    }

    /// Read one page of a Redis key's value, by type: a string in byte windows, a hash · set in scan pages, a list ·
    /// zset by index, a stream by entry id, a RedisJSON document whole. `nextCursor` reads the next page; a missing key
    /// is `"missing"`. Values that are not UTF-8 come as hex bytes. The response is limited to about 192 KiB — a page
    /// holds fewer elements to fit, and an element too large on its own is cut (`truncated`); a stream entry with too
    /// many fields shows only its first fields (`omitted` says how many — the rest cannot be read here). There is no command
    /// input — this tool only reads. Uses only connections the person opened in Pitwall's DB explorer during this app
    /// run (`notOpenedByUser`) and never uses a Redis server marked production (`productionBlocked`). Redis connections
    /// only.
    pub DbToolKvGetMethod: "db.tool.kvGet" {
        params: DbToolKvGetParams,
        result: DbToolKvGet,
        tier: Read,
        roles: [Owner],
        mcp: { group: "db", name: "db_kv_get" },
    }
}

/// `database` 입력: DB 번호(10진 숫자 1 ~ 10자). 같은 번호의 다른 표기는 표지의 열쇠가 DB 번호를 빼므로 표지를 비껴가지 못하지만,
/// 입력 꼴은 좁게 받는다.
fn check_database(database: Option<&str>) -> Result<(), RpcError> {
    match database {
        Some(d) if d.is_empty() || d.len() > 10 || !d.bytes().all(|b| b.is_ascii_digit()) => Err(
            RpcError::invalid_params("database must be a Redis database number such as \"0\""),
        ),
        _ => Ok(()),
    }
}

fn json_len<T: Serialize>(value: &T) -> usize {
    serde_json::to_vec(value).map_or(usize::MAX, |v| v.len())
}

impl DbSessions {
    /// 관문(M5-06 의 입구)을 지나 그 연결의 KV 연결을 연다. Redis 연결이 아니면 `unsupported`.
    async fn kv_tool_open(
        self: &Arc<Self>,
        ctx: &Ctx,
        tool: &str,
        target: (&str, Option<&str>, &str, Option<&str>),
    ) -> Result<(Box<dyn KvConnection>, Option<crate::session::DbTunnelLease>), RpcError> {
        let (project, worktree, id, database) = target;
        check_text("worktree", worktree)?;
        check_database(database)?;
        let (s, scope) = (Arc::clone(self), ctx.scope().clone());
        let (project_owned, worktree_owned, id_owned, database_owned) = (
            project.to_owned(),
            worktree.map(str::to_owned),
            id.to_owned(),
            database.map(str::to_owned),
        );
        let grant: DbGrant = blocking(move || {
            let agent = agent_grant(&scope, &s.connections.host()?.projects, &project_owned)?;
            db_grant_for_agent(
                &s,
                &agent,
                worktree_owned.as_deref(),
                &id_owned,
                database_owned.as_deref(),
            )
        })
        .await?;
        let driver = grant
            .agent_driver()
            .and_then(|kind| self.drivers.get(kind.id()))
            .ok_or_else(|| {
                db_error(DbError::new(
                    DbErrorKind::Unsupported,
                    "the db driver of this connection is not installed",
                ))
            })?;
        if !driver.capabilities().kv {
            return Err(db_error(DbError::new(
                DbErrorKind::Unsupported,
                format!(
                    "{tool} reads Redis connections only; `{id}` is an SQL database — use db_schema or db_query"
                ),
            )));
        }
        let opened =
            crate::connection::connect(self, &grant, worktree, id, database.map(str::to_owned))
                .await?;
        match opened.connection {
            DbConnection::Kv(conn) => Ok((conn, opened.tunnel)),
            other => {
                let _ = other.close().await;
                Err(RpcError::invalid_params(
                    "this connection is not a key-value store",
                ))
            }
        }
    }

    /// `db_kv_scan`.
    async fn tool_kv_scan(
        self: &Arc<Self>,
        ctx: &Ctx,
        p: DbToolKvScanParams,
    ) -> Result<DbToolKvScan, RpcError> {
        let limit = check_limit(p.limit.unwrap_or(DEFAULT_LIST_LIMIT))?;
        if let Some(pattern) = &p.pattern
            && (pattern.is_empty() || pattern.len() > MAX_PATTERN_BYTES)
        {
            return Err(RpcError::invalid_params(format!(
                "match must be 1-{MAX_PATTERN_BYTES} bytes"
            )));
        }
        if let Some(cursor) = &p.cursor
            && (cursor.is_empty()
                || cursor.len() > 20
                || !cursor.bytes().all(|b| b.is_ascii_digit()))
        {
            return Err(RpcError::invalid_params(
                "cursor must be a nextCursor returned by db_kv_scan",
            ));
        }
        let (mut conn, tunnel) = self
            .kv_tool_open(
                ctx,
                "db_kv_scan",
                (
                    &p.project_id,
                    p.worktree.as_deref(),
                    &p.connection_id,
                    p.database.as_deref(),
                ),
            )
            .await?;
        let scanned = scan_page(conn.as_mut(), p.pattern, p.cursor, limit).await;
        let _ = within(Budget::PAGE.connect, "close", conn.close()).await;
        drop(tunnel);
        scanned
    }

    /// `db_kv_get`.
    async fn tool_kv_get(
        self: &Arc<Self>,
        ctx: &Ctx,
        p: DbToolKvGetParams,
    ) -> Result<DbToolKvGet, RpcError> {
        let limit = check_limit(p.limit.unwrap_or(DEFAULT_LIST_LIMIT))?;
        let bytes = p.key.to_bytes().ok_or_else(|| {
            RpcError::invalid_params("a key's hex must be `0x` and an even number of hex digits")
        })?;
        if bytes.len() > MAX_REQUEST_BYTES {
            return Err(RpcError::invalid_params(format!(
                "the key is {} bytes, over the {MAX_REQUEST_BYTES} byte limit",
                bytes.len()
            )));
        }
        check_text("cursor", p.cursor.as_deref())?;
        let (mut conn, tunnel) = self
            .kv_tool_open(
                ctx,
                "db_kv_get",
                (
                    &p.project_id,
                    p.worktree.as_deref(),
                    &p.connection_id,
                    p.database.as_deref(),
                ),
            )
            .await?;
        let read = read_page(conn.as_mut(), p.key, p.cursor, limit).await;
        let _ = within(Budget::PAGE.connect, "close", conn.close()).await;
        drop(tunnel);
        read
    }
}

/// 키 한 쪽(머리 주석의 「응답 상한」 · 「`SCAN` 의 중복」).
async fn scan_page(
    conn: &mut dyn KvConnection,
    pattern: Option<String>,
    cursor: Option<String>,
    limit: u32,
) -> Result<DbToolKvScan, RpcError> {
    let room = RESPONSE_BUDGET - FRAME_ROOM;
    let mut keys: Vec<KvKeyInfo> = Vec::new();
    let mut seen: HashSet<KvKey> = HashSet::new();
    let mut used = 0_usize;
    let mut cursor = cursor;
    let mut count = scan_count(limit, 0);
    let mut pages = 0;
    loop {
        let page = conn
            .scan(KvScanRequest {
                pattern: pattern.clone(),
                cursor: cursor.clone(),
                count,
            })
            .await
            .map_err(db_error)?;
        let fresh: Vec<KvKeyInfo> = page
            .keys
            .into_iter()
            .filter(|k| !seen.contains(&k.key))
            .collect();
        let size: usize = fresh.iter().map(|k| json_len(k) + 1).sum();
        let over_limit = keys.len() + fresh.len() > limit as usize;
        if !keys.is_empty() && (over_limit || used.saturating_add(size) > room) {
            // 이 묶음은 싣지 않는다 — 다음 쪽이 그 앞의 커서부터 다시 읽는다(빠지는 키가 없다). 다음 쪽의 첫 묶음은 늘 실으므로
            // 커서는 나아간다.
            return Ok(DbToolKvScan {
                keys,
                next_cursor: cursor,
            });
        }
        if used.saturating_add(size) > room {
            if count > 1 {
                count /= 2;
                continue;
            }
            return Err(RpcError::invalid_params(
                "a key name at this cursor is too large to list in one tool response; narrow `match`",
            ));
        }
        used += size;
        for key in fresh {
            seen.insert(key.key.clone());
            keys.push(key);
        }
        cursor = page.next_cursor;
        pages += 1;
        let have = u32::try_from(keys.len()).unwrap_or(u32::MAX);
        if cursor.is_none() || have >= limit || pages >= MAX_SCAN_PAGES {
            return Ok(DbToolKvScan {
                keys,
                next_cursor: cursor,
            });
        }
        count = scan_count(limit, have);
    }
}

/// 값 한 쪽(머리 주석의 「응답 상한」).
async fn read_page(
    conn: &mut dyn KvConnection,
    key: KvKey,
    cursor: Option<String>,
    limit: u32,
) -> Result<DbToolKvGet, RpcError> {
    let Some(info) = conn.key_info(key.clone()).await.map_err(db_error)? else {
        return Ok(DbToolKvGet {
            kind: None,
            ttl_ms: None,
            value: KvValue::Missing,
            total: None,
            next_cursor: None,
            truncated: false,
            omitted: None,
        });
    };
    let windowed = matches!(info.kind, KvType::String | KvType::Json);
    let mut count = if windowed { TOOL_STRING_WINDOW } else { limit };
    let room = RESPONSE_BUDGET - FRAME_ROOM;
    loop {
        let page = conn
            .read(
                key.clone(),
                KvRange {
                    cursor: cursor.clone(),
                    count,
                },
            )
            .await
            .map_err(db_error)?;
        let KvValuePage {
            mut value,
            total,
            next_cursor,
        } = page;
        let fits = json_len(&value) <= room;
        // string 은 창만 줄인다(자르면 다음 쪽 커서와 사이가 빈다). JSON 문서는 쪽이 없어 줄일 창이 없다.
        let shrinkable = count > 1 && !matches!(value, KvValue::Json(_)) && elements(&value) != 1;
        if !fits && shrinkable {
            count /= 2;
            continue;
        }
        let (truncated, omitted) = if fits {
            (false, None)
        } else {
            let cut = cut_to_fit(&mut value, room);
            if json_len(&value) > room {
                return Err(RpcError::invalid_params(
                    "the value at this cursor is too large for one tool response even after cutting it",
                ));
            }
            (cut.values || cut.omitted.is_some(), cut.omitted)
        };
        return Ok(DbToolKvGet {
            // 그사이 키가 지워졌으면(`missing`) 타입 · TTL 도 없다.
            kind: (!matches!(value, KvValue::Missing)).then_some(info.kind),
            ttl_ms: (!matches!(value, KvValue::Missing))
                .then_some(info.ttl_ms)
                .flatten(),
            value,
            total,
            next_cursor,
            truncated,
            omitted,
        });
    }
}

/// 쪽의 원소 수(string · JSON 은 0 — 창으로 줄인다).
fn elements(value: &KvValue) -> usize {
    match value {
        KvValue::Missing | KvValue::String(_) | KvValue::Json(_) => 0,
        KvValue::Hash(v) => v.len(),
        KvValue::List(v) | KvValue::Set(v) => v.len(),
        KvValue::Zset(v) => v.len(),
        KvValue::Stream(v) => v.len(),
    }
}

fn values_mut(value: &mut KvValue) -> Vec<&mut DbValue> {
    match value {
        KvValue::Missing => Vec::new(),
        KvValue::String(v) | KvValue::Json(v) => vec![v],
        KvValue::Hash(fields) => fields
            .iter_mut()
            .flat_map(|f| [&mut f.field, &mut f.value])
            .collect(),
        KvValue::List(v) | KvValue::Set(v) => v.iter_mut().collect(),
        KvValue::Zset(v) => v.iter_mut().map(|m| &mut m.member).collect(),
        KvValue::Stream(entries) => entries
            .iter_mut()
            .flat_map(|e| e.fields.iter_mut())
            .flat_map(|f| [&mut f.field, &mut f.value])
            .collect(),
    }
}

/// 값을 자를 때 남기는 몫의 하한(바이트). 이보다 짧은 값은 자르지 않는다 — 감싼 표지(`truncated` 의 틀)가 값보다 커진다.
const MIN_CUT: usize = 1024;

/// [`cut_to_fit`] 이 한 것.
#[derive(Debug, Default, PartialEq, Eq)]
struct Cut {
    /// 값 하나 이상을 앞부분으로 잘랐다.
    values: bool,
    /// 뒤쪽 원소 · 필드를 덜었다.
    omitted: Option<DbToolKvOmitted>,
}

/// 쪽이 `room` 안에 들게 한다. ① 긴 값을 같은 몫(하한 [`MIN_CUT`])으로 앞부분만 남긴다 — 감싸서 커지는 값은 건드리지 않는다
/// ② 그래도 넘치면 뒤에서부터 덜어 낸다: 스트림 항목 하나면 그 항목의 필드를, 원소가 여럿이면 원소를(덜어 낸 수를 남긴다). 다 해도
/// 넘치면 부른 쪽이 거절한다(필드 이름 하나가 예산보다 큰 꼴 등은 ①이 막는다).
fn cut_to_fit(value: &mut KvValue, room: usize) -> Cut {
    let mut cut = Cut::default();
    let n = values_mut(value).len().max(1);
    let mut share = room / n;
    while json_len(value) > room && share >= MIN_CUT {
        for v in values_mut(value) {
            cut.values |= cut_value(v, share);
        }
        share /= 2;
    }
    if json_len(value) > room {
        cut.omitted = omit_tail(value, room);
    }
    cut
}

/// 뒤쪽 부분을 덜어 `room` 안에 들게 한다. 첫 부분은 남긴다(쪽이 비면 읽을 것이 없다 — 그 부분 하나가 넘치면 부른 쪽이 거절한다).
fn omit_tail(value: &mut KvValue, room: usize) -> Option<DbToolKvOmitted> {
    fn keep<T: Serialize>(parts: &mut Vec<T>, base: usize, room: usize) -> (u32, u32) {
        let total = parts.len();
        let mut used = base;
        let mut kept = 0;
        for part in parts.iter() {
            let size = json_len(part) + 1;
            if kept > 0 && used + size > room {
                break;
            }
            used += size;
            kept += 1;
        }
        parts.truncate(kept);
        (
            u32::try_from(kept).unwrap_or(u32::MAX),
            u32::try_from(total).unwrap_or(u32::MAX),
        )
    }
    // 틀의 크기: 그 목록을 비운 값.
    let base = |value: &KvValue| {
        let mut empty = value.clone();
        match &mut empty {
            KvValue::Stream(entries) if entries.len() == 1 => entries[0].fields.clear(),
            KvValue::Hash(v) => v.clear(),
            KvValue::List(v) | KvValue::Set(v) => v.clear(),
            KvValue::Zset(v) => v.clear(),
            KvValue::Stream(v) => v.clear(),
            KvValue::Missing | KvValue::String(_) | KvValue::Json(_) => {}
        }
        json_len(&empty)
    };
    let frame = base(value);
    let (of, (shown, total)) = match value {
        KvValue::Stream(entries) if entries.len() == 1 => (
            DbToolKvOmittedKind::Fields,
            keep(&mut entries[0].fields, frame, room),
        ),
        KvValue::Stream(v) => (DbToolKvOmittedKind::Elements, keep(v, frame, room)),
        KvValue::Hash(v) => (DbToolKvOmittedKind::Elements, keep(v, frame, room)),
        KvValue::List(v) | KvValue::Set(v) => (DbToolKvOmittedKind::Elements, keep(v, frame, room)),
        KvValue::Zset(v) => (DbToolKvOmittedKind::Elements, keep(v, frame, room)),
        KvValue::Missing | KvValue::String(_) | KvValue::Json(_) => return None,
    };
    (shown < total).then_some(DbToolKvOmitted { of, shown, total })
}

/// 값 하나를 `max` 바이트(글은 UTF-8 바이트, 바이트 값은 hex 글자) 안으로. 자른 결과의 JSON 이 원래보다 작을 때만 바꾼다.
/// 바꿨으면 `true`.
fn cut_value(value: &mut DbValue, max: usize) -> bool {
    let prefix = |text: &str| {
        let mut end = max.min(text.len());
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text[..end].to_owned()
    };
    let candidate = match &*value {
        DbValue::Text(text) if text.len() > max => Some(DbValue::Truncated(DbTruncated {
            of: TruncatedKind::Text,
            prefix: prefix(text),
            size: text.len() as u64,
            type_name: None,
        })),
        DbValue::Json(text) if text.len() > max => Some(DbValue::Truncated(DbTruncated {
            of: TruncatedKind::Json,
            prefix: prefix(text),
            size: text.len() as u64,
            type_name: None,
        })),
        DbValue::Truncated(t) if t.prefix.len() > max => Some(DbValue::Truncated(DbTruncated {
            prefix: prefix(&t.prefix),
            ..t.clone()
        })),
        DbValue::Bytes(b) if b.hex.len() > max.max(2) => {
            let keep = max.saturating_sub(2) / 2;
            let mut b = b.clone();
            b.hex.truncate(2 + keep * 2);
            b.truncated = true;
            Some(DbValue::Bytes(b))
        }
        _ => None,
    };
    match candidate {
        Some(smaller) if json_len(&smaller) < json_len(&*value) => {
            *value = smaller;
            true
        }
        _ => false,
    }
}

/// 툴 메서드를 붙인다(`crate::session::register` 가 부른다).
pub(crate) fn register(router: &mut Router, sessions: Arc<DbSessions>) {
    let scan = Arc::clone(&sessions);
    router
        .unary(DbToolKvScanMethod, move |ctx, p: DbToolKvScanParams| {
            let s = Arc::clone(&scan);
            async move { s.tool_kv_scan(&ctx, p).await }
        })
        .unary(DbToolKvGetMethod, move |ctx, p: DbToolKvGetParams| {
            let s = Arc::clone(&sessions);
            async move { s.tool_kv_get(&ctx, p).await }
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use pw_db_types::{CellLimit, KvField};

    #[test]
    fn database_numbers_are_digits_only() {
        assert!(check_database(None).is_ok());
        assert!(check_database(Some("0")).is_ok());
        assert!(check_database(Some("15")).is_ok());
        for bad in ["", " 0", "+0", "-1", "0x1", "a", "12345678901"] {
            assert!(check_database(Some(bad)).is_err(), "{bad:?}");
        }
    }

    /// 원소 하나가 응답을 넘치면 그 값만 앞부분으로 자르고 표지를 남긴다 — 글은 글자 경계, 바이트는 hex 두 글자 단위.
    #[test]
    fn values_too_large_for_the_response_are_cut_with_a_mark() {
        let big = "가".repeat(100_000);
        let mut value = KvValue::Hash(vec![KvField {
            field: DbValue::text("f", CellLimit::DEFAULT),
            value: DbValue::text(big.clone(), CellLimit(usize::MAX)),
        }]);
        assert!(cut_to_fit(&mut value, 64 * 1024).values);
        assert!(json_len(&value) <= 64 * 1024);
        let KvValue::Hash(fields) = &value else {
            unreachable!()
        };
        assert_eq!(fields[0].field, DbValue::Text("f".into()));
        let DbValue::Truncated(t) = &fields[0].value else {
            panic!("{:?}", fields[0].value)
        };
        assert_eq!(t.size, big.len() as u64);
        assert!(big.starts_with(&t.prefix) && !t.prefix.is_empty());

        let mut bytes = KvValue::List(vec![DbValue::bytes(
            &[0xff; 200_000],
            CellLimit(usize::MAX),
        )]);
        assert!(cut_to_fit(&mut bytes, 64 * 1024).values);
        let KvValue::List(items) = &bytes else {
            unreachable!()
        };
        let DbValue::Bytes(b) = &items[0] else {
            panic!()
        };
        assert!(b.truncated && b.hex.len() % 2 == 0 && b.hex.starts_with("0xff"));
        assert!(json_len(&bytes) <= 64 * 1024);

        let mut small = KvValue::Set(vec![DbValue::text("a", CellLimit::DEFAULT)]);
        assert_eq!(cut_to_fit(&mut small, 64 * 1024), Cut::default());
    }

    fn text(t: &str) -> DbValue {
        DbValue::text(t, CellLimit::DEFAULT)
    }

    /// 리뷰 F1: 필드가 아주 많은 스트림 항목 하나는 값 자르기로는 들지 않는다 — 짧은 값을 감싸 키우지 않고, 항목의 뒤 필드를
    /// 덜어 예산 안에 들며 몇 개를 실었는지 남긴다.
    #[test]
    fn a_stream_entry_with_too_many_fields_keeps_its_first_fields_within_the_room() {
        let room = RESPONSE_BUDGET - FRAME_ROOM;
        let fields: Vec<KvField> = (0..30_000)
            .map(|i| KvField {
                field: text(&format!("f{i}")),
                value: text("v"),
            })
            .collect();
        let mut value = KvValue::Stream(vec![pw_db_types::KvStreamEntry {
            id: "1-1".into(),
            fields,
        }]);
        let before = json_len(&value);
        let cut = cut_to_fit(&mut value, room);
        assert!(json_len(&value) <= room && json_len(&value) < before);
        assert!(!cut.values, "short values are not wrapped");
        let omitted = cut.omitted.unwrap();
        assert_eq!(
            (omitted.of, omitted.total),
            (DbToolKvOmittedKind::Fields, 30_000)
        );
        let KvValue::Stream(entries) = &value else {
            unreachable!()
        };
        assert_eq!(entries[0].fields.len() as u32, omitted.shown);
        assert!(
            omitted.shown > 1_000 && omitted.shown < 30_000,
            "{omitted:?}"
        );
        assert_eq!(entries[0].fields[0].field, text("f0"));
        assert!(
            entries[0]
                .fields
                .iter()
                .all(|f| matches!((&f.field, &f.value), (DbValue::Text(_), DbValue::Text(_))))
        );
    }

    /// 감싼 결과가 원래보다 크면 자르지 않는다 — 짧은 값은 몫이 작아도 그대로다.
    #[test]
    fn cutting_never_makes_a_value_larger() {
        // 감싼 표지(`truncated` 의 틀)가 남는 앞부분보다 큰 값은 그대로 둔다.
        for (value, max) in [(text("abcdef"), 3), (text(&"x".repeat(80)), 40)] {
            let mut v = value.clone();
            assert!(!cut_value(&mut v, max), "{value:?}");
            assert_eq!(v, value);
        }
        // 바이트 값은 틀이 같아(`truncated` 칸이 늘 있다) 자르면 늘 작아진다.
        let bytes = DbValue::bytes(&[1, 2, 3, 4, 5, 6], CellLimit::DEFAULT);
        let mut b = bytes.clone();
        assert!(cut_value(&mut b, 6));
        assert!(json_len(&b) < json_len(&bytes));
        let mut long = text(&"y".repeat(10_000));
        assert!(cut_value(&mut long, 2_000));
        assert!(json_len(&long) < 10_000);
    }

    /// Redis 가 한 걸음에 준 원소가 많은 쪽(작은 hash · set — listpack)이 원소 하나로 줄일 수 없을 때는 뒤 원소를 덜고 그 수를 남긴다.
    #[test]
    fn a_page_of_many_elements_that_cannot_shrink_omits_its_tail() {
        let room = 64 * 1024;
        let mut value = KvValue::Hash(
            (0..5_000)
                .map(|i| KvField {
                    field: text(&format!("field{i:05}")),
                    value: text(&"v".repeat(40)),
                })
                .collect(),
        );
        let cut = cut_to_fit(&mut value, room);
        assert!(json_len(&value) <= room);
        let omitted = cut.omitted.unwrap();
        assert_eq!(
            (omitted.of, omitted.total),
            (DbToolKvOmittedKind::Elements, 5_000)
        );
        assert!(omitted.shown > 0 && omitted.shown < 5_000);
        assert!(!cut.values);
    }

    /// zset 의 멤버 하나 · set 의 멤버 하나가 예산보다 크면 그 값만 자른다(덜지 않는다).
    #[test]
    fn one_huge_member_is_cut_not_omitted() {
        let room = RESPONSE_BUDGET - FRAME_ROOM;
        let big = "m".repeat(600 * 1024);
        for mut value in [
            KvValue::Zset(vec![pw_db_types::KvScored {
                member: DbValue::text(big.clone(), CellLimit(usize::MAX)),
                score: "1".into(),
            }]),
            KvValue::Set(vec![DbValue::text(big.clone(), CellLimit(usize::MAX))]),
            KvValue::Json(DbValue::json(big.clone(), CellLimit(usize::MAX))),
        ] {
            let cut = cut_to_fit(&mut value, room);
            assert!(json_len(&value) <= room);
            assert_eq!((cut.values, cut.omitted), (true, None));
        }
    }
}
