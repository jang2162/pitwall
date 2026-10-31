//! 드라이버 등록부. 코어는 이것으로만 드라이버를 고른다 — 드라이버 크레이트를 모르고 `DbDriver` trait 객체만 안다.
//!
//! 데스크톱 크레이트가 앱을 만들 때 드라이버를 한 줄씩 넣는다(`apps/desktop/src-tauri/src/db.rs`, M3-06 ~ M3-09).

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use pw_db_driver::DbDriver;
use pw_db_types::{Capabilities, DriverInfo};
use pw_rpc::{Router, rpc_method};
use serde::{Deserialize, Serialize};

#[derive(Default, Clone)]
pub struct DriverRegistry {
    drivers: BTreeMap<String, Arc<dyn DbDriver>>,
}

/// 등록을 거절한 이유.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegisterError {
    /// 같은 id 가 이미 있다.
    Duplicate(String),
    /// 지원 플래그가 갈래를 정확히 하나 고르지 않는다.
    InvalidCapabilities { id: String, reason: &'static str },
}

impl fmt::Display for RegisterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Duplicate(id) => write!(f, "db driver `{id}` is registered twice"),
            Self::InvalidCapabilities { id, reason } => {
                write!(f, "db driver `{id}` has invalid capabilities: {reason}")
            }
        }
    }
}

impl std::error::Error for RegisterError {}

impl DriverRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, driver: Arc<dyn DbDriver>) -> Result<(), RegisterError> {
        let id = driver.id().to_owned();
        if let Err(reason) = driver.capabilities().validate() {
            return Err(RegisterError::InvalidCapabilities { id, reason });
        }
        if self.drivers.contains_key(&id) {
            return Err(RegisterError::Duplicate(id));
        }
        self.drivers.insert(id, driver);
        Ok(())
    }

    /// 설정 파일의 `driver` 값으로 고른다.
    pub fn get(&self, id: &str) -> Option<Arc<dyn DbDriver>> {
        self.drivers.get(id).cloned()
    }

    /// 등록된 드라이버의 id · 방언 · 지원 플래그(id 순).
    pub fn infos(&self) -> Vec<DriverInfo> {
        self.drivers
            .values()
            .map(|d| DriverInfo {
                id: d.id().to_owned(),
                dialect: d.dialect(),
                capabilities: d.capabilities(),
            })
            .collect()
    }

    pub fn is_empty(&self) -> bool {
        self.drivers.is_empty()
    }
}

/// `db.driver.list` 의 항목 하나: 등록된 드라이버와 그 지원 플래그 전부.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DbDriverEntry {
    /// 드라이버 id — 연결 설정의 `driver` 값(`postgres` · `mysql` · `sqlite` · `redis`, 프로세스 밖 드라이버면 그 id).
    pub kind: String,
    pub capabilities: Capabilities,
}

rpc_method! {
    /// 등록된 드라이버와 지원 플래그(id 순). 연결 없이 읽는 정적 정보다 — 연결 편집 다이얼로그(M3-14)는 드라이버가 받는
    /// `sslmode`(`capabilities.sslModes`)로 나머지를 고를 수 없게 하고, DB 탐색기(M3-13)는 플래그로 트리 단계 · 기능을 정한다.
    /// 비밀 · 프로젝트와 상관없어 범위를 묻지 않는다. MCP 에 내지 않는다.
    pub DbDriverListMethod: "db.driver.list" {
        params: (),
        result: Vec<DbDriverEntry>,
        tier: Read,
        roles: [Owner],
    }
}

impl DriverRegistry {
    /// `db.driver.list` 의 답(id 순).
    pub fn entries(&self) -> Vec<DbDriverEntry> {
        self.drivers
            .values()
            .map(|d| DbDriverEntry {
                kind: d.id().to_owned(),
                capabilities: d.capabilities(),
            })
            .collect()
    }
}

/// `db.driver.list` 를 붙인다. 데스크톱이 앱의 등록부를 넘긴다(`apps/desktop/src-tauri/src/main.rs`).
pub fn register(router: &mut Router, drivers: DriverRegistry) {
    let entries = drivers.entries();
    router.unary(DbDriverListMethod, move |_ctx, (): ()| {
        let entries = entries.clone();
        async move { Ok(entries) }
    });
}

#[cfg(test)]
mod tests {
    use pw_db_driver::DbConnection;
    use pw_db_driver::fake::FakeDriver;
    use pw_db_types::Capabilities;

    use super::*;

    #[tokio::test]
    async fn drivers_are_found_by_id_and_used_through_the_trait() {
        let mut registry = DriverRegistry::new();
        assert!(registry.is_empty());
        registry.register(Arc::new(FakeDriver::new())).unwrap();
        let infos = registry.infos();
        assert_eq!(infos.len(), 1);
        assert_eq!(infos[0].id, "fake");
        assert!(infos[0].capabilities.sql);

        let driver = registry.get("fake").unwrap();
        let connected = driver.connect(FakeDriver::spec()).await.unwrap();
        assert!(connected.connection.matches(&driver.capabilities()));
        assert!(matches!(connected.connection, DbConnection::Sql(_)));
        assert!(registry.get("postgres").is_none());
    }

    #[test]
    fn duplicates_and_branchless_drivers_are_refused() {
        let mut registry = DriverRegistry::new();
        registry.register(Arc::new(FakeDriver::new())).unwrap();
        assert_eq!(
            registry.register(Arc::new(FakeDriver::new())),
            Err(RegisterError::Duplicate("fake".into()))
        );
        let neither = FakeDriver::with_capabilities(Capabilities {
            sql: false,
            ..FakeDriver::full_capabilities()
        });
        let mut fresh = DriverRegistry::new();
        assert!(matches!(
            fresh.register(Arc::new(neither)),
            Err(RegisterError::InvalidCapabilities { .. })
        ));
        assert!(fresh.is_empty());
    }
}
