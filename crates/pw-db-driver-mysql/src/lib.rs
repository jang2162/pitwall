//! MySQL/MariaDB 드라이버(14-m3-plan.md M3-07): `mysql_async`(기능 `default-rustls-ring`, 내장 루트를 끄고 OS 루트만 — 결정
//! M3-D1 · M3-D10). MariaDB 도 같은 드라이버다.
//!
//! - [`MysqlDriver`]: 연결 · 연결 테스트. 연결 하나는 풀 없이 TCP 연결 하나다(콘솔 탭 하나 = 고정 연결 하나, M3-05).
//! - `conn`: SQL 갈래 — 결과 집합 스트리밍 커서, 등급을 따르는 커서 버리기, `KILL QUERY` 취소, 트랜잭션.
//! - [`tls`]: `sslmode` → `SslOpts`. `allow` · `prefer` 의 「TLS 가 안 되면 다른 쪽」 은 다시 붙어서 한다(아래 「`sslmode`」).
//! - `meta`: `information_schema` 메타데이터와 `SHOW CREATE TABLE`.
//! - `value`: 열 · 셀 · 바인드 값(결정 M3-D8).
//!
//! **`sslmode`**(libpq 뜻): `disable` 평문, `require` TLS · 검증 없음, `verify-full` 전부. `verify-ca` 는 받지 않는다(`tls` 머리
//! 주석 — `mysql_async` 의 이름 건너뛰기가 rustls 0.23 과 맞지 않는다). `prefer` 는 TLS 로
//! 붙어 보고 서버가 TLS 를 못 하면(핸드셰이크에 `CLIENT_SSL` 이 없으면) 평문으로 다시 붙는다 — 표지는 「평문」. `allow` 는 평문으로
//! 붙어 보고 서버가 평문을 거절하면(`require_secure_transport` 의 오류 3159) TLS 로 다시 붙는다. 계정 단위 `REQUIRE SSL` 의 거절은
//! 비밀번호가 틀린 것과 같은 1045 라 가를 수 없어 다시 붙지 않는다(로그인 실패 횟수 잠금을 건드리지 않게).
//!
//! 실 DB 시험은 `tests/live_mysql.rs` 다(`#[cfg(pw_live_mysql)]`, 이름에 `live_mysql` — `pw_db_driver::live` 머리 주석). MariaDB 는
//! 같은 시험을 `PW_TEST_MARIADB_URL` 로 한 번 더 돈다.

mod conn;
mod error;
mod meta;
pub mod tls;
mod value;

use std::sync::Arc;
use std::time::Duration;

use mysql_async::prelude::Queryable;
use mysql_async::{Conn, DriverError, Opts, OptsBuilder};
use pw_db_driver::budget::within;
use pw_db_driver::tls::TlsAttempt;
use pw_db_driver::{BoxFuture, Connected, DbConnection, DbDriver, DbResult, TlsPlan};
use pw_db_types::{
    Capabilities, ConnectionSpec, DbError, DbErrorKind, DriverKind, ServerInfo, SqlDialect, Target,
};

use crate::conn::{Interrupter, MysqlConnection};
use crate::error::{Phase, code, map, server_code};
pub use crate::tls::RootSource;

/// MySQL/MariaDB 드라이버. 등록부의 id 는 `mysql`.
#[derive(Clone)]
pub struct MysqlDriver {
    roots: RootSource,
}

impl Default for MysqlDriver {
    fn default() -> Self {
        Self::new()
    }
}

impl MysqlDriver {
    /// OS 신뢰 저장소의 루트로 검증하는 드라이버.
    pub fn new() -> Self {
        Self::with_root_source(Arc::new(tls::native_roots))
    }

    /// 루트 공급자를 바꾼다 — 시험이 시험 CA 를 넣고 빼는 자리다(14-m3-plan.md M3-07 ⑥). 내장 루트는 어느 쪽이든 꺼져 있다.
    pub fn with_root_source(roots: RootSource) -> Self {
        Self { roots }
    }

    async fn os_roots(&self) -> DbResult<Vec<Vec<u8>>> {
        let roots = self.roots.clone();
        tokio::task::spawn_blocking(move || roots())
            .await
            .map_err(|e| {
                DbError::new(
                    DbErrorKind::Tls,
                    format!("reading the OS trust store failed: {e}"),
                )
            })?
            .map_err(|e| DbError::new(DbErrorKind::Tls, e))
    }

    /// 계획대로 붙는다. 붙은 연결 · 그 설정 · TLS 로 붙었는지.
    async fn open(&self, spec: &ConnectionSpec) -> DbResult<(Conn, Opts, bool)> {
        let plan = tls::plan(&spec.tls, &spec.target)?;
        let Target::Network {
            host,
            port,
            connect_to,
        } = &spec.target
        else {
            return Err(DbError::new(
                DbErrorKind::Invalid,
                "MySQL connects to a host and port",
            ));
        };
        let (address, address_port) = match connect_to {
            Some(tunnel) => (tunnel.host.clone(), tunnel.port),
            None => (host.clone(), *port),
        };
        let base = OptsBuilder::default()
            .ip_or_hostname(address)
            .tcp_port(address_port)
            .user(spec.user.clone())
            .pass(spec.password.as_ref().map(|p| p.expose().to_owned()))
            .db_name(spec.database.clone())
            // `localhost` 에서 `@@socket` 으로 갈아타지 않는다 — TLS 와 `KILL` 연결이 같은 TCP 길을 쓴다.
            .prefer_socket(false)
            .stmt_cache_size(0);
        match plan.attempt {
            TlsAttempt::Never => self.plain(base).await,
            TlsAttempt::Required => self.secure(base, spec, &plan).await,
            TlsAttempt::TlsFirst => match self.secure(base.clone(), spec, &plan).await {
                Err(Attempt::Mysql(mysql_async::Error::Driver(
                    DriverError::NoClientSslFlagFromServer,
                ))) => self.plain(base).await,
                other => other,
            },
            TlsAttempt::PlainFirst => match self.plain(base.clone()).await {
                Err(Attempt::Mysql(e))
                    if server_code(&e) == Some(code::SECURE_TRANSPORT_REQUIRED) =>
                {
                    self.secure(base, spec, &plan).await
                }
                other => other,
            },
        }
        .map_err(|attempt| match attempt {
            Attempt::Mysql(e) => map(e, Phase::Connect),
            Attempt::Db(e) => e,
        })
    }

    async fn plain(&self, base: OptsBuilder) -> Result<(Conn, Opts, bool), Attempt> {
        let opts = Opts::from(base);
        let conn = Conn::new(opts.clone()).await?;
        Ok((conn, opts, false))
    }

    async fn secure(
        &self,
        base: OptsBuilder,
        spec: &ConnectionSpec,
        plan: &TlsPlan,
    ) -> Result<(Conn, Opts, bool), Attempt> {
        let roots = self.os_roots().await?;
        let name = plan
            .server_name
            .clone()
            .expect("a network target has a TLS name");
        let opts = Opts::from(base.ssl_opts(tls::ssl_opts(plan, &spec.tls, roots, name)?));
        let conn = Conn::new(opts.clone()).await?;
        Ok((conn, opts, true))
    }
}

/// 붙는 시도의 오류. `mysql_async` 오류는 다음 시도를 고르려고 그대로 들고 있다.
enum Attempt {
    Mysql(mysql_async::Error),
    Db(DbError),
}

impl From<mysql_async::Error> for Attempt {
    fn from(e: mysql_async::Error) -> Self {
        Self::Mysql(e)
    }
}

impl From<DbError> for Attempt {
    fn from(e: DbError) -> Self {
        Self::Db(e)
    }
}

impl DbDriver for MysqlDriver {
    fn id(&self) -> &str {
        DriverKind::Mysql.id()
    }

    fn dialect(&self) -> Option<SqlDialect> {
        Some(SqlDialect::Mysql)
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            sql: true,
            kv: false,
            server_cursor: true,
            cancel: true,
            transactions: true,
            savepoints: true,
            read_only_transaction: true,
            explain: true,
            metadata: true,
            schemas: false,
            multiple_databases: true,
            ssl_modes: tls::SSL_MODES.to_vec(),
        }
    }

    fn connect(&self, spec: ConnectionSpec) -> BoxFuture<'_, DbResult<Connected>> {
        Box::pin(async move {
            let timeout = Duration::from_millis(u64::from(spec.connect_timeout_ms));
            let (mut conn, opts, used_tls) = within(timeout, "connect", self.open(&spec)).await?;
            let version: Option<String> = conn
                .query_first("SELECT VERSION()")
                .await
                .map_err(|e| map(e, Phase::Run))?;
            let tls = tls::plan(&spec.tls, &spec.target)?.status(used_tls);
            let interrupter = Arc::new(Interrupter::new(opts, conn.id(), timeout));
            Ok(Connected {
                connection: DbConnection::Sql(Box::new(MysqlConnection::new(conn, interrupter))),
                server: ServerInfo { version, tls },
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use pw_db_types::SslMode;

    use super::*;

    #[test]
    fn capabilities_are_valid_and_take_every_sslmode_but_verify_ca() {
        let driver = MysqlDriver::new();
        let capabilities = driver.capabilities();
        capabilities.validate().unwrap();
        let expected: Vec<_> = SslMode::ALL
            .into_iter()
            .filter(|m| *m != SslMode::VerifyCa)
            .collect();
        assert_eq!(capabilities.ssl_modes, expected);
        assert!(!capabilities.schemas, "a MySQL database is the schema");
        assert_eq!(driver.id(), "mysql");
    }

    #[tokio::test]
    async fn file_targets_are_refused_before_any_io() {
        let spec = ConnectionSpec::new(
            DriverKind::Mysql.id(),
            Target::File {
                path: std::env::temp_dir().join("a.db"),
            },
        );
        let error = MysqlDriver::new().connect(spec).await.err().unwrap();
        assert_eq!(error.kind, DbErrorKind::Invalid);
    }
}
