//! Redis 명령 분류(14-m3-plan.md M3-09, 결정 M3-D15 · M3-D20): 허용 목록 분류와 명령 글 토크나이저.
//!
//! DBX(<https://github.com/t8y2/dbx>, 커밋 `c5e7e0430653aaef198800a48f9dec46a7cfed5e`, Apache-2.0)의
//! `crates/dbx-driver-redis/src/lib.rs` 에서 `classify_command` · `parse_command_argv` 와 그 시험을 옮겨 **수정함**
//! (고지 `licenses/vendored.md`). 바꾼 것:
//!
//! - 분류 결과를 [`KvCommandClass`] 로 바꾸고 SQL 다섯 갈래와 같은 등급 이름 공간에 잇는다([`KvCommandClass::grade`]).
//!   `Confirm` 은 명령별 등급 자리를 둔다 — 지금은 모두 「범위 없는 쓰기」 와 같은 확인이다.
//! - 연결을 점유하는 막히는 명령(`BLPOP` · `BRPOP` · `BLMOVE` · `BLMPOP` · `BRPOPLPUSH` · `BZPOPMIN` · `BZPOPMAX` · `BZMPOP` ·
//!   `WAIT` · `WAITAOF`)을 허용 · 확인 목록에서 빼 거절로 옮겼다. 한 DB 번호의 연결 하나를 콘솔과 키 브라우저가 함께 쓰므로
//!   (multiplexed) 막힌 명령 뒤에서 키 브라우저가 멈춘다. `BLOCK` 인자가 붙은 `XREAD` · `XREADGROUP` 도 같다 —
//!   인자를 보는 [`classify_argv`] 를 더했다.
//! - 키 공간 전체를 한 번에 도는 스캔을 막는다: `SCAN` · `HSCAN` · `SSCAN` · `ZSCAN` 의 `COUNT` 가 [`MAX_SCAN_COUNT`] 를 넘으면
//!   거절(까닭 [`BlockReason::ScanCount`] — 키 브라우저를 쓰라고 안내한다).
//! - 대상 키를 덮는 쓰기는 `RENAME` 처럼 확인이다: `BITOP`, `STORE` · `STOREDIST` 가 붙은 `GEORADIUS` · `GEORADIUSBYMEMBER`,
//!   `REPLACE` 가 붙은 `COPY` · `RESTORE`(DBX 는 쓰기).
//! - 서버를 오래 붙잡는 명령은 그 등급 그대로 확인이다(14 M3-22): `LCS`, `COUNT n ANY`(n ≤ [`MAX_GEO_ANY_COUNT`])로 묶지 않은 `GEOSEARCH` · `GEORADIUS`
//!   류(DBX 는 허용 · 쓰기). 읽기는 `Confirm(Read)` 라 read-only 연결에서도 확인 뒤 돈다. `SORT_RO` 는 비용이 목록 길이에
//!   비례하므로 분류가 아니라 드라이버의 길이 사전 확인이 묶는다(`SORT` 는 늘 확인).
//! - 거절에 까닭([`BlockReason`])을 싣고, 여러 줄 묶음의 분류([`classify_script`])를 여기 하나에 둔다.
//! - DBX 데스크톱의 「안전 확인 건너뛰기」 는 옮기지 않았다. 토크나이저는 오류 글만 그대로다.
//!
//! **막는 기준**(14-m3-plan.md M3-09): 키 공간 전체를 도는 서버 작업(`KEYS` · `FLUSH*` · 큰 `COUNT` 의 스캔)과 연결의 상태를
//! 바꾸거나 점유하는 명령이다. 키 하나 안의 읽기(`HGETALL` · `SMEMBERS` · `LRANGE 0 -1` · `JSON.GET`)는 허용하고, 콘솔 응답
//! 상한으로 끊는다(드라이버 — `pw-db-driver-redis`). 키 브라우저(M3-18)는 쪽 읽기를 쓴다.
//!
//! **분류기는 이것 하나다** — 관문(M3-22 `db.kv.command`)이 부르고, 드라이버(`pw-db-driver-redis`)는 부르지 않는다
//! (`KvConnection::command` 는 부르는 쪽이 먼저 분류했다고 본다). 모르는 명령(새 Redis 판 · 모듈 · 프록시 명령)은 거절이다.
//! 연결의 상태를 바꾸는 명령(`SELECT` · `AUTH` · `HELLO` · `CLIENT` · `MULTI` · `RESET` …)과 연결을 점유하는 명령
//! (`MONITOR` · `SUBSCRIBE` 류 · 위 막히는 명령)은 목록에 없으므로 거절이다(결정 M3-D15).

use pw_db_types::RiskGrade;

/// 명령 하나의 분류(DBX `RedisCommandSafety`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KvCommandClass {
    /// 읽기. 그대로 돈다.
    Allowed,
    /// 쓰기. read-only 연결에서는 거절, 프로덕션이면 확인(결정 M3-D20).
    Write,
    /// 확인이 필요한 명령. 등급은 확인의 무게다 — 지우기 · 덮기 같은 쓰기는 `UnboundedWrite`, 서버를 오래 붙잡는 읽기
    /// (`LCS` · `COUNT n ANY` 없는 `GEOSEARCH` 류)는 `Read`(read-only 연결에서도 확인 뒤 돈다).
    Confirm(RiskGrade),
    /// 거절. 서버에 보내지 않는다.
    Blocked(BlockReason),
}

/// 거절의 까닭(창이 안내 글을 고른다).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BlockReason {
    /// 허용 목록 밖(모르는 명령 · 키 공간 전체 · 연결 상태를 바꾸는 명령 · 형식이 틀린 인자).
    NotAllowed,
    /// 서버에서 기다리며 연결을 붙잡는 명령(`BLPOP` 류 · `WAIT` · `BLOCK` 이 붙은 `XREAD`).
    Blocking,
    /// 스캔의 `COUNT` 가 [`MAX_SCAN_COUNT`] 를 넘는다 — 키 브라우저를 쓴다.
    ScanCount,
}

/// 콘솔 스캔(`SCAN` · `HSCAN` · `SSCAN` · `ZSCAN`)의 `COUNT` 상한. 키 브라우저의 한 묶음(1,000키 — 14 M3-09 ①)의 열 배다.
/// 실측(redis 7.2.10, 키 100만 개, `SLOWLOG`): `COUNT 10000` 한 번이 서버를 2 ms 남짓 붙잡고, `COUNT 2000000` 은 349 ms 로
/// 거절되는 `KEYS *`(240 ms)보다 길었다. 드라이버의 스캔 묶음도 같은 값으로 묶는다(`pw_db_driver_redis::MAX_SCAN_COUNT`).
pub const MAX_SCAN_COUNT: u64 = 10_000;

impl KvCommandClass {
    /// SQL 다섯 갈래와 같은 이름 공간의 등급(읽기 = 조회, `Write` = 쓰기, `Confirm` = 그 등급의 확인). 거절이면 `None`.
    pub fn grade(self) -> Option<RiskGrade> {
        match self {
            Self::Allowed => Some(RiskGrade::Read),
            Self::Write => Some(RiskGrade::Write),
            Self::Confirm(grade) => Some(grade),
            Self::Blocked(_) => None,
        }
    }

    /// 둘 중 더 위험한 쪽. 거절이 가장 위에 있다(먼저 만난 거절의 까닭을 남긴다). 어느 한쪽이 확인이면 둘 중 높은 등급의
    /// 확인이다(`LCS` 와 `SET` 의 묶음은 쓰기 등급의 확인).
    pub fn max(self, other: Self) -> Self {
        match (self, other) {
            (Self::Blocked(_), _) => self,
            (_, Self::Blocked(_)) => other,
            (Self::Confirm(_), _) | (_, Self::Confirm(_)) => {
                Self::Confirm(self.grade().max(other.grade()).unwrap_or(RiskGrade::Read))
            }
            _ if other.grade() > self.grade() => other,
            _ => self,
        }
    }
}

/// 여러 줄 명령 글(콘솔에서 한 번에 보낸 것)의 분류: 줄마다 명령 하나로 나눠 분류하고, 하나라도 거절이면 전체가 거절, 아니면
/// 가장 높은 등급이다. 빈 줄은 건너뛴다. 따옴표 안의 줄바꿈은 받지 않는다(줄이 곧 명령 경계다). 명령이 없거나 어느 줄이든
/// 나눌 수 없으면 그 오류.
pub fn classify_script(text: &str) -> Result<(Vec<Argv>, KvCommandClass), String> {
    let mut commands = Vec::new();
    let mut class = KvCommandClass::Allowed;
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let argv = parse_command_argv(line)?;
        class = class.max(classify_argv(&argv));
        commands.push(argv);
    }
    if commands.is_empty() {
        return Err("Redis command is empty".to_string());
    }
    Ok((commands, class))
}

/// 명령 하나의 인자(이름 포함, 바이트).
pub type Argv = Vec<Vec<u8>>;

/// 명령 글을 argv 로 나눈다(redis-cli 와 같은 따옴표 · `\` 이스케이프 · `\xHH` 바이트). 끝의 `;` 는 버린다.
pub fn parse_command_argv(command_text: &str) -> Result<Vec<Vec<u8>>, String> {
    // Strip trailing semicolons so commands like "HGETALL aaa;" work naturally
    let command_text = command_text.trim_end().trim_end_matches(';');
    let mut argv: Vec<Vec<u8>> = Vec::new();
    let mut current: Vec<u8> = Vec::new();
    let mut chars = command_text.chars().peekable();
    let mut quote: Option<char> = None;
    let mut escaping = false;
    let mut token_started = false;

    while let Some(ch) = chars.next() {
        if escaping {
            token_started = true;
            if ch == 'x' {
                let hex1 = chars.peek().copied();
                if let Some(h1) = hex1
                    && h1.is_ascii_hexdigit()
                {
                    chars.next();
                    let hex2 = chars.peek().copied();
                    if let Some(h2) = hex2
                        && h2.is_ascii_hexdigit()
                    {
                        chars.next();
                        let byte =
                            u8::from_str_radix(&format!("{h1}{h2}"), 16).expect("two hex digits");
                        current.push(byte);
                        escaping = false;
                        continue;
                    }
                }
                current.push(b'x');
            } else {
                let byte = match ch {
                    'n' => b'\n',
                    'r' => b'\r',
                    't' => b'\t',
                    'a' => b'\x07',
                    'b' => b'\x08',
                    'f' => b'\x0c',
                    'v' => b'\x0b',
                    '\\' => b'\\',
                    '"' => b'"',
                    '\'' => b'\'',
                    other => {
                        let mut buf = [0u8; 4];
                        let s = other.encode_utf8(&mut buf);
                        current.extend_from_slice(s.as_bytes());
                        escaping = false;
                        continue;
                    }
                };
                current.push(byte);
            }
            escaping = false;
            continue;
        }

        if ch == '\\' {
            token_started = true;
            escaping = true;
            continue;
        }

        if let Some(q) = quote {
            if ch == q {
                quote = None;
            } else {
                let mut buf = [0u8; 4];
                let s = ch.encode_utf8(&mut buf);
                current.extend_from_slice(s.as_bytes());
            }
            continue;
        }

        if ch == '"' || ch == '\'' {
            token_started = true;
            quote = Some(ch);
            continue;
        }

        if ch.is_whitespace() {
            if token_started {
                argv.push(std::mem::take(&mut current));
            }
            token_started = false;
            while matches!(chars.peek(), Some(next) if next.is_whitespace()) {
                chars.next();
            }
            continue;
        }

        token_started = true;
        let mut buf = [0u8; 4];
        let s = ch.encode_utf8(&mut buf);
        current.extend_from_slice(s.as_bytes());
    }

    if escaping {
        current.push(b'\\');
    }
    if quote.is_some() {
        return Err("Redis command has an unterminated quote".to_string());
    }
    if token_started {
        argv.push(current);
    }
    if argv.first().is_none_or(Vec::is_empty) {
        return Err("Redis command is empty".to_string());
    }
    Ok(argv)
}

/// argv 하나를 분류한다 — 명령 이름([`classify_command`])에 인자가 바꾸는 규칙을 더한다. 빈 argv 는 거절이다.
pub fn classify_argv<A: AsRef<[u8]>>(argv: &[A]) -> KvCommandClass {
    let Some(name) = argv.first() else {
        return KvCommandClass::Blocked(BlockReason::NotAllowed);
    };
    let Ok(name) = std::str::from_utf8(name.as_ref()) else {
        return KvCommandClass::Blocked(BlockReason::NotAllowed);
    };
    let class = classify_command(name);
    let args: Vec<&[u8]> = argv[1..].iter().map(|a| a.as_ref()).collect();
    let upper = name.to_ascii_uppercase();
    let refused = match upper.as_str() {
        "XREAD" => stream_read_options(&args, 0),
        "XREADGROUP" => match args.first() {
            // `GROUP <그룹> <소비자>` 뒤가 선택 인자다.
            Some(group) if group.eq_ignore_ascii_case(b"GROUP") && args.len() >= 3 => {
                stream_read_options(&args, 3)
            }
            _ => Some(BlockReason::NotAllowed),
        },
        "SCAN" => scan_options(&args, 1),
        "HSCAN" | "SSCAN" | "ZSCAN" => scan_options(&args, 2),
        _ => None,
    };
    if let Some(reason) = refused {
        return KvCommandClass::Blocked(reason);
    }
    let overwrites = match upper.as_str() {
        "GEORADIUS" | "GEORADIUSBYMEMBER" => has_word(&args, &[b"STORE", b"STOREDIST"]),
        "COPY" | "RESTORE" => has_word(&args, &[b"REPLACE"]),
        _ => false,
    };
    if overwrites {
        return KvCommandClass::Confirm(RiskGrade::UnboundedWrite);
    }
    // 서버를 오래 붙잡는 명령(응답 상한은 응답만 끊고 서버 작업은 못 막는다 — 14 M3-22): 그 등급 그대로 확인을 받는다.
    let heavy = match upper.as_str() {
        "GEOSEARCH" => !geo_counted(&args, 1),
        "GEORADIUS" | "GEORADIUS_RO" => !geo_counted(&args, 5),
        "GEORADIUSBYMEMBER" | "GEORADIUSBYMEMBER_RO" => !geo_counted(&args, 4),
        _ => false,
    };
    match class {
        KvCommandClass::Allowed | KvCommandClass::Write if heavy => {
            KvCommandClass::Confirm(class.grade().unwrap_or(RiskGrade::Read))
        }
        class => class,
    }
}

/// `GEOSEARCH` · `GEORADIUS` 류가 확인 없이 도는 `COUNT n ANY` 의 n 상한. `ANY` 는 n 개를 찾으면 멈추므로 n 이 작을 때만 서버
/// 일을 줄인다 — 영역 안 점 수보다 큰 n 은 끝까지 찾는다(재리뷰 2 C′: 30만 점에 `COUNT 9223372036854775807 ANY` 372 ms). 콘솔
/// 응답 상한(원소 1만 — `pw_db_driver_redis::MAX_REPLY_ELEMENTS`)과 같은 값이다.
pub const MAX_GEO_ANY_COUNT: u64 = 10_000;

/// 위치 검색(`GEOSEARCH` · `GEORADIUS` 류)이 `COUNT n ANY`(0 < n ≤ [`MAX_GEO_ANY_COUNT`])로 묶였는가 — `ANY` 가 없으면 `COUNT` 가 있어도 서버는 영역 전체를
/// 찾고 정렬한다(재리뷰 C: 30만 점에서 `COUNT 10` 20 ms, `COUNT 10 ANY` 0 ms). 선택 인자를 `start` 부터 자리대로 걷는다 — 키 ·
/// 멤버 · `STORE` 대상 이름이 `COUNT` · `ANY` 여도 자리로 가르므로 비켜 가지 않는다. 모르는 낱말은 하나씩 건너뛴다(서버가
/// 오류로 답한다).
fn geo_counted(args: &[&[u8]], start: usize) -> bool {
    let mut i = start;
    let (mut counted, mut any) = (false, false);
    while let Some(word) = args.get(i) {
        i += match word.to_ascii_uppercase().as_slice() {
            b"COUNT" => {
                counted = args
                    .get(i + 1)
                    .and_then(|n| std::str::from_utf8(n).ok())
                    .and_then(|n| n.parse::<u64>().ok())
                    .is_some_and(|n| (1..=MAX_GEO_ANY_COUNT).contains(&n));
                2
            }
            b"ANY" => {
                any = true;
                1
            }
            b"FROMMEMBER" | b"STORE" | b"STOREDIST" => 2,
            b"FROMLONLAT" | b"BYRADIUS" => 3,
            b"BYBOX" => 4,
            _ => 1,
        };
    }
    counted && any
}

/// `XREAD` · `XREADGROUP` 의 선택 인자를 자리대로 걷는다(`start` 부터 `STREAMS` 까지 — `COUNT n` · `BLOCK ms` · `NOACK`).
/// 그룹 · 소비자 · 키 이름이 `STREAMS` · `BLOCK` 같은 낱말이어도 자리로 가르므로 비켜 가지 않는다. `BLOCK` 이면 거절, 모르는
/// 선택 인자나 `STREAMS` 가 없는 형식도 거절이다(서버가 받는 새 선택 인자는 검토 전까지 막는다).
fn stream_read_options(args: &[&[u8]], start: usize) -> Option<BlockReason> {
    let mut i = start;
    loop {
        let Some(word) = args.get(i) else {
            return Some(BlockReason::NotAllowed);
        };
        let word = match word {
            w if w.eq_ignore_ascii_case(b"STREAMS") => return None,
            w => w.to_ascii_uppercase(),
        };
        match word.as_slice() {
            b"BLOCK" => return Some(BlockReason::Blocking),
            b"COUNT" => i += 2,
            b"NOACK" => i += 1,
            _ => return Some(BlockReason::NotAllowed),
        }
    }
}

/// 스캔의 선택 인자를 자리대로 걷는다(`MATCH p` · `COUNT n` · `TYPE t` · `NOVALUES` · `NOSCORES`). `COUNT` 가 수가 아니거나
/// [`MAX_SCAN_COUNT`] 를 넘으면 거절, 모르는 선택 인자도 거절이다.
fn scan_options(args: &[&[u8]], start: usize) -> Option<BlockReason> {
    let mut i = start;
    while let Some(word) = args.get(i) {
        match word.to_ascii_uppercase().as_slice() {
            b"COUNT" => {
                let count = args
                    .get(i + 1)
                    .and_then(|n| std::str::from_utf8(n).ok())
                    .and_then(|n| n.parse::<u64>().ok());
                match count {
                    Some(n) if n <= MAX_SCAN_COUNT => i += 2,
                    _ => return Some(BlockReason::ScanCount),
                }
            }
            b"MATCH" | b"TYPE" if i + 1 < args.len() => i += 2,
            b"NOVALUES" | b"NOSCORES" => i += 1,
            _ => return Some(BlockReason::NotAllowed),
        }
    }
    None
}

/// 인자 중에 그 낱말이 있는가(대소문자 무시). 키 이름이 같은 낱말이면 더 엄한 쪽(확인)으로 틀린다.
fn has_word(args: &[&[u8]], words: &[&[u8]]) -> bool {
    args.iter()
        .any(|a| words.iter().any(|w| a.eq_ignore_ascii_case(w)))
}

/// 명령 이름 하나를 분류한다(대소문자 무시). 인자를 봐야 하는 규칙은 [`classify_argv`] 에 있다.
pub fn classify_command(command: &str) -> KvCommandClass {
    // Read access is an explicit allowlist. Commands added by a newer Redis
    // version, module, or proxy remain high-risk until they are reviewed.
    match command.to_ascii_uppercase().as_str() {
        "BITCOUNT"
        | "BITFIELD_RO"
        | "BITPOS"
        | "COMMAND"
        | "DBSIZE"
        | "DUMP"
        | "ECHO"
        | "EXISTS"
        | "EXPIRETIME"
        | "GEODIST"
        | "GEOHASH"
        | "GEOPOS"
        | "GEORADIUS_RO"
        | "GEORADIUSBYMEMBER_RO"
        | "GEOSEARCH"
        | "GET"
        | "GETBIT"
        | "GETRANGE"
        | "HEXISTS"
        | "HGET"
        | "HGETALL"
        | "HKEYS"
        | "HLEN"
        | "HMGET"
        | "HRANDFIELD"
        | "HSCAN"
        | "HSTRLEN"
        | "HPTTL"
        | "HTTL"
        | "HVALS"
        | "INFO"
        | "LASTSAVE"
        | "LINDEX"
        | "LLEN"
        | "LPOS"
        | "LRANGE"
        | "MGET"
        | "OBJECT"
        | "PEXPIRETIME"
        | "PFCOUNT"
        | "PING"
        | "PTTL"
        | "PUBSUB"
        | "RANDOMKEY"
        | "ROLE"
        | "SCAN"
        | "SCARD"
        | "SDIFF"
        | "SINTER"
        | "SINTERCARD"
        | "SISMEMBER"
        | "SMEMBERS"
        | "SMISMEMBER"
        | "SORT_RO"
        | "SRANDMEMBER"
        | "SSCAN"
        | "STRLEN"
        | "SUNION"
        | "TIME"
        | "TTL"
        | "TYPE"
        | "XINFO"
        | "XLEN"
        | "XPENDING"
        | "XRANGE"
        | "XREAD"
        | "XREVRANGE"
        | "ZCARD"
        | "ZCOUNT"
        | "ZDIFF"
        | "ZINTER"
        | "ZINTERCARD"
        | "ZLEXCOUNT"
        | "ZMSCORE"
        | "ZRANDMEMBER"
        | "ZRANGE"
        | "ZRANGEBYLEX"
        | "ZRANGEBYSCORE"
        | "ZRANK"
        | "ZREVRANGE"
        | "ZREVRANGEBYLEX"
        | "ZREVRANGEBYSCORE"
        | "ZREVRANK"
        | "ZSCAN"
        | "ZSCORE"
        | "ZUNION"
        | "BF.CARD"
        | "BF.EXISTS"
        | "BF.INFO"
        | "BF.MEXISTS"
        | "CF.COUNT"
        | "CF.EXISTS"
        | "CF.INFO"
        | "CF.MEXISTS"
        | "CMS.INFO"
        | "CMS.QUERY"
        | "FT._LIST"
        | "FT.AGGREGATE"
        | "FT.DICTDUMP"
        | "FT.EXPLAIN"
        | "FT.EXPLAINCLI"
        | "FT.INFO"
        | "FT.PROFILE"
        | "FT.SEARCH"
        | "FT.SPELLCHECK"
        | "FT.SYNDUMP"
        | "FT.TAGVALS"
        | "GRAPH.RO_QUERY"
        | "JSON.ARRINDEX"
        | "JSON.ARRLEN"
        | "JSON.GET"
        | "JSON.MGET"
        | "JSON.OBJKEYS"
        | "JSON.OBJLEN"
        | "JSON.RESP"
        | "JSON.STRLEN"
        | "JSON.TYPE"
        | "TDIGEST.BYRANK"
        | "TDIGEST.BYREVRANK"
        | "TDIGEST.CDF"
        | "TDIGEST.INFO"
        | "TDIGEST.MAX"
        | "TDIGEST.MIN"
        | "TDIGEST.QUANTILE"
        | "TDIGEST.RANK"
        | "TDIGEST.REVRANK"
        | "TDIGEST.TRIMMED_MEAN"
        | "TOPK.INFO"
        | "TOPK.LIST"
        | "TOPK.QUERY"
        | "TS.GET"
        | "TS.INFO"
        | "TS.MGET"
        | "TS.MRANGE"
        | "TS.QUERYINDEX"
        | "TS.RANGE" => KvCommandClass::Allowed,
        // 두 문자열의 최장 공통 부분열 — 서버가 두 길이의 곱만큼 일한다(응답은 작아 상한으로 못 막는다).
        "LCS" => KvCommandClass::Confirm(RiskGrade::Read),
        "BLPOP" | "BRPOP" | "BLMOVE" | "BLMPOP" | "BRPOPLPUSH" | "BZPOPMIN" | "BZPOPMAX"
        | "BZMPOP" | "WAIT" | "WAITAOF" => KvCommandClass::Blocked(BlockReason::Blocking),
        "DEL" | "UNLINK" | "EXPIRE" | "EXPIREAT" | "PEXPIRE" | "PEXPIREAT" | "RENAME"
        | "RENAMENX" | "GETDEL" | "HDEL" | "HEXPIRE" | "HEXPIREAT" | "HPEXPIRE" | "HPEXPIREAT"
        | "HPERSIST" | "JSON.ARRPOP" | "JSON.ARRTRIM" | "JSON.CLEAR" | "JSON.DEL"
        | "JSON.FORGET" | "LPOP" | "LMOVE" | "LMPOP" | "RPOP" | "RPOPLPUSH" | "LREM" | "LTRIM"
        | "SPOP" | "SREM" | "ZREM" | "ZPOPMAX" | "ZPOPMIN" | "ZMPOP" | "ZREMRANGEBYLEX"
        | "ZREMRANGEBYRANK" | "ZREMRANGEBYSCORE" | "XDEL" | "XTRIM" | "MOVE" | "SORT"
        | "SDIFFSTORE" | "SINTERSTORE" | "SUNIONSTORE" | "ZDIFFSTORE" | "ZINTERSTORE"
        | "ZRANGESTORE" | "ZUNIONSTORE" | "PFMERGE" | "GEOSEARCHSTORE" | "BITOP" => {
            KvCommandClass::Confirm(RiskGrade::UnboundedWrite)
        }
        "APPEND" | "BITFIELD" | "COPY" | "DECR" | "DECRBY" | "GEOADD" | "GEORADIUS"
        | "GEORADIUSBYMEMBER" | "GETEX" | "GETSET" | "INCR" | "INCRBY" | "INCRBYFLOAT" | "SET"
        | "SETEX" | "PSETEX" | "SETNX" | "SETRANGE" | "MSET" | "MSETNX" | "PERSIST" | "HSET"
        | "HMSET" | "HINCRBY" | "HINCRBYFLOAT" | "HSETNX" | "JSON.ARRAPPEND" | "JSON.ARRINSERT"
        | "JSON.MERGE" | "JSON.MSET" | "JSON.NUMINCRBY" | "JSON.NUMMULTBY" | "JSON.SET"
        | "JSON.STRAPPEND" | "JSON.TOGGLE" | "LINSERT" | "LSET" | "LPUSH" | "LPUSHX" | "PFADD"
        | "RPUSH" | "RPUSHX" | "RESTORE" | "SADD" | "ZADD" | "ZINCRBY" | "SETBIT" | "SPUBLISH"
        | "PUBLISH" | "TOUCH" | "XADD" | "XACK" | "XAUTOCLAIM" | "XCLAIM" | "XREADGROUP"
        | "XSETID" => KvCommandClass::Write,
        // 연결 상태 · 점유 명령, 키 공간 전체 명령, 모르는 명령은 모두 여기다(머리 주석).
        _ => KvCommandClass::Blocked(BlockReason::NotAllowed),
    }
}

#[cfg(test)]
mod tests;
