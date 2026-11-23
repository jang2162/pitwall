//! LAN 검색 광고(18-m7-plan M7-24 ③): mDNS 로 「이 LAN 에 직접 연결을 받는 Relay 가 있다」 만 알린다.
//!
//! - 광고에 싣는 것은 **무작위 이름**(데몬이 뜰 때마다 새로 — `pw-<16 hex>`) · 같은 무작위 호스트 이름 · 포트 · 링크 프로토콜 판(TXT
//!   `pv`)뿐이다. **머신 이름(`relay.toml` 의 `name` · 호스트 이름) · 기기 키 지문(머신 id) · 사용자명은 싣지 않는다** — 같은 LAN 의
//!   누구든 광고를 읽는다. 사람이 어느 머신인지 고르는 것은 주소로 하고, 그 머신이 맞는지는 Noise · 기억한 머신 키(또는 코드 페어링의
//!   PAKE)가 판정한다 — 가짜 광고로 다른 Relay 에 붙게 해도 링크 · 페어링이 서지 않는다(`pw_link` 의 quic 시험).
//! - 광고는 듣는 LAN 주소의 인터페이스에서만 낸다(다른 인터페이스 — 공인 · Tailscale — 에는 응답하지 않는다). Tailscale 쪽 검색은
//!   Desktop 이 로컬 Tailscale API 의 기기 목록으로 한다.

use std::collections::HashMap;
use std::net::IpAddr;

/// 광고 하나의 내용(시험이 본다).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Advertisement {
    pub instance: String,
    pub host: String,
    pub port: u16,
    pub properties: HashMap<String, String>,
}

/// 무작위 표지(16 hex).
pub fn token() -> String {
    let mut bytes = [0u8; 8];
    // 난수를 못 얻으면 시각으로 — 드러나는 것은 없다(표지는 광고마다 다르기만 하면 된다).
    if getrandom::fill(&mut bytes).is_err() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        bytes = (nanos as u64).to_be_bytes();
    }
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// `token` · `port` 로 만든 광고.
pub fn advertisement(token: &str, port: u16) -> Advertisement {
    let instance = format!("pw-{token}");
    Advertisement {
        host: format!("{instance}.local."),
        instance,
        port,
        properties: HashMap::from([(
            pw_link::quic::MDNS_PROTOCOL_KEY.to_owned(),
            pw_link::PROTOCOL.to_string(),
        )]),
    }
}

/// 떠 있는 광고. 버리면 거둔다.
pub struct Advertiser {
    daemon: mdns_sd::ServiceDaemon,
    fullname: Option<String>,
    token: String,
    port: u16,
    addresses: Vec<IpAddr>,
}

impl Advertiser {
    pub fn new(port: u16) -> Result<Self, String> {
        let daemon = mdns_sd::ServiceDaemon::new().map_err(|e| e.to_string())?;
        // 모든 인터페이스를 끄고 듣는 LAN 주소의 인터페이스만 켠다([`Self::set`]).
        daemon
            .disable_interface(mdns_sd::IfKind::All)
            .map_err(|e| e.to_string())?;
        Ok(Self {
            daemon,
            fullname: None,
            token: token(),
            port,
            addresses: Vec::new(),
        })
    }

    /// 광고할 LAN 주소를 바꾼다. 비면 광고를 거둔다.
    pub fn set(&mut self, addresses: Vec<IpAddr>) {
        if addresses == self.addresses {
            return;
        }
        self.withdraw();
        for ip in &self.addresses {
            let _ = self.daemon.disable_interface(mdns_sd::IfKind::Addr(*ip));
        }
        self.addresses = addresses;
        if self.addresses.is_empty() {
            return;
        }
        for ip in &self.addresses {
            let _ = self.daemon.enable_interface(mdns_sd::IfKind::Addr(*ip));
        }
        let ad = advertisement(&self.token, self.port);
        match mdns_sd::ServiceInfo::new(
            pw_link::quic::MDNS_SERVICE,
            &ad.instance,
            &ad.host,
            &self.addresses[..],
            ad.port,
            ad.properties,
        ) {
            Ok(info) => {
                let fullname = info.get_fullname().to_owned();
                match self.daemon.register(info) {
                    Ok(()) => self.fullname = Some(fullname),
                    Err(e) => tracing::warn!(%e, "cannot advertise the relay on the LAN"),
                }
            }
            Err(e) => tracing::warn!(%e, "cannot build the relay LAN advertisement"),
        }
    }

    fn withdraw(&mut self) {
        if let Some(fullname) = self.fullname.take() {
            let _ = self.daemon.unregister(&fullname);
        }
    }
}

impl Drop for Advertiser {
    fn drop(&mut self) {
        self.withdraw();
        let _ = self.daemon.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 광고에는 무작위 이름 · 포트 · 프로토콜 판뿐이다 — 머신 이름 · 사용자명 · 키 지문이 없다. 표지는 광고마다 다르다.
    #[test]
    fn the_advertisement_carries_no_machine_name_user_or_key() {
        let key = pw_link::DeviceKey::generate().unwrap();
        let machine = key.id();
        let ad = advertisement(&token(), 7420);
        assert_eq!(ad.port, 7420);
        assert_eq!(
            ad.properties.keys().collect::<Vec<_>>(),
            [pw_link::quic::MDNS_PROTOCOL_KEY]
        );
        assert!(ad.instance.starts_with("pw-") && ad.instance.len() == 3 + 16);
        assert_eq!(ad.host, format!("{}.local.", ad.instance));
        let text = format!("{ad:?}");
        let mut private: Vec<String> = vec![machine.as_str().to_owned()];
        private.extend(
            ["USER", "LOGNAME", "HOSTNAME"]
                .iter()
                .filter_map(|v| std::env::var(v).ok())
                .filter(|v| v.len() >= 3),
        );
        if let Ok(host) = std::process::Command::new("hostname").output() {
            let host = String::from_utf8_lossy(&host.stdout).trim().to_owned();
            if host.len() >= 3 {
                private.push(host);
            }
        }
        for value in private {
            assert!(!text.contains(&value), "the advertisement carries {value}");
        }
        assert_ne!(token(), token());
    }
}
