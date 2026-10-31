//! 콘솔 명령(`KvConnection::command`)의 상한(14-m3-plan.md M3-09 — 키 하나 안의 읽기는 허용하고 상한으로 끊는다).
//!
//! - **보내기 전 크기 묻기**([`size_probe`]): 키 하나를 통째로 읽는 알려진 명령(`GET` · `HGETALL` · `HKEYS` · `HVALS` ·
//!   `SMEMBERS` · `LRANGE` · `ZRANGE` 류 · `XRANGE` 류 · `JSON.GET`)은 같은 연결에서 길이(`STRLEN` · `HLEN` · `SCARD` · `LLEN` ·
//!   `ZCARD` · `XLEN` · `MEMORY USAGE`)를 먼저 묻고, 응답이 상한을 넘을 것이면 그 명령을 서버에 보내지 않는다(오류 코드
//!   [`KEY_TOO_LARGE`] — 키 브라우저의 쪽 읽기를 쓰라는 안내). `LIMIT` · `COUNT` 로 이미 상한 안에 묶인 것은 묻지 않는다.
//! - **응답 상한**([`limited_reply`]): 그 밖의 응답(여러 키에 걸친 `SUNION` 등)은 원소 [`MAX_REPLY_ELEMENTS`] · 값 바이트
//!   [`PAGE_BUDGET`] 에서 끊고 `KvReply::Truncated` 로 감싼다. 둘 다 **중첩 전체의 합**이다(`XREAD` 처럼 맨 위 원소가 하나여도).
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
    /// 길이 명령들(이름 포함 argv — `STRLEN k` · `HSTRLEN k f` · `MEMORY USAGE k`). 답(정수, 없는 키는 0)의 합으로 판정한다.
    pub commands: Vec<Vec<String>>,
    check: Check,
}

#[derive(Debug, PartialEq, Eq)]
enum Check {
    /// 길이가 바이트다(`STRLEN` · `HSTRLEN` · `MEMORY USAGE`) — [`PAGE_BUDGET`] 와 견준다.
    Bytes,
    /// 바이트 범위(`GETRANGE`)가 고르는 바이트 수.
    ByteRange { start: i64, stop: i64 },
    /// 길이 × `factor` 가 응답의 원소 수다(`HGETALL` 은 필드마다 둘).
    Elements { factor: u64 },
    /// 번호 범위(`LRANGE` · `ZRANGE`)가 고르는 수 × `factor`.
    Range { start: i64, stop: i64, factor: u64 },
    /// `SRANDMEMBER` · `HRANDFIELD` · `ZRANDMEMBER` 의 count: 양수면 많아야 길이만큼, **음수면 길이와 상관없이 |count| 개**
    /// (같은 원소가 되풀이된다 — 원소 하나짜리 키로 응답을 부풀린다). 빈 키는 빈 답이다.
    Random { count: i64, factor: u64 },
}

impl SizeProbe {
    fn new(command: &str, key: &str, check: Check) -> Option<Self> {
        Some(Self {
            commands: vec![vec![command.to_owned(), key.to_owned()]],
            check,
        })
    }

    fn each(commands: Vec<Vec<String>>, check: Check) -> Option<Self> {
        (!commands.is_empty()).then_some(Self { commands, check })
    }

    /// `size` 는 길이 명령 답의 합.
    pub(crate) fn too_large(&self, size: u64) -> bool {
        match self.check {
            Check::Bytes => size > PAGE_BUDGET as u64,
            Check::ByteRange { start, stop } => range_len(size, start, stop) > PAGE_BUDGET as u64,
            Check::Elements { factor } => size.saturating_mul(factor) > MAX_REPLY_ELEMENTS,
            Check::Range {
                start,
                stop,
                factor,
            } => range_len(size, start, stop).saturating_mul(factor) > MAX_REPLY_ELEMENTS,
            Check::Random { count, factor } => {
                let picked = match u64::try_from(count) {
                    Ok(count) => count.min(size),
                    Err(_) if size == 0 => 0,
                    Err(_) => count.unsigned_abs(),
                };
                picked.saturating_mul(factor) > MAX_REPLY_ELEMENTS
            }
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
///
/// 목록(14-m3-plan.md M3-09 「무거운 명령 기준」 — 키 하나를 통째로 읽는 것과 응답을 부풀리는 것):
/// - 값 바이트: `GET` · `MGET`(`STRLEN` 합), `HGET` · `HMGET`(`HSTRLEN` 합), `GETRANGE`(고르는 바이트), `JSON.GET` ·
///   `DUMP`(`MEMORY USAGE`).
/// - 원소 수: `HGETALL` · `HKEYS` · `HVALS` · `SMEMBERS`, 키 하나의 `SUNION` · `SINTER` · `SDIFF`(`SCARD`) ·
///   `ZUNION` · `ZINTER` · `ZDIFF`(`ZCARD`), 번호 범위 `LRANGE` · `ZRANGE` 류, 묶지 않은 `ZRANGEBYSCORE` 류 ·
///   `XRANGE` 류, `COUNT` 로 묶지 않은(또는 `COUNT 0`) `XREAD` · `XREADGROUP`(stream 마다 `XLEN` 합, `XREAD` 의 id `$` 는 뺀다), `SORT_RO`
///   (`LIMIT` 과 상관없이 목록 길이 — `LLEN` · `SCARD` · `ZCARD` 합), `XINFO STREAM … FULL COUNT 0`, 큰 count 의
///   `SRANDMEMBER` · `HRANDFIELD` · `ZRANDMEMBER`(음수 count 는 키 크기와 상관없이 부푼다).
///
/// 이 밖(여러 키를 합치는 `SUNION` 류 · 원소 하나가 큰 `LINDEX` 등 — 14 M3-22 의 이월 목록)은 응답 상한([`limited_reply`]) ·
/// 응답 타임아웃 · 취소가 묶는다. 서버를 오래 붙잡는 명령(`LCS` 등)은 분류(`pw_db_sql::kv_command`)가 확인을 받는다.
pub(crate) fn size_probe(name: &str, args: &[String]) -> Option<SizeProbe> {
    let key = args.first()?;
    let upper = name.to_ascii_uppercase();
    let has = |word: &str| args.iter().any(|a| a.eq_ignore_ascii_case(word));
    let factor = if has("WITHSCORES") || has("WITHVALUES") {
        2
    } else {
        1
    };
    // **사전 확인이 못 읽는 수는 묶이지 않은 것으로 본다**(재리뷰 2 H): 읽지 못했다고 묻기를 건너뛰면, 서버가 받는 꼴(예:
    // `XREAD COUNT -1` — 서버는 음수를 0, 곧 무제한으로 읽는다)이 사전 확인을 비켜 간다. 서버가 거절할 꼴이면 묻고 난 뒤
    // 서버가 거절한다 — 막히는 것이 지나가는 것보다 낫다. 범위의 못 읽는 끝은 키 전체(`0` · `-1`)로 본다.
    let int = |i: usize| args.get(i).and_then(|a| a.parse::<i64>().ok());
    let start_of = |i: usize| int(i).unwrap_or(0);
    let stop_of = |i: usize| int(i).unwrap_or(-1);
    // 선택 인자 `LIMIT <offset> <count>` · `COUNT <n>` 으로 묶였는가. 선택 인자는 고정 인자(키 · 범위) 뒤 `from` 부터만 찾는다 —
    // 키 이름이 `LIMIT` · `COUNT` 여도 선택 인자로 읽지 않는다. 같은 선택 인자를 겹쳐 주면 서버는 **마지막 것**을 쓴다
    // (`LIMIT 0 5 LIMIT 0 -1` 은 전부 — 재리뷰 3 J) — 선택 인자를 걸으며 마지막 것을 남긴다. 낱말이 없으면 묶이지 않은 것, 수를
    // 못 읽거나 음수(`-1` = 전부)여도 묶이지 않은 것이다.
    let bounded_by = |word: &str, from: usize, skip: usize, times: u64| {
        last_option(args, from, word, skip)
            .and_then(|n| n.parse::<u64>().ok())
            .is_some_and(|n| n.saturating_mul(factor).saturating_mul(times) <= MAX_REPLY_ELEMENTS)
    };
    let each = |command: &str, keys: &[String]| -> Vec<Vec<String>> {
        keys.iter()
            .map(|k| vec![command.to_owned(), k.clone()])
            .collect()
    };
    match upper.as_str() {
        "GET" => SizeProbe::new("STRLEN", key, Check::Bytes),
        "MGET" => SizeProbe::each(each("STRLEN", args), Check::Bytes),
        "HGET" => SizeProbe::each(
            vec![vec!["HSTRLEN".into(), key.clone(), args.get(1)?.clone()]],
            Check::Bytes,
        ),
        "HMGET" => SizeProbe::each(
            args[1..]
                .iter()
                .map(|f| vec!["HSTRLEN".into(), key.clone(), f.clone()])
                .collect(),
            Check::Bytes,
        ),
        "GETRANGE" => SizeProbe::new(
            "STRLEN",
            key,
            Check::ByteRange {
                start: start_of(1),
                stop: stop_of(2),
            },
        ),
        "JSON.GET" | "DUMP" => SizeProbe::new("MEMORY", key, Check::Bytes),
        "HGETALL" => SizeProbe::new("HLEN", key, Check::Elements { factor: 2 }),
        "HKEYS" | "HVALS" => SizeProbe::new("HLEN", key, Check::Elements { factor: 1 }),
        // 키 하나의 집합 연산은 그 키 전체다(`SMEMBERS` · `ZRANGE 0 -1` 과 같다). 여러 키는 응답 상한이 끊는다.
        "SMEMBERS" => SizeProbe::new("SCARD", key, Check::Elements { factor: 1 }),
        "SUNION" | "SINTER" | "SDIFF" if args.len() == 1 => {
            SizeProbe::new("SCARD", key, Check::Elements { factor: 1 })
        }
        // `numkeys` 를 못 읽으면 키 하나로 본다. 입력은 zset 뿐 아니라 set 도 된다(점수 1) — `SORT_RO` 처럼 둘을 함께 묻고
        // 맞는 하나만 센다(재리뷰 2 I).
        "ZUNION" | "ZINTER" | "ZDIFF" if int(0).is_none_or(|n| n == 1) => {
            let input = args.get(1)?;
            SizeProbe::each(
                ["ZCARD", "SCARD"]
                    .iter()
                    .map(|c| vec![(*c).to_owned(), input.clone()])
                    .collect(),
                Check::Elements { factor },
            )
        }
        "SRANDMEMBER" | "HRANDFIELD" | "ZRANDMEMBER" => {
            // count 가 없으면 원소 하나다. 못 읽으면 키 전체(가장 큰 양수 — 길이만큼)로 본다.
            args.get(1)?;
            let count = int(1).unwrap_or(i64::MAX);
            if count.unsigned_abs().saturating_mul(factor) <= MAX_REPLY_ELEMENTS {
                return None;
            }
            let length = match upper.as_str() {
                "SRANDMEMBER" => "SCARD",
                "HRANDFIELD" => "HLEN",
                _ => "ZCARD",
            };
            SizeProbe::new(length, key, Check::Random { count, factor })
        }
        "LRANGE" => SizeProbe::new(
            "LLEN",
            key,
            Check::Range {
                start: start_of(1),
                stop: stop_of(2),
                factor: 1,
            },
        ),
        "ZRANGE" if has("BYSCORE") || has("BYLEX") => (!bounded_by("LIMIT", 3, 2, 1))
            .then(|| SizeProbe::new("ZCARD", key, Check::Elements { factor }))?,
        "ZRANGE" | "ZREVRANGE" => SizeProbe::new(
            "ZCARD",
            key,
            Check::Range {
                start: start_of(1),
                stop: stop_of(2),
                factor,
            },
        ),
        "ZRANGEBYSCORE" | "ZREVRANGEBYSCORE" | "ZRANGEBYLEX" | "ZREVRANGEBYLEX" => {
            (!bounded_by("LIMIT", 3, 2, 1))
                .then(|| SizeProbe::new("ZCARD", key, Check::Elements { factor }))?
        }
        "XRANGE" | "XREVRANGE" => (!bounded_by("COUNT", 3, 1, 1))
            .then(|| SizeProbe::new("XLEN", key, Check::Elements { factor: 1 }))?,
        // `[COUNT n] [BLOCK ms] [NOACK] STREAMS k1 … kn id1 … idn` — `COUNT` 는 stream 마다다. 둘 다 `COUNT 0` 은 묶지 않은
        // 것이다(`XRANGE` 류의 0 은 빈 답이라 다르다). `XREAD` 의 id `$` 는 그 뒤의 새 항목만이라 BLOCK 없이는 빈 답이다 —
        // 묻지 않는다. `XREADGROUP` 의 `>` 는 쌓인 만큼 주므로 묻는다.
        "XREAD" => stream_read_probe(args, 0, true),
        "XREADGROUP" if key.eq_ignore_ascii_case("GROUP") => stream_read_probe(args, 3, false),
        "SORT_RO" => {
            // 비용(`BY` · 정렬)은 목록 길이에 비례한다 — `LIMIT` 과 상관없이 길이를 묻는다. 타입마다 길이 명령이 다르므로 셋을
            // 묻고 합한다(다른 타입의 `WRONGTYPE` 은 0 — `refuse_if_too_large`).
            SizeProbe::each(
                ["LLEN", "SCARD", "ZCARD"]
                    .iter()
                    .map(|c| vec![(*c).to_owned(), key.clone()])
                    .collect(),
                Check::Elements { factor: 1 },
            )
        }
        "XINFO" => {
            // `XINFO STREAM k FULL [COUNT n]` — 기본 10, `COUNT 0` 은 모든 항목. 선택 인자는 키 뒤(3 부터)에서만 찾는다 — 키
            // 이름이 `COUNT` · `FULL` 이어도 선택 인자로 읽지 않는다. `COUNT` 의 수를 못 읽으면 묶이지 않은 것이다.
            let options = args.get(2..).unwrap_or_default();
            let full = key.eq_ignore_ascii_case("STREAM")
                && options.iter().any(|a| a.eq_ignore_ascii_case("FULL"));
            let stream = args.get(1)?;
            // 겹친 `COUNT` 는 서버가 거절한다 — 그래도 다른 자리와 같이 마지막 것을 읽는다.
            let has_count = options.iter().any(|a| a.eq_ignore_ascii_case("COUNT"));
            let unbounded = has_count
                && last_option(args, 2, "COUNT", 1)
                    .and_then(|n| n.parse::<i64>().ok())
                    .is_none_or(|n| n <= 0 || n as u64 > MAX_REPLY_ELEMENTS);
            (full && unbounded)
                .then(|| SizeProbe::new("XLEN", stream, Check::Elements { factor: 1 }))?
        }
        _ => None,
    }
}

/// `from` 부터 선택 인자를 걸어 **마지막** `word` 의 `skip` 번째 값(`LIMIT o c` 의 c 는 2, `COUNT n` 의 n 은 1). 낱말을 만나면 그
/// 값들을 건너뛰고(값 자리의 같은 낱말을 선택 인자로 읽지 않는다), 다른 낱말은 하나씩 건넌다. 낱말이 없거나 마지막 낱말 뒤에
/// 값이 모자라면 `None` 이다 — 부르는 쪽은 묶이지 않은 것으로 본다.
fn last_option<'a>(args: &'a [String], from: usize, word: &str, skip: usize) -> Option<&'a str> {
    let mut i = from;
    let mut last = None;
    while let Some(arg) = args.get(i) {
        if arg.eq_ignore_ascii_case(word) {
            last = Some(args.get(i + skip).map(String::as_str));
            i += skip + 1;
        } else {
            i += 1;
        }
    }
    last.flatten()
}

/// `XREAD` · `XREADGROUP` 의 사전 확인. 선택 인자를 `start` 부터 `STREAMS` 까지 자리대로 걷는다 — 키 자리의 `COUNT` 낱말을
/// 선택 인자로 읽지 않는다(분류기의 `stream_read_options` 와 같은 걸음). `COUNT` 는 1 이상일 때만 묶는다 — 서버는 음수를 0
/// 으로, 0 을 무제한으로 읽는다(재리뷰 2 H). 수를 못 읽어도 묶이지 않은 것으로 본다. 꼴 자체가 틀리면(`STREAMS` 가 없음 ·
/// 모르는 선택 인자 · 키와 id 의 수가 다름) 묻지 않는다 — 서버가 오류로 답한다.
fn stream_read_probe(args: &[String], start: usize, skip_dollar: bool) -> Option<SizeProbe> {
    let mut i = start;
    let mut count = None;
    loop {
        let word = args.get(i)?.to_ascii_uppercase();
        match word.as_str() {
            "STREAMS" => break,
            "COUNT" => {
                count = args.get(i + 1)?.parse::<i64>().ok();
                i += 2;
            }
            "BLOCK" => i += 2,
            "NOACK" => i += 1,
            _ => return None,
        }
    }
    let rest = &args[i + 1..];
    if rest.is_empty() || !rest.len().is_multiple_of(2) {
        return None;
    }
    let (keys, ids) = rest.split_at(rest.len() / 2);
    let keys: Vec<String> = keys
        .iter()
        .zip(ids)
        .filter(|(_, id)| !(skip_dollar && id.as_str() == "$"))
        .map(|(k, _)| k.clone())
        .collect();
    let bounded = count
        .and_then(|n| u64::try_from(n).ok())
        .filter(|&n| n > 0)
        .is_some_and(|n| n.saturating_mul(keys.len() as u64) <= MAX_REPLY_ELEMENTS);
    if bounded {
        return None;
    }
    SizeProbe::each(
        keys.iter()
            .map(|k| vec!["XLEN".to_owned(), k.clone()])
            .collect(),
        Check::Elements { factor: 1 },
    )
}

/// 응답을 상한에서 끊는다. 상한 안이면 그대로다.
///
/// 원소 수 · 바이트는 **중첩 전체의 합**이다 — 배열 속 배열(`XREAD` · `XRANGE` 의 항목 · `XINFO … FULL`), map 의 쌍과 그 값까지
/// 센다. 맨 위만 세면 맨 위 원소 하나짜리 응답(`XREAD` 의 stream 하나)이 상한을 통째로 지난다. 넘으면 앞에서부터(깊이 우선)
/// 상한 안에 드는 것만 싣고, `Truncated` 에 원래 크기를 적는다.
pub(crate) fn limited_reply(value: Value) -> KvReply {
    let (elements, bytes) = measure(&value);
    if elements <= MAX_REPLY_ELEMENTS && bytes <= PAGE_BUDGET as u64 {
        return reply(value);
    }
    let shown = Cut::default().take(value);
    KvReply::Truncated(KvReplyTruncated {
        reply: Box::new(reply(shown)),
        elements,
        bytes,
    })
}

/// 응답 전체의 (원소 수, 값 바이트). 맨 위 값 자신은 세지 않는다 — 배열의 원소 · map 의 쌍 하나가 원소 하나이고, 그 안의
/// 원소를 더한다.
fn measure(value: &Value) -> (u64, u64) {
    match value {
        Value::Array(items) | Value::Set(items) | Value::Push { data: items, .. } => {
            items.iter().fold((0, 0), |(e, b), item| {
                let (ie, ib) = measure(item);
                (e + 1 + ie, b + ib)
            })
        }
        Value::Map(pairs) => pairs.iter().fold((0, 0), |(e, b), (k, v)| {
            let ((ke, kb), (ve, vb)) = (measure(k), measure(v));
            (e + 1 + ke + ve, b + kb + vb)
        }),
        Value::Attribute { data, .. } => measure(data),
        leaf => (0, value_bytes(leaf)),
    }
}

/// 깊이 우선으로 상한 안에 드는 앞부분을 고른다. 한 번 넘으면 그 뒤(형제 · 바깥의 다음 원소)는 모두 뺀다. 아무것도 싣지
/// 않았을 때의 첫 값은 크더라도 싣는다(값 하나는 변환이 4 MiB 에서 자른다).
#[derive(Default)]
struct Cut {
    elements: u64,
    bytes: u64,
    full: bool,
}

impl Cut {
    fn take(&mut self, value: Value) -> Value {
        match value {
            Value::Array(items) | Value::Set(items) | Value::Push { data: items, .. } => {
                Value::Array(self.items(items))
            }
            Value::Map(pairs) => {
                let mut kept = Vec::new();
                for (k, v) in pairs {
                    if !self.room() {
                        break;
                    }
                    let k_leaf = !nested(&k);
                    let k = self.take(k);
                    if k_leaf && self.full {
                        break;
                    }
                    let v_leaf = !nested(&v);
                    let v = self.take(v);
                    if v_leaf && self.full {
                        break;
                    }
                    kept.push((k, v));
                }
                Value::Map(kept)
            }
            Value::Attribute { data, .. } => self.take(*data),
            leaf => {
                let size = value_bytes(&leaf);
                if self.bytes > 0 && self.bytes.saturating_add(size) > PAGE_BUDGET as u64 {
                    self.full = true;
                    return Value::Nil;
                }
                self.bytes = self.bytes.saturating_add(size);
                leaf
            }
        }
    }

    fn items(&mut self, items: Vec<Value>) -> Vec<Value> {
        let mut kept = Vec::new();
        for item in items {
            if !self.room() {
                break;
            }
            let leaf = !nested(&item);
            let item = self.take(item);
            // 바이트에 걸린 값은 싣지 않는다(자리표 `Nil` 을 버린다).
            if leaf && self.full {
                break;
            }
            kept.push(item);
        }
        kept
    }

    /// 원소 하나를 더 실을 자리가 있으면 세고 true.
    fn room(&mut self) -> bool {
        if self.full || self.elements >= MAX_REPLY_ELEMENTS {
            self.full = true;
            return false;
        }
        self.elements += 1;
        true
    }
}

/// 원소를 품는 값인가(배열 · map).
fn nested(value: &Value) -> bool {
    matches!(
        value,
        Value::Array(_)
            | Value::Set(_)
            | Value::Push { .. }
            | Value::Map(_)
            | Value::Attribute { .. }
    )
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

    /// 묻기의 첫 길이 명령 이름.
    fn asks(text: &str) -> String {
        let probe = probe(text).unwrap_or_else(|| panic!("{text} is not probed"));
        probe.commands[0][0].clone()
    }

    #[test]
    fn whole_key_reads_are_probed_and_bounded_ones_are_not() {
        assert_eq!(asks("GET k"), "STRLEN");
        assert_eq!(asks("JSON.GET k $"), "MEMORY");
        assert_eq!(asks("HGETALL k"), "HLEN");
        assert_eq!(asks("smembers k"), "SCARD");
        assert_eq!(asks("LRANGE k 0 -1"), "LLEN");
        assert_eq!(asks("ZRANGE k 0 -1 WITHSCORES"), "ZCARD");
        assert_eq!(asks("XRANGE s - +"), "XLEN");
        // 이미 상한 안에 묶인 것 · 키 하나를 통째로 읽지 않는 것은 묻지 않는다.
        assert_eq!(probe("XRANGE s - + COUNT 100"), None);
        assert_eq!(probe("ZRANGEBYSCORE z -inf +inf LIMIT 0 100"), None);
        assert!(probe("ZRANGEBYSCORE z -inf +inf LIMIT 0 -1").is_some());
        assert_eq!(probe("HEXISTS k f"), None);
        // 못 읽는 범위는 키 전체로 본다(재리뷰 2 — 묻고 나서 서버가 거절한다).
        assert_eq!(asks("LRANGE k x y"), "LLEN");
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

    /// M3-22 리뷰 M3: 통째 읽기(`GETRANGE 0 -1` · `DUMP` · `MGET` · `HGET`)와 부푸는 읽기(음수 count)도 먼저 묻는다.
    #[test]
    fn whole_reads_and_amplifying_reads_are_probed() {
        assert_eq!(asks("GETRANGE k 0 -1"), "STRLEN");
        assert_eq!(asks("DUMP k"), "MEMORY");
        assert_eq!(
            probe("MGET a b c").unwrap().commands,
            vec![args("STRLEN a"), args("STRLEN b"), args("STRLEN c")]
        );
        assert_eq!(
            probe("HGET h f").unwrap().commands,
            vec![args("HSTRLEN h f")]
        );
        assert_eq!(probe("HMGET h f g").unwrap().commands.len(), 2);
        assert_eq!(asks("SRANDMEMBER s -2000000"), "SCARD");
        assert_eq!(asks("HRANDFIELD h -6000 WITHVALUES"), "HLEN");
        assert_eq!(asks("ZRANDMEMBER z 20000"), "ZCARD");
        assert_eq!(
            probe("XREAD STREAMS a b 0 0").unwrap().commands,
            vec![args("XLEN a"), args("XLEN b")]
        );
        assert_eq!(asks("XINFO STREAM s FULL COUNT 0"), "XLEN");
        // 키 하나의 집합 연산은 그 키 전체다(14 M3-22 판정(2)). 여러 키는 묻지 않는다(응답 상한).
        for text in ["SUNION s", "SINTER s", "SDIFF s"] {
            assert_eq!(asks(text), "SCARD", "{text}");
        }
        assert!(probe("ZUNION 1 z WITHSCORES").unwrap().too_large(5_001));
        assert!(!probe("ZUNION 1 z").unwrap().too_large(5_001));
        for text in [
            "SUNION a b",
            "SDIFF a b",
            "ZUNION 2 a b",
            "ZINTER 2 a b WITHSCORES",
        ] {
            assert_eq!(probe(text), None, "{text}");
        }
        // 이미 상한 안에 묶인 것은 묻지 않는다.
        assert_eq!(probe("SRANDMEMBER s -10000"), None);
        assert_eq!(probe("SRANDMEMBER s"), None);
        assert_eq!(probe("HRANDFIELD h -5000 WITHVALUES"), None);
        assert_eq!(probe("XREAD COUNT 100 STREAMS a b 0 0"), None);
        // 재리뷰 A: `COUNT 0` 은 묶지 않은 것이고, 키 자리의 `COUNT` 낱말은 선택 인자가 아니다.
        assert_eq!(
            probe("XREAD COUNT 0 STREAMS a 0").unwrap().commands,
            vec![args("XLEN a")]
        );
        assert_eq!(
            probe("XREAD STREAMS a COUNT 0 0").unwrap().commands,
            vec![args("XLEN a"), args("XLEN COUNT")]
        );
        // 재리뷰 F: id `$` 의 stream 은 빈 답이라 묻지 않는다.
        assert_eq!(probe("XREAD STREAMS a $"), None);
        assert_eq!(
            probe("XREAD STREAMS a b $ 0").unwrap().commands,
            vec![args("XLEN b")]
        );
        // 재리뷰 E: `XREADGROUP` 도 같은 규칙이고 `>` 는 쌓인 만큼이라 묻는다.
        assert_eq!(
            probe("XREADGROUP GROUP g c STREAMS a >").unwrap().commands,
            vec![args("XLEN a")]
        );
        assert!(probe("XREADGROUP GROUP g c COUNT 0 NOACK STREAMS a >").is_some());
        assert_eq!(probe("XREADGROUP GROUP g c COUNT 10 STREAMS a >"), None);
        assert_eq!(
            probe("XREADGROUP GROUP STREAMS COUNT STREAMS a >")
                .unwrap()
                .commands,
            vec![args("XLEN a")],
            "group and consumer names are skipped by position"
        );
        // 재리뷰 D: `SORT_RO` 는 `LIMIT` 과 상관없이 목록 길이를 묻는다(타입마다 길이 명령 — 맞지 않는 타입은 0).
        for text in [
            "SORT_RO k",
            "SORT_RO k LIMIT 0 10",
            "SORT_RO k BY nosort GET #",
        ] {
            assert_eq!(
                probe(text).unwrap().commands,
                vec![args("LLEN k"), args("SCARD k"), args("ZCARD k")],
                "{text}"
            );
        }
        assert!(probe("SORT_RO k").unwrap().too_large(10_001));
        // 재리뷰 2 I: 한 키 ZUNION 류의 입력은 set 도 된다 — ZCARD 와 SCARD 를 함께 묻는다.
        for text in ["ZUNION 1 z", "ZINTER 1 z WITHSCORES", "ZDIFF 1 z"] {
            assert_eq!(
                probe(text).unwrap().commands,
                vec![args("ZCARD z"), args("SCARD z")],
                "{text}"
            );
        }
        assert!(!probe("SORT_RO k").unwrap().too_large(10_000));
        assert!(
            probe("XREAD COUNT 6000 STREAMS a b 0 0").is_some(),
            "COUNT is per stream"
        );
        assert_eq!(
            probe("XINFO STREAM s FULL"),
            None,
            "the default COUNT is 10"
        );
        assert_eq!(probe("XINFO STREAM s"), None);
    }

    #[test]
    fn amplifying_reads_are_judged_by_the_reply_not_the_key() {
        // 음수 count 는 원소 하나짜리 키도 부풀린다. 빈 키는 빈 답이다.
        let negative = probe("SRANDMEMBER s -2000000").unwrap();
        assert!(negative.too_large(1));
        assert!(!negative.too_large(0));
        // 양수 count 는 많아야 길이만큼이다.
        let positive = probe("SRANDMEMBER s 2000000").unwrap();
        assert!(!positive.too_large(10_000) && positive.too_large(10_001));
        let with_values = probe("HRANDFIELD h -6000 WITHVALUES").unwrap();
        assert!(with_values.too_large(1), "two reply elements per field");
        // 바이트 범위는 고르는 바이트로 판정한다.
        let whole = probe("GETRANGE k 0 -1").unwrap();
        assert!(!whole.too_large(PAGE_BUDGET as u64) && whole.too_large(PAGE_BUDGET as u64 + 1));
        assert!(!probe("GETRANGE k 0 99").unwrap().too_large(u64::MAX));
    }

    /// M3-22 리뷰 H2: 중첩 응답(`XREAD` 의 stream 하나 속 항목들)도 원소 · 바이트를 모두 센다.
    #[test]
    fn nested_replies_count_every_level() {
        let entry = |i: i64| {
            Value::Array(vec![
                Value::BulkString(format!("{i}-0").into_bytes()),
                Value::Array(vec![
                    Value::BulkString(b"f".to_vec()),
                    Value::BulkString(b"v".to_vec()),
                ]),
            ])
        };
        let xread = Value::Array(vec![Value::Array(vec![
            Value::BulkString(b"st".to_vec()),
            Value::Array((0..200_000).map(entry).collect()),
        ])]);
        let KvReply::Truncated(cut) = limited_reply(xread) else {
            panic!("truncated")
        };
        // 맨 위 1 + stream [이름, 항목들] 2 + 항목마다 (1 + id 1 + 필드 배열 1 + 필드 2).
        assert_eq!(cut.elements, 1 + 2 + 200_000 * 5);
        let shown = count(&cut.reply);
        assert!(shown <= MAX_REPLY_ELEMENTS, "{shown} elements shown");
        assert!(shown > MAX_REPLY_ELEMENTS - 10, "{shown} elements shown");

        // 바이트도 중첩 전체로: 안쪽 배열의 큰 값 셋.
        let big = Value::Array(vec![Value::Array(vec![
            Value::BulkString(vec![
                b'a';
                3 * 1024
                    * 1024
            ]);
            3
        ])]);
        let KvReply::Truncated(cut) = limited_reply(big) else {
            panic!("truncated")
        };
        assert_eq!((cut.elements, cut.bytes), (4, 9 * 1024 * 1024));
        let KvReply::Array(outer) = *cut.reply else {
            panic!("array")
        };
        let KvReply::Array(inner) = &outer[0] else {
            panic!("inner array")
        };
        assert_eq!(inner.len(), 1);

        // map 의 값 속 배열도 센다.
        let map = Value::Map(vec![(
            Value::SimpleString("k".into()),
            Value::Array((0..20_000).map(Value::Int).collect()),
        )]);
        let KvReply::Truncated(cut) = limited_reply(map) else {
            panic!("truncated")
        };
        assert_eq!(cut.elements, 1 + 20_000);
        assert!(count(&cut.reply) <= MAX_REPLY_ELEMENTS);
    }

    /// 보이는 응답의 원소 수(맨 위 값은 세지 않는다 — `measure` 와 같다).
    fn count(reply: &KvReply) -> u64 {
        match reply {
            KvReply::Array(items) => items.iter().map(|i| 1 + count(i)).sum(),
            KvReply::Map(pairs) => pairs
                .iter()
                .map(|p| 1 + count(&p.key) + count(&p.value))
                .sum(),
            _ => 0,
        }
    }

    #[test]
    fn the_error_names_the_key_browser() {
        let error = too_large();
        assert_eq!(error.code.as_deref(), Some(KEY_TOO_LARGE));
        assert!(error.message.contains("key browser"));
    }
}

/// 재리뷰 2 H 와 그 꼴의 훑기: **사전 확인이 못 읽는 수는 묶이지 않은 것으로 본다**, 선택 인자는 고정 인자 뒤에서만 찾는다.
#[cfg(test)]
mod unreadable_numbers {
    use super::*;

    fn probe(text: &str) -> Option<SizeProbe> {
        let argv: Vec<String> = text.split_whitespace().map(str::to_owned).collect();
        size_probe(&argv[0], &argv[1..])
    }

    fn asked(text: &str) -> Vec<String> {
        probe(text)
            .unwrap_or_else(|| panic!("{text} was not probed"))
            .commands
            .into_iter()
            .map(|c| c.join(" "))
            .collect()
    }

    /// H: 서버는 음수 `COUNT` 를 0 으로, 0 을 무제한으로 읽는다. 수가 아닌 `COUNT` 도 묻는다.
    #[test]
    fn negative_or_unreadable_stream_counts_are_unbounded() {
        for text in [
            "XREAD COUNT -1 STREAMS st 0",
            "XREAD COUNT abc STREAMS st 0",
            "XREADGROUP GROUP g c COUNT -1 STREAMS st >",
            "XREADGROUP GROUP g c COUNT -9223372036854775808 NOACK STREAMS st >",
            "XREADGROUP GROUP g c COUNT 1e3 STREAMS st >",
        ] {
            assert_eq!(asked(text), ["XLEN st"], "{text}");
        }
        assert_eq!(probe("XREAD COUNT 10 STREAMS st 0"), None);
    }

    /// 범위의 못 읽는 끝은 키 전체로 본다(서버가 거절할 꼴이어도 먼저 묻는다).
    #[test]
    fn unreadable_range_ends_read_the_whole_key() {
        assert_eq!(asked("GETRANGE k 0 end"), ["STRLEN k"]);
        assert!(
            probe("GETRANGE k 0 end")
                .unwrap()
                .too_large(5 * 1024 * 1024)
        );
        assert!(
            !probe("GETRANGE k start 9")
                .unwrap()
                .too_large(5 * 1024 * 1024)
        );
        assert!(probe("LRANGE l 0 end").unwrap().too_large(10_001));
        assert!(probe("ZRANGE z first -1").unwrap().too_large(10_001));
        assert!(!probe("LRANGE l 0 99").unwrap().too_large(10_001));
    }

    /// 못 읽는 count 는 키 전체(길이만큼)다. count 가 없으면 원소 하나라 묻지 않는다.
    #[test]
    fn unreadable_random_counts_read_the_whole_key() {
        assert_eq!(asked("SRANDMEMBER s many"), ["SCARD s"]);
        assert!(probe("SRANDMEMBER s many").unwrap().too_large(10_001));
        assert!(!probe("SRANDMEMBER s many").unwrap().too_large(10_000));
        assert_eq!(asked("HRANDFIELD h x WITHVALUES"), ["HLEN h"]);
        assert_eq!(probe("SRANDMEMBER s"), None);
    }

    /// 못 읽는 `numkeys` 는 키 하나로 본다.
    #[test]
    fn unreadable_numkeys_probe_the_first_input() {
        assert_eq!(asked("ZUNION one z"), ["ZCARD z", "SCARD z"]);
        assert_eq!(probe("ZUNION 2 a b"), None);
    }

    /// 선택 인자는 고정 인자 뒤에서만 찾는다 — 키 이름이 `LIMIT` · `COUNT` 여도 그것으로 묶였다고 보지 않는다. 묶인 수를 못
    /// 읽어도 묶이지 않은 것이다.
    #[test]
    fn option_words_in_key_positions_do_not_bound_the_read() {
        assert_eq!(asked("ZRANGEBYSCORE LIMIT 0 5"), ["ZCARD LIMIT"]);
        assert_eq!(asked("ZRANGE LIMIT 0 5 BYSCORE"), ["ZCARD LIMIT"]);
        assert_eq!(asked("XRANGE COUNT 5 +"), ["XLEN COUNT"]);
        assert_eq!(asked("ZRANGEBYSCORE z 0 5 LIMIT 0 many"), ["ZCARD z"]);
        assert_eq!(asked("XRANGE st - + COUNT -1"), ["XLEN st"]);
        assert_eq!(probe("ZRANGEBYSCORE z 0 5 LIMIT 0 10"), None);
        assert_eq!(probe("XRANGE st - + COUNT 10"), None);
    }

    /// `XINFO STREAM k FULL COUNT n` 도 같다 — 키가 `COUNT` 여도, `COUNT` 를 못 읽어도 묻는다. 기본(COUNT 없음)은 10 이다.
    #[test]
    fn xinfo_full_counts_are_read_after_the_key() {
        assert_eq!(asked("XINFO STREAM COUNT FULL COUNT 0"), ["XLEN COUNT"]);
        assert_eq!(asked("XINFO STREAM st FULL COUNT many"), ["XLEN st"]);
        assert_eq!(probe("XINFO STREAM COUNT FULL"), None);
        assert_eq!(probe("XINFO STREAM st FULL COUNT 100"), None);
    }
}

/// 재리뷰 3 J 와 훑기 표의 「겹친 옵션」 열: 같은 선택 인자를 겹쳐 주면 서버는 마지막 것을 쓴다(`XINFO … FULL` 은 거절 — 그래도
/// 마지막 것을 읽는다). 표의 행마다 「작은 값 뒤에 무제한 값」 꼴이 사전 확인 · 확인 · 거절 중 하나에 걸린다. 행 수를 단언해
/// 표에서 행이 빠지거나 시험이 행을 건너뛰지 않게 한다.
#[cfg(test)]
mod overlapped_options {
    use super::*;
    use pw_db_sql::kv_command::{BlockReason, KvCommandClass, classify_argv};

    #[derive(Debug, Clone, Copy, PartialEq)]
    enum Caught {
        /// 드라이버가 길이를 먼저 묻는다(사전 확인).
        Probed,
        /// 분류가 확인을 요구한다(GEO 면제가 풀린다).
        Confirmed,
        /// 분류가 거절한다(스캔 `COUNT` 상한).
        Refused,
    }

    /// (작은 값 뒤에 무제한 값을 겹친 명령, 걸리는 곳, 겹치지 않은 작은 값 하나 — 걸리지 않아야 한다).
    const ROWS: &[(&str, Caught, &str)] = &[
        (
            "ZRANGEBYSCORE z -inf +inf LIMIT 0 5 LIMIT 0 -1",
            Caught::Probed,
            "ZRANGEBYSCORE z -inf +inf LIMIT 0 5",
        ),
        (
            "ZREVRANGEBYSCORE z +inf -inf LIMIT 0 5 LIMIT 0 -1",
            Caught::Probed,
            "ZREVRANGEBYSCORE z +inf -inf LIMIT 0 5",
        ),
        (
            "ZRANGEBYLEX z - + LIMIT 0 5 LIMIT 0 -1",
            Caught::Probed,
            "ZRANGEBYLEX z - + LIMIT 0 5",
        ),
        (
            "ZREVRANGEBYLEX z + - LIMIT 0 5 LIMIT 0 -1",
            Caught::Probed,
            "ZREVRANGEBYLEX z + - LIMIT 0 5",
        ),
        (
            "ZRANGE z -inf +inf BYSCORE LIMIT 0 5 LIMIT 0 -1",
            Caught::Probed,
            "ZRANGE z -inf +inf BYSCORE LIMIT 0 5",
        ),
        (
            "ZRANGE z - + BYLEX REV LIMIT 0 5 LIMIT 0 -1",
            Caught::Probed,
            "ZRANGE z - + BYLEX REV LIMIT 0 5",
        ),
        (
            "XRANGE s - + COUNT 5 COUNT 1000000",
            Caught::Probed,
            "XRANGE s - + COUNT 5",
        ),
        (
            "XREVRANGE s + - COUNT 5 COUNT 1000000",
            Caught::Probed,
            "XREVRANGE s + - COUNT 5",
        ),
        (
            "XREAD COUNT 5 COUNT 0 STREAMS s 0",
            Caught::Probed,
            "XREAD COUNT 5 STREAMS s 0",
        ),
        (
            "XREADGROUP GROUP g c COUNT 5 COUNT 0 STREAMS s >",
            Caught::Probed,
            "XREADGROUP GROUP g c COUNT 5 STREAMS s >",
        ),
        (
            "XINFO STREAM s FULL COUNT 5 COUNT 0",
            Caught::Probed,
            "XINFO STREAM s FULL COUNT 5",
        ),
        (
            "GEOSEARCH g FROMLONLAT 0 0 BYRADIUS 1 km COUNT 3 ANY COUNT 99999999",
            Caught::Confirmed,
            "GEOSEARCH g FROMLONLAT 0 0 BYRADIUS 1 km COUNT 3 ANY",
        ),
        (
            "GEORADIUS_RO g 0 0 1 km COUNT 3 ANY COUNT 99999999",
            Caught::Confirmed,
            "GEORADIUS_RO g 0 0 1 km COUNT 3 ANY",
        ),
        (
            "GEORADIUSBYMEMBER_RO g m 1 km COUNT 3 ANY COUNT 99999999",
            Caught::Confirmed,
            "GEORADIUSBYMEMBER_RO g m 1 km COUNT 3 ANY",
        ),
        (
            "SCAN 0 COUNT 10 COUNT 1000000",
            Caught::Refused,
            "SCAN 0 COUNT 10",
        ),
        (
            "SSCAN s 0 COUNT 10 COUNT 1000000",
            Caught::Refused,
            "SSCAN s 0 COUNT 10",
        ),
    ];

    fn argv(text: &str) -> Vec<String> {
        text.split_whitespace().map(str::to_owned).collect()
    }

    fn caught(text: &str) -> Option<Caught> {
        let argv = argv(text);
        match classify_argv(&argv) {
            KvCommandClass::Blocked(BlockReason::ScanCount) => return Some(Caught::Refused),
            KvCommandClass::Blocked(other) => panic!("{text}: refused for {other:?}"),
            KvCommandClass::Confirm(_) => return Some(Caught::Confirmed),
            KvCommandClass::Allowed | KvCommandClass::Write => {}
        }
        size_probe(&argv[0], &argv[1..]).map(|_| Caught::Probed)
    }

    #[test]
    fn the_last_of_overlapped_options_is_judged() {
        assert_eq!(
            ROWS.len(),
            16,
            "a row was added or removed — update the report table too"
        );
        let mut checked = 0;
        for &(overlapped, expected, single) in ROWS {
            assert_eq!(caught(overlapped), Some(expected), "{overlapped}");
            assert_eq!(caught(single), None, "{single}");
            checked += 1;
        }
        assert_eq!(checked, ROWS.len());
    }

    /// 값 자리의 같은 낱말은 선택 인자가 아니다(`LIMIT 0 LIMIT` 의 끝 `LIMIT` 은 count 자리) — 못 읽는 수라 묻는다.
    #[test]
    fn a_trailing_option_word_without_its_values_is_unbounded() {
        assert!(caught("ZRANGEBYSCORE z -inf +inf LIMIT 0 5 LIMIT").is_some());
        assert!(caught("ZRANGEBYSCORE z -inf +inf LIMIT 0 LIMIT").is_some());
        assert!(caught("XRANGE s - + COUNT 5 COUNT").is_some());
        assert_eq!(caught("XRANGE s - + COUNT 5 WITHSCORES"), None);
    }
}
