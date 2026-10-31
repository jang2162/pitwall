//! 콘솔 명령(`KvConnection::command`)의 상한(14-m3-plan.md M3-09 — 키 하나 안의 읽기는 허용하고 상한으로 끊는다).
//!
//! - **보내기 전 크기 묻기**([`size_probe`]): 키 하나를 통째로 읽는 알려진 명령(`GET` · `HGETALL` · `HKEYS` · `HVALS` ·
//!   `SMEMBERS` · `LRANGE` · `ZRANGE` 류 · `XRANGE` 류 · `JSON.GET`)은 같은 연결에서 길이(`STRLEN` · `HLEN` · `SCARD` · `LLEN` ·
//!   `ZCARD` · `XLEN` · `MEMORY USAGE`)를 먼저 묻고, 응답이 상한을 넘을 것이면 그 명령을 서버에 보내지 않는다(오류 코드
//!   [`KEY_TOO_LARGE`] — 키 브라우저의 쪽 읽기를 쓰라는 안내). `LIMIT` · `COUNT` 로 이미 상한 안에 묶인 것은 묻지 않는다.
//! - **응답 상한**([`limited_reply`]): 그 밖의 응답(여러 키에 걸친 `SUNION` 등)은 맨 위 원소 [`MAX_REPLY_ELEMENTS`] · 값 바이트
//!   [`PAGE_BUDGET`] 에서 끊고 `KvReply::Truncated` 로 감싼다.
//!
//! **한계**: redis-rs 는 응답 전체를 메모리로 받은 뒤에 넘겨주므로 응답 상한은 RPC 로 나가는 양 · 창 메모리만 묶는다. 묻기를
//! 지난 큰 응답(묻기와 읽기 사이에 키가 커진 경우, 목록 밖 명령)의 Host 일시 메모리는 응답 타임아웃 · 취소가 묶는다.

use pw_db_types::{DbError, DbErrorKind, KvReply, KvReplyTruncated};
use redis::Value;

use crate::reply::reply;
use crate::{MAX_REPLY_ELEMENTS, PAGE_BUDGET};

/// 크기를 먼저 물어 거절한 명령의 오류 코드.
pub(crate) const KEY_TOO_LARGE: &str = "KEY_TOO_LARGE";

pub(crate) fn too_large() -> DbError {
    DbError::new(
        DbErrorKind::Invalid,
        "the key is too large to read whole in the console — use the key browser (paged reads)",
    )
    .with_code(KEY_TOO_LARGE)
}

/// 보내기 전에 물을 길이 명령과 판정.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct SizeProbe {
    /// 길이 명령(`MEMORY` 면 `MEMORY USAGE`).
    pub command: &'static str,
    pub key: String,
    check: Check,
}

#[derive(Debug, PartialEq, Eq)]
enum Check {
    /// 길이가 바이트다(`STRLEN` · `MEMORY USAGE`) — [`PAGE_BUDGET`] 와 견준다.
    Bytes,
    /// 길이 × `factor` 가 응답의 맨 위 원소 수다(`HGETALL` 은 필드마다 둘).
    Elements { factor: u64 },
    /// 번호 범위(`LRANGE` · `ZRANGE`)가 고르는 수 × `factor`.
    Range { start: i64, stop: i64, factor: u64 },
}

impl SizeProbe {
    fn new(command: &'static str, key: &str, check: Check) -> Option<Self> {
        Some(Self {
            command,
            key: key.to_owned(),
            check,
        })
    }

    /// `size` 는 길이 명령의 답.
    pub(crate) fn too_large(&self, size: u64) -> bool {
        match self.check {
            Check::Bytes => size > PAGE_BUDGET as u64,
            Check::Elements { factor } => size.saturating_mul(factor) > MAX_REPLY_ELEMENTS,
            Check::Range {
                start,
                stop,
                factor,
            } => range_len(size, start, stop).saturating_mul(factor) > MAX_REPLY_ELEMENTS,
        }
    }
}

/// 번호 범위가 고르는 원소 수(음수 번호는 끝에서부터 — Redis 와 같다).
fn range_len(len: u64, start: i64, stop: i64) -> u64 {
    let len = i64::try_from(len).unwrap_or(i64::MAX);
    let resolve = |i: i64| if i < 0 { len.saturating_add(i) } else { i };
    let start = resolve(start).max(0);
    let stop = resolve(stop).min(len - 1);
    if start > stop {
        0
    } else {
        u64::try_from(stop - start + 1).unwrap_or(u64::MAX)
    }
}

/// 크기를 먼저 물어야 하는 명령이면 그 묻기. 인자를 읽지 못하면(형식이 틀림) 묻지 않는다 — 서버가 오류로 답한다.
pub(crate) fn size_probe(name: &str, args: &[String]) -> Option<SizeProbe> {
    let key = args.first()?;
    let upper = name.to_ascii_uppercase();
    let has = |word: &str| args.iter().any(|a| a.eq_ignore_ascii_case(word));
    let factor = if has("WITHSCORES") { 2 } else { 1 };
    let int = |i: usize| args.get(i).and_then(|a| a.parse::<i64>().ok());
    // `LIMIT <offset> <count>` · `COUNT <n>` 의 수. 음수(`-1` = 전부)면 묶이지 않은 것이다.
    let bounded_by = |word: &str, skip: usize| {
        args.iter()
            .position(|a| a.eq_ignore_ascii_case(word))
            .and_then(|i| args.get(i + skip))
            .and_then(|n| n.parse::<i64>().ok())
            .and_then(|n| u64::try_from(n).ok())
            .is_some_and(|n| n.saturating_mul(factor) <= MAX_REPLY_ELEMENTS)
    };
    match upper.as_str() {
        "GET" => SizeProbe::new("STRLEN", key, Check::Bytes),
        "JSON.GET" => SizeProbe::new("MEMORY", key, Check::Bytes),
        "HGETALL" => SizeProbe::new("HLEN", key, Check::Elements { factor: 2 }),
        "HKEYS" | "HVALS" => SizeProbe::new("HLEN", key, Check::Elements { factor: 1 }),
        "SMEMBERS" => SizeProbe::new("SCARD", key, Check::Elements { factor: 1 }),
        "LRANGE" => SizeProbe::new(
            "LLEN",
            key,
            Check::Range {
                start: int(1)?,
                stop: int(2)?,
                factor: 1,
            },
        ),
        "ZRANGE" if has("BYSCORE") || has("BYLEX") => (!bounded_by("LIMIT", 2))
            .then(|| SizeProbe::new("ZCARD", key, Check::Elements { factor }))?,
        "ZRANGE" | "ZREVRANGE" => SizeProbe::new(
            "ZCARD",
            key,
            Check::Range {
                start: int(1)?,
                stop: int(2)?,
                factor,
            },
        ),
        "ZRANGEBYSCORE" | "ZREVRANGEBYSCORE" | "ZRANGEBYLEX" | "ZREVRANGEBYLEX" => {
            (!bounded_by("LIMIT", 2))
                .then(|| SizeProbe::new("ZCARD", key, Check::Elements { factor }))?
        }
        "XRANGE" | "XREVRANGE" => (!bounded_by("COUNT", 1))
            .then(|| SizeProbe::new("XLEN", key, Check::Elements { factor: 1 }))?,
        _ => None,
    }
}

/// 응답을 상한에서 끊는다. 상한 안이면 그대로다.
pub(crate) fn limited_reply(value: Value) -> KvReply {
    let bytes = value_bytes(&value);
    let elements = match &value {
        Value::Array(items) | Value::Set(items) | Value::Push { data: items, .. } => {
            items.len() as u64
        }
        Value::Map(pairs) => pairs.len() as u64,
        _ => 1,
    };
    if elements <= MAX_REPLY_ELEMENTS && bytes <= PAGE_BUDGET as u64 {
        return reply(value);
    }
    let shown = match value {
        Value::Array(items) | Value::Set(items) | Value::Push { data: items, .. } => {
            Value::Array(keep(items, value_bytes))
        }
        Value::Map(pairs) => Value::Map(keep(pairs, |(k, v)| value_bytes(k) + value_bytes(v))),
        // 값 하나는 변환이 4 MiB 에서 자른다.
        other => other,
    };
    KvReply::Truncated(KvReplyTruncated {
        reply: Box::new(reply(shown)),
        elements,
        bytes,
    })
}

/// 앞에서부터 원소 수 · 바이트 상한 안에 드는 것만. 첫 원소는 크더라도 싣는다(값은 변환이 자른다).
fn keep<T>(items: Vec<T>, size: impl Fn(&T) -> u64) -> Vec<T> {
    let mut used = 0_u64;
    let mut kept = Vec::new();
    for item in items {
        let next = used.saturating_add(size(&item));
        let full = kept.len() as u64 >= MAX_REPLY_ELEMENTS
            || (!kept.is_empty() && next > PAGE_BUDGET as u64);
        if full {
            break;
        }
        used = next;
        kept.push(item);
    }
    kept
}

/// 값 바이트의 합(수 · 불은 8 바이트로 친다).
fn value_bytes(value: &Value) -> u64 {
    match value {
        Value::BulkString(b) => b.len() as u64,
        Value::SimpleString(s) => s.len() as u64,
        Value::VerbatimString { text, .. } => text.len() as u64,
        Value::Array(items) | Value::Set(items) | Value::Push { data: items, .. } => {
            items.iter().map(value_bytes).sum()
        }
        Value::Map(pairs) => pairs
            .iter()
            .map(|(k, v)| value_bytes(k) + value_bytes(v))
            .sum(),
        Value::Attribute { data, .. } => value_bytes(data),
        _ => 8,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(text: &str) -> Vec<String> {
        text.split_whitespace().map(str::to_owned).collect()
    }

    fn probe(text: &str) -> Option<SizeProbe> {
        let argv = args(text);
        size_probe(&argv[0], &argv[1..])
    }

    #[test]
    fn whole_key_reads_are_probed_and_bounded_ones_are_not() {
        assert_eq!(probe("GET k").unwrap().command, "STRLEN");
        assert_eq!(probe("JSON.GET k $").unwrap().command, "MEMORY");
        assert_eq!(probe("HGETALL k").unwrap().command, "HLEN");
        assert_eq!(probe("smembers k").unwrap().command, "SCARD");
        assert_eq!(probe("LRANGE k 0 -1").unwrap().command, "LLEN");
        assert_eq!(probe("ZRANGE k 0 -1 WITHSCORES").unwrap().command, "ZCARD");
        assert_eq!(probe("XRANGE s - +").unwrap().command, "XLEN");
        // 이미 상한 안에 묶인 것 · 키 하나를 통째로 읽지 않는 것은 묻지 않는다.
        assert_eq!(probe("XRANGE s - + COUNT 100"), None);
        assert_eq!(probe("ZRANGEBYSCORE z -inf +inf LIMIT 0 100"), None);
        assert!(probe("ZRANGEBYSCORE z -inf +inf LIMIT 0 -1").is_some());
        assert_eq!(probe("HGET k f"), None);
        assert_eq!(probe("LRANGE k x y"), None, "malformed: the server answers");
        assert_eq!(probe("GET"), None);
    }

    #[test]
    fn probes_judge_the_reply_size() {
        let hgetall = probe("HGETALL k").unwrap();
        assert!(!hgetall.too_large(5_000));
        assert!(hgetall.too_large(5_001), "two reply elements per field");
        let get = probe("GET k").unwrap();
        assert!(!get.too_large(PAGE_BUDGET as u64));
        assert!(get.too_large(PAGE_BUDGET as u64 + 1));
        // 범위는 고르는 수로 판정한다.
        let small = probe("LRANGE k 0 99").unwrap();
        assert!(!small.too_large(1_000_000));
        let all = probe("LRANGE k 0 -1").unwrap();
        assert!(!all.too_large(10_000) && all.too_large(10_001));
        let tail = probe("LRANGE k -10 -1").unwrap();
        assert!(!tail.too_large(u64::MAX));
        assert_eq!(range_len(5, 3, 1), 0);
        assert_eq!(range_len(0, 0, -1), 0);
    }

    #[test]
    fn replies_over_the_limits_are_truncated_at_the_top() {
        let many = Value::Array((0..15_000).map(Value::Int).collect());
        let KvReply::Truncated(cut) = limited_reply(many) else {
            panic!("truncated")
        };
        assert_eq!(cut.elements, 15_000);
        let KvReply::Array(shown) = *cut.reply else {
            panic!("array")
        };
        assert_eq!(shown.len() as u64, MAX_REPLY_ELEMENTS);

        let big = Value::Array(vec![Value::BulkString(vec![b'a'; 3 * 1024 * 1024]); 3]);
        let KvReply::Truncated(cut) = limited_reply(big) else {
            panic!("truncated")
        };
        assert_eq!((cut.elements, cut.bytes), (3, 9 * 1024 * 1024));
        let KvReply::Array(shown) = *cut.reply else {
            panic!("array")
        };
        assert_eq!(shown.len(), 1);

        let small = Value::Array(vec![Value::Int(1)]);
        assert!(matches!(limited_reply(small), KvReply::Array(_)));
    }

    #[test]
    fn the_error_names_the_key_browser() {
        let error = too_large();
        assert_eq!(error.code.as_deref(), Some(KEY_TOO_LARGE));
        assert!(error.message.contains("key browser"));
    }
}
