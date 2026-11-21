//! 페어링 표(18-m7-plan M7-03 ⑧ · M7-02 ② · 결정 M7-D13): Relay 가 받는 Desktop 기기 키와 그 기기의 role.
//!
//! - 링크의 role 은 **링크가 증명한 기기의 role** 이다 — 표에 없는 키의 링크는 role 이 없고 RPC 를 하나도 받지 않는다.
//! - 표는 `$DATA/relay/pairings.json`([`PAIRINGS_FILE`] — 0600)이다. 키를 넣는 길(`pitwall-relay pair --from-ssh`, M7-06)은 **데몬
//!   소켓 명령이 아니라** 그 프로세스가 파일에 직접 쓰고 데몬이 다시 읽는다([`PairingTable::replace`]) — 소켓만 열 수 있는 프로세스는
//!   표를 바꾸지 못한다. 같은 uid 로 그 폴더에 쓸 수 있는 프로세스는 막지 못한다(결정 M7-D13 의 대가).
//! - **기기 해제 · role 변경은 그 기기의 열린 링크를 끊는다**([`PairingTable::watch`] — 링크가 표를 따라간다). 열린 구독 · 스트림은
//!   role 을 다시 보지 않으므로(M7-02 리뷰 권고 4) 끊는 것이 계약이다.

use std::io;
use std::path::Path;
use std::sync::Arc;

use pw_rpc::Role;
use serde::{Deserialize, Serialize};
use tokio::sync::watch;

use crate::device::{PublicKey, read_private};

/// 페어링 표 파일의 이름(`$DATA/relay/` 아래).
pub const PAIRINGS_FILE: &str = "pairings.json";

/// 페어링된 기기 하나.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pairing {
    pub key: PublicKey,
    pub role: Role,
    /// 사람이 읽는 이름(기기 화면 — M7-23). 판정에 쓰지 않는다.
    #[serde(default)]
    pub name: String,
}

/// 데몬이 쥐는 표. 복제는 같은 표를 가리킨다.
#[derive(Clone)]
pub struct PairingTable {
    tx: Arc<watch::Sender<Arc<Vec<Pairing>>>>,
}

impl PairingTable {
    pub fn new(pairings: Vec<Pairing>) -> Self {
        Self {
            tx: Arc::new(watch::Sender::new(Arc::new(dedup(pairings)))),
        }
    }

    /// 그 키의 role. 표에 없으면 `None`(증명되지 않은 기기).
    pub fn role_of(&self, key: &PublicKey) -> Option<Role> {
        self.tx
            .borrow()
            .iter()
            .find(|p| p.key == *key)
            .map(|p| p.role)
    }

    pub fn snapshot(&self) -> Arc<Vec<Pairing>> {
        Arc::clone(&self.tx.borrow())
    }

    /// 표를 통째로 바꾼다(데몬이 파일을 다시 읽었을 때). 빠지거나 role 이 바뀐 기기의 링크는 끊긴다.
    pub fn replace(&self, pairings: Vec<Pairing>) {
        self.tx.send_replace(Arc::new(dedup(pairings)));
    }

    /// 키 하나를 넣거나 role 을 바꾼다.
    pub fn upsert(&self, pairing: Pairing) {
        self.tx.send_modify(|table| {
            let mut next: Vec<Pairing> = table
                .iter()
                .filter(|p| p.key != pairing.key)
                .cloned()
                .collect();
            next.push(pairing);
            *table = Arc::new(next);
        });
    }

    /// 키 하나를 뺀다(해제). 있었으면 참.
    pub fn remove(&self, key: &PublicKey) -> bool {
        let mut removed = false;
        self.tx.send_if_modified(|table| {
            let next: Vec<Pairing> = table.iter().filter(|p| p.key != *key).cloned().collect();
            removed = next.len() != table.len();
            if removed {
                *table = Arc::new(next);
            }
            removed
        });
        removed
    }

    /// 표가 바뀔 때마다 깨는 수신기(링크가 자기 기기의 role 을 다시 본다).
    pub fn watch(&self) -> watch::Receiver<Arc<Vec<Pairing>>> {
        self.tx.subscribe()
    }
}

/// 같은 키가 둘이면 뒤의 것을 쓴다.
fn dedup(pairings: Vec<Pairing>) -> Vec<Pairing> {
    let mut out: Vec<Pairing> = Vec::with_capacity(pairings.len());
    for p in pairings {
        out.retain(|q| q.key != p.key);
        out.push(p);
    }
    out
}

#[derive(Serialize, Deserialize)]
struct PairingsFile {
    devices: Vec<Pairing>,
}

/// 페어링 표 파일을 읽는다. 없으면 빈 표다. 다른 사용자에게 열린 파일 · 심링크는 읽지 않는다.
pub fn load(path: &Path) -> io::Result<Vec<Pairing>> {
    match read_private(path) {
        Ok(text) => serde_json::from_str::<PairingsFile>(&text)
            .map(|f| f.devices)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string())),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e),
    }
}

/// 페어링 표 파일을 쓴다(같은 폴더의 임시 파일 0600 에 쓰고 이름을 바꾼다 — 읽는 쪽이 반쯤 쓴 파일을 보지 않는다). 폴더는 0700 으로
/// 만들거나 확인한다. 동시에 쓰는 두 프로세스의 잠금은 부르는 쪽(M7-06 의 `pair`)이 건다.
pub fn save(path: &Path, pairings: &[Pairing]) -> io::Result<()> {
    use std::io::Write;
    let dir = path
        .parent()
        .ok_or_else(|| io::Error::other("the pairing file has no folder"))?;
    crate::device::private_dir(dir)?;
    let text = serde_json::to_vec_pretty(&PairingsFile {
        devices: pairings.to_vec(),
    })
    .map_err(io::Error::other)?;
    let tmp = dir.join(format!(".{PAIRINGS_FILE}.{}.tmp", std::process::id()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options.open(&tmp)?;
    file.write_all(&text)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::DeviceKey;

    fn pairing(role: Role) -> Pairing {
        Pairing {
            key: DeviceKey::generate().unwrap().public(),
            role,
            name: "mac".into(),
        }
    }

    #[test]
    fn the_table_answers_the_role_of_a_paired_key_only() {
        let owner = pairing(Role::Owner);
        let table = PairingTable::new(vec![owner.clone()]);
        assert_eq!(table.role_of(&owner.key), Some(Role::Owner));
        let stranger = DeviceKey::generate().unwrap().public();
        assert_eq!(table.role_of(&stranger), None);

        table.upsert(Pairing {
            role: Role::Readonly,
            ..owner.clone()
        });
        assert_eq!(table.role_of(&owner.key), Some(Role::Readonly));
        assert_eq!(table.snapshot().len(), 1);
        assert!(table.remove(&owner.key));
        assert!(!table.remove(&owner.key));
        assert_eq!(table.role_of(&owner.key), None);
    }

    #[test]
    fn the_file_round_trips_and_is_private() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("relay").join(PAIRINGS_FILE);
        assert!(load(&path).unwrap().is_empty());
        let devices = vec![pairing(Role::Owner), pairing(Role::Readonly)];
        save(&path, &devices).unwrap();
        assert_eq!(load(&path).unwrap(), devices);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
    }
}
