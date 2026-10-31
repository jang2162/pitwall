//! 메타데이터와 실행 계획. 목록은 `sqlite_schema`, 열 · 인덱스 · FK 는 `pragma_table_info` · `pragma_index_list` ·
//! `pragma_foreign_key_list` 테이블 값 함수로 읽는다(값을 SQL 글에 잇지 않고 바인드한다).
//!
//! SQLite 에는 스키마 단계가 없다 — [`ObjectRef`] · [`ObjectQuery`] 의 `database`(없으면 `schema`)가 붙인 DB 의 이름이고, 둘 다
//! 없으면 `main` 이다.

use std::collections::BTreeMap;

use pw_db_types::{
    ColumnInfo, DbError, Ddl, ExplainPlan, ForeignKeyInfo, IndexInfo, ObjectInfo, ObjectKind,
    ObjectPage, ObjectQuery, ObjectRef, PlanFormat,
};
use rusqlite::{Connection, params};

use crate::error::{invalid, map};

/// `INTEGER PRIMARY KEY`(rowid 별칭) 테이블은 인덱스 목록에 기본 키가 없다 — 그 자리에 이 이름의 기본 키 항목을 만든다.
pub(crate) const ROWID_PRIMARY_KEY: &str = "PRIMARY";

type MetaResult<T> = Result<T, DbError>;

pub(crate) fn objects(conn: &Connection, query: &ObjectQuery) -> MetaResult<ObjectPage> {
    if !(1..=500).contains(&query.limit) {
        return Err(invalid("the page size must be between 1 and 500"));
    }
    let (kind, type_name) = match query.kind {
        ObjectKind::Table => (ObjectKind::Table, "table"),
        ObjectKind::View => (ObjectKind::View, "view"),
        ObjectKind::Trigger => (ObjectKind::Trigger, "trigger"),
        // 구체화 뷰 · 시퀀스 · 함수 · 프로시저는 SQLite 에 없다.
        _ => {
            return Ok(ObjectPage {
                items: Vec::new(),
                next_cursor: None,
            });
        }
    };
    let schema = quote(schema_of(
        query.database.as_deref(),
        query.schema.as_deref(),
    ));
    // 내부 테이블(`sqlite_sequence` · `sqlite_stat1` …)은 빼고, 한 개 더 읽어 다음 쪽이 있는지 본다.
    let sql = format!(
        "SELECT name FROM {schema}.sqlite_schema \
         WHERE type = ?1 AND name NOT LIKE 'sqlite\\_%' ESCAPE '\\' \
         AND (?2 IS NULL OR instr(lower(name), lower(?2)) > 0) \
         AND (?3 IS NULL OR name > ?3) \
         ORDER BY name LIMIT ?4"
    );
    let limit = query.limit as usize;
    let mut names: Vec<String> = conn
        .prepare(&sql)
        .and_then(|mut statement| {
            statement
                .query_map(
                    params![type_name, query.filter, query.cursor, query.limit + 1],
                    |row| row.get(0),
                )?
                .collect()
        })
        .map_err(sql_error)?;
    let next_cursor = if names.len() > limit {
        names.truncate(limit);
        names.last().cloned()
    } else {
        None
    };
    Ok(ObjectPage {
        items: names
            .into_iter()
            .map(|name| ObjectInfo {
                name,
                kind,
                comment: None,
            })
            .collect(),
        next_cursor,
    })
}

/// `pragma_table_info` 한 줄.
struct TableColumn {
    name: String,
    db_type: String,
    not_null: bool,
    default: Option<String>,
    /// 기본 키 안의 자리(1 부터). 기본 키가 아니면 0.
    pk: u32,
}

fn table_columns(conn: &Connection, object: &ObjectRef) -> MetaResult<Vec<TableColumn>> {
    let columns: Vec<TableColumn> = conn
        .prepare(
            "SELECT name, type, \"notnull\", dflt_value, pk FROM pragma_table_info(?1, ?2) ORDER BY cid",
        )
        .and_then(|mut statement| {
            statement
                .query_map(params![object.name, schema_of_ref(object)], |row| {
                    Ok(TableColumn {
                        name: row.get(0)?,
                        db_type: row.get(1)?,
                        not_null: row.get(2)?,
                        default: row.get(3)?,
                        pk: row.get(4)?,
                    })
                })?
                .collect()
        })
        .map_err(sql_error)?;
    if columns.is_empty() {
        return Err(no_such_object(object));
    }
    Ok(columns)
}

/// 기본 키 열(키 안의 순서대로).
fn primary_key(columns: &[TableColumn]) -> Vec<String> {
    let mut keyed: Vec<&TableColumn> = columns.iter().filter(|c| c.pk > 0).collect();
    keyed.sort_by_key(|c| c.pk);
    keyed.into_iter().map(|c| c.name.clone()).collect()
}

/// `pragma_index_list` 한 줄.
struct IndexRow {
    name: String,
    unique: bool,
    /// `c`(CREATE INDEX) · `u`(UNIQUE 제약) · `pk`(기본 키).
    origin: String,
    partial: bool,
}

fn index_rows(conn: &Connection, object: &ObjectRef) -> MetaResult<Vec<IndexRow>> {
    conn.prepare("SELECT name, \"unique\", origin, partial FROM pragma_index_list(?1, ?2)")
        .and_then(|mut statement| {
            statement
                .query_map(params![object.name, schema_of_ref(object)], |row| {
                    Ok(IndexRow {
                        name: row.get(0)?,
                        unique: row.get(1)?,
                        origin: row.get(2)?,
                        partial: row.get(3)?,
                    })
                })?
                .collect()
        })
        .map_err(sql_error)
}

/// rowid 별칭(`INTEGER PRIMARY KEY` 열 하나)이면 그 열 — NULL 을 넣을 수 없다(`notnull` 은 0 으로 나온다).
fn rowid_alias<'c>(columns: &'c [TableColumn], indexes: &[IndexRow]) -> Option<&'c str> {
    let mut keyed = columns.iter().filter(|c| c.pk > 0);
    let only = keyed.next()?;
    let alias = keyed.next().is_none()
        && only.db_type.eq_ignore_ascii_case("INTEGER")
        && !indexes.iter().any(|i| i.origin == "pk");
    alias.then_some(only.name.as_str())
}

pub(crate) fn columns(conn: &Connection, object: &ObjectRef) -> MetaResult<Vec<ColumnInfo>> {
    let columns = table_columns(conn, object)?;
    let indexes = index_rows(conn, object)?;
    let alias = rowid_alias(&columns, &indexes).map(str::to_owned);
    Ok(columns
        .into_iter()
        .map(|c| ColumnInfo {
            nullable: !c.not_null && alias.as_deref() != Some(c.name.as_str()),
            name: c.name,
            db_type: c.db_type,
            default: c.default,
            comment: None,
        })
        .collect())
}

/// 기본 키가 먼저, 나머지는 이름 순서. rowid 별칭의 기본 키는 [`ROWID_PRIMARY_KEY`] 항목으로 만든다.
pub(crate) fn indexes(conn: &Connection, object: &ObjectRef) -> MetaResult<Vec<IndexInfo>> {
    let columns = table_columns(conn, object)?;
    let rows = index_rows(conn, object)?;
    let mut indexes = Vec::with_capacity(rows.len() + 1);
    for row in &rows {
        let index = ObjectRef {
            name: row.name.clone(),
            ..object.clone()
        };
        let index_columns: Vec<Option<String>> = conn
            .prepare("SELECT name FROM pragma_index_info(?1, ?2) ORDER BY seqno")
            .and_then(|mut statement| {
                statement
                    .query_map(params![index.name, schema_of_ref(&index)], |r| r.get(0))?
                    .collect()
            })
            .map_err(sql_error)?;
        indexes.push(IndexInfo {
            name: row.name.clone(),
            // 식 인덱스의 열은 이름이 없다.
            columns: index_columns
                .into_iter()
                .map(|c| c.unwrap_or_else(|| "(expression)".to_owned()))
                .collect(),
            primary: row.origin == "pk",
            unique: row.unique,
            partial: row.partial,
        });
    }
    if !rows.iter().any(|r| r.origin == "pk") {
        let key = primary_key(&columns);
        if !key.is_empty() {
            indexes.push(IndexInfo {
                name: ROWID_PRIMARY_KEY.to_owned(),
                columns: key,
                primary: true,
                unique: true,
                partial: false,
            });
        }
    }
    indexes.sort_by(|a, b| b.primary.cmp(&a.primary).then_with(|| a.name.cmp(&b.name)));
    Ok(indexes)
}

/// `pragma_foreign_key_list` 의 FK 하나(열마다 한 줄).
struct ForeignKeyRows {
    table: String,
    columns: Vec<String>,
    /// 부모 열. 적지 않았으면 `None`.
    to: Vec<Option<String>>,
}

/// SQLite 의 FK 는 이름을 돌려주지 않는다 — `fk_<pragma id>` 로 부른다.
pub(crate) fn foreign_keys(
    conn: &Connection,
    object: &ObjectRef,
) -> MetaResult<Vec<ForeignKeyInfo>> {
    table_columns(conn, object)?;
    let rows: Vec<(i64, String, String, Option<String>)> = conn
        .prepare(
            "SELECT id, \"table\", \"from\", \"to\" FROM pragma_foreign_key_list(?1, ?2) ORDER BY id, seq",
        )
        .and_then(|mut statement| {
            statement
                .query_map(params![object.name, schema_of_ref(object)], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
                })?
                .collect()
        })
        .map_err(sql_error)?;
    let mut grouped: BTreeMap<i64, ForeignKeyRows> = BTreeMap::new();
    for (id, table, from, to) in rows {
        let entry = grouped.entry(id).or_insert_with(|| ForeignKeyRows {
            table,
            columns: Vec::new(),
            to: Vec::new(),
        });
        entry.columns.push(from);
        entry.to.push(to);
    }
    grouped
        .into_iter()
        .map(|(id, rows)| {
            let references = ObjectRef {
                database: object.database.clone(),
                schema: object.schema.clone(),
                name: rows.table,
            };
            // `REFERENCES parent` 처럼 열을 적지 않으면 부모의 기본 키를 가리킨다.
            let referenced_columns = match rows.to.into_iter().collect::<Option<Vec<String>>>() {
                Some(columns) => columns,
                None => primary_key(&table_columns(conn, &references)?),
            };
            Ok(ForeignKeyInfo {
                name: format!("fk_{id}"),
                columns: rows.columns,
                references,
                referenced_columns,
            })
        })
        .collect()
}

/// 객체의 `CREATE` 글. 테이블이면 그 테이블의 인덱스 · 트리거의 `CREATE` 를 잇는다(자동 인덱스는 글이 없어 빠진다).
pub(crate) fn ddl(conn: &Connection, object: &ObjectRef) -> MetaResult<Ddl> {
    let schema = quote(schema_of_ref(object));
    let found: Option<(String, Option<String>)> = conn
        .prepare(&format!(
            "SELECT type, sql FROM {schema}.sqlite_schema WHERE name = ?1"
        ))
        .and_then(|mut statement| {
            let mut rows =
                statement.query_map([&object.name], |row| Ok((row.get(0)?, row.get(1)?)))?;
            rows.next().transpose()
        })
        .map_err(sql_error)?;
    let Some((kind, Some(sql))) = found else {
        return Err(no_such_object(object));
    };
    let mut parts = vec![sql];
    if kind == "table" {
        let attached: Vec<String> = conn
            .prepare(&format!(
                "SELECT sql FROM {schema}.sqlite_schema \
                 WHERE tbl_name = ?1 AND type IN ('index', 'trigger') AND sql IS NOT NULL ORDER BY type, name"
            ))
            .and_then(|mut statement| {
                statement
                    .query_map([&object.name], |row| row.get(0))?
                    .collect()
            })
            .map_err(sql_error)?;
        parts.extend(attached);
    }
    Ok(Ddl {
        text: parts
            .iter()
            .map(|part| format!("{part};"))
            .collect::<Vec<_>>()
            .join("\n"),
    })
}

/// `EXPLAIN QUERY PLAN` 을 들여쓴 트리 글로(한 단계 두 칸). 문장을 실행하지 않는다.
pub(crate) fn explain(conn: &Connection, sql: &str) -> rusqlite::Result<ExplainPlan> {
    let mut statement = conn.prepare(&format!("EXPLAIN QUERY PLAN {sql}"))?;
    let rows: Vec<(i64, i64, String)> = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(3)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let mut depth: BTreeMap<i64, usize> = BTreeMap::new();
    let mut lines = Vec::with_capacity(rows.len());
    for (id, parent, detail) in rows {
        let level = depth.get(&parent).map_or(0, |d| d + 1);
        depth.insert(id, level);
        lines.push(format!("{}{detail}", "  ".repeat(level)));
    }
    Ok(ExplainPlan {
        format: PlanFormat::Text,
        text: lines.join("\n"),
    })
}

fn schema_of<'a>(database: Option<&'a str>, schema: Option<&'a str>) -> &'a str {
    database.or(schema).unwrap_or("main")
}

fn schema_of_ref(object: &ObjectRef) -> &str {
    schema_of(object.database.as_deref(), object.schema.as_deref())
}

/// 식별자를 큰따옴표로 감싼다(안의 `"` 는 둘로).
pub(crate) fn quote(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}

fn no_such_object(object: &ObjectRef) -> DbError {
    invalid(format!("no such table or view: {}", object.name))
}

fn sql_error(error: rusqlite::Error) -> DbError {
    map(error, None)
}
