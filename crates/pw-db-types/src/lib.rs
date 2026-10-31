//! DB 계층의 공용 타입(`docs/design/02-tech-stack.md` §2.7, 14-m3-plan.md 결정 M3-D3 · M3-D8).
//!
//! 드라이버 trait(`crates/pw-db-driver`)의 입출력은 모두 이 크레이트의 serde 타입이다 — 드라이버 크레이트의 타입이 trait 밖으로
//! 새지 않아, 프로세스 밖 구현(JDBC 브리지 F6-3b · 외부 확장 M8)이 같은 trait 를 직렬화로 프록시할 수 있다.
//!
//! - [`value`]: 셀 값 [`DbValue`] — JS 안전 정수 · 문자열 decimal · hex 바이트 · 셀 상한.
//! - [`spec`]: 드라이버가 받는 연결 설정 [`ConnectionSpec`], 드라이버 · 방언 · `sslmode` 의 이름.
//! - [`result`]: 문장 실행 요청 · 열 메타 · 행 묶음 · 트랜잭션 상태 · 실행 계획 · TLS 상태.
//! - [`meta`]: 메타데이터 DTO(쪽 단위 객체 목록 · 열 · 인덱스 · FK · DDL).
//! - [`kv`]: Redis 스캔 · 값 · 명령 응답.
//! - [`capability`]: 지원 플래그 [`Capabilities`].
//! - [`error`]: 드라이버 오류 [`DbError`].
//!
//! 창이 쓸 타입은 `rpc_type!` 로 생성기에 등록돼 `packages/rpc-client` 의 TS 타입이 된다(메서드가 아직 참조하지 않아도).
//! [`ConnectionSpec`] 은 비밀을 담으므로 등록하지 않는다.

pub mod capability;
pub mod error;
pub mod kv;
pub mod meta;
pub mod result;
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
