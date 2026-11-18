//! 감지 출처 파일 읽기. 감지는 미신뢰 프로젝트에서도 돌므로 저장소가 정한 파일을 믿지 않는다:
//!
//! - 루트 바로 아래의 이름만 읽는다(정확한 대소문자 — [`entries`]).
//! - 링크는 따르되 실제 경로가 루트 안이어야 한다 — 루트 밖(`/etc/…` · 홈의 비밀 파일)을 가리키면 읽지 않고 까닭이다.
//! - 정규 파일만 연다 — FIFO · 장치 파일은 열기만 해도 멈추거나 끝이 없다.
//! - [`MAX_BYTES`] 를 넘으면 그만큼만 읽고 까닭이다. UTF-8 이 아니면 까닭이다.

use std::collections::BTreeSet;
use std::fs;
use std::io::Read as _;
use std::path::{Path, PathBuf};

/// 출처 파일 하나의 크기 상한(1 MiB). 사람이 쓰는 `package.json` · `Makefile` 은 이 크기에 닿지 않는다.
pub const MAX_BYTES: u64 = 1024 * 1024;

/// 루트에 있는 항목 이름(UTF-8 인 것만).
pub fn entries(root: &Path) -> Result<BTreeSet<String>, String> {
    let read = fs::read_dir(root)
        .map_err(|e| format!("cannot list the project root {}: {e}", root.display()))?;
    let mut out = BTreeSet::new();
    for item in read {
        let item =
            item.map_err(|e| format!("cannot list the project root {}: {e}", root.display()))?;
        if let Ok(name) = item.file_name().into_string() {
            out.insert(name);
        }
    }
    Ok(out)
}

/// 루트의 `name` 을 글로 읽는다. 없으면 `None`. 읽지 않는 까닭(루트 밖 링크 · 정규 파일 아님 · 너무 큼 · UTF-8 아님)은 오류.
pub fn read(root: &Path, name: &str) -> Result<Option<(PathBuf, String)>, String> {
    let path = root.join(name);
    match fs::symlink_metadata(&path) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{name}: {e}")),
    }
    let real = fs::canonicalize(&path).map_err(|e| format!("{name}: {e}"))?;
    let real_root = fs::canonicalize(root)
        .map_err(|e| format!("{name}: cannot resolve the project root: {e}"))?;
    if !real.starts_with(&real_root) {
        return Err(format!(
            "{name} is a link that points outside the project; Pitwall does not read it"
        ));
    }
    let meta = fs::metadata(&real).map_err(|e| format!("{name}: {e}"))?;
    if !meta.is_file() {
        return Err(format!("{name} is not a regular file"));
    }
    let mut bytes = Vec::new();
    fs::File::open(&real)
        .and_then(|f| f.take(MAX_BYTES + 1).read_to_end(&mut bytes))
        .map_err(|e| format!("{name}: {e}"))?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(format!(
            "{name} is larger than {} KiB; Pitwall does not read a file that large for task detection",
            MAX_BYTES / 1024
        ));
    }
    let text = String::from_utf8(bytes).map_err(|_| format!("{name} is not UTF-8 text"))?;
    Ok(Some((path, text)))
}
