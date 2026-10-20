//! 기여 레지스트리: 내장 확장과 사용자 확장(`$DATA/extensions/<폴더>/`)을 읽어 기여를 모은다.
//!
//! 등록 규칙(오류 처리 · 엔진 버전 · 겹침 · 어디까지 읽는가)의 정본은 `docs/design/07-extensibility.md` 「2. 확장 매니페스트」 「레지스트리 (`crates/pw-ext`)」 표다.

use std::path::{Path, PathBuf};

use semver::Version;
use serde_json::Value;

use crate::builtin;
use crate::manifest::{
    Diagnostic, ExternalToolDecl, KeymapDecl, KeymapKind, LanguageDecl, Manifest, PathRef, Pos,
    Severity, TranslationDecl, parse_manifest,
};
use crate::source::{EntryKind, ExtensionRoot, MANIFEST, RelPath, SourceError};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Appearance {
    Light,
    Dark,
}

impl Appearance {
    pub const ALL: [Appearance; 2] = [Appearance::Light, Appearance::Dark];

    pub fn key(self) -> &'static str {
        match self {
            Appearance::Light => "light",
            Appearance::Dark => "dark",
        }
    }
}

/// 읽어 둔 테마 파일. 토큰 검사는 화면이 한다 — 앱이 아는 토큰 목록의 정본이 UI 쪽(`TOKENS`)에 있다.
#[derive(Debug, Clone, PartialEq)]
pub struct LoadedTheme {
    pub id: String,
    pub label: String,
    /// 파일에 있는 변형. 하나 이상이다.
    pub appearances: Vec<Appearance>,
    /// 파일 내용 그대로.
    pub content: Value,
}

/// 읽어 둔 UI 번역 하나(언어 하나). 내용(Fluent 문법)은 등록할 때 검사했다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedTranslation {
    /// BCP 47 언어 태그. 기여 이름 그대로다.
    pub locale: String,
    /// 그 언어 폴더의 `.ftl` 파일. 이름순이다.
    pub files: Vec<TranslationFile>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranslationFile {
    /// 사람이 읽는 위치(`builtin:i18n-default/i18n/ko/app.ftl` 또는 절대 경로).
    pub display: String,
    /// 파일 이름(`app.ftl`).
    pub name: String,
    pub source: String,
}

/// 등록된 확장 하나.
#[derive(Debug, Clone)]
pub struct Extension {
    pub manifest: Manifest,
    pub root: ExtensionRoot,
    /// `manifest.contributes.themes` 와 같은 순서.
    pub themes: Vec<LoadedTheme>,
    /// `manifest.contributes.translations` 와 같은 순서.
    pub translations: Vec<LoadedTranslation>,
}

/// 확장 폴더 안의 파일 하나. 내장이면 바이너리에서, 아니면 디스크에서 읽는다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedFile {
    pub root: ExtensionRoot,
    pub path: RelPath,
}

impl ResolvedFile {
    pub fn read(&self) -> Result<std::borrow::Cow<'static, [u8]>, SourceError> {
        self.root.read(&self.path)
    }

    /// 사람이 읽는 위치(`builtin:keymap-default/keymaps/default.toml` 또는 절대 경로).
    pub fn display(&self) -> String {
        self.root.display(Some(&self.path))
    }
}

impl ResolvedFile {
    /// 폴더를 가리키는 것에서 그 바로 아래 파일 하나(`queries/rust` → `queries/rust/highlights.scm`).
    pub fn join(&self, name: &str) -> Result<ResolvedFile, String> {
        Ok(ResolvedFile {
            root: self.root.clone(),
            path: RelPath::parse(&format!("{}/{name}", self.path))?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedKeymap {
    pub extension: String,
    pub id: String,
    pub kind: KeymapKind,
    pub file: ResolvedFile,
}

/// 언어 기여와 그 문법 · 쿼리의 위치. 내용은 구문 분석 서비스(`pw-syntax`)가 처음 쓸 때 읽는다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedLanguage {
    pub extension: String,
    /// 기여 이름(`[contributes.languages.<id>]`).
    pub id: String,
    /// Linguist 언어 이름. 언어 판별(`lang.detect`)의 결과와 이 이름으로 잇는다.
    pub linguist: String,
    /// tree-sitter 문법 WASM.
    pub grammar: Option<ResolvedFile>,
    /// 쿼리 폴더(`highlights.scm` 등). [`ResolvedFile::join`] 으로 그 안의 파일을 가리킨다.
    pub queries: Option<ResolvedFile>,
}

#[derive(Debug, Clone)]
pub struct LoadOptions {
    /// 이 앱의 버전. `engines.pitwall` 범위와 맞춰 본다.
    pub engine: Version,
    /// 사용자 확장 폴더(`$DATA/extensions`). 없으면 내장만 읽는다.
    pub user_dir: Option<PathBuf>,
}

impl LoadOptions {
    /// 사용자 확장을 `$DATA/extensions/` 에서 찾는다(`docs/design/03-architecture.md` 「5-0. 파일 위치 — XDG 규칙」).
    pub fn with_data_dir(engine: Version, data: &Path) -> Self {
        Self {
            engine,
            user_dir: Some(data.join(USER_DIR)),
        }
    }
}

/// 레지스트리나 RPC 가 내용을 통째로 읽는 파일(테마 · 키맵)의 크기 상한. 넘으면 오류다 — 시작할 때 메모리에 올린다.
pub const MAX_CONTENT_BYTES: u64 = 1024 * 1024;

/// `$DATA` 아래 사용자 확장 폴더 이름.
pub const USER_DIR: &str = "extensions";

#[derive(Debug, Clone, Default)]
pub struct Registry {
    extensions: Vec<Extension>,
    diagnostics: Vec<Diagnostic>,
}

impl Registry {
    /// 내장 확장과 `user_dir` 아래 폴더들을 읽는다. 진단은 [`Self::diagnostics`] 와 로그(tracing)에 남는다.
    pub fn load(options: &LoadOptions) -> Self {
        let mut roots = builtin::roots();
        let mut registry = Self::default();
        if let Some(dir) = &options.user_dir {
            roots.extend(registry.discover(dir));
        }
        for root in roots {
            registry.add(&options.engine, root);
        }
        for d in &registry.diagnostics {
            match d.severity {
                Severity::Error => tracing::error!(target: "pw_ext", "{d}"),
                Severity::Warning => tracing::warn!(target: "pw_ext", "{d}"),
            }
        }
        registry
    }

    /// 주어진 폴더들만 읽는다(시험 · 도구용). 내장 확장은 넘긴 것만 읽는다.
    pub fn from_roots(engine: &Version, roots: impl IntoIterator<Item = ExtensionRoot>) -> Self {
        let mut registry = Self::default();
        for root in roots {
            registry.add(engine, root);
        }
        registry
    }

    pub fn extensions(&self) -> &[Extension] {
        &self.extensions
    }

    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    pub fn themes(&self) -> impl Iterator<Item = (&Extension, &LoadedTheme)> {
        self.extensions
            .iter()
            .flat_map(|e| e.themes.iter().map(move |t| (e, t)))
    }

    pub fn theme(&self, id: &str) -> Option<(&Extension, &LoadedTheme)> {
        self.themes().find(|(_, t)| t.id == id)
    }

    pub fn keymaps(&self) -> impl Iterator<Item = (&Extension, &KeymapDecl)> {
        self.extensions
            .iter()
            .flat_map(|e| e.manifest.contributes.keymaps.iter().map(move |k| (e, k)))
    }

    /// 키맵 기여와 그 파일. 파일 내용은 키맵 엔진이 [`ResolvedFile::read`] 로 읽는다.
    pub fn keymap(&self, id: &str) -> Option<ResolvedKeymap> {
        self.keymaps()
            .find(|(_, k)| k.id == id)
            .map(|(e, k)| ResolvedKeymap {
                extension: e.manifest.id.clone(),
                id: k.id.clone(),
                kind: k.kind,
                file: ResolvedFile {
                    root: e.root.clone(),
                    path: k.file.path.clone(),
                },
            })
    }

    pub fn external_tools(&self) -> impl Iterator<Item = (&Extension, &ExternalToolDecl)> {
        self.extensions.iter().flat_map(|e| {
            e.manifest
                .contributes
                .external_tools
                .iter()
                .map(move |t| (e, t))
        })
    }

    pub fn languages(&self) -> impl Iterator<Item = (&Extension, &LanguageDecl)> {
        self.extensions
            .iter()
            .flat_map(|e| e.manifest.contributes.languages.iter().map(move |l| (e, l)))
    }

    /// 언어 기여를 그 파일 위치와 함께. 등록 순서(내장 → 사용자 확장)다.
    pub fn resolved_languages(&self) -> impl Iterator<Item = ResolvedLanguage> + '_ {
        self.languages().map(|(e, l)| {
            let resolve = |p: &PathRef| ResolvedFile {
                root: e.root.clone(),
                path: p.path.clone(),
            };
            ResolvedLanguage {
                extension: e.manifest.id.clone(),
                id: l.id.clone(),
                linguist: l.linguist.clone(),
                grammar: l.grammar.as_ref().map(resolve),
                queries: l.queries.as_ref().map(resolve),
            }
        })
    }

    /// Linguist 이름(대소문자 무시)으로 언어 기여 하나. 여럿이면 먼저 등록된 것(내장 → 사용자 확장)이다.
    pub fn language_for_linguist(&self, linguist: &str) -> Option<ResolvedLanguage> {
        self.resolved_languages()
            .find(|l| l.linguist.eq_ignore_ascii_case(linguist))
    }

    /// 읽어 둔 UI 번역. 등록 순서(내장 → 사용자 확장)다.
    pub fn loaded_translations(&self) -> impl Iterator<Item = (&Extension, &LoadedTranslation)> {
        self.extensions
            .iter()
            .flat_map(|e| e.translations.iter().map(move |t| (e, t)))
    }

    pub fn translations(&self) -> impl Iterator<Item = (&Extension, &TranslationDecl)> {
        self.extensions.iter().flat_map(|e| {
            e.manifest
                .contributes
                .translations
                .iter()
                .map(move |t| (e, t))
        })
    }

    /// `dir` 바로 아래 폴더들. 이름순이고 `.` 으로 시작하는 것은 건너뛴다. 폴더가 없으면 사용자 확장이 없는 것이다.
    fn discover(&mut self, dir: &Path) -> Vec<ExtensionRoot> {
        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
            Err(e) => {
                self.diagnostics.push(Diagnostic {
                    severity: Severity::Warning,
                    file: dir.display().to_string(),
                    pos: None,
                    message: format!("cannot list extensions: {e}"),
                });
                return Vec::new();
            }
        };
        let mut folders: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
        folders.sort();
        folders.into_iter().map(ExtensionRoot::Dir).collect()
    }

    fn add(&mut self, engine: &Version, root: ExtensionRoot) {
        let manifest_path = RelPath::parse(MANIFEST).expect("constant path");
        let file = root.display(Some(&manifest_path));
        let source = match root.read(&manifest_path) {
            Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
            Err(SourceError::NotFound) => {
                self.diagnostics.push(Diagnostic {
                    severity: Severity::Warning,
                    file: root.display(None),
                    pos: None,
                    message: format!("no {MANIFEST}; the folder is not loaded as an extension"),
                });
                return;
            }
            Err(e) => {
                self.push_error(&file, None, format!("cannot read: {e}"));
                return;
            }
        };
        let (manifest, diagnostics) = parse_manifest(&source, &file);
        self.diagnostics.extend(diagnostics);
        let Some(manifest) = manifest else { return };

        let errors_before = self.error_count();
        if !manifest.engine.matches(engine) {
            self.push_error(
                &file,
                manifest.engine_pos,
                format!(
                    "requires Pitwall {} (engines.pitwall), this is {engine}",
                    manifest.engine
                ),
            );
        }
        if self.extensions.iter().any(|e| e.manifest.id == manifest.id) {
            self.push_error(
                &file,
                manifest.id_pos,
                format!("extension `{}` is already loaded", manifest.id),
            );
        }
        self.check_files(&root, &file, &manifest);
        self.check_unique_ids(&file, &manifest);
        let themes = self.read_themes(&root, &file, &manifest);
        let translations = self.read_translations(&root, &manifest);
        if self.error_count() == errors_before {
            self.extensions.push(Extension {
                manifest,
                root,
                themes,
                translations,
            });
        }
    }

    fn check_files(&mut self, root: &ExtensionRoot, file: &str, manifest: &Manifest) {
        let c = &manifest.contributes;
        // (경로, 종류, 레지스트리나 RPC 가 내용을 읽는가). 읽는 파일만 크기를 본다.
        let mut expected: Vec<(&PathRef, EntryKind, bool)> = Vec::new();
        expected.extend(c.themes.iter().map(|t| (&t.file, EntryKind::File, true)));
        expected.extend(c.keymaps.iter().map(|k| (&k.file, EntryKind::File, true)));
        for language in &c.languages {
            expected.extend(language.grammar.iter().map(|p| (p, EntryKind::File, false)));
            expected.extend(language.queries.iter().map(|p| (p, EntryKind::Dir, false)));
        }
        expected.extend(
            c.translations
                .iter()
                .map(|t| (&t.dir, EntryKind::Dir, false)),
        );
        expected.extend(
            manifest
                .wasm
                .iter()
                .map(|w| (&w.module, EntryKind::File, false)),
        );
        for (path_ref, want, read) in expected {
            let problem = match root.kind(&path_ref.path) {
                Ok(kind) if kind != want => match kind {
                    EntryKind::Dir => "is a folder, expected a file".to_owned(),
                    EntryKind::File => "is a file, expected a folder".to_owned(),
                },
                Ok(_) if !read => continue,
                Ok(_) => match root.size(&path_ref.path) {
                    Ok(size) if size <= MAX_CONTENT_BYTES => continue,
                    Ok(size) => format!("is {size} bytes, over the {MAX_CONTENT_BYTES}-byte limit"),
                    Err(e) => e.to_string(),
                },
                Err(e) => e.to_string(),
            };
            self.push_error(file, path_ref.pos, format!("`{}` {problem}", path_ref.path));
        }
    }

    fn check_unique_ids(&mut self, file: &str, manifest: &Manifest) {
        let c = &manifest.contributes;
        let mut taken = Vec::new();
        let mut check = |kind: &str, id: &str, pos, owner: Option<&str>| {
            if let Some(owner) = owner {
                taken.push((
                    pos,
                    format!("{kind} `{id}` is already provided by `{owner}`"),
                ));
            }
        };
        for t in &c.themes {
            check(
                "theme",
                &t.id,
                t.pos,
                self.theme(&t.id).map(|(e, _)| e.manifest.id.as_str()),
            );
        }
        for k in &c.keymaps {
            let owner = self
                .keymaps()
                .find(|(_, x)| x.id == k.id)
                .map(|(e, _)| e.manifest.id.as_str());
            check("keymap", &k.id, k.pos, owner);
        }
        for t in &c.external_tools {
            let owner = self
                .external_tools()
                .find(|(_, x)| x.id == t.id)
                .map(|(e, _)| e.manifest.id.as_str());
            check("external tool", &t.id, t.pos, owner);
        }
        for l in &c.languages {
            let owner = self
                .languages()
                .find(|(_, x)| x.id == l.id)
                .map(|(e, _)| e.manifest.id.as_str());
            check("language", &l.id, l.pos, owner);
        }
        // 언어 태그는 대소문자를 가리지 않는다(BCP 47) — `KO` 팩이 내장 `ko` 옆에 따로 등록되지 않게. 한 매니페스트 안에서
        // 대소문자만 다른 두 표(`ko` · `KO`)도 겹침이다(TOML 은 둘을 다른 키로 본다).
        for (i, t) in c.translations.iter().enumerate() {
            let owner = self
                .translations()
                .find(|(_, x)| x.locale.eq_ignore_ascii_case(&t.locale))
                .map(|(e, _)| e.manifest.id.as_str())
                .or_else(|| {
                    c.translations[..i]
                        .iter()
                        .any(|x| x.locale.eq_ignore_ascii_case(&t.locale))
                        .then_some(manifest.id.as_str())
                });
            check("translation", &t.locale, t.pos, owner);
        }
        for (pos, message) in taken {
            self.push_error(file, pos, message);
        }
    }

    fn read_themes(
        &mut self,
        root: &ExtensionRoot,
        file: &str,
        manifest: &Manifest,
    ) -> Vec<LoadedTheme> {
        let mut out = Vec::new();
        for decl in &manifest.contributes.themes {
            // 없는 파일은 check_files 가 이미 오류로 남겼다.
            let Ok(bytes) = root.read(&decl.file.path) else {
                continue;
            };
            match parse_theme_file(&bytes, &decl.id) {
                Ok(theme) => out.push(theme),
                Err(e) => self.push_error(
                    file,
                    decl.file.pos,
                    format!("theme file `{}`: {e}", decl.file.path),
                ),
            }
        }
        out
    }

    /// 언어 폴더 바로 아래의 `.ftl` 파일을 읽어 Fluent 문법을 검사한다(다른 파일 · 하위 폴더는 보지 않는다). 오류는 그 `.ftl`
    /// 파일의 줄 · 열로 남기고 확장 전체를 거절한다 — 잘못된 번역이 앱을 멈추지 않고 그 확장만 빠진다. 한 언어 안에서 같은
    /// 메시지 id 를 두 파일이 정의해도 오류다(어느 쪽이 이길지 파일 이름에 기대게 된다).
    fn read_translations(
        &mut self,
        root: &ExtensionRoot,
        manifest: &Manifest,
    ) -> Vec<LoadedTranslation> {
        let manifest_file = root.display(Some(&RelPath::parse(MANIFEST).expect("constant path")));
        let mut out = Vec::new();
        for decl in &manifest.contributes.translations {
            // 없는 폴더는 check_files 가 이미 오류로 남겼다.
            let Ok(paths) = root.files_in(&decl.dir.path) else {
                continue;
            };
            let paths: Vec<RelPath> = paths
                .into_iter()
                .filter(|p| p.as_str().ends_with(".ftl"))
                .collect();
            if paths.is_empty() {
                self.push_error(
                    &manifest_file,
                    decl.dir.pos,
                    format!(
                        "translation `{}`: `{}` has no .ftl files",
                        decl.locale, decl.dir.path
                    ),
                );
                continue;
            }
            let mut files = Vec::new();
            let mut defined: Vec<(String, String)> = Vec::new();
            for path in paths {
                let display = root.display(Some(&path));
                let source = match read_text(root, &path) {
                    Ok(source) => source,
                    Err(e) => {
                        self.push_error(&display, None, e);
                        continue;
                    }
                };
                match fluent_ids(&source) {
                    Ok(ids) => {
                        for id in ids {
                            if let Some((_, first)) = defined.iter().find(|(x, _)| *x == id) {
                                let message =
                                    format!("message `{id}` is already defined in {first}");
                                self.push_error(&display, None, message);
                            } else {
                                defined.push((id, display.clone()));
                            }
                        }
                    }
                    Err(errors) => {
                        for (pos, message) in errors {
                            self.push_error(&display, Some(pos_at(&source, pos)), message);
                        }
                        continue;
                    }
                }
                let name = path
                    .as_str()
                    .rsplit('/')
                    .next()
                    .unwrap_or_default()
                    .to_owned();
                files.push(TranslationFile {
                    display,
                    name,
                    source,
                });
            }
            out.push(LoadedTranslation {
                locale: decl.locale.clone(),
                files,
            });
        }
        out
    }

    fn push_error(&mut self, file: &str, pos: Option<Pos>, message: String) {
        self.diagnostics.push(Diagnostic {
            severity: Severity::Error,
            file: file.to_owned(),
            pos,
            message,
        });
    }

    fn error_count(&self) -> usize {
        self.diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .count()
    }
}

/// 크기 상한([`MAX_CONTENT_BYTES`])을 넘지 않는 UTF-8 텍스트 파일.
fn read_text(root: &ExtensionRoot, path: &RelPath) -> Result<String, String> {
    let size = root.size(path).map_err(|e| e.to_string())?;
    if size > MAX_CONTENT_BYTES {
        return Err(format!(
            "{size} bytes, over the {MAX_CONTENT_BYTES}-byte limit"
        ));
    }
    let bytes = root.read(path).map_err(|e| e.to_string())?;
    String::from_utf8(bytes.into_owned()).map_err(|_| "not UTF-8".to_owned())
}

/// Fluent 리소스가 정의한 메시지 · 용어 id(용어는 `-` 를 붙인다). 문법 오류가 있으면 (바이트 위치, 설명) 목록.
fn fluent_ids(source: &str) -> Result<Vec<String>, Vec<(usize, String)>> {
    use fluent_syntax::ast::Entry;
    match fluent_syntax::parser::parse(source) {
        Ok(resource) => Ok(resource
            .body
            .iter()
            .filter_map(|entry| match entry {
                Entry::Message(m) => Some(m.id.name.to_owned()),
                Entry::Term(t) => Some(format!("-{}", t.id.name)),
                _ => None,
            })
            .collect()),
        Err((_, errors)) => Err(errors
            .into_iter()
            .map(|e| (e.pos.start, format!("invalid Fluent: {}", e.kind)))
            .collect()),
    }
}

/// 바이트 위치의 줄 · 열(1부터, 열은 글자 수).
fn pos_at(source: &str, offset: usize) -> Pos {
    let mut end = offset.min(source.len());
    while !source.is_char_boundary(end) {
        end -= 1;
    }
    let before = &source[..end];
    let line = before.matches('\n').count() + 1;
    let column = before
        .rsplit('\n')
        .next()
        .unwrap_or_default()
        .chars()
        .count()
        + 1;
    Pos {
        line: u32::try_from(line).unwrap_or(u32::MAX),
        column: u32::try_from(column).unwrap_or(u32::MAX),
    }
}

/// 테마 파일의 겉모양만 본다: `id`(= 기여 이름) · `label` · `variants` 에 light · dark 중 하나 이상이 객체로.
fn parse_theme_file(bytes: &[u8], id: &str) -> Result<LoadedTheme, String> {
    let content: Value = serde_json::from_slice(bytes).map_err(|e| format!("invalid JSON: {e}"))?;
    let file_id = content.get("id").and_then(Value::as_str);
    if file_id != Some(id) {
        return Err(format!(
            "`id` must be `{id}` (the contribution name), found {file_id:?}"
        ));
    }
    let label = content
        .get("label")
        .and_then(Value::as_str)
        .ok_or("`label` must be a string")?
        .to_owned();
    let variants = content
        .get("variants")
        .and_then(Value::as_object)
        .ok_or("`variants` must be an object")?;
    let mut appearances = Vec::new();
    for appearance in Appearance::ALL {
        match variants.get(appearance.key()) {
            None => {}
            Some(Value::Object(_)) => appearances.push(appearance),
            Some(_) => return Err(format!("`variants.{}` must be an object", appearance.key())),
        }
    }
    if appearances.is_empty() {
        return Err("`variants` needs `light`, `dark` or both".to_owned());
    }
    Ok(LoadedTheme {
        id: id.to_owned(),
        label,
        appearances,
        content,
    })
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn engine() -> Version {
        Version::new(1, 2, 0)
    }

    /// `files` 를 임시 폴더에 쓴 확장 하나.
    fn ext(dir: &Path, folder: &str, files: &[(&str, &str)]) -> ExtensionRoot {
        let root = dir.join(folder);
        for (path, content) in files {
            let path = root.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, content).unwrap();
        }
        ExtensionRoot::Dir(root)
    }

    /// 진단에 나오는 파일 경로: 확장 폴더 + 매니페스트의 상대 경로([`ExtensionRoot::display`] 와 같은 꼴). Windows 에서는
    /// 확장 폴더 뒤만 `\` 이고 상대 경로 안은 `/` 다 — `dir.join("x/a/b")` 로 쓰면 `x` 뒤 구분자가 달라진다.
    fn shown(dir: &Path, folder: &str, rel: &str) -> PathBuf {
        dir.join(folder).join(rel)
    }

    fn manifest(id: &str, engine: &str, body: &str) -> String {
        format!(
            "id = \"{id}\"\nname = \"N\"\nversion = \"1.0.0\"\nengines = {{ pitwall = \"{engine}\" }}\n{body}"
        )
    }

    const DARK_ONLY: &str =
        r##"{"id": "dusk", "label": "Dusk", "variants": {"dark": {"ui.background": "#000"}}}"##;

    fn messages(registry: &Registry) -> Vec<String> {
        registry
            .diagnostics()
            .iter()
            .map(ToString::to_string)
            .collect()
    }

    #[test]
    fn loads_a_single_variant_theme_and_reports_its_appearance() {
        let dir = tempfile::tempdir().unwrap();
        let root = ext(
            dir.path(),
            "dusk",
            &[
                (
                    "extension.toml",
                    &manifest(
                        "dev.example.dusk",
                        ">=1.0",
                        "[contributes.themes.dusk]\nfile = \"themes/dusk.json\"\n",
                    ),
                ),
                ("themes/dusk.json", DARK_ONLY),
            ],
        );
        let registry = Registry::from_roots(&engine(), [root]);
        assert_eq!(messages(&registry), Vec::<String>::new());
        let (extension, theme) = registry.theme("dusk").unwrap();
        assert_eq!(extension.manifest.id, "dev.example.dusk");
        assert_eq!(theme.appearances, [Appearance::Dark]);
        assert_eq!(theme.label, "Dusk");
    }

    const KO_PACK: &str = "[contributes.translations.ko-kp]\ndir = \"i18n/ko-kp\"\n";

    #[test]
    fn loads_translation_files_in_name_order_and_skips_other_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = ext(
            dir.path(),
            "pack",
            &[
                (
                    "extension.toml",
                    &manifest("dev.example.pack", "*", KO_PACK),
                ),
                ("i18n/ko-kp/b.ftl", "b = 나\n"),
                ("i18n/ko-kp/a.ftl", "a = 가\n-brand = 핏월\n"),
                ("i18n/ko-kp/README.md", "not fluent"),
                ("i18n/ko-kp/nested/c.ftl", "c = 다\n"),
            ],
        );
        let registry = Registry::from_roots(&engine(), [root]);
        assert_eq!(messages(&registry), Vec::<String>::new());
        let (extension, translation) = registry.loaded_translations().next().unwrap();
        assert_eq!(extension.manifest.id, "dev.example.pack");
        assert_eq!(translation.locale, "ko-kp");
        let names: Vec<&str> = translation.files.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["a.ftl", "b.ftl"]);
        assert_eq!(translation.files[1].source, "b = 나\n");
    }

    /// 잘못된 .ftl 은 그 파일의 줄 · 열로 오류가 나고 그 확장만 빠진다 — 다른 확장과 앱은 그대로다.
    #[test]
    fn a_broken_ftl_file_rejects_only_its_extension() {
        let dir = tempfile::tempdir().unwrap();
        let broken = ext(
            dir.path(),
            "broken",
            &[
                (
                    "extension.toml",
                    &manifest("dev.example.broken", "*", KO_PACK),
                ),
                ("i18n/ko-kp/app.ftl", "ok = 좋다\n한글키 = 값\n"),
            ],
        );
        let fine = ext(
            dir.path(),
            "fine",
            &[
                (
                    "extension.toml",
                    &manifest(
                        "dev.example.fine",
                        "*",
                        "[contributes.translations.fr]\ndir = \"fr\"\n",
                    ),
                ),
                ("fr/app.ftl", "ok = bien\n"),
            ],
        );
        let registry = Registry::from_roots(&engine(), [broken, fine]);
        let ids: Vec<&str> = registry
            .extensions()
            .iter()
            .map(|e| e.manifest.id.as_str())
            .collect();
        assert_eq!(ids, ["dev.example.fine"]);
        let file = shown(dir.path(), "broken", "i18n/ko-kp/app.ftl");
        let messages = messages(&registry);
        assert_eq!(messages.len(), 1, "{messages:?}");
        assert!(
            messages[0].starts_with(&format!("{}:2:1: error: invalid Fluent: ", file.display())),
            "{messages:?}"
        );
    }

    #[test]
    fn rejects_a_message_defined_twice_in_one_language_and_an_empty_language_folder() {
        let dir = tempfile::tempdir().unwrap();
        let twice = ext(
            dir.path(),
            "twice",
            &[
                (
                    "extension.toml",
                    &manifest("dev.example.twice", "*", KO_PACK),
                ),
                ("i18n/ko-kp/a.ftl", "same = 하나\n"),
                ("i18n/ko-kp/b.ftl", "same = 둘\n"),
            ],
        );
        let empty = ext(
            dir.path(),
            "empty",
            &[
                (
                    "extension.toml",
                    &manifest(
                        "dev.example.empty",
                        "*",
                        "[contributes.translations.fr]\ndir = \"fr\"\n",
                    ),
                ),
                ("fr/notes.txt", ""),
            ],
        );
        let registry = Registry::from_roots(&engine(), [twice, empty]);
        assert!(registry.extensions().is_empty());
        let a = shown(dir.path(), "twice", "i18n/ko-kp/a.ftl");
        let b = shown(dir.path(), "twice", "i18n/ko-kp/b.ftl");
        let manifest_file = shown(dir.path(), "empty", "extension.toml");
        assert_eq!(
            messages(&registry),
            [
                format!(
                    "{}: error: message `same` is already defined in {}",
                    b.display(),
                    a.display()
                ),
                format!(
                    "{}:6:7: error: translation `fr`: `fr` has no .ftl files",
                    manifest_file.display()
                ),
            ]
        );
    }

    /// 대소문자만 다른 언어 태그도 같은 언어다 — 내장 `ko` 옆의 `KO` 팩, 한 매니페스트 안의 `fr` · `FR` 은 거절된다.
    #[test]
    fn language_tags_collide_regardless_of_case() {
        let dir = tempfile::tempdir().unwrap();
        let upper = ext(
            dir.path(),
            "upper",
            &[
                (
                    "extension.toml",
                    &manifest(
                        "dev.example.upper",
                        "*",
                        "[contributes.translations.KO]\ndir = \"ko\"\n",
                    ),
                ),
                ("ko/app.ftl", "menu-file = 대문자파일\n"),
            ],
        );
        let twice = ext(
            dir.path(),
            "twice",
            &[
                (
                    "extension.toml",
                    &manifest(
                        "dev.example.twice",
                        "*",
                        "[contributes.translations.fr]\ndir = \"fr\"\n\n[contributes.translations.FR]\ndir = \"fr\"\n",
                    ),
                ),
                ("fr/app.ftl", "menu-file = Fichier\n"),
            ],
        );
        let mut roots = builtin::roots();
        roots.extend([upper, twice]);
        let registry = Registry::from_roots(&engine(), roots);
        let locales: Vec<&str> = registry
            .loaded_translations()
            .map(|(_, t)| t.locale.as_str())
            .collect();
        assert_eq!(locales, ["en", "ko"]);
        let messages = messages(&registry);
        assert!(
            messages.iter().any(|m| m.ends_with(
                "error: translation `KO` is already provided by `dev.pitwall.i18n-default`"
            )),
            "{messages:?}"
        );
        assert!(
            messages.iter().any(|m| m
                .ends_with("error: translation `FR` is already provided by `dev.example.twice`")),
            "{messages:?}"
        );
    }

    /// 내장과 같은 언어 태그를 가진 언어 팩은 거절된다(07 「레지스트리」 표 「겹침」 행) — 먼저 등록된 내장이 이긴다.
    #[test]
    fn a_language_pack_cannot_replace_a_builtin_language() {
        let dir = tempfile::tempdir().unwrap();
        let pack = ext(
            dir.path(),
            "ko",
            &[
                (
                    "extension.toml",
                    &manifest(
                        "dev.example.ko",
                        "*",
                        "[contributes.translations.ko]\ndir = \"ko\"\n",
                    ),
                ),
                ("ko/app.ftl", "language-name = 가짜\n"),
            ],
        );
        let mut roots = builtin::roots();
        roots.push(pack);
        let registry = Registry::from_roots(&engine(), roots);
        let owner: Vec<&str> = registry
            .loaded_translations()
            .filter(|(_, t)| t.locale == "ko")
            .map(|(e, _)| e.manifest.id.as_str())
            .collect();
        assert_eq!(owner, ["dev.pitwall.i18n-default"]);
        let messages = messages(&registry);
        assert!(
            messages.iter().any(|m| m.ends_with(
                "error: translation `ko` is already provided by `dev.pitwall.i18n-default`"
            )),
            "{messages:?}"
        );
    }

    #[test]
    fn rejects_an_extension_for_another_pitwall_version() {
        let dir = tempfile::tempdir().unwrap();
        let root = ext(
            dir.path(),
            "next",
            &[("extension.toml", &manifest("dev.example.next", ">=2.0", ""))],
        );
        let registry = Registry::from_roots(&engine(), [root]);
        assert!(registry.extensions().is_empty());
        let file = shown(dir.path(), "next", "extension.toml");
        assert_eq!(
            messages(&registry),
            [format!(
                "{}:4:23: error: requires Pitwall >=2.0 (engines.pitwall), this is 1.2.0",
                file.display()
            )]
        );
    }

    #[test]
    fn accepts_an_extension_whose_range_covers_this_version() {
        let dir = tempfile::tempdir().unwrap();
        let root = ext(
            dir.path(),
            "ok",
            &[(
                "extension.toml",
                &manifest("dev.example.ok", ">=1.0, <2", ""),
            )],
        );
        let registry = Registry::from_roots(&engine(), [root]);
        assert_eq!(messages(&registry), Vec::<String>::new());
        assert_eq!(registry.extensions().len(), 1);
    }

    #[test]
    fn rejects_missing_files_and_wrong_kinds_with_the_manifest_position() {
        let dir = tempfile::tempdir().unwrap();
        let body = "[contributes.keymaps.vim]\nkind = \"bindings\"\nfile = \"keymaps/vim.toml\"\n\n[contributes.translations.ko]\ndir = \"i18n/ko.ftl\"\n";
        let root = ext(
            dir.path(),
            "broken",
            &[
                ("extension.toml", &manifest("dev.example.broken", "*", body)),
                ("i18n/ko.ftl", ""),
            ],
        );
        let registry = Registry::from_roots(&engine(), [root]);
        assert!(registry.extensions().is_empty());
        let file = shown(dir.path(), "broken", "extension.toml")
            .display()
            .to_string();
        assert_eq!(
            messages(&registry),
            [
                format!("{file}:7:8: error: `keymaps/vim.toml` not found"),
                format!("{file}:10:7: error: `i18n/ko.ftl` is a file, expected a folder"),
            ]
        );
    }

    #[cfg(unix)]
    #[test]
    fn rejects_a_theme_reached_through_a_symlink_out_of_the_folder() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("elsewhere")).unwrap();
        fs::write(dir.path().join("elsewhere/dusk.json"), DARK_ONLY).unwrap();
        let root = ext(
            dir.path(),
            "sneaky",
            &[(
                "extension.toml",
                &manifest(
                    "dev.example.sneaky",
                    "*",
                    "[contributes.themes.dusk]\nfile = \"themes/dusk.json\"\n",
                ),
            )],
        );
        std::os::unix::fs::symlink(
            dir.path().join("elsewhere"),
            dir.path().join("sneaky/themes"),
        )
        .unwrap();
        let registry = Registry::from_roots(&engine(), [root]);
        assert!(registry.extensions().is_empty());
        assert!(
            messages(&registry)[0]
                .ends_with("error: `themes/dusk.json` resolves outside the extension folder"),
            "{:?}",
            messages(&registry)
        );
    }

    #[test]
    fn keeps_the_first_extension_when_ids_collide() {
        let dir = tempfile::tempdir().unwrap();
        let theme = "[contributes.themes.dusk]\nfile = \"dusk.json\"\n";
        let first = ext(
            dir.path(),
            "a",
            &[
                ("extension.toml", &manifest("dev.example.a", "*", theme)),
                ("dusk.json", DARK_ONLY),
            ],
        );
        let same_id = ext(
            dir.path(),
            "b",
            &[("extension.toml", &manifest("dev.example.a", "*", ""))],
        );
        let same_theme = ext(
            dir.path(),
            "c",
            &[
                ("extension.toml", &manifest("dev.example.c", "*", theme)),
                ("dusk.json", DARK_ONLY),
            ],
        );
        let registry = Registry::from_roots(&engine(), [first, same_id, same_theme]);
        let ids: Vec<&str> = registry
            .extensions()
            .iter()
            .map(|e| e.manifest.id.as_str())
            .collect();
        assert_eq!(ids, ["dev.example.a"]);
        let messages = messages(&registry);
        assert!(
            messages[0].ends_with("1:6: error: extension `dev.example.a` is already loaded"),
            "{messages:?}"
        );
        assert!(
            messages[1]
                .ends_with("5:21: error: theme `dusk` is already provided by `dev.example.a`"),
            "{messages:?}"
        );
    }

    /// 키맵 · 외부 도구 · 언어 · UI 번역도 먼저 등록된 것을 지킨다. 각 겹침은 그 기여 하나만 겹치는 확장으로 따로 본다.
    #[test]
    fn keeps_the_first_keymap_tool_language_and_translation() {
        let dir = tempfile::tempdir().unwrap();
        let body = "[contributes.keymaps.vim]\nkind = \"bindings\"\nfile = \"vim.toml\"\n\n\
                    [contributes.external_tools.zed]\nlabel = \"Zed\"\ncommand = [\"zed\", \"{project_root}\"]\n\n\
                    [contributes.languages.kotlin]\nlinguist = \"Kotlin\"\n\n\
                    [contributes.translations.ko]\ndir = \"i18n/ko\"\n";
        let first = ext(
            dir.path(),
            "a",
            &[
                ("extension.toml", &manifest("dev.example.a", "*", body)),
                ("vim.toml", ""),
                ("i18n/ko/main.ftl", ""),
            ],
        );
        let cases = [
            (
                "keymap",
                "[contributes.keymaps.vim]\nkind = \"base\"\nfile = \"vim.toml\"\n",
            ),
            (
                "external tool",
                "[contributes.external_tools.zed]\nlabel = \"Z\"\ncommand = [\"z\"]\n",
            ),
            (
                "language",
                "[contributes.languages.kotlin]\nlinguist = \"Kotlin\"\n",
            ),
            (
                "translation",
                "[contributes.translations.ko]\ndir = \"i18n/ko\"\n",
            ),
        ];
        let mut roots = vec![first];
        for (i, (_, body)) in cases.iter().enumerate() {
            roots.push(ext(
                dir.path(),
                &format!("b{i}"),
                &[
                    (
                        "extension.toml",
                        &manifest(&format!("dev.example.b{i}"), "*", body),
                    ),
                    ("vim.toml", ""),
                    ("i18n/ko/main.ftl", ""),
                ],
            ));
        }
        let registry = Registry::from_roots(&engine(), roots);
        let ids: Vec<&str> = registry
            .extensions()
            .iter()
            .map(|e| e.manifest.id.as_str())
            .collect();
        assert_eq!(ids, ["dev.example.a"]);
        let messages = messages(&registry);
        assert_eq!(messages.len(), cases.len(), "{messages:?}");
        for ((kind, _), message) in cases.iter().zip(&messages) {
            assert!(
                message.contains(&format!("error: {kind} `"))
                    && message.ends_with("is already provided by `dev.example.a`"),
                "{message}"
            );
        }
    }

    #[test]
    fn rejects_a_theme_or_keymap_file_over_the_size_limit() {
        let dir = tempfile::tempdir().unwrap();
        let big = format!(
            r#"{{"id": "dusk", "label": "Dusk", "variants": {{"dark": {{}}}}, "pad": "{}"}}"#,
            "x".repeat(MAX_CONTENT_BYTES as usize)
        );
        let body = "[contributes.themes.dusk]\nfile = \"dusk.json\"\n\n[contributes.keymaps.vim]\nkind = \"base\"\nfile = \"vim.toml\"\n";
        let root = ext(
            dir.path(),
            "big",
            &[
                ("extension.toml", &manifest("dev.example.big", "*", body)),
                ("dusk.json", &big),
                ("vim.toml", &"#".repeat(MAX_CONTENT_BYTES as usize + 1)),
            ],
        );
        let registry = Registry::from_roots(&engine(), [root]);
        assert!(registry.extensions().is_empty());
        let messages = messages(&registry);
        assert_eq!(messages.len(), 2, "{messages:?}");
        assert!(messages[0].contains("`dusk.json` is"), "{messages:?}");
        assert!(
            messages[1].ends_with(&format!(
                "`vim.toml` is {} bytes, over the {MAX_CONTENT_BYTES}-byte limit",
                MAX_CONTENT_BYTES + 1
            )),
            "{messages:?}"
        );
    }

    #[test]
    fn accepts_a_content_file_at_the_size_limit() {
        let dir = tempfile::tempdir().unwrap();
        let body = "[contributes.keymaps.vim]\nkind = \"base\"\nfile = \"vim.toml\"\n";
        let root = ext(
            dir.path(),
            "edge",
            &[
                ("extension.toml", &manifest("dev.example.edge", "*", body)),
                ("vim.toml", &"#".repeat(MAX_CONTENT_BYTES as usize)),
            ],
        );
        let registry = Registry::from_roots(&engine(), [root]);
        assert_eq!(messages(&registry), Vec::<String>::new());
    }

    #[test]
    fn rejects_theme_files_with_a_wrong_shape() {
        let dir = tempfile::tempdir().unwrap();
        let cases = [
            (
                r#"{"id": "other", "label": "x", "variants": {"dark": {}}}"#,
                "`id` must be `dusk`",
            ),
            (
                r#"{"id": "dusk", "label": "x", "variants": {}}"#,
                "needs `light`, `dark` or both",
            ),
            (
                r#"{"id": "dusk", "label": "x", "variants": {"light": 1}}"#,
                "`variants.light` must be an object",
            ),
            ("not json", "invalid JSON"),
        ];
        for (i, (content, expected)) in cases.into_iter().enumerate() {
            let root = ext(
                dir.path(),
                &format!("t{i}"),
                &[
                    (
                        "extension.toml",
                        &manifest(
                            &format!("dev.example.t{i}"),
                            "*",
                            "[contributes.themes.dusk]\nfile = \"dusk.json\"\n",
                        ),
                    ),
                    ("dusk.json", content),
                ],
            );
            let registry = Registry::from_roots(&engine(), [root]);
            assert!(registry.extensions().is_empty());
            assert!(
                messages(&registry)[0].contains(expected),
                "{:?}",
                messages(&registry)
            );
        }
    }

    #[test]
    fn discovers_user_extensions_after_the_builtin_ones() {
        let dir = tempfile::tempdir().unwrap();
        ext(
            dir.path(),
            "dusk",
            &[
                ("extension.toml", &manifest("dev.example.dusk", "*", "")),
                ("unused.txt", ""),
            ],
        );
        ext(dir.path(), "not-an-extension", &[("README.md", "")]);
        ext(dir.path(), ".hidden", &[("extension.toml", "broken")]);
        let registry = Registry::load(&LoadOptions {
            engine: engine(),
            user_dir: Some(dir.path().to_owned()),
        });
        let ids: Vec<&str> = registry
            .extensions()
            .iter()
            .map(|e| e.manifest.id.as_str())
            .collect();
        assert!(ids.ends_with(&["dev.example.dusk"]), "{ids:?}");
        assert!(ids.contains(&"dev.pitwall.theme-default"), "{ids:?}");
        let warning = messages(&registry)
            .into_iter()
            .find(|m| m.contains("not-an-extension"))
            .unwrap();
        assert!(
            warning
                .ends_with("warning: no extension.toml; the folder is not loaded as an extension"),
            "{warning}"
        );
    }

    #[test]
    fn a_missing_user_folder_means_builtin_only() {
        let dir = tempfile::tempdir().unwrap();
        let registry = Registry::load(&LoadOptions {
            engine: engine(),
            user_dir: Some(dir.path().join("absent")),
        });
        assert!(registry.extensions().iter().all(|e| e.root.is_builtin()));
    }
}
