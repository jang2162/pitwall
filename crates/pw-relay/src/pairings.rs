//! 페어링 표 파일(`$DATA/relay/pairings.json` — M7-03)을 따라간다. 표를 쓰는 것은 그 머신의 사람 · 설치 명령(`pair` — M7-06 ·
//! M7-23)이고 데몬은 읽기만 한다 — 데몬 소켓으로 표를 바꾸는 길은 없다(`crate::command`). 파일이 바뀌면 다시 읽어 표를 바꾸고,
//! 빠지거나 role 이 바뀐 기기의 링크는 `pw_link` 가 끊는다.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use pw_link::PairingTable;
use pw_link::pairing::{PAIRINGS_FILE, load};

/// 다시 읽어 보는 간격.
pub const POLL_EVERY: Duration = Duration::from_secs(1);

pub fn file(data: &Path) -> PathBuf {
    data.join(PAIRINGS_FILE)
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

    use pw_link::{DeviceKey, Pairing, pairing::save};
    use pw_rpc::Role;

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
}
