//! Mason 패키지 정의 하나를 읽고(15-m4-plan.md M4-03 ③), 한 대상에 맞춰 「무엇을 어디서 받아 무엇을 실행하나」 로 푼다.
//!
//! 스키마는 mason-org 의 `package.schema.json` 을 기준으로 한다. registry 는 받은 데이터라 미신뢰다:
//! - 모르는 키(최상위 · `source`)가 있으면 그 패키지를 「지원하지 않음(모르는 키)」 으로 두고 나머지 패키지는 읽는다
//!   (07-extensibility.md 「Mason registry 형식 변경」 리스크). 대상 항목(`asset` · `download`) 안의 키는 표현식이 읽는 값이라
//!   모르는 키를 따지지 않는다.
//! - 글 · 목록에 상한을 두고, 저장 이름 · 설치 폴더 안 경로는 폴더 밖으로 나가지 못하게 검사하고, 받을 주소는 HTTPS 만 받는다
//!   (결정 M4-D17 — 받기 자체의 검사는 설치기 M4-04 가 다시 한다).
//!
//! 지원 범위: purl 형식 `github` · `generic` · `npm` · `pypi`(§3 — 나머지는 「지원하지 않음」), `bin` 의 위임 접두
//! `npm:` · `pypi:` · `python:` 과 설치 폴더 안 경로(표현식 `{{source.asset.bin}}` 등을 푼 것), 표현식은 [`crate::expr`].

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::expr::{self, Context};
use crate::overrides::Override;
use crate::purl::Purl;
use crate::target::ToolTarget;

/// 글 하나의 상한(바이트). 설명이 가장 길고 1 KiB 남짓이다.
pub const MAX_TEXT_BYTES: usize = 8 * 1024;
/// 목록 · 표 하나의 상한(항목 수).
pub const MAX_LIST_ITEMS: usize = 256;

const TOP_KEYS: &[&str] = &[
    "name",
    "description",
    "homepage",
    "licenses",
    "languages",
    "categories",
    "source",
    "bin",
    "share",
    "opt",
    "deprecation",
    // 읽지만 쓰지 않는 것(설정 스키마 · Neovim 연결 · registry CI 표지).
    "schemas",
    "neovim",
    "ci_skip",
];

const SOURCE_KEYS: &[&str] = &[
    "id",
    "asset",
    "download",
    "extra_packages",
    // 옛 판에만 쓰인다 — Pitwall 은 registry 가 가리키는 지금 판만 설치한다.
    "version_overrides",
    "supported_platforms",
    "build",
    "bin",
];

/// 위임 접두 중 Mason 이 쓰는 것(지원하지 않는 것을 「경로」 로 잘못 읽지 않게 접두 꼴만 본다).
fn bin_prefix(value: &str) -> Option<(&str, &str)> {
    let (prefix, rest) = value.split_once(':')?;
    let ok = !prefix.is_empty()
        && prefix.starts_with(|c: char| c.is_ascii_lowercase())
        && prefix
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    ok.then_some((prefix, rest))
}

/// 「지원하지 않음」 의 까닭. `reason` 은 [`ToolUnsupported::reason`] 과 같다.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(tag = "reason", rename_all = "camelCase")]
pub enum ToolUnsupported {
    /// 스키마에 없는 키(최상위는 키 이름, `source` 아래는 `source.<키>`).
    #[serde(rename_all = "camelCase")]
    UnknownKey { key: String },
    /// 형식이 맞지 않는다(필수 키가 없다 · 형이 다르다 · 상한을 넘는다 · 경로가 폴더 밖이다).
    #[serde(rename_all = "camelCase")]
    InvalidDefinition { message: String },
    /// M4 가 설치하지 않는 purl 형식(`cargo` · `golang` · `openvsx` 등 — 15-m4-plan.md §3).
    #[serde(rename_all = "camelCase")]
    PurlType { purl_type: String },
    /// 지원하지 않는 `bin` 위임(`node:` · `java-jar:` · `exec:` 등).
    #[serde(rename_all = "camelCase")]
    ToolBin { bin: String, value: String },
    /// 모르는 표현식(필터 · 함수 · 오타). 대상별 항목에 있으면 그 대상만 빠진다.
    #[serde(rename_all = "camelCase")]
    Expression { expression: String },
    /// 이 대상의 항목이 없다(또는 `supported_platforms` 밖이다, Pitwall 이 지원하지 않는 기기다).
    Target,
    /// 소스에서 빌드한다(`source.build`).
    Build,
    /// 받을 주소가 `https://` 가 아니다.
    #[serde(rename_all = "camelCase")]
    InsecureUrl { url: String },
}

impl ToolUnsupported {
    /// 까닭별로 셀 때의 열쇠(직렬화의 `reason` 과 같다).
    pub fn reason(&self) -> &'static str {
        match self {
            ToolUnsupported::UnknownKey { .. } => "unknownKey",
            ToolUnsupported::InvalidDefinition { .. } => "invalidDefinition",
            ToolUnsupported::PurlType { .. } => "purlType",
            ToolUnsupported::ToolBin { .. } => "bin",
            ToolUnsupported::Expression { .. } => "expression",
            ToolUnsupported::Target => "target",
            ToolUnsupported::Build => "build",
            ToolUnsupported::InsecureUrl { .. } => "insecureUrl",
        }
    }
}

impl std::fmt::Display for ToolUnsupported {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ToolUnsupported::UnknownKey { key } => write!(f, "unknown key `{key}`"),
            ToolUnsupported::InvalidDefinition { message } => {
                write!(f, "invalid definition: {message}")
            }
            ToolUnsupported::PurlType { purl_type } => write!(f, "purl type `{purl_type}`"),
            ToolUnsupported::ToolBin { bin, value } => write!(f, "bin `{bin}` = `{value}`"),
            ToolUnsupported::Expression { expression } => {
                write!(f, "unsupported expression `{{{{{expression}}}}}`")
            }
            ToolUnsupported::Target => f.write_str("no entry for this target"),
            ToolUnsupported::Build => f.write_str("built from source"),
            ToolUnsupported::InsecureUrl { url } => write!(f, "not an https URL: {url}"),
        }
    }
}

fn invalid(message: impl Into<String>) -> ToolUnsupported {
    ToolUnsupported::InvalidDefinition {
        message: message.into(),
    }
}

/// 설치하는 purl 형식.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum PurlKind {
    Github,
    Generic,
    Npm,
    Pypi,
}

impl PurlKind {
    fn parse(kind: &str) -> Option<PurlKind> {
        match kind {
            "github" => Some(PurlKind::Github),
            "generic" => Some(PurlKind::Generic),
            "npm" => Some(PurlKind::Npm),
            "pypi" => Some(PurlKind::Pypi),
            _ => None,
        }
    }
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct ToolDeprecation {
    pub since: String,
    pub message: String,
}

/// registry 의 패키지 하나. 형식 문제는 [`Package::problem`] 에 남기고 목록에는 둔다(검색에 「지원하지 않음」 으로 보이게).
#[derive(Debug, Clone)]
pub struct Package {
    pub name: String,
    pub description: String,
    pub homepage: String,
    /// SPDX 표현식(registry 가 적은 그대로).
    pub licenses: Vec<String>,
    pub languages: Vec<String>,
    /// `LSP` · `DAP` · `Formatter` · `Linter` · `Runtime` · `Compiler` 등.
    pub categories: Vec<String>,
    /// `source.id`(purl).
    pub source_id: String,
    pub deprecation: Option<ToolDeprecation>,
    /// 패키지 전체가 「지원하지 않음」 인 까닭(모르는 키 · 형식 · purl 형식 · 빌드). 대상별 까닭은 [`Package::resolve`] 가 낸다.
    pub problem: Option<ToolUnsupported>,
    purl: Option<Purl>,
    raw: Map<String, Value>,
}

/// 패키지 정의로 읽을 수 없는 항목(이름도 없다). 목록에서 빠진다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unreadable(pub String);

impl Package {
    /// 정의 하나를 읽는다. 이름조차 없으면 [`Unreadable`].
    pub fn parse(value: &Value) -> Result<Package, Unreadable> {
        let obj = value
            .as_object()
            .ok_or_else(|| Unreadable("package definition is not an object".into()))?;
        let name = obj
            .get("name")
            .and_then(Value::as_str)
            .filter(|n| valid_name(n))
            .ok_or_else(|| Unreadable("package definition has no valid `name`".into()))?
            .to_owned();
        let mut problem = None;
        let mut note = |p: ToolUnsupported| {
            if problem.is_none() {
                problem = Some(p);
            }
        };
        let mut text = |key: &str| match obj.get(key) {
            Some(Value::String(s)) if s.len() <= MAX_TEXT_BYTES => s.clone(),
            Some(_) => {
                note(invalid(format!("`{key}` is not a string within the limit")));
                String::new()
            }
            None => {
                note(invalid(format!("`{key}` is missing")));
                String::new()
            }
        };
        let description = text("description");
        let homepage = text("homepage");
        let mut problem_list = Vec::new();
        let mut list = |key: &str| match string_list(obj.get(key), key) {
            Ok(v) => v,
            Err(e) => {
                problem_list.push(e);
                Vec::new()
            }
        };
        let licenses = list("licenses");
        let languages = list("languages");
        let categories = list("categories");
        for p in problem_list {
            note(p);
        }

        for key in obj.keys() {
            if !TOP_KEYS.contains(&key.as_str()) {
                note(ToolUnsupported::UnknownKey { key: key.clone() });
            }
        }
        let source = obj.get("source").and_then(Value::as_object);
        let mut source_id = String::new();
        let mut purl = None;
        match source {
            None => note(invalid("`source` is missing")),
            Some(source) => {
                for key in source.keys() {
                    if !SOURCE_KEYS.contains(&key.as_str()) {
                        note(ToolUnsupported::UnknownKey {
                            key: format!("source.{key}"),
                        });
                    }
                }
                match source.get("id").and_then(Value::as_str) {
                    None => note(invalid("`source.id` is missing")),
                    Some(id) => {
                        source_id = id.to_owned();
                        match Purl::parse(id) {
                            Err(e) => note(invalid(format!("source.id: {e}"))),
                            Ok(p) if p.version.is_none() => {
                                note(invalid("source.id has no version"))
                            }
                            Ok(p) => {
                                if PurlKind::parse(&p.kind).is_none() {
                                    note(ToolUnsupported::PurlType {
                                        purl_type: p.kind.clone(),
                                    });
                                }
                                purl = Some(p);
                            }
                        }
                    }
                }
                if source.contains_key("build") {
                    note(ToolUnsupported::Build);
                }
            }
        }
        let deprecation = match obj.get("deprecation") {
            None | Some(Value::Null) => None,
            Some(d) => {
                let since = d.get("since").and_then(Value::as_str);
                let message = d.get("message").and_then(Value::as_str);
                match (since, message) {
                    (Some(s), Some(m)) if s.len() + m.len() <= MAX_TEXT_BYTES => {
                        Some(ToolDeprecation {
                            since: s.to_owned(),
                            message: m.to_owned(),
                        })
                    }
                    _ => {
                        note(invalid("`deprecation` needs `since` and `message`"));
                        None
                    }
                }
            }
        };
        Ok(Package {
            name,
            description,
            homepage,
            licenses,
            languages,
            categories,
            source_id,
            deprecation,
            problem,
            purl,
            raw: obj.clone(),
        })
    }

    /// 원래 정의(내장 스냅숏을 만들 때 그대로 옮긴다).
    pub fn raw(&self) -> &Map<String, Value> {
        &self.raw
    }

    /// registry 가 가리키는 판(purl 의 판). 읽지 못했으면 빈 글.
    pub fn version(&self) -> &str {
        self.purl
            .as_ref()
            .and_then(|p| p.version.as_deref())
            .unwrap_or("")
    }

    /// `target` 에서 무엇을 받아 무엇을 실행하나. `over` 는 Pitwall 의 덮어쓰기 정의([`crate::overrides`]) — 있으면 그 판 ·
    /// 받을 주소 · `bin` 이 registry 의 것을 이긴다.
    pub fn resolve(
        &self,
        target: Option<ToolTarget>,
        over: Option<&Override>,
    ) -> Result<ResolvedTool, ToolUnsupported> {
        if let Some(p) = &self.problem {
            return Err(p.clone());
        }
        let target = target.ok_or(ToolUnsupported::Target)?;
        let purl = self.purl.as_ref().ok_or_else(|| invalid("no purl"))?;
        let kind = PurlKind::parse(&purl.kind).ok_or_else(|| ToolUnsupported::PurlType {
            purl_type: purl.kind.clone(),
        })?;
        let source = self
            .raw
            .get("source")
            .and_then(Value::as_object)
            .ok_or_else(|| invalid("no source"))?;
        let version = match over.and_then(|o| o.version.as_deref()) {
            Some(v) => v.to_owned(),
            None => purl.version.clone().unwrap_or_default(),
        };

        if let Some(platforms) = source.get("supported_platforms") {
            let names = string_list(Some(platforms), "source.supported_platforms")?;
            if !names.iter().any(|n| target.matches(n)) {
                return Err(ToolUnsupported::Target);
            }
        }

        // 고른 대상 항목을 판으로 먼저 푼다 — `bin` 등이 `{{source.asset.*}}` 로 그 값을 읽는다.
        let version_only = Context {
            version: &version,
            source: &Value::Null,
        };
        let override_download = over.and_then(|o| o.download_json());
        let (asset, download) = if let Some(d) = &override_download {
            (None, pick(Some(d), target, "download")?)
        } else {
            (
                pick(source.get("asset"), target, "source.asset")?,
                pick(source.get("download"), target, "source.download")?,
            )
        };
        let asset = asset.map(|a| render_value(a, &version_only)).transpose()?;
        let download = download
            .map(|d| render_value(d, &version_only))
            .transpose()?;
        let mut source_ctx = Map::new();
        if let Some(a) = &asset {
            source_ctx.insert("asset".into(), a.clone());
        }
        if let Some(d) = &download {
            source_ctx.insert("download".into(), d.clone());
        }
        let source_ctx = Value::Object(source_ctx);
        let ctx = Context {
            version: &version,
            source: &source_ctx,
        };

        let resolved_source = match kind {
            PurlKind::Github | PurlKind::Generic => {
                let files = if let Some(d) = &download {
                    download_files(d)?
                } else if let (PurlKind::Github, Some(a)) = (kind, &asset) {
                    asset_files(a, purl, &version)?
                } else {
                    return Err(invalid(
                        "a github or generic package needs `source.asset` or `source.download`",
                    ));
                };
                if files.is_empty() {
                    return Err(invalid("no files to download"));
                }
                ToolSource::Files { files }
            }
            PurlKind::Npm | PurlKind::Pypi => {
                let extra = match source.get("extra_packages") {
                    None => Vec::new(),
                    Some(v) => string_list(Some(v), "source.extra_packages")?
                        .iter()
                        .map(|s| ToolPackageSpec::parse(s, kind))
                        .collect::<Result<_, _>>()?,
                };
                let package = purl.full_name();
                if !valid_name(&package) {
                    return Err(invalid("package name is not valid"));
                }
                let spec = ToolPackageSpec {
                    name: package,
                    version: Some(version.clone()),
                };
                if kind == PurlKind::Npm {
                    ToolSource::Npm {
                        package: spec,
                        extra,
                    }
                } else {
                    ToolSource::Pypi {
                        package: spec,
                        extra,
                    }
                }
            }
        };

        let mut bin_raw = string_map(self.raw.get("bin"), "bin")?;
        let mut share_raw = string_map(self.raw.get("share"), "share")?;
        let opt_raw = string_map(self.raw.get("opt"), "opt")?;
        if let Some(o) = over {
            bin_raw.extend(o.bin.clone());
            share_raw.extend(o.share.clone());
        }
        let mut bin = BTreeMap::new();
        for (name, value) in &bin_raw {
            if !valid_file_name(name) {
                return Err(invalid(format!("bin name `{name}` is not a plain name")));
            }
            let rendered = render(value, &ctx)?;
            bin.insert(name.clone(), ToolBin::parse(name, &rendered, kind)?);
        }
        let share = render_paths(&share_raw, &ctx, "share")?;
        let opt = render_paths(&opt_raw, &ctx, "opt")?;
        Ok(ResolvedTool {
            name: self.name.clone(),
            version,
            kind,
            target,
            source: resolved_source,
            bin,
            share,
            opt,
            overridden: over.is_some(),
        })
    }
}

/// 한 대상에서 받을 것 · 실행할 것.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedTool {
    pub name: String,
    /// 받을 판(덮어쓰기 정의가 있으면 그 판).
    pub version: String,
    pub kind: PurlKind,
    pub target: ToolTarget,
    pub source: ToolSource,
    /// 실행 파일 이름 → 어디 있나.
    pub bin: BTreeMap<String, ToolBin>,
    /// 설치 폴더 안 경로(설치 폴더 기준) → 원본(설치 폴더 기준). Mason 의 `share` 링크.
    pub share: BTreeMap<String, String>,
    pub opt: BTreeMap<String, String>,
    /// Pitwall 의 덮어쓰기 정의가 이겼다.
    pub overridden: bool,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ToolSource {
    /// 파일을 받는다(`github` 릴리스 자산 · `generic` URL).
    Files { files: Vec<ToolDownload> },
    /// npm 패키지(+ `extra_packages`)를 설치한다.
    Npm {
        package: ToolPackageSpec,
        extra: Vec<ToolPackageSpec>,
    },
    /// PyPI 패키지(+ `extra_packages`)를 전용 venv 에 설치한다.
    Pypi {
        package: ToolPackageSpec,
        extra: Vec<ToolPackageSpec>,
    },
}

/// npm · PyPI 패키지 이름과 판(판이 없으면 그 레지스트리의 최신 — registry 가 적은 그대로).
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct ToolPackageSpec {
    pub name: String,
    pub version: Option<String>,
}

impl ToolPackageSpec {
    /// npm `name@1.2.3` · `@scope/name@1` · `name`, PyPI `name==1.2` · `name@1.2` · `name`.
    fn parse(s: &str, kind: PurlKind) -> Result<ToolPackageSpec, ToolUnsupported> {
        let (name, version) = if kind == PurlKind::Pypi && s.contains("==") {
            let (n, v) = s.split_once("==").unwrap_or((s, ""));
            (n, Some(v))
        } else {
            match s.rfind('@') {
                Some(at) if at > 0 => (&s[..at], Some(&s[at + 1..])),
                _ => (s, None),
            }
        };
        let version = version.map(str::to_owned);
        if !valid_name(name) || version.as_deref().is_some_and(|v| !valid_version(v)) {
            return Err(invalid(format!("extra package `{s}` is not valid")));
        }
        Ok(ToolPackageSpec {
            name: name.to_owned(),
            version,
        })
    }
}

/// 받을 파일 하나.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct ToolDownload {
    /// 저장 이름(경로 구분자 없는 파일 이름). 압축 형식은 이것으로 정한다 — kotlin-lsp 는 `.sit` 주소를 `kotlin-lsp.zip` 으로 받는다.
    pub name: String,
    pub url: String,
    /// 풀 곳(설치 폴더 기준, `/` 로 끝나면 폴더). 없으면 설치 폴더.
    pub dest: Option<String>,
    pub format: ArchiveFormat,
}

/// 저장 이름의 끝으로 정한 형식.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum ArchiveFormat {
    TarGz,
    TarXz,
    Tar,
    Zip,
    /// `.gz` 단일 파일.
    Gz,
    /// 그 밖(`.jar` · `.vsix` 가 아닌 실행 파일 등) — 받은 그대로.
    Plain,
}

impl ArchiveFormat {
    pub fn from_name(name: &str) -> ArchiveFormat {
        let lower = name.to_ascii_lowercase();
        if lower.ends_with(".tar.gz") || lower.ends_with(".tgz") {
            ArchiveFormat::TarGz
        } else if lower.ends_with(".tar.xz") || lower.ends_with(".txz") {
            ArchiveFormat::TarXz
        } else if lower.ends_with(".tar") {
            ArchiveFormat::Tar
        } else if lower.ends_with(".zip") || lower.ends_with(".vsix") {
            ArchiveFormat::Zip
        } else if lower.ends_with(".gz") {
            ArchiveFormat::Gz
        } else {
            ArchiveFormat::Plain
        }
    }
}

/// 실행 파일이 있는 곳.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(tag = "kind", content = "value", rename_all = "camelCase")]
pub enum ToolBin {
    /// 설치 폴더 안 경로(`/` 구분).
    Path(String),
    /// npm 패키지의 실행 파일 이름(`node_modules/.bin/<이름>`).
    Npm(String),
    /// venv 의 실행 파일 이름(`venv/bin/<이름>`).
    Pypi(String),
    /// venv 의 python 으로 돌리는 스크립트(설치 폴더 안 경로).
    Python(String),
}

impl ToolBin {
    fn parse(bin: &str, value: &str, kind: PurlKind) -> Result<ToolBin, ToolUnsupported> {
        let unsupported = || ToolUnsupported::ToolBin {
            bin: bin.to_owned(),
            value: value.to_owned(),
        };
        match bin_prefix(value) {
            Some(("npm", name)) if kind == PurlKind::Npm && valid_file_name(name) => {
                Ok(ToolBin::Npm(name.to_owned()))
            }
            Some(("pypi", name)) if kind == PurlKind::Pypi && valid_file_name(name) => {
                Ok(ToolBin::Pypi(name.to_owned()))
            }
            Some(("python", path)) => {
                check_relative(path).map_err(|_| unsupported())?;
                Ok(ToolBin::Python(path.to_owned()))
            }
            Some(_) => Err(unsupported()),
            None => {
                check_relative(value).map_err(|m| invalid(format!("bin `{bin}`: {m}")))?;
                Ok(ToolBin::Path(value.to_owned()))
            }
        }
    }
}

/// `asset` · `download`(객체 하나 또는 목록)에서 `target` 에 맞는 첫 항목. 값이 없으면 `None`, 있는데 맞는 항목이 없으면
/// [`ToolUnsupported::Target`]. `target` 키가 없는 항목은 모든 대상에 맞는다.
fn pick<'a>(
    value: Option<&'a Value>,
    target: ToolTarget,
    key: &str,
) -> Result<Option<&'a Value>, ToolUnsupported> {
    let Some(value) = value else {
        return Ok(None);
    };
    let entries: Vec<&Value> = match value {
        Value::Array(items) if items.len() <= MAX_LIST_ITEMS => items.iter().collect(),
        Value::Object(_) => vec![value],
        _ => return Err(invalid(format!("`{key}` is not an object or a list"))),
    };
    for entry in entries {
        if !entry.is_object() {
            return Err(invalid(format!(
                "`{key}` has an entry that is not an object"
            )));
        }
        let matches = match entry.get("target") {
            None => true,
            Some(Value::String(t)) => target.matches(t),
            Some(Value::Array(ts)) => ts
                .iter()
                .any(|t| t.as_str().is_some_and(|t| target.matches(t))),
            Some(_) => return Err(invalid(format!("`{key}.target` is not a string or a list"))),
        };
        if matches {
            return Ok(Some(entry));
        }
    }
    Err(ToolUnsupported::Target)
}

fn render(template: &str, ctx: &Context<'_>) -> Result<String, ToolUnsupported> {
    expr::render(template, ctx).map_err(|e| ToolUnsupported::Expression {
        expression: e.expression,
    })
}

/// 객체 · 목록 안의 글을 모두 푼다(키는 그대로).
fn render_value(value: &Value, ctx: &Context<'_>) -> Result<Value, ToolUnsupported> {
    Ok(match value {
        Value::String(s) => Value::String(render(s, ctx)?),
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|v| render_value(v, ctx))
                .collect::<Result<_, _>>()?,
        ),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| Ok((k.clone(), render_value(v, ctx)?)))
                .collect::<Result<_, ToolUnsupported>>()?,
        ),
        other => other.clone(),
    })
}

/// github 릴리스 자산(`file` — 글 하나 또는 목록, `이름[:풀 곳]`).
fn asset_files(
    asset: &Value,
    purl: &Purl,
    version: &str,
) -> Result<Vec<ToolDownload>, ToolUnsupported> {
    let files = match asset.get("file") {
        Some(Value::String(s)) => vec![s.as_str()],
        Some(Value::Array(items)) if items.len() <= MAX_LIST_ITEMS => items
            .iter()
            .map(|v| {
                v.as_str()
                    .ok_or_else(|| invalid("`asset.file` has a non-string"))
            })
            .collect::<Result<_, _>>()?,
        _ => return Err(invalid("`source.asset` has no `file`")),
    };
    let repo = purl.full_name();
    if purl.namespace.is_none()
        || !repo.split('/').all(valid_url_segment)
        || !valid_url_segment(version)
    {
        return Err(invalid(
            "github purl needs `owner/repo` and a plain version",
        ));
    }
    files
        .into_iter()
        .map(|spec| {
            let (name, dest) = split_dest(spec)?;
            Ok(ToolDownload {
                url: format!("https://github.com/{repo}/releases/download/{version}/{name}"),
                format: ArchiveFormat::from_name(&name),
                name,
                dest,
            })
        })
        .collect()
}

/// `download.files`(저장 이름 → 주소).
fn download_files(download: &Value) -> Result<Vec<ToolDownload>, ToolUnsupported> {
    let files = download
        .get("files")
        .and_then(Value::as_object)
        .ok_or_else(|| invalid("`source.download` has no `files`"))?;
    if files.len() > MAX_LIST_ITEMS {
        return Err(invalid("`source.download.files` is too long"));
    }
    files
        .iter()
        .map(|(name, url)| {
            let url = url
                .as_str()
                .ok_or_else(|| invalid("`download.files` has a non-string URL"))?;
            let (name, dest) = split_dest(name)?;
            check_url(url)?;
            Ok(ToolDownload {
                format: ArchiveFormat::from_name(&name),
                name,
                url: url.to_owned(),
                dest,
            })
        })
        .collect()
}

fn split_dest(spec: &str) -> Result<(String, Option<String>), ToolUnsupported> {
    let (name, dest) = match spec.split_once(':') {
        Some((n, d)) => (n, Some(d)),
        None => (spec, None),
    };
    if !valid_file_name(name) {
        return Err(invalid(format!("file name `{name}` is not a plain name")));
    }
    if let Some(d) = dest {
        check_relative(d).map_err(|m| invalid(format!("file `{name}` destination: {m}")))?;
    }
    Ok((name.to_owned(), dest.map(str::to_owned)))
}

fn check_url(url: &str) -> Result<(), ToolUnsupported> {
    let insecure = || ToolUnsupported::InsecureUrl {
        url: url.chars().take(MAX_TEXT_BYTES).collect(),
    };
    if url.len() > MAX_TEXT_BYTES {
        return Err(insecure());
    }
    let parsed = url::Url::parse(url).map_err(|_| insecure())?;
    if parsed.scheme() != "https" || parsed.host_str().is_none_or(str::is_empty) {
        return Err(insecure());
    }
    Ok(())
}

fn render_paths(
    raw: &BTreeMap<String, String>,
    ctx: &Context<'_>,
    key: &str,
) -> Result<BTreeMap<String, String>, ToolUnsupported> {
    let mut out = BTreeMap::new();
    for (dest, src) in raw {
        let src = render(src, ctx)?;
        check_relative(dest).map_err(|m| invalid(format!("{key} `{dest}`: {m}")))?;
        check_relative(&src).map_err(|m| invalid(format!("{key} `{dest}` source: {m}")))?;
        out.insert(dest.clone(), src);
    }
    Ok(out)
}

/// 설치 폴더 안의 상대 경로인가(`/` 구분, 끝의 `/` 허용). 절대 경로 · 드라이브 · `..` · 빈 세그먼트 · `\` 는 거절.
pub fn check_relative(path: &str) -> Result<(), String> {
    if path.is_empty() || path.len() > MAX_TEXT_BYTES {
        return Err("empty or too long path".into());
    }
    if path.starts_with('/') || path.contains('\\') || path.contains(':') {
        return Err(format!("`{path}` is not a relative path"));
    }
    if path.chars().any(char::is_control) {
        return Err(format!("`{path}` has a control character"));
    }
    let trimmed = path.strip_suffix('/').unwrap_or(path);
    for seg in trimmed.split('/') {
        if seg.is_empty() || seg == ".." {
            return Err(format!(
                "`{path}` leaves the install folder or has an empty segment"
            ));
        }
    }
    Ok(())
}

/// 경로 구분자 없는 파일 이름.
pub fn valid_file_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 255
        && name != "."
        && name != ".."
        && !name.contains(['/', '\\', ':'])
        && !name.chars().any(char::is_control)
}

/// 패키지 이름(npm 스코프의 `@` · `/` 를 받는다).
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 214
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '@' | '/'))
        && !name
            .split('/')
            .any(|s| s.is_empty() || s == "." || s == "..")
}

fn valid_version(v: &str) -> bool {
    !v.is_empty()
        && v.len() <= 128
        && v.chars().all(|c| {
            c.is_ascii_alphanumeric()
                || matches!(c, '-' | '_' | '.' | '+' | '~' | '^' | '<' | '>' | '=' | '*')
        })
}

/// URL 경로 세그먼트에 그대로 넣어도 되는 글.
fn valid_url_segment(s: &str) -> bool {
    !s.is_empty()
        && s != "."
        && s != ".."
        && s.len() <= 255
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '+'))
}

fn string_list(value: Option<&Value>, key: &str) -> Result<Vec<String>, ToolUnsupported> {
    match value {
        None => Ok(Vec::new()),
        Some(Value::Array(items)) if items.len() <= MAX_LIST_ITEMS => items
            .iter()
            .map(|v| match v.as_str() {
                Some(s) if s.len() <= MAX_TEXT_BYTES => Ok(s.to_owned()),
                _ => Err(invalid(format!(
                    "`{key}` has a non-string or too long item"
                ))),
            })
            .collect(),
        Some(_) => Err(invalid(format!("`{key}` is not a list within the limit"))),
    }
}

fn string_map(
    value: Option<&Value>,
    key: &str,
) -> Result<BTreeMap<String, String>, ToolUnsupported> {
    match value {
        None => Ok(BTreeMap::new()),
        Some(Value::Object(map)) if map.len() <= MAX_LIST_ITEMS => map
            .iter()
            .map(|(k, v)| match v.as_str() {
                Some(s) if s.len() <= MAX_TEXT_BYTES && k.len() <= MAX_TEXT_BYTES => {
                    Ok((k.clone(), s.to_owned()))
                }
                _ => Err(invalid(format!(
                    "`{key}.{k}` is not a string within the limit"
                ))),
            })
            .collect(),
        Some(_) => Err(invalid(format!("`{key}` is not a table within the limit"))),
    }
}

#[cfg(test)]
mod tests;
