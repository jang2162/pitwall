//! 프로젝트 요약(`project.info`, MCP `project_info` — `docs/design/05-mcp.md` §2 「프로젝트」).
//!
//! 등록된 루트 바로 아래만 본다: 빌드 파일 이름과, 설정 폴더에 있는 설정 파일 이름. **파일 내용은 읽지 않는다** — 설정을
//! 읽으면 구버전 파일의 마이그레이션(디스크 쓰기)이 따라오고, 조회 툴이 사용자 저장소를 고치게 된다. 언어는 빌드 파일 이름에서
//! 짐작한 것이다(자동 감지 F1 이 들어오면 그 결과로 바꾼다).

use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::candidates::BUILD_FILES;
use super::{ConfigMode, Project};
use crate::config::AppPaths;

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct ProjectInfo {
    pub project: Project,
    /// 주 루트 폴더가 디스크에 있는가. 없으면(옮기거나 지운 폴더) 아래 목록은 비어 있다.
    pub root_exists: bool,
    /// 주 루트에 있는 빌드 파일 이름(루트 후보와 같은 목록 · 순서).
    pub build_files: Vec<String>,
    /// 빌드 파일에서 짐작한 언어(`rust`, `typescript` …). 먼저 나온 빌드 파일 순.
    pub languages: Vec<String>,
    pub config: ConfigSummary,
}

/// 설정을 어디서 읽는가와 거기 있는 파일. 내용은 담지 않는다.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct ConfigSummary {
    pub mode: ConfigMode,
    /// 저장소 모드면 `<root>/.pitwall`, 개인 전용이면 `$CONFIG/projects/<slug>`.
    pub dir: String,
    /// 그 폴더의 설정 파일 id(`project`, `run/dev-server` …). `.local.toml` 은 같은 id 로 합친다. 이름순.
    pub files: Vec<String>,
}

/// 빌드 파일 → 언어. 표에 없는 빌드 파일은 언어를 더하지 않는다.
const LANGUAGES: &[(&str, &str)] = &[
    ("Cargo.toml", "rust"),
    ("package.json", "javascript"),
    ("deno.json", "typescript"),
    ("build.gradle", "java"),
    ("settings.gradle", "java"),
    ("pom.xml", "java"),
    ("build.gradle.kts", "kotlin"),
    ("settings.gradle.kts", "kotlin"),
    ("go.mod", "go"),
    ("pyproject.toml", "python"),
    ("setup.py", "python"),
    ("Gemfile", "ruby"),
    ("composer.json", "php"),
    ("mix.exs", "elixir"),
    ("build.sbt", "scala"),
    ("CMakeLists.txt", "cpp"),
];

/// 설정 폴더에서 읽는 하위 폴더. 설정 계층(`config::source`)의 범위와 같다.
const CONFIG_SUBDIRS: &[&str] = &["run", "db"];

pub fn info(project: Project, paths: &AppPaths) -> ProjectInfo {
    let root = project.root().to_path_buf();
    let root_exists = root.is_dir();
    let build_files: Vec<String> = if root_exists {
        BUILD_FILES
            .iter()
            .filter(|name| root.join(name).is_file())
            .map(|name| (*name).to_owned())
            .collect()
    } else {
        Vec::new()
    };
    let mut languages: Vec<String> = Vec::new();
    for file in &build_files {
        let language = LANGUAGES
            .iter()
            .find(|(name, _)| name == file)
            .map(|(_, language)| *language);
        // package.json 옆에 tsconfig.json 이 있으면 TypeScript 프로젝트로 본다.
        let language = match language {
            Some("javascript") if root.join("tsconfig.json").is_file() => Some("typescript"),
            other => other,
        };
        if let Some(language) = language
            && !languages.iter().any(|l| l == language)
        {
            languages.push(language.to_owned());
        }
    }
    // 개인 전용 slug 가 경로 한 칸이 아니면(레지스트리가 만들지 않는 값) 폴더를 정할 수 없다. 그때는 빈 요약이다.
    let dir = project
        .config_source(paths, &root)
        .map(|source| source.dir().to_path_buf())
        .ok();
    let config = ConfigSummary {
        mode: project.config_mode,
        dir: dir
            .as_ref()
            .map(|d| d.display().to_string())
            .unwrap_or_default(),
        files: dir.as_deref().map(config_files).unwrap_or_default(),
    };
    ProjectInfo {
        project,
        root_exists,
        build_files,
        languages,
        config,
    }
}

fn config_files(dir: &Path) -> Vec<String> {
    let mut ids = Vec::new();
    collect(dir, None, &mut ids);
    for sub in CONFIG_SUBDIRS {
        collect(&dir.join(sub), Some(sub), &mut ids);
    }
    ids.sort();
    ids.dedup();
    ids
}

fn collect(dir: &Path, prefix: Option<&str>, ids: &mut Vec<String>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some(stem) = name
            .strip_suffix(".local.toml")
            .or_else(|| name.strip_suffix(".toml"))
        else {
            continue;
        };
        if stem.is_empty() || stem.starts_with('.') {
            continue;
        }
        ids.push(match prefix {
            Some(prefix) => format!("{prefix}/{stem}"),
            None => stem.to_owned(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ConfigSource;

    fn project(root: &Path, mode: ConfigMode) -> Project {
        Project {
            id: "0123456789abcdef".into(),
            name: "p".into(),
            host: "local".into(),
            roots: vec![root.display().to_string()],
            favorite: false,
            tags: Vec::new(),
            config_mode: mode,
            slug: "p".into(),
            created_at: 0,
            opened_at: None,
        }
    }

    #[test]
    fn summarises_build_files_languages_and_config_file_ids() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("repo");
        fs::create_dir_all(root.join(".pitwall/run")).unwrap();
        for f in ["Cargo.toml", "package.json", "tsconfig.json", "go.mod"] {
            fs::write(root.join(f), "").unwrap();
        }
        for f in [
            ".pitwall/project.toml",
            ".pitwall/project.local.toml",
            ".pitwall/run/dev.toml",
            ".pitwall/notes.txt",
        ] {
            fs::write(root.join(f), "").unwrap();
        }
        let paths = AppPaths::under(&tmp.path().join("app"));
        let info = info(project(&root, ConfigMode::Repository), &paths);
        assert!(info.root_exists);
        assert_eq!(info.build_files, ["package.json", "Cargo.toml", "go.mod"]);
        assert_eq!(info.languages, ["typescript", "rust", "go"]);
        assert_eq!(info.config.files, ["project", "run/dev"]);
        assert_eq!(
            info.config.dir,
            ConfigSource::project(&root).dir().display().to_string()
        );
    }

    #[test]
    fn personal_mode_reads_the_personal_folder_and_a_missing_root_is_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::under(&tmp.path().join("app"));
        let personal = paths.personal_project_dir("p");
        fs::create_dir_all(&personal).unwrap();
        fs::write(personal.join("project.toml"), "").unwrap();
        let info = info(
            project(&tmp.path().join("gone"), ConfigMode::Personal),
            &paths,
        );
        assert!(!info.root_exists);
        assert!(info.build_files.is_empty());
        assert_eq!(info.config.dir, personal.display().to_string());
        assert_eq!(info.config.files, ["project"]);
    }
}
