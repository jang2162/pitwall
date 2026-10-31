//! 그리드 편집 DML(M3-12, 결정 M3-D9): 테이블 · 행 키 · 바뀐 셀 · 추가 행 · 지운 행 → 방언별 `DELETE` · `UPDATE` · `INSERT`.
//!
//! - **값은 바인드**다 — SQL 글에 값을 이어 붙이지 않는다. 자리 표시는 방언의 것(PG `$1`, MySQL · SQLite `?`).
//! - **미리보기 글**은 같은 문장에 값을 리터럴로 적은 판이다. 리터럴은 [`crate::literal`](M3-16 의 정본 — 서버 설정과 무관한 꼴)
//!   이고, 바인드 판과 미리보기 판을 **한 조각 목록에서 함께** 만든다 — 두 판의 구조가 갈리지 않는다. 리터럴로 적을 수 없는 값
//!   (잘린 값 · 방언에 없는 값 · PG 의 NUL)은 바인드로는 들어가더라도 거절한다 — 사람이 본 글과 서버가 받은 값이 같아야 한다.
//! - **테이블은 늘 한정한다** — 세션의 `USE` · `search_path` 에 기대지 않는다: PG 는 `"db"."schema"."t"`(DB 가 연결과 다르면 서버가
//!   거절한다), MySQL · MariaDB 는 `` `db`.`t` ``, SQLite 는 `"schema"."t"`(없으면 `main` — 메타데이터 조회와 같은 자리).
//! - **행은 키로 가리킨다**: 키 열(M3-10 의 판정 `pw_db::schema::row_key` — 부른 쪽이 준다)마다 `"k" = ?` 를 `AND` 로. 수정 · 삭제의
//!   키는 정확히 키 열이어야 하고, 키 열을 고치는 수정은 거절한다(키 없는 테이블 · 키 수정은 이월 — 14 §3). 키 값의 NULL 은 거절
//!   (키 열은 NOT NULL 이다).
//! - **순서**: 삭제 → 수정 → 추가. 지운 행의 키를 새 행이 다시 쓸 수 있고, 지운 행이 쥐던 유일 값을 수정이 받을 수 있다.
//! - **MySQL · MariaDB 의 수정**은 값이 그대로면 영향 행 수가 0 이다(`CLIENT_FOUND_ROWS` 를 켜지 않는다). 그래서 수정마다 같은 키의
//!   잠그는 읽기(`SELECT 1 … FOR UPDATE` — 현재 행을 읽는다. 스냅숏 읽기면 그사이 지워진 행도 보인다)를 [`EditStatement::probe`] 로
//!   함께 내고, 실행하는 쪽이 0 일 때만 그것으로 행이 있는지 본다.
//!
//! 문장의 등급은 실행하는 쪽이 분류기(`crate::risk::classify_statement`)로 다시 매긴다 — 이 모듈은 등급을 정하지 않는다.

use pw_db_types::{DbValue, ObjectRef, SqlDialect};
use serde::{Deserialize, Serialize};

use crate::literal::{LiteralError, literal, quote_ident};

/// 한 번에 받는 변경(수정 + 추가 + 삭제)의 상한. 변경 하나가 문장 하나다.
pub const MAX_EDIT_CHANGES: usize = 10_000;

/// 열 하나와 그 값.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DbEditCell {
    pub column: String,
    pub value: DbValue,
}

/// 행 하나의 수정: 키로 행을 가리키고 `values` 의 열을 바꾼다.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DbEditUpdate {
    /// 키 열 전부와 그 (원래) 값.
    pub key: Vec<DbEditCell>,
    /// 바꿀 열과 새 값. 키 열은 넣지 않는다.
    pub values: Vec<DbEditCell>,
}

/// 새 행. 빠진 열은 기본값이다(값이 하나도 없으면 모든 열이 기본값).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DbEditInsert {
    pub values: Vec<DbEditCell>,
}

/// 행 하나의 삭제.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DbEditDelete {
    pub key: Vec<DbEditCell>,
}

/// 제출할 변경. 문장 번호는 삭제 → 수정 → 추가 순이다.
#[derive(
    Debug, Clone, PartialEq, Default, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DbEditChanges {
    #[serde(default)]
    pub updates: Vec<DbEditUpdate>,
    #[serde(default)]
    pub inserts: Vec<DbEditInsert>,
    #[serde(default)]
    pub deletes: Vec<DbEditDelete>,
}

impl DbEditChanges {
    pub fn len(&self) -> usize {
        self.updates.len() + self.inserts.len() + self.deletes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

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
pub enum DbEditChangeKind {
    Update,
    Insert,
    Delete,
}

/// 변경 하나를 가리킨다: 종류와 그 목록(`changes.updates` 등)의 번호(0부터).
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
pub struct DbEditChangeRef {
    pub kind: DbEditChangeKind,
    pub index: u32,
}

/// 변경을 문장으로 만들 수 없는 까닭.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum DbEditErrorReason {
    /// 변경이 없다.
    NoChanges,
    /// 변경이 [`MAX_EDIT_CHANGES`] 를 넘는다.
    TooManyChanges,
    /// 테이블 이름이 한정되지 않았다(PG 는 DB · 스키마, MySQL 은 DB 가 있어야 한다).
    UnqualifiedTable,
    /// 키 없는 테이블(PK · NOT NULL 열만의 유일 키가 없다 — 결정 M3-D9).
    NoRowKey,
    /// 수정 · 삭제의 키가 테이블의 키 열과 다르다(빠짐 · 남는 열).
    KeyMismatch,
    /// 수정이 키 열을 바꾼다(이월 — 14 §3).
    KeyColumnChanged,
    /// 키 값이 NULL 이다.
    NullKey,
    /// 수정에 바꿀 열이 없다.
    NoValues,
    /// 같은 열이 한 변경에 두 번 있다.
    DuplicateColumn,
    /// 같은 행(같은 키)을 두 변경이 가리킨다.
    DuplicateRow,
    /// 열 · 테이블 이름이 비었다.
    EmptyName,
    /// 셀 상한에서 잘린 값(원래 값이 아니다).
    Truncated,
    /// 그 방언에 리터럴이 없는 값(배열 · NaN 등).
    Unsupported,
    /// 그 방언이 NUL 을 담지 못한다(PG 문자열 · 식별자).
    Nul,
}

/// [`build`] 의 실패: 어느 변경 · 어느 열(알면)과 까닭.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DbEditError {
    pub change: Option<DbEditChangeRef>,
    pub column: Option<String>,
    pub reason: DbEditErrorReason,
}

impl DbEditError {
    fn new(reason: DbEditErrorReason) -> Self {
        Self {
            change: None,
            column: None,
            reason,
        }
    }

    fn at(change: DbEditChangeRef, column: Option<&str>, reason: DbEditErrorReason) -> Self {
        Self {
            change: Some(change),
            column: column.map(str::to_owned),
            reason,
        }
    }

    pub fn describe(&self) -> &'static str {
        match self.reason {
            DbEditErrorReason::NoChanges => "there is no change to submit",
            DbEditErrorReason::TooManyChanges => "too many changes in one submit",
            DbEditErrorReason::UnqualifiedTable => {
                "the table must be qualified (postgres: database and schema, mysql: database)"
            }
            DbEditErrorReason::NoRowKey => {
                "the table has no row key (a primary key or a unique key of not-null columns)"
            }
            DbEditErrorReason::KeyMismatch => "the row key must name exactly the key columns",
            DbEditErrorReason::KeyColumnChanged => "editing a key column is not supported",
            DbEditErrorReason::NullKey => "a row key value is null",
            DbEditErrorReason::NoValues => "an update changes no column",
            DbEditErrorReason::DuplicateColumn => "a column appears twice in one change",
            DbEditErrorReason::DuplicateRow => "two changes name the same row",
            DbEditErrorReason::EmptyName => "a table or column name is empty",
            DbEditErrorReason::Truncated => "the value was truncated at the cell limit",
            DbEditErrorReason::Unsupported => "the dialect has no literal for this value",
            DbEditErrorReason::Nul => "the dialect cannot hold a NUL character",
        }
    }
}

impl From<LiteralError> for DbEditErrorReason {
    fn from(e: LiteralError) -> Self {
        match e {
            LiteralError::Truncated => Self::Truncated,
            LiteralError::Unsupported => Self::Unsupported,
            LiteralError::Nul => Self::Nul,
        }
    }
}

/// 변경 하나의 문장.
#[derive(Debug, Clone, PartialEq)]
pub struct EditStatement {
    pub change: DbEditChangeRef,
    /// 서버에 보낼 글(자리 표시).
    pub sql: String,
    /// 자리 표시 순서의 바인드 값.
    pub params: Vec<DbValue>,
    /// 미리보기 글: `sql` 의 자리 표시마다 같은 값의 리터럴.
    pub preview: String,
    /// MySQL 수정의 「행이 있는가」 잠그는 읽기(머리 주석). 그 밖은 `None`.
    pub probe: Option<EditProbe>,
}

/// 같은 키의 행을 잠그며 읽는 조회(`SELECT 1 FROM … WHERE <키> FOR UPDATE`).
#[derive(Debug, Clone, PartialEq)]
pub struct EditProbe {
    pub sql: String,
    pub params: Vec<DbValue>,
}

/// 바인드 판과 미리보기 판을 함께 쌓는다.
struct Builder {
    dialect: SqlDialect,
    sql: String,
    preview: String,
    params: Vec<DbValue>,
}

impl Builder {
    fn new(dialect: SqlDialect) -> Self {
        Self {
            dialect,
            sql: String::new(),
            preview: String::new(),
            params: Vec::new(),
        }
    }

    fn text(&mut self, s: &str) {
        self.sql.push_str(s);
        self.preview.push_str(s);
    }

    fn value(&mut self, value: &DbValue) -> Result<(), LiteralError> {
        self.preview.push_str(&literal(self.dialect, value)?);
        self.params.push(value.clone());
        match self.dialect {
            SqlDialect::Postgres => {
                self.sql.push('$');
                self.sql.push_str(&self.params.len().to_string());
            }
            SqlDialect::Mysql | SqlDialect::Sqlite => self.sql.push('?'),
        }
        Ok(())
    }
}

/// 편집할 테이블을 방언의 한정 꼴로 맞춘다: PG `{ database, schema, name }`(둘 다 있어야 한다), MySQL `{ database, name }`(스키마
/// 칸에 온 DB 도 받는다), SQLite `{ schema, name }`(DB 칸에 온 붙인 DB 이름도 받고, 없으면 `main`). 메타데이터 조회(키 판정)와
/// 문장이 같은 테이블을 가리키게 둘 다 이것을 쓴다. 한정할 수 없으면 `UnqualifiedTable`.
pub fn edit_table(dialect: SqlDialect, table: &ObjectRef) -> Result<ObjectRef, DbEditError> {
    let unqualified = || DbEditError::new(DbEditErrorReason::UnqualifiedTable);
    let one_of = |default: Option<&str>| -> Result<String, DbEditError> {
        match (table.database.as_deref(), table.schema.as_deref()) {
            (Some(d), Some(s)) if d != s => Err(unqualified()),
            (Some(d), _) | (None, Some(d)) => Ok(d.to_owned()),
            (None, None) => default.map(str::to_owned).ok_or_else(unqualified),
        }
    };
    let canonical = match dialect {
        SqlDialect::Postgres => ObjectRef {
            database: Some(table.database.clone().ok_or_else(unqualified)?),
            schema: Some(table.schema.clone().ok_or_else(unqualified)?),
            name: table.name.clone(),
        },
        // MySQL 은 DB 가 곧 스키마다 — 둘이 다르면 어느 쪽인지 모른다.
        SqlDialect::Mysql => ObjectRef {
            database: Some(one_of(None)?),
            schema: None,
            name: table.name.clone(),
        },
        // 붙인 DB 의 이름. 메타데이터 조회(`pw-db-driver-sqlite`)와 같이 없으면 `main` 이다.
        SqlDialect::Sqlite => ObjectRef {
            database: None,
            schema: Some(one_of(Some("main"))?),
            name: table.name.clone(),
        },
    };
    for part in [
        &canonical.database,
        &canonical.schema,
        &Some(canonical.name.clone()),
    ]
    .into_iter()
    .flatten()
    {
        if part.is_empty() {
            return Err(DbEditError::new(DbEditErrorReason::EmptyName));
        }
        quote_ident(dialect, part).map_err(|e| DbEditError::new(e.into()))?;
    }
    Ok(canonical)
}

/// 테이블의 한정 이름(머리 주석의 방언별 꼴, [`edit_table`] 로 맞춘 것).
pub fn qualified_table(dialect: SqlDialect, table: &ObjectRef) -> Result<String, DbEditError> {
    let table = edit_table(dialect, table)?;
    let mut parts = Vec::with_capacity(3);
    for part in [table.database, table.schema, Some(table.name)]
        .into_iter()
        .flatten()
    {
        parts.push(quote_ident(dialect, &part).map_err(|e| DbEditError::new(e.into()))?);
    }
    Ok(parts.join("."))
}

/// 변경을 문장으로. `key` 는 테이블의 행 키 열(M3-10 의 판정)이다 — 비었으면 `NoRowKey`.
pub fn build(
    dialect: SqlDialect,
    table: &ObjectRef,
    key: &[String],
    changes: &DbEditChanges,
) -> Result<Vec<EditStatement>, DbEditError> {
    if key.is_empty() {
        return Err(DbEditError::new(DbEditErrorReason::NoRowKey));
    }
    if changes.is_empty() {
        return Err(DbEditError::new(DbEditErrorReason::NoChanges));
    }
    if changes.len() > MAX_EDIT_CHANGES {
        return Err(DbEditError::new(DbEditErrorReason::TooManyChanges));
    }
    let table_name = qualified_table(dialect, table)?;
    let mut key_names = Vec::with_capacity(key.len());
    for k in key {
        if k.is_empty() {
            return Err(DbEditError::new(DbEditErrorReason::EmptyName));
        }
        key_names.push(quote_ident(dialect, k).map_err(|e| DbEditError::new(e.into()))?);
    }
    let ctx = Ctx {
        dialect,
        key,
        key_names: &key_names,
    };

    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::with_capacity(changes.len());
    for (i, delete) in changes.deletes.iter().enumerate() {
        let at = DbEditChangeRef {
            kind: DbEditChangeKind::Delete,
            index: i as u32,
        };
        let values = ctx.key_values(at, &delete.key)?;
        ctx.remember(at, &values, &mut seen)?;
        let mut b = Builder::new(dialect);
        b.text("DELETE FROM ");
        b.text(&table_name);
        ctx.where_key(&mut b, at, &values)?;
        out.push(statement(at, b, None));
    }
    for (i, update) in changes.updates.iter().enumerate() {
        let at = DbEditChangeRef {
            kind: DbEditChangeKind::Update,
            index: i as u32,
        };
        let values = ctx.key_values(at, &update.key)?;
        ctx.remember(at, &values, &mut seen)?;
        if update.values.is_empty() {
            return Err(DbEditError::at(at, None, DbEditErrorReason::NoValues));
        }
        let set = ctx.columns(at, &update.values)?;
        if let Some(cell) = update.values.iter().find(|c| key.contains(&c.column)) {
            return Err(DbEditError::at(
                at,
                Some(&cell.column),
                DbEditErrorReason::KeyColumnChanged,
            ));
        }
        let mut b = Builder::new(dialect);
        b.text("UPDATE ");
        b.text(&table_name);
        b.text(" SET ");
        for (n, (name, cell)) in set.iter().zip(&update.values).enumerate() {
            if n > 0 {
                b.text(", ");
            }
            b.text(name);
            b.text(" = ");
            b.value(&cell.value)
                .map_err(|e| DbEditError::at(at, Some(&cell.column), e.into()))?;
        }
        ctx.where_key(&mut b, at, &values)?;
        let probe = match dialect {
            SqlDialect::Mysql => {
                let mut p = Builder::new(dialect);
                p.text("SELECT 1 FROM ");
                p.text(&table_name);
                ctx.where_key(&mut p, at, &values)?;
                p.text(" FOR UPDATE");
                Some(EditProbe {
                    sql: p.sql,
                    params: p.params,
                })
            }
            SqlDialect::Postgres | SqlDialect::Sqlite => None,
        };
        out.push(statement(at, b, probe));
    }
    for (i, insert) in changes.inserts.iter().enumerate() {
        let at = DbEditChangeRef {
            kind: DbEditChangeKind::Insert,
            index: i as u32,
        };
        let names = ctx.columns(at, &insert.values)?;
        let mut b = Builder::new(dialect);
        b.text("INSERT INTO ");
        b.text(&table_name);
        if insert.values.is_empty() {
            b.text(match dialect {
                SqlDialect::Mysql => " () VALUES ()",
                SqlDialect::Postgres | SqlDialect::Sqlite => " DEFAULT VALUES",
            });
        } else {
            b.text(" (");
            b.text(&names.join(", "));
            b.text(") VALUES (");
            for (n, cell) in insert.values.iter().enumerate() {
                if n > 0 {
                    b.text(", ");
                }
                b.value(&cell.value)
                    .map_err(|e| DbEditError::at(at, Some(&cell.column), e.into()))?;
            }
            b.text(")");
        }
        out.push(statement(at, b, None));
    }
    Ok(out)
}

fn statement(change: DbEditChangeRef, b: Builder, probe: Option<EditProbe>) -> EditStatement {
    EditStatement {
        change,
        sql: b.sql,
        params: b.params,
        preview: b.preview,
        probe,
    }
}

struct Ctx<'a> {
    dialect: SqlDialect,
    key: &'a [String],
    key_names: &'a [String],
}

impl Ctx<'_> {
    /// 키 셀을 키 열 순서의 값으로. 정확히 키 열이어야 하고 NULL 이 없어야 한다.
    fn key_values<'c>(
        &self,
        at: DbEditChangeRef,
        cells: &'c [DbEditCell],
    ) -> Result<Vec<&'c DbValue>, DbEditError> {
        if cells.len() != self.key.len() {
            return Err(DbEditError::at(at, None, DbEditErrorReason::KeyMismatch));
        }
        let mut values = Vec::with_capacity(self.key.len());
        for k in self.key {
            let mut found = cells.iter().filter(|c| &c.column == k);
            let Some(cell) = found.next() else {
                return Err(DbEditError::at(at, Some(k), DbEditErrorReason::KeyMismatch));
            };
            if found.next().is_some() {
                return Err(DbEditError::at(
                    at,
                    Some(k),
                    DbEditErrorReason::DuplicateColumn,
                ));
            }
            if cell.value == DbValue::Null {
                return Err(DbEditError::at(at, Some(k), DbEditErrorReason::NullKey));
            }
            values.push(&cell.value);
        }
        Ok(values)
    }

    /// 같은 행을 두 변경이 가리키면 거절한다.
    fn remember(
        &self,
        at: DbEditChangeRef,
        values: &[&DbValue],
        seen: &mut std::collections::HashSet<String>,
    ) -> Result<(), DbEditError> {
        let canonical = serde_json::to_string(values).unwrap_or_default();
        if seen.insert(canonical) {
            Ok(())
        } else {
            Err(DbEditError::at(at, None, DbEditErrorReason::DuplicateRow))
        }
    }

    /// ` WHERE "k1" = ? AND "k2" = ?`.
    fn where_key(
        &self,
        b: &mut Builder,
        at: DbEditChangeRef,
        values: &[&DbValue],
    ) -> Result<(), DbEditError> {
        b.text(" WHERE ");
        for (n, ((name, column), value)) in
            self.key_names.iter().zip(self.key).zip(values).enumerate()
        {
            if n > 0 {
                b.text(" AND ");
            }
            b.text(name);
            b.text(" = ");
            b.value(value)
                .map_err(|e| DbEditError::at(at, Some(column), e.into()))?;
        }
        Ok(())
    }

    /// 셀들의 열 이름을 따옴표로. 빈 이름 · 같은 열 두 번은 거절.
    fn columns(
        &self,
        at: DbEditChangeRef,
        cells: &[DbEditCell],
    ) -> Result<Vec<String>, DbEditError> {
        let mut names = Vec::with_capacity(cells.len());
        for (n, cell) in cells.iter().enumerate() {
            if cell.column.is_empty() {
                return Err(DbEditError::at(at, None, DbEditErrorReason::EmptyName));
            }
            if cells[..n].iter().any(|c| c.column == cell.column) {
                return Err(DbEditError::at(
                    at,
                    Some(&cell.column),
                    DbEditErrorReason::DuplicateColumn,
                ));
            }
            names.push(
                quote_ident(self.dialect, &cell.column)
                    .map_err(|e| DbEditError::at(at, Some(&cell.column), e.into()))?,
            );
        }
        Ok(names)
    }
}

// 실패의 `data`(`DbEditError`)는 메서드의 출력이 아니라 오류에 실리므로 창이 쓸 타입으로 따로 등록한다.
pw_rpc::rpc_type!(DbEditError, DbEditErrorReason);

#[cfg(test)]
mod tests;
