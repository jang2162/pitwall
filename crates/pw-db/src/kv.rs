//! Redis RPC `db.kv.*`(14-m3-plan.md M3-22, 결정 M3-D15 · M3-D18 · M3-D19 · M3-D20): M3-09 의 `KvConnection` 을 실행 관문
//! 뒤에 내놓는다.
//!
//! - **연결 · 세션**은 M3-05 의 것을 그대로 쓴다: `db.session.open { projectId, connectionId, database? }` 가 Redis 연결 하나를
//!   연다 — `database` 는 DB 번호(`"0"` …)이고 **DB 번호마다 연결 하나**다(`SELECT` 는 분류가 막는다, 결정 M3-D15). 모든 메서드는
//!   `sessionId` 를 받고, 그 세션을 연 프로젝트 창의 연결에서만 돈다([`DbGrant`](crate::gate::DbGrant) — 다른 프로젝트 창 · 웰컴
//!   · 설정 · 파일 창은 `PERMISSION_DENIED`, 결정 M3-D18). 부를 때마다 신뢰와 연결 설정을 다시 본다(미신뢰면 `untrusted`, 설정이
//!   바뀌었으면 `configChanged`).
//! - **등급 정책**(결정 M3-D20, 판정은 [`judge`]): 명령은 `pw_db_sql::kv_command` 의 분류(허용 목록 — 분류기는 그것 하나다)로
//!   등급을 매긴다. `Blocked` 는 거절(`PERMISSION_DENIED`, `data.reason = "blockedCommand"`), read-only 연결은 조회만, `Confirm`
//!   은 확인, 프로덕션 연결은 조회 밖을 모두 확인한다. **전용 메서드는 따로 등급을 두지 않는다** — 서버에 보낼 것과 같은 명령
//!   (`setTtl` = `PEXPIRE`, `persist` = `PERSIST`, `delete` = `DEL`, `rename` = `RENAME` · 덮지 않으면 `RENAMENX`)을 같은
//!   분류기에 넣어 그 등급 · 확인을 받는다(한 분류기). 그래서 `setTtl` 은 `PEXPIRE` 처럼 확인이고(1 ms 는 지우기와 같다),
//!   `RENAMENX` 도 분류대로 확인이다. 확인이 필요하면 [`code::CONFIRM_REQUIRED`](crate::gate::code::CONFIRM_REQUIRED)(`data` 는
//!   [`DbKvConfirmRequired`])가 해시를 돌려주고, 창이 확인을 받은 뒤 `confirmed { grade, statementsHash }` 로 다시 부른다. 해시는
//!   세션 · 그 세션의 확인 순번 · 메서드 · 명령 인자(바이트)에 묶인다 — 다른 키 · 다른 세션에 쓰이지 않는다. **쓰기가 관문을
//!   지나면 그 세션의 확인을 모두 쓴다**(SQL 과 같다 — 한 번만 쓰인다). 읽기는 쓰지 않는다: 한 세션을 키 트리 · 값 뷰어 · 명령
//!   콘솔이 함께 쓰므로, 확인 다이얼로그가 떠 있는 동안 키 트리가 스캔을 이어 가도 확인이 깨지지 않게.
//! - **거절은 서버에 보내기 전**이다 — 드라이버는 분류하지 않는다(`KvConnection::command` 는 부르는 쪽이 분류했다고 본다).
//! - **진행 · 취소**(M3-05 와 같다): 스캔 스트림은 맨 앞에 `accepted { executionId }` 를 내고, `db.query.cancel { executionId }`
//!   가 드라이버 인터럽트로 멈춘다. 진행은 `db.queryProgress`(`rows` = 지금까지 받은 키 수). 스트림 취소 · 단항 메서드의 요청
//!   취소(`request.cancel` — 창의 `AbortSignal`)는 드라이버 호출을 버린다 — Redis 드라이버는 그때 그 연결을 버리고 다음 요청이
//!   새 연결(DB 번호 · 인증을 다시 건다)을 쓴다. 버린 응답 뒤에 다음 요청이 서지 않는다. 도는 요청은 세션의 실행 자리
//!   (`DbSession::running`)에 있어, 창이 닫히면(세션 닫기) 응답 타임아웃을 기다리지 않고 곧바로 멈춘다.
//! - **인터럽트는 실행에 묶인다**([`BoundInterrupt`]): Redis 의 인터럽트는 연결 전체를 깨우므로, 그 실행이 실행 자리를 쥔
//!   동안만 보낸다 — 끝난 실행의 늦은 취소(`db.query.cancel`)가 같은 세션의 무관한 다음 요청을 멈추지 않게.
//! - **한 세션의 요청은 차례로 돈다**: 연결 잠금을 기다린다(SQL 세션처럼 `busy` 로 거절하지 않는다 — 키 트리 · 값 뷰어 · 콘솔이
//!   한 세션을 함께 쓴다). 스캔 스트림은 묶음 하나를 읽는 동안만 잡고, 창이 credit 을 주기를 기다리는 동안은 놓는다. 긴
//!   스캔은 **묶음마다** 범위(그 창의 세션인지) · 신뢰 · 연결 설정을 다시 본다 — 신뢰를 거두거나 설정이 바뀌면 다음 묶음에서 끝난다.
//! - 입력은 모두 struct 다(M5 가 툴로 켤 수 있는 꼴, 결정 M3-D19). `mcp: true` 는 달지 않는다.

use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use pw_db_driver::{
    BoxFuture, Budget, DbConnection, DbResult, Interrupt, KvConnection, QueryCancel,
};
use pw_db_sql::kv_command::{BlockReason, KvCommandClass, MAX_SCAN_COUNT, classify_argv};
use pw_db_types::{
    DbError, DbErrorKind, KvKey, KvKeyInfo, KvRange, KvRenameOutcome, KvReply, KvScanRequest,
    KvValuePage, RiskGrade,
};
use pw_rpc::{Ctx, JsNumber, Router, RpcError, StreamSink, code as rpc_code, rpc_method};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio_util::sync::CancellationToken;

use crate::execute::DbQueryProgress;
use crate::gate::{DbConfirmed, code};
use crate::session::{DbSession, DbSessions, Link, Running, TOPIC_QUERY_PROGRESS, db_error, lock};

/// 스캔 한 묶음의 기본 키 수(키 브라우저 한 묶음 — 14 M3-09 ①).
pub const DEFAULT_SCAN_COUNT: u32 = 1_000;
/// 값 읽기 한 쪽의 기본 크기(컬렉션은 원소 수, string 은 바이트 창 — `KvRange::count`).
pub const DEFAULT_READ_COUNT: u32 = 1_000;
/// `range.count` 의 상한: 드라이버의 쪽 예산(4 MiB — `pw_db_driver_redis::PAGE_BUDGET`)과 같다.
pub const MAX_READ_COUNT: u32 = 4 * 1024 * 1024;
/// 패턴의 상한(바이트).
pub const MAX_PATTERN_BYTES: usize = 16 * 1024;
/// 요청 하나의 키 · 인자 바이트 합의 상한(명령 · 지울 키 목록). 드라이버의 쪽 예산과 같다.
pub const MAX_REQUEST_BYTES: usize = 4 * 1024 * 1024;
/// 한 번에 지울 키 수의 상한(키 브라우저 한 묶음 상한과 같다).
pub const MAX_DELETE_KEYS: usize = MAX_SCAN_COUNT as usize;
/// 커서 글의 상한.
const MAX_CURSOR_CHARS: usize = 256;
/// `db.queryProgress` 를 내는 간격.
const PROGRESS_EVERY: Duration = Duration::from_millis(500);

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DbKvScanParams {
    pub session_id: String,
    /// `MATCH` 패턴(glob). 없으면 모든 키.
    #[serde(default)]
    pub pattern: Option<String>,
    /// 한 묶음의 키 수(`COUNT`, 1 ~ 10,000, 기본 1,000).
    #[serde(default)]
    pub count: Option<u32>,
}

/// 스캔 스트림의 항목. 드라이버 오류는 스트림을 실패로 끝낸다(3003, `data.error`), 취소는 `CANCELLED` 다.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum DbKvScanEvent {
    /// 관문을 지났다. `executionId` 로 취소한다(`db.query.cancel`).
    Accepted { execution_id: String },
    /// 키 한 묶음(키마다 타입 · TTL · 크기).
    Keys { keys: Vec<KvKeyInfo> },
    /// 스캔이 끝났다.
    Done {
        /// 받은 키 수(스캔 중 바뀐 키는 두 번 오거나 빠질 수 있다 — Redis `SCAN` 의 보장).
        #[specta(type = JsNumber)]
        keys: u64,
        #[specta(type = JsNumber)]
        elapsed_ms: u64,
    },
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DbKvGetParams {
    pub session_id: String,
    pub key: KvKey,
    /// 쪽(앞 쪽의 `nextCursor` 와 크기). 없으면 첫 쪽, 크기 1,000.
    #[serde(default)]
    pub range: Option<KvRange>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DbKvSetTtlParams {
    pub session_id: String,
    pub key: KvKey,
    /// 새 수명(밀리초, 1 이상). 지우려면 `db.kv.delete`, 만료를 없애려면 `db.kv.persist`.
    #[specta(type = JsNumber)]
    pub ttl_ms: u64,
    #[serde(default)]
    pub confirmed: Option<DbConfirmed>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DbKvPersistParams {
    pub session_id: String,
    pub key: KvKey,
    #[serde(default)]
    pub confirmed: Option<DbConfirmed>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DbKvDeleteParams {
    pub session_id: String,
    /// 지울 키(1 ~ 10,000개).
    pub keys: Vec<KvKey>,
    #[serde(default)]
    pub confirmed: Option<DbConfirmed>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DbKvRenameParams {
    pub session_id: String,
    pub from: KvKey,
    pub to: KvKey,
    /// 대상이 있으면 덮는다(`RENAME` — 「덮어쓰기」 확인이 필요하다). 아니면 대상이 있을 때 바꾸지 않고 `targetExists`(`RENAMENX`).
    #[serde(default)]
    pub overwrite: bool,
    #[serde(default)]
    pub confirmed: Option<DbConfirmed>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DbKvCommandParams {
    pub session_id: String,
    /// 명령 이름과 인자.
    pub argv: Vec<String>,
    #[serde(default)]
    pub confirmed: Option<DbConfirmed>,
}

/// `db.kv.delete` 의 답.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DbKvDeleted {
    /// 지운 키 수(없던 키는 세지 않는다).
    #[specta(type = JsNumber)]
    pub deleted: u64,
}

/// 확인이 필요한 일.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum DbKvAction {
    SetTtl,
    Persist,
    Delete,
    Rename,
    Command,
}

impl DbKvAction {
    fn tag(self) -> &'static [u8] {
        match self {
            Self::SetTtl => b"setTtl",
            Self::Persist => b"persist",
            Self::Delete => b"delete",
            Self::Rename => b"rename",
            Self::Command => b"command",
        }
    }
}

/// [`code::CONFIRM_REQUIRED`] 의 `data`(`db.kv.*`). 창은 등급 · 서버에 갈 명령을 보여 주고 확인을 받은 뒤 `confirmed { grade,
/// statementsHash, name? }` 로 다시 부른다.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DbKvConfirmRequired {
    pub grade: RiskGrade,
    pub action: DbKvAction,
    /// 서버에 갈 명령(보이기용). 인자마다 UTF-8 이면 글 그대로, 아니면 `{ "hex": "0x…" }` — 글 `"0xff"` 와 바이트 `0xff` 를
    /// 가른다. 지울 키가 많으면 모두 싣는다(상한 10,000).
    pub argv: Vec<KvKey>,
    pub statements_hash: String,
    /// 연결 이름을 쳐야 한다(프로덕션 연결의 파괴 등급 — Redis 분류에는 아직 파괴 등급이 없다).
    pub name_required: bool,
    /// 앞서 실은 `confirmed` 를 받지 않은 까닭(`hashMismatch` · `gradeTooLow` · `nameMismatch`). 처음 묻는 것이면 `null`.
    pub rejected: Option<String>,
}

// 메서드가 참조하지 않는 오류 `data` 의 타입 — 생성기에 따로 등록한다.
pw_rpc::rpc_type!(DbKvConfirmRequired);

rpc_method! {
    /// 키를 묶음으로 찾는다(스트림, `SCAN` + 묶음마다 타입 · TTL · 크기). 항목은 `accepted` → `keys` … → `done`. 그 세션을 연 프로젝트
    /// 창만 · 신뢰 · 연결 설정이 연 때와 같음. 취소는 `db.query.cancel { executionId }` 나 스트림 취소.
    pub DbKvScan: "db.kv.scan" {
        params: DbKvScanParams,
        stream: DbKvScanEvent,
        tier: Read,
        roles: [Owner],
    }

    /// 값 한 쪽(타입별, 쪽 하나 4 MiB 예산). 키가 없으면 `value` 가 `missing`.
    pub DbKvGet: "db.kv.get" {
        params: DbKvGetParams,
        result: KvValuePage,
        tier: Read,
        roles: [Owner],
    }

    /// TTL 을 바꾼다. `PEXPIRE` 와 같은 등급 — 확인(3001)이 필요하다(짧은 TTL 은 지우기와 같다). read-only 연결은 거절. 키가
    /// 없으면 `false`.
    pub DbKvSetTtl: "db.kv.setTtl" {
        params: DbKvSetTtlParams,
        result: bool,
        tier: Write,
        roles: [Owner],
    }

    /// TTL 을 지운다(`PERSIST` 와 같은 등급 — 쓰기, 프로덕션 연결은 확인). 키가 없거나 TTL 이 없었으면 `false`.
    pub DbKvPersist: "db.kv.persist" {
        params: DbKvPersistParams,
        result: bool,
        tier: Write,
        roles: [Owner],
    }

    /// 키를 지운다(확인 3001 이 필요하다 — `DEL` 과 같은 등급). 지운 수를 돌려준다.
    pub DbKvDelete: "db.kv.delete" {
        params: DbKvDeleteParams,
        result: DbKvDeleted,
        tier: Write,
        roles: [Owner],
    }

    /// 이름을 바꾼다. `overwrite` 면 `RENAME`(대상을 덮는다), 아니면 `RENAMENX`(대상이 있으면 바꾸지 않고 `targetExists`) —
    /// 둘 다 분류대로 확인(3001)이 필요하다.
    pub DbKvRename: "db.kv.rename" {
        params: DbKvRenameParams,
        result: KvRenameOutcome,
        tier: Write,
        roles: [Owner],
    }

    /// 명령 하나(콘솔). 분류(`pw_db_sql::kv_command`)로 관문을 지난다 — 거절 명령은 permission denied(`data.reason =
    /// "blockedCommand"`, `data.block` 은 `notAllowed` · `blocking` · `scanCount`), read-only 연결은 읽기만, 확인 명령은 3001.
    /// 키를 통째로 읽는 큰 명령은 드라이버가 보내기 전에 거절하고(3003, `data.error.code = "KEY_TOO_LARGE"`), 큰 응답은
    /// `truncated` 로 끊는다.
    pub DbKvCommand: "db.kv.command" {
        params: DbKvCommandParams,
        result: KvReply,
        tier: Exec,
        roles: [Owner],
    }
}

/// 관문을 지날 쓰기 하나: 서버에 보낼 것과 같은 명령, 그 분류의 등급, 확인.
struct Gated<'a> {
    action: DbKvAction,
    grade: RiskGrade,
    /// 명령(이름 포함, 바이트) — 해시의 재료이고 보이기용 명령의 원본이다.
    argv: Vec<Vec<u8>>,
    confirmed: Option<&'a DbConfirmed>,
}

impl<'a> Gated<'a> {
    /// 명령 하나를 분류기(`classify_argv` — 하나뿐이다)에 넣어 등급을 정한다. 전용 메서드도 따로 등급을 두지 않고 같은 효과의
    /// 명령을 넣는다. 거절 명령이면 `blockedCommand`. 조회면 `None`(관문의 확인 · read-only 판정이 없다).
    fn classify(
        action: DbKvAction,
        argv: Vec<Vec<u8>>,
        confirmed: Option<&'a DbConfirmed>,
    ) -> Result<Option<Self>, RpcError> {
        let grade = match classify_argv(&argv) {
            KvCommandClass::Blocked(reason) => {
                let name = argv
                    .first()
                    .map(|n| String::from_utf8_lossy(n).into_owned())
                    .unwrap_or_default();
                return Err(blocked(&name, reason));
            }
            class => class.grade().unwrap_or(RiskGrade::Destructive),
        };
        Ok((grade != RiskGrade::Read).then_some(Self {
            action,
            grade,
            argv,
            confirmed,
        }))
    }

    /// 보이기용 명령: 인자마다 UTF-8 이면 글, 아니면 hex(그 바이트 그대로 — 받은 꼴이 아니라 서버에 갈 것).
    fn shown(&self) -> Vec<KvKey> {
        self.argv.iter().map(|a| KvKey::from_bytes(a)).collect()
    }
}

/// 키를 서버에 보낼 바이트로. hex 가 틀렸으면 invalid params.
fn key_bytes(key: &KvKey) -> Result<Vec<u8>, RpcError> {
    key.to_bytes().ok_or_else(|| {
        RpcError::invalid_params("a key's hex must be `0x` and an even number of hex digits")
    })
}

fn too_large(what: &str, bytes: usize) -> RpcError {
    RpcError::invalid_params(format!(
        "the {what} is {bytes} bytes, over the {MAX_REQUEST_BYTES} byte limit"
    ))
}

/// 확인 해시: 세션 열쇠(세션 id · 확인 순번) · 메서드 · 인자. 길이를 앞에 붙여 이어 붙여도 경계가 섞이지 않는다.
fn kv_hash(confirm_key: &str, action: DbKvAction, parts: &[Vec<u8>]) -> String {
    use sha2::{Digest, Sha256};
    use std::fmt::Write as _;
    let mut hasher = Sha256::new();
    let mut part = |bytes: &[u8]| {
        hasher.update((bytes.len() as u64).to_le_bytes());
        hasher.update(bytes);
    };
    part(b"pitwall.db.kv.confirm.v1");
    part(confirm_key.as_bytes());
    part(action.tag());
    for p in parts {
        part(p);
    }
    let mut hex = String::with_capacity(64);
    for byte in hasher.finalize() {
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

/// read-only 거절(`PERMISSION_DENIED`, `data.reason = "readOnlyConnection"`).
fn read_only_refusal(action: DbKvAction) -> RpcError {
    RpcError::new(
        rpc_code::PERMISSION_DENIED,
        "this connection is read-only: only reads run",
    )
    .with_data(json!({ "reason": "readOnlyConnection", "action": action }))
}

/// 거절 명령(`PERMISSION_DENIED`, `data.reason = "blockedCommand"`).
fn blocked(command: &str, reason: BlockReason) -> RpcError {
    let (block, message) = match reason {
        BlockReason::NotAllowed => (
            "notAllowed",
            format!(
                "`{command}` is not allowed in the console (unknown, keyspace-wide or connection-state command)"
            ),
        ),
        BlockReason::Blocking => (
            "blocking",
            format!("`{command}` blocks the connection while it waits; it is not allowed"),
        ),
        BlockReason::ScanCount => (
            "scanCount",
            format!(
                "`{command}` with a COUNT over {MAX_SCAN_COUNT} is not allowed; use the key browser"
            ),
        ),
    };
    RpcError::new(rpc_code::PERMISSION_DENIED, message)
        .with_data(json!({ "reason": "blockedCommand", "block": block, "command": command }))
}

/// 등급 정책(결정 M3-D20, `crate::gate` 의 SQL 판정과 같은 규칙). read-only 연결은 조회 밖을 거절, 확인이 필요하면 3001.
/// 지나면 `Ok`. 해시는 지금의 세션 확인 순번으로 만든다 — 연결 잠금을 쥔 채로 부른다(그사이 다른 쓰기가 순번을 올리지 않게).
fn judge(session: &DbSession, gated: &Gated<'_>) -> Result<(), RpcError> {
    if session.read_only && gated.grade != RiskGrade::Read {
        return Err(read_only_refusal(gated.action));
    }
    let name_required = session.production && gated.grade == RiskGrade::Destructive;
    let needs_confirm = gated.grade >= RiskGrade::UnboundedWrite
        || (session.production && gated.grade >= RiskGrade::Write);
    if !needs_confirm {
        return Ok(());
    }
    let hash = kv_hash(&session.confirm_key(), gated.action, &gated.argv);
    let rejected = match gated.confirmed {
        None => None,
        Some(c) if c.statements_hash != hash => Some("hashMismatch"),
        Some(c) if c.grade < gated.grade => Some("gradeTooLow"),
        Some(c) if name_required && c.name.as_deref() != Some(session.name.as_str()) => {
            Some("nameMismatch")
        }
        Some(_) => return Ok(()),
    };
    let data = DbKvConfirmRequired {
        grade: gated.grade,
        action: gated.action,
        argv: gated.shown(),
        statements_hash: hash,
        name_required,
        rejected: rejected.map(str::to_owned),
    };
    Err(RpcError::new(
        code::CONFIRM_REQUIRED,
        "confirm before running this command",
    )
    .with_data(serde_json::to_value(data).unwrap_or_default()))
}

/// 세션의 KV 연결. 끊겼으면 3003(`disconnected`), SQL 연결이면 invalid params.
fn kv_conn(link: &mut Link) -> Result<&mut Box<dyn KvConnection>, RpcError> {
    match link.conn.as_mut() {
        Some(DbConnection::Kv(conn)) => Ok(conn),
        Some(DbConnection::Sql(_)) => Err(not_kv()),
        None => Err(db_error(DbError::new(
            DbErrorKind::Disconnected,
            "the connection is closed",
        ))),
    }
}

fn not_kv() -> RpcError {
    RpcError::invalid_params("this connection is not a key-value store")
}

/// 도는 KV 요청 하나가 세션의 실행 자리를 쥔 동안. 끝나거나 버려지면 자리를 비운다.
///
/// 버려질 때(요청 취소 · 스트림 취소) 인터럽트를 띄우지 않는다: 드라이버 호출 future 가 함께 버려지고, Redis 드라이버는 그때
/// 연결을 버린다(`pw_db_driver_redis` 의 `connection` 머리 주석). 늦게 도는 인터럽트는 그사이 자리를 얻은 다음 요청을 맞힌다.
struct Slot {
    session: Arc<DbSession>,
    running: Arc<Running>,
}

impl Slot {
    fn take(session: &Arc<DbSession>, running: &Arc<Running>) -> Self {
        *lock(&session.running) = Some(Arc::clone(running));
        Self {
            session: Arc::clone(session),
            running: Arc::clone(running),
        }
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        let mut slot = lock(&self.session.running);
        if slot.as_ref().is_some_and(|r| Arc::ptr_eq(r, &self.running)) {
            *slot = None;
        }
    }
}

/// 실행 하나에 묶인 드라이버 인터럽트. 그 실행이 세션의 실행 자리를 쥔 동안만 보낸다 — Redis 의 인터럽트는 연결에서 도는
/// 요청을 무엇이든 깨우므로, 끝난 실행의 늦은 취소가 다음 요청을 멈추지 않게.
///
/// 드라이버 인터럽트를 실행 자리 잠금을 쥔 채로 부른다(future 를 만드는 것까지). Redis 드라이버는 부를 때 곧바로 깨우므로
/// [`Slot`] 이 자리를 비우는 것과 겹치지 않는다. 기다리는 것(정리 시간)은 잠금을 놓은 뒤다.
struct BoundInterrupt {
    session: Weak<DbSession>,
    execution_id: String,
    inner: Arc<dyn Interrupt>,
}

impl Interrupt for BoundInterrupt {
    fn interrupt(&self) -> BoxFuture<'_, DbResult<()>> {
        let idle = || -> BoxFuture<'_, DbResult<()>> { Box::pin(std::future::ready(Ok(()))) };
        let Some(session) = self.session.upgrade() else {
            return idle();
        };
        let slot = lock(&session.running);
        if !slot
            .as_ref()
            .is_some_and(|r| r.execution_id == self.execution_id)
        {
            return idle();
        }
        let sent = self.inner.interrupt();
        drop(slot);
        sent
    }
}

/// 스캔 스트림이 버려지면(창의 스트림 취소 · 연결이 닫힘) 실행 표에서 빼고 멈춘다.
struct ScanGuard {
    sessions: Arc<DbSessions>,
    running: Arc<Running>,
}

impl Drop for ScanGuard {
    fn drop(&mut self) {
        lock(&self.sessions.executions).remove(&self.running.execution_id);
        self.running.stop.cancel();
    }
}

impl DbSessions {
    /// KV 세션: 그 세션을 연 프로젝트 창(관문 ①)인지, KV 연결인지 보고, 신뢰 · 연결 설정을 다시 본다(관문 ②).
    async fn kv_session(&self, ctx: &Ctx, session_id: &str) -> Result<Arc<DbSession>, RpcError> {
        let (session, _grant) = self.session_for(ctx, session_id)?;
        if !session.driver.capabilities.kv {
            return Err(not_kv());
        }
        self.recheck(&session).await?;
        Ok(session)
    }

    fn kv_running(&self, session: &Arc<DbSession>, conn: &dyn KvConnection) -> Arc<Running> {
        let execution_id = self.next_id("dbk-");
        let interrupt = Arc::new(BoundInterrupt {
            session: Arc::downgrade(session),
            execution_id: execution_id.clone(),
            inner: conn.interrupter(),
        });
        Arc::new(Running {
            execution_id,
            session_id: session.id.clone(),
            project_id: session.project_id.clone(),
            dialect: None,
            cancel: QueryCancel::new(interrupt, Budget::CONSOLE.cancel_cleanup),
            stop: CancellationToken::new(),
            current: std::sync::Mutex::new(None),
        })
    }

    /// 연결이 끊겼으면 세션을 「끊김」 으로 두고 알린다.
    fn kv_settle(&self, session: &DbSession, link: &mut Link, error: Option<&DbError>) {
        if error.is_some_and(|e| e.kind == DbErrorKind::Disconnected) {
            let notice = link.lose(false);
            session.sync(link);
            self.publish(session, Some(notice));
        }
    }

    /// 단항 KV 요청 하나: 연결 잠금을 기다려 쥐고, 쓰기면 등급 정책(확인)을 보고, 세션의 실행 자리에서 드라이버를 부른다.
    async fn kv_call<T, F>(
        &self,
        session: &Arc<DbSession>,
        gated: Option<Gated<'_>>,
        call: F,
    ) -> Result<T, RpcError>
    where
        T: Send,
        F: for<'c> FnOnce(&'c mut dyn KvConnection) -> BoxFuture<'c, DbResult<T>>,
    {
        let mut link = tokio::select! {
            link = session.link.lock() => link,
            () = session.closed.cancelled() => return Err(db_error(DbError::new(DbErrorKind::Disconnected, "the session is closed"))),
        };
        if let Some(gated) = &gated {
            judge(session, gated)?;
            // 쓰기가 관문을 지났다 — 낸 확인은 이것으로 끝이다(재사용 · 늦게 온 확인 거절).
            session.spend_confirmations();
            tracing::info!(target: "pw_db", session = %session.id, action = ?gated.action, grade = ?gated.grade, "running a kv write");
        }
        let conn = kv_conn(&mut link)?;
        let running = self.kv_running(session, conn.as_ref());
        let slot = Slot::take(session, &running);
        let result = running.cancel.run(call(conn.as_mut())).await;
        drop(slot);
        self.kv_settle(session, &mut link, result.as_ref().err());
        result.map_err(db_error)
    }

    /// `db.kv.scan`.
    pub(crate) async fn kv_scan(
        self: Arc<Self>,
        ctx: Ctx,
        params: DbKvScanParams,
        sink: StreamSink<DbKvScanEvent>,
    ) -> Result<(), RpcError> {
        let count = params.count.unwrap_or(DEFAULT_SCAN_COUNT);
        if count == 0 || u64::from(count) > MAX_SCAN_COUNT {
            return Err(RpcError::invalid_params(format!(
                "count must be 1-{MAX_SCAN_COUNT}"
            )));
        }
        if let Some(pattern) = &params.pattern
            && pattern.len() > MAX_PATTERN_BYTES
        {
            return Err(RpcError::invalid_params(format!(
                "the pattern is over {MAX_PATTERN_BYTES} bytes"
            )));
        }
        let session = self.kv_session(&ctx, &params.session_id).await?;
        let running = {
            let mut link = session.link.lock().await;
            let conn = kv_conn(&mut link)?;
            self.kv_running(&session, conn.as_ref())
        };
        lock(&self.executions).insert(running.execution_id.clone(), Arc::clone(&running));
        let _guard = ScanGuard {
            sessions: Arc::clone(&self),
            running: Arc::clone(&running),
        };
        tracing::info!(target: "pw_db", session = %session.id, execution = %running.execution_id, "scanning kv keys");
        let started = Instant::now();
        let mut keys = 0_u64;
        let mut reported = started;
        self.emit(
            &sink,
            &session,
            &running,
            DbKvScanEvent::Accepted {
                execution_id: running.execution_id.clone(),
            },
        )
        .await?;
        self.kv_progress(&session, &running, keys, started, false);
        let mut cursor = None;
        let mut first = true;
        loop {
            // 긴 스캔은 묶음마다 관문을 다시 지난다: 그 창의 세션인지(닫혔으면 not found), 신뢰, 연결 설정.
            if !std::mem::take(&mut first) {
                let again = self.kv_session(&ctx, &params.session_id).await?;
                if !Arc::ptr_eq(&again, &session) {
                    return Err(db_error(DbError::new(
                        DbErrorKind::Disconnected,
                        "the session is closed",
                    )));
                }
            }
            let request = KvScanRequest {
                pattern: params.pattern.clone(),
                cursor: cursor.take(),
                count,
            };
            let page = self.scan_page(&session, &running, request).await?;
            keys += page.keys.len() as u64;
            cursor = page.next_cursor;
            if !page.keys.is_empty() {
                self.emit(
                    &sink,
                    &session,
                    &running,
                    DbKvScanEvent::Keys { keys: page.keys },
                )
                .await?;
            }
            if cursor.is_none() {
                break;
            }
            if reported.elapsed() >= PROGRESS_EVERY {
                reported = Instant::now();
                self.kv_progress(&session, &running, keys, started, false);
            }
        }
        self.kv_progress(&session, &running, keys, started, true);
        self.emit(
            &sink,
            &session,
            &running,
            DbKvScanEvent::Done {
                keys,
                elapsed_ms: started.elapsed().as_millis() as u64,
            },
        )
        .await
    }

    /// 스캔 한 묶음: 연결 잠금은 이 묶음을 읽는 동안만 쥔다.
    async fn scan_page(
        &self,
        session: &Arc<DbSession>,
        running: &Arc<Running>,
        request: KvScanRequest,
    ) -> Result<pw_db_types::KvScanPage, RpcError> {
        let mut link = tokio::select! {
            link = session.link.lock() => link,
            () = running.stop.cancelled() => return Err(RpcError::cancelled()),
            () = session.closed.cancelled() => return Err(db_error(DbError::new(DbErrorKind::Disconnected, "the session is closed"))),
        };
        if running.stop.is_cancelled() {
            return Err(RpcError::cancelled());
        }
        let conn = kv_conn(&mut link)?;
        let slot = Slot::take(session, running);
        let result = running.cancel.run(conn.scan(request)).await;
        drop(slot);
        self.kv_settle(session, &mut link, result.as_ref().err());
        match result {
            Ok(page) => Ok(page),
            Err(e) if e.kind == DbErrorKind::Cancelled => Err(RpcError::cancelled()),
            Err(e) => Err(db_error(e)),
        }
    }

    /// 항목 하나를 보낸다(창의 credit 을 기다린다). 그동안 취소되면 `CANCELLED`, 세션이 닫히면(`db.session.close`) `disconnected`.
    async fn emit(
        &self,
        sink: &StreamSink<DbKvScanEvent>,
        session: &DbSession,
        running: &Running,
        event: DbKvScanEvent,
    ) -> Result<(), RpcError> {
        tokio::select! {
            biased;
            () = running.stop.cancelled() => Err(RpcError::cancelled()),
            () = session.closed.cancelled() => Err(db_error(DbError::new(DbErrorKind::Disconnected, "the session is closed"))),
            sent = sink.send(event) => sent,
        }
    }

    fn kv_progress(
        &self,
        session: &DbSession,
        running: &Running,
        keys: u64,
        started: Instant,
        finished: bool,
    ) {
        let Some(bus) = self.bus() else { return };
        let payload = DbQueryProgress {
            session_id: session.id.clone(),
            execution_id: running.execution_id.clone(),
            statement_index: 0,
            rows: keys,
            elapsed_ms: started.elapsed().as_millis() as u64,
            finished,
        };
        let _ = bus.publish(
            pw_core::bus::Scope::Project(session.project_id.clone()),
            TOPIC_QUERY_PROGRESS,
            &payload,
        );
    }

    /// `db.kv.get`.
    pub(crate) async fn kv_get(
        &self,
        ctx: &Ctx,
        params: DbKvGetParams,
    ) -> Result<KvValuePage, RpcError> {
        let range = params.range.unwrap_or(KvRange {
            cursor: None,
            count: DEFAULT_READ_COUNT,
        });
        if range.count == 0 || range.count > MAX_READ_COUNT {
            return Err(RpcError::invalid_params(format!(
                "range.count must be 1-{MAX_READ_COUNT}"
            )));
        }
        if range
            .cursor
            .as_ref()
            .is_some_and(|c| c.chars().count() > MAX_CURSOR_CHARS)
        {
            return Err(RpcError::invalid_params("range.cursor is too long"));
        }
        check_key_size(&params.key)?;
        let session = self.kv_session(ctx, &params.session_id).await?;
        let key = params.key;
        self.kv_call(&session, None, move |c| c.read(key, range))
            .await
    }

    /// `db.kv.setTtl`: `PEXPIRE <key> <ttlMs>` 의 등급.
    pub(crate) async fn kv_set_ttl(
        &self,
        ctx: &Ctx,
        params: DbKvSetTtlParams,
    ) -> Result<bool, RpcError> {
        if params.ttl_ms == 0 || params.ttl_ms > i64::MAX as u64 {
            return Err(RpcError::invalid_params(
                "ttlMs must be at least 1 (use db.kv.delete to remove a key, db.kv.persist to drop the TTL)",
            ));
        }
        let key = key_bytes(&params.key)?;
        check_size("key", key.len())?;
        let session = self.kv_session(ctx, &params.session_id).await?;
        let gated = Gated::classify(
            DbKvAction::SetTtl,
            vec![
                b"PEXPIRE".to_vec(),
                key,
                params.ttl_ms.to_string().into_bytes(),
            ],
            params.confirmed.as_ref(),
        )?;
        let (target, ttl) = (params.key.clone(), params.ttl_ms);
        self.kv_call(&session, gated, move |c| c.set_ttl(target, ttl))
            .await
    }

    /// `db.kv.persist`: `PERSIST <key>` 의 등급.
    pub(crate) async fn kv_persist(
        &self,
        ctx: &Ctx,
        params: DbKvPersistParams,
    ) -> Result<bool, RpcError> {
        let key = key_bytes(&params.key)?;
        check_size("key", key.len())?;
        let session = self.kv_session(ctx, &params.session_id).await?;
        let gated = Gated::classify(
            DbKvAction::Persist,
            vec![b"PERSIST".to_vec(), key],
            params.confirmed.as_ref(),
        )?;
        let target = params.key.clone();
        self.kv_call(&session, gated, move |c| c.persist(target))
            .await
    }

    /// `db.kv.delete`: `DEL <key>…` 의 등급.
    pub(crate) async fn kv_delete(
        &self,
        ctx: &Ctx,
        params: DbKvDeleteParams,
    ) -> Result<DbKvDeleted, RpcError> {
        if params.keys.is_empty() || params.keys.len() > MAX_DELETE_KEYS {
            return Err(RpcError::invalid_params(format!(
                "keys must hold 1-{MAX_DELETE_KEYS} keys"
            )));
        }
        let keys = params
            .keys
            .iter()
            .map(key_bytes)
            .collect::<Result<Vec<_>, _>>()?;
        check_size("key list", keys.iter().map(Vec::len).sum())?;
        let session = self.kv_session(ctx, &params.session_id).await?;
        let mut argv = vec![b"DEL".to_vec()];
        argv.extend(keys);
        let gated = Gated::classify(DbKvAction::Delete, argv, params.confirmed.as_ref())?;
        let keys = params.keys.clone();
        let deleted = self
            .kv_call(&session, gated, move |c| c.delete(keys))
            .await?;
        Ok(DbKvDeleted { deleted })
    }

    /// `db.kv.rename`: `RENAME <from> <to>`(덮는다) · `RENAMENX <from> <to>` 의 등급.
    pub(crate) async fn kv_rename(
        &self,
        ctx: &Ctx,
        params: DbKvRenameParams,
    ) -> Result<KvRenameOutcome, RpcError> {
        let (from, to) = (key_bytes(&params.from)?, key_bytes(&params.to)?);
        check_size("key pair", from.len() + to.len())?;
        let session = self.kv_session(ctx, &params.session_id).await?;
        let name: &[u8] = if params.overwrite {
            b"RENAME"
        } else {
            b"RENAMENX"
        };
        let gated = Gated::classify(
            DbKvAction::Rename,
            vec![name.to_vec(), from, to],
            params.confirmed.as_ref(),
        )?;
        let (a, b, overwrite) = (params.from.clone(), params.to.clone(), params.overwrite);
        self.kv_call(&session, gated, move |c| c.rename(a, b, overwrite))
            .await
    }

    /// `db.kv.command`.
    pub(crate) async fn kv_command(
        &self,
        ctx: &Ctx,
        params: DbKvCommandParams,
    ) -> Result<KvReply, RpcError> {
        let Some(name) = params.argv.first() else {
            return Err(RpcError::invalid_params("argv is empty"));
        };
        check_size("command", params.argv.iter().map(String::len).sum())?;
        let session = self.kv_session(ctx, &params.session_id).await?;
        let gated = Gated::classify(
            DbKvAction::Command,
            params.argv.iter().map(|a| a.as_bytes().to_vec()).collect(),
            params.confirmed.as_ref(),
        )
        .inspect_err(|_| {
            tracing::info!(target: "pw_db", session = %session.id, command = %name, "kv command refused");
        })?;
        let argv = params.argv.clone();
        self.kv_call(&session, gated, move |c| c.command(argv))
            .await
    }
}

fn check_size(what: &str, bytes: usize) -> Result<(), RpcError> {
    if bytes > MAX_REQUEST_BYTES {
        return Err(too_large(what, bytes));
    }
    Ok(())
}

fn check_key_size(key: &KvKey) -> Result<(), RpcError> {
    check_size("key", key_bytes(key)?.len())
}

pub(crate) fn register(router: &mut Router, sessions: Arc<DbSessions>) {
    let (scan, get, ttl, persist, delete, rename, command) = (
        Arc::clone(&sessions),
        Arc::clone(&sessions),
        Arc::clone(&sessions),
        Arc::clone(&sessions),
        Arc::clone(&sessions),
        Arc::clone(&sessions),
        sessions,
    );
    router
        .stream(DbKvScan, move |ctx, params: DbKvScanParams, sink| {
            Arc::clone(&scan).kv_scan(ctx, params, sink)
        })
        .unary(DbKvGet, move |ctx, params: DbKvGetParams| {
            let s = Arc::clone(&get);
            async move { s.kv_get(&ctx, params).await }
        })
        .unary(DbKvSetTtl, move |ctx, params: DbKvSetTtlParams| {
            let s = Arc::clone(&ttl);
            async move { s.kv_set_ttl(&ctx, params).await }
        })
        .unary(DbKvPersist, move |ctx, params: DbKvPersistParams| {
            let s = Arc::clone(&persist);
            async move { s.kv_persist(&ctx, params).await }
        })
        .unary(DbKvDelete, move |ctx, params: DbKvDeleteParams| {
            let s = Arc::clone(&delete);
            async move { s.kv_delete(&ctx, params).await }
        })
        .unary(DbKvRename, move |ctx, params: DbKvRenameParams| {
            let s = Arc::clone(&rename);
            async move { s.kv_rename(&ctx, params).await }
        })
        .unary(DbKvCommand, move |ctx, params: DbKvCommandParams| {
            let s = Arc::clone(&command);
            async move { s.kv_command(&ctx, params).await }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hash_binds_the_session_the_action_and_every_argument() {
        let base = kv_hash("dbs-1#0", DbKvAction::Delete, &[b"a".to_vec()]);
        assert_eq!(base.len(), 64);
        assert_eq!(
            base,
            kv_hash("dbs-1#0", DbKvAction::Delete, &[b"a".to_vec()])
        );
        for other in [
            kv_hash("dbs-2#0", DbKvAction::Delete, &[b"a".to_vec()]),
            kv_hash("dbs-1#1", DbKvAction::Delete, &[b"a".to_vec()]),
            kv_hash("dbs-1#0", DbKvAction::Command, &[b"a".to_vec()]),
            kv_hash("dbs-1#0", DbKvAction::Delete, &[b"b".to_vec()]),
            // 경계가 섞이지 않는다: ["ab"] 와 ["a", "b"] 는 다르다.
            kv_hash(
                "dbs-1#0",
                DbKvAction::Delete,
                &[b"a".to_vec(), b"b".to_vec()],
            ),
        ] {
            assert_ne!(base, other);
        }
        assert_ne!(
            kv_hash("s#0", DbKvAction::Delete, &[b"ab".to_vec()]),
            kv_hash("s#0", DbKvAction::Delete, &[b"a".to_vec(), b"b".to_vec()]),
        );
    }

    fn grade(argv: &[&[u8]]) -> Option<RiskGrade> {
        let argv = argv.iter().map(|a| a.to_vec()).collect();
        Gated::classify(DbKvAction::Command, argv, None)
            .unwrap()
            .map(|g| g.grade)
    }

    /// 전용 메서드는 같은 효과의 명령을 분류기에 넣는다 — 키가 UTF-8 이 아니어도 같은 등급이다.
    #[test]
    fn the_dedicated_methods_take_the_grade_of_their_command() {
        let confirm = Some(RiskGrade::UnboundedWrite);
        assert_eq!(grade(&[b"PEXPIRE", b"\xffk", b"1"]), confirm);
        assert_eq!(grade(&[b"DEL", b"a", b"\xff"]), confirm);
        assert_eq!(grade(&[b"RENAME", b"a", b"b"]), confirm);
        assert_eq!(grade(&[b"RENAMENX", b"a", b"b"]), confirm);
        assert_eq!(grade(&[b"PERSIST", b"\xff"]), Some(RiskGrade::Write));
        assert_eq!(grade(&[b"GET", b"a"]), None, "reads pass no gate");
        let refused = Gated::classify(DbKvAction::Command, vec![b"FLUSHALL".to_vec()], None);
        assert!(refused.is_err_and(|e| e.code == rpc_code::PERMISSION_DENIED));
    }

    /// 보이기용 명령은 글 `"0xff"` 와 바이트 `0xff` 를 가른다.
    #[test]
    fn the_shown_command_tells_text_from_bytes() {
        let gated = Gated::classify(
            DbKvAction::Delete,
            vec![b"DEL".to_vec(), b"0xff".to_vec(), vec![0xff]],
            None,
        )
        .unwrap()
        .unwrap();
        let shown = serde_json::to_value(gated.shown()).unwrap();
        assert_eq!(shown, json!(["DEL", "0xff", { "hex": "0xff" }]));
    }
}
