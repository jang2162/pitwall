//! `db.driver.list`(M3-14 · M3-13): 등록된 드라이버의 id(`kind`)와 지원 플래그 전부. 범위를 묻지 않는 정적 정보다.

mod common;

use std::sync::Arc;

use common::Fixture;
use pw_db_driver::fake::FakeDriver;
use pw_db_types::{Capabilities, SqlDialect};
use pw_rpc::ConnectionScope;
use serde_json::Value;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_driver_list_carries_every_capability_of_each_driver() {
    let pg = FakeDriver::named("postgres", SqlDialect::Postgres);
    let lite = FakeDriver::with_capabilities(Capabilities {
        ssl_modes: Vec::new(),
        schemas: false,
        ..FakeDriver::full_capabilities()
    });
    let mut drivers = pw_db::DriverRegistry::new();
    drivers.register(Arc::new(pg.clone())).unwrap();
    drivers.register(Arc::new(lite.clone())).unwrap();
    // 미신뢰 프로젝트 · Host 범위 연결에서도 읽힌다.
    let f = Fixture::build(false, |router, _| {
        pw_db::registry::register(router, drivers)
    });
    for mut conn in [f.window(), f.connect(ConnectionScope::Host)] {
        let list = conn.call("db.driver.list", Value::Null).await.unwrap();
        let entries = list.as_array().expect("an array");
        let kinds: Vec<&str> = entries
            .iter()
            .map(|e| e["kind"].as_str().unwrap())
            .collect();
        assert_eq!(kinds, ["fake", "postgres"], "{list}");
        for (entry, driver) in entries.iter().zip([&lite, &pg]) {
            let capabilities: Capabilities =
                serde_json::from_value(entry["capabilities"].clone()).unwrap();
            assert_eq!(capabilities, pw_db_driver::DbDriver::capabilities(driver));
        }
        assert_eq!(
            entries[0]["capabilities"]["sslModes"],
            serde_json::json!([])
        );
        assert_eq!(entries[0]["capabilities"]["schemas"], false);
        assert_eq!(entries[1]["capabilities"]["schemas"], true);
    }
}
