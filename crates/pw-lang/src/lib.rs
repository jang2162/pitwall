//! 파일 → 언어 판별과 벤더 · 생성 · 문서 파일 판정(`docs/design/07-extensibility.md` 「언어 판별 — GitHub Linguist 데이터」).
//!
//! 판별 순서([`detect`]):
//! 1. 설정의 명시적 매핑(`[languages.mapping] "*.conf" = "nginx"` — [`Mapping`])
//! 2. `.gitattributes` 의 `linguist-language`([`attributes`])
//! 3. ~ 4. Linguist 의 판별 단계 그대로(모드라인 → 파일명 → shebang → 확장자 → XML 머리 → manpage → heuristics, [`linguist`])
//! 5. 확장의 `[contributes.languages]` 확장자([`Contributed`])
//! 6. 실패 → 언어 없음(화면은 플레인 텍스트로 연다)
//!
//! Linguist 데이터는 `linguist/` 에 버전을 고정해 두고 빌드 때 표로 바꾼다(`build.rs`, 받는 법은 `CONTRIBUTING.md` 「Linguist 데이터」 절).
//! 언어 이름은 Linguist 이름이다(언어 확장이 `linguist = "<이름>"` 으로 잇는다).

pub mod attributes;
pub mod generated;
pub mod linguist;
pub mod methods;
#[cfg(test)]
mod ruby_regex;
#[cfg(test)]
mod tests;

use bstr::ByteSlice;
use gix_attributes::glob::Pattern;
use gix_attributes::glob::pattern::Case;
use gix_attributes::glob::wildmatch::Mode;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use attributes::Attrs;
pub use linguist::{HEAD_BYTES, SKIPPED_PATTERNS};

/// 언어를 정한 단계.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum LangSource {
    /// 설정의 `[languages.mapping]`.
    Mapping,
    /// `.gitattributes` 의 `linguist-language`.
    Gitattributes,
    Modeline,
    Filename,
    Shebang,
    Extension,
    /// 앞 두 줄의 `xml version=`.
    Xml,
    Manpage,
    Heuristics,
    /// Linguist 단계가 후보를 여럿 남겼다(Linguist 는 여기서 분류기를 쓴다 — 옮기지 않았다). 대표 확장자 · popular.yml 로
    /// 고른다(`linguist::pick`).
    Ambiguous,
    /// 확장의 `[contributes.languages]` 확장자.
    Contributed,
    /// 정하지 못했다(플레인 텍스트).
    None,
}

/// 한 파일의 판별 결과.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detection {
    /// Linguist 이름. 확장이 Linguist 에 없는 이름을 기여했으면 그 이름이다.
    pub language: Option<String>,
    pub source: LangSource,
    pub vendored: bool,
    pub generated: bool,
    pub documentation: bool,
}

/// 판별할 파일 하나.
#[derive(Debug, Clone, Copy)]
pub struct Input<'a> {
    /// 저장소 꼭대기 기준 경로(`/` 구분). 저장소 밖이면 프로젝트 루트 기준, 그것도 없으면 파일 이름.
    /// Linguist 의 경로 규칙(vendor · documentation · generated · heuristics 의 확장자)이 이것을 본다.
    pub repo_path: &'a str,
    /// 설정 매핑이 볼 경로: 프로젝트 루트 기준(`/` 구분). 프로젝트 밖이면 파일 이름.
    pub project_path: &'a str,
    /// 파일 앞부분. 없으면 빈 파일로 본다(`linguist::detect`).
    pub head: Option<&'a str>,
}

/// 설정의 `[languages.mapping]`: 글롭 → 언어. 글롭 문법과 맞추는 방법은 `.gitattributes` 의 패턴과 같다(`/` 가 없으면 파일
/// 이름에, 있으면 프로젝트 루트 기준 경로에 맞춘다).
///
/// 여럿이 맞으면 프로젝트 설정이 전역 설정을 이기고, 같은 설정 안에서는 긴 패턴이 이긴다(길이가 같으면 글자 순으로 뒤의 것) —
/// 설정 값은 키 순서를 지키지 않으므로(JSON 객체) 적은 순서로 정하지 않는다.
#[derive(Debug, Default)]
pub struct Mapping {
    /// 우선순위가 높은 것부터.
    rules: Vec<(Pattern, String)>,
}

impl Mapping {
    /// `project` · `global` 은 각 설정의 `languages.mapping` 값. 언어는 Linguist 이름 · 별칭(대소문자 무시)이거나 확장이 기여한
    /// 언어의 `linguist` 이름이어야 한다. 모르는 언어 · 글롭이 아닌 키 · 문자열이 아닌 값은 `warnings` 에 넣고 건너뛴다.
    pub fn from_settings(
        project: Option<&Value>,
        global: Option<&Value>,
        contributed: &Contributed,
        warnings: &mut Vec<String>,
    ) -> Self {
        let mut rules = Vec::new();
        for (scope, value) in [("project", project), ("global", global)] {
            let Some(value) = value else { continue };
            let Some(table) = value.as_object() else {
                warnings.push(format!("{scope} `languages.mapping` is not a table"));
                continue;
            };
            let mut entries: Vec<(&String, &Value)> = table.iter().collect();
            entries.sort_by(|a, b| b.0.len().cmp(&a.0.len()).then_with(|| b.0.cmp(a.0)));
            for (glob, language) in entries {
                let Some(language) = language.as_str() else {
                    warnings.push(format!(
                        "{scope} `languages.mapping.\"{glob}\"` is not a string"
                    ));
                    continue;
                };
                let Some(name) = resolve_language(language, contributed) else {
                    warnings.push(format!(
                        "{scope} `languages.mapping.\"{glob}\"`: unknown language `{language}`"
                    ));
                    continue;
                };
                match Pattern::from_bytes(glob.as_bytes()) {
                    Some(p) if !p.is_negative() => rules.push((p, name)),
                    _ => warnings.push(format!(
                        "{scope} `languages.mapping.\"{glob}\"`: not a usable pattern"
                    )),
                }
            }
        }
        Self { rules }
    }

    pub fn get(&self, path: &str) -> Option<&str> {
        let basename = path.rfind('/').map(|i| i + 1);
        self.rules
            .iter()
            .find(|(p, _)| {
                p.matches_repo_relative_path(
                    path.as_bytes().as_bstr(),
                    basename,
                    Some(false),
                    Case::Sensitive,
                    Mode::NO_MATCH_SLASH_LITERAL,
                )
            })
            .map(|(_, name)| name.as_str())
    }
}

/// 이름 · 별칭을 Linguist 이름으로. Linguist 에 없으면 확장이 기여한 이름에서 찾는다.
fn resolve_language(name: &str, contributed: &Contributed) -> Option<String> {
    if let Some(id) = linguist::find_by_alias(name).or_else(|| linguist::find_by_name(name)) {
        return Some(linguist::language(id).name.to_owned());
    }
    contributed
        .names()
        .find(|n| n.eq_ignore_ascii_case(name))
        .map(str::to_owned)
}

/// 확장의 `[contributes.languages]` 가 더한 확장자 → `linguist` 이름과, 언어마다의 줄 주석(`line_comment`). 같은 확장자 · 같은
/// 언어의 줄 주석을 둘이 기여하면 먼저 등록된 것(내장 → 사용자 확장 순)이 이긴다.
#[derive(Debug, Default, Clone)]
pub struct Contributed {
    /// 소문자 확장자(점 없이) → 이름.
    extensions: Vec<(String, String)>,
    /// 이름 → 줄 주석 기호.
    line_comments: Vec<(String, String)>,
}

impl Contributed {
    pub fn from_registry(registry: &pw_ext::Registry) -> Self {
        let mut extensions: Vec<(String, String)> = Vec::new();
        let mut line_comments: Vec<(String, String)> = Vec::new();
        for (_, decl) in registry.languages() {
            for ext in &decl.extensions {
                let ext = ext.trim_start_matches('.').to_lowercase();
                if !ext.is_empty() && !extensions.iter().any(|(e, _)| *e == ext) {
                    extensions.push((ext, decl.linguist.clone()));
                }
            }
            if let Some(token) = decl.line_comment.as_deref().filter(|t| !t.is_empty())
                && !line_comments.iter().any(|(n, _)| *n == decl.linguist)
            {
                line_comments.push((decl.linguist.clone(), token.to_owned()));
            }
        }
        Self {
            extensions,
            line_comments,
        }
    }

    #[cfg(test)]
    pub(crate) fn from_pairs(pairs: &[(&str, &str)]) -> Self {
        Self {
            extensions: pairs
                .iter()
                .map(|(e, n)| ((*e).to_owned(), (*n).to_owned()))
                .collect(),
            line_comments: Vec::new(),
        }
    }

    /// 확장이 기여한 언어 이름(확장자 · 줄 주석 어느 쪽으로든). 처음 나온 순서이고 겹치지 않는다.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        let mut seen: Vec<&str> = Vec::new();
        self.extensions
            .iter()
            .map(|(_, n)| n.as_str())
            .chain(self.line_comments.iter().map(|(n, _)| n.as_str()))
            .filter(move |n| {
                let new = !seen.contains(n);
                if new {
                    seen.push(n);
                }
                new
            })
    }

    /// 언어(`linguist` 이름)의 줄 주석 기호. 기여한 확장이 없거나 적지 않았으면 `None`.
    pub fn line_comment(&self, language: &str) -> Option<&str> {
        self.line_comments
            .iter()
            .find(|(n, _)| n == language)
            .map(|(_, t)| t.as_str())
    }

    /// 가장 긴 확장자부터(`a.d.ts` 는 `d.ts`, 그다음 `ts`).
    fn get(&self, file_name: &str) -> Option<&str> {
        let lower = file_name.to_lowercase();
        let mut rest = lower.as_str();
        while let Some(dot) = rest.find('.') {
            rest = &rest[dot + 1..];
            if let Some((_, name)) = self.extensions.iter().find(|(e, _)| e == rest) {
                return Some(name);
            }
        }
        None
    }
}

/// 판별 순서 1 ~ 6 과 벤더 · 생성 · 문서 판정. `attrs` 는 그 파일의 `.gitattributes` 속성(없으면 기본값).
pub fn detect(
    input: Input<'_>,
    attrs: &Attrs,
    mapping: &Mapping,
    contributed: &Contributed,
) -> Detection {
    let head = input.head.map(|h| truncate(h, HEAD_BYTES));
    let (language, source) = language(input, head, attrs, mapping, contributed);
    let path = input.repo_path;
    Detection {
        language,
        source,
        vendored: attrs
            .vendored
            .unwrap_or_else(|| linguist::vendored_path(path)),
        generated: attrs
            .generated
            .unwrap_or_else(|| generated::generated(path, head).is_some()),
        documentation: attrs
            .documentation
            .unwrap_or_else(|| linguist::documentation_path(path)),
    }
}

fn language(
    input: Input<'_>,
    head: Option<&str>,
    attrs: &Attrs,
    mapping: &Mapping,
    contributed: &Contributed,
) -> (Option<String>, LangSource) {
    if let Some(name) = mapping.get(input.project_path) {
        return (Some(name.to_owned()), LangSource::Mapping);
    }
    // Linguist 와 같이 별칭으로 찾고, 모르는 이름이면 다음 단계로 넘어간다.
    if let Some(id) = attrs.language.as_deref().and_then(linguist::find_by_alias) {
        return (
            Some(linguist::language(id).name.to_owned()),
            LangSource::Gitattributes,
        );
    }
    if let Some((id, strategy)) = linguist::detect(input.repo_path, head.map(str::as_bytes)) {
        let source = match strategy {
            linguist::Strategy::Modeline => LangSource::Modeline,
            linguist::Strategy::Filename => LangSource::Filename,
            linguist::Strategy::Shebang => LangSource::Shebang,
            linguist::Strategy::Extension => LangSource::Extension,
            linguist::Strategy::Xml => LangSource::Xml,
            linguist::Strategy::Manpage => LangSource::Manpage,
            linguist::Strategy::Heuristics => LangSource::Heuristics,
            linguist::Strategy::Ambiguous => LangSource::Ambiguous,
        };
        return (Some(linguist::language(id).name.to_owned()), source);
    }
    let name = input
        .repo_path
        .rsplit('/')
        .next()
        .unwrap_or(input.repo_path);
    if let Some(name) = contributed.get(name) {
        return (Some(name.to_owned()), LangSource::Contributed);
    }
    (None, LangSource::None)
}

/// 글자 경계에서 `max` 바이트 이하로 자른다.
fn truncate(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}
