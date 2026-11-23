//! 직접 연결 듣기를 데몬에 잇는다(18-m7-plan M7-24 ① ②): `[listen]` 이 고른 인터페이스 주소마다 QUIC 엔드포인트를 열어
//! [`Daemon::serve_direct`] 로 받고, [`crate::listen::RESCAN_EVERY`] 마다 인터페이스를 다시 읽어 맞춘다. LAN 주소에는 mDNS 광고를
//! 낸다(`crate::advertise`). `"all"` 이면 시작할 때 경고를 남긴다(Desktop 기기 화면도 [`ListenState`] 로 경고한다).

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use pw_link::quic::QuicListener;

use crate::daemon::Daemon;
use crate::listen::{self, Address, ListenConfig, ListenState, Listening};

/// 열린 엔드포인트 하나와 그 받기 태스크.
struct Open {
    listener: Arc<QuicListener>,
    task: tokio::task::JoinHandle<()>,
}

/// 데몬이 끝날 때까지 돈다. `scan` 은 인터페이스 주소를 읽는다(시험은 가짜를 넣는다).
pub async fn run(
    daemon: Daemon,
    config: ListenConfig,
    codes: Arc<dyn pw_link::CodeBook>,
    state: ListenState,
    scan: impl Fn() -> Vec<Address> + Send + 'static,
) {
    if !config.enabled() {
        tracing::info!("relay.toml [listen] has no interfaces; not listening for direct links");
        return;
    }
    if config.wide() {
        tracing::warn!(
            port = config.port,
            "relay.toml [listen] interfaces has \"all\": direct links are accepted on every interface, public addresses included"
        );
    }
    let mut listening: Listening<Open> = Listening::new(config.port);
    let mut advertiser = if config.advertises() {
        match crate::advertise::Advertiser::new(config.port) {
            Ok(advertiser) => Some(advertiser),
            Err(e) => {
                tracing::warn!(%e, "cannot start the LAN advertisement; direct links still work by address");
                None
            }
        }
    } else {
        None
    };
    loop {
        let found = scan();
        let wanted = listen::select(&found, &config.interfaces);
        let changes = listening.reconcile(
            &wanted,
            |addr| {
                let listener = Arc::new(QuicListener::bind(addr)?);
                let task = tokio::spawn({
                    let (daemon, listener, codes) =
                        (daemon.clone(), Arc::clone(&listener), Arc::clone(&codes));
                    async move { daemon.serve_direct(listener, codes).await }
                });
                Ok(Open { listener, task })
            },
            |open| {
                open.listener.close();
                open.task.abort();
            },
        );
        for addr in &changes.opened {
            tracing::info!(%addr, "listening for direct links");
        }
        for addr in &changes.closed {
            tracing::info!(%addr, "an interface address went away; stopped listening there");
        }
        for (addr, why) in &changes.failed {
            tracing::warn!(%addr, %why, "cannot listen for direct links on this address (is the port taken?)");
        }
        let addresses = listening.addresses();
        state.set(&config, &addresses);
        if let Some(advertiser) = advertiser.as_mut() {
            advertiser.set(lan_addresses(&found, &addresses));
        }
        tokio::time::sleep(listen::RESCAN_EVERY).await;
    }
}

/// 듣는 주소 가운데 LAN 주소(광고는 LAN 에만).
fn lan_addresses(found: &[Address], listening: &[SocketAddr]) -> Vec<IpAddr> {
    listening
        .iter()
        .map(SocketAddr::ip)
        .filter(|ip| found.iter().any(|a| a.ip == *ip && listen::is_lan(a)))
        .collect()
}
