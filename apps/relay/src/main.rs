//! `pitwall-relay` — 원격 머신의 Relay(18-m7-plan M7-04, `docs/design/08-remote-mobile.md` 「Relay」). 창이 없다 — 창이 하는 일
//! (승인 · askpass · 열기 · 신뢰 묻기)은 링크로 붙은 Desktop 이 한다(M7-12 · M7-15 · M7-20).
//!
//! - `pitwall-relay proxy`: SSH 가 띄운다. stdio 를 이 판의 데몬 소켓에 잇고, 데몬이 없으면 띄운다.
//! - `pitwall-relay daemon`: 데몬(앞에서 돈다 — `proxy` 가 `setsid` 로 띄운다). 한 판에 하나.
//! - `pitwall-relay status`: 떠 있는 데몬의 상태(JSON 한 줄).
//!
//! 몸통은 `crates/pw-relay` 다. 하위 명령은 M7-15(`askpass` · `open`) · M7-20(`mcp`)이 더한다.

use std::process::ExitCode;
use std::sync::Arc;

use pw_core::config::AppPaths;
use pw_link::{RelayIdentity, RelayKeyFile, RelayParts};
use pw_relay::command::{Command, NotYet};
use pw_relay::daemon::{Daemon, DaemonParts, Ended, MAX_UNPROVEN};
use tracing_subscriber::Layer as _;
use tracing_subscriber::fmt;
use tracing_subscriber::layer::SubscriberExt as _;

const USAGE: &str = "usage: pitwall-relay <proxy|daemon|status|--version>";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("proxy") => proxy(),
        Some("daemon") => daemon(),
        Some("status") => status(),
        Some("--version" | "version") => {
            println!(
                "pitwall-relay {} ({})",
                pw_relay::VERSION,
                pw_relay::RELEASE_CHANNEL
            );
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
}

fn paths() -> Result<AppPaths, ExitCode> {
    pw_relay::paths::from_env().map_err(|e| {
        eprintln!("pitwall-relay: cannot resolve the app folders: {e}");
        ExitCode::from(1)
    })
}

fn socket_path(paths: &AppPaths) -> Result<std::path::PathBuf, ExitCode> {
    pw_relay::socket::endpoint(
        paths,
        pw_relay::VERSION,
        &std::env::temp_dir(),
        pw_relay::socket::current_uid(),
    )
    .map_err(|e| {
        eprintln!("pitwall-relay: {e}");
        ExitCode::from(1)
    })
}

fn runtime(threads: usize) -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(threads)
        .enable_all()
        .build()
        .expect("tokio runtime")
}

fn proxy() -> ExitCode {
    let (paths, socket) = match paths().and_then(|p| socket_path(&p).map(|s| (p, s))) {
        Ok(found) => found,
        Err(code) => return code,
    };
    let uid = pw_relay::socket::current_uid();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    rt.block_on(async {
        let spawn = || {
            let exe = std::env::current_exe()?;
            pw_relay::proxy::spawn_daemon(&exe, &["daemon"])
        };
        let stream = match pw_relay::proxy::connect_or_spawn(&socket, uid, spawn).await {
            Ok(stream) => stream,
            Err(e) => {
                eprintln!(
                    "pitwall-relay proxy: {e}. The daemon log is under {}",
                    pw_relay::proxy::log_hint(&paths).display()
                );
                return ExitCode::from(1);
            }
        };
        match pw_relay::proxy::pipe(stream).await {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("pitwall-relay proxy: {e}");
                ExitCode::from(1)
            }
        }
    })
}

fn status() -> ExitCode {
    let socket = match paths().and_then(|p| socket_path(&p)) {
        Ok(socket) => socket,
        Err(code) => return code,
    };
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    let uid = pw_relay::socket::current_uid();
    match rt.block_on(pw_relay::command::request(
        &socket,
        uid,
        Command::Status,
        serde_json::Value::Null,
    )) {
        Ok(Ok(status)) => {
            println!("{status}");
            ExitCode::SUCCESS
        }
        Ok(Err(e)) => {
            eprintln!("pitwall-relay status: {e}");
            ExitCode::from(1)
        }
        Err(e) => {
            eprintln!("pitwall-relay status: no daemon of this version is running ({e})");
            ExitCode::from(3)
        }
    }
}

fn daemon() -> ExitCode {
    let paths = match paths() {
        Ok(paths) => paths,
        Err(code) => return code,
    };
    // 소켓 폴더(`$STATE/relay`)는 로그 폴더의 부모이기도 하다 — 로그가 umask 권한으로 먼저 만들지 않게 0700 으로 먼저 만든다.
    if let Err(e) = pw_relay::socket::private_dir(&paths.state, pw_relay::socket::current_uid()) {
        eprintln!("pitwall-relay daemon: {e}");
        return ExitCode::from(1);
    }
    init_logging(&paths);
    let home = pw_core::config::PathEnv::current().home;
    let config = match pw_relay::config::load(&pw_relay::paths::relay_toml(&paths), home.as_deref())
    {
        Ok(config) => config,
        Err(e) => {
            tracing::error!(%e, "cannot read relay.toml; not starting");
            eprintln!("pitwall-relay daemon: {e}");
            return ExitCode::from(78);
        }
    };
    for table in &config.not_yet {
        tracing::warn!(table = %table, "relay.toml has a table this relay does not use yet; ignoring it");
    }
    let socket = match socket_path(&paths) {
        Ok(socket) => socket,
        Err(code) => return code,
    };
    let uid = pw_relay::socket::current_uid();
    let lock = match pw_relay::socket::lock(&socket, uid) {
        Ok(Some(lock)) => lock,
        Ok(None) => {
            tracing::info!("another daemon of this version holds the socket; leaving");
            return ExitCode::SUCCESS;
        }
        Err(e) => {
            tracing::error!(%e, "cannot use the relay socket folder");
            eprintln!("pitwall-relay daemon: {e}");
            return ExitCode::from(1);
        }
    };
    let rt = runtime(2);
    let ended = rt.block_on(async {
        let listener = match pw_relay::socket::bind(&socket, uid, &lock) {
            Ok(listener) => listener,
            Err(e) => {
                tracing::error!(%e, "cannot open the relay socket");
                return Err(ExitCode::from(1));
            }
        };
        let key = match RelayKeyFile::in_dir(&paths.data).load_or_create() {
            Ok(key) => key,
            Err(e) => {
                tracing::error!(%e, "cannot read the relay device key");
                return Err(ExitCode::from(1));
            }
        };
        let pairings_file = pw_relay::pairings::file(&paths.data);
        let pairings = pw_relay::pairings::initial(&pairings_file);
        tokio::spawn(pw_relay::pairings::follow(pairings_file, pairings.clone()));
        let assembled = pw_relay::host::assemble(paths.clone(), &config, tokio::runtime::Handle::current());
        let host = Arc::new(assembled.host);
        let daemon = Daemon::new(DaemonParts {
            link: RelayParts {
                identity: Arc::new(RelayIdentity {
                    key: Arc::new(key),
                    versions: pw_relay::versions(),
                }),
                pairings,
                host: host.clone() as Arc<dyn pw_core::Host>,
                limits: pw_link::LinkLimits::default(),
            },
            activity: Arc::new(assembled.activity),
            narrow: Arc::new(NotYet),
            idle: config.idle,
            name: config.name.clone(),
            max_unproven: MAX_UNPROVEN,
        });
        tracing::info!(socket = %socket.display(), roots = config.roots.len(), "relay daemon listening");
        let shutdown = tokio_util::sync::CancellationToken::new();
        tokio::spawn(stop_on_signal(shutdown.clone()));
        let ended = daemon.run(listener, shutdown).await;
        drop(host);
        Ok(ended)
    });
    // 남은 터미널 · 실행의 프로세스 트리를 끝낸다(유휴로 끝났으면 남은 것이 없다).
    pw_run::shutdown();
    drop(lock);
    match ended {
        Ok(Ended::Idle) => {
            tracing::info!("relay daemon exits: idle");
            ExitCode::SUCCESS
        }
        Ok(Ended::Shutdown) => {
            tracing::info!("relay daemon exits: asked to stop");
            ExitCode::SUCCESS
        }
        Err(code) => code,
    }
}

async fn stop_on_signal(shutdown: tokio_util::sync::CancellationToken) {
    use tokio::signal::unix::{SignalKind, signal};
    let (Ok(mut term), Ok(mut int)) = (
        signal(SignalKind::terminate()),
        signal(SignalKind::interrupt()),
    ) else {
        return;
    };
    tokio::select! {
        _ = term.recv() => {}
        _ = int.recv() => {}
    }
    shutdown.cancel();
}

/// `$STATE/relay/logs/relay.<날짜>.log`(03 「로그 · 진단 (D22)」). 경고 이상은 stderr 에도(앞에서 띄운 데몬일 때 보인다).
fn init_logging(paths: &AppPaths) {
    let dir = pw_relay::paths::logs_dir(paths);
    let level = pw_diag::LogConfig::new(dir.clone()).level_from_env().level;
    let stderr = fmt::layer()
        .with_ansi(false)
        .with_writer(std::io::stderr)
        .with_filter(tracing::level_filters::LevelFilter::WARN.min(level));
    let file = pw_diag::RollingFile::open(
        dir,
        pw_relay::paths::LOG_COMPONENT,
        pw_diag::logging::DEFAULT_RETENTION_DAYS,
        Arc::new(pw_diag::SystemClock),
    );
    let registry = tracing_subscriber::registry().with(stderr);
    let installed = match file {
        Ok(file) => tracing::subscriber::set_global_default(
            registry.with(
                fmt::layer()
                    .with_ansi(false)
                    .with_writer(file)
                    .with_filter(level),
            ),
        ),
        Err(e) => {
            eprintln!("pitwall-relay: no log file ({e}); warnings go to stderr only");
            tracing::subscriber::set_global_default(registry)
        }
    };
    if installed.is_ok() {
        tracing::info!(version = pw_relay::VERSION, "pitwall-relay daemon starting");
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    /// Relay 바이너리가 링크한 메서드 선언이 생성물(schema.json)을 덮는다 — Relay 에서 뺀 `bundle.*` 말고는 모두 있고, 생성물에
    /// 없는 것도 없다(M7-02 리뷰 권고 3 — 앱의 `generated_rpc_covers_the_router` 와 같은 자리).
    #[test]
    fn generated_rpc_covers_the_relay_router_but_bundles() {
        let schema: serde_json::Value = serde_json::from_str(include_str!(
            "../../../packages/rpc-client/src/generated/schema.json"
        ))
        .unwrap();
        let generated: BTreeSet<String> = schema["methods"]
            .as_object()
            .expect("schema.json 의 methods 객체")
            .keys()
            .filter(|name| {
                !pw_host::RELAY_EXCLUDED_PREFIXES
                    .iter()
                    .any(|p| name.starts_with(p))
            })
            .cloned()
            .collect();
        let dir = tempfile::tempdir().unwrap();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let assembled = pw_relay::host::assemble(
            pw_relay::paths::relay_paths(&pw_core::config::AppPaths::under(dir.path())),
            &pw_relay::config::RelayConfig::default(),
            rt.handle().clone(),
        );
        let routed: BTreeSet<String> = assembled
            .host
            .router()
            .methods()
            .into_iter()
            .map(|m| m.name.to_owned())
            .collect();
        assert!(routed.len() > 100);
        assert_eq!(generated, routed);
    }

    /// 생성물의 토픽(`TOPICS`)이 모두 Relay 바이너리에 선언돼 있다 — 선언 크레이트를 링크하지 않으면 그 토픽 구독이 오류 없이
    /// 거절된다(M7-02 리뷰 권고 3).
    #[test]
    fn generated_topics_are_declared_in_the_relay() {
        let generated = include_str!("../../../packages/rpc-client/src/generated/rpc.ts");
        let block = generated
            .split_once("export const TOPICS")
            .expect("rpc.ts 의 TOPICS")
            .1
            .split_once("\n};")
            .expect("TOPICS 의 끝")
            .0;
        let topics: BTreeSet<&str> = block
            .lines()
            .filter_map(|line| line.trim().strip_prefix('"')?.split_once('"'))
            .map(|(name, _)| name)
            .collect();
        assert!(topics.len() > 10, "{topics:?}");
        let declared: BTreeSet<&str> = pw_rpc::events().into_iter().map(|e| e.name).collect();
        let missing: Vec<_> = topics.difference(&declared).collect();
        assert!(
            missing.is_empty(),
            "Relay 에 선언이 링크되지 않은 토픽 {missing:?}"
        );
    }
}
