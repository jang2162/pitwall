//! `.gitattributes` 의 linguist 속성(`linguist-language` · `linguist-vendored` · `linguist-generated` ·
//! `linguist-documentation`)을 `gix-attributes` 로 읽는다(결정 M1-D20).
//!
//! 읽는 파일은 GitHub 가 보는 것과 같다: 저장소 꼭대기부터 그 파일의 폴더까지의 `.gitattributes`, 그리고 `$GIT_DIR/info/attributes`
//! (연결 워크트리는 메인 저장소의 것). 사람의 전역 파일(`core.attributesFile`, `~/.config/git/attributes`)은 읽지 않는다 —
//! GitHub 서버에는 그것이 없으므로 넣으면 결과가 GitHub 와 갈린다. 패턴은 대소문자를 가린다(GitHub 와 같다).
//!
//! ⚠ 이 크레이트가 디스크에서 읽는 것은 이 모듈의 [`read_small`] 하나로만 읽는다. 클론한 저장소의 파일은 믿을 수 없다:
//! - **심링크를 따라가지 않는다**(git 도 `.gitattributes` 심링크를 읽지 않는다 — `unable to access … Too many levels of
//!   symbolic links`). 따라가면 링크 하나로 루트 밖 파일을 읽는다(결정 M1-D8).
//! - **일반 파일만 읽는다.** FIFO 를 열면 쓰는 쪽이 올 때까지 영구히 막히고(호출마다 블로킹 스레드 하나가 묶인다), 장치
//!   파일(`/dev/zero`)은 끝이 없다. 열 때 막히지 않게 열고(unix `O_NONBLOCK`) 연 핸들의 종류를 본다.
//! - **크기 상한**([`MAX_ATTRIBUTES_BYTES`])을 넘으면 파일째 무시한다(git 도 큰 속성 파일을 무시한다 — 기준은 100 MB 로 더 크다).

use std::collections::BTreeSet;
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use bstr::ByteSlice;
use gix_attributes::glob::pattern::Case;
use gix_attributes::search::{MetadataCollection, Outcome};
use gix_attributes::{Search, StateRef};

/// `.gitattributes` · `info/attributes` 한 파일의 상한. 넘으면 그 파일을 무시한다.
pub const MAX_ATTRIBUTES_BYTES: u64 = 1024 * 1024;

/// `.git` 파일(`gitdir: …`) · `commondir` 의 상한.
const MAX_POINTER_BYTES: u64 = 4096;

const NAMES: [&str; 4] = [
    "linguist-language",
    "linguist-vendored",
    "linguist-generated",
    "linguist-documentation",
];

/// 한 파일의 linguist 속성. `None` 은 「적히지 않았다」 — Linguist 자체 판정을 쓴다.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Attrs {
    pub language: Option<String>,
    pub vendored: Option<bool>,
    pub generated: Option<bool>,
    pub documentation: Option<bool>,
}

/// 한 저장소의 속성 파일들. 묶음 호출마다 그 묶음이 지나는 폴더의 파일만 한 번 읽는다.
pub struct RepoAttributes {
    search: Search,
    collection: MetadataCollection,
}

impl RepoAttributes {
    /// `top` 은 저장소(작업 트리) 꼭대기, `gitdir` 은 `info/attributes` 를 찾을 git 디렉터리, `dirs` 는 파일들이 든 폴더의
    /// `top` 기준 상대 경로(`/` 구분, 꼭대기는 빈 문자열). 읽지 못한 파일은 경고를 남기고 없는 것으로 본다.
    pub fn load<'a>(
        top: &Path,
        gitdir: Option<&Path>,
        dirs: impl IntoIterator<Item = &'a str>,
    ) -> Self {
        let mut collection = MetadataCollection::default();
        let no_globals: [PathBuf; 0] = [];
        let mut search = Search::new_globals(no_globals, &mut Vec::new(), &mut collection)
            .expect("the built-in attribute macros parse");
        // 모든 조상 폴더. 뒤에 넣은 목록이 먼저 맞으므로(우선순위가 높다) 얕은 폴더부터 넣는다.
        let mut all: BTreeSet<(usize, String)> = BTreeSet::new();
        for dir in dirs {
            let mut cur = dir.trim_matches('/');
            loop {
                all.insert((depth(cur), cur.to_owned()));
                match cur.rfind('/') {
                    Some(i) => cur = &cur[..i],
                    None if !cur.is_empty() => cur = "",
                    None => break,
                }
            }
        }
        for (_, dir) in &all {
            let file = if dir.is_empty() {
                top.join(".gitattributes")
            } else {
                top.join(dir).join(".gitattributes")
            };
            // 매크로(`[attr]`)는 꼭대기 파일에서만 정의할 수 있다(git 과 같다).
            add_file(
                &mut search,
                &mut collection,
                file,
                Some(top),
                dir.is_empty(),
            );
        }
        if let Some(gitdir) = gitdir {
            let file = gitdir.join("info").join("attributes");
            add_file(&mut search, &mut collection, file, None, true);
        }
        Self { search, collection }
    }

    /// `rel` 은 `top` 기준 상대 경로(`/` 구분).
    pub fn get(&self, rel: &str) -> Attrs {
        let mut out = Outcome::default();
        out.initialize_with_selection(&self.collection, NAMES);
        self.search.pattern_matching_relative_path(
            rel.as_bytes().as_bstr(),
            Case::Sensitive,
            Some(false),
            &mut out,
        );
        let mut attrs = Attrs::default();
        for m in out.iter_selected() {
            let state = m.assignment.state;
            let name = m.assignment.name.as_str();
            if name == NAMES[0] {
                if let StateRef::Value(v) = state {
                    attrs.language = Some(v.as_bstr().to_str_lossy().into_owned());
                }
                continue;
            }
            // Linguist `boolean_attribute`: 적혔으면 `false`(값 · `-속성`)만 거짓이다.
            let value = match state {
                StateRef::Unspecified => None,
                StateRef::Set => Some(true),
                StateRef::Unset => Some(false),
                StateRef::Value(v) => Some(v.as_bstr() != "false"),
            };
            match name {
                "linguist-vendored" => attrs.vendored = value,
                "linguist-generated" => attrs.generated = value,
                "linguist-documentation" => attrs.documentation = value,
                _ => {}
            }
        }
        attrs
    }
}

/// 속성 파일 하나를 [`read_small`] 로 읽어 넣는다. 없으면 조용히, 못 읽으면(심링크 · 일반 파일 아님 · 너무 큼) 경고하고 건너뛴다.
fn add_file(
    search: &mut Search,
    collection: &mut MetadataCollection,
    file: PathBuf,
    root: Option<&Path>,
    macros: bool,
) {
    let bytes = match read_small(&file, MAX_ATTRIBUTES_BYTES) {
        Ok(Some(bytes)) => bytes,
        Ok(None) => return,
        Err(e) => {
            tracing::warn!(target: "pw_lang", file = %file.display(), %e, "ignoring an attributes file");
            return;
        }
    };
    if let Err(e) = search.add_patterns_buffer(&bytes, file.clone(), root, collection, macros) {
        tracing::warn!(target: "pw_lang", file = %file.display(), %e, "cannot parse an attributes file");
    }
}

/// 일반 파일 `path` 를 `max` 바이트까지 읽는다(모듈 머리 주석의 규칙). 없으면 `Ok(None)`. 심링크 · 일반 파일이 아닌 것 ·
/// `max` 를 넘는 파일은 오류다.
pub fn read_small(path: &Path, max: u64) -> io::Result<Option<Vec<u8>>> {
    let file = match open_no_follow(path) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    // 연 핸들을 본다 — 경로로 다시 보면 그사이 바뀐 것을 본다.
    let meta = file.metadata()?;
    if !meta.is_file() {
        return Err(io::Error::other("not a regular file"));
    }
    if meta.len() > max {
        return Err(io::Error::other(format!("larger than {max} bytes")));
    }
    let mut bytes = Vec::new();
    file.take(max + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > max {
        return Err(io::Error::other(format!("larger than {max} bytes")));
    }
    Ok(Some(bytes))
}

/// 마지막 구성 요소가 심링크면 실패하게, FIFO 에서 막히지 않게 연다.
#[cfg(unix)]
fn open_no_follow(path: &Path) -> io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
}

/// Windows 에는 열 때 막히는 FIFO 가 없다. 심링크는 열기 전에 본다.
#[cfg(not(unix))]
fn open_no_follow(path: &Path) -> io::Result<File> {
    if std::fs::symlink_metadata(path)?.file_type().is_symlink() {
        return Err(io::Error::other("symbolic link"));
    }
    File::open(path)
}

fn depth(dir: &str) -> usize {
    if dir.is_empty() {
        0
    } else {
        dir.matches('/').count() + 1
    }
}

/// 작업 트리 꼭대기와 `info/attributes` 를 찾을 git 디렉터리.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repo {
    pub top: PathBuf,
    pub gitdir: Option<PathBuf>,
}

/// `start` 와 그 조상에서 가장 가까운 `.git` 이 있는 저장소. `.git` 이 파일이면(연결 워크트리 · 서브모듈) `gitdir:` 이
/// 가리키는 곳, 그 안에 `commondir` 이 있으면 그것(메인 저장소)이 git 디렉터리다. git 을 실행하지 않는다.
pub fn find_repo(start: &Path) -> Option<Repo> {
    for dir in start.ancestors() {
        let dot_git = dir.join(".git");
        // 링크를 따라가지 않고 본다. `.git` 이 FIFO 등이면 읽지 않는다.
        let Ok(meta) = std::fs::symlink_metadata(&dot_git) else {
            continue;
        };
        if meta.is_dir() {
            return Some(Repo {
                top: dir.to_path_buf(),
                gitdir: Some(dot_git),
            });
        }
        let pointer = |file: &Path, prefix: &str| -> Option<String> {
            let bytes = read_small(file, MAX_POINTER_BYTES).ok()??;
            let text = String::from_utf8(bytes).ok()?;
            let line = text.lines().next()?;
            Some(line.strip_prefix(prefix)?.trim().to_owned())
        };
        let gitdir = meta
            .is_file()
            .then(|| pointer(&dot_git, "gitdir:"))
            .flatten()
            .map(|p| dir.join(p));
        let common = gitdir
            .as_ref()
            .and_then(|g| Some(g.join(pointer(&g.join("commondir"), "")?)));
        return Some(Repo {
            top: dir.to_path_buf(),
            gitdir: common.or(gitdir),
        });
    }
    None
}
