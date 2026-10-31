use pw_db_types::{Secret, SocketTarget, TlsSettings};

use super::*;

fn spec(connect_to: Option<SocketTarget>) -> ConnectionSpec {
    let mut spec = ConnectionSpec::new(
        DriverKind::Postgres.id(),
        Target::Network {
            host: "db.internal".into(),
            port: 5432,
            connect_to,
        },
    );
    spec.user = Some("app".into());
    spec.password = Some(Secret::new("hunter2"));
    spec
}

#[tokio::test]
async fn a_tunnel_connects_to_the_local_port_but_keeps_the_db_host_for_tls() {
    let tunnelled = pg_config(
        &spec(Some(SocketTarget {
            host: "127.0.0.1".into(),
            port: 50_123,
        })),
        PgSslMode::Require,
    )
    .await
    .unwrap();
    assert_eq!(
        tunnelled.get_hosts(),
        &[tokio_postgres::config::Host::Tcp("db.internal".into())]
    );
    assert_eq!(
        tunnelled.get_hostaddrs(),
        &["127.0.0.1".parse::<IpAddr>().unwrap()]
    );
    assert_eq!(tunnelled.get_ports(), &[50_123]);

    let direct = pg_config(&spec(None), PgSslMode::Disable).await.unwrap();
    assert!(direct.get_hostaddrs().is_empty());
    assert_eq!(direct.get_ports(), &[5432]);
    assert_eq!(direct.get_password(), Some(&b"hunter2"[..]));
}

#[test]
fn sslmode_attempts_follow_libpq() {
    let order = |mode| {
        attempts(
            TlsPlan::new(&TlsSettings::mode(mode), &spec(None).target, &SslMode::ALL)
                .unwrap()
                .attempt,
        )
    };
    assert_eq!(order(SslMode::Disable), &[PgSslMode::Disable]);
    assert_eq!(
        order(SslMode::Allow),
        &[PgSslMode::Disable, PgSslMode::Require]
    );
    assert_eq!(
        order(SslMode::Prefer),
        &[PgSslMode::Prefer, PgSslMode::Disable]
    );
    for mode in [SslMode::Require, SslMode::VerifyCa, SslMode::VerifyFull] {
        assert_eq!(order(mode), &[PgSslMode::Require], "{mode:?}");
    }
    // 닿지 못한 것은 다른 방식으로 다시 해도 같다.
    assert!(!retry(&DbError::new(DbErrorKind::Connect, "refused")));
    assert!(retry(&DbError::new(
        DbErrorKind::Auth,
        "no pg_hba.conf entry"
    )));
}

#[test]
fn pool_keys_differ_by_password_and_mode() {
    let a = spec(None);
    let mut b = spec(None);
    b.password = Some(Secret::new("other"));
    assert_ne!(
        pool_key(&a, PgSslMode::Disable),
        pool_key(&b, PgSslMode::Disable)
    );
    assert_ne!(
        pool_key(&a, PgSslMode::Disable),
        pool_key(&a, PgSslMode::Require)
    );
    assert_eq!(
        pool_key(&a, PgSslMode::Disable),
        pool_key(&spec(None), PgSslMode::Disable)
    );
}

/// 고른 스키마는 연결 시작 옵션 `search_path` 로 간다(M3-15) — 따옴표 친 식별자, 공백 · `\` 는 이스케이프, `public` 이 뒤에 붙는다.
/// 스키마가 다르면 풀 열쇠도 다르다(풀의 연결을 다른 스키마의 세션이 받지 않게). 모든 세션의 옵션은
/// `standard_conforming_strings=on` 으로 시작한다(M3-17 리뷰 X2).
#[tokio::test]
async fn a_chosen_schema_goes_to_the_startup_search_path() {
    let mut chosen = spec(None);
    chosen.schema = Some("my \"bill\\ing".into());
    let config = pg_config(&chosen, PgSslMode::Disable).await.unwrap();
    assert_eq!(
        config.get_options(),
        Some(r#"-c standard_conforming_strings=on -c search_path="my\ ""bill\\ing",public"#)
    );
    assert_eq!(search_path_option("public"), r#"-c search_path="public""#);
    let plain = pg_config(&spec(None), PgSslMode::Disable).await.unwrap();
    assert_eq!(
        plain.get_options(),
        Some("-c standard_conforming_strings=on")
    );
    assert_ne!(
        pool_key(&chosen, PgSslMode::Disable),
        pool_key(&spec(None), PgSslMode::Disable)
    );
}

/// `standard_conforming_strings` 가 켜지지 않은 세션은 쓰지 않는다 — 옵션을 버리는 중계기 뒤에서 꺼진 채 붙으면 분류기와 서버가
/// 같은 글을 다르게 읽는다(M3-17 리뷰 X2). 거절 글에 까닭이 있다.
#[test]
fn a_session_without_standard_strings_is_refused() {
    assert!(require_standard_strings("on").is_ok());
    for value in ["off", "On", ""] {
        let error = require_standard_strings(value).unwrap_err();
        assert_eq!(error.kind, DbErrorKind::Unsupported, "{value}");
        assert!(
            error.message.contains("standard_conforming_strings"),
            "{error:?}"
        );
    }
}

#[test]
fn the_driver_is_a_valid_sql_driver_with_every_sslmode() {
    let driver = PgDriver::new();
    assert_eq!(driver.id(), "postgres");
    let capabilities = driver.capabilities();
    capabilities.validate().unwrap();
    assert_eq!(capabilities.ssl_modes, SslMode::ALL);
}

#[tokio::test]
async fn a_file_target_is_refused_before_connecting() {
    let spec = ConnectionSpec::new(
        DriverKind::Postgres.id(),
        Target::File {
            path: std::env::temp_dir().join("x.db"),
        },
    );
    let error = PgDriver::new().test(spec).await.unwrap_err();
    assert_eq!(error.kind, DbErrorKind::Invalid);
}
