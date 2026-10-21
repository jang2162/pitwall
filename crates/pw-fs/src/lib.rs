//! 파일 시스템 서비스(`docs/design/12-m1-plan.md` M1-01): 프로젝트 루트 안의 목록 · 상태 · 읽기 · 원자 쓰기.
//!
//! - [`scope`]: 접근 범위(결정 M1-D8) — 프로젝트 루트(활성 워크트리 포함) 밖 경로를 거절한다.
//! - [`judge`]: 판정(텍스트/바이너리 · 인코딩 · 줄바꿈 · BOM)과 크기 등급(결정 M1-D9)의 기준값.
//! - [`list`]: 폴더 한 단계 목록과 ignore 여부.
//! - [`read`]: 전체 · 범위 읽기와 `version`.
//! - [`write`]: 임시 파일 → rename 원자 쓰기.
//! - [`ops`]: 파일 조작(M1-21) — `fs.create` · `fs.rename` · `fs.copy` · `fs.trash`, Finder 드롭 들여오기.
//! - [`search`]: 텍스트 검색 `search.text`(M1-07) 스트림.
//! - [`methods`]: `fs.list` · `fs.stat` · `fs.read` · `fs.write` RPC.
//! - [`watch`]: 파일 감시(M1-06) — `fs.watch` 스트림과 `fs.changed` 이벤트.
//! - [`index`]: 파일 목록 인덱스와 퍼지 매칭 `search.files`(M1-12).

pub mod handle;
mod ignore_rules;
pub mod index;
pub mod judge;
pub mod list;
pub mod methods;
pub mod ops;
pub mod read;
pub mod scope;
pub mod search;
pub mod watch;
pub mod write;

use std::fs;
use std::hash::{DefaultHasher, Hash as _, Hasher as _};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock, PoisonError};

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

/// 시험이 판정 뒤 · 열기 직전에 끼우는 동작.
#[cfg(test)]
type BeforeOpen = Box<dyn Fn(&Path) + Send>;

/// 쓰기 잠금의 갈래 수. 실제 경로의 해시로 고른다 — 다른 파일끼리 가끔 같은 갈래를 기다릴 뿐 맞음에는 영향이 없다.
const WRITE_LOCKS: usize = 32;

/// 파일 시스템 서비스. Host 를 만든 뒤 프로젝트 레지스트리를 붙인다([`Fs::attach`]).
pub struct Fs {
    projects: OnceLock<Projects>,
    /// 같은 파일의 「version 확인 → rename」 을 한 번에 하나씩. 핸들러는 블로킹 스레드에서 함께 돌므로, 묶지 않으면 같은
    /// `expectedVersion` 으로 들어온 두 쓰기가 모두 확인을 통과해 하나가 말없이 사라진다. 이 프로세스 안의 쓰기만 묶는다 —
    /// 다른 프로세스(에이전트)의 쓰기와의 경합은 잠금 없이 막을 수 없다(그 쓰기는 다음 `fs.write` 에서 `CONFLICT` 로 드러난다).
    write_locks: [Mutex<()>; WRITE_LOCKS],
    /// 시험: 판정 뒤 · 파일을 열기 직전에 부른다(그 사이 경로를 바꿔 읽기 경합을 재현한다).
    #[cfg(test)]
    before_open: Mutex<Option<BeforeOpen>>,
}

impl Default for Fs {
    fn default() -> Self {
        Self {
            projects: OnceLock::new(),
            write_locks: std::array::from_fn(|_| Mutex::new(())),
            #[cfg(test)]
            before_open: Mutex::new(None),
        }
    }
}

impl Fs {
    pub fn new() -> Self {
        Self::default()
    }

    /// 판정한 경로를 읽기로 연다. 연 핸들의 실제 경로를 다시 판정한다([`Roots::open_inside`]).
    fn open(&self, roots: &Roots, real: &Path) -> io::Result<fs::File> {
        #[cfg(test)]
        if let Some(hook) = self
            .before_open
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
        {
            hook(real);
        }
        roots.open_inside(real)
    }

    fn write_lock(&self, real: &Path) -> std::sync::MutexGuard<'_, ()> {
        let mut hasher = DefaultHasher::new();
        real.hash(&mut hasher);
        let slot = usize::try_from(hasher.finish() % WRITE_LOCKS as u64).unwrap_or(0);
        // 잠금 안에서 패닉한 쓰기가 있어도 지킬 상태가 없다(잠금은 순서만 정한다).
        self.write_locks[slot]
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
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
        let entries = list::list(&dir, &roots).map_err(|e| io_error(&dir.real, e))?;
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
            // 깨진 링크는 `fs.list` 처럼 0 이다(링크 자신의 길이가 아니라).
            size: if kind == EntryKind::Symlink && target_kind.is_none() {
                0
            } else {
                meta.len()
            },
            mtime_ms: mtime_ms(&meta),
            readonly: read_only(&target.real, &meta),
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
            let snap =
                read::read_whole(real, &|p| self.open(&roots, p)).map_err(|e| io_error(real, e))?;
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
        let range = read::read_range(real, offset, params.length.unwrap_or(READ_ONLY_MAX), &|p| {
            self.open(&roots, p)
        })
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
        let _guard = self.write_lock(&real);
        let current = match fs::metadata(&real) {
            Ok(meta) if meta.is_dir() => {
                return Err(RpcError::invalid_params(format!(
                    "`{}` is a folder",
                    real.display()
                )));
            }
            Ok(_) => Some(
                read::read_whole(&real, &|p| self.open(&roots, p))
                    .map_err(|e| io_error(&real, e))?,
            ),
            Err(e) if e.kind() == io::ErrorKind::NotFound => None,
            Err(e) => return Err(io_error(&real, e)),
        };
        let current_version = current.as_ref().map(read::Snapshot::version);
        if current_version != params.expected_version {
            return Err(RpcError::new(code::CONFLICT, "conflict").with_data(
                serde_json::json!({ "path": real.display().to_string(), "current": current_version }),
            ));
        }
        if current.as_ref().is_some_and(|s| read_only(&real, &s.meta)) {
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

/// 이 프로세스가 그 파일에 쓸 수 없다. unix 는 권한 비트가 아니라 실제 판정(`access(W_OK)`)이다 — 남의 소유 0644 파일은 쓰기
/// 비트가 있어도 쓸 수 없고, 폴더에 쓰기 권한만 있으면 rename 으로 바뀌어 소유자가 바뀐다. Windows 는 읽기 전용 속성.
fn read_only(path: &Path, meta: &fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt as _;
        let _ = meta;
        let Ok(c) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
            return true;
        };
        // SAFETY: `c` 는 NUL 로 끝나는 살아 있는 문자열이고 access 는 그것을 읽기만 한다.
        unsafe { libc::access(c.as_ptr(), libc::W_OK) != 0 }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        meta.permissions().readonly()
    }
}

/// 심링크 고리(ELOOP, Windows 는 `ERROR_CANT_RESOLVE_FILENAME`).
fn is_link_loop(e: &io::Error) -> bool {
    #[cfg(unix)]
    let code = libc::ELOOP;
    #[cfg(windows)]
    let code = 1921;
    e.raw_os_error() == Some(code)
}

fn denied(d: Denied) -> RpcError {
    RpcError::invalid_params(d.to_string())
}

fn io_error(path: &Path, e: io::Error) -> RpcError {
    let shown = path.display().to_string();
    if let Some(escaped) = e
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<scope::Escaped>())
    {
        return RpcError::invalid_params(escaped.to_string());
    }
    if is_link_loop(&e) {
        return RpcError::invalid_params(format!("`{shown}` goes through a symlink loop"));
    }
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
