//! `extension.toml` 파서(`docs/design/07-extensibility.md` 「2. 확장 매니페스트」).
//!
//! 파일을 읽지 않는 순수 함수다. 매니페스트가 가리키는 파일이 있는지는 레지스트리가 본다(`registry.rs`).
//! 키 규칙과 모르는 키 · 기여 종류를 경고로 두는 이유는 `docs/design/07-extensibility.md` 「2. 확장 매니페스트」 「레지스트리 (`crates/pw-ext`)」 표.

use std::fmt;
use std::ops::Range;

use semver::{Version, VersionReq};
use toml_edit::{Document, Item, TableLike};

use crate::source::RelPath;

/// 파일 안의 위치. 줄 · 열은 1부터 센다(열은 문자 단위).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pos {
    pub line: u32,
    pub column: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

/// 매니페스트 · 확장 파일의 문제 하나. 오류가 하나라도 있는 확장은 등록하지 않는다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub severity: Severity,
    /// 문제가 있는 파일(`builtin:<폴더>/extension.toml` 또는 절대 경로).
    pub file: String,
    pub pos: Option<Pos>,
    pub message: String,
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let severity = match self.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
        };
        match self.pos {
            Some(Pos { line, column }) => write!(f, "{}:{line}:{column}: ", self.file)?,
            None => write!(f, "{}: ", self.file)?,
        }
        write!(f, "{severity}: {}", self.message)
    }
}

/// 매니페스트가 가리키는 파일 경로와 그 값이 적힌 자리.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathRef {
    pub path: RelPath,
    pub pos: Option<Pos>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    pub id: String,
    pub id_pos: Option<Pos>,
    pub name: String,
    pub version: Version,
    /// `engines.pitwall`.
    pub engine: VersionReq,
    pub engine_pos: Option<Pos>,
    pub authors: Vec<String>,
    pub description: Option<String>,
    pub license: Option<String>,
    pub contributes: Contributes,
    pub wasm: Option<Wasm>,
}

/// `[contributes.<종류>.<id>]`. 각 목록은 매니페스트에 적힌 순서다.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Contributes {
    pub themes: Vec<ThemeDecl>,
    pub keymaps: Vec<KeymapDecl>,
    pub languages: Vec<LanguageDecl>,
    pub external_tools: Vec<ExternalToolDecl>,
    pub translations: Vec<TranslationDecl>,
}

/// `[contributes.themes.<id>]`. 파일 형식은 07 「3. 테마」 「형식」 행 — `id` 는 표 이름과 같아야 한다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThemeDecl {
    pub id: String,
    pub pos: Option<Pos>,
    pub file: PathRef,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeymapKind {
    /// 전체 키맵. 기본 키맵 대신 하나를 고른다.
    Base,
    /// 베이스 위에 병합하는 일부 바인딩.
    Bindings,
}

/// `[contributes.keymaps.<id>]`(`docs/design/10-ui.md` §4.6). 파일 내용은 키맵 엔진이 읽는다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeymapDecl {
    pub id: String,
    pub pos: Option<Pos>,
    pub kind: KeymapKind,
    pub file: PathRef,
}

/// `[contributes.languages.<id>]`(07 §2 · §4.3). 지금은 저장만 한다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LanguageDecl {
    pub id: String,
    pub pos: Option<Pos>,
    pub linguist: String,
    pub extensions: Vec<String>,
    pub grammar: Option<PathRef>,
    /// 폴더.
    pub queries: Option<PathRef>,
    pub line_comment: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Macos,
    Linux,
    Windows,
}

/// `[contributes.external_tools.<id>]`(`docs/design/04-features.md` F8-1 · F8-2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalToolDecl {
    pub id: String,
    pub pos: Option<Pos>,
    pub label: String,
    /// 명령 템플릿. 변수는 [`TEMPLATE_VARS`] 만 쓴다.
    pub command: String,
    /// 비어 있으면 모든 OS.
    pub platforms: Vec<Platform>,
}

/// `[contributes.translations.<locale>]`. `dir` 은 그 언어의 Fluent 리소스(`.ftl`)를 담은 폴더다 — 파일 이름과 형식은
/// i18n 기능이 정한다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranslationDecl {
    /// BCP 47 언어 태그(`ko`, `en-US`).
    pub locale: String,
    pub pos: Option<Pos>,
    pub dir: PathRef,
}

/// `[wasm]`. 읽기만 하고 실행하지 않는다(L2 런타임은 이후).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wasm {
    pub module: PathRef,
    pub capabilities: Vec<String>,
}

/// 외부 도구 명령 템플릿의 변수.
pub const TEMPLATE_VARS: [&str; 5] = ["project_root", "file", "line", "column", "branch"];

/// 매니페스트를 읽는다. 오류가 없을 때만 `Some` 이다. 경고는 어느 쪽이든 함께 돌려준다.
pub fn parse_manifest(source: &str, file: &str) -> (Option<Manifest>, Vec<Diagnostic>) {
    let mut p = Parser {
        source,
        file,
        diagnostics: Vec::new(),
    };
    let manifest = match Document::parse(source) {
        Ok(doc) => p.manifest(doc.as_table()),
        Err(e) => {
            let message = e.message().trim().to_owned();
            p.error(e.span(), format!("invalid TOML: {message}"));
            None
        }
    };
    let failed = p.diagnostics.iter().any(|d| d.severity == Severity::Error);
    (manifest.filter(|_| !failed), p.diagnostics)
}

struct Parser<'s> {
    source: &'s str,
    file: &'s str,
    diagnostics: Vec<Diagnostic>,
}

type Span = Option<Range<usize>>;

impl Parser<'_> {
    fn pos(&self, span: &Span) -> Option<Pos> {
        let start = span.as_ref()?.start.min(self.source.len());
        let before = &self.source[..start];
        let line = before.matches('\n').count() + 1;
        let line_start = before.rfind('\n').map_or(0, |i| i + 1);
        let column = before[line_start..].chars().count() + 1;
        Some(Pos {
            line: u32::try_from(line).unwrap_or(u32::MAX),
            column: u32::try_from(column).unwrap_or(u32::MAX),
        })
    }

    fn push(&mut self, severity: Severity, span: Span, message: String) {
        let pos = self.pos(&span);
        self.diagnostics.push(Diagnostic {
            severity,
            file: self.file.to_owned(),
            pos,
            message,
        });
    }

    fn error(&mut self, span: Span, message: String) {
        self.push(Severity::Error, span, message);
    }

    fn warn(&mut self, span: Span, message: String) {
        self.push(Severity::Warning, span, message);
    }

    fn manifest(&mut self, root: &dyn TableLike) -> Option<Manifest> {
        let mut f = Fields::new(root, Some(0..0), "the manifest");
        let id = f.string(self, "id", true);
        if let Some((id, span)) = &id
            && !is_extension_id(id)
        {
            self.error(
                span.clone(),
                format!(
                    "extension id `{id}` must be dot-separated lowercase segments like `dev.example.my-theme`"
                ),
            );
        }
        let name = f.string(self, "name", true);
        let version = f.string(self, "version", true).and_then(|(raw, span)| {
            Version::parse(&raw)
                .map_err(|e| self.error(span, format!("version `{raw}` is not semver: {e}")))
                .ok()
        });
        let engine = f.table(self, "engines", true).and_then(|(engines, span)| {
            let mut e = Fields::new(engines, span, "`engines`");
            let pitwall = e.string(self, "pitwall", true);
            e.finish(self);
            let (raw, span) = pitwall?;
            match VersionReq::parse(&raw) {
                Ok(req) => Some((req, span)),
                Err(e) => {
                    self.error(
                        span,
                        format!("`engines.pitwall` `{raw}` is not a version range: {e}"),
                    );
                    None
                }
            }
        });
        let authors = f.strings(self, "authors").unwrap_or_default();
        let description = f.string(self, "description", false).map(|(s, _)| s);
        let license = f.string(self, "license", false).map(|(s, _)| s);
        let contributes = match f.table(self, "contributes", false) {
            Some((table, span)) => self.contributes(table, span),
            None => Contributes::default(),
        };
        let wasm = f
            .table(self, "wasm", false)
            .and_then(|(table, span)| self.wasm(table, span));
        f.finish(self);

        let ((id, id_span), (name, _), version, (engine, engine_span)) =
            (id?, name?, version?, engine?);
        Some(Manifest {
            id_pos: self.pos(&id_span),
            id,
            name,
            version,
            engine_pos: self.pos(&engine_span),
            engine,
            authors,
            description,
            license,
            contributes,
            wasm,
        })
    }

    fn contributes(&mut self, table: &dyn TableLike, span: Span) -> Contributes {
        let mut out = Contributes::default();
        for (kind, item) in table.iter() {
            let kind_span = key_span(table, kind).or_else(|| span.clone());
            let Some(entries) = item.as_table_like() else {
                self.error(kind_span, format!("`contributes.{kind}` must be a table"));
                continue;
            };
            match kind {
                "themes" => {
                    for (id, entry, at) in self.entries(entries, kind, kind_span, is_segment) {
                        let mut f = Fields::new(entry, at.clone(), &format!("theme `{id}`"));
                        let file = f.path(self, "file", true);
                        f.finish(self);
                        if let Some(file) = file {
                            out.themes.push(ThemeDecl {
                                pos: self.pos(&at),
                                id,
                                file,
                            });
                        }
                    }
                }
                "keymaps" => {
                    for (id, entry, at) in self.entries(entries, kind, kind_span, is_segment) {
                        let mut f = Fields::new(entry, at.clone(), &format!("keymap `{id}`"));
                        let kind = f.string(self, "kind", true).and_then(|(raw, span)| {
                            match raw.as_str() {
                                "base" => Some(KeymapKind::Base),
                                "bindings" => Some(KeymapKind::Bindings),
                                _ => {
                                    self.error(
                                        span,
                                        format!(
                                            "keymap `{id}`: kind `{raw}` must be `base` or `bindings`"
                                        ),
                                    );
                                    None
                                }
                            }
                        });
                        let file = f.path(self, "file", true);
                        f.finish(self);
                        if let (Some(kind), Some(file)) = (kind, file) {
                            out.keymaps.push(KeymapDecl {
                                pos: self.pos(&at),
                                id,
                                kind,
                                file,
                            });
                        }
                    }
                }
                "languages" => {
                    for (id, entry, at) in self.entries(entries, kind, kind_span, is_segment) {
                        let mut f = Fields::new(entry, at.clone(), &format!("language `{id}`"));
                        let linguist = f.string(self, "linguist", true);
                        let extensions = f.strings(self, "extensions").unwrap_or_default();
                        let grammar = f.path(self, "grammar", false);
                        let queries = f.path(self, "queries", false);
                        let line_comment = f.string(self, "line_comment", false).map(|(s, _)| s);
                        f.finish(self);
                        if let Some((linguist, _)) = linguist {
                            out.languages.push(LanguageDecl {
                                pos: self.pos(&at),
                                id,
                                linguist,
                                extensions,
                                grammar,
                                queries,
                                line_comment,
                            });
                        }
                    }
                }
                "external_tools" => {
                    for (id, entry, at) in self.entries(entries, kind, kind_span, is_segment) {
                        let mut f =
                            Fields::new(entry, at.clone(), &format!("external tool `{id}`"));
                        let label = f.string(self, "label", true);
                        let command = f.string(self, "command", true);
                        if let Some((command, span)) = &command
                            && let Err(e) = check_template(command)
                        {
                            self.error(span.clone(), format!("external tool `{id}`: {e}"));
                        }
                        let platforms = f.platforms(self, &id);
                        f.finish(self);
                        if let (Some((label, _)), Some((command, _))) = (label, command) {
                            out.external_tools.push(ExternalToolDecl {
                                pos: self.pos(&at),
                                id,
                                label,
                                command,
                                platforms,
                            });
                        }
                    }
                }
                "translations" => {
                    for (locale, entry, at) in self.entries(entries, kind, kind_span, is_locale) {
                        let mut f =
                            Fields::new(entry, at.clone(), &format!("translation `{locale}`"));
                        let dir = f.path(self, "dir", true);
                        f.finish(self);
                        if let Some(dir) = dir {
                            out.translations.push(TranslationDecl {
                                pos: self.pos(&at),
                                locale,
                                dir,
                            });
                        }
                    }
                }
                _ => self.warn(
                    kind_span,
                    format!(
                        "unknown contribution kind `{kind}` is ignored (not supported by this Pitwall)"
                    ),
                ),
            }
        }
        out
    }

    /// `[contributes.<kind>.<id>]` 표들. 이름 규칙에 맞지 않거나 표가 아닌 것은 오류를 남기고 건너뛴다.
    fn entries<'t>(
        &mut self,
        table: &'t dyn TableLike,
        kind: &str,
        kind_span: Span,
        valid_name: fn(&str) -> bool,
    ) -> Vec<(String, &'t dyn TableLike, Span)> {
        let mut out = Vec::new();
        for (name, item) in table.iter() {
            let span = key_span(table, name)
                .or_else(|| item.span())
                .or_else(|| kind_span.clone());
            if !valid_name(name) {
                self.error(
                    span,
                    format!("`contributes.{kind}.{name}`: `{name}` is not a valid name"),
                );
                continue;
            }
            match item.as_table_like() {
                Some(entry) => out.push((name.to_owned(), entry, span)),
                None => self.error(span, format!("`contributes.{kind}.{name}` must be a table")),
            }
        }
        out
    }

    fn wasm(&mut self, table: &dyn TableLike, span: Span) -> Option<Wasm> {
        let mut f = Fields::new(table, span, "`wasm`");
        let module = f.path(self, "module", true);
        let capabilities = f.strings(self, "capabilities").unwrap_or_default();
        f.finish(self);
        Some(Wasm {
            module: module?,
            capabilities,
        })
    }
}

/// 표 하나에서 아는 키를 꺼낸다. [`Fields::finish`] 가 꺼내지 않은 키를 경고한다.
struct Fields<'t> {
    table: &'t dyn TableLike,
    /// 키가 없을 때 가리킬 자리(표 머리).
    at: Span,
    what: String,
    seen: Vec<&'static str>,
}

impl<'t> Fields<'t> {
    fn new(table: &'t dyn TableLike, at: Span, what: &str) -> Self {
        Self {
            table,
            at,
            what: what.to_owned(),
            seen: Vec::new(),
        }
    }

    fn item(&mut self, p: &mut Parser<'_>, key: &'static str, required: bool) -> Option<&'t Item> {
        self.seen.push(key);
        let item = self.table.get(key);
        if item.is_none() && required {
            p.error(self.at.clone(), format!("{}: missing `{key}`", self.what));
        }
        item
    }

    fn string(
        &mut self,
        p: &mut Parser<'_>,
        key: &'static str,
        required: bool,
    ) -> Option<(String, Span)> {
        let item = self.item(p, key, required)?;
        match item.as_str() {
            Some(s) if !s.trim().is_empty() => Some((s.to_owned(), item.span())),
            Some(_) => {
                p.error(item.span(), format!("{}: `{key}` is empty", self.what));
                None
            }
            None => {
                p.error(
                    item.span(),
                    format!("{}: `{key}` must be a string", self.what),
                );
                None
            }
        }
    }

    fn strings(&mut self, p: &mut Parser<'_>, key: &'static str) -> Option<Vec<String>> {
        let item = self.item(p, key, false)?;
        let strings = item.as_array().and_then(|array| {
            array
                .iter()
                .map(|v| v.as_str().map(str::to_owned))
                .collect()
        });
        if strings.is_none() {
            p.error(
                item.span(),
                format!("{}: `{key}` must be an array of strings", self.what),
            );
        }
        strings
    }

    fn table(
        &mut self,
        p: &mut Parser<'_>,
        key: &'static str,
        required: bool,
    ) -> Option<(&'t dyn TableLike, Span)> {
        let span = key_span(self.table, key);
        let item = self.item(p, key, required)?;
        let span = item.span().or(span).or_else(|| self.at.clone());
        match item.as_table_like() {
            Some(table) => Some((table, span)),
            None => {
                p.error(span, format!("{}: `{key}` must be a table", self.what));
                None
            }
        }
    }

    fn path(&mut self, p: &mut Parser<'_>, key: &'static str, required: bool) -> Option<PathRef> {
        let (raw, span) = self.string(p, key, required)?;
        match RelPath::parse(&raw) {
            Ok(path) => Some(PathRef {
                path,
                pos: p.pos(&span),
            }),
            Err(e) => {
                p.error(span, format!("{}: `{key}`: {e}", self.what));
                None
            }
        }
    }

    fn platforms(&mut self, p: &mut Parser<'_>, id: &str) -> Vec<Platform> {
        let Some(item) = self.item(p, "platforms", false) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for name in self.strings_of(p, item, "platforms") {
            match name.as_str() {
                "macos" => out.push(Platform::Macos),
                "linux" => out.push(Platform::Linux),
                "windows" => out.push(Platform::Windows),
                _ => p.error(
                    item.span(),
                    format!(
                        "external tool `{id}`: platform `{name}` must be `macos`, `linux` or `windows`"
                    ),
                ),
            }
        }
        out
    }

    fn strings_of(&self, p: &mut Parser<'_>, item: &Item, key: &str) -> Vec<String> {
        let strings: Option<Vec<String>> = item.as_array().and_then(|array| {
            array
                .iter()
                .map(|v| v.as_str().map(str::to_owned))
                .collect()
        });
        strings.unwrap_or_else(|| {
            p.error(
                item.span(),
                format!("{}: `{key}` must be an array of strings", self.what),
            );
            Vec::new()
        })
    }

    fn finish(self, p: &mut Parser<'_>) {
        for (key, item) in self.table.iter() {
            if !self.seen.contains(&key) {
                let span = key_span(self.table, key).or_else(|| item.span());
                p.warn(
                    span,
                    format!("{}: unknown key `{key}` is ignored", self.what),
                );
            }
        }
    }
}

fn key_span(table: &dyn TableLike, key: &str) -> Span {
    table.key(key).and_then(toml_edit::Key::span)
}

/// 이름 마디: 소문자 · 숫자 · `-`, `-` 로 시작하거나 끝나지 않는다.
fn is_segment(s: &str) -> bool {
    !s.is_empty()
        && !s.starts_with('-')
        && !s.ends_with('-')
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// `dev.pitwall.theme-default` 처럼 마디가 둘 이상.
fn is_extension_id(s: &str) -> bool {
    let segments: Vec<&str> = s.split('.').collect();
    segments.len() >= 2 && segments.iter().all(|s| is_segment(s))
}

/// BCP 47 언어 태그의 모양만 본다: 2~3자 언어 + `-` 로 이은 1~8자 영숫자 마디.
fn is_locale(s: &str) -> bool {
    let mut parts = s.split('-');
    let language = parts.next().unwrap_or_default();
    (2..=3).contains(&language.len())
        && language.bytes().all(|b| b.is_ascii_alphabetic())
        && parts.all(|p| (1..=8).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_alphanumeric()))
}

/// `{name}` 변수가 [`TEMPLATE_VARS`] 안에 있고 중괄호가 짝을 이루는지 본다.
fn check_template(command: &str) -> Result<(), String> {
    let mut rest = command;
    while let Some(i) = rest.find(['{', '}']) {
        if rest[i..].starts_with('}') {
            return Err(format!("command `{command}` has an unmatched `}}`"));
        }
        let after = &rest[i + 1..];
        let end = after
            .find(['{', '}'])
            .filter(|&j| after[j..].starts_with('}'))
            .ok_or_else(|| format!("command `{command}` has an unmatched `{{`"))?;
        let name = &after[..end];
        if !TEMPLATE_VARS.contains(&name) {
            return Err(format!(
                "command `{command}` uses unknown variable `{{{name}}}` (known: {})",
                TEMPLATE_VARS.map(|v| format!("{{{v}}}")).join(", ")
            ));
        }
        rest = &after[end + 1..];
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = "/ext/extension.toml";

    fn parse(source: &str) -> (Option<Manifest>, Vec<Diagnostic>) {
        parse_manifest(source, FILE)
    }

    fn errors(diagnostics: &[Diagnostic]) -> Vec<String> {
        diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .map(ToString::to_string)
            .collect()
    }

    const MINIMAL: &str = r#"
id = "dev.example.sample"
name = "Sample"
version = "0.3.0"
engines = { pitwall = ">=1.0" }
"#;

    #[test]
    fn reads_the_design_example_with_every_contribution_kind() {
        let source = format!(
            r#"{MINIMAL}authors = ["a", "b"]
license = "MIT"

[contributes.themes.dusk]
file = "themes/dusk.json"

[contributes.keymaps.vscode-style]
kind = "base"
file = "keymaps/vscode-style.toml"

[contributes.languages.kotlin]
linguist = "Kotlin"
extensions = ["kt", "kts"]
grammar = "grammars/kotlin.wasm"
queries = "queries/kotlin/"
line_comment = "//"

[contributes.external_tools.zed]
label = "Zed"
command = "zed {{project_root}} {{file}}:{{line}}:{{column}}"
platforms = ["macos", "linux"]

[contributes.translations.ko]
dir = "i18n/ko"

[wasm]
module = "plugin.wasm"
capabilities = ["fs:read:project"]
"#
        );
        let (manifest, diagnostics) = parse(&source);
        assert_eq!(diagnostics, vec![]);
        let m = manifest.unwrap();
        assert_eq!(m.id, "dev.example.sample");
        assert_eq!(m.version, Version::new(0, 3, 0));
        assert!(m.engine.matches(&Version::new(1, 2, 0)));
        assert_eq!(m.authors, ["a", "b"]);
        assert_eq!(
            m.contributes.themes[0].file.path.as_str(),
            "themes/dusk.json"
        );
        assert_eq!(m.contributes.keymaps[0].kind, KeymapKind::Base);
        assert_eq!(
            m.contributes.languages[0]
                .queries
                .as_ref()
                .unwrap()
                .path
                .as_str(),
            "queries/kotlin"
        );
        assert_eq!(
            m.contributes.external_tools[0].platforms,
            [Platform::Macos, Platform::Linux]
        );
        assert_eq!(m.contributes.translations[0].locale, "ko");
        assert_eq!(m.wasm.unwrap().module.path.as_str(), "plugin.wasm");
    }

    #[test]
    fn reports_errors_with_file_line_and_column() {
        let (manifest, diagnostics) = parse(
            "id = \"dev.example.sample\"\nname = \"Sample\"\nversion = \"one\"\nengines = { pitwall = \">=1.0\" }\n",
        );
        assert!(manifest.is_none());
        assert_eq!(
            errors(&diagnostics),
            [format!(
                "{FILE}:3:11: error: version `one` is not semver: unexpected character 'o' while parsing major version number"
            )]
        );
    }

    #[test]
    fn reports_invalid_toml_with_its_position() {
        let (manifest, diagnostics) = parse("id = \"dev.example.sample\"\nname = \n");
        assert!(manifest.is_none());
        let d = &diagnostics[0];
        assert_eq!(d.severity, Severity::Error);
        assert_eq!(d.pos.map(|p| p.line), Some(2), "{d}");
        assert!(d.message.starts_with("invalid TOML"), "{d}");
    }

    #[test]
    fn requires_id_name_version_and_engine() {
        let (manifest, diagnostics) = parse("authors = []\n");
        assert!(manifest.is_none());
        let errors = errors(&diagnostics);
        for key in ["id", "name", "version", "engines"] {
            assert!(
                errors
                    .iter()
                    .any(|e| e.ends_with(&format!("missing `{key}`"))),
                "{key}: {errors:?}"
            );
        }
    }

    #[test]
    fn rejects_a_bad_engine_range_and_id() {
        let (manifest, diagnostics) = parse(
            "id = \"Sample\"\nname = \"S\"\nversion = \"1.0.0\"\nengines = { pitwall = \"later\" }\n",
        );
        assert!(manifest.is_none());
        let errors = errors(&diagnostics);
        assert!(
            errors[0].starts_with(&format!("{FILE}:1:6: error: extension id `Sample`")),
            "{errors:?}"
        );
        assert!(
            errors[1].starts_with(&format!("{FILE}:4:23: error: `engines.pitwall` `later`")),
            "{errors:?}"
        );
    }

    #[test]
    fn warns_and_ignores_unknown_contribution_kinds_and_keys() {
        let source = format!(
            "{MINIMAL}homepage = \"https://example.dev\"\n\n[contributes.snippets.rust]\nfile = \"s.json\"\n\n[contributes.themes.dusk]\nfile = \"dusk.json\"\nfuture = 1\n"
        );
        let (manifest, diagnostics) = parse(&source);
        let manifest = manifest.expect("warnings do not reject the extension");
        assert_eq!(manifest.contributes.themes.len(), 1);
        let warnings: Vec<String> = diagnostics.iter().map(ToString::to_string).collect();
        assert_eq!(
            warnings,
            [
                format!(
                    "{FILE}:8:14: warning: unknown contribution kind `snippets` is ignored (not supported by this Pitwall)"
                ),
                format!("{FILE}:13:1: warning: theme `dusk`: unknown key `future` is ignored"),
                format!("{FILE}:6:1: warning: the manifest: unknown key `homepage` is ignored"),
            ]
        );
    }

    #[test]
    fn rejects_paths_that_leave_the_extension_folder() {
        for path in [
            "../outside.json",
            "themes/../../x.json",
            "/etc/x.json",
            "..\\\\x.json",
        ] {
            let source = format!("{MINIMAL}\n[contributes.themes.dusk]\nfile = \"{path}\"\n");
            let (manifest, diagnostics) = parse(&source);
            assert!(manifest.is_none(), "{path}");
            let errors = errors(&diagnostics);
            assert_eq!(errors.len(), 1, "{path}: {errors:?}");
            assert!(
                errors[0].starts_with(&format!("{FILE}:8:8: error: theme `dusk`: `file`")),
                "{errors:?}"
            );
        }
    }

    #[test]
    fn checks_keymap_kind_tool_templates_and_platforms() {
        let source = format!(
            r#"{MINIMAL}
[contributes.keymaps.vim]
kind = "overlay"
file = "vim.toml"

[contributes.external_tools.zed]
label = "Zed"
command = "zed {{root}}"

[contributes.external_tools.code]
label = "Code"
command = "code {{file"
platforms = ["beos"]
"#
        );
        let (manifest, diagnostics) = parse(&source);
        assert!(manifest.is_none());
        let errors = errors(&diagnostics);
        assert_eq!(errors.len(), 4, "{errors:?}");
        assert!(errors[0].contains("kind `overlay` must be `base` or `bindings`"));
        assert!(errors[1].contains("unknown variable `{root}`"));
        assert!(errors[2].contains("unmatched `{`"));
        assert!(errors[3].contains("platform `beos`"));
    }

    #[test]
    fn checks_contribution_names() {
        let source = format!(
            "{MINIMAL}\n[contributes.themes.Dusk]\nfile = \"d.json\"\n\n[contributes.translations.korean]\ndir = \"ko\"\n"
        );
        let (_, diagnostics) = parse(&source);
        let errors = errors(&diagnostics);
        assert_eq!(errors.len(), 2, "{errors:?}");
        assert!(errors[0].contains("`Dusk` is not a valid name"));
        assert!(errors[1].contains("`korean` is not a valid name"));
    }

    #[test]
    fn template_check_accepts_every_known_variable() {
        assert_eq!(
            check_template("t {project_root} {file}:{line}:{column} {branch}"),
            Ok(())
        );
        assert_eq!(check_template("open -a Terminal"), Ok(()));
        assert!(check_template("x }").is_err());
        assert!(check_template("x {a{file}}").is_err());
    }
}
