//! Redis 드라이버(14-m3-plan.md M3-09, 결정 M3-D10 · M3-D15): `KvConnection` 의 구현.
//!
//! - **연결**: `redis` 크레이트는 기능 `tokio-comp` 만 켠다 — 크레이트 자체의 연결 경로(`Client` · `ConnectionManager` 의 자동
//!   재연결)와 TLS 기능은 쓰지 않는다(14 §3). Pitwall 이 TCP(또는 터널의 로컬 포트) → `tokio-rustls`(`ring`) +
//!   `rustls-platform-verifier` 스트림을 만들어 `MultiplexedConnection::new_with_config` 에 넘긴다([`tls`] · [`connection`]).
//!   `sslmode` 는 [`SSL_MODES`] 셋만 받는다.
//! - **DB 번호마다 연결 하나**: 연결 설정의 `database`(`"0"` …)로 핸드셰이크에서 고르고 `SELECT` 로 바꾸지 않는다 — 콘솔과 키
//!   브라우저가 같은 연결을 함께 쓰다 서로의 DB 를 바꾸지 않게.
//! - **응답 타임아웃**(기본 [`DEFAULT_RESPONSE_TIMEOUT`], 설정 `response_timeout_ms`)과 **취소**: 진행 중 요청을 버리고 그
//!   연결을 다시 만든다([`connection`] 머리 주석).
//! - **명령 분류는 여기 없다** — 부르는 쪽(관문 M3-22)이 `pw_db_sql::kv_command` 로 먼저 한다.
//!
//! 실 DB 시험은 `#[cfg(pw_live_redis)]` 를 달고 이름에 `live_redis` 를 넣는다(`pw_db_driver::live` 머리 주석). 하한은
//! `scripts/db-live.toml` 의 `redis`.

mod browse;
mod connection;
mod console;
mod reply;
mod tls;

#[cfg(all(test, pw_live_redis))]
mod live_tests;

use std::time::Duration;

use pw_db_driver::{BoxFuture, Connected, DbConnection, DbDriver, DbResult, TlsPlan};
use pw_db_types::{
    Capabilities, ConnectionSpec, DbError, DbErrorKind, DriverKind, ServerInfo, SqlDialect,
    SslMode, Target,
};
use redis::RedisConnectionInfo;

use crate::browse::RedisKv;
use crate::connection::{Connection, Endpoint};

/// 이 드라이버가 받는 `sslmode`(결정 M3-D10). `allow` · `prefer` 는 조용히 평문으로 떨어질 수 있고, `verify-ca` 는
/// OS 검증기에 「이름만 빼고」 가 없어 받지 않는다.
pub const SSL_MODES: [SslMode; 3] = [SslMode::Disable, SslMode::Require, SslMode::VerifyFull];

/// 응답 하나를 기다리는 기본 상한(결정 M3-D15).
pub const DEFAULT_RESPONSE_TIMEOUT: Duration = Duration::from_secs(30);

/// 값 읽기 한 쪽의 예산(바이트). 값 하나가 이보다 크면 앞부분만 싣는다.
pub const PAGE_BUDGET: usize = 4 * 1024 * 1024;

/// 키 스캔 한 묶음의 키 수 상한 — 콘솔 스캔의 `COUNT` 상한(`pw_db_sql::kv_command::MAX_SCAN_COUNT`)과 같은 값이다.
pub const MAX_SCAN_COUNT: u32 = 10_000;

/// 콘솔 명령 응답의 맨 위 원소 수 상한. 넘으면 앞부분만 싣고 `KvReply::Truncated` 로 감싼다(바이트 상한은 [`PAGE_BUDGET`]).
pub const MAX_REPLY_ELEMENTS: u64 = 10_000;

#[derive(Debug, Default)]
pub struct RedisDriver;

impl RedisDriver {
    pub fn new() -> Self {
        Self
    }
}

impl DbDriver for RedisDriver {
    fn id(&self) -> &str {
        DriverKind::Redis.id()
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
            ssl_modes: SSL_MODES.to_vec(),
        }
    }

    fn connect(&self, spec: ConnectionSpec) -> BoxFuture<'_, DbResult<Connected>> {
        Box::pin(async move {
            let read_only = spec.read_only;
            let endpoint = endpoint(&spec)?;
            let (mut connection, tls) = Connection::open(endpoint).await?;
            let version = server_version(&mut connection).await?;
            Ok(Connected {
                connection: DbConnection::Kv(Box::new(RedisKv::new(connection, read_only))),
                server: ServerInfo { version, tls },
            })
        })
    }
}

/// 설정을 푼다: 붙을 주소 · TLS 계획 · DB 번호 · 자격 증명 · 시간 상한.
fn endpoint(spec: &ConnectionSpec) -> DbResult<Endpoint> {
    let Target::Network {
        host,
        port,
        connect_to,
    } = &spec.target
    else {
        return Err(DbError::new(
            DbErrorKind::Invalid,
            "a Redis connection needs a host and port",
        ));
    };
    let plan = TlsPlan::new(&spec.tls, &spec.target, &SSL_MODES)?;
    let db = database(spec.database.as_deref())?;
    let mut info = RedisConnectionInfo::default().set_db(db);
    if let Some(user) = spec.user.as_deref().filter(|u| !u.is_empty()) {
        info = info.set_username(user);
    }
    if let Some(password) = &spec.password {
        info = info.set_password(password.expose());
    }
    let (host, port) = match connect_to {
        Some(socket) => (socket.host.clone(), socket.port),
        None => (host.clone(), *port),
    };
    Ok(Endpoint {
        host,
        port,
        plan,
        tls: spec.tls.clone(),
        info,
        connect_timeout: Duration::from_millis(u64::from(spec.connect_timeout_ms)),
        response_timeout: spec
            .response_timeout_ms
            .map_or(DEFAULT_RESPONSE_TIMEOUT, |ms| {
                Duration::from_millis(u64::from(ms))
            }),
    })
}

/// DB 번호. 없으면 0.
fn database(database: Option<&str>) -> DbResult<i64> {
    match database.map(str::trim).filter(|d| !d.is_empty()) {
        None => Ok(0),
        Some(text) => text.parse::<u32>().map(i64::from).map_err(|_| {
            DbError::new(
                DbErrorKind::Invalid,
                format!("a Redis database is a number, got {text:?}"),
            )
        }),
    }
}

/// `INFO server` 의 `redis_version`(Valkey 는 `valkey_version` 도 함께 낸다).
async fn server_version(connection: &mut Connection) -> DbResult<Option<String>> {
    let info: String = connection
        .query(|mut c| async move { redis::cmd("INFO").arg("server").query_async(&mut c).await })
        .await?;
    let field = |name: &str| {
        info.lines()
            .find_map(|line| line.strip_prefix(name)?.strip_prefix(':'))
            .map(|v| v.trim().to_owned())
    };
    Ok(field("valkey_version")
        .map(|v| format!("{v} (Valkey)"))
        .or_else(|| field("redis_version")))
}

#[cfg(test)]
mod tests {
    use pw_db_types::{SocketTarget, TlsSettings};

    use super::*;

    fn spec() -> ConnectionSpec {
        ConnectionSpec::new(
            DriverKind::Redis.id(),
            Target::Network {
                host: "cache.internal".into(),
                port: 6379,
                connect_to: None,
            },
        )
    }

    #[test]
    fn capabilities_are_a_valid_kv_branch_with_three_ssl_modes() {
        let caps = RedisDriver::new().capabilities();
        assert_eq!(caps.validate(), Ok(()));
        assert!(caps.kv && caps.cancel && caps.multiple_databases);
        assert_eq!(
            caps.ssl_modes,
            [SslMode::Disable, SslMode::Require, SslMode::VerifyFull]
        );
        assert_eq!(RedisDriver::new().id(), "redis");
        assert_eq!(RedisDriver::new().dialect(), None);
    }

    #[test]
    fn other_ssl_modes_and_file_targets_are_refused() {
        for mode in [SslMode::Allow, SslMode::Prefer, SslMode::VerifyCa] {
            let mut spec = spec();
            spec.tls = TlsSettings::mode(mode);
            let error = endpoint(&spec).err().expect("refused");
            assert_eq!(error.kind, DbErrorKind::Invalid, "{mode:?}");
        }
        let file = ConnectionSpec::new(
            DriverKind::Redis.id(),
            Target::File {
                path: std::env::temp_dir().join("x.rdb"),
            },
        );
        assert_eq!(endpoint(&file).err().unwrap().kind, DbErrorKind::Invalid);
    }

    #[test]
    fn the_endpoint_uses_the_tunnel_but_names_tls_after_the_host() {
        let mut spec = spec();
        spec.target = Target::Network {
            host: "cache.internal".into(),
            port: 6379,
            connect_to: Some(SocketTarget {
                host: "127.0.0.1".into(),
                port: 50_001,
            }),
        };
        spec.database = Some("2".into());
        spec.response_timeout_ms = Some(1_000);
        let endpoint = endpoint(&spec).unwrap();
        assert_eq!(
            (endpoint.host.as_str(), endpoint.port),
            ("127.0.0.1", 50_001)
        );
        assert_eq!(endpoint.plan.server_name.as_deref(), Some("cache.internal"));
        assert_eq!(endpoint.info.db(), 2);
        assert_eq!(endpoint.response_timeout, Duration::from_secs(1));

        spec.response_timeout_ms = None;
        assert_eq!(
            super::endpoint(&spec).unwrap().response_timeout,
            DEFAULT_RESPONSE_TIMEOUT
        );
    }

    /// 드라이버의 스캔 묶음 상한과 콘솔 스캔의 `COUNT` 상한은 같은 값이다(리뷰 5).
    #[test]
    fn the_scan_limit_matches_the_console_limit() {
        assert_eq!(
            u64::from(MAX_SCAN_COUNT),
            pw_db_sql::kv_command::MAX_SCAN_COUNT
        );
    }

    #[test]
    fn databases_are_numbers() {
        assert_eq!(database(None), Ok(0));
        assert_eq!(database(Some("")), Ok(0));
        assert_eq!(database(Some("15")), Ok(15));
        assert_eq!(database(Some("x")).unwrap_err().kind, DbErrorKind::Invalid);
        assert_eq!(database(Some("-1")).unwrap_err().kind, DbErrorKind::Invalid);
    }

    /// 닫힌 포트 · 응답 없는 서버는 연결 오류 · 타임아웃이고, 받지 않는 `sslmode` 는 소켓을 열기 전에 거절된다.
    #[tokio::test]
    async fn connect_fails_cleanly() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let mut closed = ConnectionSpec::new(
            DriverKind::Redis.id(),
            Target::Network {
                host: "127.0.0.1".into(),
                port,
                connect_to: None,
            },
        );
        closed.tls = TlsSettings::mode(SslMode::Disable);
        let error = RedisDriver::new().connect(closed).await.err().unwrap();
        assert_eq!(error.kind, DbErrorKind::Connect, "{error}");

        // 받아 주기만 하고 답하지 않는 서버: 핸드셰이크가 연결 타임아웃에서 끝난다.
        let silent = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = silent.local_addr().unwrap().port();
        let hold = tokio::spawn(async move {
            let (_socket, _) = silent.accept().await.unwrap();
            tokio::time::sleep(Duration::from_secs(30)).await;
        });
        let mut quiet = ConnectionSpec::new(
            DriverKind::Redis.id(),
            Target::Network {
                host: "127.0.0.1".into(),
                port,
                connect_to: None,
            },
        );
        quiet.tls = TlsSettings::mode(SslMode::Disable);
        quiet.connect_timeout_ms = 300;
        let error = RedisDriver::new().connect(quiet).await.err().unwrap();
        assert_eq!(error.kind, DbErrorKind::Timeout, "{error}");
        hold.abort();
    }
}
