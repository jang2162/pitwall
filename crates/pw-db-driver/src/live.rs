//! 실 DB 시험 도우미(14-m3-plan.md M3-01, 결정 M3-D16).
//!
//! **시험을 다는 법**: `#[cfg(pw_live_pg)]`(MySQL · MariaDB 는 `pw_live_mysql`, Redis 는 `pw_live_redis`)를 달고 이름에
//! `live_pg` · `live_mysql` · `live_redis` 를 넣는다. 환경 변수가 없으면 시험이 목록에 없다(크레이트의 빌드 스크립트
//! `live_cfg.rs` 머리 주석). URL 은 [`LiveDb::url`]. **live 시험은 `pw-db*` 크레이트에만 둔다** — 그 크레이트의 `Cargo.toml` 에
//! `build = "../pw-db-driver/live_cfg.rs"` 가 있어야 표지가 켜지고(없으면 `unexpected_cfgs` 로 lint 가 실패한다),
//! `scripts/db-live.mjs` 는 `pw-db*` 패키지만 빌드해 돌린다(데스크톱 크레이트에 둔 live 시험은 돌지 않는다).
//!
//! **준비 확인**(`live_<db>_ready`): 드라이버 없이 `std::net` 으로 서버가 떴는지만 본다 — PG 는 `SSLRequest` 에 대한 1바이트
//! 응답, MySQL · MariaDB 는 첫 핸드셰이크 패킷의 프로토콜 판본 바이트(10), Redis 는 `PING` → `+PONG`. CI 의 `db-live` 잡에서는
//! 서비스가 다 뜨기를 기다리는 자리이기도 하다([`wait_ready`]). 이 시험은 실행 개수 하한 계산에서 뺀다(`scripts/db-live.mjs`).

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

/// 실 DB 의 종류. MariaDB 는 MySQL 시험을 그 URL 로 한 번 더 돈다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveDb {
    Pg,
    Mysql,
    Redis,
}

impl LiveDb {
    pub fn env(self) -> &'static str {
        match self {
            Self::Pg => "PW_TEST_PG_URL",
            Self::Mysql => "PW_TEST_MYSQL_URL",
            Self::Redis => "PW_TEST_REDIS_URL",
        }
    }

    fn default_port(self) -> u16 {
        match self {
            Self::Pg => 5432,
            Self::Mysql => 3306,
            Self::Redis => 6379,
        }
    }

    /// 시험이 쓸 URL. 표지(`cfg(pw_live_*)`)가 켜진 시험에서만 부른다 — 빌드 때 있던 변수가 실행 때 없으면 패닉한다.
    pub fn url(self) -> String {
        std::env::var(self.env())
            .ok()
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| {
                panic!(
                    "{} is not set (the live test was built with it)",
                    self.env()
                )
            })
    }
}

/// URL 에서 꺼낸 접속 정보. 준비 확인에 필요한 것만.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    pub host: String,
    pub port: u16,
    pub password: Option<String>,
}

/// `scheme://[user[:password]@]host[:port][/…]` 를 읽는다. IPv6 는 `[::1]`.
pub fn endpoint(db: LiveDb, url: &str) -> Result<Endpoint, String> {
    let rest = url
        .split_once("://")
        .map(|(_, rest)| rest)
        .ok_or_else(|| format!("not a URL: {}", redact(url)))?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let (userinfo, hostport) = match authority.rsplit_once('@') {
        Some((user, host)) => (Some(user), host),
        None => (None, authority),
    };
    let password = userinfo
        .and_then(|u| u.split_once(':'))
        .map(|(_, p)| p.to_owned())
        .filter(|p| !p.is_empty());
    let (host, port) = if let Some(v6) = hostport.strip_prefix('[') {
        let (host, after) = v6
            .split_once(']')
            .ok_or_else(|| format!("bad IPv6 host in {}", redact(url)))?;
        (host.to_owned(), after.strip_prefix(':'))
    } else {
        match hostport.rsplit_once(':') {
            Some((host, port)) => (host.to_owned(), Some(port)),
            None => (hostport.to_owned(), None),
        }
    };
    if host.is_empty() {
        return Err(format!("no host in {}", redact(url)));
    }
    let port = match port {
        None | Some("") => db.default_port(),
        Some(p) => p
            .parse()
            .map_err(|_| format!("bad port in {}", redact(url)))?,
    };
    Ok(Endpoint {
        host,
        port,
        password,
    })
}

/// 오류 글에 비밀번호가 새지 않게 `user:***@` 로 가린다.
fn redact(url: &str) -> String {
    match (url.find("://"), url.rfind('@')) {
        (Some(scheme), Some(at)) if at > scheme => {
            let userinfo = &url[scheme + 3..at];
            let user = userinfo.split(':').next().unwrap_or_default();
            format!("{}{user}:***{}", &url[..scheme + 3], &url[at..])
        }
        _ => url.to_owned(),
    }
}

/// 서버가 답하는지 한 번 본다. 답하면 서버를 설명하는 한 줄.
pub fn probe(db: LiveDb, endpoint: &Endpoint, timeout: Duration) -> Result<String, String> {
    let address = (endpoint.host.as_str(), endpoint.port)
        .to_socket_addrs()
        .map_err(|e| format!("cannot resolve {}: {e}", endpoint.host))?
        .next()
        .ok_or_else(|| format!("{} has no address", endpoint.host))?;
    let mut stream = TcpStream::connect_timeout(&address, timeout).map_err(|e| e.to_string())?;
    stream
        .set_read_timeout(Some(timeout))
        .and_then(|()| stream.set_write_timeout(Some(timeout)))
        .map_err(|e| e.to_string())?;
    match db {
        LiveDb::Pg => pg_ssl_request(&mut stream),
        LiveDb::Mysql => mysql_handshake(&mut stream),
        LiveDb::Redis => redis_ping(&mut stream, endpoint.password.as_deref()),
    }
}

/// `SSLRequest`(길이 8, 코드 80877103) → 서버는 `S`(TLS 가능) 또는 `N`(불가) 한 바이트로 답한다.
fn pg_ssl_request(stream: &mut TcpStream) -> Result<String, String> {
    let mut request = Vec::with_capacity(8);
    request.extend_from_slice(&8_i32.to_be_bytes());
    request.extend_from_slice(&80_877_103_i32.to_be_bytes());
    stream.write_all(&request).map_err(|e| e.to_string())?;
    let mut answer = [0_u8; 1];
    stream.read_exact(&mut answer).map_err(|e| e.to_string())?;
    match answer[0] {
        b'S' => Ok("postgres: SSLRequest answered S (TLS available)".to_owned()),
        b'N' => Ok("postgres: SSLRequest answered N (no TLS)".to_owned()),
        other => Err(format!(
            "postgres: unexpected SSLRequest answer byte {other:#04x}"
        )),
    }
}

/// 서버가 먼저 보내는 핸드셰이크: 길이 3바이트 · 순번 1바이트 · 프로토콜 판본(10) · 서버 판본 글(NUL 끝).
fn mysql_handshake(stream: &mut TcpStream) -> Result<String, String> {
    let mut header = [0_u8; 4];
    stream.read_exact(&mut header).map_err(|e| e.to_string())?;
    let length =
        usize::from(header[0]) | usize::from(header[1]) << 8 | usize::from(header[2]) << 16;
    if length == 0 || length > 1 << 16 {
        return Err(format!("mysql: unexpected handshake length {length}"));
    }
    let mut payload = vec![0_u8; length];
    stream.read_exact(&mut payload).map_err(|e| e.to_string())?;
    match payload[0] {
        10 => {
            let version = payload[1..]
                .split(|b| *b == 0)
                .next()
                .map(|v| String::from_utf8_lossy(v).into_owned())
                .unwrap_or_default();
            Ok(format!("mysql: handshake protocol 10, server {version}"))
        }
        0xff => Err(format!(
            "mysql: the server refused the connection: {}",
            String::from_utf8_lossy(payload.get(3..).unwrap_or_default())
        )),
        other => Err(format!("mysql: unexpected protocol version byte {other}")),
    }
}

/// (비밀번호가 있으면 `AUTH` 뒤) `PING` → `+PONG`.
fn redis_ping(stream: &mut TcpStream, password: Option<&str>) -> Result<String, String> {
    if let Some(password) = password {
        let auth = format!("*2\r\n$4\r\nAUTH\r\n${}\r\n{password}\r\n", password.len());
        stream
            .write_all(auth.as_bytes())
            .map_err(|e| e.to_string())?;
        let reply = read_line(stream)?;
        if reply != "+OK" {
            return Err(format!("redis: AUTH answered {reply}"));
        }
    }
    stream
        .write_all(b"*1\r\n$4\r\nPING\r\n")
        .map_err(|e| e.to_string())?;
    match read_line(stream)?.as_str() {
        "+PONG" => Ok("redis: PING answered +PONG".to_owned()),
        other => Err(format!("redis: PING answered {other}")),
    }
}

fn read_line(stream: &mut TcpStream) -> Result<String, String> {
    let mut line = Vec::new();
    let mut byte = [0_u8; 1];
    while !line.ends_with(b"\r\n") {
        if line.len() > 512 {
            return Err("redis: reply line too long".to_owned());
        }
        stream.read_exact(&mut byte).map_err(|e| e.to_string())?;
        line.push(byte[0]);
    }
    line.truncate(line.len() - 2);
    Ok(String::from_utf8_lossy(&line).into_owned())
}

/// 서버가 답할 때까지 `deadline` 까지 다시 묻는다(CI 서비스 컨테이너의 첫 기동은 수십 초 걸린다).
pub fn wait_ready(db: LiveDb, url: &str, deadline: Duration) -> Result<String, String> {
    let endpoint = endpoint(db, url)?;
    let started = Instant::now();
    loop {
        let error = match probe(db, &endpoint, Duration::from_secs(5)) {
            Ok(info) => return Ok(info),
            Err(error) => error,
        };
        if started.elapsed() >= deadline {
            return Err(format!(
                "{}:{} not ready after {:?}: {error}",
                endpoint.host, endpoint.port, deadline
            ));
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

#[cfg(test)]
mod tests {
    use std::net::TcpListener;

    use super::*;

    #[test]
    fn urls_give_host_port_and_password() {
        let pg = endpoint(LiveDb::Pg, "postgres://app:s3cret@127.0.0.1:55432/pitwall").unwrap();
        assert_eq!((pg.host.as_str(), pg.port), ("127.0.0.1", 55432));
        assert_eq!(pg.password.as_deref(), Some("s3cret"));
        let mysql = endpoint(LiveDb::Mysql, "mysql://root@localhost/test").unwrap();
        assert_eq!(
            (mysql.host.as_str(), mysql.port, mysql.password),
            ("localhost", 3306, None)
        );
        let redis = endpoint(LiveDb::Redis, "redis://:pw@[::1]:6380/0").unwrap();
        assert_eq!((redis.host.as_str(), redis.port), ("::1", 6380));
        assert_eq!(redis.password.as_deref(), Some("pw"));
        let error = endpoint(LiveDb::Pg, "postgres://app:s3cret@:x/db").unwrap_err();
        assert!(!error.contains("s3cret"), "{error}");
    }

    /// 준비 확인이 바이트를 맞게 읽는지 가짜 서버로 본다(실 DB 없이 세 OS 에서 돈다).
    #[test]
    fn probes_read_each_protocol_s_first_bytes() {
        fn serve(reply: Vec<u8>, expect_request: usize) -> Endpoint {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let port = listener.local_addr().unwrap().port();
            std::thread::spawn(move || {
                let (mut socket, _) = listener.accept().unwrap();
                let mut request = vec![0_u8; expect_request];
                socket.read_exact(&mut request).unwrap();
                socket.write_all(&reply).unwrap();
            });
            Endpoint {
                host: "127.0.0.1".into(),
                port,
                password: None,
            }
        }
        let timeout = Duration::from_secs(5);
        let pg = probe(LiveDb::Pg, &serve(b"N".to_vec(), 8), timeout).unwrap();
        assert!(pg.contains("no TLS"), "{pg}");

        let mut handshake = vec![0x10, 0, 0, 0, 10];
        handshake.extend_from_slice(b"11.8.3-MariaDB\0");
        handshake.resize(4 + 0x10, 0);
        let mysql = probe(LiveDb::Mysql, &serve(handshake, 0), timeout).unwrap();
        assert!(mysql.contains("11.8.3-MariaDB"), "{mysql}");

        let redis = probe(LiveDb::Redis, &serve(b"+PONG\r\n".to_vec(), 14), timeout).unwrap();
        assert!(redis.contains("PONG"), "{redis}");
        let wrong = probe(
            LiveDb::Redis,
            &serve(b"-NOAUTH x\r\n".to_vec(), 14),
            timeout,
        );
        assert!(wrong.is_err());
    }

    /// 준비 확인 시험(이름 `live_<db>_ready` — 하한 계산에서 뺀다). 서비스가 다 뜨기를 기다리는 자리이기도 하다.
    #[cfg(any(pw_live_pg, pw_live_mysql, pw_live_redis))]
    fn ready(db: LiveDb) {
        let info = wait_ready(db, &db.url(), Duration::from_secs(120)).unwrap();
        println!("{info}");
    }

    #[cfg(pw_live_pg)]
    #[test]
    fn live_pg_ready() {
        ready(LiveDb::Pg);
    }

    #[cfg(pw_live_mysql)]
    #[test]
    fn live_mysql_ready() {
        ready(LiveDb::Mysql);
    }

    #[cfg(pw_live_redis)]
    #[test]
    fn live_redis_ready() {
        ready(LiveDb::Redis);
    }
}
