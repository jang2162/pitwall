//! 페어링 표 파일(`$DATA/relay/pairings.json` — M7-03)을 따라간다. 표를 쓰는 것은 그 머신의 사람 · 설치 명령(`pair` — M7-06 ·
//! M7-23)이고 데몬은 읽기만 한다 — 데몬 소켓으로 표를 바꾸는 길은 없다(`crate::command`). 파일이 바뀌면 다시 읽어 표를 바꾸고,
//! 빠지거나 role 이 바뀐 기기의 링크는 `pw_link` 가 끊는다.
//!
//! **쓰기**([`pair`] · [`unpair`] — `pitwall-relay pair --from-ssh` · `unpair`, 18-m7-plan M7-06 ⑧ ⑨)는 데몬 소켓 명령이 아니라
//! 그 프로세스가 파일에 직접 쓴다. 두 프로세스(두 Desktop 이 동시에 SSH 로 붙음)가 읽고-고치고-쓰는 사이에 서로의 키를 지우지
//! 않게 잠금 파일(`pairings.lock` — [`PAIRINGS_LOCK`], `flock`)을 쥐고 한다. 데몬은 잠그지 않는다 — 쓰기가 임시 파일 + rename 이라
//! 반쯤 쓴 파일을 읽지 않는다.

use std::fs::File;
use std::io;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use pw_link::pairing::{PAIRINGS_FILE, load, save};
use pw_link::{DeviceId, MachineId, Pairing, PairingTable, PublicKey, RelayKeyFile};
use pw_rpc::Role;

/// 다시 읽어 보는 간격.
pub const POLL_EVERY: Duration = Duration::from_secs(1);

pub fn file(data: &Path) -> PathBuf {
    data.join(PAIRINGS_FILE)
}

/// `pair` 가 새 키를 넣은 뒤 끝나기 전에 기다리는 시간 — 떠 있는 데몬이 파일을 다시 읽을 때까지([`POLL_EVERY`] 마다 본다). 기다리지
/// 않으면 Desktop 이 페어링 직후 붙을 때 데몬이 아직 옛 표로 거절한다(`Unpaired`). 데몬 소켓으로 다시 읽기를 청하는 길은 두지 않는다
/// — 페어링 표를 바꾸는 것은 소켓 명령이 아니다(M7-04 ⑦).
pub const SETTLE: Duration = Duration::from_millis(POLL_EVERY.as_millis() as u64 + 500);

/// 쓰기 잠금 파일의 이름(`$DATA/relay/` 아래).
pub const PAIRINGS_LOCK: &str = "pairings.lock";

/// 기기 이름에 받는 글자(`[A-Za-z0-9._-]`, 64자까지). 이름은 판정에 쓰지 않지만 기기 화면(M7-23)이 보이는 글이다.
pub fn valid_name(name: &str) -> bool {
    name.len() <= 64
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// [`pair`] 의 결과.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paired {
    /// 이 머신의 id(Relay 기기 키의 지문 — 메인 판정 M7-P6). 키가 없으면 이때 만든다.
    pub machine: MachineId,
    /// 표에 새로 넣었는가(이미 있으면 거짓 — role 을 바꾸지 않는다).
    pub added: bool,
}

/// SSH 자동 페어링(결정 M7-D13 · 18-m7-plan M7-06 ⑧): `key` 가 표에 없으면 `owner` 로 넣는다. 이미 있으면 그대로 둔다 — 사람이 낮춰
/// 둔 role(`readonly` 등)을 SSH 로 다시 붙는 것만으로 올리지 않는다. `data` 는 `$DATA/relay`.
pub fn pair(data: &Path, key: &PublicKey, name: &str) -> io::Result<Paired> {
    if !valid_name(name) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "the device name takes letters, digits, `.`, `_`, `-` (64 at most)",
        ));
    }
    let _lock = lock(data)?;
    let machine = machine_id(data)?;
    let path = file(data);
    let mut devices = load(&path)?;
    let added = !devices.iter().any(|p| p.key == *key);
    if added {
        devices.push(Pairing {
            key: *key,
            role: Role::Owner,
            name: name.to_owned(),
        });
        save(&path, &devices)?;
    }
    Ok(Paired { machine, added })
}

/// 기기 해제(18-m7-plan M7-06 ⑨): 지문이 `device` 인 키를 표에서 뺀다. 있었으면 참. 데몬은 파일을 다시 읽고 그 기기의 열린 링크를
/// 끊는다(`pw_link::PairingTable::replace`).
pub fn unpair(data: &Path, device: &DeviceId) -> io::Result<bool> {
    let _lock = lock(data)?;
    let path = file(data);
    let mut devices = load(&path)?;
    let before = devices.len();
    devices.retain(|p| p.key.id() != *device);
    if devices.len() == before {
        return Ok(false);
    }
    save(&path, &devices)?;
    Ok(true)
}

/// 이 머신의 id. 키 파일이 없으면 만든다 — 막 뜬 데몬이 같은 순간 만들었으면(`AlreadyExists`) 그것을 읽는다.
fn machine_id(data: &Path) -> io::Result<MachineId> {
    let file = RelayKeyFile::in_dir(data);
    match file.load_or_create() {
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => file.load_or_create(),
        found => found,
    }
    .map(|key| key.id())
}

/// 쓰기 잠금을 쥔다(기다린다). 버리면 놓인다.
fn lock(data: &Path) -> io::Result<File> {
    // 폴더는 `RelayKeyFile` · `save` 와 같은 확인(0700 · 심링크 아님)을 지난다 — 잠금 파일을 남의 폴더에 만들지 않게.
    pw_link::pairing::ensure_dir(data)?;
    let path = data.join(PAIRINGS_LOCK);
    let mut options = std::fs::OpenOptions::new();
    options.create(true).truncate(false).write(true);
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let file = options.open(&path)?;
    // SAFETY: 열린 파일의 fd 다. flock 은 메모리를 건드리지 않는다.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(file)
}

/// 읽은 때의 파일 표지(수정 시각 · 크기). [`follow`] 가 이것과 다르면 다시 읽는다.
pub type Stamp = Option<(SystemTime, u64)>;

/// 처음 읽는다. 읽지 못하면 빈 표다(fail-closed — 아무 기기도 붙지 못한다). 표지는 **읽기 전에** 잰다 — 읽은 뒤 · [`follow`] 가
/// 시작하기 전에 파일이 바뀌면(기기 해제) 그 바뀜이 「이미 본 것」 이 되지 않게.
pub fn initial(path: &Path) -> (PairingTable, Stamp) {
    let seen = stamp(path);
    (PairingTable::new(read(path)), seen)
}

fn read(path: &Path) -> Vec<pw_link::Pairing> {
    load(path).unwrap_or_else(|e| {
        tracing::warn!(%e, path = %path.display(), "cannot read the pairing table; no device is paired");
        Vec::new()
    })
}

fn stamp(path: &Path) -> Stamp {
    let meta = std::fs::symlink_metadata(path).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

/// 파일이 바뀔 때마다 표를 바꾼다(끝나지 않는다). `seen` 은 [`initial`] 이 돌려준 표지.
pub async fn follow(path: PathBuf, table: PairingTable, mut seen: Stamp) {
    loop {
        tokio::time::sleep(POLL_EVERY).await;
        // 다시 읽기 전에 잰다 — 읽는 사이의 바뀜은 다음 차례에 다시 읽는다.
        let now = stamp(&path);
        if now != seen {
            seen = now;
            table.replace(read(&path));
            tracing::info!(
                devices = table.snapshot().len(),
                "reloaded the pairing table"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use pw_link::DeviceKey;

    use super::*;

    fn pairing(key: &DeviceKey) -> Pairing {
        Pairing {
            key: key.public(),
            role: Role::Owner,
            name: "mac".into(),
        }
    }

    async fn until(what: &str, check: impl Fn() -> bool) {
        let deadline = tokio::time::Instant::now() + POLL_EVERY * 5;
        while !check() {
            assert!(tokio::time::Instant::now() < deadline, "{what}");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// 파일에서 기기를 빼면 데몬의 표에서도 빠진다(그 기기의 링크는 `pw_link` 가 `PairingChanged` 로 끊는다) — 처음 읽은 뒤 ·
    /// 따라가기 전에 바뀐 것도 놓치지 않는다.
    #[tokio::test]
    async fn an_unpaired_device_leaves_the_table_even_right_after_the_first_read() {
        let dir = tempfile::tempdir().unwrap();
        // `$DATA/relay` 자리(0700 — 저장이 확인한다).
        let path = file(&dir.path().join("relay"));
        let (a, b) = (
            DeviceKey::generate().unwrap(),
            DeviceKey::generate().unwrap(),
        );
        save(&path, &[pairing(&a), pairing(&b)]).unwrap();
        let (table, seen) = initial(&path);
        assert_eq!(table.role_of(&a.public()), Some(Role::Owner));

        // 처음 읽은 뒤 · 따라가기 전에 A 를 뺀다.
        save(&path, &[pairing(&b)]).unwrap();
        tokio::spawn(follow(path.clone(), table.clone(), seen));
        until("the unpaired device is still in the table", || {
            table.role_of(&a.public()).is_none()
        })
        .await;
        assert_eq!(table.role_of(&b.public()), Some(Role::Owner));

        // 따라가는 중의 해제.
        save(&path, &[]).unwrap();
        until("the second unpairing is missed", || {
            table.snapshot().is_empty()
        })
        .await;
    }

    /// SSH 자동 페어링: 없는 키는 owner 로 들고, 이미 있는 키는 role 을 바꾸지 않는다. 머신 id 는 이 머신의 기기 키 지문이다.
    #[test]
    fn pairing_adds_a_missing_key_as_owner_and_keeps_a_present_role() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("relay");
        let (a, b) = (
            DeviceKey::generate().unwrap(),
            DeviceKey::generate().unwrap(),
        );
        let first = pair(&data, &a.public(), "mac").unwrap();
        assert!(first.added);
        assert_eq!(
            first.machine,
            RelayKeyFile::in_dir(&data).load_or_create().unwrap().id()
        );
        assert_eq!(
            load(&file(&data)).unwrap(),
            vec![pairing(&a)],
            "only the paired key is in the table"
        );
        // 사람이 낮춘 role 은 SSH 로 다시 붙어도 그대로다.
        save(
            &file(&data),
            &[Pairing {
                role: Role::Readonly,
                ..pairing(&a)
            }],
        )
        .unwrap();
        let again = pair(&data, &a.public(), "mac").unwrap();
        assert!(!again.added);
        assert_eq!(again.machine, first.machine);
        assert_eq!(load(&file(&data)).unwrap()[0].role, Role::Readonly);
        // 다른 Desktop 의 키는 따로 든다.
        assert!(pair(&data, &b.public(), "other").unwrap().added);
        assert_eq!(load(&file(&data)).unwrap().len(), 2);
        assert!(pair(&data, &b.public(), "bad name;").is_err());
    }

    /// 해제는 그 지문의 키만 뺀다.
    #[test]
    fn unpairing_removes_only_that_device() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("relay");
        let (a, b) = (
            DeviceKey::generate().unwrap(),
            DeviceKey::generate().unwrap(),
        );
        pair(&data, &a.public(), "a").unwrap();
        pair(&data, &b.public(), "b").unwrap();
        assert!(unpair(&data, &a.id()).unwrap());
        assert!(!unpair(&data, &a.id()).unwrap());
        assert_eq!(
            load(&file(&data)).unwrap(),
            vec![Pairing {
                name: "b".into(),
                ..pairing(&b)
            }]
        );
    }

    /// 여러 프로세스가 동시에 키를 넣어도 서로의 키를 지우지 않는다(잠금 — 읽고 고치고 쓰는 사이).
    #[test]
    fn concurrent_pairings_keep_every_key() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("relay");
        pw_link::pairing::ensure_dir(&data).unwrap();
        let keys: Vec<DeviceKey> = (0..24).map(|_| DeviceKey::generate().unwrap()).collect();
        std::thread::scope(|s| {
            for key in &keys {
                let data = &data;
                s.spawn(move || pair(data, &key.public(), "mac").unwrap());
            }
        });
        let table = load(&file(&data)).unwrap();
        for key in &keys {
            assert!(
                table.iter().any(|p| p.key == key.public()),
                "a concurrent pairing lost a key ({} of {} left)",
                table.len(),
                keys.len()
            );
        }
    }
}
