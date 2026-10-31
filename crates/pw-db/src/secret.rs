//! 비밀 — 비밀마다 키체인 항목과 대상 묶기, `env` 참조(M3-02, 결정 M3-D12).
//!
//! - 키체인 항목 하나 = 비밀 하나: 서비스 [`SERVICE`], 계정 `<프로젝트 id>/<연결 id>/<키>`([`account`]).
//! - **대상 묶기**: 항목에는 값과 함께 저장할 때의 대상([`DbSecretTarget`] — 드라이버 · 호스트 · 포트 · 사용자 · 터널 `host_ref` ·
//!   `sslmode` 와 인증서 경로)을 적는다. 쓸 때 지금 설정의 대상과 다르면 그 비밀을 쓰지 않고 「비밀 필요」 로 답한다 — 커밋된 설정을
//!   누가 바꿔 호스트를 갈아 끼우거나 TLS 를 내려도 저장한 비밀이 사람 모르게 나가지 않는다.
//! - 값은 RPC 응답 · 로그 · 알림 어디에도 나가지 않는다: 값은 [`Secret`] 으로만 들고(`Debug` 가 가린다), 이 모듈의 로그는 계정 ·
//!   이유만 적는다. `db.secret.set` 의 입력 타입도 `Debug` 에서 값을 가린다.
//! - 키체인은 [`SecretStore`] 뒤에 있다. 앱은 [`KeyringStore`](OS 키체인), 시험은 [`MemorySecretStore`] 를 쓴다 — 시험이 사람의
//!   키체인을 건드리지 않는다.
//! - `env` 참조의 값은 [`EnvLookup`] 으로 그 프로젝트의 런타임 환경에서 읽는다(연결을 열 때만, [`crate::trust`]).

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fmt;
use std::path::Path;
use std::sync::Arc;

use pw_db_types::{DriverKind, Secret, SslMode};
use pw_rpc::{ConnectionScope, Router, RpcError, rpc_method};
use pw_runtime::Runtime;
use serde::{Deserialize, Serialize};

use crate::config::{
    Connections, DbConnectionConfig, DbPasswordRef, check_connection_id, lock, not_found,
    require_project_window,
};

/// 키체인 서비스 이름(14-m3-plan.md M3-02).
pub const SERVICE: &str = "dev.pitwall.db";
/// 비밀 값의 길이 상한(바이트).
pub const SECRET_LIMIT: usize = 64 * 1024;

/// 비밀의 종류. 지금은 비밀번호 하나다.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    Serialize,
    Deserialize,
    specta::Type,
    schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum DbSecretKey {
    Password,
}

impl DbSecretKey {
    pub const ALL: [Self; 1] = [Self::Password];

    pub fn id(self) -> &'static str {
        match self {
            Self::Password => "password",
        }
    }
}

/// 키체인 항목의 계정: `<프로젝트 id>/<연결 id>/<키>`. 프로젝트 id(16진수)와 연결 id 에는 `/` 가 없다.
pub fn account(project_id: &str, connection_id: &str, key: DbSecretKey) -> String {
    format!("{project_id}/{connection_id}/{}", key.id())
}

/// 비밀을 보내도 되는 대상. 이 중 하나라도 바뀌면 저장한 비밀을 쓰지 않는다.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DbSecretTarget {
    pub driver: DriverKind,
    pub host: Option<String>,
    /// 쓰이는 포트(적지 않았으면 드라이버 기본).
    pub port: Option<u16>,
    pub user: Option<String>,
    /// 터널 목적지(`ssh_tunnel.host_ref`).
    pub tunnel: Option<String>,
    /// 쓰이는 `sslmode`(적지 않았으면 기본값).
    pub sslmode: SslMode,
    pub sslrootcert: Option<String>,
    pub sslcert: Option<String>,
    pub sslkey: Option<String>,
}

impl DbSecretTarget {
    pub fn of(config: &DbConnectionConfig) -> Self {
        Self {
            driver: config.driver,
            host: config.host.clone(),
            port: config.effective_port(),
            user: config.user.clone(),
            tunnel: config.ssh_tunnel.as_ref().map(|t| t.host_ref.clone()),
            sslmode: config.effective_ssl_mode(),
            sslrootcert: config.sslrootcert.clone(),
            sslcert: config.sslcert.clone(),
            sslkey: config.sslkey.clone(),
        }
    }
}

/// 비밀을 쓰지 못하는 이유.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum SecretMissing {
    /// 키체인에 항목이 없다(또는 읽을 수 없는 항목).
    Missing,
    /// 저장할 때의 대상과 지금 설정이 다르다 — 그 비밀은 쓰지 않는다.
    TargetChanged,
    /// 키체인을 쓸 수 없다(잠김 · 서비스 없음 등).
    Unavailable,
}

/// 키체인 항목에 넣는 것(JSON). 값과 대상을 한 항목에 둔다 — 둘을 따로 고칠 수 없다.
#[derive(Serialize, Deserialize)]
struct Item {
    v: u32,
    target: DbSecretTarget,
    value: Secret,
}

/// 키체인 접근의 실패. 메시지에 값을 넣지 않는다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreError(pub String);

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for StoreError {}

/// 비밀 저장소(서비스 [`SERVICE`] 아래의 계정 → 문자열).
pub trait SecretStore: Send + Sync {
    /// 없으면 `Ok(None)`.
    fn get(&self, account: &str) -> Result<Option<Secret>, StoreError>;
    fn set(&self, account: &str, value: &Secret) -> Result<(), StoreError>;
    /// 없어도 `Ok`.
    fn delete(&self, account: &str) -> Result<(), StoreError>;
}

/// OS 키체인(`keyring` — macOS Keychain · Windows Credential Manager · Secret Service).
pub struct KeyringStore {
    service: String,
}

impl KeyringStore {
    /// 서비스 [`SERVICE`].
    pub fn new() -> Self {
        Self::with_service(SERVICE)
    }

    /// 다른 서비스 이름(사람이 고르는 시험 전용 이름 등).
    pub fn with_service(service: impl Into<String>) -> Self {
        Self {
            service: service.into(),
        }
    }

    fn entry(&self, account: &str) -> Result<keyring::Entry, StoreError> {
        keyring::Entry::new(&self.service, account).map_err(|e| StoreError(e.to_string()))
    }
}

impl Default for KeyringStore {
    fn default() -> Self {
        Self::new()
    }
}

impl SecretStore for KeyringStore {
    fn get(&self, account: &str) -> Result<Option<Secret>, StoreError> {
        match self.entry(account)?.get_password() {
            Ok(value) => Ok(Some(Secret::new(value))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(StoreError(e.to_string())),
        }
    }

    fn set(&self, account: &str, value: &Secret) -> Result<(), StoreError> {
        self.entry(account)?
            .set_password(value.expose())
            .map_err(|e| StoreError(e.to_string()))
    }

    fn delete(&self, account: &str) -> Result<(), StoreError> {
        match self.entry(account)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(StoreError(e.to_string())),
        }
    }
}

/// 메모리 저장소(시험용 — 사람의 키체인을 건드리지 않는다). 읽은 계정을 센다.
#[derive(Default)]
pub struct MemorySecretStore {
    items: std::sync::Mutex<BTreeMap<String, Secret>>,
    reads: std::sync::Mutex<Vec<String>>,
}

impl MemorySecretStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// 들어 있는 계정.
    pub fn accounts(&self) -> Vec<String> {
        lock(&self.items).keys().cloned().collect()
    }

    /// 항목의 원문(값과 대상의 JSON).
    pub fn raw(&self, account: &str) -> Option<String> {
        lock(&self.items)
            .get(account)
            .map(|s| s.expose().to_owned())
    }

    /// 항목을 그대로 넣는다(옛 형식 · 깨진 항목 시험).
    pub fn put_raw(&self, account: &str, raw: &str) {
        lock(&self.items).insert(account.to_owned(), Secret::new(raw));
    }

    /// 지금까지 읽은 계정(순서대로).
    pub fn reads(&self) -> Vec<String> {
        lock(&self.reads).clone()
    }
}

impl SecretStore for MemorySecretStore {
    fn get(&self, account: &str) -> Result<Option<Secret>, StoreError> {
        lock(&self.reads).push(account.to_owned());
        Ok(lock(&self.items).get(account).cloned())
    }

    fn set(&self, account: &str, value: &Secret) -> Result<(), StoreError> {
        lock(&self.items).insert(account.to_owned(), value.clone());
        Ok(())
    }

    fn delete(&self, account: &str) -> Result<(), StoreError> {
        lock(&self.items).remove(account);
        Ok(())
    }
}

/// 프로젝트의 런타임 환경에서 변수 하나를 읽는다(`env` 참조를 푸는 자리).
pub trait EnvLookup: Send + Sync {
    /// `root` 는 프로젝트 루트. 없으면 `None`.
    fn var(&self, project_id: &str, root: &Path, name: &str) -> Option<Secret>;
}

/// 앱의 런타임 환경(`pw_runtime::Runtime::env` — 로그인 셸 · mise 등). 신뢰하지 않은 프로젝트에서는 런타임이 제공자를 돌리지
/// 않지만, 그 전에 신뢰 게이트가 막는다.
pub struct RuntimeEnv(pub Arc<Runtime>);

impl EnvLookup for RuntimeEnv {
    fn var(&self, project_id: &str, root: &Path, name: &str) -> Option<Secret> {
        let resolved = self.0.env(project_id, root);
        let value = if name == "PATH" {
            Some(resolved.env.path.clone())
        } else {
            resolved.env.vars.get(OsStr::new(name)).cloned()
        };
        value.and_then(|v| v.into_string().ok()).map(Secret::new)
    }
}

/// 키체인 항목 하나를 읽은 결과.
pub(crate) enum Stored {
    Usable(Secret),
    Unusable(SecretMissing),
}

impl Connections {
    /// 저장한 비밀을 지금 설정의 대상으로 쓸 수 있는가. 대상이 다르면 값을 버린다.
    pub(crate) fn stored_secret(
        &self,
        project_id: &str,
        connection_id: &str,
        key: DbSecretKey,
        config: &DbConnectionConfig,
    ) -> Stored {
        let account = account(project_id, connection_id, key);
        let raw = match self.secrets.get(&account) {
            Ok(Some(raw)) => raw,
            Ok(None) => return Stored::Unusable(SecretMissing::Missing),
            Err(e) => {
                tracing::warn!(target: "pw_db", %account, %e, "cannot read the keychain");
                return Stored::Unusable(SecretMissing::Unavailable);
            }
        };
        let item: Item = match serde_json::from_str(raw.expose()) {
            Ok(item) => item,
            Err(_) => {
                tracing::warn!(target: "pw_db", %account, "unreadable keychain item; asking again");
                return Stored::Unusable(SecretMissing::Missing);
            }
        };
        if item.target != DbSecretTarget::of(config) {
            tracing::info!(target: "pw_db", %account, "the connection target changed since the secret was saved; not using it");
            return Stored::Unusable(SecretMissing::TargetChanged);
        }
        Stored::Usable(item.value)
    }

    /// `db.secret.set`.
    pub fn set_secret(
        &self,
        scope: &ConnectionScope,
        params: &DbSecretSetParams,
    ) -> Result<(), RpcError> {
        require_project_window(scope, &params.project_id)?;
        check_connection_id(&params.connection_id)?;
        if params.value.expose().len() > SECRET_LIMIT {
            return Err(RpcError::invalid_params(format!(
                "a secret is at most {SECRET_LIMIT} bytes"
            )));
        }
        let loaded = self.load(&params.project_id)?;
        let connection = loaded
            .get(&params.connection_id)
            .ok_or_else(|| not_found(&params.connection_id))?;
        let config = connection.config.as_ref().map_err(|e| {
            RpcError::invalid_params(format!("the connection config is invalid: {e}"))
        })?;
        if config.password != Some(DbPasswordRef::Keychain) {
            return Err(RpcError::invalid_params(
                "the connection does not keep its password in the keychain (`password = { secret_ref = \"keychain\" }`)",
            ));
        }
        let item = Item {
            v: 1,
            target: DbSecretTarget::of(config),
            value: params.value.clone(),
        };
        let raw = serde_json::to_string(&item).map_err(|e| RpcError::internal(e.to_string()))?;
        let account = account(&params.project_id, &params.connection_id, params.key);
        self.secrets
            .set(&account, &Secret::new(raw))
            .map_err(|e| RpcError::internal(format!("cannot write the keychain: {e}")))?;
        tracing::info!(target: "pw_db", %account, "db secret saved");
        Ok(())
    }

    /// `db.secret.clear`.
    pub fn clear_secret(
        &self,
        scope: &ConnectionScope,
        params: &DbSecretClearParams,
    ) -> Result<(), RpcError> {
        require_project_window(scope, &params.project_id)?;
        check_connection_id(&params.connection_id)?;
        let account = account(&params.project_id, &params.connection_id, params.key);
        self.secrets
            .delete(&account)
            .map_err(|e| RpcError::internal(format!("cannot write the keychain: {e}")))?;
        tracing::info!(target: "pw_db", %account, "db secret cleared");
        Ok(())
    }

    /// 연결을 지울 때 그 연결의 비밀을 모두 지운다.
    pub(crate) fn clear_all_secrets(&self, project_id: &str, connection_id: &str) {
        for key in DbSecretKey::ALL {
            let account = account(project_id, connection_id, key);
            if let Err(e) = self.secrets.delete(&account) {
                tracing::warn!(target: "pw_db", %account, %e, "cannot remove a db secret");
            }
        }
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DbSecretSetParams {
    pub project_id: String,
    pub connection_id: String,
    pub key: DbSecretKey,
    #[specta(type = String)]
    #[schemars(with = "String")]
    pub value: Secret,
}

impl fmt::Debug for DbSecretSetParams {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DbSecretSetParams")
            .field("project_id", &self.project_id)
            .field("connection_id", &self.connection_id)
            .field("key", &self.key)
            .field("value", &self.value)
            .finish()
    }
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DbSecretClearParams {
    pub project_id: String,
    pub connection_id: String,
    pub key: DbSecretKey,
}

rpc_method! {
    /// 비밀을 키체인에 넣는다 — 지금 설정의 대상(드라이버 · 호스트 · 포트 · 사용자 · 터널 · `sslmode` 와 인증서 경로)과 함께.
    /// 연결 설정이 `password = { secret_ref = "keychain" }` 이어야 한다. 그 프로젝트 창의 연결만. 값은 돌려주지 않는다.
    pub DbSecretSet: "db.secret.set" {
        params: DbSecretSetParams,
        result: (),
        tier: Write,
        roles: [Owner],
    }

    /// 비밀을 키체인에서 지운다(없어도 성공). 그 프로젝트 창의 연결만.
    pub DbSecretClear: "db.secret.clear" {
        params: DbSecretClearParams,
        result: (),
        tier: Write,
        roles: [Owner],
    }
}

pub(crate) fn register(router: &mut Router, connections: Arc<Connections>) {
    async fn blocking<T: Send + 'static>(
        f: impl FnOnce() -> Result<T, RpcError> + Send + 'static,
    ) -> Result<T, RpcError> {
        tokio::task::spawn_blocking(f)
            .await
            .map_err(|e| RpcError::internal(e.to_string()))?
    }
    let (set, clear) = (Arc::clone(&connections), connections);
    router
        .unary(DbSecretSet, move |ctx, params: DbSecretSetParams| {
            let (c, scope) = (Arc::clone(&set), ctx.scope().clone());
            blocking(move || c.set_secret(&scope, &params))
        })
        .unary(DbSecretClear, move |ctx, params: DbSecretClearParams| {
            let (c, scope) = (Arc::clone(&clear), ctx.scope().clone());
            blocking(move || c.clear_secret(&scope, &params))
        });
}
