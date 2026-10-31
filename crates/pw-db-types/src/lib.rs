//! DB 계층의 공용 타입(`docs/design/02-tech-stack.md` §2.7, 14-m3-plan.md 결정 M3-D3 · M3-D8).
//!
//! 드라이버 trait(`crates/pw-db-driver`)의 입출력은 모두 이 크레이트의 serde 타입이다 — 드라이버 크레이트의 타입이 trait 밖으로
//! 새지 않아, 프로세스 밖 구현(JDBC 브리지 F6-3b · 외부 확장 M8)이 같은 trait 를 직렬화로 프록시할 수 있다.
//!
//! - [`value`]: 셀 값 [`DbValue`] — JS 안전 정수 · 문자열 decimal · hex 바이트 · 셀 상한.
//! - [`spec`]: 드라이버가 받는 연결 설정 [`ConnectionSpec`], 드라이버 · 방언 · `sslmode` 의 이름.
//! - [`risk`]: 문장 위험도 등급 [`RiskGrade`] — 값은 M3-03 의 분류기가 매긴다.
//! - [`result`]: 문장 실행 요청 · 열 메타 · 행 묶음 · 트랜잭션 상태 · 실행 계획 · TLS 상태.
//! - [`meta`]: 메타데이터 DTO(쪽 단위 객체 목록 · 열 · 인덱스 · FK · DDL).
//! - [`kv`]: Redis 스캔 · 값 · 명령 응답.
//! - [`capability`]: 지원 플래그 [`Capabilities`].
//! - [`error`]: 드라이버 오류 [`DbError`].
//!
//! **크기 · 개수 · TTL 필드는 값 규칙의 예외다**: [`DbValue`] 의 정수는 ±(2^53−1) 밖이면 문자열이지만, 바이트 크기
//! (`DbBytes::size` · `DbTruncated::size`) · 바뀐 행 수(`StatementEnd::rows_affected`) · Redis 의 `ttl_ms` · `size` · `total` 은
//! `u64` 를 TS `number` 로 낸다 — 2^53(약 9 페타바이트 · 9천조 행 · 28만 년)을 넘지 않는다고 본다.
//!
//! 창이 쓸 타입은 `rpc_type!` 로 생성기에 등록돼 `packages/rpc-client` 의 TS 타입이 된다(메서드가 아직 참조하지 않아도).
//! [`ConnectionSpec`] 은 비밀을 담으므로 등록하지 않는다.

pub mod capability;
pub mod error;
pub mod kv;
pub mod meta;
pub mod result;
pub mod risk;
pub mod spec;
pub mod value;

pub use capability::{Capabilities, DriverInfo};
pub use error::{DbError, DbErrorKind};
pub use kv::{
    KvField, KvKey, KvKeyBytes, KvKeyInfo, KvRange, KvRenameOutcome, KvReply, KvReplyPair,
    KvScanPage, KvScanRequest, KvScored, KvStreamEntry, KvType, KvValue, KvValuePage,
};
pub use meta::{
    ColumnInfo, DatabaseInfo, Ddl, ForeignKeyInfo, IndexInfo, ObjectInfo, ObjectKind, ObjectPage,
    ObjectQuery, ObjectRef, SchemaInfo,
};
pub use result::{
    BeginOptions, ColumnMeta, ExecuteRequest, ExplainPlan, FetchLimit, PlanFormat, Row, RowBatch,
    ServerInfo, StatementEnd, StatementStarted, TlsStatus, TransactionState,
};
pub use risk::RiskGrade;
pub use spec::{
    ConnectionSpec, DriverKind, Secret, SocketTarget, SqlDialect, SslMode, Target, TlsSettings,
};
pub use value::{
    CellLimit, DbBytes, DbFloat, DbInt, DbOther, DbTruncated, DbValue, MAX_SAFE_INTEGER,
    TruncatedKind,
};

// 창이 쓰는 타입. 메서드(M3-02 이후)가 참조하기 전에도 TS 로 나간다.
pw_rpc::rpc_type!(
    DbValue,
    DbError,
    RiskGrade,
    Capabilities,
    DriverInfo,
    DriverKind,
    SqlDialect,
    SslMode,
    ColumnMeta,
    RowBatch,
    StatementStarted,
    StatementEnd,
    TransactionState,
    ExplainPlan,
    TlsStatus,
    ServerInfo,
    DatabaseInfo,
    SchemaInfo,
    ObjectQuery,
    ObjectPage,
    ColumnInfo,
    IndexInfo,
    ForeignKeyInfo,
    Ddl,
    KvScanRequest,
    KvScanPage,
    KvRange,
    KvValuePage,
    KvReply,
    KvRenameOutcome,
);
