//! 「같은 저장소」 판정에 쓰는 git 사실(18-m7-plan M7-10 ②): 체크아웃의 저장소(공용 git 폴더)와 역사의 바닥 커밋, 그리고 커밋을
//! 가졌는가. git CLI 가 아니라 gix 로 읽는다 — 신뢰 전의 프로젝트에서도 돈다(저장소의 설정 · 훅을 실행하지 않는다).
//!
//! **바닥 커밋**은 `HEAD` 에서 닿는 커밋 가운데 부모가 없는 것(루트 커밋 — `git rev-list --max-parents=0 HEAD`)과, 얕은 클론이면
//! 얕은 경계의 커밋(`.git/shallow`)이다. 두 체크아웃이 공통 커밋을 하나라도 가지면 한쪽의 바닥 커밋 가운데 하나를 다른 쪽이
//! 가진다 — 다른 쪽이 커밋 C 를 가지면 C 의 조상도 모두 갖고, C 에서 내려가면 이쪽에서 바닥 커밋 하나에 닿는다. 그래서
//! 판정은 「이쪽의 바닥을 저쪽이 가졌나」 를 양쪽으로 묻는 것이다([`super::add`]).

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};

/// 한 체크아웃의 역사 바닥.
#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct HistoryBottoms {
    /// 루트 커밋과 얕은 경계 커밋(hex, 정렬 · 중복 없음). `HEAD` 가 아직 커밋이 없으면 비어 있다.
    pub commits: Vec<String>,
    /// 얕은 클론인가. 그러면 바닥이 진짜 루트가 아닐 수 있다 — 공통 커밋이 깊이 밖에만 있으면 찾지 못한다.
    pub shallow: bool,
}

/// git 저장소가 아니거나 읽지 못한 까닭.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitReadError(pub String);

impl std::fmt::Display for GitReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

fn err(e: impl std::fmt::Display) -> GitReadError {
    GitReadError(e.to_string())
}

/// `path` 를 담은 작업 트리의 저장소. 저장소 밖 · bare 면 `None`. 저장소의 설정(`.git/config`)은 읽지 않는다 — 신뢰 전의
/// 체크아웃이라 그 설정이 고르는 프로그램 · 경로를 따르지 않는다(pw-git `repo::open_options` 의 미신뢰와 같다).
fn open(path: &Path) -> Result<Option<gix::Repository>, GitReadError> {
    use gix::discover::repository::Path as Found;
    use gix::discover::upwards::Error as NotFound;
    let found = match gix::discover::upwards(path) {
        Ok((found, _trust)) => found,
        Err(e) => {
            return match e.downcast_any_ref::<NotFound>() {
                Some(
                    NotFound::NoGitRepository { .. }
                    | NotFound::NoGitRepositoryWithinCeiling { .. }
                    | NotFound::NoGitRepositoryWithinFs { .. },
                ) => Ok(None),
                _ => Err(err(e)),
            };
        }
    };
    let work_dir = match found {
        Found::WorkTree(work_dir) | Found::LinkedWorkTree { work_dir, .. } => work_dir,
        Found::Repository(_) => return Ok(None),
    };
    let options = gix::open::Options::default()
        .filter_config_section(|meta| meta.source.kind() != gix::config::source::Kind::Repository);
    gix::open_opts(&work_dir, options).map(Some).map_err(err)
}

/// `path` 를 담은 저장소의 공용 git 폴더(워크트리끼리 같다 — 같은 클론인가의 열쇠). git 저장소가 아니면 `None`.
pub fn repository(path: &Path) -> Result<Option<String>, GitReadError> {
    Ok(open(path)?.map(|repo| {
        let dir = repo.common_dir();
        super::super::path::normalize(dir)
            .unwrap_or_else(|_| dir.to_path_buf())
            .display()
            .to_string()
    }))
}

/// 역사를 걷다 멈춘 까닭.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WalkError {
    Git(GitReadError),
    /// `stop` 이 섰다 — 판정이 취소되거나 시간 상한을 넘겼다(18-m7-plan M7-11, M7-10 리뷰 권고 2).
    Stopped,
}

impl From<GitReadError> for WalkError {
    fn from(e: GitReadError) -> Self {
        WalkError::Git(e)
    }
}

/// `path` 를 담은 저장소의 역사 바닥. git 저장소가 아니면 `None`. 역사 전체를 걸으므로 큰 저장소에서 오래 걸린다 — 커밋마다
/// `stop` 을 보고 서 있으면 [`WalkError::Stopped`] 로 멈춘다(블로킹 스레드의 걷기는 future 를 버려도 저절로 멈추지 않는다).
pub fn bottoms(path: &Path, stop: &AtomicBool) -> Result<Option<HistoryBottoms>, WalkError> {
    let Some(repo) = open(path)? else {
        return Ok(None);
    };
    let shallow = repo.shallow_commits().map_err(err)?;
    let mut commits: Vec<String> = shallow
        .as_ref()
        .map(|s| s.iter().map(|id| id.to_hex().to_string()).collect())
        .unwrap_or_default();
    // 아직 커밋이 없는 브랜치(unborn)면 걸을 것이 없다.
    if repo.head().map_err(err)?.is_unborn() {
        return Ok(Some(HistoryBottoms {
            commits,
            shallow: shallow.is_some(),
        }));
    }
    let head = repo.head_id().map_err(err)?.detach();
    for info in repo.rev_walk([head]).all().map_err(err)? {
        if stop.load(Ordering::Relaxed) {
            return Err(WalkError::Stopped);
        }
        let info = info.map_err(err)?;
        if info.parent_ids.is_empty() {
            commits.push(info.id.to_hex().to_string());
        }
    }
    commits.sort();
    commits.dedup();
    Ok(Some(HistoryBottoms {
        commits,
        shallow: shallow.is_some(),
    }))
}

/// `commits` 가운데 `path` 의 저장소가 가진 것. 꼴이 틀린 id 는 없는 것으로 본다.
pub fn contained(path: &Path, commits: &[String]) -> Result<Vec<String>, GitReadError> {
    let Some(repo) = open(path)? else {
        return Ok(Vec::new());
    };
    Ok(commits
        .iter()
        .filter(|hex| gix::ObjectId::from_hex(hex.as_bytes()).is_ok_and(|id| repo.has_object(id)))
        .cloned()
        .collect())
}
