//! 드라이버가 받는 연결 설정과 드라이버 · 방언 · TLS 의 이름.
//!
//! [`ConnectionSpec`] 은 설정 파일(`.pitwall/db/<id>.toml`, M3-02)을 푼 결과다 — 비밀은 이미 꺼냈고(키체인 · env 참조는
//! M3-02 가 푼다) 터널은 이미 열렸다(M3-11 이 `connect_to` 를 채운다). 파일 형식 · RPC 타입은 M3-02 의 것이고, 이것은
//! 드라이버 trait 의 입력이다. 비밀번호가 들어 있으므로 RPC 응답 · 로그로 내보내지 않는다 — TS 타입이 없고, `Debug` 는
//! 비밀을 가린다.

use std::fmt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// 내장 드라이버. 설정 파일의 `driver` 값이다.
///
/// 드라이버 등록부의 키는 이 enum 이 아니라 문자열 id 다([`DriverKind::id`]) — 프로세스 밖 드라이버(JDBC 브리지 F6-3b)가
/// 이 enum 을 고치지 않고 들어올 자리다.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    Serialize,
    Deserialize,
    specta::Type,
    schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum DriverKind {
    Postgres,
    Mysql,
    Sqlite,
    Redis,
}

impl DriverKind {
    pub const ALL: [Self; 4] = [Self::Postgres, Self::Mysql, Self::Sqlite, Self::Redis];

    /// 등록부의 드라이버 id(설정 파일의 `driver` 값과 같다).
    pub fn id(self) -> &'static str {
        match self {
            Self::Postgres => "postgres",
            Self::Mysql => "mysql",
            Self::Sqlite => "sqlite",
            Self::Redis => "redis",
        }
    }
}

/// SQL 방언 — 위험도 분류(M3-03)와 문장 나누기(M3-04)가 고른다. MariaDB 는 MySQL 이다.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    Serialize,
    Deserialize,
    specta::Type,
    schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum SqlDialect {
    Postgres,
    Mysql,
    Sqlite,
}

/// libpq 의 `sslmode` 뜻 그대로(결정 M3-D10). 드라이버마다 받는 값은 [`crate::Capabilities::ssl_modes`].
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    Serialize,
    Deserialize,
    specta::Type,
    schemars::JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum SslMode {
    /// TLS 없음.
    Disable,
    /// 평문을 먼저, 서버가 요구하면 TLS(검증 없음).
    Allow,
    /// TLS 를 먼저, 안 되면 평문(검증 없음).
    Prefer,
    /// TLS 필수, 인증서를 검증하지 않는다.
    Require,
    /// TLS 필수, 인증서 사슬만 검증(이름은 보지 않는다).
    VerifyCa,
    /// TLS 필수, 사슬과 이름 모두 검증.
    VerifyFull,
}

impl SslMode {
    pub const ALL: [Self; 6] = [
        Self::Disable,
        Self::Allow,
        Self::Prefer,
        Self::Require,
        Self::VerifyCa,
        Self::VerifyFull,
    ];
}

/// TLS 설정(`sslmode` 와 인증서 경로).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TlsSettings {
    pub mode: SslMode,
    /// 서버 인증서를 검증할 CA 묶음(PEM). 없으면 OS 신뢰 저장소만.
    pub root_cert: Option<PathBuf>,
    /// 클라이언트 인증서 · 키(PEM).
    pub client_cert: Option<PathBuf>,
    pub client_key: Option<PathBuf>,
}

impl TlsSettings {
    pub fn mode(mode: SslMode) -> Self {
        Self {
            mode,
            root_cert: None,
            client_cert: None,
            client_key: None,
        }
    }
}

/// 연결 대상.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum Target {
    /// TCP 서버(PG · MySQL · Redis).
    Network {
        /// 설정의 DB 호스트. TLS 이름 검증은 늘 이것으로 한다(터널 너머에서도 — 결정 M3-D10).
        host: String,
        port: u16,
        /// 실제로 붙을 주소. 터널이 열려 있으면 `127.0.0.1:<로컬 포트>`(M3-11), 없으면 `None` 이고 `host:port` 로 붙는다.
        connect_to: Option<SocketTarget>,
    },
    /// SQLite 파일. 루트 판정(루트 밖 확인)은 연결을 열기 전에 끝났다(M3-02).
    File { path: PathBuf },
}

/// 실제로 붙을 TCP 주소.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SocketTarget {
    pub host: String,
    pub port: u16,
}

/// 비밀(비밀번호 등). `Debug` 는 값을 가린다.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// 드라이버가 서버에 보낼 때만 꺼낸다.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(***)")
    }
}

/// 드라이버가 연결을 여는 데 필요한 것 전부.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionSpec {
    /// 드라이버 id([`DriverKind::id`] 또는 프로세스 밖 드라이버의 id).
    pub driver: String,
    pub target: Target,
    /// 처음 고를 DB. Redis 는 DB 번호(`"0"` …) — Redis 는 DB 번호마다 연결을 따로 연다(결정 M3-D15).
    pub database: Option<String>,
    pub user: Option<String>,
    pub password: Option<Secret>,
    pub tls: TlsSettings,
    /// read-only 연결. SQLite 는 파일을 읽기 전용으로 연다(결정 M3-D6). 다른 드라이버의 읽기 전용 실행은 실행 관문(M3-05)이
    /// 읽기 전용 트랜잭션으로 강제한다.
    pub read_only: bool,
    /// 연결을 여는 데 쓰는 시간 상한(밀리초). 연결 테스트도 같다.
    pub connect_timeout_ms: u32,
    /// 응답 하나를 기다리는 시간 상한(밀리초, Redis — 결정 M3-D15). `None` 이면 드라이버 기본값.
    pub response_timeout_ms: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_hides_the_secret() {
        let spec = ConnectionSpec {
            driver: DriverKind::Postgres.id().to_owned(),
            target: Target::Network {
                host: "db.internal".into(),
                port: 5432,
                connect_to: None,
            },
            database: None,
            user: Some("app".into()),
            password: Some(Secret::new("hunter2")),
            tls: TlsSettings::mode(SslMode::VerifyFull),
            read_only: false,
            connect_timeout_ms: 10_000,
            response_timeout_ms: None,
        };
        let debug = format!("{spec:?}");
        assert!(!debug.contains("hunter2"), "{debug}");
        assert!(debug.contains("Secret(***)"), "{debug}");
    }

    #[test]
    fn sslmode_uses_the_libpq_spelling() {
        let names: Vec<String> = SslMode::ALL
            .iter()
            .map(|m| {
                serde_json::to_value(m)
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect();
        assert_eq!(
            names,
            [
                "disable",
                "allow",
                "prefer",
                "require",
                "verify-ca",
                "verify-full"
            ]
        );
        for kind in DriverKind::ALL {
            assert_eq!(serde_json::to_value(kind).unwrap(), kind.id());
        }
    }
}
