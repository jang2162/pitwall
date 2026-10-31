//! DB 드라이버 등록부를 채우는 자리(14-m3-plan.md M3-01 「공유 부품의 주인」). 코어(`pw-db`)는 드라이버 크레이트를 모르므로
//! 드라이버는 여기서만 넣는다 — 드라이버 행(M3-06 ~ M3-09)이 자기 줄을 하나씩 더하고, 아래 시험의 id 목록에도 더한다:
//!
//! ```text
//! registry.register(Arc::new(pw_db_driver_pg::PgDriver::new())).expect("db driver ids are unique");
//! ```

use std::sync::Arc;

use pw_db::DriverRegistry;
use pw_db::config::Connections;
use pw_db::secret::{KeyringStore, RuntimeEnv};
use pw_runtime::Runtime;

/// 연결 설정 · 비밀 · 신뢰 게이트(M3-02). 비밀은 OS 키체인(서비스 `dev.pitwall.db`), `env` 참조는 그 프로젝트의 런타임 환경에서
/// 푼다. Host 를 만든 뒤 레지스트리를 붙인다(`Connections::attach`).
pub fn connections(runtime: &Arc<Runtime>) -> Arc<Connections> {
    Arc::new(Connections::new(
        Arc::new(KeyringStore::new()),
        Arc::new(RuntimeEnv(Arc::clone(runtime))),
    ))
}

/// 앱의 드라이버 등록부. 실행 파이프라인(M3-05)이 Host 를 만들 때 받는다.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the execution pipeline (M3-05) takes the registry"
    )
)]
pub fn drivers() -> DriverRegistry {
    let mut registry = DriverRegistry::new();
    registry
        .register(Arc::new(pw_db_driver_mysql::MysqlDriver::new()))
        .expect("db driver ids are unique");
    registry
        .register(Arc::new(pw_db_driver_pg::PgDriver::new()))
        .expect("db driver ids are unique");
    registry
        .register(Arc::new(pw_db_driver_redis::RedisDriver::new()))
        .expect("db driver ids are unique");
    registry
        .register(Arc::new(pw_db_driver_sqlite::SqliteDriver::new()))
        .expect("db driver ids are unique");
    registry
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_registry_holds_exactly_the_built_in_drivers() {
        // 등록은 갈래 검사 · 중복 검사를 지나야 한다(`DriverRegistry::register`). 드라이버 행이 자기 id 를 더한다.
        let ids: Vec<String> = super::drivers().infos().into_iter().map(|i| i.id).collect();
        assert_eq!(ids, ["mysql", "postgres", "redis", "sqlite"]);
    }
}
