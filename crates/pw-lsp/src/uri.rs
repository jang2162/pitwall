//! 경로 ↔ `file://` URI. LSP 의 URI 는 퍼센트 인코딩한 `file://` 이고(Windows 는 `file:///C:/…`), 서버마다 대소문자 · 인코딩이 조금씩
//! 달라 받은 URI 는 경로로 바꿔 비교한다(M4-05).

use std::path::{Path, PathBuf};

/// 절대 경로 → `file://` URI 글. 상대 경로면 `None`.
pub fn file_uri(path: &Path) -> Option<String> {
    url::Url::from_file_path(path).ok().map(String::from)
}

/// `file://` URI → 경로. 다른 스킴이거나 읽지 못하면 `None`.
pub fn uri_path(uri: &str) -> Option<PathBuf> {
    let url = url::Url::parse(uri).ok()?;
    if url.scheme() != "file" {
        return None;
    }
    url.to_file_path().ok()
}

/// `lsp-types` 의 URI 형으로.
pub fn lsp_uri(path: &Path) -> Option<lsp_types::Uri> {
    file_uri(path)?.parse().ok()
}

/// 시험용: 유닉스 꼴 절대 경로(`/p/a.rs`)를 이 OS 의 절대 경로로 — Windows 는 드라이브가 있어야 절대 경로라 `C:\p\a.rs`.
#[cfg(test)]
pub(crate) fn native(unix: &str) -> PathBuf {
    if cfg!(windows) {
        PathBuf::from(format!("C:{}", unix.replace('/', "\\")))
    } else {
        PathBuf::from(unix)
    }
}

/// 시험용: [`native`] 경로의 `file://` URI(Windows 는 `file:///C:/p/a.rs`).
#[cfg(test)]
pub(crate) fn native_uri(unix: &str) -> String {
    file_uri(&native(unix)).expect("an absolute path")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn paths_and_uris_round_trip_with_percent_encoding() {
        let path = Path::new("/tmp/한 글/a#b.rs");
        let uri = file_uri(path).unwrap();
        assert_eq!(uri, "file:///tmp/%ED%95%9C%20%EA%B8%80/a%23b.rs");
        assert_eq!(uri_path(&uri).unwrap(), path);
        assert_eq!(lsp_uri(path).unwrap().as_str(), uri);
        assert_eq!(file_uri(Path::new("relative")), None);
        assert_eq!(uri_path("jdt://contents/x"), None);
    }

    #[cfg(windows)]
    #[test]
    fn windows_paths_keep_the_drive_letter() {
        let path = Path::new(r"C:\tmp\a b.rs");
        let uri = file_uri(path).unwrap();
        assert_eq!(uri, "file:///C:/tmp/a%20b.rs");
        assert_eq!(uri_path(&uri).unwrap(), path);
    }
}
