//! 직접 연결의 Desktop 쪽 도우미(18-m7-plan M7-24 ③ ④ · M7-23 판정 5): 코드로 페어링한 수동 배치 Relay 를 전역 설정의
//! `[[remote_hosts]]` 에 더하기, 로컬 Tailscale 의 기기 목록 읽기.
//!
//! - 코드 페어링이 끝나면 `nickname` · `host`(주소) · `ssh = false`(그리고 기본이 아니면 `direct_port`)로 항목 하나를 **덧붙인다** —
//!   다른 항목 · 주석 · 서식은 그대로다. 사람이 `[[remote_hosts]]` 를 인라인 배열로 썼으면 고치지 않고 거절한다(손으로 더하라고).
//! - Tailscale 은 로컬 API(`/localapi/v0/status` — 유닉스 소켓, 새 의존 없이 HTTP/1.0 한 번 — 메인 판정 M7-P14)를 먼저 보고,
//!   없으면(macOS 의 앱 판은 소켓이 없다) `tailscale status --json` 을 부른다. 온라인인 다른 기기만 후보다. 어느 기기에 Relay 가
//!   떠 있는지는 직접 연결 포트에 QUIC 핸드셰이크만 해 본다(`pw_link::quic::probe` — 누구인지는 증명하지 않는다).

use std::path::Path;

use serde::Deserialize;

/// 설정의 키(`pw_link::remote_hosts::KEY` — 그 모듈은 기능 `bootstrap` 에만 있다).
const KEY: &str = "remote_hosts";
/// 설정 파일의 이름(`pw_core::config::write::SETTINGS_FILE`).
const SETTINGS_FILE: &str = "settings.toml";
/// 쓰는 사이 파일이 바뀌면 다시 해 보는 수.
const ATTEMPTS: usize = 3;

/// 항목 이름(`nickname`)에 쓸 수 있는 글자: `[A-Za-z0-9._-]`, 64자까지.
pub fn valid_nickname(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// 주소(호스트 이름 · IP)에 쓸 수 있는 글자 — 공백 · 제어 문자 · `-` 로 시작하는 값 · `@` 는 받지 않는다.
pub fn valid_address(address: &str) -> bool {
    !address.is_empty()
        && address.len() <= 253
        && !address.starts_with('-')
        && address
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':' | '_' | '%'))
}

/// `config_dir/settings.toml` 의 `[[remote_hosts]]` 에 직접 연결 전용 항목을 덧붙인다. 같은 `nickname` 이 이미 있으면 거절한다.
pub fn add_direct_host(
    config_dir: &Path,
    nickname: &str,
    address: &str,
    port: u16,
) -> Result<(), String> {
    let path = config_dir.join(SETTINGS_FILE);
    for _ in 0..ATTEMPTS {
        let current = match std::fs::read_to_string(&path) {
            Ok(text) => Some(text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(format!("{}: {e}", path.display())),
        };
        let next = with_host(current.as_deref(), nickname, address, port)?;
        // 쓰기 직전에 다시 본다 — 그 사이 사람이 고쳤으면 새 내용으로 다시 만든다.
        let again = std::fs::read_to_string(&path).ok();
        if again != current {
            continue;
        }
        if current.is_none() {
            std::fs::create_dir_all(config_dir).map_err(|e| e.to_string())?;
            // 없던 파일 — 그 사이 다른 쪽이 만들었으면 그것을 읽어 다시 한다.
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(mut file) => {
                    use std::io::Write;
                    return file
                        .write_all(next.as_bytes())
                        .and_then(|()| file.sync_all())
                        .map_err(|e| format!("{}: {e}", path.display()));
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(format!("{}: {e}", path.display())),
            }
        }
        return pw_core::config::migrate::write_atomic(&path, &next)
            .map_err(|e| format!("{}: {e}", path.display()));
    }
    Err(format!(
        "{} kept changing while adding the host; try again",
        path.display()
    ))
}

fn with_host(
    current: Option<&str>,
    nickname: &str,
    address: &str,
    port: u16,
) -> Result<String, String> {
    let mut doc: toml_edit::DocumentMut =
        current
            .unwrap_or("schema = 1\n")
            .parse()
            .map_err(|e: toml_edit::TomlError| {
                format!("settings.toml does not parse: {}", e.message())
            })?;
    let mut table = toml_edit::Table::new();
    table.insert("nickname", toml_edit::value(nickname));
    table.insert("host", toml_edit::value(address));
    table.insert("ssh", toml_edit::value(false));
    if port != pw_link::quic::DEFAULT_PORT {
        table.insert("direct_port", toml_edit::value(i64::from(port)));
    }
    match doc.get_mut(KEY) {
        None => {
            let mut list = toml_edit::ArrayOfTables::new();
            list.push(table);
            doc.insert(KEY, toml_edit::Item::ArrayOfTables(list));
        }
        Some(toml_edit::Item::ArrayOfTables(list)) => {
            let taken = list.iter().any(|t| {
                t.get("nickname").and_then(toml_edit::Item::as_str) == Some(nickname)
                    || (t.get("nickname").is_none()
                        && t.get("host").and_then(toml_edit::Item::as_str) == Some(nickname))
            });
            if taken {
                return Err(format!("a remote host named `{nickname}` already exists"));
            }
            list.push(table);
        }
        Some(_) => {
            return Err(
                "remote_hosts in settings.toml is not written as [[remote_hosts]] tables; add the host by hand"
                    .to_owned(),
            );
        }
    }
    Ok(doc.to_string())
}

/// Tailscale 기기 하나(로컬 API 의 `Peer` 값 중 쓰는 칸).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct TailscalePeer {
    #[serde(default)]
    pub host_name: String,
    #[serde(rename = "DNSName", default)]
    pub dns_name: String,
    #[serde(rename = "TailscaleIPs", default)]
    pub tailscale_ips: Vec<String>,
    #[serde(default)]
    pub online: bool,
}

#[derive(Deserialize)]
struct TailscaleStatus {
    #[serde(rename = "Peer", default)]
    peer: std::collections::BTreeMap<String, TailscalePeer>,
}

/// `/localapi/v0/status` · `tailscale status --json` 의 몸통에서 온라인인 다른 기기들.
pub fn tailscale_peers(json: &[u8]) -> Result<Vec<TailscalePeer>, String> {
    let status: TailscaleStatus =
        serde_json::from_slice(json).map_err(|e| format!("tailscale status: {e}"))?;
    Ok(status.peer.into_values().filter(|p| p.online).collect())
}

/// 기기의 붙을 주소: MagicDNS 이름(끝 점을 뗀다), 없으면 첫 IPv4, 없으면 첫 주소.
pub fn peer_address(peer: &TailscalePeer) -> Option<String> {
    let dns = peer.dns_name.trim_end_matches('.');
    if !dns.is_empty() {
        return Some(dns.to_owned());
    }
    peer.tailscale_ips
        .iter()
        .find(|ip| ip.parse::<std::net::Ipv4Addr>().is_ok())
        .or_else(|| peer.tailscale_ips.first())
        .cloned()
}

/// HTTP/1.0 응답에서 몸통을 꺼낸다(상태가 200 일 때만).
pub fn http_body(response: &[u8]) -> Result<&[u8], String> {
    let split = response
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or("tailscale: a malformed answer")?;
    let head = String::from_utf8_lossy(&response[..split]);
    let status = head.lines().next().unwrap_or_default();
    if status.split_whitespace().nth(1) != Some("200") {
        return Err(format!("tailscale: {status}"));
    }
    Ok(&response[split + 4..])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 코드 페어링 뒤의 항목은 덧붙기만 한다 — 다른 항목 · 주석은 그대로, 같은 이름은 거절, 인라인 배열은 고치지 않는다.
    #[test]
    fn a_paired_host_is_appended_to_the_settings() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(SETTINGS_FILE);
        std::fs::write(
            &path,
            "schema = 1\n# my hosts\n[[remote_hosts]]\nnickname = \"build\"\nhost = \"build.ts.net\"\n",
        )
        .unwrap();
        add_direct_host(dir.path(), "mini", "192.168.1.20", 7420).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("# my hosts"));
        assert!(text.contains("nickname = \"build\""));
        assert!(
            text.contains("nickname = \"mini\"\nhost = \"192.168.1.20\"\nssh = false\n"),
            "{text}"
        );
        assert!(!text.contains("direct_port"));
        assert!(add_direct_host(dir.path(), "build", "10.0.0.1", 7420).is_err());
        add_direct_host(dir.path(), "box", "box.ts.net", 7500).unwrap();
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .contains("direct_port = 7500")
        );

        let inline = tempfile::tempdir().unwrap();
        std::fs::write(
            inline.path().join(SETTINGS_FILE),
            "schema = 1\nremote_hosts = [{ host = \"a\" }]\n",
        )
        .unwrap();
        assert!(add_direct_host(inline.path(), "mini", "1.2.3.4", 7420).is_err());

        let empty = tempfile::tempdir().unwrap();
        add_direct_host(&empty.path().join("new"), "mini", "1.2.3.4", 7420).unwrap();
        assert!(
            std::fs::read_to_string(empty.path().join("new").join(SETTINGS_FILE))
                .unwrap()
                .starts_with("schema = 1\n")
        );
    }

    #[test]
    fn names_and_addresses_are_checked() {
        assert!(valid_nickname("mini-2.lan"));
        assert!(!valid_nickname("a b") && !valid_nickname("") && !valid_nickname(&"x".repeat(65)));
        assert!(
            valid_address("192.168.1.20")
                && valid_address("mini.tail1234.ts.net")
                && valid_address("fe80::1%en0")
        );
        assert!(
            !valid_address("-oProxyCommand=x") && !valid_address("a b") && !valid_address("u@h")
        );
    }

    #[test]
    fn tailscale_status_gives_the_online_peers() {
        let json = br#"{"Self":{"HostName":"me"},"Peer":{
            "k1":{"HostName":"mini","DNSName":"mini.tail1234.ts.net.","TailscaleIPs":["100.101.1.2","fd7a:115c:a1e0::2"],"Online":true},
            "k2":{"HostName":"old","DNSName":"","TailscaleIPs":["fd7a:115c:a1e0::3","100.101.1.3"],"Online":true},
            "k3":{"HostName":"off","TailscaleIPs":["100.101.1.4"],"Online":false}}}"#;
        let peers = tailscale_peers(json).unwrap();
        assert_eq!(peers.len(), 2);
        let addresses: Vec<_> = peers.iter().filter_map(peer_address).collect();
        assert_eq!(addresses, ["mini.tail1234.ts.net", "100.101.1.3"]);
        assert!(tailscale_peers(b"not json").is_err());
        assert_eq!(
            http_body(b"HTTP/1.0 200 OK\r\nContent-Type: x\r\n\r\n{}").unwrap(),
            b"{}"
        );
        assert!(http_body(b"HTTP/1.0 403 Forbidden\r\n\r\n").is_err());
    }
}
