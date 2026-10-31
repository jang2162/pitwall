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
//! - DBX 데스크톱의 「안전 확인 건너뛰기」 는 옮기지 않았다. 토크나이저는 오류 글만 그대로다.
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
    /// 지우기 · 덮기 같은 확인이 필요한 쓰기. 등급은 확인의 무게다.
    Confirm(RiskGrade),
    /// 거절. 서버에 보내지 않는다.
    Blocked,
}

impl KvCommandClass {
    /// SQL 다섯 갈래와 같은 이름 공간의 등급(읽기 = 조회, `Write` = 쓰기, `Confirm` = 그 등급의 확인). 거절이면 `None`.
    pub fn grade(self) -> Option<RiskGrade> {
        match self {
            Self::Allowed => Some(RiskGrade::Read),
            Self::Write => Some(RiskGrade::Write),
            Self::Confirm(grade) => Some(grade),
            Self::Blocked => None,
        }
    }
}

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
        return KvCommandClass::Blocked;
    };
    let Ok(name) = std::str::from_utf8(name.as_ref()) else {
        return KvCommandClass::Blocked;
    };
    let class = classify_command(name);
    let upper = name.to_ascii_uppercase();
    if matches!(upper.as_str(), "XREAD" | "XREADGROUP") && blocks(&argv[1..]) {
        return KvCommandClass::Blocked;
    }
    class
}

/// `XREAD` · `XREADGROUP` 의 선택 인자(`STREAMS` 앞)에 `BLOCK` 이 있는가. `STREAMS` 뒤는 키 · id 라 보지 않는다.
fn blocks<A: AsRef<[u8]>>(args: &[A]) -> bool {
    args.iter()
        .map(|a| a.as_ref())
        .take_while(|a| !a.eq_ignore_ascii_case(b"STREAMS"))
        .any(|a| a.eq_ignore_ascii_case(b"BLOCK"))
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
        | "LCS"
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
        "DEL" | "UNLINK" | "EXPIRE" | "EXPIREAT" | "PEXPIRE" | "PEXPIREAT" | "RENAME"
        | "RENAMENX" | "GETDEL" | "HDEL" | "HEXPIRE" | "HEXPIREAT" | "HPEXPIRE" | "HPEXPIREAT"
        | "HPERSIST" | "JSON.ARRPOP" | "JSON.ARRTRIM" | "JSON.CLEAR" | "JSON.DEL"
        | "JSON.FORGET" | "LPOP" | "LMOVE" | "LMPOP" | "RPOP" | "RPOPLPUSH" | "LREM" | "LTRIM"
        | "SPOP" | "SREM" | "ZREM" | "ZPOPMAX" | "ZPOPMIN" | "ZMPOP" | "ZREMRANGEBYLEX"
        | "ZREMRANGEBYRANK" | "ZREMRANGEBYSCORE" | "XDEL" | "XTRIM" | "MOVE" | "SORT"
        | "SDIFFSTORE" | "SINTERSTORE" | "SUNIONSTORE" | "ZDIFFSTORE" | "ZINTERSTORE"
        | "ZRANGESTORE" | "ZUNIONSTORE" | "PFMERGE" | "GEOSEARCHSTORE" => {
            KvCommandClass::Confirm(RiskGrade::UnboundedWrite)
        }
        "APPEND" | "BITFIELD" | "BITOP" | "COPY" | "DECR" | "DECRBY" | "GEOADD" | "GEORADIUS"
        | "GEORADIUSBYMEMBER" | "GETEX" | "GETSET" | "INCR" | "INCRBY" | "INCRBYFLOAT" | "SET"
        | "SETEX" | "PSETEX" | "SETNX" | "SETRANGE" | "MSET" | "MSETNX" | "PERSIST" | "HSET"
        | "HMSET" | "HINCRBY" | "HINCRBYFLOAT" | "HSETNX" | "JSON.ARRAPPEND" | "JSON.ARRINSERT"
        | "JSON.MERGE" | "JSON.MSET" | "JSON.NUMINCRBY" | "JSON.NUMMULTBY" | "JSON.SET"
        | "JSON.STRAPPEND" | "JSON.TOGGLE" | "LINSERT" | "LSET" | "LPUSH" | "LPUSHX" | "PFADD"
        | "RPUSH" | "RPUSHX" | "RESTORE" | "SADD" | "ZADD" | "ZINCRBY" | "SETBIT" | "SPUBLISH"
        | "PUBLISH" | "TOUCH" | "XADD" | "XACK" | "XAUTOCLAIM" | "XCLAIM" | "XREADGROUP"
        | "XSETID" => KvCommandClass::Write,
        // 막히는 명령(BLPOP · WAIT …)과 연결 상태 · 점유 명령, 모르는 명령은 모두 여기다(머리 주석).
        _ => KvCommandClass::Blocked,
    }
}

#[cfg(test)]
mod tests;
