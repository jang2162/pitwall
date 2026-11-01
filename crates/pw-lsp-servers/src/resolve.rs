//! 실행 파일 해석(결정 M4-D2 · D7). 순서:
//!
//! 1. 사용자 설정 경로(`[lsp.<id>] path`). 적었는데 쓸 수 없으면 다음 단계로 가지 않고 [`LspResolution::InvalidPath`] 다 — 사람이
//!    고른 경로가 틀린 것을 다른 실행 파일로 말없이 가리지 않는다.
//! 2. 프로젝트 로컬(정의의 `project_local`, 신뢰한 프로젝트만): 해석하는 디렉터리에서 프로젝트 루트까지 올라가며 처음 있는 것.
//! 3. 그 디렉터리의 런타임 환경 PATH. 정의가 `resolve_hint = "rustup"` 이고 찾은 것이 rustup 프록시면 `rustup which` 가 성공할
//!    때만 쓴다(신뢰하지 않은 프로젝트에서는 `rustup which` 를 돌리지 않고 건너뛴다 — 툴체인 파일을 읽는 것도 프로젝트 코드다).
//! 4. Pitwall 설치본(`$DATA/tools`) — [`ToolLocator`](M4-04 가 구현한다).
//! 5. 없으면 [`LspResolution::NotInstalled`] — 설치를 제안한다(M4-17).
//!
//! 판(`--version`)은 부르지 않는다: 설치본은 설치 기록의 판, 그 밖은 「모름」 이다.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use pw_ext::{LanguageServerDecl, ResolveHint};
use pw_runtime::RuntimeEnv;
use pw_runtime::program::find_program;
use serde::{Deserialize, Serialize};

use crate::rustup;
use crate::settings::{ConfiguredPath, LspSettingsOrigin, ServerConfig};

/// Pitwall 설치본(`$DATA/tools`)에서 실행 파일을 찾는다. 설치기(M4-04 `pw-tools`)가 구현한다.
pub trait ToolLocator: Send + Sync {
    /// Mason 패키지 `package` 의 실행 파일 `bin`. 설치 중인 판은 고르지 않는다.
    fn locate(&self, package: &str, bin: &str) -> Located;
}

/// [`ToolLocator::locate`] 의 답.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Located {
    /// 쓸 수 있는 설치본. `version` 은 설치 기록의 판이다.
    Installed {
        path: PathBuf,
        version: String,
    },
    /// 설치본은 있지만 쓸 수 없다(venv 를 만든 python 이 사라짐 등) — 다시 설치해야 한다.
    Broken {
        reason: String,
    },
    Absent,
}

/// 설치본이 없는 locator. 설치기(M4-04)가 붙기 전의 기본값이다.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoTools;

impl ToolLocator for NoTools {
    fn locate(&self, _package: &str, _bin: &str) -> Located {
        Located::Absent
    }
}

/// 해석의 단계.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum LspResolveStep {
    Settings,
    ProjectLocal,
    Path,
    Installed,
}

/// 보았지만 고르지 않은 후보와 그 까닭(상태 팝업 · 설치 안내가 보인다).
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct LspSkipped {
    pub step: LspResolveStep,
    #[specta(type = String)]
    pub path: PathBuf,
    pub reason: String,
}

/// 해석 결과.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum LspResolution {
    Found {
        /// 절대 경로.
        #[specta(type = String)]
        path: PathBuf,
        step: LspResolveStep,
        /// 설치본만 안다(설치 기록). 그 밖은 `null`(모름).
        version: Option<String>,
        skipped: Vec<LspSkipped>,
    },
    /// `[lsp.<id>] enabled = false`.
    Disabled,
    /// 설정 경로를 쓸 수 없다. 다음 단계로 가지 않는다.
    InvalidPath {
        path: String,
        origin: LspSettingsOrigin,
        reason: String,
    },
    /// 어디에도 없다 — 설치를 제안한다. `reinstall` 은 설치본이 깨졌을 때 그 까닭이다.
    NotInstalled {
        mason: Option<String>,
        reinstall: Option<String>,
        skipped: Vec<LspSkipped>,
    },
}

impl LspResolution {
    /// 찾은 실행 파일.
    pub fn path(&self) -> Option<&Path> {
        match self {
            LspResolution::Found { path, .. } => Some(path),
            _ => None,
        }
    }
}

/// 해석에 쓰는 프로젝트 쪽 값.
pub struct ResolveContext<'a> {
    /// 프로젝트를 신뢰하는가. 아니면 프로젝트 로컬과 rustup 확인을 건너뛴다.
    pub trusted: bool,
    /// 프로젝트 루트(활성 워크트리면 그 안의 루트).
    pub project_root: &'a Path,
    /// 해석하는 디렉터리(서버의 작업 공간 루트). `project_root` 안이다.
    pub dir: &'a Path,
    /// `dir` 의 런타임 환경.
    pub env: &'a RuntimeEnv,
    pub tools: &'a dyn ToolLocator,
}

/// 서버 하나의 실행 파일을 찾는다(머리 주석의 순서).
pub fn resolve(
    def: &LanguageServerDecl,
    config: &ServerConfig,
    ctx: &ResolveContext<'_>,
) -> LspResolution {
    if !config.enabled {
        return LspResolution::Disabled;
    }
    if let Some(configured) = &config.path {
        return configured_path(configured, ctx);
    }
    let mut skipped = Vec::new();
    if ctx.trusted
        && let Some(local) = &def.project_local
        && let Some(path) = project_local(ctx.project_root, ctx.dir, local.as_str(), ctx.env)
    {
        return found(path, LspResolveStep::ProjectLocal, None, skipped);
    }
    if let Some(path) = find_program(&def.command, &ctx.env.path) {
        match path_candidate(def, &path, ctx) {
            Ok(path) => return found(path, LspResolveStep::Path, None, skipped),
            Err(reason) => skipped.push(LspSkipped {
                step: LspResolveStep::Path,
                path,
                reason,
            }),
        }
    }
    let mut reinstall = None;
    if let Some(package) = &def.mason {
        match ctx.tools.locate(package, &def.command) {
            Located::Installed { path, version } => {
                return found(path, LspResolveStep::Installed, Some(version), skipped);
            }
            Located::Broken { reason } => reinstall = Some(reason),
            Located::Absent => {}
        }
    }
    LspResolution::NotInstalled {
        mason: def.mason.clone(),
        reinstall,
        skipped,
    }
}

fn found(
    path: PathBuf,
    step: LspResolveStep,
    version: Option<String>,
    skipped: Vec<LspSkipped>,
) -> LspResolution {
    LspResolution::Found {
        path,
        step,
        version,
        skipped,
    }
}

/// 설정 경로: 절대 경로, PATH 에서 찾을 이름, 또는(프로젝트 설정만) 프로젝트 루트 기준 상대 경로.
fn configured_path(configured: &ConfiguredPath, ctx: &ResolveContext<'_>) -> LspResolution {
    let raw = configured.value.as_str();
    let as_path = Path::new(raw);
    let has_separator = raw.contains('/') || (cfg!(windows) && raw.contains('\\'));
    let invalid = |reason: &str| LspResolution::InvalidPath {
        path: raw.to_owned(),
        origin: configured.origin,
        reason: reason.to_owned(),
    };
    let found_at = if as_path.is_absolute() {
        find_at(as_path, ctx.env)
    } else if !has_separator {
        find_program(raw, &ctx.env.path)
    } else if configured.origin == LspSettingsOrigin::Project {
        find_at(&ctx.project_root.join(as_path), ctx.env)
    } else {
        return invalid(
            "a relative path in the global settings has nothing to be relative to; use an absolute path or a name on PATH",
        );
    };
    match found_at {
        Some(path) => found(path, LspResolveStep::Settings, None, Vec::new()),
        None if as_path.is_absolute() || has_separator => invalid("not an executable file"),
        None => invalid("not found on PATH"),
    }
}

/// 해석하는 디렉터리부터 프로젝트 루트까지 올라가며 `rel` 이 가리키는 실행 파일. 루트 밖으로는 올라가지 않는다.
fn project_local(root: &Path, dir: &Path, rel: &str, env: &RuntimeEnv) -> Option<PathBuf> {
    dirs_up_to_root(root, dir)
        .into_iter()
        .flat_map(|d| executable_names(&d.join(rel), env))
        .find_map(|p| find_program(&p.to_string_lossy(), OsStr::new("")))
}

/// `dir` 부터 `root` 까지의 폴더(가까운 것부터). 둘 다 실제 경로(심볼릭 링크 · `..` 를 푼 것)로 견주므로 `<root>/..` 나 루트 밖을
/// 가리키는 링크는 루트 안이 아니다 — 그러면 `root` 하나다. `root` 의 실제 경로를 얻지 못하면 비어 있다.
pub(crate) fn dirs_up_to_root(root: &Path, dir: &Path) -> Vec<PathBuf> {
    let Ok(root) = std::fs::canonicalize(root) else {
        return Vec::new();
    };
    let start = std::fs::canonicalize(dir)
        .ok()
        .filter(|d| d.starts_with(&root))
        .unwrap_or_else(|| root.clone());
    start
        .ancestors()
        .take_while(|d| d.starts_with(&root))
        .map(Path::to_path_buf)
        .collect()
}

/// 경로로 적은 실행 파일(프로젝트 로컬 · 설정의 경로)로 볼 파일. Windows 는 PATHEXT 확장자를 붙인 이름만 본다 — npm 이
/// `node_modules/.bin` 에 함께 까는 확장자 없는 sh 스크립트는 `CreateProcess` 로 띄울 수 없다(`.cmd` 를 고른다).
fn executable_names(path: &Path, env: &RuntimeEnv) -> Vec<PathBuf> {
    let pathext = env.get("PATHEXT").and_then(OsStr::to_str);
    candidate_names(path, cfg!(windows), pathext)
}

/// [`executable_names`] 의 플랫폼 무관한 꼴(시험이 두 갈래를 모두 본다).
pub(crate) fn candidate_names(path: &Path, windows: bool, pathext: Option<&str>) -> Vec<PathBuf> {
    if !windows {
        return vec![path.to_path_buf()];
    }
    let Some(name) = path.file_name().and_then(OsStr::to_str) else {
        return Vec::new();
    };
    let pathext = pathext.filter(|p| !p.is_empty()).unwrap_or(DEFAULT_PATHEXT);
    pw_runtime::program::windows_names(name, pathext)
        .into_iter()
        .map(|n| path.with_file_name(n))
        .collect()
}

const DEFAULT_PATHEXT: &str = ".COM;.EXE;.BAT;.CMD";

/// 경로로 적은 실행 파일([`executable_names`] 의 후보 중 처음 있는 것).
fn find_at(path: &Path, env: &RuntimeEnv) -> Option<PathBuf> {
    executable_names(path, env)
        .into_iter()
        .find_map(|p| find_program(&p.to_string_lossy(), OsStr::new("")))
}

/// PATH 에서 찾은 것을 쓸 수 있는지. rustup 프록시는 `rustup which` 가 성공할 때만이다.
fn path_candidate(
    def: &LanguageServerDecl,
    path: &Path,
    ctx: &ResolveContext<'_>,
) -> Result<PathBuf, String> {
    if def.resolve_hint != Some(ResolveHint::Rustup) {
        return Ok(path.to_path_buf());
    }
    let Some(rustup) = rustup::proxy_of(path) else {
        return Ok(path.to_path_buf());
    };
    if !ctx.trusted {
        return Err("a rustup proxy; not checked in an untrusted project".to_owned());
    }
    match rustup::version(&rustup, ctx.env, rustup::WHICH_TIMEOUT) {
        Ok(v) if v >= rustup::MIN_VERSION => {}
        Ok((major, minor, patch)) => {
            let (a, b, c) = rustup::MIN_VERSION;
            return Err(format!(
                "a rustup proxy; rustup {major}.{minor}.{patch} is older than {a}.{b}.{c} and may install a toolchain while checking, so it is not checked"
            ));
        }
        Err(reason) => return Err(format!("a rustup proxy; {reason}")),
    }
    match rustup::which(
        &rustup,
        &def.command,
        ctx.dir,
        ctx.env,
        rustup::WHICH_TIMEOUT,
    ) {
        // 프록시를 그대로 쓴다 — 서버를 그 디렉터리에서 띄우면 프록시가 같은 툴체인을 고른다.
        Ok(_) => Ok(path.to_path_buf()),
        Err(reason) => Err(format!("a rustup proxy without the component: {reason}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Windows 는 경로로 적은 실행 파일에 PATHEXT 확장자를 붙인 이름만 본다(확장자 없는 sh 스크립트는 띄울 수 없다). 그 밖은 그대로다.
    #[test]
    fn path_candidates_add_pathext_only_on_windows() {
        let bin = Path::new("/p/node_modules/.bin/typescript-language-server");
        assert_eq!(candidate_names(bin, false, Some(".EXE;.CMD")), [bin]);
        let names: Vec<String> = candidate_names(bin, true, Some(".EXE;.CMD"))
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            names,
            [
                "typescript-language-server.EXE",
                "typescript-language-server.CMD"
            ]
        );
        // PATHEXT 가 없으면 cmd.exe 의 기본값.
        assert_eq!(candidate_names(bin, true, None).len(), 4);
        // 이미 확장자가 있으면 그 이름 하나.
        let exe = Path::new("C:/tools/ra.exe");
        assert_eq!(candidate_names(exe, true, Some(".EXE")), [exe]);
    }
}
