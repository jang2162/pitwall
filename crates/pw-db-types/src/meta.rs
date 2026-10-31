//! 메타데이터 DTO: DB · 스키마 · 객체 목록(쪽 단위) · 열 · 인덱스 · FK · DDL.
//!
//! 드라이버가 trait 메서드로 채우고(M3-06 ~ M3-08) 스키마 탐색 RPC 와 캐시(M3-10)가 그대로 내놓는다. 목록은 쪽 단위다 —
//! 테이블 1만 개 스키마에서도 첫 쪽만 읽는다(결정 M3-D17).

use serde::{Deserialize, Serialize};

/// DB(카탈로그) 하나.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DatabaseInfo {
    pub name: String,
}

/// 스키마 하나(PG). MySQL 은 DB 가 곧 스키마라 스키마 목록이 없다(`Capabilities::schemas` 가 `false`).
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct SchemaInfo {
    pub name: String,
}

/// 객체 종류.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    Serialize,
    Deserialize,
    specta::Type,
    schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum ObjectKind {
    Table,
    View,
    MaterializedView,
    Sequence,
    Function,
    Procedure,
    Trigger,
}

/// 객체를 가리키는 이름. 세션의 `USE` · `search_path` 에 기대지 않게 늘 한정한다(M3-12).
#[derive(
    Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct ObjectRef {
    pub database: Option<String>,
    pub schema: Option<String>,
    pub name: String,
}

/// 객체 목록 한 쪽의 요청.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct ObjectQuery {
    pub database: Option<String>,
    pub schema: Option<String>,
    pub kind: ObjectKind,
    /// 이름에 이 글이 든 것만(서버 쪽 필터, 대소문자 무시).
    pub filter: Option<String>,
    /// 앞 쪽의 [`ObjectPage::next_cursor`]. 처음 쪽은 `None`.
    pub cursor: Option<String>,
    /// 쪽 크기(1 ~ 500).
    pub limit: u32,
}

/// 객체 목록의 한 쪽.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct ObjectPage {
    pub items: Vec<ObjectInfo>,
    /// 다음 쪽이 있으면 그 커서(드라이버마다 뜻이 다르다 — 창은 그대로 돌려줄 뿐이다).
    pub next_cursor: Option<String>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct ObjectInfo {
    pub name: String,
    pub kind: ObjectKind,
    pub comment: Option<String>,
}

/// 테이블 · 뷰의 열.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct ColumnInfo {
    pub name: String,
    pub db_type: String,
    pub nullable: bool,
    /// 기본값 식의 글.
    pub default: Option<String>,
    pub comment: Option<String>,
}

/// 인덱스. 그리드 편집의 키 판정(M3-10 — PK 또는 NOT NULL 열만으로 된 유일 인덱스)이 `primary` · `unique` · `partial` 을 본다.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct IndexInfo {
    pub name: String,
    pub columns: Vec<String>,
    pub primary: bool,
    pub unique: bool,
    /// 조건이 붙은 인덱스(PG 부분 인덱스). 키로 쓰지 않는다.
    pub partial: bool,
}

/// 외래 키.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct ForeignKeyInfo {
    pub name: String,
    pub columns: Vec<String>,
    pub references: ObjectRef,
    pub referenced_columns: Vec<String>,
}

/// 객체의 DDL 글.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct Ddl {
    pub text: String,
}
