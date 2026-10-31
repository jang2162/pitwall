//! SQL 실행의 입출력: 문장 실행 요청, 열 메타, 행 묶음, 트랜잭션 상태, 실행 계획, 연결 결과.
//!
//! 이것은 드라이버 trait 의 입출력이다. 창이 받는 스트림 항목(`started` → `columns` → `rows` … → `done`, 결정 M3-D7)은 실행
//! 파이프라인(M3-05)이 이 타입들로 만든다.

use pw_rpc::JsNumber;
use serde::{Deserialize, Serialize};

use crate::value::{CellLimit, DbValue};

/// 문장 하나의 실행 요청. 스크립트를 문장으로 나누는 것은 파이프라인이다(M3-04) — 드라이버는 늘 한 문장을 받는다.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecuteRequest {
    pub sql: String,
    /// 바인드 값(그리드 편집의 DML — M3-12). 값을 SQL 글에 이어 붙이지 않는다. 자리 표시 문법은 방언의 것(`$1` · `?`).
    pub params: Vec<DbValue>,
    /// 셀 하나의 상한. 넘는 값은 [`DbValue::Truncated`] · 잘린 [`DbValue::Bytes`] 로 온다.
    #[serde(with = "cell_limit")]
    pub cell_limit: CellLimit,
}

impl ExecuteRequest {
    pub fn sql(sql: impl Into<String>) -> Self {
        Self {
            sql: sql.into(),
            params: Vec::new(),
            cell_limit: CellLimit::DEFAULT,
        }
    }
}

mod cell_limit {
    use serde::{Deserialize, Deserializer, Serializer};

    use crate::value::CellLimit;

    pub fn serialize<S: Serializer>(limit: &CellLimit, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_u64(limit.0 as u64)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<CellLimit, D::Error> {
        let bytes = u64::deserialize(d)?;
        Ok(CellLimit(usize::try_from(bytes).unwrap_or(usize::MAX)))
    }
}

/// 결과 열 하나.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct ColumnMeta {
    pub name: String,
    /// DB 가 부르는 타입 이름(`int8` · `BIGINT UNSIGNED` · `TEXT` …).
    pub db_type: String,
    /// 열이 NULL 을 받을 수 있는지. 드라이버가 모르면 `None`.
    pub nullable: Option<bool>,
}

/// 행 하나. 값의 순서는 열의 순서다.
pub type Row = Vec<DbValue>;

/// 문장을 시작한 결과. 행이 있는 문장이면 커서가 열려 있고 [`FetchLimit`] 만큼씩 읽는다.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct StatementStarted {
    /// 결과 열. 행을 돌려주지 않는 문장(`INSERT` 등)은 빈 목록이고 커서도 없다 — 곧바로 [`StatementStarted::end`] 가 있다.
    pub columns: Vec<ColumnMeta>,
    /// 행이 없는 문장의 끝. `Some` 이면 읽을 커서가 없다.
    pub end: Option<StatementEnd>,
}

/// 한 번에 읽을 양. 둘 중 먼저 닿는 쪽에서 멈춘다(첫 묶음은 500행 또는 256 KiB — 결정 M3-D7).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct FetchLimit {
    pub max_rows: u32,
    /// 행 값의 대략 크기 합(바이트). 한 행은 이것을 넘어도 통째로 온다.
    pub max_bytes: u32,
}

impl FetchLimit {
    /// 첫 화면 몫(결정 M3-D7).
    pub const FIRST_SCREEN: Self = Self {
        max_rows: 500,
        max_bytes: 256 * 1024,
    };

    pub const fn rows(max_rows: u32) -> Self {
        Self {
            max_rows,
            max_bytes: u32::MAX,
        }
    }
}

/// 커서에서 읽은 행 묶음.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RowBatch {
    pub rows: Vec<Row>,
    /// 커서가 다 읽혔으면 그 끝. `None` 이면 더 읽을 행이 있을 수 있다.
    pub end: Option<StatementEnd>,
}

/// 문장의 끝.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct StatementEnd {
    /// 바뀐 행 수(`UPDATE` 등). 드라이버가 알려 주지 않는 문장은 `None`. 2^53 을 넘지 않는다고 본다.
    #[specta(type = Option<JsNumber>)]
    pub rows_affected: Option<u64>,
}

/// 연결이 보는 트랜잭션 상태. 세션 상태(`disconnected` · `configChanged` 등 — M3-05)는 이것 위에 얹는다.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum TransactionState {
    /// 열린 트랜잭션이 없다(자동 커밋).
    Idle,
    InTransaction,
    /// 오류 · 취소 뒤 롤백만 되는 트랜잭션(PG).
    Failed,
}

/// 트랜잭션 시작 옵션.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BeginOptions {
    /// 읽기 전용 트랜잭션(PG `BEGIN READ ONLY` · MySQL `START TRANSACTION READ ONLY` · SQLite `query_only`, 결정 M3-D6).
    pub read_only: bool,
}

/// 실행 계획.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct ExplainPlan {
    pub format: PlanFormat,
    /// 드라이버가 받은 계획 그대로(PG · MySQL 은 JSON 글).
    pub text: String,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum PlanFormat {
    Json,
    Text,
}

/// 연결의 TLS 상태(10-ui.md 의 「서버 인증서를 검증하지 않음」 · 「평문」 표지 — M3-19).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum TlsStatus {
    /// TLS, 인증서를 검증했다.
    Verified,
    /// TLS, 인증서를 검증하지 않았다(`require` · `prefer` · `allow`) 또는 이름을 보지 않았다(`verify-ca`).
    Unverified,
    /// TLS 없음.
    Plaintext,
    /// 파일 DB(SQLite) — 네트워크가 없다.
    NotApplicable,
}

/// 연결을 연 결과(연결 테스트의 답이기도 하다).
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct ServerInfo {
    /// 서버가 알린 판본(`16.4` · `8.4.6` · `11.8.3-MariaDB` …).
    pub version: Option<String>,
    pub tls: TlsStatus,
}
