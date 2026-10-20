//! 읽기와 `version`. `version` 은 크기 + mtime + 내용 해시(SHA-256)를 이은 불투명한 문자열이다 — mtime 단위 안에서 같은 크기로
//! 바뀐 내용도 해시가 가른다. 파일 전체를 읽었을 때만 만든다(범위 읽기는 해시할 내용이 없다).

use std::fs::{self, File};
use std::io::{self, Read as _, Seek as _, SeekFrom};
use std::path::Path;
use std::time::UNIX_EPOCH;

use sha2::{Digest as _, Sha256};

use crate::judge::SNIFF_BYTES;

/// 한 번에 읽은 파일 내용과 그 순간의 메타데이터.
pub struct Snapshot {
    pub bytes: Vec<u8>,
    pub meta: fs::Metadata,
}

impl Snapshot {
    pub fn version(&self) -> String {
        version(&self.bytes, &self.meta)
    }
}

pub fn version(bytes: &[u8], meta: &fs::Metadata) -> String {
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos());
    let hash = Sha256::digest(bytes);
    let hex: String = hash.iter().map(|b| format!("{b:02x}")).collect();
    format!("{}-{mtime}-{hex}", bytes.len())
}

/// 파일 전체. 읽는 사이 크기 · mtime 이 바뀌면(다른 프로세스가 쓰는 중) 다시 읽는다 — 내용과 `version` 이 서로 다른 순간의 것이
/// 되지 않게.
pub fn read_whole(path: &Path) -> io::Result<Snapshot> {
    const ATTEMPTS: usize = 5;
    for _ in 0..ATTEMPTS {
        let mut file = File::open(path)?;
        let before = file.metadata()?;
        let mut bytes = Vec::with_capacity(usize::try_from(before.len()).unwrap_or(0));
        file.read_to_end(&mut bytes)?;
        // 경로로 다시 본다 — rename 으로 바꿔 끼워졌으면 연 핸들이 아니라 이름이 가리키는 파일이 달라졌다.
        let after = fs::metadata(path)?;
        if same(&before, &after) && bytes.len() as u64 == after.len() {
            return Ok(Snapshot { bytes, meta: after });
        }
    }
    Err(io::Error::other(format!(
        "{} kept changing while it was read",
        path.display()
    )))
}

fn same(a: &fs::Metadata, b: &fs::Metadata) -> bool {
    a.len() == b.len() && a.modified().ok() == b.modified().ok()
}

/// 범위 읽기의 결과.
pub struct Range {
    /// 파일 앞부분(최대 8 KiB) — 판정용.
    pub head: Vec<u8>,
    pub bytes: Vec<u8>,
    pub meta: fs::Metadata,
}

/// `offset` 부터 최대 `length` 바이트. 판정에 쓸 파일 앞부분도 함께 읽는다(최대 8 KiB).
pub fn read_range(path: &Path, offset: u64, length: u64) -> io::Result<Range> {
    let mut file = File::open(path)?;
    let meta = file.metadata()?;
    let mut head = Vec::new();
    (&mut file)
        .take(SNIFF_BYTES as u64)
        .read_to_end(&mut head)?;
    let mut bytes = Vec::new();
    if offset == 0 && length <= head.len() as u64 {
        bytes.extend_from_slice(&head[..usize::try_from(length).unwrap_or(head.len())]);
    } else {
        file.seek(SeekFrom::Start(offset))?;
        file.take(length).read_to_end(&mut bytes)?;
    }
    Ok(Range { head, bytes, meta })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_changes_with_content_of_the_same_size_and_mtime() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("a");
        fs::write(&path, "aaaa").unwrap();
        let meta = fs::metadata(&path).unwrap();
        assert_ne!(version(b"aaaa", &meta), version(b"aaab", &meta));
        assert_eq!(
            version(b"aaaa", &meta),
            read_whole(&path).unwrap().version()
        );
    }

    #[test]
    fn ranges_read_only_the_asked_bytes_and_the_head() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("big");
        let data: Vec<u8> = (0..20_000u32).map(|i| (i % 251) as u8).collect();
        fs::write(&path, &data).unwrap();
        let r = read_range(&path, 10_000, 100).unwrap();
        assert_eq!(r.bytes, &data[10_000..10_100]);
        assert_eq!(r.head, &data[..SNIFF_BYTES]);
        let r = read_range(&path, 0, 10).unwrap();
        assert_eq!(r.bytes, &data[..10]);
        // 끝을 넘는 범위는 있는 만큼.
        let r = read_range(&path, 19_990, 100).unwrap();
        assert_eq!(r.bytes, &data[19_990..]);
        let r = read_range(&path, 30_000, 100).unwrap();
        assert!(r.bytes.is_empty());
    }
}
