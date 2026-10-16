//! 루트 후보 추천(F2-4 의 추천 부분). 등록 프로젝트가 경로를 포함하지 않을 때 「프로젝트 찾기」 가 보여 줄 폴더들이다.
//!
//! 순서:
//! 1. 경로가 연결 워크트리 안이면 **메인 체크아웃 꼭대기** — F2-5 의 목적은 워크트리가 아니라 메인 저장소를 등록하는 것이다.
//! 2. **가장 가까운 VCS 루트**(`.git` 이 디렉터리든 파일이든 있는 가장 가까운 조상).
//! 3. **빌드 파일이 있는 조상**을 가까운 것부터. VCS 루트가 있으면 그 루트까지만 본다 — 저장소 밖의 빌드 파일은 대개 다른 것이다.
//!
//! 같은 폴더는 한 번만 나오고, 앞 순번 자리에 이유를 모아 붙인다.
//!
//! **연결 워크트리 안에서 나온 후보(2 · 3)는 메인 체크아웃 쪽 같은 위치로 옮긴다** — 워크트리 경로가 프로젝트로 등록되면
//! F2-5(워크트리는 메인 저장소 프로젝트의 활성 워크트리로 연다)가 어긋난다. 워크트리 꼭대기는 1 과 합쳐지고, 메인 쪽에 없는 폴더
//! (워크트리에만 있는 새 폴더)는 빠진다. 빌드 파일 이름은 워크트리에서 본 것이다. 워크트리 자체를 등록하려면 「폴더 직접 선택」.
//!
//! **너무 넓은 폴더는 후보가 아니다**: 홈 디렉터리, 홈의 조상(`/Users` 등), 파일 시스템 루트. 홈에 `.git` 이 있는(dotfiles 저장소)
//! 경우도 마찬가지다 — 홈을 프로젝트로 열면 파일 감시 · 검색이 홈 전체를 훑는다. 그런 폴더가 필요하면 「폴더 직접 선택」 으로 고른다.
//! 위로 올라가기는 홈에서 멈춘다(홈 밖의 경로면 루트까지 간다).

use std::fs;
use std::path::{Path, PathBuf};

use super::path::worktree_link;

/// 루트 후보로 보는 빌드 파일(F2 흐름). 이름이 정확히 같은 파일만 본다.
pub const BUILD_FILES: &[&str] = &[
    "package.json",
    "Cargo.toml",
    "build.gradle",
    "build.gradle.kts",
    "settings.gradle",
    "settings.gradle.kts",
    "pom.xml",
    "go.mod",
    "pyproject.toml",
    "setup.py",
    "Gemfile",
    "composer.json",
    "mix.exs",
    "build.sbt",
    "deno.json",
    "CMakeLists.txt",
];

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Candidate {
    pub path: PathBuf,
    /// 연결 워크트리의 메인 체크아웃 꼭대기.
    pub worktree_main: bool,
    /// 가장 가까운 VCS 루트.
    pub vcs: bool,
    /// 이 폴더에 있는 [`BUILD_FILES`], 그 목록의 순서대로.
    pub build_files: Vec<String>,
}

/// `path` 는 정규화된 경로여야 한다. 파일이면 그 폴더부터 본다. `home` 은 정규화된 홈 디렉터리.
pub fn candidates(path: &Path, home: Option<&Path>) -> Vec<Candidate> {
    let start = if path.is_file() {
        path.parent().unwrap_or(path)
    } else {
        path
    };
    let too_broad = |dir: &Path| dir.parent().is_none() || home.is_some_and(|h| h.starts_with(dir));

    let link = worktree_link(start);
    let mut out: Vec<Candidate> = Vec::new();
    if let Some(link) = &link
        && !too_broad(&link.main_top)
    {
        out.push(Candidate {
            path: link.main_top.clone(),
            worktree_main: true,
            ..Candidate::default()
        });
    }

    let mut builds: Vec<Candidate> = Vec::new();
    let mut vcs_root: Option<PathBuf> = None;
    for dir in start.ancestors() {
        if too_broad(dir) {
            break;
        }
        let build_files: Vec<String> = BUILD_FILES
            .iter()
            .filter(|name| dir.join(name).is_file())
            .map(|name| (*name).to_owned())
            .collect();
        if !build_files.is_empty() {
            builds.push(Candidate {
                path: dir.to_path_buf(),
                build_files,
                ..Candidate::default()
            });
        }
        if fs::symlink_metadata(dir.join(".git")).is_ok() {
            vcs_root = Some(dir.to_path_buf());
            break;
        }
    }

    let found = vcs_root
        .map(|root| Candidate {
            path: root,
            vcs: true,
            ..Candidate::default()
        })
        .into_iter()
        .chain(builds);
    for candidate in found {
        let candidate = match &link {
            None => candidate,
            Some(link) => {
                let Some(main) = link.map_to_main(&candidate.path) else {
                    continue;
                };
                if too_broad(&main) || !main.is_dir() {
                    continue;
                }
                Candidate {
                    path: main,
                    ..candidate
                }
            }
        };
        merge(&mut out, candidate);
    }
    out
}

fn merge(out: &mut Vec<Candidate>, next: Candidate) {
    match out.iter_mut().find(|c| c.path == next.path) {
        Some(existing) => {
            existing.worktree_main |= next.worktree_main;
            existing.vcs |= next.vcs;
            existing.build_files.extend(next.build_files);
        }
        None => out.push(next),
    }
}
