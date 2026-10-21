//! hex 뷰어의 페이지 읽기 `fs.readBytes`(M1-23, 결정 M1-D10). `fs.read` 의 범위 읽기와 달리 판정을 하지 않아 파일 앞부분을 따로
//! 읽지 않는다 — 1 GB 파일의 끝 페이지를 보면 그 페이지의 바이트만 읽는다. 한 번에 [`HEX_PAGE_BYTES`](64 KiB)까지.
//!
//! 일반 파일만 읽는다. FIFO · 장치 · 소켓은 열면 멈추거나 끝이 없으므로 열기 전에 거절한다.

use std::fs;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::Arc;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use pw_rpc::{Router, RpcError, rpc_method};
use serde::{Deserialize, Serialize};

use crate::limits::HEX_PAGE_BYTES;
use crate::list::mtime_ms;
use crate::{Fs, denied, io_error};

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct FsReadBytesParams {
    pub project_id: String,
    /// 활성 워크트리의 꼭대기(`fs.read` 와 같다).
    #[serde(default)]
    pub worktree: Option<String>,
    /// 절대 경로.
    pub path: String,
    /// 시작 바이트.
    #[specta(type = pw_rpc::JsNumber)]
    pub offset: u64,
    /// 최대 바이트. 64 KiB 까지.
    #[specta(type = pw_rpc::JsNumber)]
    pub length: u64,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct FsBytes {
    /// 정규화한 실제 경로.
    pub path: String,
    /// 파일 전체 크기.
    #[specta(type = pw_rpc::JsNumber)]
    pub size: u64,
    #[specta(type = Option<pw_rpc::JsNumber>)]
    pub mtime_ms: Option<i64>,
    /// 읽은 범위의 시작(요청한 `offset`).
    #[specta(type = pw_rpc::JsNumber)]
    pub offset: u64,
    /// 읽은 바이트, 표준 base64. 파일 끝을 넘는 부분은 없다.
    pub data: String,
    /// 읽은 범위가 파일 끝까지 닿았다.
    pub eof: bool,
}

rpc_method! {
    /// 파일의 바이트 범위(64 KiB 까지)를 판정 없이 읽는다(hex 뷰어의 페이지). 일반 파일만.
    pub FsReadBytesMethod: "fs.readBytes" {
        params: FsReadBytesParams,
        result: FsBytes,
        tier: Read,
    }
}

/// `offset` 부터 최대 `length` 바이트. 그 범위 밖은 읽지 않는다.
pub(crate) fn read_page(
    file: &mut (impl Read + Seek),
    offset: u64,
    length: u64,
) -> io::Result<Vec<u8>> {
    file.seek(SeekFrom::Start(offset))?;
    let mut bytes = Vec::with_capacity(usize::try_from(length).unwrap_or(0));
    file.take(length).read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn not_regular(path: &Path) -> RpcError {
    RpcError::invalid_params(format!("`{}` is not a regular file", path.display()))
}

impl Fs {
    pub fn read_bytes(&self, params: &FsReadBytesParams) -> Result<FsBytes, RpcError> {
        if params.length > HEX_PAGE_BYTES {
            return Err(RpcError::invalid_params(format!(
                "a byte page is at most {HEX_PAGE_BYTES} bytes"
            )));
        }
        let roots = self.roots(&params.project_id, params.worktree.as_deref())?;
        let target = roots.readable(Path::new(&params.path)).map_err(denied)?;
        let real = &target.real;
        let meta = fs::metadata(real).map_err(|e| io_error(real, e))?;
        if !meta.is_file() {
            return Err(not_regular(real));
        }
        let mut file = self.open(&roots, real).map_err(|e| io_error(real, e))?;
        // 판정과 열기 사이에 바꿔 끼워졌으면 연 핸들로 다시 본다.
        let meta = file.metadata().map_err(|e| io_error(real, e))?;
        if !meta.is_file() {
            return Err(not_regular(real));
        }
        let bytes =
            read_page(&mut file, params.offset, params.length).map_err(|e| io_error(real, e))?;
        Ok(FsBytes {
            path: real.display().to_string(),
            size: meta.len(),
            mtime_ms: mtime_ms(&meta),
            offset: params.offset,
            eof: params.offset.saturating_add(bytes.len() as u64) >= meta.len(),
            data: BASE64.encode(&bytes),
        })
    }
}

pub(crate) fn register(router: &mut Router, fs: Arc<Fs>) {
    router.unary(FsReadBytesMethod, move |_ctx, params: FsReadBytesParams| {
        let s = Arc::clone(&fs);
        async move {
            tokio::task::spawn_blocking(move || s.read_bytes(&params))
                .await
                .map_err(|e| RpcError::internal(e.to_string()))?
        }
    });
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    /// 읽은 바이트를 센다.
    struct Counting<R> {
        inner: R,
        read: u64,
    }

    impl<R: Read> Read for Counting<R> {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let n = self.inner.read(buf)?;
            self.read += n as u64;
            Ok(n)
        }
    }

    impl<R: Seek> Seek for Counting<R> {
        fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
            self.inner.seek(pos)
        }
    }

    #[test]
    fn a_page_reads_only_its_own_bytes() {
        let data: Vec<u8> = (0..300_000u32).map(|i| (i % 253) as u8).collect();
        let mut file = Counting {
            inner: Cursor::new(&data),
            read: 0,
        };
        let page = read_page(&mut file, 200_000, HEX_PAGE_BYTES).unwrap();
        assert_eq!(page, &data[200_000..200_000 + HEX_PAGE_BYTES as usize]);
        assert_eq!(file.read, HEX_PAGE_BYTES);

        // 끝 페이지는 남은 만큼만.
        file.read = 0;
        let last = read_page(&mut file, 290_000, HEX_PAGE_BYTES).unwrap();
        assert_eq!(last, &data[290_000..]);
        assert_eq!(file.read, 10_000);
    }
}
