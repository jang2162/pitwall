//! 실 DB 시험 도우미: `PW_TEST_MYSQL_URL` 에서 연결 설정 · 관리 연결을 만들고, 시험마다 겹치지 않는 이름을 준다.

pub mod proxy;

use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::atomic::{AtomicU32, Ordering};

use mysql_async::{Conn, Opts, OptsBuilder};
use pw_db_driver::live::{self, LiveDb};
use pw_db_driver::{DbConnection, DbDriver, SqlConnection};
use pw_db_driver_mysql::MysqlDriver;
use pw_db_types::{
    ConnectionSpec, DbValue, DriverKind, ExecuteRequest, FetchLimit, RiskGrade, Row, Secret,
    SslMode, Target, TlsSettings,
};

pub fn url() -> String {
    LiveDb::Mysql.url()
}

/// URL 의 서버 주소(중계가 붙을 곳).
pub fn server_address() -> SocketAddr {
    let endpoint = live::endpoint(LiveDb::Mysql, &url()).unwrap();
    (endpoint.host.as_str(), endpoint.port)
        .to_socket_addrs()
        .unwrap()
        .next()
        .unwrap()
}

/// URL 의 서버로 가는 연결 설정.
pub fn spec(mode: SslMode) -> ConnectionSpec {
    let url = url();
    let endpoint = live::endpoint(LiveDb::Mysql, &url).unwrap();
    let rest = url.split_once("://").unwrap().1;
    let (userinfo, after) = rest
        .rsplit_once('@')
        .map_or((None, rest), |(u, a)| (Some(u), a));
    let mut spec = ConnectionSpec::new(
        DriverKind::Mysql.id(),
        Target::Network {
            host: endpoint.host,
            port: endpoint.port,
            connect_to: None,
        },
    );
    spec.user = userinfo.map(|u| u.split(':').next().unwrap().to_owned());
    spec.password = endpoint.password.map(Secret::new);
    spec.database = after
        .split_once('/')
        .map(|(_, path)| path.split(['?', '#']).next().unwrap().to_owned())
        .filter(|d| !d.is_empty());
    spec.tls = TlsSettings::mode(mode);
    spec
}

/// 드라이버 밖의 관리 연결(PROCESSLIST · 다른 세션에서 보기 · 픽스처).
pub async fn admin() -> Conn {
    let opts = OptsBuilder::from_opts(Opts::from_url(&url()).unwrap()).prefer_socket(false);
    Conn::new(opts).await.unwrap()
}

pub async fn open(mode: SslMode) -> (Box<dyn SqlConnection>, String) {
    let connected = MysqlDriver::new().connect(spec(mode)).await.unwrap();
    let DbConnection::Sql(conn) = connected.connection else {
        unreachable!("mysql is an sql driver")
    };
    (conn, connected.server.version.unwrap_or_default())
}

pub fn is_mariadb(version: &str) -> bool {
    version.contains("MariaDB")
}

/// 시험 · 프로세스마다 다른 이름.
pub fn unique(prefix: &str) -> String {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    format!(
        "{prefix}_{}_{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::SeqCst)
    )
}

/// 문장을 끝까지 돌려 모든 행을 받는다.
pub async fn rows(conn: &mut dyn SqlConnection, sql: &str, grade: RiskGrade) -> Vec<Row> {
    run(conn, ExecuteRequest::new(sql, grade)).await
}

pub async fn run(conn: &mut dyn SqlConnection, request: ExecuteRequest) -> Vec<Row> {
    let started = conn.execute(request).await.unwrap();
    let mut all = Vec::new();
    if started.end.is_some() {
        return all;
    }
    loop {
        let batch = conn.fetch(FetchLimit::rows(10_000)).await.unwrap();
        all.extend(batch.rows);
        if batch.end.is_some() {
            return all;
        }
    }
}

pub async fn write(conn: &mut dyn SqlConnection, sql: &str) {
    let started = conn
        .execute(ExecuteRequest::new(sql, RiskGrade::Write))
        .await
        .unwrap();
    assert!(started.end.is_some(), "{sql} returned rows");
}

pub async fn scalar(conn: &mut dyn SqlConnection, sql: &str) -> DbValue {
    rows(conn, sql, RiskGrade::Read).await.remove(0).remove(0)
}

/// `n` 행(0 ‥ n−1)과 100바이트 채움 열을 만드는 조회. 1,000 × 1,000 까지 — 재귀 CTE 깊이(MySQL 기본 1,000) 안이다.
pub fn numbers(n: u64) -> String {
    format!(
        "WITH RECURSIVE s(n) AS (SELECT 0 UNION ALL SELECT n + 1 FROM s WHERE n < 999) \
         SELECT a.n * 1000 + b.n AS id, REPEAT('x', 100) AS pad FROM s a CROSS JOIN s b \
         WHERE a.n * 1000 + b.n < {n}"
    )
}
