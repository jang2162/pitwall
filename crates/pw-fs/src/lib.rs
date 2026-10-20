//! 파일 시스템 서비스(`docs/design/12-m1-plan.md` M1-01): 프로젝트 루트 안의 목록 · 상태 · 읽기 · 원자 쓰기.
//!
//! - [`scope`]: 접근 범위(결정 M1-D8) — 프로젝트 루트(활성 워크트리 포함) 밖 경로를 거절한다.
//! - [`judge`]: 판정(텍스트/바이너리 · 인코딩 · 줄바꿈 · BOM)과 크기 등급(결정 M1-D9)의 기준값.
//! - [`list`]: 폴더 한 단계 목록과 ignore 여부.
//! - [`read`]: 전체 · 범위 읽기와 `version`.
//! - [`write`]: 임시 파일 → rename 원자 쓰기.
//! - [`methods`]: `fs.list` · `fs.stat` · `fs.read` · `fs.write` RPC.

pub mod judge;
pub mod list;
pub mod methods;
pub mod read;
pub mod scope;
pub mod write;

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use pw_core::registry::methods::rpc_error;
use pw_core::registry::path::{normalize, worktree_link};
use pw_core::registry::{Project, Registry as Projects};
use pw_rpc::{RpcError, code};

use crate::judge::{Bom, READ_ONLY_MAX, SNIFF_BYTES, judge, size_class};
use crate::list::{EntryKind, mtime_ms};
use crate::methods::{
    FsContent, FsList, FsPathParams, FsRead, FsReadParams, FsStat, FsWriteParams, FsWritten,
};
use crate::scope::{Denied, Roots};

const UTF8_BOM: &[u8] = &[0xEF, 0xBB, 0xBF];

/// 파일 시스템 서비스. Host 를 만든 뒤 프로젝트 레지스트리를 붙인다([`Fs::attach`]).
#[derive(Default)]
pub struct Fs {
    projects: OnceLock<Projects>,
}

impl Fs {
    pub fn new() -> Self {
        Self::default()
    }

    /// 두 번째부터는 무시한다.
    pub fn attach(&self, projects: Projects) {
        let _ = self.projects.set(projects);
    }

    /// 프로젝트의 등록 루트와, `worktree` 가 있으면 그 워크트리 안의 프로젝트 루트.
    pub fn roots(&self, project_id: &str, worktree: Option<&str>) -> Result<Roots, RpcError> {
        let projects = self.projects.get().ok_or_else(|| {
            RpcError::internal("the file system service is not attached to a project registry")
        })?;
        let project = projects.get(project_id).map_err(rpc_error)?;
        let mut roots: Vec<PathBuf> = project.roots.iter().map(PathBuf::from).collect();
        if let Some(top) = worktree {
            roots.push(worktree_root(&project, top)?);
        }
        Ok(Roots::new(roots))
    }

    pub fn list(&self, params: &FsPathParams) -> Result<FsList, RpcError> {
        let roots = self.roots(&params.project_id, params.worktree.as_deref())?;
        let dir = roots.readable(Path::new(&params.path)).map_err(denied)?;
        let entries = list::list(&dir).map_err(|e| io_error(&dir.real, e))?;
        Ok(FsList {
            path: dir.real.display().to_string(),
            entries,
        })
    }

    pub fn stat(&self, params: &FsPathParams) -> Result<FsStat, RpcError> {
        let roots = self.roots(&params.project_id, params.worktree.as_deref())?;
        let requested = Path::new(&params.path);
        let target = roots.readable(requested).map_err(denied)?;
        let lmeta = fs::symlink_metadata(requested).map_err(|e| io_error(&target.real, e))?;
        let kind = EntryKind::of(lmeta.file_type());
        let (target_kind, meta) = if kind == EntryKind::Symlink {
            match fs::metadata(requested) {
                Ok(m) => (Some(EntryKind::of(m.file_type())), m),
                Err(_) => (None, lmeta),
            }
        } else {
            (None, lmeta)
        };
        Ok(FsStat {
            path: target.real.display().to_string(),
            kind,
            target: target_kind,
            size: meta.len(),
            mtime_ms: mtime_ms(&meta),
            readonly: meta.permissions().readonly(),
            size_class: meta.is_file().then(|| size_class(meta.len())),
        })
    }

    pub fn read(&self, params: &FsReadParams) -> Result<FsRead, RpcError> {
        let roots = self.roots(&params.project_id, params.worktree.as_deref())?;
        let target = roots.readable(Path::new(&params.path)).map_err(denied)?;
        let real = &target.real;
        let meta = fs::metadata(real).map_err(|e| io_error(real, e))?;
        if meta.is_dir() {
            return Err(RpcError::invalid_params(format!(
                "`{}` is a folder",
                real.display()
            )));
        }
        if params.length.is_some_and(|l| l > READ_ONLY_MAX) {
            return Err(RpcError::invalid_params(format!(
                "a range read is at most {READ_ONLY_MAX} bytes"
            )));
        }
        if params.offset.is_none() && params.length.is_none() {
            if meta.len() > READ_ONLY_MAX {
                return Err(RpcError::invalid_params(format!(
                    "`{}` is larger than {READ_ONLY_MAX} bytes; read a range",
                    real.display()
                )));
            }
            let snap = read::read_whole(real).map_err(|e| io_error(real, e))?;
            let head = &snap.bytes[..snap.bytes.len().min(SNIFF_BYTES)];
            let start = Bom::detect(head).map_or(0, Bom::byte_len);
            let judgment = judge(head, &snap.bytes[start..], snap.meta.len(), false);
            return Ok(FsRead {
                path: real.display().to_string(),
                size: snap.meta.len(),
                mtime_ms: mtime_ms(&snap.meta),
                offset: 0,
                eof: true,
                version: Some(snap.version()),
                content: content(params.bytes, judgment.binary, &snap.bytes, start),
                judgment,
            });
        }
        let offset = params.offset.unwrap_or(0);
        let range = read::read_range(real, offset, params.length.unwrap_or(READ_ONLY_MAX))
            .map_err(|e| io_error(real, e))?;
        let size = range.meta.len();
        let eof = offset.saturating_add(range.bytes.len() as u64) >= size;
        let start = if offset == 0 {
            Bom::detect(&range.head)
                .map_or(0, Bom::byte_len)
                .min(range.bytes.len())
        } else {
            0
        };
        let judgment = judge(&range.head, &range.bytes[start..], size, !eof);
        Ok(FsRead {
            path: real.display().to_string(),
            size,
            mtime_ms: mtime_ms(&range.meta),
            offset,
            eof,
            version: None,
            content: content(params.bytes, judgment.binary, &range.bytes, start),
            judgment,
        })
    }

    pub fn write(&self, params: &FsWriteParams) -> Result<FsWritten, RpcError> {
        self.write_with(params, |_| Ok(()))
    }

    fn write_with(
        &self,
        params: &FsWriteParams,
        fault: impl Fn(write::Stage) -> io::Result<()>,
    ) -> Result<FsWritten, RpcError> {
        let roots = self.roots(&params.project_id, params.worktree.as_deref())?;
        let real = roots.writable(Path::new(&params.path)).map_err(denied)?;
        let bytes = match (&params.content, params.bom) {
            (FsContent::Text { text }, bom) => {
                let mut bytes = Vec::with_capacity(text.len() + 3);
                if bom {
                    bytes.extend_from_slice(UTF8_BOM);
                }
                bytes.extend_from_slice(text.as_bytes());
                bytes
            }
            (FsContent::Base64 { .. }, true) => {
                return Err(RpcError::invalid_params(
                    "`bom` applies only to text content",
                ));
            }
            (FsContent::Base64 { data }, false) => BASE64
                .decode(data)
                .map_err(|e| RpcError::invalid_params(format!("content is not base64: {e}")))?,
        };
        let current = match fs::metadata(&real) {
            Ok(meta) if meta.is_dir() => {
                return Err(RpcError::invalid_params(format!(
                    "`{}` is a folder",
                    real.display()
                )));
            }
            Ok(_) => Some(read::read_whole(&real).map_err(|e| io_error(&real, e))?),
            Err(e) if e.kind() == io::ErrorKind::NotFound => None,
            Err(e) => return Err(io_error(&real, e)),
        };
        let current_version = current.as_ref().map(read::Snapshot::version);
        if current_version != params.expected_version {
            return Err(RpcError::new(code::CONFLICT, "conflict").with_data(
                serde_json::json!({ "path": real.display().to_string(), "current": current_version }),
            ));
        }
        if current
            .as_ref()
            .is_some_and(|s| s.meta.permissions().readonly())
        {
            return Err(RpcError::invalid_params(format!(
                "`{}` is read-only",
                real.display()
            )));
        }
        write::write_atomic_with(&real, &bytes, fault).map_err(|e| io_error(&real, e))?;
        let meta = fs::metadata(&real).map_err(|e| io_error(&real, e))?;
        Ok(FsWritten {
            path: real.display().to_string(),
            size: meta.len(),
            mtime_ms: mtime_ms(&meta),
            version: read::version(&bytes, &meta),
        })
    }
}

/// 텍스트(UTF-8, 앞의 BOM 을 뺀 것)로 보낼 수 있으면 텍스트, 아니면 바이트 그대로 base64.
fn content(force_bytes: bool, binary: bool, raw: &[u8], start: usize) -> FsContent {
    if !force_bytes
        && !binary
        && let Ok(text) = std::str::from_utf8(&raw[start..])
        && (start == 0 || raw.starts_with(UTF8_BOM))
    {
        return FsContent::Text {
            text: text.to_owned(),
        };
    }
    FsContent::Base64 {
        data: BASE64.encode(raw),
    }
}

/// 활성 워크트리 안의 프로젝트 루트(`ActiveWorktree::root` 와 같은 계산). `top` 이 그 프로젝트의 워크트리가 아니면 invalid params.
fn worktree_root(project: &Project, top: &str) -> Result<PathBuf, RpcError> {
    let not_ours = || {
        RpcError::invalid_params(format!(
            "`{top}` is not a worktree of project `{}`",
            project.id
        ))
    };
    let top = normalize(Path::new(top)).map_err(|_| {
        RpcError::invalid_params(format!("worktree `{top}` must be an absolute path"))
    })?;
    let link = worktree_link(&top).ok_or_else(not_ours)?;
    let rel = project
        .root()
        .strip_prefix(&link.main_top)
        .map_err(|_| not_ours())?;
    // 빈 상대 경로를 `join` 하면 끝에 구분자가 붙는다.
    Ok(if rel.as_os_str().is_empty() {
        link.worktree_top
    } else {
        link.worktree_top.join(rel)
    })
}

fn denied(d: Denied) -> RpcError {
    RpcError::invalid_params(d.to_string())
}

fn io_error(path: &Path, e: io::Error) -> RpcError {
    let shown = path.display().to_string();
    match e.kind() {
        io::ErrorKind::NotFound => {
            RpcError::new(code::NOT_FOUND, "not found").with_data(serde_json::Value::String(shown))
        }
        io::ErrorKind::NotADirectory => {
            RpcError::invalid_params(format!("`{shown}` is not a folder"))
        }
        io::ErrorKind::IsADirectory => RpcError::invalid_params(format!("`{shown}` is a folder")),
        _ => RpcError::internal(format!("{shown}: {e}")),
    }
}

#[cfg(test)]
mod tests;
