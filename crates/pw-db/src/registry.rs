//! 드라이버 등록부. 코어는 이것으로만 드라이버를 고른다 — 드라이버 크레이트를 모르고 `DbDriver` trait 객체만 안다.
//!
//! 데스크톱 크레이트가 앱을 만들 때 드라이버를 한 줄씩 넣는다(`apps/desktop/src-tauri/src/db.rs`, M3-06 ~ M3-09).

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use pw_db_driver::DbDriver;
use pw_db_types::DriverInfo;

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
