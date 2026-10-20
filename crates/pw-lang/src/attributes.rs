//! `.gitattributes` 의 linguist 속성(`linguist-language` · `linguist-vendored` · `linguist-generated` ·
//! `linguist-documentation`)을 `gix-attributes` 로 읽는다(결정 M1-D20).
//!
//! 읽는 파일은 GitHub 가 보는 것과 같다: 저장소 꼭대기부터 그 파일의 폴더까지의 `.gitattributes`, 그리고 `$GIT_DIR/info/attributes`
//! (연결 워크트리는 메인 저장소의 것). 사람의 전역 파일(`core.attributesFile`, `~/.config/git/attributes`)은 읽지 않는다 —
//! GitHub 서버에는 그것이 없으므로 넣으면 결과가 GitHub 와 갈린다. 패턴은 대소문자를 가린다(GitHub 와 같다).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use bstr::ByteSlice;
use gix_attributes::glob::pattern::Case;
use gix_attributes::search::{MetadataCollection, Outcome};
use gix_attributes::{Search, StateRef};

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
        let mut buf = Vec::new();
        let no_globals: [PathBuf; 0] = [];
        let mut search = Search::new_globals(no_globals, &mut buf, &mut collection)
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
            if let Err(e) = search.add_patterns_file(
                file.clone(),
                true,
                Some(top),
                &mut buf,
                &mut collection,
                dir.is_empty(),
            ) {
                tracing::warn!(target: "pw_lang", file = %file.display(), %e, "cannot read .gitattributes");
            }
        }
        if let Some(gitdir) = gitdir {
            let file = gitdir.join("info").join("attributes");
            if let Err(e) =
                search.add_patterns_file(file.clone(), true, None, &mut buf, &mut collection, true)
            {
                tracing::warn!(target: "pw_lang", file = %file.display(), %e, "cannot read info/attributes");
            }
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
        let Ok(meta) = std::fs::metadata(&dot_git) else {
            continue;
        };
        if meta.is_dir() {
            return Some(Repo {
                top: dir.to_path_buf(),
                gitdir: Some(dot_git),
            });
        }
        let gitdir = std::fs::read_to_string(&dot_git)
            .ok()
            .and_then(|t| Some(t.lines().next()?.strip_prefix("gitdir:")?.trim().to_owned()))
            .map(|p| dir.join(p));
        let common = gitdir.as_ref().and_then(|g| {
            let c = std::fs::read_to_string(g.join("commondir")).ok()?;
            Some(g.join(c.trim()))
        });
        return Some(Repo {
            top: dir.to_path_buf(),
            gitdir: common.or(gitdir),
        });
    }
    None
}
