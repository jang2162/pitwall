//! 가짜 KV 드라이버(14-m3-plan.md M3-09, §3 의 「가짜 KV 드라이버」 행) — 관문(M3-22)이 Redis 없이 「서버에 닿지 않았다」 를
//! 시험한다.
//!
//! 키 공간은 DB 번호마다 메모리에 있고 드라이버 전체가 함께 쓴다(서버처럼 — 같은 DB 번호의 두 연결은 같은 키를 본다). 연결마다
//! 따로인 것은 DB 번호 · read-only · 인터럽트다. 불린 메서드는 연결 번호(`connect` 순서대로 0 부터)와 함께 차례대로 기록한다
//! ([`FakeKvCall`], [`FakeKvDriver::calls_of`]). `command` 는 `PING` · `GET` · `SET` · `DEL` · `EXISTS` · `TYPE` · `TTL` 만
//! 흉내 내고 나머지는 오류 응답이다 — 분류는 부르는 쪽이 먼저 한다는 계약대로 여기서도 분류하지 않는다.
//! [`FakeKvDriver::hang_on`] 으로 고른 명령은 그 연결의 인터럽트가 올 때까지 끝나지 않는다(취소 시험).
//! `command` 는 실제 드라이버처럼 read-only 연결에서도 쓴다 — read-only 판정은 관문(M3-22)의 것이다. 관문 시험은 「키가 안
//! 생겼다」 가 아니라 「명령이 서버에 닿지 않았다」([`FakeKvDriver::commands`])로 판정한다(가짜의 거절에 기대면 관문이 빠져도
//! 통과한다). 전용 메서드(`set_ttl` · `persist` · `delete` · `rename`)의 read-only 거절은 실제 드라이버와 같다.
//!
//! ```
//! # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
//! use pw_db_driver::fake::kv::{FakeKvCall, FakeKvDriver, FakeKvValue};
//! use pw_db_driver::{DbConnection, DbDriver};
//! use pw_db_types::{KvKey, KvReply};
//!
//! let driver = FakeKvDriver::new();
//! driver.insert(0, b"greeting", FakeKvValue::String(b"hi".to_vec()));
//! let DbConnection::Kv(mut conn) = driver.connect(FakeKvDriver::spec(0)).await.unwrap().connection else {
//!     unreachable!()
//! };
//! let reply = conn.command(vec!["GET".into(), "greeting".into()]).await.unwrap();
//! assert!(matches!(reply, KvReply::Bulk(_)));
//! assert_eq!(driver.commands(), vec![vec!["GET".to_owned(), "greeting".to_owned()]]);
//! assert!(driver.calls_of(0).contains(&FakeKvCall::Connect { db: 0 }));
//! # });
//! ```

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use pw_db_types::{
    Capabilities, CellLimit, ConnectionSpec, DbError, DbErrorKind, DbValue, DriverKind, KvField,
    KvKey, KvKeyInfo, KvRange, KvRenameOutcome, KvReply, KvScanPage, KvScanRequest, KvScored,
    KvType, KvValue, KvValuePage, ServerInfo, SqlDialect, SslMode, Target, TlsSettings, TlsStatus,
};
use tokio::sync::Notify;

use crate::driver::{
    BoxFuture, Connected, DbConnection, DbDriver, DbResult, Interrupt, KvConnection,
};

/// 가짜 KV 드라이버가 기록하는 호출.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FakeKvCall {
    Connect {
        db: u32,
    },
    Scan {
        pattern: Option<String>,
        cursor: Option<String>,
    },
    KeyInfo(KvKey),
    Read(KvKey),
    SetTtl(KvKey, u64),
    Persist(KvKey),
    Delete(Vec<KvKey>),
    Rename {
        from: KvKey,
        to: KvKey,
        overwrite: bool,
    },
    /// 서버에 닿은 명령(분류를 지난 것만 와야 한다).
    Command(Vec<String>),
    /// 인터럽트. 그 연결에서 도는 요청이 없으면 기록만 한다.
    Interrupt,
    Close,
}

/// 값. stream · JSON 은 흉내 내지 않는다.
#[derive(Debug, Clone, PartialEq)]
pub enum FakeKvValue {
    String(Vec<u8>),
    Hash(Vec<(Vec<u8>, Vec<u8>)>),
    List(Vec<Vec<u8>>),
    Set(Vec<Vec<u8>>),
    Zset(Vec<(Vec<u8>, f64)>),
}

impl FakeKvValue {
    fn kind(&self) -> KvType {
        match self {
            Self::String(_) => KvType::String,
            Self::Hash(_) => KvType::Hash,
            Self::List(_) => KvType::List,
            Self::Set(_) => KvType::Set,
            Self::Zset(_) => KvType::Zset,
        }
    }

    fn len(&self) -> usize {
        match self {
            Self::String(b) => b.len(),
            Self::Hash(v) => v.len(),
            Self::List(v) | Self::Set(v) => v.len(),
            Self::Zset(v) => v.len(),
        }
    }
}

#[derive(Debug, Clone)]
struct Entry {
    value: FakeKvValue,
    ttl_ms: Option<u64>,
}

type Keyspace = BTreeMap<Vec<u8>, Entry>;

#[derive(Default)]
struct KvState {
    dbs: Mutex<HashMap<u32, Keyspace>>,
    calls: Mutex<Vec<(u32, FakeKvCall)>>,
    next_connection: AtomicU32,
    connect_error: Mutex<Option<DbError>>,
    /// 인터럽트가 올 때까지 끝나지 않는 명령 이름(대문자).
    hanging: Mutex<Vec<String>>,
}

impl KvState {
    fn record(&self, connection: u32, call: FakeKvCall) {
        lock(&self.calls).push((connection, call));
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 가짜 KV 드라이버. 복제하면 같은 키 공간 · 기록을 본다.
#[derive(Clone, Default)]
pub struct FakeKvDriver {
    state: Arc<KvState>,
}

impl FakeKvDriver {
    pub fn new() -> Self {
        Self::default()
    }

    /// DB 번호 `db` 의 연결 설정.
    pub fn spec(db: u32) -> ConnectionSpec {
        let mut spec = ConnectionSpec::new(
            DriverKind::Redis.id(),
            Target::Network {
                host: "fake".into(),
                port: 6379,
                connect_to: None,
            },
        );
        spec.tls = TlsSettings::mode(SslMode::Disable);
        spec.database = Some(db.to_string());
        spec
    }

    /// 키를 심는다(만료 없음).
    pub fn insert(&self, db: u32, key: &[u8], value: FakeKvValue) {
        self.insert_with_ttl(db, key, value, None);
    }

    pub fn insert_with_ttl(&self, db: u32, key: &[u8], value: FakeKvValue, ttl_ms: Option<u64>) {
        lock(&self.state.dbs)
            .entry(db)
            .or_default()
            .insert(key.to_vec(), Entry { value, ttl_ms });
    }

    /// 지금 값.
    pub fn get(&self, db: u32, key: &[u8]) -> Option<FakeKvValue> {
        lock(&self.state.dbs)
            .get(&db)
            .and_then(|space| space.get(key))
            .map(|entry| entry.value.clone())
    }

    /// `connect` 가 이 오류로 끝난다.
    pub fn fail_connect(&self, error: DbError) {
        *lock(&self.state.connect_error) = Some(error);
    }

    /// 이 이름의 명령은 그 연결의 인터럽트가 올 때까지 끝나지 않는다(대소문자 무시).
    pub fn hang_on(&self, command: &str) {
        lock(&self.state.hanging).push(command.to_ascii_uppercase());
    }

    /// 지금까지 불린 메서드(모든 연결).
    pub fn calls(&self) -> Vec<FakeKvCall> {
        lock(&self.state.calls)
            .iter()
            .map(|(_, c)| c.clone())
            .collect()
    }

    /// 연결 하나(`connect` 순서대로 0 부터)가 부른 메서드.
    pub fn calls_of(&self, connection: u32) -> Vec<FakeKvCall> {
        lock(&self.state.calls)
            .iter()
            .filter(|(id, _)| *id == connection)
            .map(|(_, c)| c.clone())
            .collect()
    }

    /// 서버에 닿은 명령의 argv(모든 연결, 순서대로).
    pub fn commands(&self) -> Vec<Vec<String>> {
        self.calls()
            .into_iter()
            .filter_map(|c| match c {
                FakeKvCall::Command(argv) => Some(argv),
                _ => None,
            })
            .collect()
    }

    fn with_db<T>(&self, db: u32, f: impl FnOnce(&mut Keyspace) -> T) -> T {
        f(lock(&self.state.dbs).entry(db).or_default())
    }
}

impl DbDriver for FakeKvDriver {
    fn id(&self) -> &str {
        "fake-kv"
    }

    fn dialect(&self) -> Option<SqlDialect> {
        None
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            sql: false,
            kv: true,
            server_cursor: false,
            cancel: true,
            transactions: false,
            savepoints: false,
            read_only_transaction: false,
            explain: false,
            metadata: false,
            schemas: false,
            multiple_databases: true,
            ssl_modes: vec![SslMode::Disable, SslMode::Require, SslMode::VerifyFull],
        }
    }

    fn connect(&self, spec: ConnectionSpec) -> BoxFuture<'_, DbResult<Connected>> {
        Box::pin(async move {
            let id = self.state.next_connection.fetch_add(1, Ordering::SeqCst);
            let db = match spec.database.as_deref() {
                None | Some("") => 0,
                Some(text) => text.parse().map_err(|_| {
                    DbError::new(DbErrorKind::Invalid, format!("bad database {text:?}"))
                })?,
            };
            self.state.record(id, FakeKvCall::Connect { db });
            if let Some(error) = lock(&self.state.connect_error).clone() {
                return Err(error);
            }
            let connection = FakeKvConnection {
                driver: self.clone(),
                session: Arc::new(Session {
                    id,
                    in_flight: AtomicBool::new(false),
                    interrupted: AtomicBool::new(false),
                    wake: Notify::new(),
                }),
                db,
                read_only: spec.read_only,
            };
            Ok(Connected {
                connection: DbConnection::Kv(Box::new(connection)),
                server: ServerInfo {
                    version: Some("fake-kv 1.0".into()),
                    tls: TlsStatus::Plaintext,
                },
            })
        })
    }
}

/// 연결 하나의 인터럽트 상태. 그 연결의 [`FakeKvInterrupt`] 와만 함께 쓴다.
struct Session {
    id: u32,
    in_flight: AtomicBool,
    interrupted: AtomicBool,
    wake: Notify,
}

struct FakeKvConnection {
    driver: FakeKvDriver,
    session: Arc<Session>,
    db: u32,
    read_only: bool,
}

impl FakeKvConnection {
    fn record(&self, call: FakeKvCall) {
        self.driver.state.record(self.session.id, call);
    }

    fn writable(&self) -> DbResult<()> {
        if self.read_only {
            Err(DbError::new(DbErrorKind::ReadOnly, "read-only connection"))
        } else {
            Ok(())
        }
    }

    fn space<T>(&self, f: impl FnOnce(&mut Keyspace) -> T) -> T {
        self.driver.with_db(self.db, f)
    }

    /// 인터럽트가 올 때까지 기다린다.
    async fn hang(&self) -> DbResult<KvReply> {
        let session = &self.session;
        let notified = session.wake.notified();
        let mut notified = std::pin::pin!(notified);
        notified.as_mut().enable();
        session.interrupted.store(false, Ordering::SeqCst);
        session.in_flight.store(true, Ordering::SeqCst);
        while !session.interrupted.load(Ordering::SeqCst) {
            notified.as_mut().await;
            notified.set(session.wake.notified());
            notified.as_mut().enable();
        }
        session.in_flight.store(false, Ordering::SeqCst);
        Err(DbError::cancelled())
    }

    fn command_reply(&self, argv: &[String]) -> KvReply {
        let name = argv[0].to_ascii_uppercase();
        let args = &argv[1..];
        let text = |b: &[u8]| DbValue::text(String::from_utf8_lossy(b), CellLimit::DEFAULT);
        match (name.as_str(), args) {
            ("PING", []) => KvReply::Status("PONG".into()),
            ("GET", [key]) => self.space(|s| match s.get(key.as_bytes()) {
                None => KvReply::Nil,
                Some(Entry {
                    value: FakeKvValue::String(b),
                    ..
                }) => KvReply::Bulk(text(b)),
                Some(_) => KvReply::Error("WRONGTYPE (fake)".into()),
            }),
            ("SET", [key, value]) => {
                self.space(|s| {
                    s.insert(
                        key.as_bytes().to_vec(),
                        Entry {
                            value: FakeKvValue::String(value.as_bytes().to_vec()),
                            ttl_ms: None,
                        },
                    )
                });
                KvReply::Status("OK".into())
            }
            ("DEL", keys) if !keys.is_empty() => {
                let removed = self.space(|s| {
                    keys.iter()
                        .filter(|k| s.remove(k.as_bytes()).is_some())
                        .count()
                });
                KvReply::Int(DbValue::int(removed as i64))
            }
            ("EXISTS", keys) if !keys.is_empty() => {
                let found =
                    self.space(|s| keys.iter().filter(|k| s.contains_key(k.as_bytes())).count());
                KvReply::Int(DbValue::int(found as i64))
            }
            ("TYPE", [key]) => KvReply::Status(
                self.space(|s| s.get(key.as_bytes()).map(|e| type_name(&e.value.kind())))
                    .unwrap_or("none")
                    .to_owned(),
            ),
            ("TTL", [key]) => {
                let ttl = self.space(|s| {
                    s.get(key.as_bytes())
                        .map_or(-2, |e| e.ttl_ms.map_or(-1, |ms| (ms / 1000) as i64))
                });
                KvReply::Int(DbValue::int(ttl))
            }
            _ => KvReply::Error(format!("ERR unknown command '{}' (fake)", argv[0])),
        }
    }

    fn info(&self, key: &[u8]) -> Option<KvKeyInfo> {
        self.space(|s| {
            s.get(key).map(|entry| KvKeyInfo {
                key: KvKey::from_bytes(key),
                kind: entry.value.kind(),
                ttl_ms: entry.ttl_ms,
                size: Some(entry.value.len() as u64),
            })
        })
    }
}

impl KvConnection for FakeKvConnection {
    fn interrupter(&self) -> Arc<dyn Interrupt> {
        Arc::new(FakeKvInterrupt {
            driver: self.driver.clone(),
            session: self.session.clone(),
        })
    }

    fn scan(&mut self, request: KvScanRequest) -> BoxFuture<'_, DbResult<KvScanPage>> {
        Box::pin(async move {
            self.record(FakeKvCall::Scan {
                pattern: request.pattern.clone(),
                cursor: request.cursor.clone(),
            });
            let start = parse_cursor(request.cursor.as_deref())?;
            let count = request.count.max(1) as usize;
            let all: Vec<Vec<u8>> = self.space(|s| s.keys().cloned().collect());
            let end = (start + count).min(all.len());
            let keys = all[start.min(end)..end]
                .iter()
                .filter(|k| {
                    request
                        .pattern
                        .as_deref()
                        .is_none_or(|p| glob(p.as_bytes(), k))
                })
                .filter_map(|k| self.info(k))
                .collect();
            Ok(KvScanPage {
                keys,
                next_cursor: (end < all.len()).then(|| end.to_string()),
            })
        })
    }

    fn key_info(&mut self, key: KvKey) -> BoxFuture<'_, DbResult<Option<KvKeyInfo>>> {
        Box::pin(async move {
            self.record(FakeKvCall::KeyInfo(key.clone()));
            Ok(self.info(&bytes(&key)?))
        })
    }

    fn read(&mut self, key: KvKey, range: KvRange) -> BoxFuture<'_, DbResult<KvValuePage>> {
        Box::pin(async move {
            self.record(FakeKvCall::Read(key.clone()));
            let start = parse_cursor(range.cursor.as_deref())?;
            let count = range.count.max(1) as usize;
            let key = bytes(&key)?;
            let Some(value) = self.space(|s| s.get(&key).map(|e| e.value.clone())) else {
                return Ok(KvValuePage {
                    value: KvValue::Missing,
                    total: None,
                    next_cursor: None,
                });
            };
            let total = value.len();
            let end = (start + count).min(total);
            let text = |b: &[u8]| DbValue::text(String::from_utf8_lossy(b), CellLimit::DEFAULT);
            let value = match value {
                FakeKvValue::String(b) => KvValue::String(text(&b[start.min(end)..end])),
                FakeKvValue::Hash(v) => KvValue::Hash(
                    v[start.min(end)..end]
                        .iter()
                        .map(|(f, x)| KvField {
                            field: text(f),
                            value: text(x),
                        })
                        .collect(),
                ),
                FakeKvValue::List(v) => {
                    KvValue::List(v[start.min(end)..end].iter().map(|x| text(x)).collect())
                }
                FakeKvValue::Set(v) => {
                    KvValue::Set(v[start.min(end)..end].iter().map(|x| text(x)).collect())
                }
                FakeKvValue::Zset(v) => KvValue::Zset(
                    v[start.min(end)..end]
                        .iter()
                        .map(|(m, score)| KvScored {
                            member: text(m),
                            score: score.to_string(),
                        })
                        .collect(),
                ),
            };
            Ok(KvValuePage {
                value,
                total: Some(total as u64),
                next_cursor: (end < total).then(|| end.to_string()),
            })
        })
    }

    fn set_ttl(&mut self, key: KvKey, ttl_ms: u64) -> BoxFuture<'_, DbResult<bool>> {
        Box::pin(async move {
            self.record(FakeKvCall::SetTtl(key.clone(), ttl_ms));
            self.writable()?;
            let key = bytes(&key)?;
            Ok(self.space(|s| s.get_mut(&key).map(|e| e.ttl_ms = Some(ttl_ms)).is_some()))
        })
    }

    fn persist(&mut self, key: KvKey) -> BoxFuture<'_, DbResult<bool>> {
        Box::pin(async move {
            self.record(FakeKvCall::Persist(key.clone()));
            self.writable()?;
            let key = bytes(&key)?;
            Ok(self.space(|s| s.get_mut(&key).and_then(|e| e.ttl_ms.take()).is_some()))
        })
    }

    fn delete(&mut self, keys: Vec<KvKey>) -> BoxFuture<'_, DbResult<u64>> {
        Box::pin(async move {
            self.record(FakeKvCall::Delete(keys.clone()));
            self.writable()?;
            let keys = keys.iter().map(bytes).collect::<DbResult<Vec<_>>>()?;
            Ok(self.space(|s| keys.iter().filter(|k| s.remove(*k).is_some()).count()) as u64)
        })
    }

    fn rename(
        &mut self,
        from: KvKey,
        to: KvKey,
        overwrite: bool,
    ) -> BoxFuture<'_, DbResult<KvRenameOutcome>> {
        Box::pin(async move {
            self.record(FakeKvCall::Rename {
                from: from.clone(),
                to: to.clone(),
                overwrite,
            });
            self.writable()?;
            let (from, to) = (bytes(&from)?, bytes(&to)?);
            self.space(|s| {
                if !s.contains_key(&from) {
                    return Err(
                        DbError::new(DbErrorKind::Server, "ERR no such key").with_code("ERR")
                    );
                }
                if !overwrite && s.contains_key(&to) {
                    return Ok(KvRenameOutcome::TargetExists);
                }
                let entry = s.remove(&from).expect("checked");
                s.insert(to, entry);
                Ok(KvRenameOutcome::Renamed)
            })
        })
    }

    fn command(&mut self, argv: Vec<String>) -> BoxFuture<'_, DbResult<KvReply>> {
        Box::pin(async move {
            if argv.is_empty() {
                return Err(DbError::new(DbErrorKind::Invalid, "the command is empty"));
            }
            self.record(FakeKvCall::Command(argv.clone()));
            let name = argv[0].to_ascii_uppercase();
            if lock(&self.driver.state.hanging).contains(&name) {
                return self.hang().await;
            }
            Ok(self.command_reply(&argv))
        })
    }

    fn close(self: Box<Self>) -> BoxFuture<'static, DbResult<()>> {
        self.record(FakeKvCall::Close);
        Box::pin(std::future::ready(Ok(())))
    }
}

struct FakeKvInterrupt {
    driver: FakeKvDriver,
    session: Arc<Session>,
}

impl Interrupt for FakeKvInterrupt {
    fn interrupt(&self) -> BoxFuture<'_, DbResult<()>> {
        Box::pin(async move {
            let session = &self.session;
            self.driver.state.record(session.id, FakeKvCall::Interrupt);
            if session.in_flight.load(Ordering::SeqCst) {
                session.interrupted.store(true, Ordering::SeqCst);
                session.wake.notify_waiters();
            }
            Ok(())
        })
    }
}

fn bytes(key: &KvKey) -> DbResult<Vec<u8>> {
    key.to_bytes()
        .ok_or_else(|| DbError::new(DbErrorKind::Invalid, "malformed hex key"))
}

fn parse_cursor(cursor: Option<&str>) -> DbResult<usize> {
    cursor.map_or(Ok(0), |text| {
        text.parse()
            .map_err(|_| DbError::new(DbErrorKind::Invalid, format!("bad cursor {text:?}")))
    })
}

fn type_name(kind: &KvType) -> &'static str {
    match kind {
        KvType::String => "string",
        KvType::Hash => "hash",
        KvType::List => "list",
        KvType::Set => "set",
        KvType::Zset => "zset",
        KvType::Stream => "stream",
        KvType::Json => "ReJSON-RL",
        KvType::Other(_) => "other",
    }
}

/// `*` · `?` 만 아는 glob(`SCAN MATCH` 의 흉내).
fn glob(pattern: &[u8], text: &[u8]) -> bool {
    match (pattern.first(), text.first()) {
        (None, None) => true,
        (Some(b'*'), _) => {
            glob(&pattern[1..], text) || (!text.is_empty() && glob(pattern, &text[1..]))
        }
        (Some(b'?'), Some(_)) => glob(&pattern[1..], &text[1..]),
        (Some(p), Some(t)) if p == t => glob(&pattern[1..], &text[1..]),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    async fn open(driver: &FakeKvDriver, db: u32) -> Box<dyn KvConnection> {
        match driver
            .connect(FakeKvDriver::spec(db))
            .await
            .unwrap()
            .connection
        {
            DbConnection::Kv(conn) => conn,
            DbConnection::Sql(_) => unreachable!(),
        }
    }

    fn argv(text: &str) -> Vec<String> {
        text.split_whitespace().map(str::to_owned).collect()
    }

    /// 호출 기록은 연결 번호를 싣고, DB 번호가 다른 연결은 서로의 키를 보지 않으며, 같은 번호는 같은 키를 본다.
    #[tokio::test]
    async fn calls_carry_the_connection_number_and_databases_are_apart() {
        let driver = FakeKvDriver::new();
        assert_eq!(driver.capabilities().validate(), Ok(()));
        let mut zero = open(&driver, 0).await;
        let mut two = open(&driver, 2).await;
        let mut zero_again = open(&driver, 0).await;
        zero.command(argv("SET a 1")).await.unwrap();
        assert_eq!(two.command(argv("GET a")).await.unwrap(), KvReply::Nil);
        assert!(matches!(
            zero_again.command(argv("GET a")).await.unwrap(),
            KvReply::Bulk(_)
        ));
        assert_eq!(
            driver.calls_of(0),
            vec![
                FakeKvCall::Connect { db: 0 },
                FakeKvCall::Command(argv("SET a 1"))
            ]
        );
        assert_eq!(
            driver.calls_of(1),
            vec![
                FakeKvCall::Connect { db: 2 },
                FakeKvCall::Command(argv("GET a"))
            ]
        );
        assert_eq!(driver.commands().len(), 3);
        assert!(matches!(
            zero.command(argv("FLUSHALL")).await.unwrap(),
            KvReply::Error(_)
        ));
    }

    /// 인터럽트는 그 연결의 도는 요청만 멈춘다 — 다른 연결의 인터럽트는 닿지 않는다.
    #[tokio::test]
    async fn interrupts_belong_to_their_connection() {
        let driver = FakeKvDriver::new();
        driver.hang_on("blpop");
        let mut first = open(&driver, 0).await;
        let second = open(&driver, 0).await;
        let mine = first.interrupter();
        let other = second.interrupter();
        let stopper = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            other.interrupt().await.unwrap();
            tokio::time::sleep(Duration::from_millis(100)).await;
            mine.interrupt().await.unwrap();
        });
        let started = std::time::Instant::now();
        let error = first.command(argv("BLPOP q 0")).await.unwrap_err();
        stopper.await.unwrap();
        assert_eq!(error.kind, DbErrorKind::Cancelled);
        assert!(
            started.elapsed() >= Duration::from_millis(140),
            "the other connection's interrupt stopped it"
        );
        assert_eq!(
            driver.calls_of(1),
            vec![FakeKvCall::Connect { db: 0 }, FakeKvCall::Interrupt]
        );
    }

    #[tokio::test]
    async fn rename_scan_read_and_read_only_follow_redis() {
        let driver = FakeKvDriver::new();
        for i in 0..5 {
            driver.insert(
                3,
                format!("k{i}").as_bytes(),
                FakeKvValue::List((0..7).map(|n| vec![b'0' + n]).collect()),
            );
        }
        driver.insert(3, b"other", FakeKvValue::String(b"x".to_vec()));
        let mut conn = open(&driver, 3).await;
        let outcome = conn
            .rename(KvKey::Text("k0".into()), KvKey::Text("k1".into()), false)
            .await
            .unwrap();
        assert_eq!(outcome, KvRenameOutcome::TargetExists);
        assert!(driver.get(3, b"k0").is_some());
        let page = conn
            .scan(KvScanRequest {
                pattern: Some("k*".into()),
                cursor: None,
                count: 100,
            })
            .await
            .unwrap();
        assert_eq!(page.keys.len(), 5);
        assert_eq!(page.next_cursor, None);
        let first = conn
            .read(
                KvKey::Text("k2".into()),
                KvRange {
                    cursor: None,
                    count: 3,
                },
            )
            .await
            .unwrap();
        assert_eq!(
            (first.total, first.next_cursor.as_deref()),
            (Some(7), Some("3"))
        );

        let mut spec = FakeKvDriver::spec(3);
        spec.read_only = true;
        let DbConnection::Kv(mut viewer) = driver.connect(spec).await.unwrap().connection else {
            unreachable!()
        };
        let refused = viewer
            .delete(vec![KvKey::Text("k2".into())])
            .await
            .unwrap_err();
        assert_eq!(refused.kind, DbErrorKind::ReadOnly);
        assert!(driver.get(3, b"k2").is_some());
        // `command` 는 read-only 를 보지 않는다(실제 드라이버와 같다 — 관문의 몫).
        viewer
            .command(vec!["SET".into(), "written".into(), "1".into()])
            .await
            .unwrap();
        assert!(driver.get(3, b"written").is_some());
        assert!(glob(b"a*c?", b"abbbcd") && !glob(b"a*c", b"abd"));
    }
}
