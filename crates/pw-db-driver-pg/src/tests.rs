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
