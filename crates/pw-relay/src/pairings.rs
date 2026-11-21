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

/// 처음 읽는다. 읽지 못하면 빈 표다(fail-closed — 아무 기기도 붙지 못한다).
pub fn initial(path: &Path) -> PairingTable {
    PairingTable::new(read(path))
}

fn read(path: &Path) -> Vec<pw_link::Pairing> {
    load(path).unwrap_or_else(|e| {
        tracing::warn!(%e, path = %path.display(), "cannot read the pairing table; no device is paired");
        Vec::new()
    })
}

fn stamp(path: &Path) -> Option<(SystemTime, u64)> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

/// 파일이 바뀔 때마다 표를 바꾼다(끝나지 않는다).
pub async fn follow(path: PathBuf, table: PairingTable) {
    let mut seen = stamp(&path);
    loop {
        tokio::time::sleep(POLL_EVERY).await;
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
