//! 메타데이터: `information_schema`(서버 쪽 필터 + `LIMIT`), DDL 은 `SHOW CREATE TABLE`.
//!
//! MySQL 은 DB 가 곧 스키마다(`Capabilities::schemas` 가 `false`) — `ObjectRef::database` 가 없으면 연결의 지금 DB(`DATABASE()`).
//! 객체 목록은 이름의 바이트 순서로 쪽을 나눈다: 다음 쪽 커서는 앞 쪽 마지막 이름이고, 이름 비교 · 정렬은 `CAST(… AS BINARY)` 로
//! 한다 — `information_schema` 의 대소문자 무시 정렬로는 대소문자만 다른 두 테이블이 쪽 경계에서 빠질 수 있다.

use std::collections::BTreeMap;

use mysql_async::prelude::Queryable;
use mysql_async::{Conn, Params, Value};
use pw_db_types::{
    ColumnInfo, DatabaseInfo, DbError, DbErrorKind, ForeignKeyInfo, IndexInfo, ObjectInfo,
    ObjectKind, ObjectPage, ObjectQuery, ObjectRef,
};

use crate::conn::quote;

/// 쪽 크기(1 ~ 500 — `ObjectQuery::limit`).
pub(crate) fn page_size(limit: u32) -> Result<u32, DbError> {
    if (1..=500).contains(&limit) {
        Ok(limit)
    } else {
        Err(DbError::new(
            DbErrorKind::Invalid,
            "the page size must be between 1 and 500",
        ))
    }
}

fn text(value: Option<String>) -> Value {
    value.map_or(Value::NULL, |v| Value::Bytes(v.into_bytes()))
}

/// `LIKE` 의 `%` · `_` 를 글자로(이스케이프 문자 `!`).
fn like_escape(filter: &str) -> String {
    let mut escaped = String::with_capacity(filter.len());
    for c in filter.chars() {
        if matches!(c, '!' | '%' | '_') {
            escaped.push('!');
        }
        escaped.push(c);
    }
    escaped
}

fn non_empty(value: Option<String>) -> Option<String> {
    value.filter(|v| !v.is_empty())
}

pub(crate) async fn databases(conn: &mut Conn) -> mysql_async::Result<Vec<DatabaseInfo>> {
    let names: Vec<String> = conn
        .query("SELECT schema_name FROM information_schema.schemata ORDER BY CAST(schema_name AS BINARY)")
        .await?;
    Ok(names
        .into_iter()
        .map(|name| DatabaseInfo { name })
        .collect())
}

pub(crate) async fn objects(
    conn: &mut Conn,
    query: ObjectQuery,
    limit: u32,
) -> mysql_async::Result<ObjectPage> {
    // (테이블, 이름 열, 설명 열, 종류 조건)
    let (table, name, comment, condition) = match query.kind {
        ObjectKind::Table => (
            "tables",
            "table_name",
            "table_comment",
            "table_type IN ('BASE TABLE', 'SYSTEM VERSIONED')",
        ),
        ObjectKind::View => ("tables", "table_name", "NULL", "table_type = 'VIEW'"),
        // MariaDB 의 시퀀스. MySQL 에는 없어 빈 쪽이다.
        ObjectKind::Sequence => (
            "tables",
            "table_name",
            "table_comment",
            "table_type = 'SEQUENCE'",
        ),
        ObjectKind::Function => (
            "routines",
            "routine_name",
            "routine_comment",
            "routine_type = 'FUNCTION'",
        ),
        ObjectKind::Procedure => (
            "routines",
            "routine_name",
            "routine_comment",
            "routine_type = 'PROCEDURE'",
        ),
        ObjectKind::Trigger => ("triggers", "trigger_name", "NULL", "TRUE"),
        ObjectKind::MaterializedView => {
            return Ok(ObjectPage {
                items: Vec::new(),
                next_cursor: None,
            });
        }
    };
    let schema = match query.kind {
        ObjectKind::Function | ObjectKind::Procedure => "routine_schema",
        ObjectKind::Trigger => "trigger_schema",
        _ => "table_schema",
    };
    let sql = format!(
        "SELECT {name}, {comment} FROM information_schema.{table} \
         WHERE {schema} = COALESCE(?, DATABASE()) AND {condition} \
         AND (? IS NULL OR LOWER({name}) LIKE CONCAT('%', LOWER(?), '%') ESCAPE '!') \
         AND (? IS NULL OR CAST({name} AS BINARY) > CAST(? AS BINARY)) \
         ORDER BY CAST({name} AS BINARY) LIMIT ?"
    );
    let filter = query.filter.as_deref().map(like_escape);
    let params = vec![
        text(query.database),
        text(filter.clone()),
        text(filter),
        text(query.cursor.clone()),
        text(query.cursor),
        Value::UInt(u64::from(limit) + 1),
    ];
    let mut rows: Vec<(String, Option<String>)> =
        conn.exec(sql, Params::Positional(params)).await?;
    let more = rows.len() > limit as usize;
    rows.truncate(limit as usize);
    let next_cursor = if more {
        rows.last().map(|(name, _)| name.clone())
    } else {
        None
    };
    Ok(ObjectPage {
        items: rows
            .into_iter()
            .map(|(name, comment)| ObjectInfo {
                name,
                kind: query.kind,
                comment: non_empty(comment),
            })
            .collect(),
        next_cursor,
    })
}

fn object_params(object: ObjectRef) -> Params {
    Params::Positional(vec![text(object.database), text(Some(object.name))])
}

/// `information_schema.columns` 한 줄: 이름 · 타입 · `YES`/`NO` · 기본값 · 설명.
type ColumnRow = (String, String, String, Option<String>, Option<String>);

pub(crate) async fn columns(
    conn: &mut Conn,
    object: ObjectRef,
) -> mysql_async::Result<Vec<ColumnInfo>> {
    let rows: Vec<ColumnRow> = conn
        .exec(
            "SELECT column_name, column_type, is_nullable, column_default, column_comment \
             FROM information_schema.columns WHERE table_schema = COALESCE(?, DATABASE()) AND table_name = ? \
             ORDER BY ordinal_position",
            object_params(object),
        )
        .await?;
    Ok(rows
        .into_iter()
        .map(|(name, db_type, nullable, default, comment)| ColumnInfo {
            name,
            db_type,
            nullable: nullable == "YES",
            default,
            comment: non_empty(comment),
        })
        .collect())
}

pub(crate) async fn indexes(
    conn: &mut Conn,
    object: ObjectRef,
) -> mysql_async::Result<Vec<IndexInfo>> {
    let rows: Vec<(String, Option<String>, i64)> = conn
        .exec(
            "SELECT index_name, column_name, non_unique FROM information_schema.statistics \
             WHERE table_schema = COALESCE(?, DATABASE()) AND table_name = ? \
             ORDER BY index_name <> 'PRIMARY', CAST(index_name AS BINARY), seq_in_index",
            object_params(object),
        )
        .await?;
    let mut order = Vec::new();
    let mut by_name: BTreeMap<String, IndexInfo> = BTreeMap::new();
    for (name, column, non_unique) in rows {
        let index = by_name.entry(name.clone()).or_insert_with(|| {
            order.push(name.clone());
            IndexInfo {
                primary: name == "PRIMARY",
                unique: non_unique == 0,
                partial: false,
                name,
                columns: Vec::new(),
            }
        });
        match column {
            Some(column) => index.columns.push(column),
            // 함수 키 부분(MySQL 8 의 식 인덱스) — 열이 아니므로 그리드 편집의 키로 쓰지 않는다.
            None => index.partial = true,
        }
    }
    Ok(order
        .into_iter()
        .filter_map(|name| by_name.remove(&name))
        .collect())
}

pub(crate) async fn foreign_keys(
    conn: &mut Conn,
    object: ObjectRef,
) -> mysql_async::Result<Vec<ForeignKeyInfo>> {
    let rows: Vec<(String, String, String, String, String)> = conn
        .exec(
            "SELECT constraint_name, column_name, referenced_table_schema, referenced_table_name, \
             referenced_column_name FROM information_schema.key_column_usage \
             WHERE table_schema = COALESCE(?, DATABASE()) AND table_name = ? AND referenced_table_name IS NOT NULL \
             ORDER BY CAST(constraint_name AS BINARY), ordinal_position",
            object_params(object),
        )
        .await?;
    let mut keys: Vec<ForeignKeyInfo> = Vec::new();
    for (name, column, ref_schema, ref_table, ref_column) in rows {
        match keys.last_mut() {
            Some(key) if key.name == name => {
                key.columns.push(column);
                key.referenced_columns.push(ref_column);
            }
            _ => keys.push(ForeignKeyInfo {
                name,
                columns: vec![column],
                references: ObjectRef {
                    database: Some(ref_schema),
                    schema: None,
                    name: ref_table,
                },
                referenced_columns: vec![ref_column],
            }),
        }
    }
    Ok(keys)
}

/// `SHOW CREATE TABLE`(뷰는 뷰의 정의를 준다). 둘째 열이 DDL 글이다.
pub(crate) async fn ddl(conn: &mut Conn, object: ObjectRef) -> mysql_async::Result<Option<String>> {
    let name = match &object.database {
        Some(database) => format!("{}.{}", quote(database), quote(&object.name)),
        None => quote(&object.name),
    };
    let row: Option<mysql_async::Row> = conn
        .query_first(format!("SHOW CREATE TABLE {name}"))
        .await?;
    Ok(row.and_then(|row| row.get::<String, _>(1)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn like_wildcards_are_escaped_and_pages_are_bounded() {
        assert_eq!(like_escape("a_b%c!d"), "a!_b!%c!!d");
        assert!(page_size(0).is_err());
        assert!(page_size(501).is_err());
        assert_eq!(page_size(500).unwrap(), 500);
    }
}
