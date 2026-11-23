//! 6자리 코드 페어링의 대기 코드(18-m7-plan M7-23 ② — `pw_link::pair`). 코드는 `$DATA/relay/pairing-code.json`([`CODE_FILE`],
//! 0600)에 하나만 둔다 — 새 코드를 만들면 앞 코드는 끝난다.
//!
//! - 만드는 길은 둘: 그 머신의 사람이 `pitwall-relay pair [--role <r>] [--name <n>]`(터미널에 코드), 붙은 owner 기기의 `relay.pairCode`
//!   (기기 화면에 코드 — `crate::devices`). 어느 것도 데몬 소켓 명령이 아니다(좁은 명령은 셋 그대로 — `crate::command`).
//! - **시도는 그 코드에 대해 Relay 전체에서 센다**(파일의 `attempts` — 연결마다가 아니다, 데몬이 다시 떠도 이어진다). 시도는 PAKE 답을
//!   보내기 전에 쓴다(`pw_link::pair`). [`pw_link::pair::MAX_ATTEMPTS`] 를 다 쓰거나 [`pw_link::pair::CODE_TTL`] 이 지나거나 한 번
//!   맞으면 그 코드는 끝이다.
//! - 파일의 코드는 평문이다 — PAKE(SPAKE2)는 양쪽이 같은 암호를 쥐어야 한다. 같은 uid 로 이 폴더를 읽는 프로세스는 코드를 읽을 수
//!   있다 — 그 프로세스는 페어링 표에 직접 쓸 수도 있다(결정 M7-D13 의 대가 — 08 「보안 모델」). 코드는 로그 · 감사에 싣지 않는다.
//! - 쓰기는 페어링 표와 같은 잠금(`pairings.lock`) 아래에서 한다.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use pw_link::pair::{CODE_TTL, CodeBook, MAX_ATTEMPTS, Pending};
use pw_link::pairing::{load, read_private_file, save, write_private_file};
use pw_link::{Pairing, PairingCode, PairingTable, PublicKey};
use pw_rpc::Role;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::pairings::{file as pairings_file, lock, valid_name};

/// 대기 코드 파일의 이름(`$DATA/relay/` 아래).
pub const CODE_FILE: &str = "pairing-code.json";

/// 유닉스 ms 를 내는 시계(시험이 바꾼다).
pub type Clock = Arc<dyn Fn() -> u64 + Send + Sync>;

/// 벽시계.
pub fn system_clock() -> Clock {
    Arc::new(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
    })
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Stored {
    code: String,
    role: Role,
    #[serde(default)]
    name: Option<String>,
    expires_at_ms: u64,
    attempts: u32,
}

impl Drop for Stored {
    fn drop(&mut self) {
        zeroize::Zeroize::zeroize(&mut self.code);
    }
}

fn path(data: &Path) -> PathBuf {
    data.join(CODE_FILE)
}

fn read(data: &Path) -> io::Result<Option<Stored>> {
    match read_private_file(&path(data)) {
        Ok(text) => serde_json::from_str(&text)
            .map(Some)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string())),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

fn write(data: &Path, stored: &Stored) -> io::Result<()> {
    let text = Zeroizing::new(serde_json::to_vec(stored).map_err(io::Error::other)?);
    write_private_file(&path(data), &text)
}

fn remove(data: &Path) -> io::Result<()> {
    match std::fs::remove_file(path(data)) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// 새 대기 코드를 만든다(앞 코드는 끝난다). 돌려준 코드는 사람에게만 보인다. `data` 는 `$DATA/relay`.
pub fn issue(
    data: &Path,
    role: Role,
    name: Option<&str>,
    now_ms: u64,
) -> io::Result<(PairingCode, u64)> {
    if let Some(name) = name
        && !valid_name(name)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "the device name takes letters, digits, `.`, `_`, `-` (64 at most)",
        ));
    }
    let code = PairingCode::generate()?;
    let expires_at_ms = now_ms.saturating_add(u64::try_from(CODE_TTL.as_millis()).unwrap_or(0));
    let _lock = lock(data)?;
    write(
        data,
        &Stored {
            code: code.reveal().to_owned(),
            role,
            name: name.map(str::to_owned),
            expires_at_ms,
            attempts: 0,
        },
    )?;
    Ok((code, expires_at_ms))
}

/// 대기 코드를 버린다(있었으면 참).
pub fn cancel(data: &Path) -> io::Result<bool> {
    let _lock = lock(data)?;
    let had = read(data)?.is_some();
    remove(data)?;
    Ok(had)
}

/// 데몬의 [`CodeBook`]: 파일의 대기 코드로 시도를 받고, 맞으면 페어링 표 파일에 넣고 데몬의 표를 곧바로 바꾼다(1초 다시 읽기를
/// 기다리지 않는다 — 기기는 곧 다시 붙는다).
pub struct FileCodeBook {
    data: PathBuf,
    table: Option<PairingTable>,
    clock: Clock,
}

impl FileCodeBook {
    /// `data` 는 `$DATA/relay`, `table` 은 데몬의 표(없으면 파일만 쓴다).
    pub fn new(data: PathBuf, table: Option<PairingTable>, clock: Clock) -> Self {
        Self { data, table, clock }
    }
}

impl CodeBook for FileCodeBook {
    fn attempt(&self) -> io::Result<Option<Pending>> {
        let _lock = lock(&self.data)?;
        let Some(mut stored) = read(&self.data)? else {
            return Ok(None);
        };
        if (self.clock)() >= stored.expires_at_ms || stored.attempts >= MAX_ATTEMPTS {
            remove(&self.data)?;
            return Ok(None);
        }
        // 다 쓴 코드도 맞는 시도 하나의 `complete` 까지는 파일에 남긴다 — 그 뒤 시도는 위에서 끝난다.
        stored.attempts += 1;
        write(&self.data, &stored)?;
        let code = PairingCode::parse(&stored.code).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "the pending code is not six digits",
            )
        })?;
        Ok(Some(Pending {
            code,
            role: stored.role,
            name: stored.name.clone(),
        }))
    }

    fn complete(
        &self,
        code: &PairingCode,
        key: &PublicKey,
        role: Role,
        name: &str,
    ) -> io::Result<()> {
        let _lock = lock(&self.data)?;
        let stored = read(&self.data)?;
        let same = stored
            .as_ref()
            .and_then(|s| PairingCode::parse(&s.code))
            .is_some_and(|c| c == *code);
        if !same {
            return Err(io::Error::other("the pending code changed"));
        }
        remove(&self.data)?;
        let file = pairings_file(&self.data);
        let mut devices = load(&file)?;
        devices.retain(|p| p.key != *key);
        devices.push(Pairing {
            key: *key,
            role,
            name: name.to_owned(),
        });
        save(&file, &devices)?;
        crate::devices::record_paired(&self.data, &key.id(), (self.clock)());
        if let Some(table) = &self.table {
            table.replace(devices);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use pw_link::DeviceKey;

    use super::*;

    fn clock_at(now: Arc<AtomicU64>) -> Clock {
        Arc::new(move || now.load(Ordering::SeqCst))
    }

    fn book(data: &Path, now: &Arc<AtomicU64>) -> FileCodeBook {
        FileCodeBook::new(data.to_owned(), None, clock_at(Arc::clone(now)))
    }

    /// 만료(5분): 지난 코드는 시도를 받지 않고 파일에서 지운다.
    #[test]
    fn an_expired_code_takes_no_attempt() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("relay");
        let now = Arc::new(AtomicU64::new(1_000));
        issue(&data, Role::Owner, None, 1_000).unwrap();
        now.store(1_000 + 5 * 60 * 1000 - 1, Ordering::SeqCst);
        assert!(book(&data, &now).attempt().unwrap().is_some());
        now.store(1_000 + 5 * 60 * 1000, Ordering::SeqCst);
        assert!(book(&data, &now).attempt().unwrap().is_none());
        assert!(!path(&data).exists());
    }

    /// 시도는 그 코드에 대해 센다(책을 새로 만들어도 — 데몬이 다시 떠도 — 이어진다). 세 번 뒤에는 받지 않는다. 셋째 시도가 맞으면
    /// 그것으로 페어링된다.
    #[test]
    fn attempts_count_per_code_across_books() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("relay");
        let now = Arc::new(AtomicU64::new(0));
        let (code, _) = issue(&data, Role::Readonly, Some("phone"), 0).unwrap();
        for _ in 0..2 {
            assert!(book(&data, &now).attempt().unwrap().is_some());
        }
        let third = book(&data, &now).attempt().unwrap().expect("third attempt");
        assert_eq!(third.code, code);
        assert_eq!(third.role, Role::Readonly);
        assert_eq!(third.name.as_deref(), Some("phone"));
        let key = DeviceKey::generate().unwrap().public();
        book(&data, &now)
            .complete(&code, &key, Role::Readonly, "phone")
            .unwrap();
        assert_eq!(load(&pairings_file(&data)).unwrap()[0].role, Role::Readonly);
        // 끝난 코드.
        assert!(book(&data, &now).attempt().unwrap().is_none());

        let (_, _) = issue(&data, Role::Owner, None, 0).unwrap();
        for _ in 0..MAX_ATTEMPTS {
            assert!(book(&data, &now).attempt().unwrap().is_some());
        }
        assert!(
            book(&data, &now).attempt().unwrap().is_none(),
            "a fourth attempt"
        );
    }

    /// 새 코드를 만들면 앞 코드로는 `complete` 하지 못한다(그사이 바뀐 코드). 버린 코드도 같다.
    #[test]
    fn a_replaced_or_cancelled_code_does_not_complete() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("relay");
        let now = Arc::new(AtomicU64::new(0));
        let (first, _) = issue(&data, Role::Owner, None, 0).unwrap();
        let pending = book(&data, &now).attempt().unwrap().unwrap();
        assert_eq!(pending.code, first);
        let (_second, _) = loop {
            let issued = issue(&data, Role::Owner, None, 0).unwrap();
            if issued.0 != first {
                break issued;
            }
        };
        let key = DeviceKey::generate().unwrap().public();
        assert!(
            book(&data, &now)
                .complete(&first, &key, Role::Owner, "x")
                .is_err()
        );
        assert!(cancel(&data).unwrap());
        assert!(book(&data, &now).attempt().unwrap().is_none());
        assert!(load(&pairings_file(&data)).unwrap().is_empty());
    }

    /// 코드 파일은 이 사용자 전용(0600)이다.
    #[test]
    fn the_code_file_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("relay");
        issue(&data, Role::Owner, None, 0).unwrap();
        let mode = std::fs::metadata(path(&data)).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        assert!(issue(&data, Role::Owner, Some("bad name;"), 0).is_err());
    }
}
