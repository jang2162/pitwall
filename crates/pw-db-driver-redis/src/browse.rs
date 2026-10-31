//! `KvConnection` 의 구현: 키 스캔(키마다 타입 · TTL · 크기 파이프라인), 타입별 값 읽기(쪽 단위, 쪽 하나 [`PAGE_BUDGET`]),
//! TTL · 삭제 · 이름 바꾸기, 명령.
//!
//! 쪽 예산은 「닿으면 멈춘다」 다: 원소 수 상한이나 바이트 예산에 닿은 응답에서 쪽을 끝낸다. 바이트는 **창으로 갈 JSON 의
//! 크기**로 센다(원소마다 `{"field":…,"value":…}` 같은 덧붙임 · hex 로 두 배가 되는 바이트 값까지 — 원본 바이트만 세면 작은
//! 필드가 많은 hash 한 쪽이 예산의 몇 배가 된다). 서버에 묻는 원소 수를 남은 예산에
//! 맞춰 줄이므로 넘는 것은 마지막 응답 하나뿐이다 — 번호 범위(list · zset · stream)는 정확히 그 수가 오지만 `HSCAN` ·
//! `SSCAN` 의 `COUNT` 는 근사라 몇 개 더 올 수 있다(필드 256 KiB 의 hash 에서 쪽 하나가 4.5 MiB 쯤, 실측). 원소 하나가 예산보다
//! 크면 그 값은 앞부분만 싣는다.
//!
//! read-only 연결은 키를 바꾸는 메서드(`set_ttl` · `persist` · `delete` · `rename`)를 드라이버에서도 거절한다 — Redis 에는 DB
//! 수준의 읽기 전용 트랜잭션이 없어(결정 M3-D6 의 「DB 수준」 자리) 관문(M3-22) 뒤의 마지막 자리다. `command` 는 관문의
//! 분류(read-only 면 `Allowed` 만)를 믿는다.

use std::collections::HashSet;
use std::sync::Arc;

use pw_db_driver::{BoxFuture, DbResult, Interrupt, KvConnection};
use pw_db_types::{
    DbError, DbErrorKind, DbValue, KvField, KvKey, KvKeyInfo, KvRange, KvRenameOutcome, KvReply,
    KvScanPage, KvScanRequest, KvScored, KvStreamEntry, KvType, KvValue, KvValuePage,
};
use redis::Value;

use crate::PAGE_BUDGET;
use crate::connection::Connection;
use crate::console;
use crate::reply::{self, bytes_value};

/// 패턴이 드문 키 공간에서 `SCAN` 한 번이 빈 묶음을 낼 수 있다 — 한 묶음을 채우려고 더 묻는 상한.
const MAX_SCAN_ROUNDS: u32 = 8;

/// 컬렉션 쪽을 읽을 때 한 번에 묻는 원소 수의 상한. 첫 요청은 원소 하나([`FIRST_CHUNK`])로 크기를 보고, 그 평균으로 남은
/// 예산에 맞춰 늘린다 — `HSCAN` · `SSCAN` 의 `COUNT` 는 원소 수가 아니라 둘러볼 칸 수라, 작은 해시 테이블에 큰 `COUNT` 를
/// 주면 값이 아무리 커도 한 번에 전부 돌려준다.
const MAX_CHUNK: usize = 1_000;
const FIRST_CHUNK: usize = 1;

pub(crate) struct RedisKv {
    conn: Connection,
    read_only: bool,
}

impl RedisKv {
    pub(crate) fn new(conn: Connection, read_only: bool) -> Self {
        Self { conn, read_only }
    }

    fn writable(&self) -> DbResult<()> {
        if self.read_only {
            Err(DbError::new(
                DbErrorKind::ReadOnly,
                "this Redis connection is read-only",
            ))
        } else {
            Ok(())
        }
    }

    async fn scan_page(&mut self, request: KvScanRequest) -> DbResult<KvScanPage> {
        // 드라이버의 마지막 자리에서 묶는다 — 콘솔 스캔의 상한과 같은 값(`pw_db_sql::kv_command::MAX_SCAN_COUNT`).
        let count = request.count.clamp(1, crate::MAX_SCAN_COUNT);
        let mut cursor = match request.cursor.as_deref() {
            None => 0,
            Some(text) => parse_cursor(text)?,
        };
        let mut keys: Vec<Vec<u8>> = Vec::new();
        let mut seen = HashSet::new();
        for _ in 0..MAX_SCAN_ROUNDS {
            let pattern = request.pattern.clone();
            let (next, batch): (u64, Vec<Vec<u8>>) = self
                .conn
                .query(|mut c| async move {
                    let mut cmd = redis::cmd("SCAN");
                    cmd.arg(cursor);
                    if let Some(pattern) = pattern {
                        cmd.arg("MATCH").arg(pattern);
                    }
                    cmd.arg("COUNT").arg(count).query_async(&mut c).await
                })
                .await?;
            // SCAN 은 같은 키를 두 번 낼 수 있다(재해시 중) — 한 묶음 안에서는 한 번만.
            keys.extend(batch.into_iter().filter(|k| seen.insert(k.clone())));
            cursor = next;
            if cursor == 0 || keys.len() >= count as usize {
                break;
            }
        }
        let infos = self.key_infos(keys).await?;
        Ok(KvScanPage {
            keys: infos,
            next_cursor: (cursor != 0).then(|| cursor.to_string()),
        })
    }

    /// 키마다 `TYPE` · `PTTL` · `MEMORY USAGE` 를 한 파이프라인으로. `MEMORY USAGE` 를 못 쓰는 서버(이름을 바꾼 관리형 등)면
    /// 타입별 길이로 한 번 더 묻는다. 그사이 사라진 키는 뺀다.
    async fn key_infos(&mut self, keys: Vec<Vec<u8>>) -> DbResult<Vec<KvKeyInfo>> {
        if keys.is_empty() {
            return Ok(Vec::new());
        }
        let probe = keys.clone();
        let values: Vec<Value> = self
            .conn
            .query(|mut c| async move {
                let mut pipe = redis::pipe();
                for key in &probe {
                    pipe.cmd("TYPE").arg(key);
                    pipe.cmd("PTTL").arg(key);
                    pipe.cmd("MEMORY").arg("USAGE").arg(key);
                }
                pipe.ignore_errors().query_async(&mut c).await
            })
            .await?;
        let mut infos = Vec::with_capacity(keys.len());
        let mut no_memory = Vec::new();
        for (key, triple) in keys.into_iter().zip(values.chunks(3)) {
            let [kind, ttl, size] = triple else {
                return Err(protocol("pipeline reply is short"));
            };
            let Some(kind) = key_type(kind) else {
                continue;
            };
            let size = match size {
                Value::Int(n) => u64::try_from(*n).ok(),
                Value::ServerError(_) => {
                    no_memory.push(infos.len());
                    None
                }
                _ => None,
            };
            infos.push(KvKeyInfo {
                key: KvKey::from_bytes(&key),
                kind,
                ttl_ms: ttl_ms(ttl),
                size,
            });
        }
        if !no_memory.is_empty() {
            self.fill_lengths(&mut infos, &no_memory).await?;
        }
        Ok(infos)
    }

    async fn fill_lengths(&mut self, infos: &mut [KvKeyInfo], at: &[usize]) -> DbResult<()> {
        let probes: Vec<(Vec<u8>, Option<&'static str>)> = at
            .iter()
            .map(|&i| {
                let key = infos[i].key.to_bytes().unwrap_or_default();
                (key, length_command(&infos[i].kind))
            })
            .collect();
        let values: Vec<Value> = self
            .conn
            .query(|mut c| async move {
                let mut pipe = redis::pipe();
                for (key, command) in &probes {
                    // 길이 명령이 없는 타입(모듈 타입)은 `EXISTS` 로 자리만 채운다.
                    pipe.cmd(command.unwrap_or("EXISTS")).arg(key);
                }
                pipe.ignore_errors().query_async(&mut c).await
            })
            .await?;
        for (&i, value) in at.iter().zip(values) {
            if length_command(&infos[i].kind).is_some()
                && let Value::Int(n) = value
            {
                infos[i].size = u64::try_from(n).ok();
            }
        }
        Ok(())
    }

    async fn info_of(&mut self, key: KvKey) -> DbResult<Option<KvKeyInfo>> {
        let key = key_bytes(&key)?;
        Ok(self.key_infos(vec![key]).await?.into_iter().next())
    }

    async fn read_page(&mut self, key: KvKey, range: KvRange) -> DbResult<KvValuePage> {
        let key = key_bytes(&key)?;
        let probe = key.clone();
        let kind: Value = self
            .conn
            .query(|mut c| async move { redis::cmd("TYPE").arg(&probe).query_async(&mut c).await })
            .await?;
        let count = range.count.max(1) as usize;
        let cursor = range.cursor;
        match key_type(&kind) {
            None => Ok(KvValuePage {
                value: KvValue::Missing,
                total: None,
                next_cursor: None,
            }),
            Some(KvType::String) => self.read_string(key, cursor, count).await,
            Some(KvType::Hash) => self.read_scanned(key, Scanned::Hash, cursor, count).await,
            Some(KvType::Set) => self.read_scanned(key, Scanned::Set, cursor, count).await,
            Some(KvType::List) => self.read_ranged(key, Ranged::List, cursor, count).await,
            Some(KvType::Zset) => self.read_ranged(key, Ranged::Zset, cursor, count).await,
            Some(KvType::Stream) => self.read_stream(key, cursor, count).await,
            Some(KvType::Json) => self.read_json(key, cursor, count).await,
            Some(KvType::Other(name)) => {
                Err(DbError::unsupported(&format!("reading {name} values")))
            }
        }
    }

    /// string: `GETRANGE` 창(바이트, 상한 [`PAGE_BUDGET`]). UTF-8 글자가 창 끝에서 잘리면 그 글자 앞에서 끊고 다음 쪽이 그
    /// 글자부터 읽는다.
    async fn read_string(
        &mut self,
        key: Vec<u8>,
        cursor: Option<String>,
        window: usize,
    ) -> DbResult<KvValuePage> {
        let offset = match cursor.as_deref() {
            None => 0,
            Some(text) => parse_index(text)?,
        };
        let window = window.clamp(1, PAGE_BUDGET) as u64;
        let end = offset
            .checked_add(window - 1)
            .ok_or_else(|| DbError::new(DbErrorKind::Invalid, "the cursor is out of range"))?;
        let (mut chunk, total): (Vec<u8>, u64) = self
            .conn
            .query(|mut c| async move {
                redis::pipe()
                    .cmd("GETRANGE")
                    .arg(&key)
                    .arg(offset)
                    .arg(end)
                    .cmd("STRLEN")
                    .arg(&key)
                    .query_async(&mut c)
                    .await
            })
            .await?;
        if let Err(e) = std::str::from_utf8(&chunk)
            && e.error_len().is_none()
            && e.valid_up_to() > 0
            && offset + (chunk.len() as u64) < total
        {
            chunk.truncate(e.valid_up_to());
        }
        let next = offset + chunk.len() as u64;
        Ok(KvValuePage {
            value: KvValue::String(bytes_value(&chunk)),
            total: Some(total),
            next_cursor: (next < total && !chunk.is_empty()).then(|| next.to_string()),
        })
    }

    /// hash · set: `HSCAN` · `SSCAN`. 원소 크기를 보며 묻는 수를 늘리고, 원소 수 `count` 나 예산에 닿으면 멈춘다.
    async fn read_scanned(
        &mut self,
        key: Vec<u8>,
        kind: Scanned,
        cursor: Option<String>,
        count: usize,
    ) -> DbResult<KvValuePage> {
        let mut scan_cursor = match cursor.as_deref() {
            None => 0,
            Some(text) => parse_cursor(text)?,
        };
        let total = self.length(&key, kind.length()).await?;
        let mut fields = Vec::new();
        let mut members = Vec::new();
        let mut budget = Budget::new(count);
        loop {
            let hint = budget.next_chunk();
            let probe = key.clone();
            let (next, raw): (u64, Vec<Vec<u8>>) = self
                .conn
                .query(|mut c| async move {
                    redis::cmd(kind.command())
                        .arg(&probe)
                        .arg(scan_cursor)
                        .arg("COUNT")
                        .arg(hint)
                        .query_async(&mut c)
                        .await
                })
                .await?;
            match kind {
                Scanned::Hash => {
                    for pair in raw.chunks(2) {
                        let [field, value] = pair else {
                            return Err(protocol("HSCAN returned an odd list"));
                        };
                        let field = KvField {
                            field: bytes_value(field),
                            value: bytes_value(value),
                        };
                        budget.add(json_len(&field));
                        fields.push(field);
                    }
                }
                Scanned::Set => {
                    for member in &raw {
                        let member = bytes_value(member);
                        budget.add(json_len(&member));
                        members.push(member);
                    }
                }
            }
            scan_cursor = next;
            if scan_cursor == 0 || budget.full() {
                break;
            }
        }
        Ok(KvValuePage {
            value: match kind {
                Scanned::Hash => KvValue::Hash(fields),
                Scanned::Set => KvValue::Set(members),
            },
            total,
            next_cursor: (scan_cursor != 0).then(|| scan_cursor.to_string()),
        })
    }

    /// list · zset: 번호 범위(`LRANGE` · `ZRANGE … WITHSCORES`). 쪽 커서는 다음 번호다.
    async fn read_ranged(
        &mut self,
        key: Vec<u8>,
        kind: Ranged,
        cursor: Option<String>,
        count: usize,
    ) -> DbResult<KvValuePage> {
        let mut start = match cursor.as_deref() {
            None => 0,
            Some(text) => parse_index(text)?,
        };
        let total = self.length(&key, kind.length()).await?;
        let mut items = Vec::new();
        let mut scored = Vec::new();
        let mut budget = Budget::new(count);
        let mut ended = false;
        loop {
            let hint = budget.next_chunk() as u64;
            let probe = key.clone();
            let stop = start
                .checked_add(hint - 1)
                .ok_or_else(|| DbError::new(DbErrorKind::Invalid, "the cursor is out of range"))?;
            let raw: Vec<Vec<u8>> = self
                .conn
                .query(|mut c| async move {
                    let mut cmd = redis::cmd(kind.command());
                    cmd.arg(&probe).arg(start).arg(stop);
                    if matches!(kind, Ranged::Zset) {
                        cmd.arg("WITHSCORES");
                    }
                    cmd.query_async(&mut c).await
                })
                .await?;
            let got = match kind {
                Ranged::List => {
                    for item in &raw {
                        let item = bytes_value(item);
                        budget.add(json_len(&item));
                        items.push(item);
                    }
                    raw.len() as u64
                }
                Ranged::Zset => {
                    for pair in raw.chunks(2) {
                        let [member, score] = pair else {
                            return Err(protocol("ZRANGE WITHSCORES returned an odd list"));
                        };
                        let scored_member = KvScored {
                            member: bytes_value(member),
                            score: String::from_utf8_lossy(score).into_owned(),
                        };
                        budget.add(json_len(&scored_member));
                        scored.push(scored_member);
                    }
                    raw.len() as u64 / 2
                }
            };
            start += got;
            if got < hint {
                ended = true;
                break;
            }
            if budget.full() {
                break;
            }
        }
        let ended = ended || total.is_some_and(|t| start >= t);
        Ok(KvValuePage {
            value: match kind {
                Ranged::List => KvValue::List(items),
                Ranged::Zset => KvValue::Zset(scored),
            },
            total,
            next_cursor: (!ended).then(|| start.to_string()),
        })
    }

    /// stream: `XRANGE <커서> + COUNT n`. 쪽 커서는 마지막 항목 id 의 배타 범위(`(<id>`, Redis 6.2+).
    async fn read_stream(
        &mut self,
        key: Vec<u8>,
        cursor: Option<String>,
        count: usize,
    ) -> DbResult<KvValuePage> {
        let total = self.length(&key, "XLEN").await?;
        let mut start = cursor.unwrap_or_else(|| "-".to_owned());
        let mut entries = Vec::new();
        let mut budget = Budget::new(count);
        let mut ended = false;
        loop {
            let hint = budget.next_chunk();
            let probe = key.clone();
            let from = start.clone();
            let raw: Value = self
                .conn
                .query(|mut c| async move {
                    redis::cmd("XRANGE")
                        .arg(&probe)
                        .arg(from)
                        .arg("+")
                        .arg("COUNT")
                        .arg(hint)
                        .query_async(&mut c)
                        .await
                })
                .await?;
            let Value::Array(items) = raw else {
                return Err(protocol("XRANGE did not return a list"));
            };
            let got = items.len();
            for item in items {
                let entry = stream_entry(item)?;
                budget.add(json_len(&entry));
                start = format!("({}", entry.id);
                entries.push(entry);
            }
            if got < hint {
                ended = true;
                break;
            }
            if budget.full() {
                break;
            }
        }
        Ok(KvValuePage {
            value: KvValue::Stream(entries),
            total,
            next_cursor: (!ended).then_some(start),
        })
    }

    /// RedisJSON: `JSON.GET`(문서 하나, 예산을 넘으면 앞부분). 모듈이 답하지 않으면 string 으로 읽는다. 문서는 쪽으로 나눌
    /// 길이 없어 통째로 받는다 — 키 하나 안의 읽기는 허용하는 규칙(14-m3-plan.md M3-09)이고, Host 의 일시 메모리는 응답 타임아웃 ·
    /// 취소가 묶는다.
    async fn read_json(
        &mut self,
        key: Vec<u8>,
        cursor: Option<String>,
        count: usize,
    ) -> DbResult<KvValuePage> {
        let probe = key.clone();
        let reply = self
            .conn
            .run(|mut c| async move {
                redis::cmd("JSON.GET")
                    .arg(&probe)
                    .query_async::<Vec<u8>>(&mut c)
                    .await
            })
            .await?;
        match reply {
            Ok(text) => Ok(KvValuePage {
                value: KvValue::Json(DbValue::json(
                    String::from_utf8_lossy(&text),
                    reply::VALUE_LIMIT,
                )),
                total: Some(text.len() as u64),
                next_cursor: None,
            }),
            Err(_) => self.read_string(key, cursor, count).await,
        }
    }

    async fn length(&mut self, key: &[u8], command: &'static str) -> DbResult<Option<u64>> {
        let key = key.to_vec();
        let n: i64 = self
            .conn
            .query(|mut c| async move { redis::cmd(command).arg(&key).query_async(&mut c).await })
            .await?;
        Ok(u64::try_from(n).ok())
    }

    async fn set_ttl_ms(&mut self, key: KvKey, ttl_ms: u64) -> DbResult<bool> {
        self.writable()?;
        let key = key_bytes(&key)?;
        let changed: i64 = self
            .conn
            .query(|mut c| async move {
                redis::cmd("PEXPIRE")
                    .arg(&key)
                    .arg(ttl_ms)
                    .query_async(&mut c)
                    .await
            })
            .await?;
        Ok(changed == 1)
    }

    async fn persist_key(&mut self, key: KvKey) -> DbResult<bool> {
        self.writable()?;
        let key = key_bytes(&key)?;
        let changed: i64 = self
            .conn
            .query(|mut c| async move { redis::cmd("PERSIST").arg(&key).query_async(&mut c).await })
            .await?;
        Ok(changed == 1)
    }

    async fn delete_keys(&mut self, keys: Vec<KvKey>) -> DbResult<u64> {
        self.writable()?;
        if keys.is_empty() {
            return Ok(0);
        }
        let keys = keys.iter().map(key_bytes).collect::<DbResult<Vec<_>>>()?;
        // `UNLINK` 은 큰 값의 메모리를 서버의 다른 스레드가 거둔다 — 큰 키를 지워도 서버가 멈추지 않는다.
        let removed: u64 = self
            .conn
            .query(|mut c| async move { redis::cmd("UNLINK").arg(&keys).query_async(&mut c).await })
            .await?;
        Ok(removed)
    }

    async fn rename_key(
        &mut self,
        from: KvKey,
        to: KvKey,
        overwrite: bool,
    ) -> DbResult<KvRenameOutcome> {
        self.writable()?;
        let from = key_bytes(&from)?;
        let to = key_bytes(&to)?;
        if overwrite {
            let _: Value = self
                .conn
                .query(|mut c| async move {
                    redis::cmd("RENAME")
                        .arg(&from)
                        .arg(&to)
                        .query_async(&mut c)
                        .await
                })
                .await?;
            return Ok(KvRenameOutcome::Renamed);
        }
        let renamed: i64 = self
            .conn
            .query(|mut c| async move {
                redis::cmd("RENAMENX")
                    .arg(&from)
                    .arg(&to)
                    .query_async(&mut c)
                    .await
            })
            .await?;
        Ok(if renamed == 1 {
            KvRenameOutcome::Renamed
        } else {
            KvRenameOutcome::TargetExists
        })
    }

    async fn run_command(&mut self, argv: Vec<String>) -> DbResult<KvReply> {
        let Some((name, args)) = argv.split_first() else {
            return Err(DbError::new(DbErrorKind::Invalid, "the command is empty"));
        };
        if let Some(probe) = console::size_probe(name, args) {
            self.refuse_if_too_large(probe).await?;
        }
        let name = name.clone();
        let args = args.to_vec();
        let result = self
            .conn
            .run(|mut c| async move {
                redis::cmd(&name)
                    .arg(&args)
                    .query_async::<Value>(&mut c)
                    .await
            })
            .await?;
        Ok(match result {
            Ok(value) => console::limited_reply(value),
            Err(error) => KvReply::Error(reply::server_reply_text(&error)),
        })
    }

    /// 키 하나를 통째로 읽는 명령의 크기를 먼저 묻는다(같은 연결, 분류 밖의 내부 명령). 상한을 넘으면 그 명령을 서버에 보내지
    /// 않고 거절한다. 묻기와 읽기 사이에 키가 커지는 경합은 받아들인다 — 그때는 응답 상한(`Truncated`)과 응답 타임아웃이 막는다.
    async fn refuse_if_too_large(&mut self, probe: console::SizeProbe) -> DbResult<()> {
        let commands = probe.commands.clone();
        let answers = self
            .conn
            .run(|mut c| async move {
                let mut pipe = redis::pipe();
                for argv in &commands {
                    let cmd = pipe.cmd(&argv[0]);
                    if argv[0] == "MEMORY" {
                        cmd.arg("USAGE");
                    }
                    cmd.arg(&argv[1..]);
                }
                // 답마다 오류를 따로 받는다 — 하나가 실패해도 나머지 답을 버리지 않는다.
                pipe.ignore_errors()
                    .query_async::<Vec<redis::RedisResult<Value>>>(&mut c)
                    .await
            })
            .await?;
        // 크기를 못 물으면(모듈 없음 · 권한) 막지 않는다 — 응답 상한이 남는다. 없는 키(`nil`)는 0 이다. 다른 타입(`WRONGTYPE`)도
        // 0 이다: `MGET` 은 다른 타입 키에 nil 로 답하므로, 그 답 하나로 묻기 전체를 버리면 섞인 타입 한 키로 사전 확인을 비켜
        // 간다(재리뷰 B). `SORT_RO` 는 타입마다 길이 명령 셋을 물어 맞는 하나만 센다.
        let Ok(answers) = answers else {
            return Ok(());
        };
        let mut size = 0_u64;
        for answer in answers {
            match answer {
                Ok(Value::Int(n)) => size = size.saturating_add(u64::try_from(n).unwrap_or(0)),
                Ok(Value::Nil) => {}
                Err(e) if e.code() == Some("WRONGTYPE") => {}
                _ => return Ok(()),
            }
        }
        if probe.too_large(size) {
            return Err(console::too_large());
        }
        Ok(())
    }
}

impl KvConnection for RedisKv {
    fn interrupter(&self) -> Arc<dyn Interrupt> {
        self.conn.interrupter()
    }

    fn scan(&mut self, request: KvScanRequest) -> BoxFuture<'_, DbResult<KvScanPage>> {
        Box::pin(self.scan_page(request))
    }

    fn key_info(&mut self, key: KvKey) -> BoxFuture<'_, DbResult<Option<KvKeyInfo>>> {
        Box::pin(self.info_of(key))
    }

    fn read(&mut self, key: KvKey, range: KvRange) -> BoxFuture<'_, DbResult<KvValuePage>> {
        Box::pin(self.read_page(key, range))
    }

    fn set_ttl(&mut self, key: KvKey, ttl_ms: u64) -> BoxFuture<'_, DbResult<bool>> {
        Box::pin(self.set_ttl_ms(key, ttl_ms))
    }

    fn persist(&mut self, key: KvKey) -> BoxFuture<'_, DbResult<bool>> {
        Box::pin(self.persist_key(key))
    }

    fn delete(&mut self, keys: Vec<KvKey>) -> BoxFuture<'_, DbResult<u64>> {
        Box::pin(self.delete_keys(keys))
    }

    fn rename(
        &mut self,
        from: KvKey,
        to: KvKey,
        overwrite: bool,
    ) -> BoxFuture<'_, DbResult<KvRenameOutcome>> {
        Box::pin(self.rename_key(from, to, overwrite))
    }

    fn command(&mut self, argv: Vec<String>) -> BoxFuture<'_, DbResult<KvReply>> {
        Box::pin(self.run_command(argv))
    }

    fn close(self: Box<Self>) -> BoxFuture<'static, DbResult<()>> {
        self.conn.close();
        Box::pin(std::future::ready(Ok(())))
    }
}

#[derive(Clone, Copy)]
enum Scanned {
    Hash,
    Set,
}

impl Scanned {
    fn command(self) -> &'static str {
        match self {
            Self::Hash => "HSCAN",
            Self::Set => "SSCAN",
        }
    }

    fn length(self) -> &'static str {
        match self {
            Self::Hash => "HLEN",
            Self::Set => "SCARD",
        }
    }
}

#[derive(Clone, Copy)]
enum Ranged {
    List,
    Zset,
}

impl Ranged {
    fn command(self) -> &'static str {
        match self {
            Self::List => "LRANGE",
            Self::Zset => "ZRANGE",
        }
    }

    fn length(self) -> &'static str {
        match self {
            Self::List => "LLEN",
            Self::Zset => "ZCARD",
        }
    }
}

/// 한 쪽의 예산: 원소 수 상한과 바이트 예산. 다음에 물을 원소 수는 지금까지의 평균 크기로 남은 예산에 맞춘다 — 큰 원소의
/// 컬렉션에서 한 번의 응답이 예산을 크게 넘지 않게.
struct Budget {
    count: usize,
    items: usize,
    bytes: usize,
}

impl Budget {
    fn new(count: usize) -> Self {
        Self {
            count,
            items: 0,
            bytes: 0,
        }
    }

    fn add(&mut self, bytes: usize) {
        self.items += 1;
        self.bytes += bytes;
    }

    fn full(&self) -> bool {
        self.items >= self.count || self.bytes >= PAGE_BUDGET
    }

    fn next_chunk(&self) -> usize {
        let left = self.count.saturating_sub(self.items).max(1);
        let by_size = match self.bytes.checked_div(self.items) {
            None => FIRST_CHUNK,
            Some(average) => (PAGE_BUDGET.saturating_sub(self.bytes) / average.max(1)).max(1),
        };
        left.min(by_size).min(MAX_CHUNK)
    }
}

/// 창으로 갈 JSON 의 바이트 수(원소 사이의 `,` 하나를 더한다). 쓰지 않고 센다.
fn json_len<T: serde::Serialize>(value: &T) -> usize {
    struct Count(usize);
    impl std::io::Write for Count {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0 += buf.len();
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut count = Count(1);
    // 직렬화는 실패하지 않는다(문자열 열쇠 · 유한한 값뿐). 실패하면 센 데까지다.
    let _ = serde_json::to_writer(&mut count, value);
    count.0
}

fn key_bytes(key: &KvKey) -> DbResult<Vec<u8>> {
    key.to_bytes()
        .ok_or_else(|| DbError::new(DbErrorKind::Invalid, "the key's hex form is malformed"))
}

fn parse_cursor(text: &str) -> DbResult<u64> {
    text.parse()
        .map_err(|_| DbError::new(DbErrorKind::Invalid, format!("bad cursor {text:?}")))
}

/// 번호 커서(string 바이트 위치 · list · zset 번호). Redis 의 번호는 i64 라 그 밖은 요청 오류다.
fn parse_index(text: &str) -> DbResult<u64> {
    let index = parse_cursor(text)?;
    if index > i64::MAX as u64 {
        return Err(DbError::new(
            DbErrorKind::Invalid,
            format!("cursor {text:?} is out of range"),
        ));
    }
    Ok(index)
}

fn protocol(what: &str) -> DbError {
    DbError::new(
        DbErrorKind::Server,
        format!("unexpected Redis reply: {what}"),
    )
}

/// `TYPE` 의 답. 키가 없으면(`none`) `None`.
fn key_type(value: &Value) -> Option<KvType> {
    let name = match value {
        Value::SimpleString(s) => s.as_str(),
        Value::Okay => "OK",
        _ => return None,
    };
    Some(match name {
        "none" => return None,
        "string" => KvType::String,
        "hash" => KvType::Hash,
        "list" => KvType::List,
        "set" => KvType::Set,
        "zset" => KvType::Zset,
        "stream" => KvType::Stream,
        "ReJSON-RL" => KvType::Json,
        other => KvType::Other(other.to_owned()),
    })
}

/// `PTTL` 의 답: 만료가 없거나(-1) 키가 없으면(-2) `None`.
fn ttl_ms(value: &Value) -> Option<u64> {
    match value {
        Value::Int(ms) => u64::try_from(*ms).ok(),
        _ => None,
    }
}

fn length_command(kind: &KvType) -> Option<&'static str> {
    match kind {
        KvType::String => Some("STRLEN"),
        KvType::Hash => Some("HLEN"),
        KvType::List => Some("LLEN"),
        KvType::Set => Some("SCARD"),
        KvType::Zset => Some("ZCARD"),
        KvType::Stream => Some("XLEN"),
        KvType::Json | KvType::Other(_) => None,
    }
}

/// `XRANGE` 항목 하나: `[id, [field, value, …]]`.
fn stream_entry(item: Value) -> DbResult<KvStreamEntry> {
    let Value::Array(mut parts) = item else {
        return Err(protocol("stream entry is not a list"));
    };
    if parts.len() != 2 {
        return Err(protocol("stream entry is not [id, fields]"));
    }
    let fields = parts.pop().expect("two parts");
    let id = match parts.pop().expect("two parts") {
        Value::BulkString(id) => String::from_utf8_lossy(&id).into_owned(),
        Value::SimpleString(id) => id,
        _ => return Err(protocol("stream id is not a string")),
    };
    let fields = match fields {
        Value::Array(items) => items,
        Value::Nil => Vec::new(),
        _ => return Err(protocol("stream fields are not a list")),
    };
    let mut pairs = Vec::with_capacity(fields.len() / 2);
    let mut iter = fields.into_iter();
    while let (Some(field), Some(value)) = (iter.next(), iter.next()) {
        pairs.push(KvField {
            field: bulk(field),
            value: bulk(value),
        });
    }
    Ok(KvStreamEntry { id, fields: pairs })
}

fn bulk(value: Value) -> DbValue {
    match value {
        Value::BulkString(bytes) => bytes_value(&bytes),
        Value::SimpleString(text) => DbValue::text(text, reply::VALUE_LIMIT),
        other => match reply::reply(other) {
            KvReply::Bulk(v) | KvReply::Int(v) => v,
            _ => DbValue::Null,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budgets_stop_at_the_count_or_the_bytes() {
        let mut budget = Budget::new(10);
        assert_eq!(budget.next_chunk(), FIRST_CHUNK);
        budget.add(1);
        assert_eq!(budget.next_chunk(), 9);
        for _ in 0..8 {
            budget.add(1);
        }
        assert!(!budget.full());
        budget.add(1);
        assert!(budget.full());

        let mut big = Budget::new(100_000);
        assert_eq!(big.next_chunk(), FIRST_CHUNK);
        big.add(1024 * 1024);
        // 평균 1 MiB 원소면 남은 3 MiB 에 셋이다.
        assert_eq!(big.next_chunk(), 3);
        for _ in 0..3 {
            big.add(1024 * 1024);
        }
        assert!(big.full());

        let mut small = Budget::new(100_000);
        small.add(10);
        assert_eq!(small.next_chunk(), MAX_CHUNK);
    }

    #[test]
    fn types_ttls_and_stream_entries_are_read() {
        assert_eq!(
            key_type(&Value::SimpleString("ReJSON-RL".into())),
            Some(KvType::Json)
        );
        assert_eq!(key_type(&Value::SimpleString("none".into())), None);
        assert_eq!(
            key_type(&Value::SimpleString("TSDB-TYPE".into())),
            Some(KvType::Other("TSDB-TYPE".into()))
        );
        assert_eq!(ttl_ms(&Value::Int(-1)), None);
        assert_eq!(ttl_ms(&Value::Int(-2)), None);
        assert_eq!(ttl_ms(&Value::Int(1500)), Some(1500));

        let entry = stream_entry(Value::Array(vec![
            Value::BulkString(b"1-0".to_vec()),
            Value::Array(vec![
                Value::BulkString(b"f".to_vec()),
                Value::BulkString(b"v".to_vec()),
            ]),
        ]))
        .unwrap();
        assert_eq!(entry.id, "1-0");
        assert_eq!(
            entry.fields,
            vec![KvField {
                field: DbValue::Text("f".into()),
                value: DbValue::Text("v".into())
            }]
        );
        assert!(parse_cursor("12").is_ok() && parse_cursor("x").is_err());
    }
}
