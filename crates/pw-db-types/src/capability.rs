//! 드라이버 지원 플래그. 플래그마다 어느 갈래 trait · 어느 기능을 주는지가 정해져 있고, 창은 플래그로 기능을 켜고 끈다.
//!
//! 갈래는 둘 중 하나다 — `sql`(`SqlConnection`)이나 `kv`(`KvConnection`). 나머지 플래그는 그 갈래의 선택 기능이며, 꺼진
//! 기능의 trait 메서드는 「지원하지 않음」(`DbErrorKind::Unsupported`)으로 답한다.

use serde::{Deserialize, Serialize};

use crate::spec::{SqlDialect, SslMode};

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct Capabilities {
    /// 갈래 `SqlConnection`: 문장 실행 · 행 커서 · 트랜잭션. 창 — SQL 콘솔 · 테이블 데이터 · 스키마 탐색기.
    pub sql: bool,
    /// 갈래 `KvConnection`: 키 스캔 · 값 읽기 · TTL · 명령. 창 — Redis 화면.
    pub kv: bool,
    /// 행을 서버 커서에서 읽는 만큼만 받아 온다(PG 포털 · MySQL 결과 집합 스트리밍 · SQLite 문장 걷기). 꺼져 있으면 드라이버가
    /// 결과를 다 받아 둔 뒤 나눠 준다 — 파이프라인은 같게 부르지만 큰 결과의 메모리 · 첫 화면 시간이 보장되지 않는다.
    pub server_cursor: bool,
    /// 진행 중 실행을 서버에서 멈춘다(`Interrupt`). 창 — ■ 취소(`⌘F2`).
    pub cancel: bool,
    /// `begin` · `commit` · `rollback`. 창 — TX 자동 / 수동 전환과 [커밋] [롤백].
    pub transactions: bool,
    /// 세이브포인트(`savepoint` · `rollbackToSavepoint` · `releaseSavepoint`). 수동 커밋 탭의 그리드 편집 적용(M3-12).
    pub savepoints: bool,
    /// 읽기 전용 트랜잭션(`begin { readOnly }`). read-only 연결의 실행은 이 안에서만 돈다(결정 M3-D6) — 꺼진 드라이버의
    /// read-only 연결은 실행 관문이 거절한다.
    pub read_only_transaction: bool,
    /// 실행 계획(`explain`). 창 — 실행 계획 액션.
    pub explain: bool,
    /// 메타데이터 조회(객체 목록 · 열 · 인덱스 · FK · DDL). 창 — 스키마 탐색기 · `⌘B` DDL · 그리드 편집의 키 판정.
    pub metadata: bool,
    /// 스키마 단계가 있다(PG). 꺼져 있으면 탐색기가 DB 아래에 곧바로 객체를 놓는다(MySQL · SQLite).
    pub schemas: bool,
    /// 한 서버에 DB 가 여럿이고 고를 수 있다(`databases`). 창 — 콘솔 위 막대의 DB 고르기 · Redis DB 번호 전환.
    pub multiple_databases: bool,
    /// 이 드라이버가 받는 `sslmode` 값. 연결 편집 다이얼로그(M3-14)는 이 밖의 값을 고를 수 없게 한다. 파일 DB 는 빈 목록.
    pub ssl_modes: Vec<SslMode>,
}

impl Capabilities {
    /// 갈래가 정확히 하나인지. 둘 다거나 둘 다 아니면 그 이유.
    pub fn validate(&self) -> Result<(), &'static str> {
        match (self.sql, self.kv) {
            (true, false) | (false, true) => {}
            (true, true) => return Err("a driver is either sql or kv, not both"),
            (false, false) => return Err("a driver must be sql or kv"),
        }
        if self.kv
            && (self.transactions
                || self.savepoints
                || self.read_only_transaction
                || self.explain
                || self.schemas)
        {
            return Err("transactions, savepoints, explain and schemas are sql features");
        }
        if self.savepoints && !self.transactions {
            return Err("savepoints need transactions");
        }
        if self.read_only_transaction && !self.transactions {
            return Err("read-only transactions need transactions");
        }
        Ok(())
    }
}

/// 등록된 드라이버 하나(드라이버 등록부 — `pw_db::DriverRegistry`).
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DriverInfo {
    /// 설정 파일의 `driver` 값.
    pub id: String,
    /// SQL 갈래의 방언. KV 드라이버는 `None`.
    pub dialect: Option<SqlDialect>,
    pub capabilities: Capabilities,
}
