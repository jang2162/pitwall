//! 실 DB 시험 표지(14-m3-plan.md M3-01, 결정 M3-D16): `PW_TEST_PG_URL` · `PW_TEST_MYSQL_URL` · `PW_TEST_REDIS_URL` 이
//! 비어 있지 않으면 `cfg(pw_live_pg)` · `cfg(pw_live_mysql)` · `cfg(pw_live_redis)` 를 켠다.
//!
//! 실 DB 시험은 `#[cfg(pw_live_pg)]` 로 단다 — 환경 변수가 없으면 시험이 건너뛰어지는 것이 아니라 **목록에 없다**(`#[ignore]`
//! 는 목록에 남아 「건너뜀」 으로 보이고, 런타임에 `return` 하는 시험은 통과로 센다). 그래서 `cargo nextest run -E
//! 'test(/live_pg/)' --no-tests=fail` 은 URL 없이 돌리면 실패한다. 시험 이름에 `live_pg` · `live_mysql` · `live_redis` 를 넣는다.
//! MariaDB 는 MySQL 시험을 그 URL 로 한 번 더 돈다 — `scripts/db-live.mjs` 가 `PW_TEST_MARIADB_URL` 을 `PW_TEST_MYSQL_URL` 로
//! 넘겨 부른다.
//!
//! 이 파일은 `pw-db-driver` 와 드라이버 크레이트 넷의 빌드 스크립트다(각 `Cargo.toml` 의 `build`). 값이 바뀌면 다시 돌고 그
//! 크레이트가 다시 컴파일된다.

const MARKERS: [(&str, &str); 3] = [
    ("PW_TEST_PG_URL", "pw_live_pg"),
    ("PW_TEST_MYSQL_URL", "pw_live_mysql"),
    ("PW_TEST_REDIS_URL", "pw_live_redis"),
];

fn main() {
    let names: Vec<&str> = MARKERS.iter().map(|(_, cfg)| *cfg).collect();
    println!("cargo::rustc-check-cfg=cfg({})", names.join(", "));
    for (var, cfg) in MARKERS {
        println!("cargo::rerun-if-env-changed={var}");
        if std::env::var_os(var).is_some_and(|v| !v.is_empty()) {
            println!("cargo::rustc-cfg={cfg}");
        }
    }
}
