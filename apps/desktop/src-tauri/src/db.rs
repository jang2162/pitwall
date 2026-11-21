//! 앱의 DB 드라이버 등록부. 내장 드라이버 목록은 공용 조립(`pw_host::db::drivers`)에 있고, 여기는 M3-21 측정 번들(feature
//! `perf-fake-db`, 기본은 꺼짐)만 `postgres` 자리에 가짜를 둔다(perf_fake_db.rs).

use pw_db::DriverRegistry;

/// Host 를 만들 때 넘기는 드라이버 등록부(`pw_host::HostParts::db_drivers`).
pub fn drivers() -> DriverRegistry {
    #[cfg(feature = "perf-fake-db")]
    if crate::perf_fake_db::requested() {
        return pw_host::db::drivers_with_postgres(crate::perf_fake_db::driver());
    }
    pw_host::db::drivers()
}
