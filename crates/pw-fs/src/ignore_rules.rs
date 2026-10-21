//! 폴더별 ignore 규칙(`.gitignore` · `.ignore` · 저장소의 `info/exclude` · 사람의 전역 gitignore)을 안전하게 읽어 판정한다.
//!
//! `ignore` 크레이트의 걷기도 같은 파일을 읽지만 그냥 열어(`File::open`) 줄 단위로 끝까지 읽는다. 클론한 저장소의
//! `.gitignore -> /dev/zero` 링크 하나로 메모리가 끝없이 늘고, FIFO 를 가리키면 영구히 막히고, 루트 밖을 가리키면 루트 밖
//! 파일을 읽는다. 여기서는 ignore 파일을 모두 [`read_small`](pw_lang::attributes::read_small)(심링크 · 일반 파일 아님 · 1 MiB
//! 초과는 없는 것으로 본다)로 읽는다. 걷기에서는 `ignore` 의 ignore 기능을 끄고([`skip_ignore_files`]) 이것을 `filter_entry` 에서
//! 부른다. 검색(`search.text`) · 목록(`fs.list`) · 감시(`fs.watch`)가 모두 이 판정을 쓴다.
//!
//! 규칙은 `ignore`(ripgrep)와 같다.
//! - 우선순위: `.ignore` > `.gitignore` > `info/exclude` > 전역 gitignore. 같은 종류 안에서는 깊은 폴더의 것이 먼저이고, `!패턴`
//!   은 되살린다.
//! - `.gitignore` · `info/exclude` · 전역 gitignore 는 저장소(`.git` 이 있는 폴더와 그 아래) 안에서만, 가장 가까운 저장소
//!   꼭대기까지만 적용한다.
//! - 읽는 범위(결정 M1-D8 의 예외 — 읽은 내용은 판정에만 쓴다): `.ignore` 는 루트 안 폴더의 것, `.gitignore` 는 저장소 안 폴더의
//!   것(루트 밖이어도 그 저장소 꼭대기까지), `info/exclude` 는 저장소 git 디렉터리의 것(`pw_lang::attributes::find_repo` 로 찾는다 —
//!   `.git` 이 링크면 따라가지 않는다), 전역 gitignore 는 `core.excludesFile`(기본 `~/.config/git/ignore`. 사람 자신의 설정이라
//!   링크는 따라가되 일반 파일 · 1 MiB 까지만, 찾는 일은 `ignore` 크레이트가 git 설정을 읽어 한다). 저장소를 찾으려고 위로
//!   오르며 `.git` 을 `lstat` 하지만 저장소 꼭대기 위의 파일은 읽지 않는다.
//!
//! rg 와 다른 점: ① 루트 밖 조상의 `.ignore` 는 읽지 않는다 ② `.jj` 만 있는 폴더는 저장소로 보지 않는다 ③ 심링크 · 일반 파일이
//! 아니거나 1 MiB 를 넘는 ignore 파일은 없는 것으로 본다 ④ 전역 gitignore 의 `/` 로 시작하는 패턴은 저장소 꼭대기 기준이다(git 과
//! 같다. rg 는 맞지 않는다).

use std::collections::HashMap;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use ignore::gitignore::{Gitignore, GitignoreBuilder};
use ignore::{Match, WalkBuilder};
use pw_lang::attributes::{MAX_ATTRIBUTES_BYTES, find_repo, read_small};

use crate::scope::Roots;

/// ignore 파일 하나의 상한. 넘으면 그 파일을 없는 것으로 본다(속성 파일과 같은 값).
const MAX_IGNORE_BYTES: u64 = MAX_ATTRIBUTES_BYTES;

/// 판정 한 묶음(검색 하나 등)이 쓰는 규칙. 폴더마다 한 번 읽고 기억한다. 스레드끼리 나눠 쓴다.
pub(crate) struct IgnoreRules {
    roots: Roots,
    /// 사람의 전역 gitignore 내용. 저장소 꼭대기마다 그 꼭대기 기준 규칙으로 만든다.
    global: Option<Vec<u8>>,
    dirs: Mutex<HashMap<PathBuf, Arc<Dir>>>,
}

/// 한 폴더의 규칙. `parent` 를 따라 `/` 까지 이어진다.
struct Dir {
    parent: Option<Arc<Dir>>,
    ignore: Option<Gitignore>,
    gitignore: Option<Gitignore>,
    /// 이 폴더가 저장소 꼭대기면 그 저장소의 `info/exclude` 와 전역 규칙(이 순서로 우선).
    top: Option<Vec<Gitignore>>,
    /// 이 폴더나 그 위에 `.git` 이 있다.
    in_repo: bool,
}

impl IgnoreRules {
    pub(crate) fn new(roots: Roots) -> Self {
        let global = ignore::gitignore::gitconfig_excludes_path().and_then(|path| read_own(&path));
        Self {
            roots,
            global,
            dirs: Mutex::default(),
        }
    }

    /// 루트 안의 `path` 가 무시되는가. `path` 는 걷기가 준 경로(루트 안 링크를 지났으면 링크 쪽 경로)다.
    pub(crate) fn ignored(&self, path: &Path, is_dir: bool) -> bool {
        let Some(dir) = path.parent() else {
            return false;
        };
        let dir = self.dir(dir);
        let chain = || std::iter::successors(Some(&*dir), |d| d.parent.as_deref());
        for d in chain() {
            if let Some(ignored) = decide(d.ignore.as_ref(), path, is_dir) {
                return ignored;
            }
        }
        if !dir.in_repo {
            return false;
        }
        for d in chain() {
            if let Some(ignored) = decide(d.gitignore.as_ref(), path, is_dir) {
                return ignored;
            }
            if let Some(top) = &d.top {
                return top
                    .iter()
                    .find_map(|rules| decide(Some(rules), path, is_dir))
                    .unwrap_or(false);
            }
        }
        false
    }

    fn dir(&self, dir: &Path) -> Arc<Dir> {
        if let Some(found) = self.lock().get(dir) {
            return Arc::clone(found);
        }
        // 잠금 밖에서 읽는다. 두 스레드가 같은 폴더를 함께 읽어도 결과가 같다.
        let parent = dir.parent().map(|p| self.dir(p));
        let has_git = std::fs::symlink_metadata(dir.join(".git")).is_ok();
        let in_repo = has_git || parent.as_ref().is_some_and(|p| p.in_repo);
        let made = Arc::new(Dir {
            ignore: if self.roots.contains(dir) {
                rules(dir, &dir.join(".ignore"))
            } else {
                None
            },
            gitignore: if in_repo {
                rules(dir, &dir.join(".gitignore"))
            } else {
                None
            },
            top: has_git.then(|| self.top(dir)),
            in_repo,
            parent,
        });
        Arc::clone(self.lock().entry(dir.to_path_buf()).or_insert(made))
    }

    /// 저장소 꼭대기 `top` 의 `info/exclude` 와 전역 규칙.
    fn top(&self, top: &Path) -> Vec<Gitignore> {
        let mut out = Vec::new();
        if let Some(gitdir) = find_repo(top).and_then(|repo| repo.gitdir) {
            out.extend(rules(top, &gitdir.join("info").join("exclude")));
        }
        if let Some(global) = &self.global {
            out.extend(parse(top, global, None));
        }
        out
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<PathBuf, Arc<Dir>>> {
        self.dirs.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// `walk` 가 ignore 파일을 읽지 않게 한다(숨김 파일 · 글롭 판정은 그대로). 판정은 `filter_entry` 에서 [`IgnoreRules::ignored`] 로
/// 한다.
pub(crate) fn skip_ignore_files(walk: &mut WalkBuilder) -> &mut WalkBuilder {
    walk.ignore(false)
        .git_ignore(false)
        .git_global(false)
        .git_exclude(false)
        .parents(false)
}

/// 사람 자신의 설정 파일(전역 gitignore)을 읽는다. dotfiles 도구가 흔히 링크로 두므로 링크는 따라가되, 일반 파일만 · 상한까지만
/// 읽고 FIFO 에서 막히지 않게 연다. 없거나 못 읽으면 `None`.
fn read_own(path: &Path) -> Option<Vec<u8>> {
    #[cfg(unix)]
    let file = {
        use std::os::unix::fs::OpenOptionsExt as _;
        std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(path)
            .ok()?
    };
    #[cfg(not(unix))]
    let file = std::fs::File::open(path).ok()?;
    let meta = file.metadata().ok()?;
    if !meta.is_file() || meta.len() > MAX_IGNORE_BYTES {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(MAX_IGNORE_BYTES).read_to_end(&mut bytes).ok()?;
    Some(bytes)
}

/// 규칙 하나의 판정. 무시면 `Some(true)`, 되살림(`!패턴`)이면 `Some(false)`, 맞는 패턴이 없으면 `None`.
fn decide(rules: Option<&Gitignore>, path: &Path, is_dir: bool) -> Option<bool> {
    match rules?.matched(path, is_dir) {
        Match::None => None,
        Match::Ignore(_) => Some(true),
        Match::Whitelist(_) => Some(false),
    }
}

/// `file`(`dir` 기준 gitignore 문법)을 안전하게 읽은 규칙. 없거나 못 읽거나 비었으면 `None`.
fn rules(dir: &Path, file: &Path) -> Option<Gitignore> {
    let bytes = read_small(file, MAX_IGNORE_BYTES).ok()??;
    parse(dir, &bytes, Some(file))
}

fn parse(dir: &Path, bytes: &[u8], file: Option<&Path>) -> Option<Gitignore> {
    let mut builder = GitignoreBuilder::new(dir);
    let text = String::from_utf8_lossy(bytes);
    // git 처럼 앞의 BOM 은 뗀다.
    for line in text.trim_start_matches('\u{feff}').lines() {
        let _ = builder.add_line(file.map(Path::to_path_buf), line);
    }
    builder.build().ok().filter(|rules| !rules.is_empty())
}
