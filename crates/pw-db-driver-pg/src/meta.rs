//! 실행 계획과 메타데이터(14-m3-plan.md M3-06): `pg_catalog` 중심, 목록은 서버 쪽 필터 + `LIMIT/OFFSET`, 열 · 인덱스 · FK 는 그
//! 테이블의 OID 와 열 번호 배열(`indkey` · `conkey`)로 한 질의에, DDL 은 `pg_get_*def` 조각으로 합성한다.
//!
//! PG 는 연결한 DB 의 카탈로그만 읽는다 — 요청이 다른 DB 를 가리키면 거절한다(그 DB 로 연결을 따로 연다). 스키마가 없으면
//! 세션의 `current_schema()` 다.

use pw_db_driver::DbResult;
use pw_db_types::{
    ColumnInfo, DatabaseInfo, Ddl, ExplainPlan, ForeignKeyInfo, IndexInfo, ObjectInfo, ObjectKind,
    ObjectPage, ObjectQuery, ObjectRef, PlanFormat, SchemaInfo,
};
use tokio_postgres::Client;
use tokio_postgres::types::ToSql;

use crate::control;
use crate::error::{db_error, invalid};
use crate::session::Shared;
use crate::value::Raw;

pub(crate) enum MetaRequest {
    Explain(String),
    Databases,
    Schemas(Option<String>),
    Objects(ObjectQuery),
    Columns(ObjectRef),
    Indexes(ObjectRef),
    ForeignKeys(ObjectRef),
    Ddl(ObjectRef),
}

pub(crate) enum MetaResponse {
    Explain(ExplainPlan),
    Databases(Vec<DatabaseInfo>),
    Schemas(Vec<SchemaInfo>),
    Objects(ObjectPage),
    Columns(Vec<ColumnInfo>),
    Indexes(Vec<IndexInfo>),
    ForeignKeys(Vec<ForeignKeyInfo>),
    Ddl(Ddl),
}

pub(crate) async fn run(
    client: &Client,
    request: MetaRequest,
    shared: &Shared,
) -> DbResult<MetaResponse> {
    match request {
        MetaRequest::Explain(sql) => explain(client, &sql, shared)
            .await
            .map(MetaResponse::Explain),
        MetaRequest::Databases => databases(client).await.map(MetaResponse::Databases),
        MetaRequest::Schemas(database) => {
            same_database(shared, database.as_deref())?;
            schemas(client).await.map(MetaResponse::Schemas)
        }
        MetaRequest::Objects(query) => {
            same_database(shared, query.database.as_deref())?;
            objects(client, &query).await.map(MetaResponse::Objects)
        }
        MetaRequest::Columns(object) => {
            let oid = relation(client, shared, &object).await?;
            columns(client, oid).await.map(MetaResponse::Columns)
        }
        MetaRequest::Indexes(object) => {
            let oid = relation(client, shared, &object).await?;
            indexes(client, oid).await.map(MetaResponse::Indexes)
        }
        MetaRequest::ForeignKeys(object) => {
            let oid = relation(client, shared, &object).await?;
            foreign_keys(client, shared, oid)
                .await
                .map(MetaResponse::ForeignKeys)
        }
        MetaRequest::Ddl(object) => {
            same_database(shared, object.database.as_deref())?;
            ddl(client, &object)
                .await
                .map(|text| MetaResponse::Ddl(Ddl { text }))
        }
    }
}

async fn query(
    client: &Client,
    sql: &str,
    params: &[&(dyn ToSql + Sync)],
) -> DbResult<Vec<tokio_postgres::Row>> {
    client.query(sql, params).await.map_err(|e| db_error(&e))
}

fn get<'a, T: tokio_postgres::types::FromSql<'a>>(
    row: &'a tokio_postgres::Row,
    i: usize,
) -> DbResult<T> {
    row.try_get(i).map_err(|e| db_error(&e))
}

fn same_database(shared: &Shared, database: Option<&str>) -> DbResult<()> {
    match database {
        Some(db) if db != shared.database => Err(invalid(format!(
            "this connection reads the catalog of `{}`; open a connection to `{db}` to read its metadata",
            shared.database
        ))),
        _ => Ok(()),
    }
}

/// `EXPLAIN (FORMAT JSON) <문장>`. 문장의 위험도는 파이프라인이 먼저 봤다(`ANALYZE` 는 안의 문장 등급).
async fn explain(client: &Client, sql: &str, shared: &Shared) -> DbResult<ExplainPlan> {
    if let Some(why) = control::refused(sql) {
        return Err(invalid(why));
    }
    let rows = shared
        .busy(query(client, &format!("EXPLAIN (FORMAT JSON) {sql}"), &[]))
        .await?;
    let mut text = String::new();
    for row in &rows {
        let Raw(raw) = get(row, 0)?;
        text.push_str(&String::from_utf8_lossy(raw.unwrap_or_default()));
    }
    Ok(ExplainPlan {
        format: PlanFormat::Json,
        text,
    })
}

async fn databases(client: &Client) -> DbResult<Vec<DatabaseInfo>> {
    let rows = query(
        client,
        "SELECT datname::text FROM pg_catalog.pg_database WHERE datallowconn AND NOT datistemplate ORDER BY datname",
        &[],
    )
    .await?;
    rows.iter()
        .map(|r| Ok(DatabaseInfo { name: get(r, 0)? }))
        .collect()
}

async fn schemas(client: &Client) -> DbResult<Vec<SchemaInfo>> {
    let rows = query(
        client,
        "SELECT nspname::text FROM pg_catalog.pg_namespace \
         WHERE nspname !~ '^pg_toast' AND nspname !~ '^pg_temp_' ORDER BY nspname",
        &[],
    )
    .await?;
    rows.iter()
        .map(|r| Ok(SchemaInfo { name: get(r, 0)? }))
        .collect()
}

/// 객체 종류마다 `name` · `comment` 두 열을 내는 질의(`ns` 는 그 스키마).
fn object_source(kind: ObjectKind) -> &'static str {
    match kind {
        ObjectKind::Table => {
            "SELECT c.relname AS name, pg_catalog.obj_description(c.oid, 'pg_class') AS comment \
             FROM pg_catalog.pg_class c, ns WHERE c.relnamespace = ns.oid AND c.relkind IN ('r', 'p')"
        }
        ObjectKind::View => {
            "SELECT c.relname AS name, pg_catalog.obj_description(c.oid, 'pg_class') AS comment \
             FROM pg_catalog.pg_class c, ns WHERE c.relnamespace = ns.oid AND c.relkind = 'v'"
        }
        ObjectKind::MaterializedView => {
            "SELECT c.relname AS name, pg_catalog.obj_description(c.oid, 'pg_class') AS comment \
             FROM pg_catalog.pg_class c, ns WHERE c.relnamespace = ns.oid AND c.relkind = 'm'"
        }
        ObjectKind::Sequence => {
            "SELECT c.relname AS name, pg_catalog.obj_description(c.oid, 'pg_class') AS comment \
             FROM pg_catalog.pg_class c, ns WHERE c.relnamespace = ns.oid AND c.relkind = 'S'"
        }
        // 겹쳐 정의된(오버로드) 함수는 이름 하나로 보인다 — DDL 은 그 이름의 정의를 모두 싣는다.
        ObjectKind::Function => {
            "SELECT p.proname AS name, min(pg_catalog.obj_description(p.oid, 'pg_proc')) AS comment \
             FROM pg_catalog.pg_proc p, ns WHERE p.pronamespace = ns.oid AND p.prokind = 'f' GROUP BY p.proname"
        }
        ObjectKind::Procedure => {
            "SELECT p.proname AS name, min(pg_catalog.obj_description(p.oid, 'pg_proc')) AS comment \
             FROM pg_catalog.pg_proc p, ns WHERE p.pronamespace = ns.oid AND p.prokind = 'p' GROUP BY p.proname"
        }
        ObjectKind::Trigger => {
            "SELECT t.tgname AS name, min(pg_catalog.obj_description(t.oid, 'pg_trigger')) AS comment \
             FROM pg_catalog.pg_trigger t JOIN pg_catalog.pg_class c ON c.oid = t.tgrelid, ns \
             WHERE c.relnamespace = ns.oid AND NOT t.tgisinternal GROUP BY t.tgname"
        }
    }
}

/// `LIKE` 의 특수 글자를 글자로.
fn like_escape(filter: &str) -> String {
    let mut out = String::with_capacity(filter.len());
    for c in filter.chars() {
        if matches!(c, '\\' | '%' | '_') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

async fn objects(client: &Client, request: &ObjectQuery) -> DbResult<ObjectPage> {
    if !(1..=500).contains(&request.limit) {
        return Err(invalid("the page size is 1 to 500"));
    }
    let offset: i64 = match &request.cursor {
        None => 0,
        Some(cursor) => cursor
            .parse()
            .ok()
            .filter(|o| *o >= 0)
            .ok_or_else(|| invalid("not a cursor from this driver"))?,
    };
    let sql = format!(
        "WITH ns AS (SELECT oid FROM pg_catalog.pg_namespace WHERE nspname = COALESCE($1::text, pg_catalog.current_schema())) \
         SELECT o.name::text, o.comment FROM ({}) AS o \
         WHERE $2::text IS NULL OR o.name::text ILIKE '%' || $2::text || '%' \
         ORDER BY o.name LIMIT $3::int8 OFFSET $4::int8",
        object_source(request.kind)
    );
    let filter = request
        .filter
        .as_deref()
        .filter(|f| !f.is_empty())
        .map(like_escape);
    let fetch = i64::from(request.limit) + 1;
    let rows = query(client, &sql, &[&request.schema, &filter, &fetch, &offset]).await?;
    let more = rows.len() > request.limit as usize;
    let items = rows
        .iter()
        .take(request.limit as usize)
        .map(|r| {
            Ok(ObjectInfo {
                name: get(r, 0)?,
                kind: request.kind,
                comment: get(r, 1)?,
            })
        })
        .collect::<DbResult<Vec<_>>>()?;
    Ok(ObjectPage {
        next_cursor: more.then(|| (offset + i64::from(request.limit)).to_string()),
        items,
    })
}

/// 테이블 · 뷰 · 시퀀스 이름 → OID.
async fn relation(client: &Client, shared: &Shared, object: &ObjectRef) -> DbResult<u32> {
    same_database(shared, object.database.as_deref())?;
    find_relation(client, object)
        .await?
        .ok_or_else(|| invalid(format!("no table or view named {}", display(object))))
}

async fn find_relation(client: &Client, object: &ObjectRef) -> DbResult<Option<u32>> {
    let rows = query(
        client,
        "SELECT c.oid FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
         WHERE n.nspname = COALESCE($1::text, pg_catalog.current_schema()) AND c.relname = $2::text",
        &[&object.schema, &object.name],
    )
    .await?;
    rows.first().map(|r| get(r, 0)).transpose()
}

fn display(object: &ObjectRef) -> String {
    match &object.schema {
        Some(schema) => format!("{schema}.{}", object.name),
        None => object.name.clone(),
    }
}

async fn columns(client: &Client, oid: u32) -> DbResult<Vec<ColumnInfo>> {
    let rows = query(
        client,
        "SELECT a.attname::text, pg_catalog.format_type(a.atttypid, a.atttypmod), NOT a.attnotnull, \
                pg_catalog.pg_get_expr(d.adbin, d.adrelid), pg_catalog.col_description(a.attrelid, a.attnum) \
         FROM pg_catalog.pg_attribute a \
         LEFT JOIN pg_catalog.pg_attrdef d ON d.adrelid = a.attrelid AND d.adnum = a.attnum \
         WHERE a.attrelid = $1::oid AND a.attnum > 0 AND NOT a.attisdropped ORDER BY a.attnum",
        &[&oid],
    )
    .await?;
    rows.iter()
        .map(|r| {
            Ok(ColumnInfo {
                name: get(r, 0)?,
                db_type: get(r, 1)?,
                nullable: get(r, 2)?,
                default: get(r, 3)?,
                comment: get(r, 4)?,
            })
        })
        .collect()
}

/// 인덱스와 그 키 열(`indkey` 를 한 질의에서 푼다 — 식 인덱스의 열은 그 식, `INCLUDE` 열은 빼고).
async fn indexes(client: &Client, oid: u32) -> DbResult<Vec<IndexInfo>> {
    let rows = query(
        client,
        "SELECT i.relname::text, \
                ARRAY(SELECT COALESCE(a.attname::text, pg_catalog.pg_get_indexdef(x.indexrelid, k.ord::int, true)) \
                      FROM pg_catalog.unnest(x.indkey::int2[]) WITH ORDINALITY AS k(attnum, ord) \
                      LEFT JOIN pg_catalog.pg_attribute a ON a.attrelid = x.indrelid AND a.attnum = k.attnum \
                      WHERE k.ord <= x.indnkeyatts ORDER BY k.ord), \
                x.indisprimary, x.indisunique, x.indpred IS NOT NULL \
         FROM pg_catalog.pg_index x JOIN pg_catalog.pg_class i ON i.oid = x.indexrelid \
         WHERE x.indrelid = $1::oid ORDER BY i.relname",
        &[&oid],
    )
    .await?;
    rows.iter()
        .map(|r| {
            Ok(IndexInfo {
                name: get(r, 0)?,
                columns: get(r, 1)?,
                primary: get(r, 2)?,
                unique: get(r, 3)?,
                partial: get(r, 4)?,
            })
        })
        .collect()
}

async fn foreign_keys(client: &Client, shared: &Shared, oid: u32) -> DbResult<Vec<ForeignKeyInfo>> {
    let rows = query(
        client,
        "SELECT con.conname::text, \
                ARRAY(SELECT a.attname::text FROM pg_catalog.unnest(con.conkey) WITH ORDINALITY AS k(attnum, ord) \
                      JOIN pg_catalog.pg_attribute a ON a.attrelid = con.conrelid AND a.attnum = k.attnum ORDER BY k.ord), \
                rn.nspname::text, rc.relname::text, \
                ARRAY(SELECT a.attname::text FROM pg_catalog.unnest(con.confkey) WITH ORDINALITY AS k(attnum, ord) \
                      JOIN pg_catalog.pg_attribute a ON a.attrelid = con.confrelid AND a.attnum = k.attnum ORDER BY k.ord) \
         FROM pg_catalog.pg_constraint con \
         JOIN pg_catalog.pg_class rc ON rc.oid = con.confrelid \
         JOIN pg_catalog.pg_namespace rn ON rn.oid = rc.relnamespace \
         WHERE con.conrelid = $1::oid AND con.contype = 'f' ORDER BY con.conname",
        &[&oid],
    )
    .await?;
    rows.iter()
        .map(|r| {
            Ok(ForeignKeyInfo {
                name: get(r, 0)?,
                columns: get(r, 1)?,
                references: ObjectRef {
                    database: Some(shared.database.clone()),
                    schema: Some(get(r, 2)?),
                    name: get(r, 3)?,
                },
                referenced_columns: get(r, 4)?,
            })
        })
        .collect()
}

/// DDL 글. 테이블 · 뷰 · 구체화 뷰 · 시퀀스(`pg_class`), 없으면 함수 · 프로시저(`pg_proc`), 없으면 트리거.
async fn ddl(client: &Client, object: &ObjectRef) -> DbResult<String> {
    if let Some(oid) = find_relation(client, object).await? {
        let rows = query(
            client,
            "SELECT c.relkind::text, pg_catalog.quote_ident(n.nspname) || '.' || pg_catalog.quote_ident(c.relname) \
             FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace WHERE c.oid = $1::oid",
            &[&oid],
        )
        .await?;
        let row = rows
            .first()
            .ok_or_else(|| invalid("the object disappeared"))?;
        let kind: String = get(row, 0)?;
        let name: String = get(row, 1)?;
        return match kind.as_str() {
            "r" | "p" => table_ddl(client, oid, &name).await,
            "v" | "m" => view_ddl(client, oid, &name, kind == "m").await,
            "S" => sequence_ddl(client, oid, &name).await,
            other => Err(invalid(format!("no DDL for relation kind `{other}`"))),
        };
    }
    let functions = query(
        client,
        "SELECT pg_catalog.pg_get_functiondef(p.oid) FROM pg_catalog.pg_proc p \
         JOIN pg_catalog.pg_namespace n ON n.oid = p.pronamespace \
         WHERE n.nspname = COALESCE($1::text, pg_catalog.current_schema()) AND p.proname = $2::text \
           AND p.prokind IN ('f', 'p') ORDER BY p.oid",
        &[&object.schema, &object.name],
    )
    .await?;
    if !functions.is_empty() {
        let defs = functions
            .iter()
            .map(|r| get::<String>(r, 0).map(|d| format!("{};", d.trim_end())))
            .collect::<DbResult<Vec<_>>>()?;
        return Ok(defs.join("\n\n"));
    }
    let triggers = query(
        client,
        "SELECT pg_catalog.pg_get_triggerdef(t.oid, true) FROM pg_catalog.pg_trigger t \
         JOIN pg_catalog.pg_class c ON c.oid = t.tgrelid JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
         WHERE n.nspname = COALESCE($1::text, pg_catalog.current_schema()) AND t.tgname = $2::text \
           AND NOT t.tgisinternal ORDER BY c.relname",
        &[&object.schema, &object.name],
    )
    .await?;
    if triggers.is_empty() {
        return Err(invalid(format!("no object named {}", display(object))));
    }
    let defs = triggers
        .iter()
        .map(|r| get::<String>(r, 0).map(|d| format!("{d};")))
        .collect::<DbResult<Vec<_>>>()?;
    Ok(defs.join("\n"))
}

async fn table_ddl(client: &Client, oid: u32, name: &str) -> DbResult<String> {
    let columns = query(
        client,
        "SELECT pg_catalog.quote_ident(a.attname) || ' ' || pg_catalog.format_type(a.atttypid, a.atttypmod) \
                || CASE WHEN a.attidentity = 'a' THEN ' GENERATED ALWAYS AS IDENTITY' \
                        WHEN a.attidentity = 'd' THEN ' GENERATED BY DEFAULT AS IDENTITY' \
                        WHEN a.attgenerated = 's' THEN ' GENERATED ALWAYS AS (' || pg_catalog.pg_get_expr(d.adbin, d.adrelid) || ') STORED' \
                        WHEN d.adbin IS NOT NULL THEN ' DEFAULT ' || pg_catalog.pg_get_expr(d.adbin, d.adrelid) \
                        ELSE '' END \
                || CASE WHEN a.attnotnull THEN ' NOT NULL' ELSE '' END \
         FROM pg_catalog.pg_attribute a \
         LEFT JOIN pg_catalog.pg_attrdef d ON d.adrelid = a.attrelid AND d.adnum = a.attnum \
         WHERE a.attrelid = $1::oid AND a.attnum > 0 AND NOT a.attisdropped ORDER BY a.attnum",
        &[&oid],
    )
    .await?;
    let constraints = query(
        client,
        "SELECT 'CONSTRAINT ' || pg_catalog.quote_ident(conname) || ' ' || pg_catalog.pg_get_constraintdef(oid, true) \
         FROM pg_catalog.pg_constraint WHERE conrelid = $1::oid AND contype IN ('p', 'u', 'c', 'f', 'x') \
         ORDER BY CASE contype WHEN 'p' THEN 0 WHEN 'u' THEN 1 WHEN 'c' THEN 2 WHEN 'f' THEN 3 ELSE 4 END, conname",
        &[&oid],
    )
    .await?;
    let tail = query(
        client,
        "SELECT pg_catalog.pg_get_partkeydef($1::oid), pg_catalog.obj_description($1::oid, 'pg_class')",
        &[&oid],
    )
    .await?;
    // 제약을 받치는 인덱스(PK · UNIQUE · EXCLUDE)는 제약 줄이 만든다.
    let indexes = query(
        client,
        "SELECT pg_catalog.pg_get_indexdef(x.indexrelid) FROM pg_catalog.pg_index x \
         JOIN pg_catalog.pg_class i ON i.oid = x.indexrelid \
         WHERE x.indrelid = $1::oid AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_constraint k \
               WHERE k.conindid = x.indexrelid AND k.contype IN ('p', 'u', 'x')) ORDER BY i.relname",
        &[&oid],
    )
    .await?;
    let column_comments = query(
        client,
        "SELECT 'COMMENT ON COLUMN ' || $2::text || '.' || pg_catalog.quote_ident(a.attname) || ' IS ' \
                || pg_catalog.quote_literal(pg_catalog.col_description(a.attrelid, a.attnum)) \
         FROM pg_catalog.pg_attribute a WHERE a.attrelid = $1::oid AND a.attnum > 0 AND NOT a.attisdropped \
           AND pg_catalog.col_description(a.attrelid, a.attnum) IS NOT NULL ORDER BY a.attnum",
        &[&oid, &name],
    )
    .await?;
    let mut lines = columns
        .iter()
        .map(|r| get::<String>(r, 0))
        .collect::<DbResult<Vec<_>>>()?;
    for row in &constraints {
        lines.push(get(row, 0)?);
    }
    let mut out = format!("CREATE TABLE {name} (\n    {}\n)", lines.join(",\n    "));
    let (partition_key, comment): (Option<String>, Option<String>) = match tail.first() {
        Some(row) => (get(row, 0)?, get(row, 1)?),
        None => (None, None),
    };
    if let Some(key) = partition_key {
        out.push_str(" PARTITION BY ");
        out.push_str(&key);
    }
    out.push_str(";\n");
    for row in &indexes {
        out.push_str(&get::<String>(row, 0)?);
        out.push_str(";\n");
    }
    if let Some(comment) = comment {
        out.push_str(&format!(
            "COMMENT ON TABLE {name} IS {};\n",
            quote_literal(&comment)
        ));
    }
    for row in &column_comments {
        out.push_str(&get::<String>(row, 0)?);
        out.push_str(";\n");
    }
    Ok(out)
}

async fn view_ddl(client: &Client, oid: u32, name: &str, materialized: bool) -> DbResult<String> {
    let rows = query(
        client,
        "SELECT pg_catalog.pg_get_viewdef($1::oid, true)",
        &[&oid],
    )
    .await?;
    let body: String = rows
        .first()
        .map(|r| get(r, 0))
        .transpose()?
        .unwrap_or_default();
    let head = if materialized {
        "CREATE MATERIALIZED VIEW"
    } else {
        "CREATE OR REPLACE VIEW"
    };
    Ok(format!("{head} {name} AS\n{}\n", body.trim_end()))
}

async fn sequence_ddl(client: &Client, oid: u32, name: &str) -> DbResult<String> {
    let rows = query(
        client,
        "SELECT pg_catalog.format_type(s.seqtypid, NULL), s.seqincrement::text, s.seqmin::text, s.seqmax::text, \
                s.seqstart::text, s.seqcache::text, s.seqcycle \
         FROM pg_catalog.pg_sequence s WHERE s.seqrelid = $1::oid",
        &[&oid],
    )
    .await?;
    let row = rows
        .first()
        .ok_or_else(|| invalid("the sequence disappeared"))?;
    let (ty, increment, min, max, start, cache, cycle): (
        String,
        String,
        String,
        String,
        String,
        String,
        bool,
    ) = (
        get(row, 0)?,
        get(row, 1)?,
        get(row, 2)?,
        get(row, 3)?,
        get(row, 4)?,
        get(row, 5)?,
        get(row, 6)?,
    );
    Ok(format!(
        "CREATE SEQUENCE {name} AS {ty} INCREMENT BY {increment} MINVALUE {min} MAXVALUE {max} START WITH {start} CACHE {cache}{};\n",
        if cycle { " CYCLE" } else { " NO CYCLE" }
    ))
}

fn quote_literal(text: &str) -> String {
    let escaped = text.replace('\'', "''");
    if text.contains('\\') {
        format!("E'{}'", escaped.replace('\\', "\\\\"))
    } else {
        format!("'{escaped}'")
    }
}
