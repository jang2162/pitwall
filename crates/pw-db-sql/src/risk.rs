//! 문장 위험도 분류(M3-03, 결정 M3-D5). 분류기는 [`classify_statement`] 하나다 — 실행 관문(M3-05) · 그리드 편집(M3-12) ·
//! MCP(M5)는 이것이나 그 위의 `db.sql.classify` 만 부르고 다른 분류기를 두지 않는다(TS 쪽에도 두지 않는다).
//!
//! 출처: DBX(<https://github.com/t8y2/dbx>) 커밋 `c5e7e0430653aaef198800a48f9dec46a7cfed5e` 의
//! `crates/dbx-sql-core/src/sql_risk.rs` 에서 AST 분류(`classify_statement` · `query_is_write_capable` ·
//! `statement_is_dangerous`)와 잠금절 토큰 검사를 옮겨 수정함. Apache License 2.0 — 고지는 `licenses/vendored.md` 「DBX」.
//! WHERE 항진식 분석은 [`predicate`], 옮긴 시험은 `tests/dbx.rs`.
//!
//! DBX 에서 바꾼 것(14-m3-plan.md M3-03 행의 「고칠 것」):
//! - 등급을 04 F6-9 의 다섯 갈래([`RiskGrade`])로 — DBX 의 `Transaction` 최상위 등급과 별도 불리언 `is_dangerous` 를 버리고,
//!   범위 없는 `UPDATE` · `DELETE` 는 [`RiskGrade::UnboundedWrite`], `DROP` · `TRUNCATE` 는 DDL 과 갈라 [`RiskGrade::Destructive`].
//! - 파싱 실패는 [`RiskGrade::Destructive`] · [`RiskReason::Unparsed`] 다(fail-closed). DBX 의 키워드 폴백은 옮기지 않았다 — 읽기처럼
//!   보이는 파싱 불가 문장을 조회로 답했고, 파서가 못 읽는 `DROP …` 이 확인 없이 돌 수 있었다.
//! - 트랜잭션 제어 · `SET` · `USE` · `LOCK` · `CALL` · `ATTACH` · `VACUUM` · `PRAGMA` · `COPY` 는 쓰기 이상(결정 M3-D6 — read-only
//!   실행이 사람의 `COMMIT` 이나 `SET TRANSACTION READ WRITE` 로 풀리지 않게).
//! - 부작용 함수 거부 목록을 넓혔고([`SIDE_EFFECT_FUNCTIONS`]) `FROM` 의 함수 호출(`dblink(…)`)과 `FROM` 서브쿼리, 조회가 아닌
//!   문장 안까지 본다. 다른 세션 · 서버에 닿는 관리 함수([`SERVER_ADMIN_FUNCTIONS`])는 범위 없는 쓰기다.
//! - DBX 에 없던 것: `EXPLAIN (ANALYZE) …` 옵션 꼴도 안의 문장으로 매긴다, `COPY … PROGRAM` 은 파괴(서버에서 셸 명령을 돌린다),
//!   `ALTER TABLE … DROP COLUMN` 은 파괴, MariaDB 실행 주석 `/*M! … */` 을 펼쳐 읽는다, MySQL `:=` 대입은 쓰기, 문장 첫 낱말이
//!   `DROP` · `TRUNCATE` · `CREATE` 류면 AST 가 무엇이든 그 등급 아래로 내려가지 않는다.
//!
//! 거부 목록은 완전할 수 없다 — 목록 밖 사용자 정의 함수가 든 `SELECT` 는 조회로 나온다. 마지막 방어는 DB 수준 읽기 전용
//! 트랜잭션이다(결정 M3-D6, 실행 파이프라인 M3-05).

use std::borrow::Cow;
use std::ops::ControlFlow;

use pw_db_types::{RiskGrade, SqlDialect};
use serde::{Deserialize, Serialize};
use sqlparser::ast::{
    AlterTableOperation, CopyTarget, Delete, Expr, FromTable, ObjectName, Query, Set, SetExpr,
    SqliteOnConflict, Statement, TableFactor, Update, Visit, Visitor,
};
use sqlparser::dialect::{Dialect, MySqlDialect, PostgreSqlDialect, SQLiteDialect};
use sqlparser::parser::Parser;
use sqlparser::tokenizer::{Token, Tokenizer, Whitespace};

mod predicate;
#[cfg(test)]
mod tests;

use predicate::predicate_is_obviously_unbounded;

/// 문장 하나(또는 그것으로 받은 글)의 분류 결과.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct Classification {
    pub grade: RiskGrade,
    /// 그 등급이 된 까닭(`grade` 와 같은 등급의 까닭만, 선언 순서). 조회면 비어 있다.
    pub reasons: Vec<RiskReason>,
}

/// 까닭. 창(M3-19 의 확인 다이얼로그)이 문구로 바꿔 보인다 — 이름을 바꾸면 그 문구 표도 고친다.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    specta::Type,
    schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum RiskReason {
    /// 파서가 읽지 못했다(방언에 없는 문법 · 끊긴 문장). 파괴.
    Unparsed,
    /// `DROP …`, `ALTER TABLE … DROP COLUMN` · `DROP PARTITION`. 파괴.
    Drop,
    /// `TRUNCATE`. 파괴.
    Truncate,
    /// `COPY … TO/FROM PROGRAM` — DB 서버에서 셸 명령을 돌린다. 파괴.
    CopyProgram,
    /// `CREATE` · `ALTER` · `GRANT` · `REVOKE` · `COMMENT ON` · `RENAME`, PG 의 `SELECT … INTO 새 표`.
    Ddl,
    /// WHERE 없는 `UPDATE` · `DELETE`. 범위 없는 쓰기.
    MissingWhere,
    /// WHERE 가 항진식이거나 서브쿼리라 범위를 보장하지 않는다(`WHERE 1=1`, `id = id`, `x IS NULL OR x IS NOT NULL`). 범위 없는 쓰기.
    UnboundedWhere,
    /// 조인 · `FROM` · `USING` · 여러 표를 건드리는 `UPDATE` · `DELETE`. 범위 없는 쓰기.
    MultiTable,
    /// SQLite `UPDATE OR REPLACE` — 충돌한 다른 행을 지운다. 범위 없는 쓰기.
    OrReplace,
    /// `MERGE`. 범위 없는 쓰기.
    Merge,
    /// `INSERT` · `UPDATE` · `DELETE`(범위가 있는 것).
    Dml,
    /// `FOR UPDATE` · `FOR SHARE` · `LOCK IN SHARE MODE` 잠금 읽기.
    LockingRead,
    /// 부작용 함수(`nextval` · `pg_sleep` · `GET_LOCK` …)를 부르는 조회.
    SideEffectFunction,
    /// 다른 세션 · 서버 전체에 닿는 관리 함수(`pg_terminate_backend` · `pg_reload_conf` …). 범위 없는 쓰기.
    ServerAdminFunction,
    /// `SELECT … INTO`(MySQL 의 변수 · 파일, SQLite).
    SelectInto,
    /// `BEGIN` · `COMMIT` · `ROLLBACK` · `SAVEPOINT` · `SET TRANSACTION`.
    TransactionControl,
    /// `SET` · `USE` · `RESET` · `DISCARD` · MySQL 의 `@변수 := …`.
    SessionState,
    /// `LOCK TABLE(S)` · `UNLOCK TABLES`.
    Lock,
    /// `CALL` · `EXECUTE` · `PREPARE`.
    Procedural,
    /// `ATTACH` · `DETACH`.
    Attach,
    /// `PRAGMA`.
    Pragma,
    /// `VACUUM` · `ANALYZE` · `OPTIMIZE TABLE`.
    Maintenance,
    /// `COPY` · `LOAD DATA` 류.
    DataTransfer,
    /// 위 갈래에 들지 않는 문장(`KILL` · `FLUSH` · `LISTEN` …). 쓰기로 본다.
    Other,
}

/// 문장 하나를 분류한다. 방언은 연결의 드라이버로 정한다.
///
/// 글에 문장이 여럿 들어 있으면(나누기는 M3-04 의 일이지만 받은 글을 믿지 않는다) 가장 높은 등급이다. 문장이 없는 글(주석 ·
/// 빈칸뿐)은 아무것도 돌리지 않으므로 조회다.
pub fn classify_statement(dialect: SqlDialect, text: &str) -> Classification {
    let parser_dialect = parser_dialect(dialect);
    let text = expand_mariadb_executable_comments(dialect, text);
    let (Ok(statements), Ok(tokens)) = (
        Parser::parse_sql(parser_dialect.as_ref(), &text),
        Tokenizer::new(parser_dialect.as_ref(), &text).tokenize(),
    ) else {
        return Classification {
            grade: RiskGrade::Destructive,
            reasons: vec![RiskReason::Unparsed],
        };
    };

    let mut found = Found::default();
    for statement in &statements {
        classify_ast(statement, dialect, &mut found);
    }
    leading_keyword_floor(&tokens, &mut found);
    if contains_locking_clause(&tokens) {
        found.add(RiskGrade::Write, RiskReason::LockingRead);
    }
    // MySQL 의 `SELECT @a := 1` 은 세션 변수를 바꾼다. PG 의 `:=` 는 함수의 이름 붙은 인자라 MySQL 에서만 본다.
    if dialect == SqlDialect::Mysql
        && tokens
            .iter()
            .any(|token| matches!(token, Token::Assignment))
    {
        found.add(RiskGrade::Write, RiskReason::SessionState);
    }
    found.finish()
}

fn parser_dialect(dialect: SqlDialect) -> Box<dyn Dialect> {
    match dialect {
        SqlDialect::Postgres => Box::new(PostgreSqlDialect {}),
        SqlDialect::Mysql => Box::new(MySqlDialect {}),
        SqlDialect::Sqlite => Box::new(SQLiteDialect {}),
    }
}

/// 찾은 (등급, 까닭)을 모은다. 결과는 가장 높은 등급과 그 등급의 까닭이다.
#[derive(Default)]
struct Found(Vec<(RiskGrade, RiskReason)>);

impl Found {
    fn add(&mut self, grade: RiskGrade, reason: RiskReason) {
        self.0.push((grade, reason));
    }

    fn finish(self) -> Classification {
        let grade = self
            .0
            .iter()
            .map(|(grade, _)| *grade)
            .max()
            .unwrap_or(RiskGrade::Read);
        let mut reasons: Vec<RiskReason> = self
            .0
            .into_iter()
            .filter(|(g, _)| *g == grade)
            .map(|(_, reason)| reason)
            .collect();
        reasons.sort_unstable();
        reasons.dedup();
        Classification { grade, reasons }
    }
}

fn classify_ast(statement: &Statement, dialect: SqlDialect, found: &mut Found) {
    use RiskGrade::{Ddl, Destructive, UnboundedWrite, Write};
    // 조회는 [`classify_query`] 가 함수를 보고, `EXPLAIN` 은 `ANALYZE` 일 때 안의 문장으로 내려가 본다(돌리지 않는 `EXPLAIN` 안의
    // 함수는 돌지 않는다). 그 밖의 문장(`INSERT … SELECT pg_terminate_backend(pid) …` 등)은 여기서 본다.
    if !matches!(statement, Statement::Query(_) | Statement::Explain { .. }) {
        classify_functions(statement, found);
    }
    match statement {
        Statement::Query(query) => classify_query(query, dialect, found),
        // `EXPLAIN` 은 돌리지 않지만 `EXPLAIN ANALYZE` 는 안의 문장을 돌린다. PG 의 `EXPLAIN (ANALYZE false) …` 도 안의 문장으로
        // 매긴다 — 옵션 값까지 읽어 낮추는 이득보다 잘못 읽을 위험이 크다.
        Statement::Explain {
            analyze,
            options,
            statement,
            ..
        } => {
            let analyze_option = options.as_ref().is_some_and(|options| {
                options
                    .iter()
                    .any(|option| option.name.value.eq_ignore_ascii_case("analyze"))
            });
            if *analyze || analyze_option {
                classify_ast(statement, dialect, found);
            }
        }
        Statement::ExplainTable { .. } => {}
        statement if is_show_read_only_statement(statement) => {}

        Statement::Insert { .. } => found.add(Write, RiskReason::Dml),
        Statement::Update(update) => classify_update(update, found),
        Statement::Delete(delete) => classify_delete(delete, found),
        Statement::Merge { .. } => found.add(UnboundedWrite, RiskReason::Merge),

        Statement::Drop { .. }
        | Statement::DropFunction { .. }
        | Statement::DropDomain { .. }
        | Statement::DropProcedure { .. }
        | Statement::DropSecret { .. }
        | Statement::DropPolicy { .. }
        | Statement::DropConnector { .. }
        | Statement::DropExtension { .. }
        | Statement::DropOperator { .. }
        | Statement::DropOperatorFamily { .. }
        | Statement::DropOperatorClass { .. }
        | Statement::DropTrigger { .. } => found.add(Destructive, RiskReason::Drop),
        Statement::Truncate { .. } => found.add(Destructive, RiskReason::Truncate),
        Statement::AlterTable(alter) => {
            found.add(Ddl, RiskReason::Ddl);
            // 열 · 파티션을 버리는 ALTER 는 데이터를 지운다.
            if alter.operations.iter().any(|operation| {
                matches!(
                    operation,
                    AlterTableOperation::DropColumn { .. }
                        | AlterTableOperation::DropPartitions { .. }
                )
            }) {
                found.add(Destructive, RiskReason::Drop);
            }
        }
        Statement::CreateView { .. }
        | Statement::CreateTable { .. }
        | Statement::CreateVirtualTable { .. }
        | Statement::CreateIndex { .. }
        | Statement::CreateRole { .. }
        | Statement::CreateSecret { .. }
        | Statement::CreateServer { .. }
        | Statement::CreatePolicy { .. }
        | Statement::CreateConnector { .. }
        | Statement::CreateOperator { .. }
        | Statement::CreateOperatorFamily { .. }
        | Statement::CreateOperatorClass { .. }
        | Statement::CreateTextSearch { .. }
        | Statement::CreateExtension { .. }
        | Statement::CreateCollation { .. }
        | Statement::CreateSchema { .. }
        | Statement::CreateDatabase { .. }
        | Statement::CreateFunction { .. }
        | Statement::CreateTrigger { .. }
        | Statement::CreateProcedure { .. }
        | Statement::CreateMacro { .. }
        | Statement::CreateStage { .. }
        | Statement::CreateFileFormat { .. }
        | Statement::CreateWarehouse { .. }
        | Statement::CreateSequence { .. }
        | Statement::CreateDomain { .. }
        | Statement::CreateType { .. }
        | Statement::CreateUser { .. }
        | Statement::AlterSchema { .. }
        | Statement::AlterIndex { .. }
        | Statement::AlterView { .. }
        | Statement::AlterFunction { .. }
        | Statement::AlterType { .. }
        | Statement::AlterCollation { .. }
        | Statement::AlterOperator { .. }
        | Statement::AlterOperatorFamily { .. }
        | Statement::AlterOperatorClass { .. }
        | Statement::AlterTextSearch { .. }
        | Statement::AlterRole { .. }
        | Statement::AlterPolicy { .. }
        | Statement::AlterConnector { .. }
        | Statement::AlterUser { .. }
        | Statement::Grant { .. }
        | Statement::Deny { .. }
        | Statement::Revoke { .. }
        | Statement::Comment { .. }
        | Statement::RenameTable { .. } => found.add(Ddl, RiskReason::Ddl),

        Statement::StartTransaction { .. }
        | Statement::Commit { .. }
        | Statement::Rollback { .. }
        | Statement::Savepoint { .. }
        | Statement::ReleaseSavepoint { .. }
        | Statement::Set(Set::SetTransaction { .. }) => {
            found.add(Write, RiskReason::TransactionControl)
        }
        Statement::Set(_)
        | Statement::Use { .. }
        | Statement::Reset { .. }
        | Statement::Discard { .. }
        | Statement::Deallocate { .. }
        | Statement::AlterSession { .. } => found.add(Write, RiskReason::SessionState),
        Statement::Lock { .. } | Statement::LockTables { .. } | Statement::UnlockTables => {
            found.add(Write, RiskReason::Lock)
        }
        Statement::Call { .. } | Statement::Execute { .. } => {
            found.add(Write, RiskReason::Procedural)
        }
        // 준비한 문장은 뒤의 `EXECUTE` 에서 돈다 — 그때는 무엇인지 모르므로 준비할 때 안의 문장으로 매긴다.
        Statement::Prepare { statement, .. } => {
            found.add(Write, RiskReason::Procedural);
            classify_ast(statement, dialect, found);
        }
        Statement::Copy {
            target: CopyTarget::Program { .. },
            ..
        } => found.add(Destructive, RiskReason::CopyProgram),
        Statement::Copy { .. }
        | Statement::CopyIntoSnowflake { .. }
        | Statement::LoadData { .. }
        | Statement::Directory { .. }
        | Statement::Unload { .. }
        | Statement::ExportData { .. } => found.add(Write, RiskReason::DataTransfer),
        Statement::AttachDatabase { .. }
        | Statement::AttachDuckDBDatabase { .. }
        | Statement::DetachDuckDBDatabase { .. } => found.add(Write, RiskReason::Attach),
        // DBX 의 키워드 경로는 안전한 pragma 8개를 조회로 허용했다 — 옮기지 않는다(값을 바꾸는 꼴과 읽는 꼴이 같은 이름이다).
        Statement::Pragma { .. } => found.add(Write, RiskReason::Pragma),
        Statement::Vacuum { .. } | Statement::Analyze { .. } | Statement::OptimizeTable { .. } => {
            found.add(Write, RiskReason::Maintenance)
        }
        // 갈래에 없는 문장은 쓰기다(DBX 의 catch-all 과 같다). 파괴 · DDL 을 놓치지 않는 것은 위 목록과 [`leading_keyword_floor`] 다.
        _ => found.add(Write, RiskReason::Other),
    }
}

/// SHOW · DESCRIBE 꼴 가운데 파서가 읽기로 모형화한 것. 목록 밖 SHOW 는 쓰기(`Other`)로 떨어진다 — 안전한 쪽이다.
fn is_show_read_only_statement(statement: &Statement) -> bool {
    matches!(
        statement,
        Statement::ShowTables { .. }
            | Statement::ShowColumns { .. }
            | Statement::ShowCatalogs { .. }
            | Statement::ShowDatabases { .. }
            | Statement::ShowSchemas { .. }
            | Statement::ShowViews { .. }
            | Statement::ShowFunctions { .. }
            | Statement::ShowCreate { .. }
            | Statement::ShowVariable { .. }
            | Statement::ShowVariables { .. }
            | Statement::ShowStatus { .. }
            | Statement::ShowProcessList { .. }
            | Statement::ShowCharset(_)
            | Statement::ShowObjects(_)
            | Statement::ShowCollation { .. }
    )
}

fn classify_update(update: &Update, found: &mut Found) {
    found.add(RiskGrade::Write, RiskReason::Dml);
    classify_selection(update.selection.as_ref(), found);
    if !update.table.joins.is_empty() || update.from.is_some() {
        found.add(RiskGrade::UnboundedWrite, RiskReason::MultiTable);
    }
    if matches!(update.or, Some(SqliteOnConflict::Replace)) {
        found.add(RiskGrade::UnboundedWrite, RiskReason::OrReplace);
    }
}

fn classify_delete(delete: &Delete, found: &mut Found) {
    found.add(RiskGrade::Write, RiskReason::Dml);
    classify_selection(delete.selection.as_ref(), found);
    let from = match &delete.from {
        FromTable::WithFromKeyword(from) | FromTable::WithoutKeyword(from) => from,
    };
    if !delete.tables.is_empty()
        || delete.using.is_some()
        || from.len() != 1
        || from.first().is_none_or(|table| !table.joins.is_empty())
    {
        found.add(RiskGrade::UnboundedWrite, RiskReason::MultiTable);
    }
}

fn classify_selection(selection: Option<&Expr>, found: &mut Found) {
    match selection {
        None => found.add(RiskGrade::UnboundedWrite, RiskReason::MissingWhere),
        Some(predicate) if predicate_is_obviously_unbounded(predicate) => {
            found.add(RiskGrade::UnboundedWrite, RiskReason::UnboundedWhere)
        }
        Some(_) => {}
    }
}

fn classify_query(query: &Query, dialect: SqlDialect, found: &mut Found) {
    if let Some(with) = &query.with {
        for cte in &with.cte_tables {
            classify_query(&cte.query, dialect, found);
        }
    }
    classify_set_expr(&query.body, dialect, found);
    if !query.locks.is_empty() {
        found.add(RiskGrade::Write, RiskReason::LockingRead);
    }
    classify_functions(query, found);
}

/// 문장 안 어디든(서브쿼리 · CTE · 함수 인자 · `FROM` 의 표 함수) 부르는 부작용 함수를 등급에 더한다.
fn classify_functions(node: &impl Visit, found: &mut Found) {
    let mut visitor = FunctionVisitor::default();
    let _ = node.visit(&mut visitor);
    match visitor.effect {
        Some(FunctionEffect::ServerAdmin) => {
            found.add(RiskGrade::UnboundedWrite, RiskReason::ServerAdminFunction)
        }
        Some(FunctionEffect::SideEffect) => {
            found.add(RiskGrade::Write, RiskReason::SideEffectFunction)
        }
        None => {}
    }
}

fn classify_set_expr(expr: &SetExpr, dialect: SqlDialect, found: &mut Found) {
    match expr {
        SetExpr::Select(select) => {
            if select.into.is_some() {
                // PG 의 `SELECT … INTO 표` 는 `CREATE TABLE … AS` 와 같다. MySQL 은 변수 · 파일, SQLite 는 오류다.
                if dialect == SqlDialect::Postgres {
                    found.add(RiskGrade::Ddl, RiskReason::Ddl);
                } else {
                    found.add(RiskGrade::Write, RiskReason::SelectInto);
                }
            }
        }
        SetExpr::Query(query) => classify_query(query, dialect, found),
        SetExpr::SetOperation { left, right, .. } => {
            classify_set_expr(left, dialect, found);
            classify_set_expr(right, dialect, found);
        }
        SetExpr::Insert(statement)
        | SetExpr::Update(statement)
        | SetExpr::Delete(statement)
        | SetExpr::Merge(statement) => classify_ast(statement, dialect, found),
        SetExpr::Values(_) | SetExpr::Table(_) => {}
    }
}

/// 다른 세션을 끊거나 서버 설정 · 로그를 바꾸는 관리 함수. PG 는 `BEGIN READ ONLY` 안에서도 이것들을 막지 않는다(M3-06 이
/// live PG 17.6 에서 `pg_terminate_backend` 로 확인) — DB 수준 읽기 전용이 받쳐 주지 못하므로 분류가 첫 방어이고, 영향이 그
/// 문장의 표를 넘어 다른 세션 · 서버 전체에 닿으므로 쓰기보다 한 단계 높은 범위 없는 쓰기로 매긴다(결정 M3-D20 에서 어느
/// 연결이든 확인을 받는다). 이름은 [`SIDE_EFFECT_FUNCTIONS`] 와 같은 규칙으로 맞춘다.
const SERVER_ADMIN_FUNCTIONS: &[&str] = &[
    "pg_terminate_backend",
    "pg_cancel_backend",
    "pg_reload_conf",
    "pg_rotate_logfile",
    // Kingbase(DBX 목록에서 그대로)
    "sys_cancel_backend",
];

/// 조회를 쓰기로 만드는 부작용 함수(스키마 이름은 보지 않고 마지막 이름만, 대소문자 무시). DBX 의 `SIDE_EFFECT_SELECT_FUNCTIONS`
/// (관리 함수는 [`SERVER_ADMIN_FUNCTIONS`] 로 옮겼다)에 14-m3-plan.md M3-03 행 ④ 의 것과 SQLite `load_extension`(네이티브 코드를
/// 올린다) · MySQL `release_all_locks` · PG `pg_read_binary_file` 을 더했다.
const SIDE_EFFECT_FUNCTIONS: &[&str] = &[
    // PG
    "nextval",
    "setval",
    "set_config",
    "pg_read_file",
    "pg_read_binary_file",
    // MySQL · MariaDB
    "sleep",
    "get_lock",
    "release_lock",
    "release_all_locks",
    "benchmark",
    "load_file",
    // SQLite
    "load_extension",
];

/// 이 접두사로 시작하는 이름도 부작용 함수다: `pg_sleep` · `pg_sleep_for` · `pg_sleep_until`, `pg_advisory_lock` 류와
/// `pg_try_advisory_*`, `dblink` · `dblink_exec` 류, `lo_import` · `lo_export` · `lo_create` · `lo_unlink` 류.
const SIDE_EFFECT_FUNCTION_PREFIXES: &[&str] = &[
    "pg_sleep",
    "pg_advisory",
    "pg_try_advisory",
    "dblink",
    "lo_",
];

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum FunctionEffect {
    SideEffect,
    ServerAdmin,
}

fn function_effect(name: &ObjectName) -> Option<FunctionEffect> {
    let ident = name.0.last().and_then(|part| part.as_ident())?;
    let name = ident.value.to_ascii_lowercase();
    if SERVER_ADMIN_FUNCTIONS.contains(&name.as_str()) {
        Some(FunctionEffect::ServerAdmin)
    } else if SIDE_EFFECT_FUNCTIONS.contains(&name.as_str())
        || SIDE_EFFECT_FUNCTION_PREFIXES
            .iter()
            .any(|prefix| name.starts_with(prefix))
    {
        Some(FunctionEffect::SideEffect)
    } else {
        None
    }
}

/// 문장 전체(CTE · `FROM` 서브쿼리 · `LATERAL` · `ORDER BY` · 함수 인자 포함)에서 가장 센 부작용 함수를 찾는다. DBX 는
/// `visit_expressions` 로 표현식만 봐서 `FROM dblink(…)` 같은 표 함수를 놓쳤다.
#[derive(Default)]
struct FunctionVisitor {
    effect: Option<FunctionEffect>,
}

impl FunctionVisitor {
    fn see(&mut self, name: &ObjectName) -> ControlFlow<()> {
        self.effect = self.effect.max(function_effect(name));
        // 가장 센 것을 찾았으면 더 볼 것이 없다.
        if self.effect == Some(FunctionEffect::ServerAdmin) {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    }
}

impl Visitor for FunctionVisitor {
    type Break = ();

    fn pre_visit_expr(&mut self, expr: &Expr) -> ControlFlow<()> {
        match expr {
            Expr::Function(function) => self.see(&function.name),
            _ => ControlFlow::Continue(()),
        }
    }

    fn pre_visit_table_factor(&mut self, table_factor: &TableFactor) -> ControlFlow<()> {
        match table_factor {
            TableFactor::Table {
                name,
                args: Some(_),
                ..
            }
            | TableFactor::Function { name, .. } => self.see(name),
            _ => ControlFlow::Continue(()),
        }
    }
}

/// 잠금절(`FOR UPDATE` · `FOR SHARE` · `FOR KEY SHARE` · `FOR NO KEY UPDATE` · MySQL `LOCK IN SHARE MODE`)을 토큰으로 찾는다.
/// AST 의 `query.locks` 는 맨 바깥 조회의 것만이라 서브쿼리 · 파생 표 안의 잠금은 이것이 잡는다. 따옴표 친 이름 · 문자열 ·
/// 주석 안의 낱말은 보지 않는다(주석은 빈칸 토큰이라 낱말 사이에 끼어도 잇는다).
fn contains_locking_clause(tokens: &[Token]) -> bool {
    let mut words: Vec<String> = Vec::with_capacity(4);
    for token in tokens {
        match token {
            Token::Word(word) if word.quote_style.is_none() => {
                words.push(word.value.to_ascii_uppercase());
                if words.len() > 4 {
                    words.remove(0);
                }
                if ends_with(&words, &["FOR", "UPDATE"])
                    || ends_with(&words, &["FOR", "SHARE"])
                    || ends_with(&words, &["FOR", "KEY", "SHARE"])
                    || ends_with(&words, &["FOR", "NO", "KEY", "UPDATE"])
                    || ends_with(&words, &["LOCK", "IN", "SHARE", "MODE"])
                {
                    return true;
                }
            }
            Token::Whitespace(_) => {}
            _ => words.clear(),
        }
    }
    false
}

fn ends_with(words: &[String], pattern: &[&str]) -> bool {
    words.len() >= pattern.len()
        && words[words.len() - pattern.len()..]
            .iter()
            .zip(pattern)
            .all(|(w, p)| w == p)
}

/// 문장마다 첫 낱말이 파괴 · DDL 동사면 그 등급 아래로 내려가지 않게 한다. AST 의 갈래 목록이 파서의 새 변형(`DROP …` 의 새
/// 꼴)을 놓쳐도 쓰기로 떨어지지 않게 하는 바닥이다 — 파싱이 된 글에만 쓰고, 등급을 올리기만 한다.
fn leading_keyword_floor(tokens: &[Token], found: &mut Found) {
    let mut at_start = true;
    for token in tokens {
        match token {
            Token::Whitespace(_) => {}
            Token::SemiColon => at_start = true,
            Token::Word(word) if at_start => {
                at_start = false;
                if word.quote_style.is_some() {
                    continue;
                }
                match word.value.to_ascii_uppercase().as_str() {
                    "DROP" => found.add(RiskGrade::Destructive, RiskReason::Drop),
                    "TRUNCATE" => found.add(RiskGrade::Destructive, RiskReason::Truncate),
                    "CREATE" | "ALTER" | "GRANT" | "REVOKE" | "RENAME" | "COMMENT" => {
                        found.add(RiskGrade::Ddl, RiskReason::Ddl)
                    }
                    _ => {}
                }
            }
            _ => at_start = false,
        }
    }
}

/// MariaDB 는 `/*M!NNNNNN … */` 안을 실행한다. 파서는 MySQL 의 `/*! … */` 만 펼치므로, `M` 을 걷어 같은 꼴로 만든 뒤 읽는다.
/// 이것을 걷지 않으면 `/*M! DROP TABLE t */` 이 주석으로 읽혀 조회가 된다.
fn expand_mariadb_executable_comments(dialect: SqlDialect, text: &str) -> Cow<'_, str> {
    if dialect != SqlDialect::Mysql || !text.contains("/*M!") {
        return Cow::Borrowed(text);
    }
    // 토큰을 못 만들면 파싱도 실패해 어차피 파괴 · `unparsed` 다.
    let Ok(tokens) = Tokenizer::new(&MySqlDialect {}, text).tokenize_with_location() else {
        return Cow::Borrowed(text);
    };
    let line_starts: Vec<usize> = std::iter::once(0)
        .chain(text.match_indices('\n').map(|(index, _)| index + 1))
        .collect();
    let mut marks: Vec<usize> = tokens
        .iter()
        .filter(|token| {
            matches!(&token.token, Token::Whitespace(Whitespace::MultiLineComment(comment)) if comment.starts_with("M!"))
        })
        .filter_map(|token| {
            let start = token.span.start;
            let line_start = *line_starts.get(usize::try_from(start.line).ok()?.checked_sub(1)?)?;
            let column = usize::try_from(start.column).ok()?.checked_sub(1)?;
            let offset = line_start + text[line_start..].char_indices().nth(column)?.0;
            // `/*` 다음의 `M` 이다.
            text[offset..].starts_with("/*M!").then_some(offset + 2)
        })
        .collect();
    if marks.is_empty() {
        return Cow::Borrowed(text);
    }
    marks.sort_unstable();
    let mut expanded = text.to_owned();
    for mark in marks.into_iter().rev() {
        expanded.remove(mark);
    }
    Cow::Owned(expanded)
}
