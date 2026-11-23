//! 큰 파일의 조각 읽기 · 쓰기(18-m7-plan M7-13b). 다른 머신의 RPC 는 링크 틀 하나(64 MiB)에 실리고, 틀을 넘는 메시지는 그 머신의
//! 채널을 닫는다(`pw-link` `mux.rs`). 통째 `fs.read` · `fs.write` 는 텍스트가 JSON 에서 최대 6배(`\u00XX`)가 되므로 50 MiB 파일을
//! 실을 수 없다 — 조각은 **와이어 바이트**(직렬화한 내용의 바이트)로 자른다.
//!
//! - **읽기 `fs.readChunk`**: 조각마다 파일 전체를 한 순간으로 읽어([`crate::read::read_whole`] — 통째 읽기의 다시 읽기 규칙 그대로)
//!   통째 `fs.read` 와 같은 판정 · 내용을 만들고, 그 내용의 `offset` 부터 `maxBytes` 와이어 바이트 안에 드는 만큼을 준다. 조각마다
//!   그 순간의 `version` 이 실린다. 둘째 조각부터 `expectedVersion`(첫 조각의 것)을 받아 지금 파일과 다르면 내용 없이 `CONFLICT` —
//!   조각 사이에 파일이 바뀌었으면 어느 조각도 `version` 을 주지 않고 받는 쪽은 처음부터 다시 읽는다. 상태를 두지 않는다.
//! - **쓰기 `fs.writeChunk` → `fs.writeCommit`**: 조각을 Host 메모리에 모은 뒤 커밋에서 `expectedVersion` 을 견주고 원자 쓰기
//!   ([`crate::write`])로 한 번에 바꾼다. 모으는 동안 파일은 그대로다 — 중간에 끊기거나 버리면(`fs.writeAbort`, 연결 닫힘, 오래 쉼)
//!   원래 파일이 남는다. 모음은 그것을 연 **세션(연결)** 과 프로젝트 · 워크트리 · 실제 경로에 묶인다 — 다른 연결 · 다른 파일로는
//!   이어 쓰거나 커밋하지 못한다. 조각마다 쓰기 범위([`Fs::write_roots`])를 다시 본다.
//!
//! 상한: 조각 하나 [`CHUNK_WIRE_MAX`], 모음 하나 [`UPLOAD_MAX`], 동시 모음은 세션마다 [`UPLOADS_PER_SESSION`] · Host 전체
//! [`UPLOADS_MAX`], [`UPLOAD_IDLE`] 동안 조각이 오지 않은 모음은 버린다.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, PoisonError};
use std::time::Duration;

use tokio::time::Instant;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use pw_rpc::{ConnectionScope, FileScope, Router, RpcError, SessionId, code, rpc_method};
use serde::{Deserialize, Serialize};

use crate::judge::{Bom, Judgment, judge};
use crate::limits::{
    CHUNK_WIRE_MAX, READ_ONLY_MAX, SNIFF_BYTES, UPLOAD_MAX, UPLOADS_MAX, UPLOADS_PER_SESSION,
};
use crate::list::mtime_ms;
use crate::methods::{FsContent, FsWritten};
use crate::{Fs, UTF8_BOM, content, denied, io_error, read, scope};

/// 조각이 오지 않은 채 이만큼 지난 모음은 버린다.
pub const UPLOAD_IDLE: Duration = Duration::from_secs(5 * 60);

/// 쉰 모음을 치우는 태스크가 깨는 간격.
pub const SWEEP_EVERY: Duration = Duration::from_secs(30);

/// 와이어 예산의 하한 — 한 글자(최대 `\u00XX` 6바이트 · 4바이트 글자)와 따옴표가 늘 든다.
const MIN_WIRE: u64 = 16;

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct FsReadChunkParams {
    pub project_id: String,
    #[serde(default)]
    pub worktree: Option<String>,
    /// Absolute path.
    pub path: String,
    /// 이 조각이 시작하는 내용 안의 자리 — 첫 조각은 0, 다음부터 앞 조각의 `next`. 텍스트면 풀어 낸 텍스트의 UTF-8 바이트 자리,
    /// base64 면 파일 바이트 자리다.
    #[serde(default)]
    #[specta(type = pw_rpc::JsNumber)]
    pub offset: u64,
    /// 조각 내용(`content` 의 `text` · `data` 문자열)의 직렬화한 JSON 바이트 상한. 16 이상 [`CHUNK_WIRE_MAX`] 이하.
    #[specta(type = pw_rpc::JsNumber)]
    pub max_bytes: u64,
    /// 첫 조각의 `version`. 지금 파일이 다르면 내용 없이 `CONFLICT`(`data.current`) — 처음부터 다시 읽는다.
    #[serde(default)]
    pub expected_version: Option<String>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct FsReadChunk {
    /// 정규화한 실제 경로.
    pub path: String,
    /// 파일 전체 크기.
    #[specta(type = pw_rpc::JsNumber)]
    pub size: u64,
    #[specta(type = Option<pw_rpc::JsNumber>)]
    pub mtime_ms: Option<i64>,
    /// 이 조각을 자른 순간의 파일 전체 `version`. 모든 조각이 같아야 그 내용의 `version` 이다(`fs.write` 의 `expectedVersion`).
    pub version: String,
    /// 파일 전체의 판정(통째 `fs.read` 와 같다).
    pub judgment: Judgment,
    /// 이 조각의 시작(요청한 `offset`).
    #[specta(type = pw_rpc::JsNumber)]
    pub offset: u64,
    /// 다음 조각의 `offset`.
    #[specta(type = pw_rpc::JsNumber)]
    pub next: u64,
    /// 내용 전체의 길이(`offset` 과 같은 단위).
    #[specta(type = pw_rpc::JsNumber)]
    pub total: u64,
    /// 마지막 조각이다(`next == total`).
    pub eof: bool,
    /// 이 조각의 내용. 종류(텍스트 · base64)는 통째 `fs.read` 와 같고 모든 조각이 같다. 이어 붙이면 통째 읽기의 내용이다 — base64 는
    /// 마지막이 아닌 조각이 3바이트 배수라 문자열을 그대로 이어 붙여도 된다.
    pub content: FsContent,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct FsWriteChunkParams {
    pub project_id: String,
    #[serde(default)]
    pub worktree: Option<String>,
    /// 절대 경로(`fs.write` 와 같은 판정). 모음의 모든 조각 · 커밋이 같은 경로여야 한다.
    pub path: String,
    /// 이어 쓸 모음. 없으면 새 모음을 연다.
    #[serde(default)]
    pub upload: Option<String>,
    /// 이 조각이 놓일 자리 — 앞 조각까지 보낸 바이트(텍스트면 UTF-8, base64 면 푼 바이트). 첫 조각은 0. Host 가 모은 바이트와 다르면
    /// 거절한다(빠뜨린 · 겹친 · 순서가 바뀐 조각).
    #[serde(default)]
    #[specta(type = pw_rpc::JsNumber)]
    pub offset: u64,
    /// 이 조각의 내용(UTF-8 텍스트 조각 또는 그 조각만의 base64). 모음의 모든 조각이 같은 종류여야 한다.
    pub content: FsContent,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct FsWriteChunk {
    /// 모음의 id — 다음 조각 · 커밋 · 버리기에 준다.
    pub upload: String,
    /// 지금까지 모은 바이트.
    #[specta(type = pw_rpc::JsNumber)]
    pub received: u64,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct FsWriteCommitParams {
    pub project_id: String,
    #[serde(default)]
    pub worktree: Option<String>,
    /// 조각을 보낸 경로.
    pub path: String,
    pub upload: String,
    /// 보낸 바이트의 합(BOM 빼고). 있으면 모은 바이트와 견주고 다르면 아무것도 쓰지 않고 거절한다(웹뷰는 늘 준다).
    #[serde(default)]
    #[specta(type = Option<pw_rpc::JsNumber>)]
    pub size: Option<u64>,
    /// 텍스트 앞에 UTF-8 BOM 을 붙인다(`fs.write` 와 같다).
    #[serde(default)]
    pub bom: bool,
    /// `fs.write` 와 같다 — 지금 파일과 다르면 아무것도 쓰지 않고 `CONFLICT`. 없으면 새 파일.
    #[serde(default)]
    pub expected_version: Option<String>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct FsWriteAbortParams {
    pub project_id: String,
    pub upload: String,
}

rpc_method! {
    /// 파일 전체 읽기의 한 조각(내용의 `offset` 부터 `maxBytes` 와이어 바이트 안). 조각마다 그 순간의 파일 전체 `version` 을 싣고,
    /// `expectedVersion` 이 지금 파일과 다르면 `CONFLICT`. 50 MiB 까지.
    pub FsReadChunkMethod: "fs.readChunk" {
        params: FsReadChunkParams,
        result: FsReadChunk,
        tier: Read,
        file_scope: true,
    }

    /// 조각 쓰기의 한 조각을 Host 에 모은다(파일은 바꾸지 않는다). 모음은 이 연결 · 그 경로에 묶인다.
    pub FsWriteChunkMethod: "fs.writeChunk" {
        params: FsWriteChunkParams,
        result: FsWriteChunk,
        tier: Write,
        file_scope: true,
    }

    /// 모은 내용을 `fs.write` 처럼 원자적으로 쓴다(`expectedVersion` 충돌이면 아무것도 바꾸지 않고 `CONFLICT`). 성공 · 실패 모두
    /// 모음을 닫는다.
    pub FsWriteCommitMethod: "fs.writeCommit" {
        params: FsWriteCommitParams,
        result: FsWritten,
        tier: Write,
        file_scope: true,
    }

    /// 모음을 버린다(파일은 그대로). 없는 모음이면 아무 일도 없다.
    pub FsWriteAbortMethod: "fs.writeAbort" {
        params: FsWriteAbortParams,
        result: (),
        tier: Write,
        file_scope: true,
    }
}

/// 열린 모음.
struct Upload {
    session: SessionId,
    project_id: String,
    worktree: Option<String>,
    /// 첫 조각에서 판정한 쓸 실제 경로.
    real: PathBuf,
    text: bool,
    bytes: Vec<u8>,
    touched: Instant,
}

/// Host 의 열린 모음들([`Fs`] 가 든다).
#[derive(Default)]
pub(crate) struct Uploads {
    next: u64,
    open: HashMap<String, Upload>,
    /// 닫히면 모음을 버리도록 지켜보는 세션 — 세션마다 지켜보는 태스크 하나.
    watched: std::collections::HashSet<SessionId>,
    /// 쉰 모음을 시계로 치우는 태스크가 돈다([`reap`]).
    reaping: bool,
    /// 핸들러가 받았지만 아직 끝나지 않은 `fs.writeChunk` — 치우는 태스크는 이것이 0 이고 모음이 없을 때만 멈춘다.
    pending: usize,
}

impl Uploads {
    fn sweep(&mut self, now: Instant) {
        self.open
            .retain(|_, u| now.duration_since(u.touched) < UPLOAD_IDLE);
    }
}

/// serde_json 이 문자열 안의 글자 하나를 쓰는 바이트(`"` · `\` · 제어 문자는 이스케이프).
fn escaped_len(c: char) -> u64 {
    match c {
        '"' | '\\' | '\u{8}' | '\u{c}' | '\n' | '\r' | '\t' => 2,
        c if (c as u32) < 0x20 => 6,
        c => c.len_utf8() as u64,
    }
}

/// `text[offset..]` 에서 따옴표까지 `budget` 와이어 바이트 안에 드는 끝 자리.
fn text_end(text: &str, offset: usize, budget: u64) -> usize {
    let mut used = 2;
    let mut end = offset;
    for c in text[offset..].chars() {
        used += escaped_len(c);
        if used > budget {
            break;
        }
        end += c.len_utf8();
    }
    end
}

fn busy(what: &str) -> RpcError {
    RpcError::internal(format!("chunked write is busy: {what}"))
}

fn unknown_upload(id: &str) -> RpcError {
    RpcError::new(code::NOT_FOUND, "no such upload").with_data(serde_json::json!({ "upload": id }))
}

impl Fs {
    /// `fs.readChunk`. `file` 은 파일 범위 연결의 그 파일([`Self::roots_in`]).
    pub fn read_chunk_in(
        &self,
        file: Option<&FileScope>,
        params: &FsReadChunkParams,
    ) -> Result<FsReadChunk, RpcError> {
        if !(MIN_WIRE..=CHUNK_WIRE_MAX).contains(&params.max_bytes) {
            return Err(RpcError::invalid_params(format!(
                "`maxBytes` must be between {MIN_WIRE} and {CHUNK_WIRE_MAX}"
            )));
        }
        let roots = self.roots_in(file, &params.project_id, params.worktree.as_deref())?;
        let target = roots.readable(Path::new(&params.path)).map_err(denied)?;
        let real = &target.real;
        // 통째 `fs.read` 와 같은 판정 — 일반 파일만, 50 MiB 까지.
        let meta = std::fs::symlink_metadata(real).map_err(|e| io_error(real, e))?;
        if meta.is_dir() {
            return Err(RpcError::invalid_params(format!(
                "`{}` is a folder",
                real.display()
            )));
        }
        if !meta.is_file() {
            return Err(RpcError::invalid_params(
                scope::NotAFile(real.clone()).to_string(),
            ));
        }
        if meta.len() > READ_ONLY_MAX {
            return Err(RpcError::invalid_params(format!(
                "`{}` is larger than {READ_ONLY_MAX} bytes",
                real.display()
            )));
        }
        let snap =
            read::read_whole(real, &|p| self.open(&roots, p)).map_err(|e| io_error(real, e))?;
        if snap.meta.len() > READ_ONLY_MAX {
            return Err(RpcError::invalid_params(format!(
                "`{}` is larger than {READ_ONLY_MAX} bytes",
                real.display()
            )));
        }
        let version = snap.version();
        if let Some(expected) = &params.expected_version
            && *expected != version
        {
            return Err(RpcError::new(code::CONFLICT, "conflict").with_data(
                serde_json::json!({ "path": real.display().to_string(), "current": version }),
            ));
        }
        let head = &snap.bytes[..snap.bytes.len().min(SNIFF_BYTES)];
        let start = Bom::detect(head).map_or(0, Bom::byte_len);
        let judgment = judge(head, &snap.bytes[start..], snap.meta.len(), false);
        let whole = content(false, &judgment, &snap.bytes, start);
        let offset = usize::try_from(params.offset).unwrap_or(usize::MAX);
        let (piece, end, total) = match whole {
            FsContent::Text { text } => {
                if offset > text.len() || !text.is_char_boundary(offset) {
                    return Err(RpcError::invalid_params(
                        "`offset` is not a position in the text",
                    ));
                }
                let end = text_end(&text, offset, params.max_bytes);
                if end == offset && offset < text.len() {
                    return Err(RpcError::invalid_params("`maxBytes` is too small"));
                }
                let piece = FsContent::Text {
                    text: text[offset..end].to_owned(),
                };
                (piece, end, text.len())
            }
            FsContent::Base64 { .. } => {
                // base64 는 3바이트가 4글자, 따옴표 둘. 마지막이 아닌 조각은 3의 배수라 이어 붙일 수 있다.
                let raw = &snap.bytes;
                if offset > raw.len() {
                    return Err(RpcError::invalid_params("`offset` is past the end"));
                }
                let fits = usize::try_from((params.max_bytes - 2) / 4 * 3).unwrap_or(usize::MAX);
                let end = offset.saturating_add(fits).min(raw.len());
                let piece = FsContent::Base64 {
                    data: BASE64.encode(&raw[offset..end]),
                };
                (piece, end, raw.len())
            }
        };
        Ok(FsReadChunk {
            path: real.display().to_string(),
            size: snap.meta.len(),
            mtime_ms: mtime_ms(&snap.meta),
            version,
            judgment,
            offset: params.offset,
            next: end as u64,
            total: total as u64,
            eof: end == total,
            content: piece,
        })
    }

    /// `fs.writeChunk`. 이 세션을 처음 지켜봐야 하면 `true` 를 함께 준다(연결이 닫히면 그 세션의 모음을 버리게 하는 쪽이 본다).
    pub fn write_chunk_in(
        &self,
        session: SessionId,
        scope: &ConnectionScope,
        params: &FsWriteChunkParams,
        gone: &dyn Fn() -> bool,
    ) -> Result<FsWriteChunk, RpcError> {
        #[cfg(test)]
        if let Some(gate) = self
            .chunk_gate
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
        {
            gate();
        }
        let roots = self.write_roots(scope, &params.project_id, params.worktree.as_deref())?;
        let real = roots.writable(Path::new(&params.path)).map_err(denied)?;
        let (text, piece) = match &params.content {
            FsContent::Text { text } => (true, text.as_bytes().to_vec()),
            FsContent::Base64 { data } => (
                false,
                BASE64
                    .decode(data)
                    .map_err(|e| RpcError::invalid_params(format!("content is not base64: {e}")))?,
            ),
        };
        if piece.len() as u64 > CHUNK_WIRE_MAX {
            return Err(RpcError::invalid_params(format!(
                "a chunk is at most {CHUNK_WIRE_MAX} bytes"
            )));
        }
        let now = Instant::now();
        let mut uploads = self.uploads.lock().unwrap_or_else(PoisonError::into_inner);
        uploads.sweep(now);
        // 연결이 닫혔거나 요청이 취소됐다 — 받는 쪽이 id 를 받지 못하니 모음을 만들거나 늘리지 않는다. 잠금 안에서 본다: 닫힘 감시의
        // 버리기([`Fs::drop_uploads`])도 이 잠금을 쥐므로 그보다 먼저 넣은 모음은 그것이 치우고, 뒤면 여기서 막힌다.
        if gone() {
            return Err(RpcError::cancelled());
        }
        let Some(id) = &params.upload else {
            if params.offset != 0 {
                return Err(misplaced(0));
            }
            if piece.len() as u64 > UPLOAD_MAX {
                return Err(too_large());
            }
            if uploads.open.len() >= UPLOADS_MAX {
                return Err(busy(&format!(
                    "{UPLOADS_MAX} uploads are already open on this host"
                )));
            }
            if uploads
                .open
                .values()
                .filter(|u| u.session == session)
                .count()
                >= UPLOADS_PER_SESSION
            {
                return Err(busy(&format!(
                    "{UPLOADS_PER_SESSION} uploads are already open on this connection"
                )));
            }
            uploads.next += 1;
            let id = format!("u{}", uploads.next);
            let received = piece.len() as u64;
            uploads.open.insert(
                id.clone(),
                Upload {
                    session,
                    project_id: params.project_id.clone(),
                    worktree: params.worktree.clone(),
                    real,
                    text,
                    bytes: piece,
                    touched: now,
                },
            );
            return Ok(FsWriteChunk {
                upload: id,
                received,
            });
        };
        let upload = owned(&mut uploads.open, id, session)?;
        same_target(upload, params, &real)?;
        if upload.text != text {
            return Err(RpcError::invalid_params(
                "every chunk of an upload must be the same kind of content",
            ));
        }
        if params.offset != upload.bytes.len() as u64 {
            return Err(misplaced(upload.bytes.len() as u64));
        }
        if (upload.bytes.len() + piece.len()) as u64 > UPLOAD_MAX {
            uploads.open.remove(id);
            return Err(too_large());
        }
        upload.bytes.extend_from_slice(&piece);
        upload.touched = now;
        Ok(FsWriteChunk {
            upload: id.clone(),
            received: upload.bytes.len() as u64,
        })
    }

    /// `fs.writeCommit`. 모음은 결과와 상관없이 닫힌다.
    pub fn write_commit_in(
        &self,
        session: SessionId,
        scope: &ConnectionScope,
        params: &FsWriteCommitParams,
    ) -> Result<FsWritten, RpcError> {
        self.write_commit_with(session, scope, params, |_| Ok(()))
    }

    pub(crate) fn write_commit_with(
        &self,
        session: SessionId,
        scope: &ConnectionScope,
        params: &FsWriteCommitParams,
        fault: impl Fn(crate::write::Stage) -> std::io::Result<()>,
    ) -> Result<FsWritten, RpcError> {
        let roots = self.write_roots(scope, &params.project_id, params.worktree.as_deref())?;
        let real = roots.writable(Path::new(&params.path)).map_err(denied)?;
        let upload = {
            let mut uploads = self.uploads.lock().unwrap_or_else(PoisonError::into_inner);
            uploads.sweep(Instant::now());
            // 다른 파일 · 프로젝트로 돌린 커밋은 모음을 닫지 않고 거절한다 — 그 모음의 파일은 그대로 커밋할 수 있다.
            same_target(
                owned(&mut uploads.open, &params.upload, session)?,
                params,
                &real,
            )?;
            uploads
                .open
                .remove(&params.upload)
                .ok_or_else(|| unknown_upload(&params.upload))?
        };
        let bytes = match (upload.text, params.bom) {
            (true, true) => {
                let mut bytes = Vec::with_capacity(upload.bytes.len() + UTF8_BOM.len());
                bytes.extend_from_slice(UTF8_BOM);
                bytes.extend_from_slice(&upload.bytes);
                bytes
            }
            (true, false) => upload.bytes,
            (false, true) => {
                return Err(RpcError::invalid_params(
                    "`bom` applies only to text content",
                ));
            }
            (false, false) => upload.bytes,
        };
        let sent = bytes.len() as u64 - if params.bom { UTF8_BOM.len() as u64 } else { 0 };
        if let Some(size) = params.size
            && sent != size
        {
            return Err(RpcError::invalid_params(format!(
                "the upload holds {sent} bytes, not {size}; nothing was written"
            )));
        }
        // 상한은 쓸 파일의 크기다(BOM 포함) — 다시 열 수 있는 크기만 쓴다.
        if bytes.len() as u64 > UPLOAD_MAX {
            return Err(too_large());
        }
        self.replace(
            &roots,
            &real,
            &bytes,
            params.expected_version.as_deref(),
            fault,
        )
    }

    /// `fs.writeAbort`. 다른 연결의 모음은 버리지 못한다(없는 것처럼 아무 일도 없다).
    pub fn write_abort(&self, session: SessionId, params: &FsWriteAbortParams) {
        let mut uploads = self.uploads.lock().unwrap_or_else(PoisonError::into_inner);
        if uploads
            .open
            .get(&params.upload)
            .is_some_and(|u| u.session == session && u.project_id == params.project_id)
        {
            uploads.open.remove(&params.upload);
        }
    }

    /// `fs.writeChunk` 를 받았다 — 블로킹 일을 시작하기 **전에** 부른다(핸들러가 버려져도 감시가 남게). 이 세션을 처음 지켜봐야 하면
    /// `(true, _)`, 치우는 태스크를 띄워야 하면 `(_, true)`.
    fn arrive(&self, session: SessionId) -> (bool, bool) {
        let mut uploads = self.uploads.lock().unwrap_or_else(PoisonError::into_inner);
        uploads.pending += 1;
        let watch = uploads.watched.insert(session);
        let reap = !uploads.reaping;
        uploads.reaping = true;
        (watch, reap)
    }

    /// [`Self::arrive`] 의 짝 — 블로킹 일이 끝났다(성공 · 실패 · 버려진 핸들러 모두).
    fn depart(&self) {
        let mut uploads = self.uploads.lock().unwrap_or_else(PoisonError::into_inner);
        uploads.pending -= 1;
    }

    /// 그 세션의 모음을 모두 버린다(연결이 닫혔다).
    pub(crate) fn drop_uploads(&self, session: SessionId) {
        let mut uploads = self.uploads.lock().unwrap_or_else(PoisonError::into_inner);
        uploads.open.retain(|_, u| u.session != session);
        uploads.watched.remove(&session);
    }

    /// 받는 중인 `fs.writeChunk` 수(시험).
    #[cfg(test)]
    pub(crate) fn pending_chunks(&self) -> usize {
        self.uploads
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pending
    }

    /// 열린 모음 수(시험).
    #[cfg(test)]
    pub(crate) fn open_uploads(&self) -> usize {
        self.uploads
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .open
            .len()
    }
}

/// 모음 · 요청의 대상 — 같은 프로젝트 · 워크트리 · 실제 경로여야 이어 쓰거나 커밋한다.
trait Target {
    fn project_id(&self) -> &str;
    fn worktree(&self) -> Option<&str>;
}

impl Target for FsWriteChunkParams {
    fn project_id(&self) -> &str {
        &self.project_id
    }
    fn worktree(&self) -> Option<&str> {
        self.worktree.as_deref()
    }
}

impl Target for FsWriteCommitParams {
    fn project_id(&self) -> &str {
        &self.project_id
    }
    fn worktree(&self) -> Option<&str> {
        self.worktree.as_deref()
    }
}

fn same_target(upload: &Upload, params: &impl Target, real: &Path) -> Result<(), RpcError> {
    if upload.project_id == params.project_id()
        && upload.worktree.as_deref() == params.worktree()
        && upload.real == real
    {
        Ok(())
    } else {
        Err(RpcError::invalid_params(
            "the upload was opened for another file",
        ))
    }
}

fn misplaced(expected: u64) -> RpcError {
    RpcError::invalid_params(format!(
        "the chunk must start at byte {expected} of the upload"
    ))
}

/// 쉰 모음을 시계로 치운다 — 아무도 쓰기를 부르지 않아도 [`UPLOAD_IDLE`] 이 지난 모음은 [`SWEEP_EVERY`] 안에 사라진다. 모음이 없고
/// 받는 중인 조각도 없으면 멈춘다(다음 `fs.writeChunk` 가 다시 띄운다).
async fn reap(fs: Arc<Fs>) {
    loop {
        tokio::time::sleep(SWEEP_EVERY).await;
        let mut uploads = fs.uploads.lock().unwrap_or_else(PoisonError::into_inner);
        uploads.sweep(Instant::now());
        if uploads.open.is_empty() && uploads.pending == 0 {
            uploads.reaping = false;
            return;
        }
    }
}

fn too_large() -> RpcError {
    RpcError::invalid_params(format!(
        "a chunked write is at most {UPLOAD_MAX} bytes; nothing was written"
    ))
}

/// 이 세션의 모음. 다른 세션의 것은 없는 것과 같다 — id 를 알아도 남의 연결로는 이어 쓰지 못한다.
fn owned<'a>(
    open: &'a mut HashMap<String, Upload>,
    id: &str,
    session: SessionId,
) -> Result<&'a mut Upload, RpcError> {
    match open.get_mut(id) {
        Some(u) if u.session == session => Ok(u),
        _ => Err(unknown_upload(id)),
    }
}

pub(crate) fn register(router: &mut Router, fs: Arc<Fs>) {
    async fn blocking<T: Send + 'static>(
        f: impl FnOnce() -> Result<T, RpcError> + Send + 'static,
    ) -> Result<T, RpcError> {
        tokio::task::spawn_blocking(f)
            .await
            .map_err(|e| RpcError::internal(e.to_string()))?
    }
    let (read, chunk, commit) = (Arc::clone(&fs), Arc::clone(&fs), Arc::clone(&fs));
    router
        .unary(FsReadChunkMethod, move |ctx, params: FsReadChunkParams| {
            let s = Arc::clone(&read);
            blocking(move || s.read_chunk_in(ctx.file_scope(), &params))
        })
        .unary(
            FsWriteChunkMethod,
            move |ctx, params: FsWriteChunkParams| {
                let s = Arc::clone(&chunk);
                async move {
                    let session = ctx.session();
                    let closed = ctx.connection_closed();
                    let cancel = ctx.cancellation().clone();
                    // 감시 · 치우기는 블로킹 일 앞에서 띄운다 — 연결이 닫히거나 요청이 취소되면 세션은 이 future 를 버리지만
                    // 블로킹 일은 끝까지 돈다(그때 만든 모음도 이 둘이 치운다).
                    let (watch, reap_needed) = s.arrive(session);
                    if watch {
                        let (s, closed) = (Arc::clone(&s), closed.clone());
                        tokio::spawn(async move {
                            closed.cancelled().await;
                            s.drop_uploads(session);
                        });
                    }
                    if reap_needed {
                        tokio::spawn(reap(Arc::clone(&s)));
                    }
                    blocking(move || {
                        let gone = || closed.is_cancelled() || cancel.is_cancelled();
                        let result = s.write_chunk_in(session, ctx.scope(), &params, &gone);
                        s.depart();
                        result
                    })
                    .await
                }
            },
        )
        .unary(
            FsWriteCommitMethod,
            move |ctx, params: FsWriteCommitParams| {
                let s = Arc::clone(&commit);
                blocking(move || s.write_commit_in(ctx.session(), ctx.scope(), &params))
            },
        )
        .unary(
            FsWriteAbortMethod,
            move |ctx, params: FsWriteAbortParams| {
                let s = Arc::clone(&fs);
                async move {
                    s.write_abort(ctx.session(), &params);
                    Ok(())
                }
            },
        );
}

#[cfg(test)]
#[path = "chunks_tests.rs"]
mod tests;
