//! DB 전역 설정(`settings.toml` 의 `[db]`, 14-m3-plan.md M3-20 — 설정 창 「DB」 페이지): 그리드 보유 행 · 바이트 상한, 셀 상한,
//! Redis 응답 타임아웃 · 키 구분자, 연결 타임아웃의 기본값.
//!
//! - **Host 가 정본이다.** 범위 판정은 쓰기(`db.settings.set`)와 읽기 둘 다 여기서 한다. 쓰기는 범위 밖 값을 `INVALID_PARAMS` 로
//!   거절하고(`data` 는 [`DbSettingRefused`] — 창이 번역한다), 읽기는 사람이 손으로 고친 범위 밖 값(0 · 음수 · 아주 큰 값 ·
//!   소수 · 글)을 쓰지 않고 기본값을 쓴다(값마다 한 번 경고 로그). 어느 길로도 관문의 상한(행 · 바이트)이 꺼지지 않는다.
//! - 쓰기는 그 키 하나만 고친다(`pw_core::config::write` — 주석 · 다른 키 보존). 쓴 뒤 전역 설정을 바로 다시 읽어 `config.changed`
//!   가 나간다. 값은 쓰는 그때부터 다음 실행 · 다음 연결이 읽는다 — 이미 도는 실행 · 열린 연결은 그대로다.
//! - `[db] unattended_transaction_minutes`(M3-05)는 이 페이지에 없다 — 읽기는 세션(`crate::session`)이 한다.

use std::collections::HashSet;
use std::sync::{Arc, Mutex, OnceLock};

use pw_core::config::ConfigStore;
use pw_db_types::CellLimit;
use pw_rpc::{JsNumber, JsonValue, Router, RpcError, rpc_method};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::lock;

/// 설정 페이지의 항목. 파일의 키는 [`DbSettingKey::file_key`].
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
pub enum DbSettingKey {
    /// 그리드 보유 행 상한(결정 M3-D7).
    GridMaxRows,
    /// 그리드 보유 바이트 상한(받은 셀 바이트 합, M3-16).
    GridMaxBytes,
    /// 셀 하나의 상한(넘는 값은 잘려 온다, 결정 M3-D8).
    CellMaxBytes,
    /// Redis 응답 타임아웃(초, M3-09).
    RedisResponseTimeout,
    /// Redis 키 트리의 구분자 처음 값(M3-18 — 화면의 위 막대에서 바꾸는 것은 그 탭만).
    RedisKeySeparator,
    /// 연결 설정에 `connect_timeout` 이 없을 때 쓰는 연결 타임아웃(초, M3-14).
    ConnectTimeout,
}

/// 값의 단위(창이 칸 옆에 적는다).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum DbSettingUnit {
    Rows,
    Bytes,
    Seconds,
    Text,
}

/// 정수 항목의 범위와 기본값.
#[derive(Debug, Clone, Copy)]
struct IntSpec {
    min: i64,
    max: i64,
    default: i64,
}

const MIB: i64 = 1024 * 1024;

impl DbSettingKey {
    /// 화면의 차례.
    pub const ALL: [Self; 6] = [
        Self::GridMaxRows,
        Self::GridMaxBytes,
        Self::CellMaxBytes,
        Self::RedisResponseTimeout,
        Self::RedisKeySeparator,
        Self::ConnectTimeout,
    ];

    /// `settings.toml` 의 키(`표.키`). 설정 스냅숏에서도 이 경로로 읽는다.
    pub fn file_key(self) -> &'static str {
        match self {
            Self::GridMaxRows => crate::session::GRID_MAX_ROWS_KEY,
            Self::GridMaxBytes => crate::session::GRID_MAX_BYTES_KEY,
            Self::CellMaxBytes => "db.cell_max_bytes",
            Self::RedisResponseTimeout => "db.redis_response_timeout",
            Self::RedisKeySeparator => "db.redis_key_separator",
            Self::ConnectTimeout => "db.connect_timeout",
        }
    }

    pub fn unit(self) -> DbSettingUnit {
        match self {
            Self::GridMaxRows => DbSettingUnit::Rows,
            Self::GridMaxBytes | Self::CellMaxBytes => DbSettingUnit::Bytes,
            Self::RedisResponseTimeout | Self::ConnectTimeout => DbSettingUnit::Seconds,
            Self::RedisKeySeparator => DbSettingUnit::Text,
        }
    }

    /// 정수 항목의 범위(메인 판정 — 14-m3-plan.md M3-20 칸). 위쪽 상한은 「상한을 끄는」 값을 막는다: 행은 06 「성능 목표」 의
    /// 10만 행의 열 배, 바이트는 웹뷰 메모리(M3-16 이 64 MiB 를 고른 까닭 — 웹뷰는 글을 UTF-16 으로 들어 몇 배를 쓴다).
    fn int(self) -> Option<IntSpec> {
        let spec = |min, max, default| Some(IntSpec { min, max, default });
        match self {
            Self::GridMaxRows => spec(1, 1_000_000, crate::session::DEFAULT_GRID_MAX_ROWS as i64),
            Self::GridMaxBytes => spec(
                MIB,
                256 * MIB,
                crate::session::DEFAULT_GRID_MAX_BYTES as i64,
            ),
            Self::CellMaxBytes => spec(1024, 16 * MIB, CellLimit::DEFAULT.0 as i64),
            Self::RedisResponseTimeout => spec(1, 600, 30),
            Self::ConnectTimeout => spec(
                i64::from(*crate::config::CONNECT_TIMEOUT_RANGE.start()),
                i64::from(*crate::config::CONNECT_TIMEOUT_RANGE.end()),
                i64::from(crate::config::DEFAULT_CONNECT_TIMEOUT_SECS),
            ),
            Self::RedisKeySeparator => None,
        }
    }
}

/// 키 구분자의 기본값과 글자 수 상한.
pub const DEFAULT_KEY_SEPARATOR: &str = ":";
pub const MAX_KEY_SEPARATOR_CHARS: usize = 8;

/// 쓰기를 거절한 까닭(`INVALID_PARAMS` 의 `data`). 영어 `message` 는 로그용이고 창은 이것을 번역한다.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DbSettingRefused {
    pub key: DbSettingKey,
    pub reason: DbSettingRefusal,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum DbSettingRefusal {
    /// 정수가 아니다(소수 · 글 · 불 등).
    NotInteger,
    /// 범위 밖(양 끝 포함).
    Range {
        #[specta(type = JsNumber)]
        min: i64,
        #[specta(type = JsNumber)]
        max: i64,
    },
    /// 글이 아니다.
    NotText,
    /// 구분자가 비었거나 [`MAX_KEY_SEPARATOR_CHARS`] 글자를 넘거나 제어 문자 · 공백이 들었다.
    Separator {
        #[specta(type = JsNumber)]
        max: i64,
    },
}

impl DbSettingRefusal {
    fn message(&self, key: DbSettingKey) -> String {
        let name = key.file_key();
        match self {
            Self::NotInteger => format!("`{name}` must be an integer"),
            Self::Range { min, max } => format!("`{name}` must be {min} to {max}"),
            Self::NotText => format!("`{name}` must be a string"),
            Self::Separator { max } => format!(
                "`{name}` must be 1 to {max} characters without whitespace or control characters"
            ),
        }
    }
}

/// `value` 가 그 항목에 쓸 수 있는 값이면 파일에 쓸 TOML 값.
pub fn validate(key: DbSettingKey, value: &Value) -> Result<toml_edit::Value, DbSettingRefusal> {
    match key.int() {
        Some(spec) => {
            let n = value.as_i64().ok_or(DbSettingRefusal::NotInteger)?;
            if !(spec.min..=spec.max).contains(&n) {
                return Err(DbSettingRefusal::Range {
                    min: spec.min,
                    max: spec.max,
                });
            }
            Ok(toml_edit::Value::from(n))
        }
        None => {
            let text = value.as_str().ok_or(DbSettingRefusal::NotText)?;
            if !valid_separator(text) {
                return Err(DbSettingRefusal::Separator {
                    max: MAX_KEY_SEPARATOR_CHARS as i64,
                });
            }
            Ok(toml_edit::Value::from(text))
        }
    }
}

/// 키 구분자: 1 ~ [`MAX_KEY_SEPARATOR_CHARS`] 글자, 제어 문자 · 공백 없음.
fn valid_separator(text: &str) -> bool {
    let count = text.chars().count();
    (1..=MAX_KEY_SEPARATOR_CHARS).contains(&count)
        && !text.chars().any(|c| c.is_control() || c.is_whitespace())
}

/// 항목 하나의 지금 상태.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DbSettingItem {
    pub key: DbSettingKey,
    /// `settings.toml` 의 키(「파일로 열기」 · 안내에 적는다).
    pub file_key: String,
    pub unit: DbSettingUnit,
    /// 지금 쓰이는 값(파일 값이 틀렸거나 없으면 기본값).
    #[specta(type = JsonValue)]
    pub value: Value,
    #[specta(type = JsonValue)]
    pub default: Value,
    /// 정수 항목의 범위. 글 항목은 `None`.
    #[specta(type = Option<JsNumber>)]
    pub min: Option<i64>,
    #[specta(type = Option<JsNumber>)]
    pub max: Option<i64>,
    /// 파일에 이 키가 있다(틀렸어도). 창은 이것으로 「기본값으로」 단추를 켠다.
    pub configured: bool,
    /// 파일의 값이 틀려 기본값을 쓰고 있다 — 그 까닭.
    pub refused: Option<DbSettingRefusal>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DbSettingsView {
    /// [`DbSettingKey::ALL`] 의 차례.
    pub items: Vec<DbSettingItem>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DbSettingsSetParams {
    pub key: DbSettingKey,
    /// 새 값. `null` 이면 파일에서 그 키를 지운다(기본값으로).
    #[specta(type = Option<JsonValue>)]
    #[serde(default)]
    pub value: Option<Value>,
}

rpc_method! {
    /// 설정 창 「DB」 페이지의 항목(지금 값 · 기본값 · 범위 · 파일 값의 문제). 파일을 읽지 않는다 — 전역 설정의 지금 스냅숏이다.
    pub DbSettingsGet: "db.settings.get" {
        params: (),
        result: DbSettingsView,
        tier: Read,
        roles: [Owner],
    }

    /// 설정 창 「DB」 페이지의 항목 하나를 전역 `settings.toml` 의 `[db]` 에 쓴다(`value: null` 이면 그 키를 지운다). 범위는 Host 가
    /// 판정한다 — 범위 밖이면 쓰지 않고 `INVALID_PARAMS`(`data` 는 `DbSettingRefused`). 쓴 뒤 설정을 다시 읽고(`config.changed`)
    /// 새 상태를 돌려준다. 파일을 고칠 수 없으면(TOML 이 깨졌다 등) 쓰지 않고 오류다.
    pub DbSettingsSet: "db.settings.set" {
        params: DbSettingsSetParams,
        result: DbSettingsView,
        tier: Write,
        roles: [Owner],
    }
}

/// 전역 설정의 `[db]` 를 읽는 자리. 연결 관리자(`crate::config::Connections`)가 하나 갖고, 세션 · 실행 파이프라인은 그것을 읽는다.
#[derive(Default)]
pub struct DbSettings {
    store: OnceLock<ConfigStore>,
    /// 이미 경고한 (키, 틀린 값) — 실행마다 같은 경고를 남기지 않는다.
    warned: Mutex<HashSet<(DbSettingKey, String)>>,
}

impl DbSettings {
    /// 두 번째부터는 무시한다.
    pub fn attach(&self, store: ConfigStore) {
        let _ = self.store.set(store);
    }

    pub(crate) fn store(&self) -> Option<&ConfigStore> {
        self.store.get()
    }

    /// 파일의 값(있으면).
    fn raw(&self, key: DbSettingKey) -> Option<Value> {
        self.store
            .get()?
            .snapshot()
            .get("settings", key.file_key())
            .cloned()
    }

    /// 파일 값과 그것을 쓰지 않는 까닭.
    fn read(&self, key: DbSettingKey) -> (Option<Value>, Option<DbSettingRefusal>) {
        let Some(raw) = self.raw(key) else {
            return (None, None);
        };
        match validate(key, &raw) {
            Ok(_) => (Some(raw), None),
            Err(refusal) => {
                if lock(&self.warned).insert((key, raw.to_string())) {
                    tracing::warn!(target: "pw_db", key = key.file_key(), value = %raw, reason = %refusal.message(key), "ignoring a db setting, using the default");
                }
                (None, Some(refusal))
            }
        }
    }

    fn int(&self, key: DbSettingKey) -> i64 {
        let spec = key.int().expect("integer setting");
        self.read(key)
            .0
            .and_then(|v| v.as_i64())
            .unwrap_or(spec.default)
    }

    /// 그리드 보유 행 상한(1 ~ 100만, 기본 10만).
    pub fn grid_max_rows(&self) -> u64 {
        self.int(DbSettingKey::GridMaxRows) as u64
    }

    /// 그리드 보유 바이트 상한(1 ~ 256 MiB, 기본 64 MiB).
    pub fn grid_max_bytes(&self) -> u64 {
        self.int(DbSettingKey::GridMaxBytes) as u64
    }

    /// 셀 하나의 상한(1 KiB ~ 16 MiB, 기본 1 MiB).
    pub fn cell_limit(&self) -> CellLimit {
        CellLimit(self.int(DbSettingKey::CellMaxBytes) as usize)
    }

    /// Redis 응답 타임아웃(밀리초, 1 ~ 600 초, 기본 30 초).
    pub fn redis_response_timeout_ms(&self) -> u32 {
        (self.int(DbSettingKey::RedisResponseTimeout) as u32).saturating_mul(1000)
    }

    /// 연결 설정에 `connect_timeout` 이 없을 때의 연결 타임아웃(초, 1 ~ 600, 기본 10).
    pub fn connect_timeout_secs(&self) -> u32 {
        self.int(DbSettingKey::ConnectTimeout) as u32
    }

    /// Redis 키 구분자의 처음 값(기본 `:`).
    pub fn key_separator(&self) -> String {
        self.read(DbSettingKey::RedisKeySeparator)
            .0
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_else(|| DEFAULT_KEY_SEPARATOR.to_owned())
    }

    /// 설정 페이지에 보일 상태.
    pub fn view(&self) -> DbSettingsView {
        let items = DbSettingKey::ALL
            .iter()
            .map(|&key| {
                let (valid, refused) = self.read(key);
                let configured = self.raw(key).is_some();
                let (default, min, max) = match key.int() {
                    Some(spec) => (Value::from(spec.default), Some(spec.min), Some(spec.max)),
                    None => (Value::from(DEFAULT_KEY_SEPARATOR), None, None),
                };
                DbSettingItem {
                    key,
                    file_key: key.file_key().to_owned(),
                    unit: key.unit(),
                    value: valid.unwrap_or_else(|| default.clone()),
                    default,
                    min,
                    max,
                    configured,
                    refused,
                }
            })
            .collect();
        DbSettingsView { items }
    }

    /// `db.settings.set`: 판정 → 그 키만 쓰기 → 다시 읽기. 블로킹이다(파일 IO).
    pub fn set(&self, params: &DbSettingsSetParams) -> Result<DbSettingsView, RpcError> {
        let key = params.key;
        let value = match &params.value {
            None | Some(Value::Null) => None,
            Some(value) => Some(validate(key, value).map_err(|reason| {
                let message = reason.message(key);
                RpcError::invalid_params(message).with_data(
                    serde_json::to_value(DbSettingRefused { key, reason })
                        .expect("DbSettingRefused serializes"),
                )
            })?),
        };
        let store = self
            .store
            .get()
            .ok_or_else(|| RpcError::internal("the global settings are not attached"))?;
        let changed =
            pw_core::config::write::write_setting(store.source().dir(), key.file_key(), value)
                .map_err(|e| RpcError::internal(e.to_string()))?;
        if changed {
            tracing::info!(target: "pw_db", key = key.file_key(), "db setting written");
            store
                .reload()
                .map_err(|e| RpcError::internal(e.to_string()))?;
        }
        Ok(self.view())
    }
}

/// 핸들러를 붙인다. 쓰기는 블로킹 스레드에서 한다.
pub(crate) fn register(router: &mut Router, settings: Arc<crate::config::Connections>) {
    let get = Arc::clone(&settings);
    router
        .unary(DbSettingsGet, move |_ctx, ()| {
            let view = get.settings.view();
            async move { Ok(view) }
        })
        .unary(DbSettingsSet, move |_ctx, params: DbSettingsSetParams| {
            let c = Arc::clone(&settings);
            async move {
                tokio::task::spawn_blocking(move || c.settings.set(&params))
                    .await
                    .map_err(|e| RpcError::internal(e.to_string()))?
            }
        });
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn integer_settings_take_only_integers_inside_the_range() {
        use DbSettingKey::*;
        for (key, good, bad) in [
            (
                GridMaxRows,
                json!(1),
                vec![json!(0), json!(-1), json!(1_000_001)],
            ),
            (
                GridMaxRows,
                json!(1_000_000),
                vec![json!(i64::MAX), json!(i64::MIN)],
            ),
            (
                GridMaxBytes,
                json!(256 * MIB),
                vec![json!(0), json!(MIB - 1), json!(256 * MIB + 1)],
            ),
            (
                CellMaxBytes,
                json!(1024),
                vec![json!(1023), json!(16 * MIB + 1)],
            ),
            (RedisResponseTimeout, json!(600), vec![json!(0), json!(601)]),
            (ConnectTimeout, json!(1), vec![json!(0), json!(601)]),
        ] {
            assert!(validate(key, &good).is_ok(), "{key:?} {good}");
            for v in bad {
                assert!(
                    matches!(validate(key, &v), Err(DbSettingRefusal::Range { .. })),
                    "{key:?} {v}"
                );
            }
            for v in [
                json!(10.5),
                json!(1e18),
                json!(u64::MAX),
                json!("100"),
                json!(true),
                json!([1]),
            ] {
                assert_eq!(
                    validate(key, &v).err(),
                    Some(DbSettingRefusal::NotInteger),
                    "{key:?} {v}"
                );
            }
        }
    }

    #[test]
    fn the_separator_is_short_visible_text() {
        let key = DbSettingKey::RedisKeySeparator;
        for good in [":", "::", "/", "→", "12345678"] {
            assert!(validate(key, &json!(good)).is_ok(), "{good}");
        }
        for bad in ["", " ", "a b", "\t", "\u{0}", "123456789", ":\n"] {
            assert!(
                matches!(
                    validate(key, &json!(bad)),
                    Err(DbSettingRefusal::Separator { max: 8 })
                ),
                "{bad:?}"
            );
        }
        assert_eq!(
            validate(key, &json!(3)).err(),
            Some(DbSettingRefusal::NotText)
        );
    }

    #[test]
    fn every_key_has_a_distinct_file_key_under_db() {
        let keys: HashSet<_> = DbSettingKey::ALL.iter().map(|k| k.file_key()).collect();
        assert_eq!(keys.len(), DbSettingKey::ALL.len());
        assert!(keys.iter().all(|k| k.starts_with("db.")));
    }
}
