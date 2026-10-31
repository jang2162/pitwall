//! 드라이버가 받는 연결 설정과 드라이버 · 방언 · TLS 의 이름.
//!
//! [`ConnectionSpec`] 은 설정 파일(`.pitwall/db/<id>.toml`, M3-02)을 푼 결과다 — 비밀은 이미 꺼냈고(키체인 · env 참조는
//! M3-02 가 푼다) 터널은 이미 열렸다(M3-11 이 `connect_to` 를 채운다). 파일 형식 · RPC 타입은 M3-02 의 것이고, 이것은
//! 드라이버 trait 의 입력이다. 비밀번호가 들어 있으므로 RPC 응답 · 로그로 내보내지 않는다 — TS 타입이 없고, `Debug` 는
//! 비밀을 가린다.

use std::borrow::Cow;
use std::fmt;
use std::path::{Path, PathBuf};

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
///
/// 필드를 더해도 다른 크레이트가 깨지지 않게 `#[non_exhaustive]` 다 — [`ConnectionSpec::new`] 로 만들고 필드를 고친다.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
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
    /// 파일 DB 가 붙여도 되는 루트(실제 경로) — SQLite 의 `ATTACH` · `VACUUM INTO` 대상은 [`ConnectionSpec::file_allowed`] 를
    /// 지나야 한다(M3-08). 채우는 것은 신뢰 게이트(M3-02 `db_trust_gate`)다: 그 프로젝트의 루트(활성 워크트리 포함)와 사람이
    /// 루트 밖 확인을 준 실제 경로(프로젝트 상태에 기억한 것). 네트워크 드라이버는 빈 목록이고, 빈 목록이면 어떤 파일도 붙이지 않는다.
    pub allowed_roots: Vec<PathBuf>,
}

impl ConnectionSpec {
    /// 나머지는 안전한 기본값: DB · 사용자 · 비밀 없음, `sslmode` `verify-full`(결정 M3-D10), 읽기 쓰기, 연결 10 s,
    /// 응답 타임아웃은 드라이버 기본, 허용 루트 없음.
    pub fn new(driver: impl Into<String>, target: Target) -> Self {
        Self {
            driver: driver.into(),
            target,
            database: None,
            user: None,
            password: None,
            tls: TlsSettings::mode(SslMode::VerifyFull),
            read_only: false,
            connect_timeout_ms: 10_000,
            response_timeout_ms: None,
            allowed_roots: Vec::new(),
        }
    }

    /// 파일 DB 가 `real_path` 를 붙여도 되는가 — [`allowed_roots`](Self::allowed_roots) 하나의 안([`path_within`])일 때만.
    /// `real_path` 는 링크를 끝까지 푼 절대 경로여야 한다(푸는 것은 드라이버 몫이다). 상대 경로는 거절한다.
    pub fn file_allowed(&self, real_path: &Path) -> bool {
        real_path.is_absolute()
            && self
                .allowed_roots
                .iter()
                .any(|root| path_within(real_path, root))
    }
}

/// `path` 가 `root` 안인가(구성 요소 단위 — `/p/proj` 가 `/p/project2` 를 품지 않는다). Windows 에서는 verbatim 접두
/// (`\\?\C:\` · `\\?\UNC\`)와 보통 접두(`C:\` · `\\`)를 같은 자리로 본다: `fs::canonicalize` 는 verbatim 으로 돌려주고(드라이버가
/// 푸는 `ATTACH` 대상), 신뢰 게이트가 넘기는 경로 · 루트는 그 접두를 걷은 꼴이다(`pw_core::registry::path::simplify` — 걷으면 뜻이
/// 바뀌는 긴 경로 등은 verbatim 으로 남는다).
pub fn path_within(path: &Path, root: &Path) -> bool {
    path.starts_with(root) || plain(path).starts_with(plain(root))
}

/// 비교용 꼴: Windows 의 verbatim 접두를 걷는다. 다른 OS 에서는 그대로다.
fn plain(path: &Path) -> Cow<'_, Path> {
    #[cfg(windows)]
    if let Some(plain) = path.to_str().and_then(without_verbatim_prefix) {
        return Cow::Owned(PathBuf::from(plain));
    }
    Cow::Borrowed(path)
}

/// `\\?\C:\a` → `C:\a`, `\\?\UNC\srv\share\a` → `\\srv\share\a`. 그 밖은 `None`. 비교에만 쓴다 — 실제 경로(`canonicalize` 의
/// 결과)는 `.` · `..` · `/` 가 없어 접두만 걷어도 구성 요소가 그대로다. 문자열만 보는 함수라 모든 OS 에서 시험한다.
#[cfg_attr(not(windows), allow(dead_code))]
fn without_verbatim_prefix(path: &str) -> Option<String> {
    let rest = path.strip_prefix(r"\\?\")?;
    if let Some(unc) = rest.strip_prefix(r"UNC\") {
        return Some(format!(r"\\{unc}"));
    }
    let drive = rest.as_bytes();
    (drive.len() >= 3 && drive[0].is_ascii_alphabetic() && &drive[1..3] == b":\\")
        .then(|| rest.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_hides_the_secret() {
        let mut spec = ConnectionSpec::new(
            DriverKind::Postgres.id(),
            Target::Network {
                host: "db.internal".into(),
                port: 5432,
                connect_to: None,
            },
        );
        spec.user = Some("app".into());
        spec.password = Some(Secret::new("hunter2"));
        let debug = format!("{spec:?}");
        assert!(!debug.contains("hunter2"), "{debug}");
        assert!(debug.contains("Secret(***)"), "{debug}");
    }

    /// 14-m3-plan.md M3-08 ⑦ 의 바탕: 루트 판정은 구성 요소 단위다(`/p/proj` 가 `/p/project2` 를 품지 않는다).
    #[test]
    fn file_targets_must_sit_inside_an_allowed_root() {
        let base = std::env::temp_dir();
        let mut spec = ConnectionSpec::new(
            DriverKind::Sqlite.id(),
            Target::File {
                path: base.join("proj").join("app.db"),
            },
        );
        assert!(
            !spec.file_allowed(&base.join("proj").join("other.db")),
            "no roots, no files"
        );
        spec.allowed_roots = vec![base.join("proj"), base.join("outside-ok")];
        assert!(spec.file_allowed(&base.join("proj").join("db").join("other.db")));
        assert!(spec.file_allowed(&base.join("outside-ok").join("x.db")));
        assert!(!spec.file_allowed(&base.join("project2").join("x.db")));
        assert!(!spec.file_allowed(&base.join("outside-okay.db")));
        assert!(
            !spec.file_allowed(Path::new("proj/app.db")),
            "relative paths are refused"
        );
        assert_eq!(
            spec.tls.mode,
            SslMode::VerifyFull,
            "the default is the safe mode"
        );
    }

    /// Windows 의 `canonicalize` 는 `\\?\` 를 붙인다 — 드라이버가 푼 `ATTACH` 대상은 그 꼴이고 루트는 접두를 걷은 꼴이다.
    #[test]
    fn the_verbatim_prefix_is_dropped_for_comparison_only() {
        assert_eq!(
            without_verbatim_prefix(r"\\?\C:\Users\me\proj\app.db").as_deref(),
            Some(r"C:\Users\me\proj\app.db")
        );
        assert_eq!(
            without_verbatim_prefix(r"\\?\UNC\srv\share\app.db").as_deref(),
            Some(r"\\srv\share\app.db")
        );
        for other in [
            r"C:\Users\me\app.db",
            r"\\srv\share\app.db",
            r"\\?\GLOBALROOT\Device\x",
            r"\\?\C:",
            "/home/me/app.db",
        ] {
            assert_eq!(without_verbatim_prefix(other), None, "{other}");
        }
    }

    /// CI 의 Windows 러너: 루트는 보통 꼴(`C:\…`), 드라이버가 푼 대상은 verbatim(`\\?\C:\…`)이다. 어느 쪽이 어느 꼴이든 같은 자리다.
    #[cfg(windows)]
    #[test]
    fn windows_roots_match_verbatim_and_plain_paths_alike() {
        let mut spec = ConnectionSpec::new(
            DriverKind::Sqlite.id(),
            Target::File {
                path: PathBuf::from(r"C:\proj\app.db"),
            },
        );
        spec.allowed_roots = vec![PathBuf::from(r"C:\proj"), PathBuf::from(r"\\?\D:\other")];
        assert!(spec.file_allowed(Path::new(r"\\?\C:\proj\db\x.db")));
        assert!(spec.file_allowed(Path::new(r"C:\proj\x.db")));
        assert!(spec.file_allowed(Path::new(r"D:\other\x.db")));
        assert!(!spec.file_allowed(Path::new(r"\\?\C:\project2\x.db")));
        assert!(!spec.file_allowed(Path::new(r"\\?\D:\proj\x.db")));
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
