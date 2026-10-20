//! 확장 폴더 안의 파일: 매니페스트가 가리키는 상대 경로([`RelPath`])와, 그것을 읽는 두 출처([`ExtensionRoot`]).

use std::borrow::Cow;
use std::fmt;
use std::io::{self, Read as _};
use std::path::{Path, PathBuf};

/// 확장 폴더의 매니페스트 파일 이름.
pub const MANIFEST: &str = "extension.toml";

/// 확장 폴더 안을 가리키는 상대 경로. `/` 로 나눈 마디만 담고, 폴더 밖으로 나가는 마디(`..`)는 만들 수 없다.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RelPath(String);

impl RelPath {
    /// 매니페스트에 적힌 경로를 읽는다. 끝의 `/`(폴더 표시)와 `.` 마디는 버린다.
    ///
    /// 받지 않는 것: 빈 경로, 절대 경로, `..` 마디, `\`(Windows 에서는 구분자라 `..\..` 로 밖에 나갈 수 있다), `:`
    /// (Windows 드라이브 · 대체 스트림). 심볼릭 링크로 밖에 나가는 것은 디스크를 읽을 때 [`ExtensionRoot`] 가 막는다.
    pub fn parse(raw: &str) -> Result<Self, String> {
        if raw.starts_with('/') {
            return Err(format!(
                "`{raw}` is an absolute path; use a path inside the extension folder"
            ));
        }
        if raw.contains('\\') {
            return Err(format!("`{raw}` contains `\\`; separate segments with `/`"));
        }
        if raw.contains(':') {
            return Err(format!(
                "`{raw}` contains `:`; use a path inside the extension folder"
            ));
        }
        let mut segments = Vec::new();
        for segment in raw.trim_end_matches('/').split('/') {
            match segment {
                "." => {}
                ".." => {
                    return Err(format!("`{raw}` points outside the extension folder"));
                }
                "" => return Err(format!("`{raw}` has an empty segment")),
                _ => segments.push(segment),
            }
        }
        if segments.is_empty() {
            return Err("path is empty".to_owned());
        }
        Ok(Self(segments.join("/")))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RelPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    File,
    Dir,
}

#[derive(Debug)]
pub enum SourceError {
    NotFound,
    /// 경로가 심볼릭 링크를 거쳐 확장 폴더 밖을 가리킨다.
    Escape,
    Io(io::Error),
}

impl fmt::Display for SourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SourceError::NotFound => f.write_str("not found"),
            SourceError::Escape => f.write_str("resolves outside the extension folder"),
            SourceError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for SourceError {}

/// 확장 하나의 폴더. 내장 확장을 바이너리에 묶는 이유는 `docs/design/07-extensibility.md` 「2. 확장 매니페스트」 「레지스트리 (`crates/pw-ext`)」 표의 「내장 확장」 행.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtensionRoot {
    /// 바이너리에 묶인 `extensions/<folder>/`.
    Builtin { folder: &'static str },
    /// 디스크의 폴더(`$DATA/extensions/<폴더>/`).
    Dir(PathBuf),
}

impl ExtensionRoot {
    /// 사람이 읽는 위치. 내장은 `builtin:<폴더>/<경로>` 로 적는다.
    pub fn display(&self, rel: Option<&RelPath>) -> String {
        match (self, rel) {
            (ExtensionRoot::Builtin { folder }, None) => format!("builtin:{folder}"),
            (ExtensionRoot::Builtin { folder }, Some(rel)) => format!("builtin:{folder}/{rel}"),
            (ExtensionRoot::Dir(dir), None) => dir.display().to_string(),
            (ExtensionRoot::Dir(dir), Some(rel)) => dir.join(rel.as_str()).display().to_string(),
        }
    }

    pub fn is_builtin(&self) -> bool {
        matches!(self, ExtensionRoot::Builtin { .. })
    }

    /// 경로가 가리키는 것이 파일인지 폴더인지. 없으면 [`SourceError::NotFound`].
    pub fn kind(&self, rel: &RelPath) -> Result<EntryKind, SourceError> {
        match self {
            ExtensionRoot::Builtin { folder } => {
                let key = format!("{folder}/{rel}");
                let prefix = format!("{key}/");
                let files = crate::builtin::files();
                if files.iter().any(|f| f.path == key) {
                    Ok(EntryKind::File)
                } else if files.iter().any(|f| f.path.starts_with(&prefix)) {
                    Ok(EntryKind::Dir)
                } else {
                    Err(SourceError::NotFound)
                }
            }
            ExtensionRoot::Dir(dir) => {
                let path = resolve_inside(dir, rel)?;
                let meta = std::fs::metadata(path).map_err(io_error)?;
                Ok(if meta.is_dir() {
                    EntryKind::Dir
                } else {
                    EntryKind::File
                })
            }
        }
    }

    /// 파일 크기(바이트).
    pub fn size(&self, rel: &RelPath) -> Result<u64, SourceError> {
        match self {
            ExtensionRoot::Builtin { .. } => builtin_file(self, rel).map(|f| f.size),
            ExtensionRoot::Dir(dir) => {
                let path = resolve_inside(dir, rel)?;
                Ok(std::fs::metadata(path).map_err(io_error)?.len())
            }
        }
    }

    /// 폴더 `rel` 바로 아래의 파일들(하위 폴더 · `.` 으로 시작하는 항목 · 이름이 UTF-8 이 아닌 것은 빼고). 이름순이다.
    pub fn files_in(&self, rel: &RelPath) -> Result<Vec<RelPath>, SourceError> {
        let mut names: Vec<String> = match self {
            ExtensionRoot::Builtin { folder } => {
                let prefix = format!("{folder}/{rel}/");
                crate::builtin::files()
                    .iter()
                    .filter_map(|f| f.path.strip_prefix(&prefix))
                    .filter(|rest| !rest.contains('/'))
                    .map(str::to_owned)
                    .collect()
            }
            ExtensionRoot::Dir(dir) => {
                let path = resolve_inside(dir, rel)?;
                let mut names = Vec::new();
                for entry in std::fs::read_dir(path).map_err(io_error)? {
                    let entry = entry.map_err(SourceError::Io)?;
                    let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                        continue;
                    };
                    // 링크는 따라가서 본다. 폴더 밖을 가리키는 링크는 읽을 때(`read`) 거절된다.
                    if !name.starts_with('.') && entry.path().is_file() {
                        names.push(name);
                    }
                }
                names
            }
        };
        names.retain(|n| !n.starts_with('.'));
        names.sort();
        Ok(names
            .into_iter()
            .map(|name| RelPath(format!("{rel}/{name}")))
            .collect())
    }

    /// 내장 확장의 압축된 파일(`.wasm`)은 풀어서 준다.
    pub fn read(&self, rel: &RelPath) -> Result<Cow<'static, [u8]>, SourceError> {
        match self {
            ExtensionRoot::Builtin { .. } => {
                let file = builtin_file(self, rel)?;
                if !file.brotli {
                    return Ok(Cow::Borrowed(file.bytes));
                }
                let mut out = Vec::with_capacity(usize::try_from(file.size).unwrap_or(0));
                brotli_decompressor::Decompressor::new(file.bytes, 1 << 16)
                    .read_to_end(&mut out)
                    .map_err(SourceError::Io)?;
                Ok(Cow::Owned(out))
            }
            ExtensionRoot::Dir(dir) => {
                let path = resolve_inside(dir, rel)?;
                std::fs::read(path).map(Cow::Owned).map_err(io_error)
            }
        }
    }
}

fn builtin_file(
    root: &ExtensionRoot,
    rel: &RelPath,
) -> Result<&'static crate::builtin::BuiltinFile, SourceError> {
    let ExtensionRoot::Builtin { folder } = root else {
        unreachable!("only called for built-in roots");
    };
    let key = format!("{folder}/{rel}");
    crate::builtin::files()
        .iter()
        .find(|f| f.path == key)
        .ok_or(SourceError::NotFound)
}

/// `dir/rel` 을 실제 경로로 풀고, 심볼릭 링크를 따라간 결과가 `dir` 안인지 본다. `rel` 자체는 [`RelPath::parse`] 가
/// 이미 `..` 를 걸렀다 — 여기서 막는 것은 확장 폴더 안의 링크가 밖을 가리키는 경우다.
fn resolve_inside(dir: &Path, rel: &RelPath) -> Result<PathBuf, SourceError> {
    let root = dir.canonicalize().map_err(io_error)?;
    let target = dir.join(rel.as_str()).canonicalize().map_err(io_error)?;
    if target.starts_with(&root) {
        Ok(target)
    } else {
        Err(SourceError::Escape)
    }
}

fn io_error(e: io::Error) -> SourceError {
    if e.kind() == io::ErrorKind::NotFound {
        SourceError::NotFound
    } else {
        SourceError::Io(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rel_path_keeps_segments_inside_the_folder() {
        assert_eq!(
            RelPath::parse("themes/a.json").unwrap().as_str(),
            "themes/a.json"
        );
        assert_eq!(
            RelPath::parse("./queries/kotlin/").unwrap().as_str(),
            "queries/kotlin"
        );
    }

    #[test]
    fn rel_path_rejects_escapes_and_absolute_paths() {
        for raw in [
            "../x.json",
            "themes/../../x.json",
            "..",
            "/etc/passwd",
            "..\\..\\x",
            "C:/x.json",
            "",
            "./",
            "a//b",
        ] {
            assert!(RelPath::parse(raw).is_err(), "{raw:?} must be rejected");
        }
    }

    #[cfg(unix)]
    #[test]
    fn dir_root_rejects_a_symlink_that_leaves_the_folder() {
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.json"), "{}").unwrap();
        let ext = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), ext.path().join("themes")).unwrap();
        std::fs::write(ext.path().join("inside.json"), "{}").unwrap();

        let root = ExtensionRoot::Dir(ext.path().to_owned());
        let escaped = RelPath::parse("themes/secret.json").unwrap();
        assert!(matches!(root.read(&escaped), Err(SourceError::Escape)));
        assert!(matches!(root.kind(&escaped), Err(SourceError::Escape)));
        let inside = RelPath::parse("inside.json").unwrap();
        assert_eq!(root.read(&inside).unwrap().as_ref(), b"{}");
    }
}
