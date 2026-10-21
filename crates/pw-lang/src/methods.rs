//! `lang.detect` · `lang.list` RPC. UI 전용이라 MCP 툴이 아니다.
//!
//! 웹뷰는 경로와(있으면) 앞부분만 넘긴다. 설정 매핑 · `.gitattributes` 는 이 Host 가 읽는다. 여러 파일을 한 번에 받는다 —
//! 파일 트리가 폴더를 펼칠 때 한 번에 부르고, 한 호출 안에서는 저장소마다 속성 파일을 한 번만 읽는다.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, OnceLock};

use pw_core::config::{AppPaths, ConfigStore};
use pw_core::registry::path::{WorktreeRootError, normalize, worktree_root};
use pw_core::registry::{Project, Registry as Projects, RegistryError};
use pw_rpc::{Router, RpcError, code, rpc_method};
use serde::{Deserialize, Serialize};

use crate::attributes::{Repo, RepoAttributes, find_repo};
use crate::{Attrs, Contributed, Input, LangSource, Mapping, detect};

/// 한 호출의 파일 수 상한. 넘으면 invalid params 다(나눠 부른다).
pub const MAX_FILES: usize = 10_000;

const SETTINGS_KEY: &str = "languages.mapping";

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct LangDetectParams {
    /// 있으면 그 프로젝트의 설정 매핑과 `.gitattributes` 를 본다. 없으면 전역 매핑과 Linguist 규칙만 본다(경로 규칙은 파일
    /// 이름에만 맞춘다).
    #[serde(default)]
    pub project_id: Option<String>,
    /// 활성 워크트리의 꼭대기(`ActiveWorktree.top`, 절대 경로). 프로젝트 루트가 그 워크트리 안의 것이 된다.
    #[serde(default)]
    pub worktree: Option<String>,
    /// 최대 [`MAX_FILES`] 개. 결과는 같은 순서다.
    pub files: Vec<LangDetectFile>,
}

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct LangDetectFile {
    /// 절대 경로. 파일이 없어도 된다(저장하지 않은 새 파일).
    pub path: String,
    /// 파일 앞부분(앞 50 KiB 까지만 본다 — 넘는 부분은 버린다). 없으면 빈 파일로 본다: 내용을 보는 단계(모드라인 · shebang ·
    /// XML 머리 · 생성 파일의 앞 줄 규칙)는 맞지 않고, heuristics 는 확장자마다의 기본 규칙(`.md` → Markdown 등)이 고른다.
    #[serde(default)]
    pub head: Option<String>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct LangDetection {
    /// Linguist 언어 이름. `null` 이면 플레인 텍스트.
    pub language: Option<String>,
    pub source: LangSource,
    /// 벤더 파일(vendor.yml 또는 `linguist-vendored`).
    pub vendored: bool,
    /// 생성 파일(`pw_lang::generated` 의 규칙 또는 `linguist-generated`).
    pub generated: bool,
    /// 문서 파일(documentation.yml 또는 `linguist-documentation`).
    pub documentation: bool,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct LangDetectResult {
    pub files: Vec<LangDetection>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct LangInfo {
    /// Linguist 언어 이름(확장만 기여한 언어는 그 `linguist` 이름). `lang.detect` 의 `language` 와 같은 값이다.
    pub name: String,
    /// Linguist popular.yml 에 있다(고르기 팝업이 앞에 둔다).
    pub popular: bool,
    /// 줄 주석 기호(확장의 `[contributes.languages.<id>] line_comment`). 기여한 확장이 없거나 적지 않았으면 없다.
    pub line_comment: Option<String>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct LangList {
    /// Linguist 표의 순서(이름순), 그 뒤에 Linguist 에 없는 확장 기여 언어가 등록 순서로.
    pub languages: Vec<LangInfo>,
}

rpc_method! {
    /// 파일들의 언어와 벤더 · 생성 · 문서 여부. 판별 순서는 `pw_lang` 크레이트 머리 주석. 상대 경로 · 모르는 프로젝트 ·
    /// 그 프로젝트의 워크트리가 아닌 `worktree` · [`MAX_FILES`] 를 넘는 파일 수는 invalid params 다.
    pub LangDetect: "lang.detect" {
        params: LangDetectParams,
        result: LangDetectResult,
        tier: Read,
        file_scope: true,
    }

    /// 고를 수 있는 언어(상태바의 언어 수동 변경, F4-12)와 언어마다의 줄 주석. 앱이 시작할 때 읽은 확장 레지스트리 기준이다.
    pub LangListMethod: "lang.list" {
        params: (),
        result: LangList,
        tier: Read,
        file_scope: true,
    }
}

/// Host 를 만든 뒤에 붙이는 것(Host 의 메서드는 Host 를 만드는 중에 붙으므로).
#[derive(Clone)]
pub struct HostParts {
    pub paths: AppPaths,
    /// 전역 설정(`$CONFIG/settings.toml` · `settings.local.toml`).
    pub global: ConfigStore,
    pub projects: Projects,
}

pub struct Lang {
    contributed: Contributed,
    host: OnceLock<HostParts>,
}

impl Lang {
    pub fn new(extensions: &pw_ext::Registry) -> Self {
        Self {
            contributed: Contributed::from_registry(extensions),
            host: OnceLock::new(),
        }
    }

    /// 두 번째부터는 무시한다. 붙이지 않았으면 설정 매핑 · 프로젝트 없이 판별한다.
    pub fn attach(&self, parts: HostParts) {
        let _ = self.host.set(parts);
    }

    pub fn list(&self) -> LangList {
        let info = |name: &str, popular: bool| LangInfo {
            name: name.to_owned(),
            popular,
            line_comment: self.contributed.line_comment(name).map(str::to_owned),
        };
        let mut languages: Vec<LangInfo> = crate::linguist::languages()
            .iter()
            .map(|l| info(l.name, l.popular))
            .collect();
        for name in self.contributed.names() {
            if !languages.iter().any(|l| l.name == name) {
                languages.push(info(name, false));
            }
        }
        LangList { languages }
    }

    pub fn detect(&self, params: &LangDetectParams) -> Result<LangDetectResult, RpcError> {
        if params.files.len() > MAX_FILES {
            return Err(RpcError::invalid_params(format!(
                "too many files: {} (at most {MAX_FILES} per call)",
                params.files.len()
            )));
        }
        let host = self.host.get();
        let project = match (&params.project_id, host) {
            (Some(id), Some(host)) => Some(project_and_root(host, id, params.worktree.as_deref())?),
            (Some(_), None) => {
                return Err(RpcError::internal("lang is not attached to a host config"));
            }
            (None, _) => None,
        };
        let mut warnings = Vec::new();
        let project_mapping = match (&project, host) {
            (Some((p, root)), Some(host)) => project_setting(host, p, root),
            _ => None,
        };
        let global = host.map(|h| h.global.snapshot());
        let mapping = Mapping::from_settings(
            project_mapping.as_ref(),
            global
                .as_ref()
                .and_then(|g| g.get("settings", SETTINGS_KEY)),
            &self.contributed,
            &mut warnings,
        );
        for w in warnings {
            tracing::warn!(target: "pw_lang", "settings: {w}");
        }

        let mut files = Vec::with_capacity(params.files.len());
        for f in &params.files {
            let path = Path::new(&f.path);
            if !path.is_absolute() {
                return Err(RpcError::invalid_params(format!(
                    "path `{}` must be absolute",
                    f.path
                )));
            }
            let path = normalize(path).map_err(|_| {
                RpcError::invalid_params(format!("path `{}` must be absolute", f.path))
            })?;
            files.push(path);
        }
        let root = project.as_ref().map(|(_, r)| r.as_path());
        Ok(LangDetectResult {
            files: detect_all(&files, &params.files, root, &mapping, &self.contributed),
        })
    }
}

/// 한 파일이 놓인 자리. 경로는 `/` 로 가른 상대 경로다(`crate::Input`).
struct Located {
    /// `root` 안이고 저장소 안이면 그 저장소.
    repo: Option<Repo>,
    repo_path: String,
    project_path: String,
}

/// 같은 순서로. `root` 안의 파일만 `.gitattributes` 를 본다.
fn detect_all(
    paths: &[PathBuf],
    files: &[LangDetectFile],
    root: Option<&Path>,
    mapping: &Mapping,
    contributed: &Contributed,
) -> Vec<LangDetection> {
    let mut repos: HashMap<PathBuf, Option<Repo>> = HashMap::new();
    let located: Vec<Located> = paths
        .iter()
        .map(|path| {
            let name = path
                .file_name()
                .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
            let Some(project_path) = root.and_then(|r| relative(path, r)) else {
                return Located {
                    repo: None,
                    repo_path: name.clone(),
                    project_path: name,
                };
            };
            let parent = path.parent().unwrap_or(path).to_path_buf();
            let repo = repos
                .entry(parent.clone())
                .or_insert_with(|| find_repo(&parent))
                .clone();
            match repo.and_then(|r| Some((relative(path, &r.top)?, r))) {
                Some((repo_path, repo)) => Located {
                    repo: Some(repo),
                    repo_path,
                    project_path,
                },
                None => Located {
                    repo: None,
                    repo_path: project_path.clone(),
                    project_path,
                },
            }
        })
        .collect();

    // 저장소마다 속성 파일을 한 번 읽는다.
    let mut by_repo: HashMap<PathBuf, (Option<PathBuf>, Vec<String>)> = HashMap::new();
    for l in &located {
        if let Some(repo) = &l.repo {
            let dir = l
                .repo_path
                .rsplit_once('/')
                .map_or("", |(d, _)| d)
                .to_owned();
            by_repo
                .entry(repo.top.clone())
                .or_insert_with(|| (repo.gitdir.clone(), Vec::new()))
                .1
                .push(dir);
        }
    }
    let loaded: HashMap<PathBuf, RepoAttributes> = by_repo
        .into_iter()
        .map(|(top, (gitdir, dirs))| {
            let attrs =
                RepoAttributes::load(&top, gitdir.as_deref(), dirs.iter().map(String::as_str));
            (top, attrs)
        })
        .collect();

    located
        .iter()
        .zip(files)
        .map(|(l, file)| {
            let attrs = l
                .repo
                .as_ref()
                .and_then(|r| loaded.get(&r.top))
                .map_or_else(Attrs::default, |a| a.get(&l.repo_path));
            let d = detect(
                Input {
                    repo_path: &l.repo_path,
                    project_path: &l.project_path,
                    head: file.head.as_deref(),
                },
                &attrs,
                mapping,
                contributed,
            );
            LangDetection {
                language: d.language,
                source: d.source,
                vendored: d.vendored,
                generated: d.generated,
                documentation: d.documentation,
            }
        })
        .collect()
}

/// `path` 가 `base` 안이면 `/` 로 가른 상대 경로. 비교는 구성 요소 단위다.
fn relative(path: &Path, base: &Path) -> Option<String> {
    let rel = path.strip_prefix(base).ok()?;
    let parts: Vec<String> = rel
        .components()
        .map(|c| match c {
            Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect::<Option<_>>()?;
    (!parts.is_empty()).then(|| parts.join("/"))
}

/// 프로젝트와, 활성 워크트리가 있으면 그 워크트리 안의 프로젝트 루트(`pw_core::registry::path::worktree_root`).
fn project_and_root(
    host: &HostParts,
    id: &str,
    worktree: Option<&str>,
) -> Result<(Project, PathBuf), RpcError> {
    let project = host.projects.get(id).map_err(registry_error)?;
    let Some(top) = worktree else {
        let root = project.root().to_path_buf();
        return Ok((project, root));
    };
    match worktree_root(project.root(), Path::new(top)) {
        Ok(root) => Ok((project, root)),
        Err(WorktreeRootError::Relative) => Err(RpcError::invalid_params(format!(
            "worktree `{top}` must be an absolute path"
        ))),
        Err(WorktreeRootError::NotOurs) => Err(RpcError::invalid_params(format!(
            "`{top}` is not a worktree of project `{id}`"
        ))),
    }
}

/// 프로젝트 설정의 `languages.mapping`. 디스크에 쓰지 않고 읽는다 — 판별만 했다고 커밋된 `.pitwall/project.toml` 이
/// 마이그레이션되면 안 된다. 읽지 못하면 경고하고 없는 것으로 본다.
fn project_setting(host: &HostParts, project: &Project, root: &Path) -> Option<serde_json::Value> {
    let source = match project.config_source(&host.paths, root) {
        Ok(source) => source.without_writes(),
        Err(e) => {
            tracing::warn!(target: "pw_lang", %e, project = %project.id, "cannot read the project config");
            return None;
        }
    };
    match source.load() {
        Ok(snapshot) => snapshot.get("project", SETTINGS_KEY).cloned(),
        Err(e) => {
            tracing::warn!(target: "pw_lang", %e, project = %project.id, "cannot read the project config; using the global mapping");
            None
        }
    }
}

fn registry_error(e: RegistryError) -> RpcError {
    match e {
        RegistryError::Db(_) => RpcError::internal(e.to_string()),
        RegistryError::Unavailable(_) => {
            RpcError::new(code::STORAGE_UNAVAILABLE, "storage unavailable")
                .with_data(serde_json::Value::String(e.to_string()))
        }
        _ => RpcError::invalid_params(e.to_string()),
    }
}

pub fn register(router: &mut Router, lang: Arc<Lang>) {
    let list = lang.list();
    router.unary(LangListMethod, move |_ctx, ()| {
        let list = list.clone();
        async move { Ok(list) }
    });
    router.unary(LangDetect, move |ctx, params: LangDetectParams| {
        let lang = Arc::clone(&lang);
        // 파일 범위 호출자(단일 파일 창)는 프로젝트의 설정 · 속성 파일을 읽게 하지 못한다 — 이름 · 앞부분만으로 판별한다.
        let scoped = ctx.file_scope().is_some() && params.project_id.is_some();
        async move {
            if scoped {
                return Err(RpcError::outside_file_scope("lang.detect"));
            }
            // 속성 파일 · 프로젝트 설정 읽기(디스크 IO)와 정규식 판별을 블로킹 스레드로 보낸다.
            tokio::task::spawn_blocking(move || lang.detect(&params))
                .await
                .map_err(|e| RpcError::internal(e.to_string()))?
        }
    });
}
