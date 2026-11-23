//! 직접 연결 후보 검색(18-m7-plan M7-24 ③): 로컬 Tailscale 의 온라인 기기, LAN mDNS 광고(`pw_link::quic::MDNS_SERVICE`). 어느
//! 쪽이든 없으면 메모(`notes`)를 남기고 사람이 주소를 친다. 후보마다 직접 연결 포트에 QUIC 핸드셰이크만 해 본다(`listening` — 누구인지는
//! 증명하지 않는다. 고른 뒤 코드 페어링의 PAKE · 기억한 머신 키가 판정한다).

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::time::Duration;

#[cfg(unix)]
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[cfg(unix)]
use crate::direct::http_body;
use crate::direct::{peer_address, tailscale_peers};
use crate::{RemoteDiscovered, RemoteDiscovery, RemoteDiscoverySource};

/// 로컬 Tailscale API 의 유닉스 소켓(Linux · macOS 의 오픈 소스 tailscaled).
#[cfg(unix)]
const TAILSCALE_SOCKETS: &[&str] = &[
    "/var/run/tailscale/tailscaled.sock",
    "/var/run/tailscaled.socket",
];
/// 소켓이 없을 때 부르는 CLI(macOS 의 앱 판은 소켓 대신 이것).
const TAILSCALE_CLIS: &[&str] = &[
    "tailscale",
    "/Applications/Tailscale.app/Contents/MacOS/Tailscale",
];
const TAILSCALE_TIMEOUT: Duration = Duration::from_secs(3);
/// mDNS 광고를 기다리는 시간.
const MDNS_WAIT: Duration = Duration::from_millis(1500);
/// 후보 하나의 QUIC 핸드셰이크 시한.
const PROBE_TIMEOUT: Duration = Duration::from_millis(800);
/// 핸드셰이크를 해 보는 Tailscale 기기 수 상한.
const MAX_PROBES: usize = 64;

pub async fn run() -> RemoteDiscovery {
    let mut notes = Vec::new();
    let (tailscale, lan) = tokio::join!(tailscale(), lan());
    let mut found = Vec::new();
    match tailscale {
        Ok(peers) => found.extend(peers),
        Err(why) => notes.push(why),
    }
    match lan {
        Ok(ads) => found.extend(ads),
        Err(why) => notes.push(why),
    }
    // 핸드셰이크(동시에).
    let mut probes = tokio::task::JoinSet::new();
    for (i, d) in found.iter().enumerate().take(MAX_PROBES) {
        let (address, port) = (d.address.clone(), d.port);
        probes.spawn(async move {
            let Ok(mut addrs) = tokio::net::lookup_host((address.as_str(), port)).await else {
                return (i, false);
            };
            match addrs.next() {
                Some(addr) => (i, pw_link::quic::probe(addr, PROBE_TIMEOUT).await),
                None => (i, false),
            }
        });
    }
    while let Some(done) = probes.join_next().await {
        if let Ok((i, up)) = done {
            found[i].listening = up;
        }
    }
    // 받는 쪽이 있는 후보를 먼저.
    found.sort_by_key(|d| (!d.listening, d.label.clone()));
    RemoteDiscovery { found, notes }
}

async fn tailscale_status() -> Result<Vec<u8>, String> {
    // 로컬 API 의 유닉스 소켓(Windows 는 소켓 대신 CLI — Windows 의 Tailscale 은 명명 파이프다).
    #[cfg(unix)]
    for path in TAILSCALE_SOCKETS {
        let Ok(Ok(mut stream)) =
            tokio::time::timeout(TAILSCALE_TIMEOUT, tokio::net::UnixStream::connect(path)).await
        else {
            continue;
        };
        let request = b"GET /localapi/v0/status HTTP/1.0\r\nHost: local-tailscaled.sock\r\n\r\n";
        let read = async {
            stream.write_all(request).await?;
            let mut out = Vec::new();
            stream.take(16 << 20).read_to_end(&mut out).await?;
            Ok::<_, std::io::Error>(out)
        };
        match tokio::time::timeout(TAILSCALE_TIMEOUT, read).await {
            Ok(Ok(response)) => return http_body(&response).map(<[u8]>::to_vec),
            _ => continue,
        }
    }
    for cli in TAILSCALE_CLIS {
        let mut command = tokio::process::Command::new(cli);
        command.args(["status", "--json"]).kill_on_drop(true);
        if let Ok(Ok(output)) = tokio::time::timeout(TAILSCALE_TIMEOUT, command.output()).await
            && output.status.success()
        {
            return Ok(output.stdout);
        }
    }
    Err("tailscale is not running on this computer (no local api, no cli); type the address".into())
}

async fn tailscale() -> Result<Vec<RemoteDiscovered>, String> {
    let body = tailscale_status().await?;
    let peers = tailscale_peers(&body)?;
    Ok(peers
        .iter()
        .filter_map(|p| {
            let address = peer_address(p)?;
            Some(RemoteDiscovered {
                source: RemoteDiscoverySource::Tailscale,
                label: if p.host_name.is_empty() {
                    address.clone()
                } else {
                    p.host_name.clone()
                },
                address,
                port: pw_link::quic::DEFAULT_PORT,
                listening: false,
            })
        })
        .collect())
}

async fn lan() -> Result<Vec<RemoteDiscovered>, String> {
    tokio::task::spawn_blocking(|| {
        let daemon = mdns_sd::ServiceDaemon::new().map_err(|e| format!("mdns: {e}"))?;
        let events = daemon
            .browse(pw_link::quic::MDNS_SERVICE)
            .map_err(|e| format!("mdns: {e}"))?;
        let deadline = std::time::Instant::now() + MDNS_WAIT;
        let mut seen: BTreeMap<SocketAddr, ()> = BTreeMap::new();
        while let Some(left) = deadline.checked_duration_since(std::time::Instant::now()) {
            match events.recv_timeout(left) {
                Ok(mdns_sd::ServiceEvent::ServiceResolved(info)) => {
                    // 광고에는 이름이 없다 — 주소로 보인다. IPv4 를 먼저.
                    let mut addrs: Vec<std::net::IpAddr> = info
                        .get_addresses()
                        .iter()
                        .map(|a| a.to_ip_addr())
                        .collect();
                    addrs.sort_by_key(|a| !a.is_ipv4());
                    if let Some(ip) = addrs.into_iter().find(|ip| !ip.is_loopback()) {
                        seen.insert(SocketAddr::new(ip, info.get_port()), ());
                    }
                }
                Ok(_) => {}
                Err(_) => break,
            }
        }
        let _ = daemon.stop_browse(pw_link::quic::MDNS_SERVICE);
        let _ = daemon.shutdown();
        Ok(seen
            .into_keys()
            .map(|addr| RemoteDiscovered {
                source: RemoteDiscoverySource::Lan,
                label: addr.ip().to_string(),
                address: addr.ip().to_string(),
                port: addr.port(),
                listening: false,
            })
            .collect())
    })
    .await
    .map_err(|e| e.to_string())?
}
