//! 값 변환(결정 M3-D8): 서버가 보낸 바이너리 열 값 → [`DbValue`], 바인드 값 [`DbValue`] → 텍스트 형식 매개변수.
//!
//! `tokio-postgres` 는 결과를 늘 바이너리 형식으로 받는다(0.7.18 의 Bind 는 결과 형식을 바이너리로 고정하고 바꿀 공개 API 가
//! 없다). 바이너리로 정확히 풀리는 타입(정수 · 실수 · numeric · bool · 문자열 · bytea · json/jsonb · uuid · 배열 · enum)은 여기서
//! 풀고, 나머지(`timestamptz` 를 비롯한 날짜 · 시각 · `interval` · `money` · 모르는 타입)는 **서버의 텍스트 출력 함수**를
//! 거친 글을 쓴다: 받은 바이너리를 그 타입의 매개변수로 같은 세션에 되돌려 보내 `$n::text` 로 받는다([`text_outputs`]). 그래서
//! `timestamptz` 는 세션 `TimeZone` · `DateStyle` 그대로의 글이다 — 바이너리 디코더처럼 호스트 시간대를 적용하지 않는다.
//!
//! 되돌려 받기는 사람의 문장이 아니다: 그 문장의 `fetch` 안에서(같은 취소 · 예산) 세이브포인트로 감싸 돌고, 실패해도 트랜잭션을
//! 깨지 않고 그 값만 「표시 불가」 로 둔다([`unavailable`]). 취소 · 끊김은 그대로 `fetch` 의 오류다.

use std::collections::BTreeMap;
use std::error::Error;

use bytes::BytesMut;
use pw_db_types::{CellLimit, DbError, DbInt, DbValue};
use tokio_postgres::Client;
use tokio_postgres::types::{FromSql, IsNull, Kind, ToSql, Type, to_sql_checked};

use crate::error::{db_error, invalid};

type BoxError = Box<dyn Error + Sync + Send>;

/// 열 값의 바이트 그대로(`NULL` 은 `None`). 어떤 타입이든 받는다.
pub(crate) struct Raw<'a>(pub Option<&'a [u8]>);

impl<'a> FromSql<'a> for Raw<'a> {
    fn from_sql(_: &Type, raw: &'a [u8]) -> Result<Self, BoxError> {
        Ok(Self(Some(raw)))
    }

    fn from_sql_null(_: &Type) -> Result<Self, BoxError> {
        Ok(Self(None))
    }

    fn accepts(_: &Type) -> bool {
        true
    }
}

/// 받은 바이너리를 그 타입 그대로 서버에 되돌려 보내는 매개변수.
#[derive(Debug)]
struct RawParam<'a>(&'a [u8]);

impl ToSql for RawParam<'_> {
    fn to_sql(&self, _: &Type, out: &mut BytesMut) -> Result<IsNull, BoxError> {
        out.extend_from_slice(self.0);
        Ok(IsNull::No)
    }

    fn accepts(_: &Type) -> bool {
        true
    }

    to_sql_checked!();
}

/// 텍스트 형식 바인드 값 — 서버가 그 타입의 입력 함수로 읽는다(열 타입이 무엇이든 같은 길).
#[derive(Debug)]
pub(crate) struct TextParam(Option<String>);

impl ToSql for TextParam {
    fn to_sql(&self, _: &Type, out: &mut BytesMut) -> Result<IsNull, BoxError> {
        match &self.0 {
            None => Ok(IsNull::Yes),
            Some(text) => {
                out.extend_from_slice(text.as_bytes());
                Ok(IsNull::No)
            }
        }
    }

    fn accepts(_: &Type) -> bool {
        true
    }

    fn encode_format(&self, _: &Type) -> tokio_postgres::types::Format {
        tokio_postgres::types::Format::Text
    }

    to_sql_checked!();
}

/// 바인드 값 하나를 PG 텍스트 입력으로. 잘린 값은 원래 값이 아니므로 거절한다.
pub(crate) fn param(value: &DbValue) -> Result<TextParam, DbError> {
    Ok(TextParam(match value {
        DbValue::Null => None,
        other => Some(literal(other)?),
    }))
}

fn literal(value: &DbValue) -> Result<String, DbError> {
    Ok(match value {
        DbValue::Null => "NULL".to_owned(),
        DbValue::Bool(b) => if *b { "t" } else { "f" }.to_owned(),
        DbValue::Int(DbInt::Number(n)) => n.to_string(),
        DbValue::Int(DbInt::Text(t)) => t.clone(),
        DbValue::Float(pw_db_types::DbFloat::Number(f)) => f.to_string(),
        DbValue::Float(pw_db_types::DbFloat::Text(t)) => t.clone(),
        DbValue::Decimal(t)
        | DbValue::Text(t)
        | DbValue::Json(t)
        | DbValue::Datetime(t)
        | DbValue::Uuid(t) => t.clone(),
        DbValue::Other(o) => o.text.clone(),
        DbValue::Bytes(b) if !b.truncated => format!("\\x{}", b.hex.trim_start_matches("0x")),
        DbValue::Bytes(_) | DbValue::Truncated(_) => {
            return Err(invalid("a truncated value cannot be bound"));
        }
        DbValue::Array(items) => {
            let mut out = String::from("{");
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                match item {
                    DbValue::Null => out.push_str("NULL"),
                    DbValue::Array(_) => out.push_str(&literal(item)?),
                    other => {
                        out.push('"');
                        for c in literal(other)?.chars() {
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
            out
        }
    })
}

/// 푼 값 하나. 서버의 텍스트 출력이 필요한 값은 요청 목록의 번호다.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Cell {
    Value(DbValue),
    Array(Vec<Cell>),
    Text(usize),
}

/// 서버의 텍스트 출력이 필요한 값들(타입과 바이너리).
#[derive(Debug, Default)]
pub(crate) struct TextRequests {
    items: Vec<(Type, Vec<u8>)>,
}

impl TextRequests {
    pub(crate) fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    fn push(&mut self, ty: &Type, raw: &[u8]) -> Cell {
        self.items.push((ty.clone(), raw.to_vec()));
        Cell::Text(self.items.len() - 1)
    }
}

/// 열 값 하나를 푼다.
pub(crate) fn decode(
    ty: &Type,
    raw: Option<&[u8]>,
    limit: CellLimit,
    texts: &mut TextRequests,
) -> Cell {
    let Some(raw) = raw else {
        return Cell::Value(DbValue::Null);
    };
    match local(ty, raw, limit, texts) {
        Some(cell) => cell,
        None => texts.push(ty, raw),
    }
}

/// 바이너리로 정확히 풀리는 타입. `None` 이면 서버의 텍스트 출력을 쓴다(모르는 타입 · 깨진 바이트도).
fn local(ty: &Type, raw: &[u8], limit: CellLimit, texts: &mut TextRequests) -> Option<Cell> {
    let value = match *ty {
        Type::BOOL => DbValue::Bool(*raw.first()? != 0),
        Type::INT2 => DbValue::int(i16::from_be_bytes(raw.try_into().ok()?)),
        Type::INT4 => DbValue::int(i32::from_be_bytes(raw.try_into().ok()?)),
        Type::INT8 => DbValue::int(i64::from_be_bytes(raw.try_into().ok()?)),
        Type::OID | Type::XID | Type::CID => DbValue::int(u32::from_be_bytes(raw.try_into().ok()?)),
        // float4 는 그 자리수의 가장 짧은 글을 거쳐 f64 로 — `0.1::float4` 가 0.10000000149… 가 되지 않게(서버의 텍스트와 같다).
        Type::FLOAT4 => {
            let f = f32::from_be_bytes(raw.try_into().ok()?);
            DbValue::float(if f.is_finite() {
                f.to_string().parse().ok()?
            } else {
                f64::from(f)
            })
        }
        Type::FLOAT8 => DbValue::float(f64::from_be_bytes(raw.try_into().ok()?)),
        Type::NUMERIC => DbValue::decimal(numeric(raw)?),
        Type::TEXT
        | Type::VARCHAR
        | Type::BPCHAR
        | Type::NAME
        | Type::CHAR
        | Type::UNKNOWN
        | Type::XML => DbValue::text(std::str::from_utf8(raw).ok()?, limit),
        Type::BYTEA => DbValue::bytes(raw, limit),
        Type::JSON => DbValue::json(std::str::from_utf8(raw).ok()?, limit),
        // jsonb 바이너리는 판 바이트(1) 뒤에 글이다.
        Type::JSONB => match raw.split_first()? {
            (1, text) => DbValue::json(std::str::from_utf8(text).ok()?, limit),
            _ => return None,
        },
        Type::UUID => DbValue::Uuid(uuid(raw)?),
        _ => match ty.kind() {
            Kind::Array(element) => return array(element, raw, limit, texts),
            Kind::Enum(_) => DbValue::text(std::str::from_utf8(raw).ok()?, limit),
            Kind::Domain(base) => return local(base, raw, limit, texts),
            _ => return None,
        },
    };
    Some(Cell::Value(value))
}

/// numeric 바이너리(자리수 개수 · 가중치 · 부호 · 표시 소수 자리 · 10000 진 자리들) → 서버의 텍스트와 같은 글.
pub(crate) fn numeric(raw: &[u8]) -> Option<String> {
    let mut r = Reader(raw);
    let ndigits = usize::try_from(r.i16()?).ok()?;
    let weight = i32::from(r.i16()?);
    let sign = r.u16()?;
    let dscale = usize::from(r.u16()?);
    let digits = (0..ndigits).map(|_| r.i16()).collect::<Option<Vec<_>>>()?;
    match sign {
        0x0000 | 0x4000 => {}
        0xC000 => return Some("NaN".to_owned()),
        0xD000 => return Some("Infinity".to_owned()),
        0xF000 => return Some("-Infinity".to_owned()),
        _ => return None,
    }
    if digits.iter().any(|d| !(0..10_000).contains(d)) {
        return None;
    }
    let group = |i: i32| -> i16 {
        usize::try_from(i)
            .ok()
            .and_then(|i| digits.get(i))
            .copied()
            .unwrap_or(0)
    };
    let mut out = String::new();
    if sign == 0x4000 {
        out.push('-');
    }
    if weight < 0 {
        out.push('0');
    } else {
        for i in 0..=weight {
            if i == 0 {
                out.push_str(&group(i).to_string());
            } else {
                out.push_str(&format!("{:04}", group(i)));
            }
        }
    }
    if dscale > 0 {
        out.push('.');
        let mut fraction = String::new();
        let mut i = weight + 1;
        while fraction.len() < dscale {
            fraction.push_str(&format!("{:04}", group(i)));
            i += 1;
        }
        fraction.truncate(dscale);
        out.push_str(&fraction);
    }
    Some(out)
}

fn uuid(raw: &[u8]) -> Option<String> {
    let bytes: [u8; 16] = raw.try_into().ok()?;
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    Some(format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    ))
}

/// 배열 바이너리(차원 수 · NULL 표지 · 원소 OID · 차원마다 길이와 하한 · 원소들) → 차원만큼 겹친 배열.
fn array(element: &Type, raw: &[u8], limit: CellLimit, texts: &mut TextRequests) -> Option<Cell> {
    let mut r = Reader(raw);
    let ndim = usize::try_from(r.i32()?).ok()?;
    let _has_null = r.i32()?;
    let _element_oid = r.i32()?;
    if ndim == 0 {
        return Some(Cell::Array(Vec::new()));
    }
    let mut dims = Vec::with_capacity(ndim.min(6));
    for _ in 0..ndim {
        dims.push(usize::try_from(r.i32()?).ok()?);
        let _lower_bound = r.i32()?;
    }
    let total = dims.iter().try_fold(1_usize, |n, d| n.checked_mul(*d))?;
    // 원소마다 길이 4바이트는 있다 — 거짓 차원으로 큰 메모리를 잡지 않게.
    if total > raw.len() / 4 {
        return None;
    }
    let mut cells = Vec::with_capacity(total);
    for _ in 0..total {
        let len = r.i32()?;
        let value = if len < 0 {
            None
        } else {
            Some(r.take(usize::try_from(len).ok()?)?)
        };
        cells.push(decode(element, value, limit, texts));
    }
    for &dim in dims.iter().skip(1).rev() {
        if dim == 0 {
            return Some(Cell::Array(Vec::new()));
        }
        cells = cells.chunks(dim).map(|c| Cell::Array(c.to_vec())).collect();
    }
    Some(Cell::Array(cells))
}

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        if self.0.len() < n {
            return None;
        }
        let (head, rest) = self.0.split_at(n);
        self.0 = rest;
        Some(head)
    }

    fn i16(&mut self) -> Option<i16> {
        Some(i16::from_be_bytes(self.take(2)?.try_into().ok()?))
    }

    fn u16(&mut self) -> Option<u16> {
        Some(u16::from_be_bytes(self.take(2)?.try_into().ok()?))
    }

    fn i32(&mut self) -> Option<i32> {
        Some(i32::from_be_bytes(self.take(4)?.try_into().ok()?))
    }
}

/// 되돌려 받는 질의 하나가 싣는 값의 수(매개변수 상한 65535 아래). 1,000 에서 10,000 으로 올리니 `timestamptz` 열 10만 행의
/// 전체 시간이 절반쯤으로 줄었다(release, 같은 기기 — 보고서).
const TEXT_BATCH: usize = 10_000;

/// 세이브포인트 이름. 사람이 같은 이름을 써도 겹쳐 쌓일 뿐이다(`RELEASE` 는 가장 최근 것을 푼다).
const TEXT_SAVEPOINT: &str = "pitwall_text_output";

/// 서버의 텍스트 출력. 순서는 요청 순서이고, 실패한 값은 `Err(이유)` 다. 트랜잭션 안에서만 부른다(포털이 열려 있다).
///
/// 취소 · 끊김은 `Err` 로 돌려준다 — 문장 전체의 오류다. 그 밖의 실패는 세이브포인트까지 되돌려 트랜잭션을 그대로 두고 그 타입의
/// 값만 실패로 남긴다.
pub(crate) async fn text_outputs(
    client: &Client,
    requests: &TextRequests,
    fault: bool,
) -> Result<Vec<Result<String, String>>, DbError> {
    let mut out: Vec<Result<String, String>> = vec![Err(String::new()); requests.items.len()];
    let mut by_type: BTreeMap<u32, Vec<usize>> = BTreeMap::new();
    for (i, (ty, _)) in requests.items.iter().enumerate() {
        by_type.entry(ty.oid()).or_default().push(i);
    }
    for indices in by_type.values() {
        let ty = &requests.items[indices[0]].0;
        let savepoint = format!("SAVEPOINT {TEXT_SAVEPOINT}");
        let (saved, texts) = tokio::join!(client.batch_execute(&savepoint), async {
            let mut texts = Vec::with_capacity(indices.len());
            for chunk in indices.chunks(TEXT_BATCH) {
                let rows: Vec<String> = (1..=chunk.len())
                    .map(|n| format!("(${n}::pg_catalog.text)"))
                    .collect();
                let sql = if fault {
                    // 시험이 실패 경로를 부른다(`PgConnection::fail_text_output`) — 서버 오류(22012)로 끝난다.
                    format!(
                        "SELECT pg_catalog.length(t) / 0 FROM (VALUES {}) AS v(t)",
                        rows.join(",")
                    )
                } else {
                    format!("VALUES {}", rows.join(","))
                };
                let params: Vec<RawParam<'_>> = chunk
                    .iter()
                    .map(|&i| RawParam(&requests.items[i].1))
                    .collect();
                let typed: Vec<(&(dyn ToSql + Sync), Type)> = params
                    .iter()
                    .map(|p| (p as &(dyn ToSql + Sync), ty.clone()))
                    .collect();
                for row in client.query_typed(&sql, &typed).await? {
                    let Raw(raw) = row.try_get(0)?;
                    texts.push(
                        raw.map(|b| String::from_utf8_lossy(b).into_owned())
                            .unwrap_or_default(),
                    );
                }
            }
            Ok::<_, tokio_postgres::Error>(texts)
        });
        saved.map_err(|e| db_error(&e))?;
        match texts {
            Ok(texts) => {
                client
                    .batch_execute(&format!("RELEASE SAVEPOINT {TEXT_SAVEPOINT}"))
                    .await
                    .map_err(|e| db_error(&e))?;
                for (&i, text) in indices.iter().zip(texts) {
                    out[i] = Ok(text);
                }
            }
            Err(error) => {
                let error = db_error(&error);
                if matches!(
                    error.kind,
                    pw_db_types::DbErrorKind::Cancelled | pw_db_types::DbErrorKind::Disconnected
                ) {
                    return Err(error);
                }
                client
                    .batch_execute(&format!(
                        "ROLLBACK TO SAVEPOINT {TEXT_SAVEPOINT}; RELEASE SAVEPOINT {TEXT_SAVEPOINT}"
                    ))
                    .await
                    .map_err(|e| db_error(&e))?;
                for &i in indices {
                    out[i] = Err(error.message.clone());
                }
            }
        }
    }
    Ok(out)
}

/// 서버의 텍스트 출력을 그 타입의 값으로.
pub(crate) fn from_text(ty: &Type, text: String, limit: CellLimit) -> DbValue {
    match *ty {
        Type::DATE | Type::TIME | Type::TIMETZ | Type::TIMESTAMP | Type::TIMESTAMPTZ => {
            DbValue::Datetime(text)
        }
        Type::MONEY | Type::NUMERIC => DbValue::decimal(text),
        _ => DbValue::other(text, type_name(ty), limit),
    }
}

/// 텍스트 출력을 받지 못한 값 — 「표시 불가」. 질의 전체는 실패시키지 않는다.
pub(crate) fn unavailable(ty: &Type, reason: &str) -> DbValue {
    DbValue::other(
        format!("(cannot display: {reason})"),
        type_name(ty),
        CellLimit::DEFAULT,
    )
}

/// 결과 열의 타입 이름. 배열은 `원소[]`, `pg_catalog` · `public` 밖의 타입은 스키마를 붙인다.
pub(crate) fn type_name(ty: &Type) -> String {
    match ty.kind() {
        Kind::Array(element) => format!("{}[]", type_name(element)),
        _ if matches!(ty.schema(), "pg_catalog" | "public" | "") => ty.name().to_owned(),
        _ => format!("{}.{}", ty.schema(), ty.name()),
    }
}

/// 푼 값을 마무리한다 — 텍스트 출력 자리를 채운다.
pub(crate) fn finish(
    cell: Cell,
    texts: &TextRequests,
    outputs: &[Result<String, String>],
    limit: CellLimit,
) -> DbValue {
    match cell {
        Cell::Value(value) => value,
        Cell::Array(cells) => DbValue::Array(
            cells
                .into_iter()
                .map(|c| finish(c, texts, outputs, limit))
                .collect(),
        ),
        Cell::Text(i) => {
            let ty = &texts.items[i].0;
            match outputs.get(i) {
                Some(Ok(text)) => from_text(ty, text.clone(), limit),
                Some(Err(reason)) => unavailable(ty, reason),
                None => unavailable(ty, "no text output"),
            }
        }
    }
}

/// 행 묶음의 바이트 상한(`FetchLimit::max_bytes`)을 재는 대략 크기.
pub(crate) fn approximate_size(value: &DbValue) -> usize {
    match value {
        DbValue::Null | DbValue::Bool(_) => 1,
        DbValue::Int(_) | DbValue::Float(_) => 8,
        DbValue::Decimal(t)
        | DbValue::Text(t)
        | DbValue::Json(t)
        | DbValue::Datetime(t)
        | DbValue::Uuid(t) => t.len(),
        DbValue::Bytes(b) => b.hex.len(),
        DbValue::Array(items) => items.iter().map(approximate_size).sum::<usize>() + 2,
        DbValue::Truncated(t) => t.prefix.len(),
        DbValue::Other(o) => o.text.len(),
    }
}

#[cfg(test)]
mod tests;
