//! 연결 설정 파일 `.pitwall/db/<id>.toml` 읽기 · 쓰기와 `db.connection.*` RPC(M3-02, 14-m3-plan.md 의 M3-02 행).
//!
//! - 파일: `schema = 1`, `<id>` 는 파일 이름이다. 공유 `<id>.toml` 위에 개인 `<id>.local.toml` 을 키별로 덮는다 — 설정 계층(D9,
//!   `pw_core::config`)을 그대로 쓴다. 개인 전용 모드 프로젝트는 같은 구조가 `$CONFIG/projects/<slug>/db/` 에 있다.
//! - 비밀은 파일에 없다: `password = { secret_ref = "keychain" }`(값은 키체인, [`crate::secret`]) 또는 `{ env = "<변수>" }`(값은
//!   그 프로젝트의 런타임 환경). RPC 의 설정 타입([`DbConnectionConfig`])에도 값이 들어갈 자리가 없다.
//! - SSH 터널은 `[ssh_tunnel] host_ref` 하나뿐이다 — ssh 옵션을 적는 자리가 없다(결정 M3-D11). `host_ref` 와 (터널을 쓰면) DB
//!   `host` 는 `ssh` 의 argv 로 가므로 허용 글자 목록을 벗어나면 저장도 읽기도 거절한다([`validate`],
//!   `pw_db_driver::tunnel::check_destination`). 터널이 없는 DB `host` 는 `-` 로 시작하거나 공백 · 제어 문자를 담으면 거절한다.
//! - 읽기는 디스크에 쓰지 않는다(`ConfigSource::without_writes`) — 목록만 봐도 커밋된 파일이 마이그레이션되면 안 된다.
//! - 쓰기(`upsert` · `delete`)는 파일 쓰기와 같은 연결 범위다 — 그 프로젝트 창의 연결만(결정 M3-D18, `scope_may_write`).
//! - 감시: 데스크톱은 프로젝트 설정 스토어를 열지 않으므로 파일 감시(`fs.changed`)를 따라가 `.pitwall/` 이 바뀌면 다시 읽고, 병합
//!   값이 바뀐 연결만 `config.changed { changed: ["db/<id>"] }`(프로젝트 범위)로 낸다([`follow_files`]). 개인 전용 모드 폴더와 메인
//!   체크아웃의 `.local.toml` 폴백은 파일 감시 밖이라 보지 못한다(14 §3).

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};

use pw_core::bus::{EventBus, Filter, Received, Scope, Subscription};
use pw_core::config::write::{Links, WriteError};
use pw_core::config::{AppPaths, ConfigChanged, ConfigSnapshot, ConfigSource, LayerOrigin};
use pw_core::registry::state::scope_may_write;
use pw_core::registry::{Project, Registry, RegistryError};
use pw_db_types::{DriverKind, SslMode};
use pw_rpc::{ConnectionScope, Router, RpcError, code, rpc_method};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::secret::{EnvLookup, SecretStore};
use crate::trust::{Approvals, active_root};

/// 설정 계층의 파일 id 앞부분(`db/<id>`).
pub const FILE_PREFIX: &str = "db/";
/// 이 앱이 쓰는 연결 설정 파일의 스키마 버전.
pub const SCHEMA: u32 = 1;
/// `config.changed`(pw_core::config::TOPIC_CHANGED).
pub const TOPIC_CHANGED: &str = pw_core::config::TOPIC_CHANGED;
/// 파일 감시 토픽(`pw_fs::watch::TOPIC_CHANGED` — 이 크레이트는 pw-fs 에 기대지 않는다).
pub const FS_CHANGED: &str = "fs.changed";

/// 연결 설정. 설정 파일의 키(snake_case)를 RPC 에서는 camelCase 로 쓴다. 비밀 값은 들어갈 자리가 없다.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DbConnectionConfig {
    /// 사람이 읽는 이름. 프로덕션 연결의 파괴 등급 확인에서 이것을 친다(결정 M3-D20).
    pub name: String,
    pub driver: DriverKind,
    /// DB 호스트(SQLite 밖). 터널이 있으면 바스티언에서 본 이름이다.
    #[serde(default)]
    pub host: Option<String>,
    /// 없으면 드라이버 기본 포트([`default_port`]).
    #[serde(default)]
    pub port: Option<u16>,
    #[serde(default)]
    pub database: Option<String>,
    #[serde(default)]
    pub user: Option<String>,
    /// 비밀번호를 어디서 가져오는가. 없으면 비밀번호 없이 붙는다.
    #[serde(default)]
    pub password: Option<DbPasswordRef>,
    #[serde(default)]
    pub read_only: bool,
    #[serde(default)]
    pub production: bool,
    /// 연결 표지 색(M3-19 의 테마 토큰 이름이나 `#rrggbb`).
    #[serde(default)]
    pub color: Option<String>,
    /// libpq 뜻 그대로(결정 M3-D10). 없으면 [`default_ssl_mode`].
    #[serde(default)]
    pub sslmode: Option<SslMode>,
    #[serde(default)]
    pub sslrootcert: Option<String>,
    #[serde(default)]
    pub sslcert: Option<String>,
    #[serde(default)]
    pub sslkey: Option<String>,
    #[serde(default)]
    pub ssh_tunnel: Option<DbSshTunnel>,
    /// SQLite 파일. 상대 경로면 프로젝트 루트 기준.
    #[serde(default)]
    pub path: Option<String>,
    /// 연결을 여는 시간 상한(초, [`CONNECT_TIMEOUT_RANGE`]). 없으면 [`DEFAULT_CONNECT_TIMEOUT_SECS`]. 연결 테스트도 같다.
    #[serde(default)]
    pub connect_timeout: Option<u32>,
}

/// 비밀번호의 출처. 파일에서는 `{ secret_ref = "keychain" }` · `{ env = "<변수>" }` 다.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum DbPasswordRef {
    /// 키체인 항목(`db.secret.set` 으로 넣는다).
    Keychain,
    /// 그 프로젝트의 런타임 환경 변수. 연결을 열 때 푼다.
    Env { variable: String },
}

/// SSH 터널(결정 M3-D11). 목적지 하나뿐이다 — `~/.ssh/config` 의 호스트 이름이나 `ssh://[user@]host[:port]`.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DbSshTunnel {
    pub host_ref: String,
}

/// 저장할 파일: 커밋하는 공유 파일 또는 개인 `.local.toml`.
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    specta::Type,
    schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum DbConfigLocation {
    #[default]
    Shared,
    Local,
}

/// 연결 타임아웃을 적지 않았을 때(초).
pub const DEFAULT_CONNECT_TIMEOUT_SECS: u32 = 10;
/// 받는 연결 타임아웃(초). 밖이면 설정 오류다 — 판정은 Host 가 한다(화면은 그 거절을 보인다).
pub const CONNECT_TIMEOUT_RANGE: std::ops::RangeInclusive<u32> = 1..=600;

/// 드라이버의 기본 포트. SQLite 는 없다.
pub fn default_port(driver: DriverKind) -> Option<u16> {
    match driver {
        DriverKind::Postgres => Some(5432),
        DriverKind::Mysql => Some(3306),
        DriverKind::Redis => Some(6379),
        DriverKind::Sqlite => None,
    }
}

/// `sslmode` 를 적지 않았을 때(결정 M3-D10): DB 호스트가 루프백(`localhost` · `127.0.0.1` · `::1`)이면 `disable`, 아니면
/// `verify-full`. 터널이 있어도 같다 — 호스트는 바스티언에서 본 이름이므로 「바스티언 기준의 루프백」 이다.
pub fn default_ssl_mode(host: &str) -> SslMode {
    let host = host.trim_start_matches('[').trim_end_matches(']');
    if host.eq_ignore_ascii_case("localhost") || host == "127.0.0.1" || host == "::1" {
        SslMode::Disable
    } else {
        SslMode::VerifyFull
    }
}

impl DbConnectionConfig {
    /// 쓰이는 포트(적지 않았으면 드라이버 기본).
    pub fn effective_port(&self) -> Option<u16> {
        self.port.or_else(|| default_port(self.driver))
    }

    /// 쓰이는 `sslmode`(적지 않았으면 [`default_ssl_mode`]). SQLite 는 `disable`.
    pub fn effective_ssl_mode(&self) -> SslMode {
        match (&self.sslmode, &self.host) {
            (Some(mode), _) => *mode,
            (None, Some(host)) => default_ssl_mode(host),
            (None, None) => SslMode::Disable,
        }
    }

    /// 쓰이는 연결 타임아웃(밀리초). 연결 설정에 없으면 `default_secs`(전역 설정 `[db] connect_timeout`, 없으면
    /// [`DEFAULT_CONNECT_TIMEOUT_SECS`] — `crate::settings`).
    pub fn effective_connect_timeout_ms(&self, default_secs: u32) -> u32 {
        self.connect_timeout
            .unwrap_or(default_secs)
            .saturating_mul(1000)
    }
}

/// 연결 id(= 파일 이름): 영문자 · 숫자 · `-` · `_`, 1~64 글자. `.` 을 받지 않는다 — `a.local` 은 `a` 의 개인 파일과 겹친다.
pub fn valid_connection_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// 환경 변수 이름: `[A-Za-z_][A-Za-z0-9_]*`, 256 바이트까지.
pub fn valid_env_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    name.len() <= 256
        && bytes
            .next()
            .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
        && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// `ssh` argv 로 갈 수 있는 값: 비지 않고, `-` 로 시작하지 않고, 공백 · 제어 문자가 없다(옵션 주입 · `ProxyCommand` 를 막는다).
fn argv_safe(what: &str, value: &str) -> Result<(), String> {
    if value.is_empty() {
        return Err(format!("{what} is empty"));
    }
    if value.starts_with('-') {
        return Err(format!("{what} must not start with `-`"));
    }
    if value.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(format!(
            "{what} must not contain spaces or control characters"
        ));
    }
    Ok(())
}

/// 터널 목적지(결정 M3-D11): `~/.ssh/config` 의 호스트 이름 그대로, 또는 `ssh://[user@]host[:port]`. 글자는 허용 목록이다 — 규칙과
/// 까닭은 [`pw_db_driver::tunnel::check_destination`](ssh 를 띄우기 전에도 같은 검사를 다시 한다).
pub fn validate_host_ref(host_ref: &str) -> Result<(), String> {
    pw_db_driver::tunnel::check_destination(host_ref)
        .map_err(|why| format!("ssh_tunnel.host_ref {host_ref:?} {why}"))
}

/// 설정이 연결을 열 수 있는 꼴인가. 저장(`upsert`)은 이것을 지나야 하고, 읽은 파일이 지나지 못하면 그 연결은 「설정 오류」 다.
pub fn validate(config: &DbConnectionConfig) -> Result<(), String> {
    if config.name.trim().is_empty() {
        return Err("name is empty".to_owned());
    }
    if config.name.chars().any(char::is_control) {
        return Err("name must not contain control characters".to_owned());
    }
    if let Some(color) = &config.color
        && (color.len() > 64 || color.chars().any(|c| c.is_control() || c.is_whitespace()))
    {
        return Err("color must be a short token without spaces".to_owned());
    }
    if let Some(secs) = config.connect_timeout
        && !CONNECT_TIMEOUT_RANGE.contains(&secs)
    {
        return Err(format!(
            "connect_timeout must be {} to {} seconds",
            CONNECT_TIMEOUT_RANGE.start(),
            CONNECT_TIMEOUT_RANGE.end()
        ));
    }
    if let Some(DbPasswordRef::Env { variable }) = &config.password
        && !valid_env_name(variable)
    {
        return Err(format!(
            "password.env `{variable}` is not an environment variable name"
        ));
    }
    if config.driver == DriverKind::Sqlite {
        let Some(path) = config.path.as_deref().filter(|p| !p.is_empty()) else {
            return Err("a sqlite connection needs `path`".to_owned());
        };
        if path.chars().any(char::is_control) {
            return Err("path must not contain control characters".to_owned());
        }
        let network = [
            ("host", config.host.is_some()),
            ("port", config.port.is_some()),
            ("user", config.user.is_some()),
            ("password", config.password.is_some()),
            ("ssh_tunnel", config.ssh_tunnel.is_some()),
            ("sslmode", config.sslmode.is_some()),
        ];
        if let Some((key, _)) = network.iter().find(|(_, set)| *set) {
            return Err(format!("a sqlite connection has no `{key}`"));
        }
        return Ok(());
    }
    if config.path.is_some() {
        return Err("`path` is only for sqlite connections".to_owned());
    }
    let Some(host) = &config.host else {
        return Err(format!("a {} connection needs `host`", config.driver.id()));
    };
    argv_safe("host", host)?;
    if config.port == Some(0) {
        return Err("port must not be 0".to_owned());
    }
    for (key, value) in [
        ("user", &config.user),
        ("database", &config.database),
        ("sslrootcert", &config.sslrootcert),
        ("sslcert", &config.sslcert),
        ("sslkey", &config.sslkey),
    ] {
        if value
            .as_deref()
            .is_some_and(|v| v.chars().any(char::is_control))
        {
            return Err(format!("{key} must not contain control characters"));
        }
    }
    if let Some(tunnel) = &config.ssh_tunnel {
        validate_host_ref(&tunnel.host_ref)?;
        // 터널을 쓰면 DB 호스트도 ssh argv(`-L`)로 간다 — 목적지와 같은 허용 목록.
        pw_db_driver::tunnel::check_db_host(host).map_err(|why| format!("host {host:?} {why}"))?;
    }
    Ok(())
}

// --- 파일 형식 -----------------------------------------------------------------------------------------------

/// 파일의 키 그대로(snake_case). 모르는 키는 설정 오류다 — 특히 `[ssh_tunnel]` 에 옵션을 적는 자리가 없다는 것을 보인다.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileConnection {
    name: String,
    driver: DriverKind,
    #[serde(default)]
    host: Option<String>,
    #[serde(default)]
    port: Option<u16>,
    #[serde(default)]
    database: Option<String>,
    #[serde(default)]
    user: Option<String>,
    #[serde(default)]
    password: Option<FilePassword>,
    #[serde(default)]
    read_only: bool,
    #[serde(default)]
    production: bool,
    #[serde(default)]
    color: Option<String>,
    #[serde(default)]
    sslmode: Option<SslMode>,
    #[serde(default)]
    sslrootcert: Option<String>,
    #[serde(default)]
    sslcert: Option<String>,
    #[serde(default)]
    sslkey: Option<String>,
    #[serde(default)]
    ssh_tunnel: Option<FileTunnel>,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    connect_timeout: Option<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FilePassword {
    #[serde(default)]
    secret_ref: Option<String>,
    #[serde(default)]
    env: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileTunnel {
    host_ref: String,
}

/// 설정 계층이 병합한 값(JSON 객체, `schema` 는 빠졌다)을 연결 설정으로. 검증([`validate`])까지 한다.
pub fn from_file_value(value: &Value) -> Result<DbConnectionConfig, String> {
    let file: FileConnection = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
    let password = match file.password {
        None => None,
        Some(FilePassword {
            secret_ref: Some(r),
            env: None,
        }) if r == "keychain" => Some(DbPasswordRef::Keychain),
        Some(FilePassword {
            secret_ref: None,
            env: Some(variable),
        }) => Some(DbPasswordRef::Env { variable }),
        Some(_) => {
            return Err(
                "password must be `{ secret_ref = \"keychain\" }` or `{ env = \"<NAME>\" }`"
                    .to_owned(),
            );
        }
    };
    let config = DbConnectionConfig {
        name: file.name,
        driver: file.driver,
        host: file.host,
        port: file.port,
        database: file.database,
        user: file.user,
        password,
        read_only: file.read_only,
        production: file.production,
        color: file.color,
        sslmode: file.sslmode,
        sslrootcert: file.sslrootcert,
        sslcert: file.sslcert,
        sslkey: file.sslkey,
        ssh_tunnel: file.ssh_tunnel.map(|t| DbSshTunnel {
            host_ref: t.host_ref,
        }),
        path: file.path,
        connect_timeout: file.connect_timeout,
    };
    validate(&config)?;
    Ok(config)
}

/// 연결 설정을 파일 문서에 쓴다. 이 크레이트가 아는 키만 바꾸고(없으면 지운다) 사람이 단 주석은 그대로 둔다. `schema` 는 맨 앞.
pub fn write_document(doc: &mut toml_edit::DocumentMut, config: &DbConnectionConfig) {
    use toml_edit::{InlineTable, Item, Table, value};

    let root = doc.as_table_mut();
    let fresh = !root.contains_key("schema");
    // 있는 키는 자리에서 값만 바꾼다 — 키에 붙은 주석(파일 머리 주석은 첫 키에 붙는다)과 값 뒤 주석이 남는다.
    let mut set = |key: &str, item: Option<Item>| match item {
        None => {
            root.remove(key);
        }
        Some(mut new) => match root.get_mut(key) {
            Some(old) => {
                if let (Some(o), Some(n)) = (old.as_value(), new.as_value_mut()) {
                    *n.decor_mut() = o.decor().clone();
                }
                *old = new;
            }
            None => {
                root.insert(key, new);
            }
        },
    };
    set("schema", Some(value(i64::from(SCHEMA))));
    let text = |v: &Option<String>| v.as_ref().map(|v| value(v.as_str()));
    set("name", Some(value(config.name.as_str())));
    set("driver", Some(value(config.driver.id())));
    set("host", text(&config.host));
    set("port", config.port.map(|p| value(i64::from(p))));
    set("database", text(&config.database));
    set("user", text(&config.user));
    set(
        "password",
        config.password.as_ref().map(|p| {
            let mut table = InlineTable::new();
            match p {
                DbPasswordRef::Keychain => table.insert("secret_ref", "keychain".into()),
                DbPasswordRef::Env { variable } => table.insert("env", variable.as_str().into()),
            };
            value(table)
        }),
    );
    set("read_only", Some(value(config.read_only)));
    set("production", Some(value(config.production)));
    set("color", text(&config.color));
    set(
        "sslmode",
        config.sslmode.map(|m| {
            value(
                serde_json::to_value(m)
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_owned))
                    .unwrap_or_default(),
            )
        }),
    );
    set("sslrootcert", text(&config.sslrootcert));
    set("sslcert", text(&config.sslcert));
    set("sslkey", text(&config.sslkey));
    set("path", text(&config.path));
    set(
        "connect_timeout",
        config.connect_timeout.map(|s| value(i64::from(s))),
    );
    set(
        "ssh_tunnel",
        config.ssh_tunnel.as_ref().map(|t| {
            let mut table = Table::new();
            table.insert("host_ref", value(t.host_ref.as_str()));
            Item::Table(table)
        }),
    );
    if fresh {
        root.sort_values_by(|a, _, b, _| (b.get() == "schema").cmp(&(a.get() == "schema")));
    }
}

// --- RPC 타입 --------------------------------------------------------------------------------------------------

/// 연결 하나를 지금 열 수 있는가. 값을 읽지 않고 정한다 — 키체인 항목은 대상만 보고, 환경 변수는 이름만 본다.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum DbConnectionState {
    /// 프로젝트를 신뢰하지 않았다 — 연결 · 비밀 읽기를 하지 않는다(결정 M3-D13).
    Untrusted,
    /// 설정 파일이 틀렸다.
    Invalid { message: String },
    /// 키체인에 비밀이 없거나, 저장할 때의 대상(호스트 · TLS 등)과 지금 설정이 다르다 — 이전(`previous`) → 지금(`target`) 대상을
    /// 보이고 다시 물어 `db.secret.set` 한다.
    SecretRequired {
        reason: crate::secret::SecretMissing,
        target: Box<crate::secret::DbSecretTarget>,
        previous: Option<Box<crate::secret::DbSecretTarget>>,
    },
    /// 열기 전에 사람의 확인이 필요하다(`db.connection.approve`).
    ApprovalRequired {
        request: crate::trust::DbApprovalRequest,
    },
    /// 열 수 있다(서버에 닿는지는 열어 봐야 안다).
    Ready,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DbConnectionEntry {
    /// 연결 id(파일 이름).
    pub id: String,
    /// 설정이 틀렸으면 `null`(`state` 가 `invalid`).
    pub config: Option<DbConnectionConfig>,
    /// 값을 준 파일(공유 · 개인) — 연결 편집의 「저장 위치」 가 처음 고를 것.
    pub layers: Vec<LayerOrigin>,
    /// 앱보다 새 스키마 파일이 있어 쓰지 않는다.
    pub read_only: bool,
    pub state: DbConnectionState,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DbConnectionList {
    /// id 순.
    pub connections: Vec<DbConnectionEntry>,
    /// 프로젝트 설정을 읽지 못한 이유(그러면 목록이 비어 있다). 설정 계층은 파일 하나라도 깨지면 전체가 실패다(03 「데이터 저장」).
    pub problem: Option<String>,
    /// 확인 기록(`$DATA/projects/<id>/db.json`)을 읽지 못한 이유. 그동안은 모든 확인을 다시 묻고, 고치거나 지울 때까지 기록하지 않는다.
    pub state_problem: Option<String>,
    pub trusted: bool,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DbProjectParams {
    pub project_id: String,
    /// 활성 워크트리의 꼭대기(`ActiveWorktree.top`, 절대 경로). 공유 설정(`.pitwall/db/*.toml`)을 그 워크트리에서 읽고 쓴다.
    #[serde(default)]
    pub worktree: Option<String>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DbConnectionParams {
    pub project_id: String,
    /// 활성 워크트리의 꼭대기(`ActiveWorktree.top`, 절대 경로). 공유 설정(`.pitwall/db/*.toml`)을 그 워크트리에서 읽고 쓴다.
    #[serde(default)]
    pub worktree: Option<String>,
    pub connection_id: String,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DbConnectionUpsertParams {
    pub project_id: String,
    /// 활성 워크트리의 꼭대기(`ActiveWorktree.top`, 절대 경로). 공유 설정(`.pitwall/db/*.toml`)을 그 워크트리에서 읽고 쓴다.
    #[serde(default)]
    pub worktree: Option<String>,
    pub connection_id: String,
    pub config: DbConnectionConfig,
    /// 없으면 공유 파일(활성 워크트리의 것). 개인 파일은 프로젝트 하나에 하나 — 메인 체크아웃(개인 전용 모드면 그 폴더)에 쓴다.
    #[serde(default)]
    pub location: DbConfigLocation,
}

rpc_method! {
    /// 프로젝트의 DB 연결 목록(비밀 없이). 연결마다 신뢰 · 사용 가능 상태를 단다. 신뢰 전에도 돌려준다(상태는 모두 `untrusted`).
    pub DbConnectionListMethod: "db.connection.list" {
        params: DbProjectParams,
        result: DbConnectionList,
        tier: Read,
        roles: [Owner],
    }

    /// 연결 하나. 없으면 not found 오류.
    pub DbConnectionGet: "db.connection.get" {
        params: DbConnectionParams,
        result: DbConnectionEntry,
        tier: Read,
        roles: [Owner],
    }

    /// 연결 설정 파일을 만들거나 고친다(`location` 의 파일 하나에 연결 전부를 쓴다 — 개인 파일에 쓰면 공유 파일에만 있는 키는
    /// 그대로 남는다, 설정 계층에는 키를 지우는 문법이 없다). 그 프로젝트 창의 연결만 — 아니면 permission denied.
    pub DbConnectionUpsert: "db.connection.upsert" {
        params: DbConnectionUpsertParams,
        result: DbConnectionEntry,
        tier: Write,
        roles: [Owner],
    }

    /// 연결 설정 파일(공유 · 개인 둘 다)과 그 연결의 비밀 · 확인 기록을 지운다. 그 프로젝트 창의 연결만.
    pub DbConnectionDelete: "db.connection.delete" {
        params: DbConnectionParams,
        result: (),
        tier: Write,
        roles: [Owner],
    }
}

// --- 서비스 ----------------------------------------------------------------------------------------------------

/// Host 를 만든 뒤에 붙이는 것(Host 의 메서드는 Host 를 만드는 중에 붙으므로).
#[derive(Clone)]
pub struct HostParts {
    pub paths: AppPaths,
    pub projects: Registry,
    /// `config.changed` 를 내는 버스.
    pub bus: EventBus,
}

/// 연결 설정 · 비밀 · 신뢰 게이트(M3-02). 연결을 여는 쪽(M3-05)은 [`Connections::trust_gate`] → [`Connections::resolve`] 로
/// 드라이버에 줄 `ConnectionSpec` 을 받는다.
pub struct Connections {
    pub(crate) host: OnceLock<HostParts>,
    pub(crate) secrets: Arc<dyn SecretStore>,
    pub(crate) env: Arc<dyn EnvLookup>,
    /// 프로젝트마다 마지막으로 본 연결 파일의 병합 값(`config.changed` 를 바뀐 것만 내려고).
    seen: Mutex<HashMap<SeenKey, BTreeMap<String, Value>>>,
    /// 설정 파일 · 확인 기록 쓰기를 한 줄로 세운다(같은 파일을 두 요청이 읽고 고쳐 쓰지 않게).
    pub(crate) writing: Mutex<()>,
    /// 전역 설정의 `[db]`(M3-20 — 연결 타임아웃 기본값 · Redis 응답 타임아웃 · 그리드 · 셀 상한).
    pub settings: crate::settings::DbSettings,
}

/// (프로젝트 id, 활성 워크트리) — 워크트리마다 공유 설정이 다를 수 있다.
type SeenKey = (String, Option<String>);

pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

pub(crate) fn registry_error(e: RegistryError) -> RpcError {
    match e {
        RegistryError::NotFound(id) => {
            RpcError::new(code::NOT_FOUND, format!("project `{id}` is not registered"))
        }
        other => RpcError::internal(other.to_string()),
    }
}

/// 그 프로젝트 창의 연결이 아니면 permission denied(파일 쓰기와 같은 판정 — `pw_fs::Fs::write_roots`).
pub(crate) fn require_project_window(
    scope: &ConnectionScope,
    project_id: &str,
) -> Result<(), RpcError> {
    if scope_may_write(scope, project_id) {
        Ok(())
    } else {
        Err(RpcError::new(
            code::PERMISSION_DENIED,
            "only the project's own window may change its db connections",
        )
        .with_data(json!({ "projectId": project_id })))
    }
}

pub(crate) fn check_connection_id(id: &str) -> Result<(), RpcError> {
    if valid_connection_id(id) {
        Ok(())
    } else {
        Err(RpcError::invalid_params(format!(
            "connection id `{id}` must be 1-64 letters, digits, `-` or `_`"
        )))
    }
}

/// 읽은 연결 설정 파일 하나.
pub(crate) struct LoadedConnection {
    pub id: String,
    pub value: Value,
    pub config: Result<DbConnectionConfig, String>,
    pub layers: Vec<LayerOrigin>,
    pub read_only: bool,
}

/// 프로젝트의 연결 설정 파일들.
pub(crate) struct Loaded {
    pub project: Project,
    /// 활성 워크트리 안의 프로젝트 루트(없으면 주 루트). 상대 SQLite · 인증서 경로와 런타임 환경의 기준.
    pub root: PathBuf,
    /// 활성 워크트리의 설정(공유 파일을 쓰는 곳).
    pub source: ConfigSource,
    /// 개인 파일(`.local.toml`)을 쓰는 폴더 — 메인 체크아웃(개인 전용 모드면 그 폴더). 프로젝트 하나에 하나.
    pub local_dir: PathBuf,
    /// `local_dir` 의 설정(개인 파일을 쓰기 전의 확인 · `.gitignore` — `ConfigSource::prepare_local_write`).
    pub local_source: ConfigSource,
    pub connections: Result<Vec<LoadedConnection>, String>,
}

impl Loaded {
    pub fn get(&self, id: &str) -> Option<&LoadedConnection> {
        self.connections.as_ref().ok()?.iter().find(|c| c.id == id)
    }

    /// 대소문자만 다른 다른 연결(대소문자를 가리지 않는 파일 시스템에서는 같은 파일이다).
    pub fn case_twin(&self, id: &str) -> Option<&str> {
        self.connections
            .as_ref()
            .ok()?
            .iter()
            .find(|c| c.id != id && c.id.eq_ignore_ascii_case(id))
            .map(|c| c.id.as_str())
    }
}

fn connections_of(snapshot: &ConfigSnapshot) -> Vec<LoadedConnection> {
    snapshot
        .files
        .iter()
        .filter_map(|file| {
            let id = file.id.strip_prefix(FILE_PREFIX)?;
            if id.contains('/') {
                return None;
            }
            Some(LoadedConnection {
                id: id.to_owned(),
                value: file.value.clone(),
                config: if valid_connection_id(id) {
                    from_file_value(&file.value)
                } else {
                    Err(format!(
                        "connection id `{id}` must be 1-64 letters, digits, `-` or `_`"
                    ))
                },
                layers: file
                    .layers
                    .iter()
                    .map(|l| l.origin)
                    .filter(|o| *o != LayerOrigin::Builtin)
                    .collect(),
                read_only: file.layers.iter().any(|l| l.read_only),
            })
        })
        .collect()
}

impl Connections {
    pub fn new(secrets: Arc<dyn SecretStore>, env: Arc<dyn EnvLookup>) -> Self {
        Self {
            host: OnceLock::new(),
            secrets,
            env,
            seen: Mutex::default(),
            writing: Mutex::new(()),
            settings: crate::settings::DbSettings::default(),
        }
    }

    /// 두 번째부터는 무시한다.
    pub fn attach(&self, parts: HostParts) {
        let _ = self.host.set(parts);
    }

    pub(crate) fn host(&self) -> Result<&HostParts, RpcError> {
        self.host
            .get()
            .ok_or_else(|| RpcError::internal("db connections are not attached to a host"))
    }

    /// 프로젝트의 연결 설정을 읽는다(디스크에 쓰지 않는다). 설정을 읽지 못하면 `connections` 가 그 이유다.
    pub(crate) fn load(
        &self,
        project_id: &str,
        worktree: Option<&str>,
    ) -> Result<Loaded, RpcError> {
        let host = self.host()?;
        let project = host.projects.get(project_id).map_err(registry_error)?;
        let root = active_root(&project, worktree)?;
        let config_error = |e: pw_core::config::ConfigError| RpcError::internal(e.to_string());
        let source = project
            .config_source(&host.paths, &root)
            .map_err(config_error)?;
        let local_source = project
            .config_source(&host.paths, project.root())
            .map_err(config_error)?;
        let local_dir = local_source.dir().to_path_buf();
        let connections = source
            .clone()
            .without_writes()
            .load()
            .map(|snapshot| connections_of(&snapshot))
            .map_err(|e| e.to_string());
        if let Ok(list) = &connections {
            // 파일 감시보다 목록 요청이 먼저 바뀐 값을 봐도 `config.changed` 를 낸다 — 열린 연결(M3-05)이 놓치지 않게.
            self.announce(project_id, worktree, list);
        }
        Ok(Loaded {
            project,
            root,
            source,
            local_dir,
            local_source,
            connections,
        })
    }

    /// 연결 하나의 설정을 이루는 파일 자리(지금 없는 것도): 활성 워크트리 설정 폴더의 `db/<id>.toml` · `db/<id>.local.toml`, 연결
    /// 워크트리면 메인 체크아웃의 `db/<id>.local.toml` 폴백(`ConfigSource` 의 병합 순서). 실행마다 설정을 다시 볼 때 이 파일들의
    /// 수정 시각 · 크기가 그대로면 다시 읽지 않는다(M3-05 리뷰 L5 — 다시 읽기는 프로젝트의 연결 파일 수에 비례한다).
    pub(crate) fn connection_files(
        &self,
        project_id: &str,
        worktree: Option<&str>,
        connection_id: &str,
    ) -> Result<Vec<PathBuf>, RpcError> {
        let host = self.host()?;
        let project = host.projects.get(project_id).map_err(registry_error)?;
        let root = active_root(&project, worktree)?;
        let source = project
            .config_source(&host.paths, &root)
            .map_err(|e| RpcError::internal(e.to_string()))?;
        let folder = FILE_PREFIX.trim_end_matches('/');
        let shared = format!("{connection_id}.toml");
        let local = format!("{connection_id}.local.toml");
        let mut files = vec![
            source.dir().join(folder).join(shared),
            source.dir().join(folder).join(&local),
        ];
        if let Some(fallback) = source.local_fallback_dir() {
            files.push(fallback.join(folder).join(local));
        }
        Ok(files)
    }

    /// 본 값을 기억하고, 전에 본 적이 있으면 바뀐 연결 id 를 돌려준다.
    fn remember(&self, key: SeenKey, list: &[LoadedConnection]) -> Vec<String> {
        let now: BTreeMap<String, Value> = list
            .iter()
            .map(|c| (c.id.clone(), c.value.clone()))
            .collect();
        let mut seen = lock(&self.seen);
        let Some(before) = seen.insert(key, now.clone()) else {
            return Vec::new();
        };
        let mut changed: Vec<String> = now
            .iter()
            .filter(|(id, value)| before.get(*id) != Some(value))
            .map(|(id, _)| id.clone())
            .chain(before.keys().filter(|id| !now.contains_key(*id)).cloned())
            .collect();
        changed.sort();
        changed
    }

    /// 본 값을 기억하고, 전에 본 값과 다른 연결을 `config.changed`(프로젝트 범위)로 낸다.
    fn announce(&self, project_id: &str, worktree: Option<&str>, list: &[LoadedConnection]) {
        let changed = self.remember((project_id.to_owned(), worktree.map(str::to_owned)), list);
        if changed.is_empty() {
            return;
        }
        let Ok(host) = self.host() else { return };
        let payload = ConfigChanged {
            changed: changed
                .iter()
                .map(|id| format!("{FILE_PREFIX}{id}"))
                .collect(),
            notices: Vec::new(),
        };
        tracing::info!(target: "pw_db", project = project_id, changed = ?payload.changed, "db connection config changed");
        if let Err(e) = host.bus.publish(
            Scope::Project(project_id.to_owned()),
            TOPIC_CHANGED,
            &payload,
        ) {
            tracing::warn!(target: "pw_db", %e, "cannot publish config.changed");
        }
    }

    /// 프로젝트의 연결 설정을 다시 읽어, 전에 본 값과 다른 연결을 `config.changed` 로 낸다. 한 번도 읽지 않은 프로젝트는 건너뛴다
    /// (아무도 목록을 들고 있지 않다).
    pub fn refresh(&self, project_id: &str, worktree: Option<&str>) {
        let Ok(host) = self.host() else { return };
        let key = (project_id.to_owned(), worktree.map(str::to_owned));
        if !lock(&self.seen).contains_key(&key) {
            return;
        }
        let Ok(project) = host.projects.get(project_id) else {
            return;
        };
        let Ok(root) = active_root(&project, worktree) else {
            return;
        };
        let Ok(source) = project.config_source(&host.paths, &root) else {
            return;
        };
        let Ok(snapshot) = source.without_writes().load() else {
            // 깨진 파일은 다음 목록 요청이 `problem` 으로 보인다. 이전 값을 그대로 둔다(설정 계층의 「이전 설정 유지」).
            return;
        };
        self.announce(project_id, worktree, &connections_of(&snapshot));
    }

    /// `approvals` 는 신뢰한 프로젝트에서만 있다(미신뢰면 상태를 정하지 않는다).
    fn entry(
        &self,
        loaded: &Loaded,
        connection: &LoadedConnection,
        approvals: Option<&Approvals>,
    ) -> DbConnectionEntry {
        let state = match (&connection.config, approvals) {
            (Err(message), _) => DbConnectionState::Invalid {
                message: message.clone(),
            },
            (Ok(_), None) => DbConnectionState::Untrusted,
            (Ok(config), Some(approvals)) => {
                self.readiness(loaded, &connection.id, config, approvals)
            }
        };
        DbConnectionEntry {
            id: connection.id.clone(),
            config: connection.config.as_ref().ok().cloned(),
            layers: connection.layers.clone(),
            read_only: connection.read_only,
            state,
        }
    }

    pub(crate) fn trusted(&self, project_id: &str) -> bool {
        self.host
            .get()
            .is_some_and(|h| h.projects.is_trusted(project_id))
    }

    /// `db.connection.list`.
    pub fn list(&self, params: &DbProjectParams) -> Result<DbConnectionList, RpcError> {
        let loaded = self.load(&params.project_id, params.worktree.as_deref())?;
        let trusted = self.trusted(&params.project_id);
        let (approvals, state_problem) = if trusted {
            let (approvals, problem) = self.approvals(&params.project_id)?;
            (Some(approvals), problem)
        } else {
            (None, None)
        };
        Ok(match &loaded.connections {
            Ok(list) => DbConnectionList {
                connections: list
                    .iter()
                    .map(|c| self.entry(&loaded, c, approvals.as_ref()))
                    .collect(),
                problem: None,
                state_problem,
                trusted,
            },
            Err(problem) => DbConnectionList {
                connections: Vec::new(),
                problem: Some(problem.clone()),
                state_problem,
                trusted,
            },
        })
    }

    /// `db.connection.get`.
    pub fn get(&self, params: &DbConnectionParams) -> Result<DbConnectionEntry, RpcError> {
        check_connection_id(&params.connection_id)?;
        let loaded = self.load(&params.project_id, params.worktree.as_deref())?;
        if let Err(problem) = &loaded.connections {
            return Err(RpcError::internal(format!(
                "cannot read the project config: {problem}"
            )));
        }
        let connection = loaded
            .get(&params.connection_id)
            .ok_or_else(|| not_found(&params.connection_id))?;
        let approvals = if self.trusted(&params.project_id) {
            Some(self.approvals(&params.project_id)?.0)
        } else {
            None
        };
        Ok(self.entry(&loaded, connection, approvals.as_ref()))
    }

    /// `db.connection.upsert`.
    pub fn upsert(
        &self,
        scope: &ConnectionScope,
        params: &DbConnectionUpsertParams,
    ) -> Result<DbConnectionEntry, RpcError> {
        require_project_window(scope, &params.project_id)?;
        check_connection_id(&params.connection_id)?;
        validate(&params.config).map_err(RpcError::invalid_params)?;
        {
            let _writing = lock(&self.writing);
            let loaded = self.load(&params.project_id, params.worktree.as_deref())?;
            if let Some(twin) = loaded.case_twin(&params.connection_id) {
                return Err(RpcError::new(
                    code::CONFLICT,
                    format!(
                        "db connection `{twin}` differs from `{}` only in case; edit `{twin}` instead",
                        params.connection_id
                    ),
                ));
            }
            let dir = match params.location {
                DbConfigLocation::Shared => loaded.source.dir(),
                DbConfigLocation::Local => loaded.local_dir.as_path(),
            };
            let file = config_file(dir, &params.connection_id, params.location);
            // 저장소 안의 개인 파일은 링크를 따르지 않고(링크 · 프로젝트 밖으로 풀리는 `.pitwall` 은 거절), 처음 만들면
            // `.pitwall/.gitignore` 에 `*.local.toml` 을 둔다(03 「하이브리드 정책 (D9)」, 15-m4-plan M4-17 리뷰 M1 · M2).
            let links = match params.location {
                DbConfigLocation::Shared => Links::Follow,
                DbConfigLocation::Local => loaded
                    .local_source
                    .prepare_local_write(&file)
                    .map_err(local_write_error)?,
            };
            let mut doc = match fs::read_to_string(&file) {
                Ok(text) => text.parse::<toml_edit::DocumentMut>().map_err(|e| {
                    RpcError::invalid_params(format!("cannot rewrite {}: {e}", file.display()))
                })?,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => toml_edit::DocumentMut::new(),
                Err(e) => return Err(io_error("read", &file, &e)),
            };
            let found = doc.get("schema").and_then(toml_edit::Item::as_integer);
            if found.is_some_and(|v| v > i64::from(SCHEMA)) {
                return Err(RpcError::new(
                    code::CONFLICT,
                    format!(
                        "{} uses a newer schema than this app; it is not rewritten",
                        file.display()
                    ),
                ));
            }
            write_document(&mut doc, &params.config);
            write_file_with(&file, &doc.to_string(), links)?;
            tracing::info!(target: "pw_db", project = %params.project_id, connection = %params.connection_id, path = %file.display(), "db connection config written");
        }
        self.refresh(&params.project_id, params.worktree.as_deref());
        self.get(&DbConnectionParams {
            project_id: params.project_id.clone(),
            worktree: params.worktree.clone(),
            connection_id: params.connection_id.clone(),
        })
    }

    /// `db.connection.delete`.
    pub fn delete(
        &self,
        scope: &ConnectionScope,
        params: &DbConnectionParams,
    ) -> Result<(), RpcError> {
        require_project_window(scope, &params.project_id)?;
        check_connection_id(&params.connection_id)?;
        {
            let _writing = lock(&self.writing);
            let loaded = self.load(&params.project_id, params.worktree.as_deref())?;
            // 정확히 그 id 의 연결만 — 대소문자를 가리지 않는 파일 시스템에서 `prod` 로 `Prod.toml` 을 지우지 않는다.
            if loaded.get(&params.connection_id).is_none() {
                return Err(not_found(&params.connection_id));
            }
            let files = [
                config_file(
                    loaded.source.dir(),
                    &params.connection_id,
                    DbConfigLocation::Shared,
                ),
                config_file(
                    loaded.source.dir(),
                    &params.connection_id,
                    DbConfigLocation::Local,
                ),
                config_file(
                    &loaded.local_dir,
                    &params.connection_id,
                    DbConfigLocation::Local,
                ),
            ];
            for file in &files {
                match fs::remove_file(file) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(io_error("remove", file, &e)),
                }
            }
            self.forget(&params.project_id, &params.connection_id)?;
            tracing::info!(target: "pw_db", project = %params.project_id, connection = %params.connection_id, "db connection deleted");
        }
        self.refresh(&params.project_id, params.worktree.as_deref());
        Ok(())
    }
}

pub(crate) fn not_found(connection_id: &str) -> RpcError {
    RpcError::new(
        code::NOT_FOUND,
        format!("db connection `{connection_id}` does not exist"),
    )
}

fn io_error(what: &str, path: &Path, e: &std::io::Error) -> RpcError {
    RpcError::internal(format!("cannot {what} {}: {e}", path.display()))
}

/// 설정 폴더(`.pitwall/` 또는 개인 전용 폴더) 안의 연결 설정 파일 경로.
pub(crate) fn config_file(dir: &Path, connection_id: &str, location: DbConfigLocation) -> PathBuf {
    let name = match location {
        DbConfigLocation::Shared => format!("{connection_id}.toml"),
        DbConfigLocation::Local => format!("{connection_id}.local.toml"),
    };
    dir.join("db").join(name)
}

fn local_write_error(e: WriteError) -> RpcError {
    match e {
        WriteError::Refused { .. } => RpcError::new(code::CONFLICT, e.to_string()),
        e => RpcError::internal(e.to_string()),
    }
}

/// 같은 폴더의 임시 파일에 쓰고 rename 한다. 있는 파일은 `pw_core` 의 설정 쓰기(심링크면 대상을 고치고 권한을 옮긴다)로.
pub(crate) fn write_file(path: &Path, contents: &str) -> Result<(), RpcError> {
    write_file_with(path, contents, Links::Follow)
}

/// [`write_file`]. `Links::Refuse` 면 쓰기 직전에 `path` 가 링크인지 다시 보고 링크면 거절한다(저장소 안의 개인 파일).
fn write_file_with(path: &Path, contents: &str, links: Links) -> Result<(), RpcError> {
    if links == Links::Refuse
        && fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink())
    {
        return Err(RpcError::new(
            code::CONFLICT,
            format!(
                "{}: is a symbolic link; Pitwall does not write a personal setting through a link inside the repository",
                path.display()
            ),
        ));
    }
    if path.exists() {
        return pw_core::config::migrate::write_atomic(path, contents)
            .map_err(|e| io_error("write", path, &e));
    }
    let dir = path
        .parent()
        .ok_or_else(|| RpcError::internal(format!("{} has no folder", path.display())))?;
    fs::create_dir_all(dir).map_err(|e| io_error("create", dir, &e))?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    // 확장자가 `.toml` 이 아니어야 감시가 임시 파일을 설정으로 읽지 않는다.
    let tmp = dir.join(format!(".{name}.{}.pitwall-tmp", std::process::id()));
    let written = fs::write(&tmp, contents).and_then(|()| fs::rename(&tmp, path));
    if let Err(e) = written {
        let _ = fs::remove_file(&tmp);
        return Err(io_error("write", path, &e));
    }
    Ok(())
}

// --- 감시 --------------------------------------------------------------------------------------------------------

/// 파일 감시 이벤트 구독(모든 프로젝트의 `fs.changed`). Host 를 만든 직후에 연다.
pub fn subscribe_files(bus: &EventBus) -> Subscription {
    bus.subscribe(Filter::all().topics([FS_CHANGED]))
}

#[derive(Debug, Deserialize)]
struct FsChanged {
    /// 그 루트를 연 창의 활성 워크트리.
    #[serde(default)]
    worktree: Option<String>,
    #[serde(default)]
    paths: Vec<String>,
    #[serde(default)]
    rescan: bool,
}

/// 바뀐 경로에 `.pitwall` 폴더가 있는가(그 아래 또는 그 폴더 자신).
fn touches_config(paths: &[String]) -> bool {
    paths.iter().any(|p| {
        Path::new(p)
            .components()
            .any(|c| c.as_os_str() == pw_core::config::PROJECT_DIR)
    })
}

/// 구독이 닫힐 때까지 `.pitwall/` 이 바뀐 프로젝트의 연결 설정을 다시 읽는다([`Connections::refresh`]). 이벤트를 놓치면 읽어 둔
/// 프로젝트를 모두 다시 읽는다.
pub async fn follow_files(mut events: Subscription, connections: Arc<Connections>) {
    while let Some(received) = events.recv().await {
        let keys: Vec<SeenKey> = match received {
            Received::Event(event) => {
                let Scope::Project(project) = &event.scope else {
                    continue;
                };
                let Ok(changed) = serde_json::from_value::<FsChanged>(event.payload.clone()) else {
                    continue;
                };
                if !changed.rescan && !touches_config(&changed.paths) {
                    continue;
                }
                vec![(project.clone(), changed.worktree)]
            }
            Received::Lagged(n) => {
                tracing::warn!(target: "pw_db", dropped = n, "missed file watch events");
                lock(&connections.seen).keys().cloned().collect()
            }
        };
        let c = Arc::clone(&connections);
        let _ = tokio::task::spawn_blocking(move || {
            for (project, worktree) in keys {
                c.refresh(&project, worktree.as_deref());
            }
        })
        .await;
    }
}

/// 핸들러를 붙인다. 설정 · 키체인 읽기는 블로킹 스레드에서 한다.
pub fn register(router: &mut Router, connections: Arc<Connections>) {
    async fn blocking<T: Send + 'static>(
        f: impl FnOnce() -> Result<T, RpcError> + Send + 'static,
    ) -> Result<T, RpcError> {
        tokio::task::spawn_blocking(f)
            .await
            .map_err(|e| RpcError::internal(e.to_string()))?
    }
    let (list, get, upsert, delete) = (
        Arc::clone(&connections),
        Arc::clone(&connections),
        Arc::clone(&connections),
        Arc::clone(&connections),
    );
    router
        .unary(
            DbConnectionListMethod,
            move |_ctx, params: DbProjectParams| {
                let c = Arc::clone(&list);
                blocking(move || c.list(&params))
            },
        )
        .unary(DbConnectionGet, move |_ctx, params: DbConnectionParams| {
            let c = Arc::clone(&get);
            blocking(move || c.get(&params))
        })
        .unary(
            DbConnectionUpsert,
            move |ctx, params: DbConnectionUpsertParams| {
                let (c, scope) = (Arc::clone(&upsert), ctx.scope().clone());
                blocking(move || c.upsert(&scope, &params))
            },
        )
        .unary(
            DbConnectionDelete,
            move |ctx, params: DbConnectionParams| {
                let (c, scope) = (Arc::clone(&delete), ctx.scope().clone());
                blocking(move || c.delete(&scope, &params))
            },
        );
    crate::secret::register(router, Arc::clone(&connections));
    crate::settings::register(router, Arc::clone(&connections));
    crate::trust::register(router, connections);
}
