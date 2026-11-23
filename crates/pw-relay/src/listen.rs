//! 직접 연결의 듣는 주소(18-m7-plan M7-24 ② · 08 「연결 방식」 의 Tailscale 연동): `relay.toml [listen] interfaces` 가 고른
//! 인터페이스의 주소 하나하나에 묶는다 — **와일드카드(`0.0.0.0` · `::`)에는 어떤 값으로도 묶지 않는다.**
//!
//! - `"tailscale"`: Tailscale 주소(100.64.0.0/10 · fd7a:115c:a1e0::/48)이면서 인터페이스 이름이 Tailscale 의 것(`tailscale*` —
//!   Linux, `utun*` — macOS)인 주소. 범위만 보면 ISP 의 CGNAT 주소(100.64/10)를 받는 바깥 인터페이스가 섞인다.
//! - `"lan"`: 사설 주소(10/8 · 172.16/12 · 192.168/16 · IPv6 ULA fc00::/7 — Tailscale 범위는 뺀다). 공인 주소 · 링크 로컬
//!   (169.254/16 · fe80::/10) · 루프백은 들지 않는다.
//! - `"all"`: 루프백 · 링크 로컬이 아닌 모든 인터페이스 주소(공인 주소 포함). **명시적으로만** — 데몬 로그에 경고를 남기고 Desktop
//!   기기 화면에 보인다(`relay.devices` 의 `listen`).
//! - 기본값은 `["tailscale", "lan"]`. 빈 목록이면 직접 연결을 받지 않는다(SSH 만).
//!
//! 인터페이스는 [`RESCAN_EVERY`] 마다 다시 읽는다 — 새 주소에 묶고, 사라진 주소의 엔드포인트는 닫는다(그 위의 링크는 끊기고 Desktop
//! 이 다시 붙는다). 포트를 다른 프로그램이 쥐고 있으면 그 주소는 건너뛰고 경고한다(`SO_REUSEADDR` 없이 묶는다 — 가리지 않는다).

use std::collections::BTreeMap;
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

/// 인터페이스를 다시 읽는 간격.
pub const RESCAN_EVERY: Duration = Duration::from_secs(10);

/// `[listen] interfaces` 의 값 하나.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Interfaces {
    Tailscale,
    Lan,
    All,
}

impl Interfaces {
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "tailscale" => Some(Self::Tailscale),
            "lan" => Some(Self::Lan),
            "all" => Some(Self::All),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tailscale => "tailscale",
            Self::Lan => "lan",
            Self::All => "all",
        }
    }
}

/// `relay.toml [listen]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListenConfig {
    pub port: u16,
    pub interfaces: Vec<Interfaces>,
    /// LAN 에 mDNS 광고를 낸다(`"lan"` 이나 `"all"` 을 들을 때만 뜻이 있다 — `crate::advertise`).
    pub advertise: bool,
}

impl Default for ListenConfig {
    fn default() -> Self {
        Self {
            port: pw_link::quic::DEFAULT_PORT,
            interfaces: vec![Interfaces::Tailscale, Interfaces::Lan],
            advertise: true,
        }
    }
}

impl ListenConfig {
    /// `"all"` 을 골랐는가(경고 대상).
    pub fn wide(&self) -> bool {
        self.interfaces.contains(&Interfaces::All)
    }

    /// 직접 연결을 받는가.
    pub fn enabled(&self) -> bool {
        !self.interfaces.is_empty()
    }

    /// LAN 에 광고하는가.
    pub fn advertises(&self) -> bool {
        self.advertise
            && self
                .interfaces
                .iter()
                .any(|i| matches!(i, Interfaces::Lan | Interfaces::All))
    }
}

/// 지금의 듣기(기기 화면 — `relay.devices` 의 `listen`). 복제는 같은 것을 본다.
#[derive(Clone, Default)]
pub struct ListenState(std::sync::Arc<std::sync::RwLock<Option<pw_link::devices::RelayListen>>>);

impl ListenState {
    pub fn get(&self) -> Option<pw_link::devices::RelayListen> {
        self.0
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// 설정과 지금 듣는 주소로 고친다.
    pub fn set(&self, config: &ListenConfig, addresses: &[SocketAddr]) {
        *self
            .0
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Some(pw_link::devices::RelayListen {
                port: config.port,
                interfaces: config
                    .interfaces
                    .iter()
                    .map(|i| i.as_str().to_owned())
                    .collect(),
                wide: config.wide(),
                addresses: addresses.iter().map(ToString::to_string).collect(),
            });
    }
}

/// 인터페이스 주소 하나.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Address {
    pub interface: String,
    pub ip: IpAddr,
}

/// 이 머신의 인터페이스 주소(루프백 포함 — 거르기는 [`select`] 가 한다).
pub fn scan() -> Vec<Address> {
    match if_addrs::get_if_addrs() {
        Ok(found) => found
            .into_iter()
            .map(|i| Address {
                ip: i.ip(),
                interface: i.name,
            })
            .collect(),
        Err(e) => {
            tracing::warn!(%e, "cannot read the network interfaces; not listening for direct links");
            Vec::new()
        }
    }
}

fn tailscale_range(ip: IpAddr) -> bool {
    match ip {
        // 100.64.0.0/10
        IpAddr::V4(v4) => {
            let [a, b, ..] = v4.octets();
            a == 100 && (b & 0xc0) == 64
        }
        // fd7a:115c:a1e0::/48
        IpAddr::V6(v6) => {
            let s = v6.segments();
            s[0] == 0xfd7a && s[1] == 0x115c && s[2] == 0xa1e0
        }
    }
}

fn tailscale_interface(name: &str) -> bool {
    name.starts_with("tailscale") || name.starts_with("utun")
}

/// Tailscale 의 주소인가(범위와 인터페이스 이름 둘 다).
pub fn is_tailscale(address: &Address) -> bool {
    tailscale_range(address.ip) && tailscale_interface(&address.interface)
}

fn link_local(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_link_local(),
        IpAddr::V6(v6) => (v6.segments()[0] & 0xffc0) == 0xfe80,
    }
}

fn private(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_private(),
        // ULA fc00::/7
        IpAddr::V6(v6) => (v6.segments()[0] & 0xfe00) == 0xfc00,
    }
}

/// 사설 LAN 주소인가(Tailscale 범위 · 루프백 · 링크 로컬 · 공인 주소가 아니다).
pub fn is_lan(address: &Address) -> bool {
    let ip = address.ip;
    !ip.is_loopback()
        && !ip.is_unspecified()
        && !link_local(ip)
        && !tailscale_range(ip)
        && private(ip)
}

/// 고른 인터페이스의 주소들(정렬 · 중복 없음). 루프백 · 와일드카드 · 링크 로컬 · 멀티캐스트는 어떤 값으로도 들지 않는다.
pub fn select(addresses: &[Address], interfaces: &[Interfaces]) -> Vec<IpAddr> {
    let mut out: Vec<IpAddr> = addresses
        .iter()
        .filter(|a| {
            let ip = a.ip;
            if ip.is_loopback() || ip.is_unspecified() || ip.is_multicast() || link_local(ip) {
                return false;
            }
            interfaces.iter().any(|kind| match kind {
                Interfaces::Tailscale => is_tailscale(a),
                Interfaces::Lan => is_lan(a),
                Interfaces::All => true,
            })
        })
        .map(|a| a.ip)
        .collect();
    out.sort();
    out.dedup();
    out
}

/// 지금 듣는 주소들과 그 엔드포인트. [`Self::reconcile`] 이 바라는 주소로 맞춘다.
pub struct Listening<L> {
    port: u16,
    open: BTreeMap<IpAddr, L>,
}

/// [`Listening::reconcile`] 의 결과(로그).
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Changes {
    pub opened: Vec<SocketAddr>,
    pub closed: Vec<SocketAddr>,
    /// 묶지 못한 주소와 까닭(포트를 다른 프로그램이 쥠 등).
    pub failed: Vec<(SocketAddr, String)>,
}

impl<L> Listening<L> {
    pub fn new(port: u16) -> Self {
        Self {
            port,
            open: BTreeMap::new(),
        }
    }

    /// 지금 듣는 주소.
    pub fn addresses(&self) -> Vec<SocketAddr> {
        self.open
            .keys()
            .map(|ip| SocketAddr::new(*ip, self.port))
            .collect()
    }

    /// `wanted` 에 없는 주소는 닫고(값을 버린다 — `close` 를 부른 뒤), 새 주소는 `bind` 로 연다. 열린 것 가운데 `wanted` 에 있는
    /// 것은 그대로 둔다(그 위의 연결이 끊기지 않게).
    pub fn reconcile(
        &mut self,
        wanted: &[IpAddr],
        mut bind: impl FnMut(SocketAddr) -> std::io::Result<L>,
        mut close: impl FnMut(&L),
    ) -> Changes {
        let mut changes = Changes::default();
        let gone: Vec<IpAddr> = self
            .open
            .keys()
            .filter(|ip| !wanted.contains(ip))
            .copied()
            .collect();
        for ip in gone {
            if let Some(listener) = self.open.remove(&ip) {
                close(&listener);
                changes.closed.push(SocketAddr::new(ip, self.port));
            }
        }
        for ip in wanted {
            if self.open.contains_key(ip) {
                continue;
            }
            let addr = SocketAddr::new(*ip, self.port);
            match bind(addr) {
                Ok(listener) => {
                    self.open.insert(*ip, listener);
                    changes.opened.push(addr);
                }
                Err(e) => changes.failed.push((addr, e.to_string())),
            }
        }
        changes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn address(interface: &str, ip: &str) -> Address {
        Address {
            interface: interface.to_owned(),
            ip: ip.parse().unwrap(),
        }
    }

    fn machine() -> Vec<Address> {
        vec![
            address("lo0", "127.0.0.1"),
            address("lo0", "::1"),
            address("en0", "192.168.1.20"),
            address("en0", "fe80::1"),
            address("en1", "203.0.113.7"), // 공인 주소
            address("en1", "2001:db8::7"), // 공인 IPv6
            address("en2", "169.254.3.4"), // 링크 로컬
            address("eth1", "100.70.1.2"), // ISP CGNAT — Tailscale 이름이 아니다
            address("utun4", "100.101.102.103"),
            address("utun4", "fd7a:115c:a1e0::1234"),
            address("tailscale0", "100.88.0.1"),
            address("docker0", "172.17.0.1"),
            address("en3", "fd12:3456::9"),
        ]
    }

    fn ips(list: &[&str]) -> Vec<IpAddr> {
        let mut out: Vec<IpAddr> = list.iter().map(|s| s.parse().unwrap()).collect();
        out.sort();
        out
    }

    /// 시험 ①: 기본값(`tailscale` · `lan`)의 듣는 주소는 Tailscale · 사설 주소뿐이다 — 공인 주소 · 와일드카드 · 루프백 · 링크 로컬 ·
    /// CGNAT 바깥 인터페이스는 없다.
    #[test]
    fn the_default_binds_tailscale_and_private_addresses_only() {
        let config = ListenConfig::default();
        assert_eq!(config.port, 7420);
        let chosen = select(&machine(), &config.interfaces);
        assert_eq!(
            chosen,
            ips(&[
                "192.168.1.20",
                "100.101.102.103",
                "fd7a:115c:a1e0::1234",
                "100.88.0.1",
                "172.17.0.1",
                "fd12:3456::9",
            ])
        );
        for ip in &chosen {
            assert!(!ip.is_unspecified() && !ip.is_loopback());
        }
        for public in ["203.0.113.7", "2001:db8::7", "100.70.1.2", "169.254.3.4"] {
            assert!(!chosen.contains(&public.parse().unwrap()), "{public}");
        }
    }

    #[test]
    fn each_kind_picks_its_own_addresses() {
        assert_eq!(
            select(&machine(), &[Interfaces::Tailscale]),
            ips(&["100.101.102.103", "fd7a:115c:a1e0::1234", "100.88.0.1"])
        );
        assert_eq!(
            select(&machine(), &[Interfaces::Lan]),
            ips(&["192.168.1.20", "172.17.0.1", "fd12:3456::9"])
        );
        assert!(select(&machine(), &[]).is_empty());
    }

    /// `"all"` 은 공인 주소를 넣지만 그래도 와일드카드 · 루프백 · 링크 로컬은 없다.
    #[test]
    fn all_is_explicit_and_still_never_a_wildcard() {
        let chosen = select(&machine(), &[Interfaces::All]);
        assert!(chosen.contains(&"203.0.113.7".parse().unwrap()));
        assert!(chosen.contains(&"100.70.1.2".parse().unwrap()));
        for ip in &chosen {
            assert!(!ip.is_unspecified() && !ip.is_loopback() && !link_local(*ip));
        }
        assert!(
            ListenConfig {
                interfaces: vec![Interfaces::All],
                ..ListenConfig::default()
            }
            .wide()
        );
        assert!(!ListenConfig::default().wide());
        // 기계가 와일드카드 주소를 인터페이스 주소로 내도 들지 않는다.
        assert!(select(&[address("x", "0.0.0.0")], &[Interfaces::All]).is_empty());
    }

    /// 인터페이스가 바뀌면 새 주소는 열고, 사라진 주소는 닫고, 남은 주소는 그대로 둔다. 묶지 못한 주소는 건너뛴다.
    #[test]
    fn reconcile_follows_interfaces_that_appear_and_vanish() {
        let mut listening: Listening<u32> = Listening::new(7420);
        let mut next = 0u32;
        let mut closed = Vec::new();
        let first = listening.reconcile(
            &ips(&["192.168.1.20", "100.88.0.1"]),
            |_| {
                next += 1;
                Ok(next)
            },
            |l| closed.push(*l),
        );
        assert_eq!(first.opened.len(), 2);
        // Wi-Fi 가 바뀌어 사설 주소가 바뀌고, 새 주소의 포트는 다른 프로그램이 쥐었다.
        let second = listening.reconcile(
            &ips(&["10.0.0.5", "100.88.0.1"]),
            |addr| {
                if addr.ip() == "10.0.0.5".parse::<IpAddr>().unwrap() {
                    Err(std::io::Error::from(std::io::ErrorKind::AddrInUse))
                } else {
                    next += 1;
                    Ok(next)
                }
            },
            |l| closed.push(*l),
        );
        assert_eq!(
            second.closed,
            vec!["192.168.1.20:7420".parse::<SocketAddr>().unwrap()]
        );
        assert_eq!(second.failed.len(), 1);
        assert!(second.opened.is_empty());
        assert_eq!(
            listening.addresses(),
            vec!["100.88.0.1:7420".parse::<SocketAddr>().unwrap()]
        );
        // 인터페이스가 모두 사라지면 아무 데서도 듣지 않는다.
        let third = listening.reconcile(&[], |_| Ok(0), |l| closed.push(*l));
        assert_eq!(third.closed.len(), 1);
        assert!(listening.addresses().is_empty());
        assert_eq!(closed.len(), 2);
    }

    #[test]
    fn advertising_needs_a_lan_choice() {
        assert!(ListenConfig::default().advertises());
        let tailnet_only = ListenConfig {
            interfaces: vec![Interfaces::Tailscale],
            ..ListenConfig::default()
        };
        assert!(!tailnet_only.advertises());
        assert!(
            !ListenConfig {
                advertise: false,
                ..ListenConfig::default()
            }
            .advertises()
        );
    }
}
