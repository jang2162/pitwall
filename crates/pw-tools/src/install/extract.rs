//! 받은 파일을 설치 폴더에 푼다(결정 M4-D17 — 15-m4-plan.md M4-04 ①). 받은 것은 미신뢰다.
//!
//! - 형식은 저장 이름으로 정한다(M4-03 [`ArchiveFormat`]): `.tar.gz` · `.zip` · `.gz`(단일 파일) · 그 밖(받은 그대로). 번들 패키지가
//!   쓰지 않는 `.tar` · `.tar.xz` 는 「지원하지 않음」 이다(새로 필요해지면 그 행이 더한다).
//! - 항목 경로가 설치 폴더 밖(`..` · 절대 경로 · 드라이브 · `\`)이거나, 앞서 푼 링크를 지나거나, 같은 이름이 두 번 나오면 **그 설치
//!   전체를 실패**로 한다(부르는 쪽이 설치 폴더를 지운다). 파일은 `create_new` 로 만들어 이미 있는 것(링크 포함)을 덮지 않는다.
//! - 심볼릭 링크는 가리키는 곳이 설치 폴더 안일 때만 둔다(링크가 놓인 폴더 기준으로 글자 그대로 풀어 본다). 하드 링크 · 장치 ·
//!   FIFO 항목은 실패다.
//! - 권한(유닉스): 폴더 0755, 파일은 보관 파일이 실행 비트를 적은 일반 파일만 0755, 나머지 0644 — setuid · setgid · sticky ·
//!   group/other 쓰기 비트는 남지 않는다. `bin` 에 적힌 것은 늘 0755 다([`make_executable`]). 보관 파일의 실행 비트를 남기는 까닭:
//!   kotlin-lsp 배포물은 JBR(Java 런타임)을 담고 `bin` 의 런처가 그 안의 `jbr/bin/java` 를 부른다(M4-03 실측). 설치 폴더 밖으로
//!   나가는 길은 위의 경로 · 링크 검사가 막고, Pitwall 은 `bin` 경로만 절대 경로로 부르므로 안쪽 실행 비트가 새 실행 경로를 만들지
//!   않는다(메인 판정 2026-11-02).
//! - 풀린 바이트는 설치 하나에 상한이 있다([`Budget`] — 기본 2 GiB). 적힌 크기를 믿지 않고 실제로 쓴 바이트를 센다.

use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};

use crate::package::ArchiveFormat;

/// 풀기 실패.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtractError {
    /// 폴더 밖 경로 · 링크를 지나는 항목 · 같은 이름 두 번 · 폴더 밖 링크 · 허용하지 않는 항목 종류.
    Unsafe {
        entry: String,
        reason: &'static str,
    },
    /// 풀린 바이트가 상한을 넘었다.
    TooLarge {
        limit: u64,
    },
    /// 이 형식은 풀지 않는다.
    Unsupported {
        format: ArchiveFormat,
    },
    /// 보관 파일이 깨졌다.
    Corrupt(String),
    Io(String),
}

impl std::fmt::Display for ExtractError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExtractError::Unsafe { entry, reason } => {
                write!(f, "unsafe archive entry {entry:?}: {reason}")
            }
            ExtractError::TooLarge { limit } => {
                write!(f, "the archive unpacks to more than {limit} bytes")
            }
            ExtractError::Unsupported { format } => write!(f, "cannot unpack {format:?} archives"),
            ExtractError::Corrupt(e) => write!(f, "broken archive: {e}"),
            ExtractError::Io(e) => write!(f, "cannot unpack: {e}"),
        }
    }
}

impl std::error::Error for ExtractError {}

fn io_err(e: io::Error) -> ExtractError {
    ExtractError::Io(e.to_string())
}

/// 설치 하나가 풀 수 있는 남은 바이트.
#[derive(Debug, Clone, Copy)]
pub struct Budget {
    pub limit: u64,
    pub used: u64,
}

impl Budget {
    pub fn new(limit: u64) -> Self {
        Budget { limit, used: 0 }
    }

    fn take(&mut self, n: u64) -> Result<(), ExtractError> {
        if self.used + n > self.limit {
            return Err(ExtractError::TooLarge { limit: self.limit });
        }
        self.used += n;
        Ok(())
    }
}

/// 설치 폴더 하나에 푸는 상태. 설치의 모든 파일이 같은 것을 쓴다(같은 이름 두 번 · 링크 지나기는 파일을 넘어서도 본다).
pub struct Unpacker {
    root: PathBuf,
    budget: Budget,
    /// 지금까지 만든 항목(설치 폴더 기준). 폴더는 명시한 것만.
    seen: BTreeSet<PathBuf>,
    /// 지금까지 만든 심볼릭 링크.
    links: BTreeSet<PathBuf>,
}

/// 항목 종류.
enum Kind {
    File { mode: Option<u32> },
    Dir,
    Symlink(String),
}

impl Unpacker {
    /// `root` 는 비어 있는 설치 폴더(이미 있어야 한다).
    pub fn new(root: PathBuf, unpacked_limit: u64) -> Self {
        Unpacker {
            root,
            budget: Budget::new(unpacked_limit),
            seen: BTreeSet::new(),
            links: BTreeSet::new(),
        }
    }

    pub fn unpacked(&self) -> u64 {
        self.budget.used
    }

    /// 받은 파일 `archive` 를 `dest`(설치 폴더 기준, `/` 로 끝나면 폴더. 없으면 설치 폴더)에 푼다. `name` 은 저장 이름이다.
    /// `archive` 는 해시를 센 그 핸들이다 — 이름으로 다시 열지 않는다(리뷰 F5 — 대조한 바이트와 푸는 바이트가 같게). 처음부터 읽는다.
    pub fn unpack(
        &mut self,
        mut archive: File,
        name: &str,
        format: ArchiveFormat,
        dest: Option<&str>,
    ) -> Result<(), ExtractError> {
        let dest = dest.unwrap_or("");
        archive.seek(SeekFrom::Start(0)).map_err(io_err)?;
        match format {
            ArchiveFormat::TarGz => {
                let base = self.dir_prefix(dest)?;
                self.untar(flate2::read::GzDecoder::new(archive), &base)
            }
            ArchiveFormat::Zip => {
                let base = self.dir_prefix(dest)?;
                self.unzip(archive, &base)
            }
            ArchiveFormat::Gz => {
                let stem = name
                    .strip_suffix(".gz")
                    .or_else(|| name.strip_suffix(".GZ"))
                    .unwrap_or(name);
                let rel = self.file_target(dest, stem)?;
                self.write_entry(
                    &rel,
                    Kind::File { mode: None },
                    flate2::read::GzDecoder::new(archive),
                )
            }
            ArchiveFormat::Plain => {
                let rel = self.file_target(dest, name)?;
                self.write_entry(&rel, Kind::File { mode: None }, archive)
            }
            ArchiveFormat::Tar | ArchiveFormat::TarXz => Err(ExtractError::Unsupported { format }),
        }
    }

    /// 폴더로 쓰이는 `dest`(비거나 `/` 로 끝난다).
    fn dir_prefix(&mut self, dest: &str) -> Result<PathBuf, ExtractError> {
        if dest.is_empty() {
            return Ok(PathBuf::new());
        }
        let rel = clean(dest.trim_end_matches('/'), dest)?;
        self.ensure_dir(&rel)?;
        Ok(rel)
    }

    /// 단일 파일의 자리: `dest` 가 폴더면 그 안의 `name`, 파일 경로면 그것.
    fn file_target(&mut self, dest: &str, name: &str) -> Result<PathBuf, ExtractError> {
        if dest.is_empty() {
            return clean(name, name);
        }
        if dest.ends_with('/') {
            let dir = clean(dest.trim_end_matches('/'), dest)?;
            self.ensure_dir(&dir)?;
            return Ok(dir.join(clean(name, name)?));
        }
        clean(dest, dest)
    }

    fn untar(&mut self, reader: impl Read, base: &Path) -> Result<(), ExtractError> {
        let mut archive = tar::Archive::new(reader);
        let entries = archive
            .entries()
            .map_err(|e| ExtractError::Corrupt(e.to_string()))?;
        for entry in entries {
            let mut entry = entry.map_err(|e| ExtractError::Corrupt(e.to_string()))?;
            let raw = entry.path_bytes().into_owned();
            let display = String::from_utf8_lossy(&raw).into_owned();
            let Ok(text) = std::str::from_utf8(&raw) else {
                return Err(unsafe_entry(&display, "the name is not UTF-8"));
            };
            let header = entry.header();
            let kind = match header.entry_type() {
                tar::EntryType::Regular | tar::EntryType::Continuous => Kind::File {
                    mode: header.mode().ok(),
                },
                tar::EntryType::Directory => Kind::Dir,
                tar::EntryType::Symlink => {
                    let target = entry
                        .link_name_bytes()
                        .ok_or_else(|| unsafe_entry(&display, "a link without a target"))?;
                    let target = std::str::from_utf8(&target)
                        .map_err(|_| unsafe_entry(&display, "the link target is not UTF-8"))?
                        .to_owned();
                    Kind::Symlink(target)
                }
                tar::EntryType::XGlobalHeader | tar::EntryType::XHeader => continue,
                _ => {
                    return Err(unsafe_entry(
                        &display,
                        "hard links and special files are not allowed",
                    ));
                }
            };
            let Some(rel) = clean_entry(text)? else {
                continue;
            };
            self.write_entry(&base.join(rel), kind, &mut entry)?;
        }
        Ok(())
    }

    fn unzip(&mut self, mut file: File, base: &Path) -> Result<(), ExtractError> {
        let declared = zip_declared_entries(&mut file)?;
        file.seek(SeekFrom::Start(0)).map_err(io_err)?;
        let mut zip =
            zip::ZipArchive::new(file).map_err(|e| ExtractError::Corrupt(e.to_string()))?;
        // `zip` 크레이트는 같은 이름의 항목을 하나로 접는다 — 중앙 디렉터리가 적은 항목 수와 다르면 같은 이름이 두 번 있다.
        if declared != Some(zip.len() as u64) {
            return Err(unsafe_entry(
                "the zip archive",
                "the same name appears twice",
            ));
        }
        for i in 0..zip.len() {
            let mut entry = zip
                .by_index(i)
                .map_err(|e| ExtractError::Corrupt(e.to_string()))?;
            let name = entry
                .name()
                .map_err(|e| ExtractError::Corrupt(e.to_string()))?
                .into_owned();
            let kind = if entry.is_symlink() {
                let mut target = String::new();
                // 링크의 대상 글은 짧다 — 상한을 두고 읽는다.
                (&mut entry)
                    .take(4096)
                    .read_to_string(&mut target)
                    .map_err(|_| unsafe_entry(&name, "the link target is not UTF-8"))?;
                Kind::Symlink(target)
            } else if entry.is_dir() {
                Kind::Dir
            } else {
                Kind::File {
                    mode: entry.unix_mode(),
                }
            };
            let Some(rel) = clean_entry(name.trim_end_matches('/'))? else {
                continue;
            };
            self.write_entry(&base.join(rel), kind, &mut entry)?;
        }
        Ok(())
    }

    /// 항목 하나를 만든다. `rel` 은 깨끗한 상대 경로다.
    fn write_entry(
        &mut self,
        rel: &Path,
        kind: Kind,
        mut body: impl Read,
    ) -> Result<(), ExtractError> {
        let display = rel.to_string_lossy().into_owned();
        if !self.seen.insert(rel.to_path_buf()) {
            return Err(unsafe_entry(&display, "the same name appears twice"));
        }
        if let Some(parent) = rel.parent() {
            self.ensure_dir(parent)?;
        }
        let path = self.root.join(rel);
        match kind {
            Kind::Dir => {
                self.check_not_through_link(rel)?;
                match fs::symlink_metadata(&path) {
                    Ok(m) if m.is_dir() => Ok(()),
                    Ok(_) => Err(unsafe_entry(&display, "a folder over an existing file")),
                    Err(_) => {
                        fs::create_dir(&path).map_err(io_err)?;
                        set_mode(&path, 0o755)
                    }
                }
            }
            Kind::File { mode } => {
                let mut out = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&path)
                    .map_err(|e| {
                        if e.kind() == io::ErrorKind::AlreadyExists {
                            unsafe_entry(&display, "the same name appears twice")
                        } else {
                            io_err(e)
                        }
                    })?;
                let mut buf = vec![0u8; 64 * 1024];
                loop {
                    let n = body
                        .read(&mut buf)
                        .map_err(|e| ExtractError::Corrupt(e.to_string()))?;
                    if n == 0 {
                        break;
                    }
                    self.budget.take(n as u64)?;
                    out.write_all(&buf[..n]).map_err(io_err)?;
                }
                drop(out);
                let exec = mode.is_some_and(|m| m & 0o111 != 0);
                set_mode(&path, if exec { 0o755 } else { 0o644 })
            }
            Kind::Symlink(target) => {
                self.check_link_target(rel, &target)?;
                make_symlink(&target, &path).map_err(|e| {
                    if e.kind() == io::ErrorKind::AlreadyExists {
                        unsafe_entry(&display, "the same name appears twice")
                    } else {
                        io_err(e)
                    }
                })?;
                self.links.insert(rel.to_path_buf());
                Ok(())
            }
        }
    }

    /// 풀기를 모두 마친 뒤(설치의 모든 파일) 만든 링크마다 실제로 풀어 설치 폴더 안인지 본다(리뷰 F1). 링크를 만들 때의 검사는
    /// 대상을 글자 그대로 풀므로, 앞서 푼 안쪽 링크를 지난 뒤의 `..`(커널은 링크를 먼저 따라간 뒤 `..` 를 적용한다)가 폴더 밖으로
    /// 나가는 연쇄를 못 본다 — 그리고 뒤에 오는 링크가 앞 링크의 뜻을 바꿀 수 있으므로 모든 링크가 생긴 뒤에 본다.
    pub fn finish(&self) -> Result<(), ExtractError> {
        for link in &self.links {
            let display = link.to_string_lossy().into_owned();
            let parent: Vec<std::ffi::OsString> = link
                .parent()
                .map(|p| p.components().map(|c| c.as_os_str().to_owned()).collect())
                .unwrap_or_default();
            let target = fs::read_link(self.root.join(link)).map_err(io_err)?;
            match resolve_inside(&self.root, parent, &target, 0) {
                Some(stack) if !stack.is_empty() => {}
                _ => {
                    return Err(unsafe_entry(
                        &display,
                        "the link points outside the install folder",
                    ));
                }
            }
        }
        Ok(())
    }

    /// `rel` 폴더와 그 위가 있는지 보고 없으면 만든다. 앞서 푼 링크나 디스크의 링크를 지나면 실패.
    fn ensure_dir(&mut self, rel: &Path) -> Result<(), ExtractError> {
        let mut acc = PathBuf::new();
        for c in rel.components() {
            acc.push(c);
            let display = acc.to_string_lossy().into_owned();
            if self.links.contains(&acc) {
                return Err(unsafe_entry(&display, "the path goes through a link"));
            }
            let path = self.root.join(&acc);
            match fs::symlink_metadata(&path) {
                Ok(m) if m.file_type().is_symlink() => {
                    return Err(unsafe_entry(&display, "the path goes through a link"));
                }
                Ok(m) if m.is_dir() => {}
                Ok(_) => return Err(unsafe_entry(&display, "a folder over an existing file")),
                Err(_) => {
                    fs::create_dir(&path).map_err(io_err)?;
                    set_mode(&path, 0o755)?;
                }
            }
        }
        Ok(())
    }

    fn check_not_through_link(&self, rel: &Path) -> Result<(), ExtractError> {
        let mut acc = PathBuf::new();
        for c in rel.components() {
            acc.push(c);
            if self.links.contains(&acc) {
                return Err(unsafe_entry(
                    &rel.to_string_lossy(),
                    "the path goes through a link",
                ));
            }
        }
        Ok(())
    }

    /// 링크가 가리키는 곳이 설치 폴더 안인가(링크가 놓인 폴더 기준으로 글자 그대로 푼다).
    fn check_link_target(&self, rel: &Path, target: &str) -> Result<(), ExtractError> {
        let display = rel.to_string_lossy().into_owned();
        let outside = || unsafe_entry(&display, "the link points outside the install folder");
        if target.is_empty() || target.contains('\\') || target.contains(':') {
            return Err(outside());
        }
        let t = Path::new(target);
        if t.is_absolute() {
            return Err(outside());
        }
        let mut stack: Vec<Component<'_>> = rel
            .parent()
            .map(|p| p.components().collect())
            .unwrap_or_default();
        for c in t.components() {
            match c {
                Component::Normal(_) => stack.push(c),
                Component::CurDir => {}
                Component::ParentDir => {
                    if stack.pop().is_none() {
                        return Err(outside());
                    }
                }
                Component::RootDir | Component::Prefix(_) => return Err(outside()),
            }
        }
        if stack.is_empty() {
            // 설치 폴더 자신을 가리키는 링크도 받지 않는다(그 링크를 지나 폴더 밖 이름을 만들 수 있다).
            return Err(outside());
        }
        Ok(())
    }
}

/// 중앙 디렉터리 끝 레코드(EOCD)가 적은 항목 수(zip64 면 zip64 EOCD 의 것). 찾지 못하면 `None`.
fn zip_declared_entries(f: &mut File) -> Result<Option<u64>, ExtractError> {
    let len = f.metadata().map_err(io_err)?.len();
    // EOCD 는 22 바이트 + 주석(최대 65535).
    let tail_len = len.min(22 + 65_535 + 20);
    f.seek(SeekFrom::Start(len - tail_len)).map_err(io_err)?;
    let mut tail = vec![0u8; tail_len as usize];
    f.read_exact(&mut tail).map_err(io_err)?;
    let Some(at) = (0..tail.len().saturating_sub(21))
        .rev()
        .find(|&i| tail[i..i + 4] == [0x50, 0x4b, 0x05, 0x06])
    else {
        return Ok(None);
    };
    let total = u16::from_le_bytes([tail[at + 10], tail[at + 11]]);
    if total != 0xffff {
        return Ok(Some(u64::from(total)));
    }
    // zip64: EOCD 바로 앞의 zip64 EOCD 위치 표지(20 바이트)가 zip64 EOCD 의 자리를 준다.
    if at < 20 || tail[at - 20..at - 16] != [0x50, 0x4b, 0x06, 0x07] {
        return Ok(None);
    }
    let mut off = [0u8; 8];
    off.copy_from_slice(&tail[at - 12..at - 4]);
    let record = u64::from_le_bytes(off);
    f.seek(SeekFrom::Start(record)).map_err(io_err)?;
    let mut head = [0u8; 40];
    f.read_exact(&mut head).map_err(io_err)?;
    if head[..4] != [0x50, 0x4b, 0x06, 0x06] {
        return Ok(None);
    }
    let mut n = [0u8; 8];
    n.copy_from_slice(&head[32..40]);
    Ok(Some(u64::from_le_bytes(n)))
}

/// 링크 풀이를 따라가는 깊이의 상한(커널의 `ELOOP` 와 같은 자리).
const MAX_LINK_DEPTH: usize = 40;

/// `root` 기준 `start`(조각들) 폴더에서 `target` 을 커널처럼 푼다: 조각마다 그 자리가 링크면 그 링크를 먼저 풀고, `..` 는 푼 뒤의
/// 자리에서 한 단계 올라간다. 설치 폴더 위로 올라가거나 절대 경로 · 너무 깊은 연쇄면 `None`. 없는 자리(끊어진 링크)는 글자대로 둔다.
fn resolve_inside(
    root: &Path,
    start: Vec<std::ffi::OsString>,
    target: &Path,
    depth: usize,
) -> Option<Vec<std::ffi::OsString>> {
    if depth > MAX_LINK_DEPTH || target.is_absolute() {
        return None;
    }
    let mut stack = start;
    for c in target.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                stack.pop()?;
            }
            Component::Normal(n) => {
                stack.push(n.to_owned());
                let here: PathBuf = stack.iter().collect();
                let at = root.join(&here);
                if fs::symlink_metadata(&at).is_ok_and(|m| m.file_type().is_symlink()) {
                    let next = fs::read_link(&at).ok()?;
                    stack.pop();
                    stack = resolve_inside(root, stack, &next, depth + 1)?;
                }
            }
            Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    Some(stack)
}

fn unsafe_entry(entry: &str, reason: &'static str) -> ExtractError {
    ExtractError::Unsafe {
        entry: entry.chars().take(512).collect(),
        reason,
    }
}

/// 보관 파일 항목 이름을 깨끗한 상대 경로로. `.` 만인 항목은 `None`(건너뛴다).
fn clean_entry(name: &str) -> Result<Option<PathBuf>, ExtractError> {
    let trimmed = name.trim_start_matches("./");
    if trimmed.is_empty() || trimmed == "." {
        return Ok(None);
    }
    clean(trimmed, name).map(Some)
}

/// 상대 경로 검사: 비지 않고, 절대 · 드라이브 · `\` · `..` 가 없다. `.` 조각은 버린다.
fn clean(path: &str, display: &str) -> Result<PathBuf, ExtractError> {
    let bad = |reason| unsafe_entry(display, reason);
    if path.is_empty() {
        return Err(bad("an empty name"));
    }
    if path.starts_with('/') || path.contains('\\') || path.contains(':') || path.contains('\0') {
        return Err(bad("the name is outside the install folder"));
    }
    let mut out = PathBuf::new();
    for seg in path.split('/') {
        match seg {
            "" | "." => {}
            ".." => return Err(bad("the name is outside the install folder")),
            s => out.push(s),
        }
    }
    if out.as_os_str().is_empty() {
        return Err(bad("an empty name"));
    }
    Ok(out)
}

/// `bin` 에 적힌 파일을 0755 로. 설치 폴더 안의 일반 파일이어야 한다(링크면 그 대상도 설치 폴더 안이어야 한다).
pub fn make_executable(root: &Path, rel: &str) -> Result<PathBuf, ExtractError> {
    let rel_path = clean(rel, rel)?;
    let path = root.join(&rel_path);
    let real = fs::canonicalize(&path)
        .map_err(|_| unsafe_entry(rel, "the bin is not in the install folder"))?;
    let root_real = fs::canonicalize(root).map_err(io_err)?;
    if !real.starts_with(&root_real) {
        return Err(unsafe_entry(
            rel,
            "the bin points outside the install folder",
        ));
    }
    let meta = fs::metadata(&real).map_err(io_err)?;
    if !meta.is_file() {
        return Err(unsafe_entry(rel, "the bin is not a file"));
    }
    set_mode(&real, 0o755)?;
    Ok(path)
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> Result<(), ExtractError> {
    use std::os::unix::fs::PermissionsExt as _;
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).map_err(io_err)
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> Result<(), ExtractError> {
    Ok(())
}

#[cfg(unix)]
fn make_symlink(target: &str, link: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

/// Windows 에서는 보관 파일의 링크를 만들지 않는다(권한이 필요하고 번들 패키지의 Windows 배포물에는 없다).
#[cfg(not(unix))]
fn make_symlink(_target: &str, _link: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "symbolic links in archives are not supported on Windows",
    ))
}
