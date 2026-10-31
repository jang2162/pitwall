//! 방언별 SQL 리터럴 · 식별자 따옴표와 결과 행의 `INSERT` 글(M3-16 — 그리드의 「INSERT 문으로 복사」, `db.sql.insertText`).
//! 리터럴의 정본은 이것 하나다 — 그리드 편집의 미리보기 글(M3-12)도 이 모듈을 쓴다.
//!
//! 이 글은 사람이 나중에 다른 도구 · 다른 연결에서 실행한다. 그래서 **서버 설정과 상관없이 같은 값으로 읽히는 꼴**만 낸다:
//!
//! - **PG**: 백슬래시 · 제어 문자가 든 문자열은 `E'…'`(백슬래시를 두 번, 제어 문자는 `\xhh` · `\uhhhh`) —
//!   `standard_conforming_strings` 가 꺼져 있어도 켜져 있어도 같다. 그 밖은 `'…'`(따옴표만 두 번 — 두 설정에서 같다).
//!   바이트는 `decode('<hex>', 'hex')`(`'\x…'` 는 설정이 꺼져 있으면 `\x` 가 문자열 이스케이프로 먼저 읽힌다). NUL 이 든
//!   문자열은 PG 의 text 가 담지 못하므로 거절한다.
//! - **MySQL · MariaDB**: 백슬래시 · 제어 문자(NUL 포함)가 든 문자열은 `_utf8mb4 X'<hex>'` — `NO_BACKSLASH_ESCAPES` 가
//!   있든 없든 같은 값이다. 그 밖은 `'…'`(따옴표만 두 번). 큰따옴표는 `ANSI_QUOTES` 에서 식별자라 쓰지 않는다. 식별자는
//!   백틱(두 설정에서 같다). 바이트는 `X'<hex>'`.
//! - **SQLite**: `'…'`(따옴표만 두 번 — SQLite 문자열에는 이스케이프가 없다). 제어 문자(NUL 포함)가 든 문자열은
//!   `CAST(X'<hex>' AS TEXT)`(UTF-8 데이터베이스 기준). 바이트는 `X'<hex>'`.
//!
//! 제어 문자(줄바꿈 포함)를 글자로 두지 않는 까닭: 클립보드 · 편집기가 `\r\n` 을 바꿔 값이 달라진다.
//!
//! 숫자: 정수는 10진 글, 실수는 왕복하는 가장 짧은 글(`1e300` 꼴 포함), decimal 은 서버가 준 글. 숫자 모양이 아닌 글(`NaN`
//! 등)은 문자열 리터럴로 싼다 — 값이 문장 밖으로 새지 않게. 실수의 `-0` 은 부호를 잃을 수 있다(PG · MySQL 이 숫자 리터럴을
//! numeric · DECIMAL 로 먼저 읽는다).
//!
//! 거절: 잘린 값(`DbValue::Truncated` · 잘린 바이트 — 원래 값이 아니다), 그 방언에 없는 값(MySQL · SQLite 의 배열, MySQL 의
//! NaN · 무한대, SQLite 의 NaN), PG 문자열 · 식별자의 NUL.

use pw_db_types::{DbBytes, DbFloat, DbInt, DbValue, ObjectRef, SqlDialect};
use pw_rpc::{Router, RpcError, code, rpc_method};
use serde::{Deserialize, Serialize};

/// 리터럴로 만들 수 없는 까닭.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum LiteralError {
    /// 셀 상한에서 잘린 값(원래 값이 아니다).
    Truncated,
    /// 그 방언에 없는 값(배열 · NaN · 무한대).
    Unsupported,
    /// NUL 이 든 글을 그 방언이 담지 못한다(PG).
    Nul,
}

impl LiteralError {
    fn describe(self) -> &'static str {
        match self {
            Self::Truncated => "the value was truncated at the cell limit",
            Self::Unsupported => "the dialect has no literal for this value",
            Self::Nul => "the dialect cannot hold a NUL character",
        }
    }
}

/// 값 하나의 리터럴.
pub fn literal(dialect: SqlDialect, value: &DbValue) -> Result<String, LiteralError> {
    Ok(match value {
        DbValue::Null => "NULL".to_owned(),
        DbValue::Bool(b) => match (dialect, *b) {
            (SqlDialect::Sqlite, true) => "1".to_owned(),
            (SqlDialect::Sqlite, false) => "0".to_owned(),
            (_, true) => "TRUE".to_owned(),
            (_, false) => "FALSE".to_owned(),
        },
        DbValue::Int(DbInt::Number(n)) => n.to_string(),
        DbValue::Int(DbInt::Text(t)) => numeric_or_string(dialect, t)?,
        DbValue::Float(DbFloat::Number(f)) => float(dialect, *f)?,
        DbValue::Float(DbFloat::Text(t)) => non_finite(dialect, t)?,
        DbValue::Decimal(t) => numeric_or_string(dialect, t)?,
        DbValue::Text(t) | DbValue::Json(t) | DbValue::Datetime(t) | DbValue::Uuid(t) => {
            string(dialect, t)?
        }
        DbValue::Other(o) => string(dialect, &o.text)?,
        DbValue::Bytes(b) => bytes(dialect, b)?,
        DbValue::Array(items) => match dialect {
            SqlDialect::Postgres => string(dialect, &pg_array(items)?)?,
            SqlDialect::Mysql | SqlDialect::Sqlite => return Err(LiteralError::Unsupported),
        },
        DbValue::Truncated(_) => return Err(LiteralError::Truncated),
    })
}

/// 식별자 하나를 따옴표로 싼다 — 안의 따옴표 문자는 두 번 쓴다. PG · SQLite 는 `"`, MySQL 은 백틱.
pub fn quote_ident(dialect: SqlDialect, name: &str) -> Result<String, LiteralError> {
    if name.contains('\0') {
        return Err(LiteralError::Nul);
    }
    let q = match dialect {
        SqlDialect::Postgres | SqlDialect::Sqlite => '"',
        SqlDialect::Mysql => '`',
    };
    let mut out = String::with_capacity(name.len() + 2);
    out.push(q);
    for c in name.chars() {
        if c == q {
            out.push(q);
        }
        out.push(c);
    }
    out.push(q);
    Ok(out)
}

/// 테이블을 모를 때 쓰는 자리표시 이름(DataGrip 과 같다). 사람이 고쳐 쓴다.
pub const PLACEHOLDER_TABLE: &str = "MY_TABLE";

/// 행마다 `INSERT INTO <테이블> (<열>…) VALUES (<값>…);` 한 줄. 테이블이 없으면 [`PLACEHOLDER_TABLE`].
///
/// 실패하면 그 셀(행 · 열 번호, 0부터)과 까닭.
pub fn insert_text(
    dialect: SqlDialect,
    table: Option<&ObjectRef>,
    columns: &[String],
    rows: &[Vec<DbValue>],
) -> Result<String, InsertTextError> {
    let header_error = |reason| InsertTextError {
        row: None,
        column: None,
        reason,
    };
    let table = match table {
        Some(t) => {
            let mut parts = Vec::with_capacity(3);
            for part in [
                t.database.as_deref(),
                t.schema.as_deref(),
                Some(t.name.as_str()),
            ]
            .into_iter()
            .flatten()
            {
                parts.push(quote_ident(dialect, part).map_err(header_error)?);
            }
            parts.join(".")
        }
        None => PLACEHOLDER_TABLE.to_owned(),
    };
    let mut names = Vec::with_capacity(columns.len());
    for (i, c) in columns.iter().enumerate() {
        names.push(quote_ident(dialect, c).map_err(|reason| InsertTextError {
            row: None,
            column: Some(i as u32),
            reason,
        })?);
    }
    let head = format!("INSERT INTO {table} ({}) VALUES (", names.join(", "));
    let mut out = String::new();
    for (r, row) in rows.iter().enumerate() {
        if row.len() != columns.len() {
            return Err(InsertTextError {
                row: Some(r as u32),
                column: None,
                reason: LiteralError::Unsupported,
            });
        }
        out.push_str(&head);
        for (c, value) in row.iter().enumerate() {
            if c > 0 {
                out.push_str(", ");
            }
            out.push_str(&literal(dialect, value).map_err(|reason| InsertTextError {
                row: Some(r as u32),
                column: Some(c as u32),
                reason,
            })?);
        }
        out.push_str(");\n");
    }
    Ok(out)
}

/// [`insert_text`] 의 실패: 어느 셀(머리면 행이 없다)과 까닭.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct InsertTextError {
    pub row: Option<u32>,
    pub column: Option<u32>,
    pub reason: LiteralError,
}

fn string(dialect: SqlDialect, text: &str) -> Result<String, LiteralError> {
    match dialect {
        SqlDialect::Postgres => {
            if text.contains('\0') {
                return Err(LiteralError::Nul);
            }
            if text.chars().any(|c| c == '\\' || c.is_control()) {
                let mut out = String::with_capacity(text.len() + 3);
                out.push_str("E'");
                for c in text.chars() {
                    match c {
                        '\\' => out.push_str("\\\\"),
                        '\'' => out.push_str("''"),
                        c if c.is_control() && (c as u32) < 0x80 => {
                            out.push_str(&format!("\\x{:02x}", c as u32));
                        }
                        c if c.is_control() => out.push_str(&format!("\\u{:04x}", c as u32)),
                        c => out.push(c),
                    }
                }
                out.push('\'');
                Ok(out)
            } else {
                Ok(quoted(text))
            }
        }
        SqlDialect::Mysql => {
            if text.chars().any(|c| c == '\\' || c.is_control()) {
                Ok(format!("_utf8mb4 X'{}'", hex(text.as_bytes())))
            } else {
                Ok(quoted(text))
            }
        }
        SqlDialect::Sqlite => {
            if text.chars().any(char::is_control) {
                Ok(format!("CAST(X'{}' AS TEXT)", hex(text.as_bytes())))
            } else {
                Ok(quoted(text))
            }
        }
    }
}

/// `'…'` — 안의 `'` 를 두 번 쓴다.
fn quoted(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(DIGITS[(b >> 4) as usize] as char);
        out.push(DIGITS[(b & 0xf) as usize] as char);
    }
    out
}

fn bytes(dialect: SqlDialect, b: &DbBytes) -> Result<String, LiteralError> {
    if b.truncated {
        return Err(LiteralError::Truncated);
    }
    let digits = b.hex.strip_prefix("0x").unwrap_or(&b.hex);
    // 와이어 값을 그대로 믿지 않는다 — hex 가 아니면 문장 밖으로 샐 수 있다.
    if !digits.len().is_multiple_of(2) || !digits.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err(LiteralError::Unsupported);
    }
    let digits = digits.to_ascii_lowercase();
    Ok(match dialect {
        SqlDialect::Postgres => format!("decode('{digits}', 'hex')"),
        SqlDialect::Mysql | SqlDialect::Sqlite => format!("X'{digits}'"),
    })
}

/// 10진 숫자 모양(`-12`, `3.50`, `1e-7`)이면 그대로, 아니면 문자열 리터럴.
fn numeric_or_string(dialect: SqlDialect, text: &str) -> Result<String, LiteralError> {
    if is_number(text) {
        Ok(text.to_owned())
    } else {
        string(dialect, text)
    }
}

fn is_number(text: &str) -> bool {
    let s = text.strip_prefix(['-', '+']).unwrap_or(text);
    let (mantissa, exponent) = match s.find(['e', 'E']) {
        Some(i) => (&s[..i], Some(&s[i + 1..])),
        None => (s, None),
    };
    let (int, frac) = match mantissa.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (mantissa, None),
    };
    let digits = |p: &str| p.bytes().all(|c| c.is_ascii_digit());
    let int_ok = digits(int);
    let frac_ok = frac.is_none_or(digits);
    let some_digit = !int.is_empty() || frac.is_some_and(|f| !f.is_empty());
    let exp_ok = exponent.is_none_or(|e| {
        let e = e.strip_prefix(['-', '+']).unwrap_or(e);
        !e.is_empty() && digits(e)
    });
    int_ok && frac_ok && some_digit && exp_ok
}

fn float(dialect: SqlDialect, f: f64) -> Result<String, LiteralError> {
    if f.is_finite() {
        // `{:?}` 는 왕복하는 가장 짧은 글이고 큰 수 · 작은 수는 지수 꼴이다(`1e300`).
        Ok(format!("{f:?}"))
    } else {
        non_finite(
            dialect,
            if f.is_nan() {
                "NaN"
            } else if f > 0.0 {
                "Infinity"
            } else {
                "-Infinity"
            },
        )
    }
}

fn non_finite(dialect: SqlDialect, text: &str) -> Result<String, LiteralError> {
    match (dialect, text) {
        (SqlDialect::Postgres, "NaN" | "Infinity" | "-Infinity") => Ok(quoted(text)),
        (SqlDialect::Sqlite, "Infinity") => Ok("9e999".to_owned()),
        (SqlDialect::Sqlite, "-Infinity") => Ok("-9e999".to_owned()),
        _ => Err(LiteralError::Unsupported),
    }
}

/// PG 배열의 텍스트 입력 꼴(`{1,"a b",NULL,{…}}`). 원소는 늘 큰따옴표로 싸고 안의 `"` · `\` 를 백슬래시로 막는다.
fn pg_array(items: &[DbValue]) -> Result<String, LiteralError> {
    let mut out = String::from("{");
    for (i, item) in items.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        match item {
            DbValue::Null => out.push_str("NULL"),
            DbValue::Array(inner) => out.push_str(&pg_array(inner)?),
            other => {
                let text = pg_element(other)?;
                out.push('"');
                for c in text.chars() {
                    if c == '"' || c == '\\' {
                        out.push('\\');
                    }
                    out.push(c);
                }
                out.push('"');
            }
        }
    }
    out.push('}');
    Ok(out)
}

/// 배열 원소의 PG 텍스트 입력.
fn pg_element(value: &DbValue) -> Result<String, LiteralError> {
    Ok(match value {
        DbValue::Bool(b) => if *b { "t" } else { "f" }.to_owned(),
        DbValue::Int(DbInt::Number(n)) => n.to_string(),
        DbValue::Int(DbInt::Text(t)) => t.clone(),
        DbValue::Float(DbFloat::Number(f)) if f.is_finite() => format!("{f:?}"),
        DbValue::Float(DbFloat::Number(f)) => if f.is_nan() {
            "NaN"
        } else if *f > 0.0 {
            "Infinity"
        } else {
            "-Infinity"
        }
        .to_owned(),
        DbValue::Float(DbFloat::Text(t)) => t.clone(),
        DbValue::Decimal(t)
        | DbValue::Text(t)
        | DbValue::Json(t)
        | DbValue::Datetime(t)
        | DbValue::Uuid(t) => {
            if t.contains('\0') {
                return Err(LiteralError::Nul);
            }
            t.clone()
        }
        DbValue::Other(o) => {
            if o.text.contains('\0') {
                return Err(LiteralError::Nul);
            }
            o.text.clone()
        }
        DbValue::Bytes(b) => {
            if b.truncated {
                return Err(LiteralError::Truncated);
            }
            let digits = b.hex.strip_prefix("0x").unwrap_or(&b.hex);
            if !digits.len().is_multiple_of(2) || !digits.bytes().all(|c| c.is_ascii_hexdigit()) {
                return Err(LiteralError::Unsupported);
            }
            format!("\\x{}", digits.to_ascii_lowercase())
        }
        DbValue::Truncated(_) => return Err(LiteralError::Truncated),
        DbValue::Null | DbValue::Array(_) => unreachable!("handled by pg_array"),
    })
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SqlInsertTextParams {
    pub dialect: SqlDialect,
    /// 테이블. 모르면(콘솔 결과) `null` — 자리표시 이름 [`PLACEHOLDER_TABLE`].
    pub table: Option<ObjectRef>,
    pub columns: Vec<String>,
    pub rows: Vec<Vec<DbValue>>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct SqlInsertTextResult {
    pub text: String,
}

rpc_method! {
    /// 결과 행을 그 방언의 `INSERT` 글로(행마다 한 문장, 리터럴 규칙은 `pw_db_sql::literal`). 연결을 열지 않는 계산이다.
    /// 잘린 값 · 방언에 없는 값이 있으면 `INVALID_PARAMS` 이고 `data` 가 그 셀(`InsertTextError`)이다. 셀이
    /// [`MAX_INSERT_CELLS`] 를 넘어도 `INVALID_PARAMS`.
    pub DbSqlInsertText: "db.sql.insertText" {
        params: SqlInsertTextParams,
        result: SqlInsertTextResult,
        tier: Read,
    }
}

// 실패의 `data`(`InsertTextError`)는 메서드의 출력이 아니라 오류에 실리므로 창이 쓸 타입으로 따로 등록한다.
pw_rpc::rpc_type!(InsertTextError, LiteralError);

/// 한 번에 받는 셀 수의 상한(행 × 열). 그리드 보유 행 상한(10만) × 열 20 이다 — 넘으면 창이 나눠 부른다.
pub const MAX_INSERT_CELLS: usize = 2_000_000;

/// 메서드를 붙인다. 상태 없는 계산이다.
pub fn register(router: &mut Router) {
    router.unary(DbSqlInsertText, |_ctx, p: SqlInsertTextParams| async move {
        tokio::task::spawn_blocking(move || {
            let cells = p.rows.len().saturating_mul(p.columns.len().max(1));
            if cells > MAX_INSERT_CELLS {
                return Err(RpcError::invalid_params(format!(
                    "{cells} cells are over the {MAX_INSERT_CELLS} cell limit"
                )));
            }
            insert_text(p.dialect, p.table.as_ref(), &p.columns, &p.rows)
                .map(|text| SqlInsertTextResult { text })
                .map_err(|e| {
                    RpcError::new(code::INVALID_PARAMS, e.reason.describe())
                        .with_data(serde_json::to_value(e).unwrap_or(serde_json::Value::Null))
                })
        })
        .await
        .map_err(|e| RpcError::internal(e.to_string()))?
    });
}

#[cfg(any(test, feature = "test-support"))]
pub mod testing;

#[cfg(test)]
mod tests;
