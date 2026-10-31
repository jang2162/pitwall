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
//! - DBX 에 없던 것: `EXPLAIN (ANALYZE) …` · `(ANALYSE)` 옵션 꼴도 안의 문장으로 매긴다, `COPY … PROGRAM` 은 파괴(서버에서 셸
//!   명령을 돌린다), `ALTER TABLE … DROP COLUMN` 은 파괴, MySQL `:=` 대입은 쓰기, MySQL `KILL` · `FLUSH` · `SET GLOBAL` 은 관리 함수와
//!   같은 범위 없는 쓰기, 문장 첫 낱말이 `DROP` · `TRUNCATE` · `CREATE` 류면 AST 가 무엇이든 그 등급 아래로 내려가지 않는다.
//! - MySQL 글은 서버가 고를 수 있는 읽기(실행 주석의 판본 · 백슬래시 이스케이프)를 모두 분류한다([`mysql`]).
//! - 파서가 못 읽는 흔한 문장 몇은 정확한 꼴일 때만 조회가 아닌 등급으로 인식한다([`recognize`]).
//! - 입력 상한: 글이 [`MAX_STATEMENT_BYTES`] 를 넘거나 연산자 토큰이 [`MAX_OPERATOR_TOKENS`] 를 넘으면 파괴 · `tooLarge`, 중첩이
//!   파서의 재귀 한도를 넘으면 파괴 · `tooDeep`. 근거는 그 상수들의 주석.
//!
//! 거부 목록은 완전할 수 없다 — 목록 밖 사용자 정의 함수가 든 `SELECT` 는 조회로 나온다. 마지막 방어는 DB 수준 읽기 전용
//! 트랜잭션이다(결정 M3-D6, 실행 파이프라인 M3-05).

use std::ops::ControlFlow;

use pw_db_types::{RiskGrade, SqlDialect};
use serde::{Deserialize, Serialize};
use sqlparser::ast::{
    AlterTableOperation, ContextModifier, CopyTarget, Delete, Expr, FromTable, ObjectName, Query,
    Set, SetExpr, SqliteOnConflict, Statement, TableFactor, Update, Visit, Visitor,
};
use sqlparser::dialect::{Dialect, MySqlDialect, PostgreSqlDialect, SQLiteDialect};
use sqlparser::keywords::Keyword;
use sqlparser::parser::{Parser, ParserError};
use sqlparser::tokenizer::{Token, TokenWithSpan, Tokenizer};

mod mysql;
mod predicate;
mod recognize;
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
    /// 파서가 읽지 못했다(방언에 없는 문법 · 끊긴 문장), 또는 MySQL 글을 서버 설정마다 다르게 나눈다. 파괴.
    Unparsed,
    /// 글이 크기 · 연산자 수 상한을 넘어 읽지 않았다([`MAX_STATEMENT_BYTES`] · [`MAX_OPERATOR_TOKENS`]). 파괴.
    TooLarge,
    /// 괄호 · 서브쿼리 · 단항 연산자의 중첩이 파서의 재귀 한도([`RECURSION_LIMIT`])를 넘었다. 파괴.
    TooDeep,
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
    /// 다른 세션 · 서버 전체에 닿는 관리 작업 — 함수(`pg_terminate_backend` · `pg_reload_conf` · `pg_notify` …)와 MySQL `KILL` ·
    /// `FLUSH` · `SET GLOBAL`. 범위 없는 쓰기.
    ServerAdmin,
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
    /// 위 갈래에 들지 않는 문장(`LISTEN` · `NOTIFY` …). 쓰기로 본다.
    Other,
}

/// 문장 하나의 바이트 상한. 넘으면 읽지 않고 파괴 · `tooLarge`. MySQL 은 읽기 수만큼 나눠 쓴다([`mysql`] — 글 길이 × 읽기 수가
/// 이 값을 넘으면 `tooLarge`).
///
/// 근거(release, `crates/pw-db-sql/examples/classify_bench.rs` — 다시 잴 때 그 머리 주석의 명령): 분류 시간은 평평한 큰 문장
/// (`INSERT … VALUES` 여러 행)에서 길이에 비례해 가장 길고, 1 MiB 에서 중앙값 약 160 ms · 최대 약 200 ms 였다 — 문장당 200 ms
/// 목표에 여유를 두려고 그 절반으로 둔다. 이보다 큰 문장(mysqldump 의 확장 INSERT 는 `--net-buffer-length` 까지 커진다)은 확인을
/// 받는다.
pub const MAX_STATEMENT_BYTES: usize = 512 << 10;

/// 연산자 토큰(연산자 기호와 따옴표 없는 키워드 — `NULL` · `TRUE` · `FALSE` · `DEFAULT` 빼고)의 상한. 넘으면 파괴 · `tooLarge`.
///
/// 같은 우선순위 연산자의 사슬(`1+1+…`, `a AND b AND …`)은 파서가 고리로 쌓아 재귀 한도에 걸리지 않지만 그 길이만큼 깊은 AST 가
/// 되고, 그 AST 를 버리는(`Drop`) 재귀와 방문 재귀가 2 MiB 스택(tokio 작업 스레드 기본)에서 넘쳐 프로세스가 abort 된다(잡을 수
/// 없다). 근거(같은 예제의 `stack`): debug 빌드에서 이 상한 근처의 사슬 여섯 꼴이 768 KiB 스택에서도 넘치지 않고(2 MiB 의 2.6 배
/// 여유), 1만이면 1 MiB 에서 넘쳤다. release 는 이 상한에서 꼴마다 수 ms 다. 연산자를 세는 것은 큰 `INSERT` 를 막지 않으려고다 —
/// 값 목록의 쉼표 · 괄호 · 부호는 세지 않는다.
pub const MAX_OPERATOR_TOKENS: usize = 5_000;

/// 파서의 재귀 한도(sqlparser 기본값과 같다). 괄호 · 서브쿼리 · 단항 연산자의 중첩이 이것을 넘으면 파괴 · `tooDeep`.
pub const RECURSION_LIMIT: usize = 50;

/// 문장 하나를 분류한다. 방언은 연결의 드라이버로 정한다(실행 관문은 요청의 방언이 아니라 연결의 것을 쓴다 — M3-05).
///
/// 글에 문장이 여럿 들어 있으면(나누기는 M3-04 의 일이지만 받은 글을 믿지 않는다) 가장 높은 등급이다. 문장이 없는 글(주석 ·
/// 빈칸뿐)은 아무것도 돌리지 않으므로 조회다.
pub fn classify_statement(dialect: SqlDialect, text: &str) -> Classification {
    if text.len() > MAX_STATEMENT_BYTES {
        return Classification::destructive(RiskReason::TooLarge);
    }
    match dialect {
        SqlDialect::Mysql => mysql::classify(text),
        SqlDialect::Postgres | SqlDialect::Sqlite => classify_text(dialect, text).classification,
    }
}

/// 한 읽기의 분류. `statements` 는 파서가 읽은 문장 수(읽지 못했거나 인식기로 매겼으면 `None`).
struct Outcome {
    classification: Classification,
    statements: Option<usize>,
}

fn classify_text(dialect: SqlDialect, text: &str) -> Outcome {
    let not_parsed = |reason| Outcome {
        classification: Classification::destructive(reason),
        statements: None,
    };
    let parser_dialect = parser_dialect(dialect);
    let Ok(tokens) = Tokenizer::new(parser_dialect.as_ref(), text).tokenize_with_location() else {
        return not_parsed(RiskReason::Unparsed);
    };
    if operator_tokens(&tokens) > MAX_OPERATOR_TOKENS {
        return not_parsed(RiskReason::TooLarge);
    }
    let parsed = Parser::new(parser_dialect.as_ref())
        .with_recursion_limit(RECURSION_LIMIT)
        .with_tokens_with_locations(tokens.clone())
        .parse_statements();
    let statements = match parsed {
        Ok(statements) => statements,
        Err(ParserError::RecursionLimitExceeded) => return not_parsed(RiskReason::TooDeep),
        Err(_) => {
            return match recognize::recognize(dialect, text, &tokens) {
                Some(classification) => Outcome {
                    classification,
                    statements: None,
                },
                None => not_parsed(RiskReason::Unparsed),
            };
        }
    };

    let mut found = Found::default();
    for statement in &statements {
        classify_ast(statement, dialect, &mut found);
    }
    let plain = || tokens.iter().map(|token| &token.token);
    leading_keyword_floor(plain(), &mut found);
    if contains_locking_clause(plain()) {
        found.add(RiskGrade::Write, RiskReason::LockingRead);
    }
    // MySQL 의 `SELECT @a := 1` 은 세션 변수를 바꾼다. PG 의 `:=` 는 함수의 이름 붙은 인자라 MySQL 에서만 본다.
    if dialect == SqlDialect::Mysql && plain().any(|token| matches!(token, Token::Assignment)) {
        found.add(RiskGrade::Write, RiskReason::SessionState);
    }
    Outcome {
        classification: found.finish(),
        statements: Some(statements.len()),
    }
}

/// 연산자 토큰 수([`MAX_OPERATOR_TOKENS`]). 빈칸 · 쉼표 · 괄호 · 마침표 · `;` · 값(수 · 문자열 · 자리표) · 이름 · 값 키워드는 세지
/// 않고, 쉼표 · 여는 괄호 뒤의 부호(`VALUES (-1, +2)`)도 세지 않는다. 나머지 토큰(연산자 기호 · 키워드)은 모두 센다 — 세지 않는
/// 쪽을 목록으로 두어, 모르는 연산자가 사슬로 이어져도 상한에 걸리게 한다.
fn operator_tokens(tokens: &[TokenWithSpan]) -> usize {
    let mut count = 0;
    let mut previous: Option<&Token> = None;
    for token in tokens.iter().map(|token| &token.token) {
        let counted = match token {
            Token::Whitespace(_) | Token::EOF => continue,
            Token::Comma
            | Token::LParen
            | Token::RParen
            | Token::Period
            | Token::SemiColon
            | Token::Number(..)
            | Token::SingleQuotedString(_)
            | Token::DoubleQuotedString(_)
            | Token::DollarQuotedString(_)
            | Token::NationalStringLiteral(_)
            | Token::EscapedStringLiteral(_)
            | Token::UnicodeStringLiteral(_)
            | Token::HexStringLiteral(_)
            | Token::SingleQuotedByteStringLiteral(_)
            | Token::Placeholder(_) => false,
            Token::Word(word) => {
                word.quote_style.is_none()
                    && !matches!(
                        word.keyword,
                        Keyword::NoKeyword
                            | Keyword::NULL
                            | Keyword::TRUE
                            | Keyword::FALSE
                            | Keyword::DEFAULT
                    )
            }
            Token::Minus | Token::Plus => !matches!(previous, Some(Token::Comma | Token::LParen)),
            _ => true,
        };
        if counted {
            count += 1;
        }
        previous = Some(token);
    }
    count
}

fn parser_dialect(dialect: SqlDialect) -> Box<dyn Dialect> {
    match dialect {
        SqlDialect::Postgres => Box::new(PostgreSqlDialect {}),
        SqlDialect::Mysql => Box::new(MySqlDialect {}),
        SqlDialect::Sqlite => Box::new(SQLiteDialect {}),
    }
}

impl Classification {
    fn destructive(reason: RiskReason) -> Self {
        Self {
            grade: RiskGrade::Destructive,
            reasons: vec![reason],
        }
    }

    fn unparsed() -> Self {
        Self::destructive(RiskReason::Unparsed)
    }
}

/// 찾은 (등급, 까닭)을 모은다. 결과는 가장 높은 등급과 그 등급의 까닭이다.
#[derive(Default)]
struct Found(Vec<(RiskGrade, RiskReason)>);

impl Found {
    fn add(&mut self, grade: RiskGrade, reason: RiskReason) {
        self.0.push((grade, reason));
    }

    /// 다른 읽기의 분류를 더한다(조회는 까닭이 없어 더할 것이 없다).
    fn add_classification(&mut self, classification: &Classification) {
        for reason in &classification.reasons {
            self.add(classification.grade, *reason);
        }
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
            // PG 는 영국식 `ANALYSE` 도 같은 옵션으로 받는다.
            let analyze_option = options.as_ref().is_some_and(|options| {
                options.iter().any(|option| {
                    option.name.value.eq_ignore_ascii_case("analyze")
                        || option.name.value.eq_ignore_ascii_case("analyse")
                })
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
        // 다른 세션 · 서버 전체에 닿는다 — PG 의 `pg_terminate_backend` · `ALTER SYSTEM` 과 같은 일은 같은 등급(관리 함수 목록 주석).
        // `SET PERSIST` · `SHUTDOWN` · PG `ALTER SYSTEM` 은 파서가 읽지 못해 이미 파괴다.
        Statement::Kill { .. } | Statement::Flush { .. } => {
            found.add(UnboundedWrite, RiskReason::ServerAdmin)
        }
        Statement::Set(set) if set_is_global(set) => {
            found.add(UnboundedWrite, RiskReason::ServerAdmin)
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

/// MySQL `SET GLOBAL x = …` · `SET @@global.x = …` — 서버 전체의 변수를 바꾼다.
fn set_is_global(set: &Set) -> bool {
    let global_name = |name: &ObjectName| {
        name.0
            .first()
            .and_then(|part| part.as_ident())
            .is_some_and(|ident| ident.value.eq_ignore_ascii_case("@@global"))
    };
    let global_scope =
        |scope: &Option<ContextModifier>| matches!(scope, Some(ContextModifier::Global));
    match set {
        Set::SingleAssignment {
            scope, variable, ..
        } => global_scope(scope) || global_name(variable),
        Set::MultipleAssignments { assignments } => assignments
            .iter()
            .any(|assignment| global_scope(&assignment.scope) || global_name(&assignment.name)),
        Set::ParenthesizedAssignments { variables, .. } => variables.iter().any(global_name),
        _ => false,
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
    // MySQL `DELETE LOW_PRIORITY | QUICK FROM t …` 의 수식어를 파서가 지울 표 이름으로 읽는다 — 수식어는 표로 세지 않는다.
    let modifier = |name: &ObjectName| {
        matches!(name.0.as_slice(), [part] if part.as_ident().is_some_and(|ident| ident.quote_style.is_none()
            && (ident.value.eq_ignore_ascii_case("LOW_PRIORITY") || ident.value.eq_ignore_ascii_case("QUICK"))))
    };
    if delete.tables.iter().any(|name| !modifier(name))
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

/// FROM 서브쿼리 · 파생 표 안의 `WITH` 는 보지 않는다 — PG 는 데이터를 바꾸는 `WITH` 를 맨 위에만 받고(그 밖은 거절), MySQL ·
/// SQLite 에는 DML 을 담는 `WITH` 가 없다. 그 안의 함수 · 잠금은 [`classify_functions`] · 잠금절 토큰 검사가 본다.
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
            found.add(RiskGrade::UnboundedWrite, RiskReason::ServerAdmin)
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

/// 다른 세션 · 서버 전체에 닿는 관리 함수 — 범위 없는 쓰기(결정 M3-D20 에서 어느 연결이든 확인을 받는다). 이름은 스키마를 떼고
/// 마지막 이름 · 대소문자 무시로 견준다.
///
/// 근거: PG 는 `BEGIN READ ONLY` 안에서도 이것들을 막지 않는다 — 읽기 전용 검사(`PreventCommandIfReadOnly` · `XactReadOnly`)를 하지
/// 않는 함수들이다(PG 17 소스 판단, M3-03 리뷰 목록). DB 수준 읽기 전용이 받쳐 주지 못하므로 분류가 첫 방어다(결정 M3-D6 의 전제가
/// 깨지는 내장 함수). live 확인: M3-06 이 PG 17.6 에서 `pg_terminate_backend` 를 확인했고, 나머지 목록의 live 표는 M3-06 이 확정
/// 중이다(확정 전) — 표가 오면 여기를 맞춘다.
const SERVER_ADMIN_FUNCTIONS: &[&str] = &[
    // 다른 세션을 끊는다
    "pg_terminate_backend",
    "pg_cancel_backend",
    // Kingbase(DBX 목록에서 그대로)
    "sys_cancel_backend",
    // 서버 설정 · 로그 · 통계
    "pg_reload_conf",
    "pg_rotate_logfile",
    "pg_log_backend_memory_contexts",
    // 복제 · WAL · 백업 · 승격
    "pg_drop_replication_slot",
    "pg_create_physical_replication_slot",
    "pg_create_logical_replication_slot",
    "pg_copy_physical_replication_slot",
    "pg_copy_logical_replication_slot",
    "pg_replication_slot_advance",
    "pg_logical_emit_message",
    "pg_promote",
    "pg_switch_wal",
    "pg_create_restore_point",
    "pg_backup_start",
    "pg_backup_stop",
    "pg_wal_replay_pause",
    "pg_wal_replay_resume",
    // 다른 세션의 `LISTEN` 에 알린다 — 같은 일의 `NOTIFY` 문장은 쓰기지만 함수 꼴은 조회로 새고 있었다
    "pg_notify",
    // adminpack: 서버 파일을 쓴다
    "pg_file_write",
    "pg_file_rename",
    "pg_file_unlink",
    "pg_file_sync",
];

/// 이 접두사로 시작하는 이름도 관리 함수다: `pg_replication_origin_*`(복제 원점 만들기 · 지우기 · 진행), `pg_stat_reset*`(통계 지우기).
const SERVER_ADMIN_FUNCTION_PREFIXES: &[&str] = &["pg_replication_origin_", "pg_stat_reset"];

/// 조회를 쓰기로 만드는 부작용 함수(이름 견주기는 [`SERVER_ADMIN_FUNCTIONS`] 와 같다). DBX 의 `SIDE_EFFECT_SELECT_FUNCTIONS`
/// (관리 함수는 [`SERVER_ADMIN_FUNCTIONS`] 로 옮겼다)에 14-m3-plan.md M3-03 행 ④ 의 것과 SQLite `load_extension`(네이티브 코드를
/// 올린다) · MySQL `release_all_locks` · PG `pg_read_binary_file` · `pg_stat_file`(서버 파일 읽기) · `txid_current` ·
/// `pg_current_xact_id`(트랜잭션 번호를 할당한다)를 더했다.
const SIDE_EFFECT_FUNCTIONS: &[&str] = &[
    // PG
    "nextval",
    "setval",
    "set_config",
    "pg_read_file",
    "pg_read_binary_file",
    "pg_stat_file",
    "txid_current",
    "pg_current_xact_id",
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
/// `pg_try_advisory_*`, `dblink` · `dblink_exec` 류, `lo_import` · `lo_export` · `lo_create` · `lo_unlink` 류, `pg_ls_dir` 류(서버
/// 디렉터리 목록).
const SIDE_EFFECT_FUNCTION_PREFIXES: &[&str] = &[
    "pg_sleep",
    "pg_advisory",
    "pg_try_advisory",
    "dblink",
    "lo_",
    "pg_ls_",
];

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum FunctionEffect {
    SideEffect,
    ServerAdmin,
}

fn function_effect(name: &ObjectName) -> Option<FunctionEffect> {
    let ident = name.0.last().and_then(|part| part.as_ident())?;
    let name = ident.value.to_ascii_lowercase();
    if SERVER_ADMIN_FUNCTIONS.contains(&name.as_str())
        || SERVER_ADMIN_FUNCTION_PREFIXES
            .iter()
            .any(|prefix| name.starts_with(prefix))
    {
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
fn contains_locking_clause<'a>(tokens: impl IntoIterator<Item = &'a Token>) -> bool {
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
fn leading_keyword_floor<'a>(tokens: impl IntoIterator<Item = &'a Token>, found: &mut Found) {
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
