//! 기기 관리(`relay.*` — 18-m7-plan M7-23 ③, `pw_link::devices`)의 몸통과 기기의 시각.
//!
//! - 시각은 페어링 표와 따로 `$DATA/relay/devices-seen.json`([`SEEN_FILE`], 0600 — 기기 id → 페어링 시각 · 마지막 접속)에 둔다
//!   (메인 판정 — 공유 struct `pw_link::Pairing` 에 칸을 더하지 않는다). 판정에 쓰지 않는다 — 기기 화면이 보일 뿐이다. 페어링 시각은
//!   `pair --from-ssh` · 코드 페어링이 적고, 마지막 접속은 데몬이 링크를 증명할 때 적는다. 읽지 못하면 비어 보인다.
//! - **해제는 표를 곧바로 바꾼다**: 파일에서 빼고(`crate::pairings::unpair` — 잠금) 데몬의 표를 그 파일로 바꾼다
//!   (`PairingTable::replace` — 1초 다시 읽기를 기다리지 않는다). 그 기기의 열린 링크 · 구독 · 터미널 붙기 · 멈춘 링크의 이어 붙기
//!   토큰이 함께 끊긴다(`pw_link::relay` 의 표 감시).

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use pw_link::devices::{DeviceAdmin, RelayDevice, RelayPairCode};
use pw_link::pairing::{load, read_private_file, write_private_file};
use pw_link::{DeviceId, PairingTable};
use pw_rpc::Role;
use serde::{Deserialize, Serialize};

use crate::codes::{Clock, issue};
use crate::pairings::{file as pairings_file, lock};

/// 기기 시각 파일의 이름(`$DATA/relay/` 아래).
pub const SEEN_FILE: &str = "devices-seen.json";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Seen {
    #[serde(default)]
    pub paired_at_ms: Option<u64>,
    #[serde(default)]
    pub last_seen_ms: Option<u64>,
}

#[derive(Default, Serialize, Deserialize)]
struct SeenFile {
    #[serde(default)]
    devices: BTreeMap<String, Seen>,
}

fn seen_path(data: &Path) -> PathBuf {
    data.join(SEEN_FILE)
}

/// 기기 시각들. 읽지 못하면 비어 있다(판정에 쓰지 않는다).
pub fn seen(data: &Path) -> BTreeMap<String, Seen> {
    read_private_file(&seen_path(data))
        .ok()
        .and_then(|text| serde_json::from_str::<SeenFile>(&text).ok())
        .map(|f| f.devices)
        .unwrap_or_default()
}

/// 잠금을 쥔 쪽이 부른다.
fn update_locked(data: &Path, change: impl FnOnce(&mut BTreeMap<String, Seen>)) -> io::Result<()> {
    let mut devices = seen(data);
    change(&mut devices);
    let text = serde_json::to_vec(&SeenFile { devices }).map_err(io::Error::other)?;
    write_private_file(&seen_path(data), &text)
}

fn warn(e: &io::Error) {
    tracing::warn!(target: "pw_relay", %e, "cannot write the device times");
}

/// 페어링 시각을 적는다 — 페어링 표의 잠금을 쥔 쪽(`pair --from-ssh` · 코드 페어링)이 부른다. 실패해도 페어링은 그대로다.
pub(crate) fn record_paired(data: &Path, device: &DeviceId, now_ms: u64) {
    if let Err(e) = update_locked(data, |devices| {
        devices.insert(
            device.to_string(),
            Seen {
                paired_at_ms: Some(now_ms),
                last_seen_ms: None,
            },
        );
    }) {
        warn(&e);
    }
}

/// 해제한 기기의 시각을 지운다(잠금을 쥔 쪽).
pub(crate) fn forget(data: &Path, device: &DeviceId) {
    if let Err(e) = update_locked(data, |devices| {
        devices.remove(device.as_str());
    }) {
        warn(&e);
    }
}

/// 링크가 증명됐을 때 마지막 접속을 적는다(데몬 — 잠금을 쥔다). 표에 없는 기기는 적지 않는다.
pub fn record_seen(data: &Path, device: &DeviceId, now_ms: u64) {
    let result = lock(data).and_then(|_lock| {
        let paired = load(&pairings_file(data))?
            .iter()
            .any(|p| p.key.id() == *device);
        if !paired {
            return Ok(());
        }
        update_locked(data, |devices| {
            devices.entry(device.to_string()).or_default().last_seen_ms = Some(now_ms);
        })
    });
    if let Err(e) = result {
        warn(&e);
    }
}

/// 데몬의 [`DeviceAdmin`].
pub struct RelayDevices {
    data: PathBuf,
    table: PairingTable,
    clock: Clock,
    listen: Option<crate::listen::ListenState>,
}

impl RelayDevices {
    /// `data` 는 `$DATA/relay`, `table` 은 데몬의 표.
    pub fn new(data: PathBuf, table: PairingTable, clock: Clock) -> Self {
        Self {
            data,
            table,
            clock,
            listen: None,
        }
    }

    /// 직접 연결 듣기를 기기 목록에 싣는다(M7-24 — `"all"` 경고).
    pub fn with_listen(mut self, listen: crate::listen::ListenState) -> Self {
        self.listen = Some(listen);
        self
    }
}

impl DeviceAdmin for RelayDevices {
    fn list(&self, caller: Option<&str>) -> io::Result<Vec<RelayDevice>> {
        // 데몬이 쥔 표가 판정의 표다(파일을 고치는 중이어도 지금 링크가 받는 것을 보인다).
        let times = seen(&self.data);
        Ok(self
            .table
            .snapshot()
            .iter()
            .map(|p| {
                let device = p.key.id().to_string();
                let time = times.get(&device).cloned().unwrap_or_default();
                RelayDevice {
                    you: caller == Some(device.as_str()),
                    device,
                    key: p.key.to_string(),
                    name: p.name.clone(),
                    role: p.role,
                    paired_at_ms: time.paired_at_ms,
                    last_seen_ms: time.last_seen_ms,
                }
            })
            .collect())
    }

    fn unpair(&self, device: &str) -> io::Result<bool> {
        let device = DeviceId::from_stored(device);
        let removed = crate::pairings::unpair(&self.data, &device)?;
        // 파일이 그대로여도(이미 없음) 데몬의 표를 파일에 맞춘다 — 곧바로.
        self.table.replace(load(&pairings_file(&self.data))?);
        if removed {
            tracing::info!(target: "pw_relay", %device, "unpaired a device");
        }
        Ok(removed)
    }

    fn pair_code(&self, role: Role, name: Option<&str>) -> io::Result<RelayPairCode> {
        let (code, expires_at_ms) = issue(&self.data, role, name, (self.clock)())?;
        tracing::info!(target: "pw_relay", ?role, "issued a pairing code");
        Ok(RelayPairCode {
            code: code.reveal().to_owned(),
            expires_at_ms,
        })
    }

    fn listen(&self) -> Option<pw_link::devices::RelayListen> {
        self.listen
            .as_ref()
            .and_then(crate::listen::ListenState::get)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use pw_link::DeviceKey;

    use super::*;

    fn data_dir() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("relay");
        pw_link::pairing::ensure_dir(&data).unwrap();
        (dir, data)
    }

    /// 마지막 접속은 페어링된 기기에만 적고, 목록이 페어링 시각과 함께 보인다. 해제하면 시각도 지운다.
    #[test]
    fn last_seen_is_recorded_for_paired_devices_only() {
        let (_dir, data) = data_dir();
        let (a, stranger) = (
            DeviceKey::generate().unwrap(),
            DeviceKey::generate().unwrap(),
        );
        crate::pairings::pair(&data, &a.public(), "mac").unwrap();
        record_seen(&data, &a.id(), 42);
        record_seen(&data, &stranger.id(), 43);
        let times = seen(&data);
        assert_eq!(times.len(), 1);
        assert_eq!(times[a.id().as_str()].last_seen_ms, Some(42));
        assert!(times[a.id().as_str()].paired_at_ms.is_some());

        let table = PairingTable::new(load(&pairings_file(&data)).unwrap());
        let admin = RelayDevices::new(data.clone(), table.clone(), Arc::new(|| 7));
        let list = admin.list(Some(a.id().as_str())).unwrap();
        assert_eq!(list.len(), 1);
        assert!(list[0].you);
        assert_eq!(list[0].last_seen_ms, Some(42));
        assert!(admin.unpair(a.id().as_str()).unwrap());
        assert!(
            table.snapshot().is_empty(),
            "the daemon table kept the device"
        );
        assert!(seen(&data).is_empty());
    }

    /// 코드를 만들 때 로그에는 role 만 남고 코드는 남지 않는다.
    #[test]
    fn issuing_a_code_logs_no_code() {
        let (_dir, data) = data_dir();
        let logs = Arc::new(Mutex::new(Vec::<u8>::new()));
        let sink = Arc::clone(&logs);
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_ansi(false)
            .with_writer(move || Writer(Arc::clone(&sink)))
            .finish();
        let issued = tracing::subscriber::with_default(subscriber, || {
            let admin =
                RelayDevices::new(data.clone(), PairingTable::new(Vec::new()), Arc::new(|| 0));
            admin.pair_code(Role::Mobile, Some("phone")).unwrap()
        });
        let text = String::from_utf8(logs.lock().unwrap().clone()).unwrap();
        assert!(text.contains("issued a pairing code"), "{text}");
        assert!(!text.contains(&issued.code), "{text}");
    }

    struct Writer(Arc<Mutex<Vec<u8>>>);

    impl std::io::Write for Writer {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
}
