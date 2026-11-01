//! `extension.toml` 파서(`docs/design/07-extensibility.md` 「2. 확장 매니페스트」).
//!
//! 파일을 읽지 않는 순수 함수다. 매니페스트가 가리키는 파일이 있는지는 레지스트리가 본다(`registry.rs`).
//! 키 규칙과 모르는 키 · 기여 종류를 경고로 두는 이유는 `docs/design/07-extensibility.md` 「2. 확장 매니페스트」 「레지스트리 (`crates/pw-ext`)」 표.

use std::collections::BTreeMap;
use std::fmt;
use std::ops::Range;

use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;
use toml_edit::{Document, Item, TableLike, Value};

use crate::source::RelPath;
use crate::template::{CommandKind, check_command};

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
    pub language_servers: Vec<LanguageServerDecl>,
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

/// `[contributes.languages.<id>]`(07 §2 · §4.3). 확장자는 언어 판별(`pw-lang`)이, 문법 · 쿼리는 구문 분석 서비스(`pw-syntax`)가
/// 처음 쓸 때 읽는다(레지스트리는 있는지만 본다).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LanguageDecl {
    pub id: String,
    pub pos: Option<Pos>,
    pub linguist: String,
    pub extensions: Vec<String>,
    /// 앱에 링크된 tree-sitter 문법의 이름(`pw-syntax` 의 내장 문법). 내장 확장만 쓴다.
    pub builtin_grammar: Option<NameRef>,
    /// tree-sitter 문법 WASM(외부 확장, M8).
    pub grammar: Option<PathRef>,
    /// 폴더.
    pub queries: Option<PathRef>,
    pub line_comment: Option<String>,
}

/// 이름 하나와 그 값의 위치.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NameRef {
    pub name: String,
    pub pos: Option<Pos>,
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
    /// 프로젝트를 여는 명령(argv 템플릿). 규칙은 [`crate::template`].
    pub command: Vec<String>,
    /// 파일까지 여는 명령(선택). 파일 문맥이 있고 이것이 있으면 `command` 대신 쓴다.
    pub file_command: Option<Vec<String>>,
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

/// `[contributes.language_servers.<id>]` — 언어 서버 정의(`docs/design/07-extensibility.md` §2, `docs/design/15-m4-plan.md`
/// M4-02). 어디서 실행 파일을 찾는지는 해석이 정한다(`crates/pw-lsp-servers`). 내장 확장의 것만 등록한다(레지스트리).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LanguageServerDecl {
    pub id: String,
    pub pos: Option<Pos>,
    /// 이 서버가 맡는 언어 id(`[contributes.languages.<id>]` — 이 확장이나 다른 확장의 것). 하나 이상이다.
    pub languages: Vec<NameRef>,
    /// Mason registry 패키지 이름(07 §4.4). 설치본(`$DATA/tools`)을 찾고 설치를 제안할 때 쓴다.
    pub mason: Option<String>,
    /// 실행 파일 이름 하나. 경로 구분자 · 공백이 없다 — 셸을 거치지 않고, 인자는 `args` 로 따로 준다.
    pub command: String,
    pub args: Vec<String>,
    /// 서버 프로세스에 더할 환경 변수(런타임 환경 위에 덮는다).
    pub env: BTreeMap<String, String>,
    /// 작업 공간 루트를 찾을 때 볼 파일 이름 글롭(`Cargo.toml`). 비면 프로젝트 루트다.
    pub root_markers: Vec<String>,
    /// `initialize` 의 `initializationOptions`(TOML 표를 JSON 으로).
    pub initialization_options: Option<Json>,
    /// `workspace/configuration` 의 답의 바탕(TOML 표를 JSON 으로). 사람의 `[lsp.<id>] settings` 가 그 위에 깊게 합쳐진다.
    pub settings: Option<Json>,
    /// 이 서버에 맡기지 않는 기능(`features.disable`, 결정 M4-D12).
    pub disabled_features: Vec<LspFeature>,
    /// 서버를 **돌리는** 런타임과 판 범위(결정 M4-D6). 프로젝트가 쓰는 런타임과 따로다.
    pub requires: Vec<RuntimeRequirement>,
    /// 실행 파일 해석의 특례(결정 M4-D7).
    pub resolve_hint: Option<ResolveHint>,
    /// 프로젝트 안의 실행 파일(프로젝트 루트 기준 상대 경로). 신뢰한 프로젝트에서만 본다.
    pub project_local: Option<RelPath>,
    /// 프로젝트 안에 있으면 `initialization_options` 에 그 절대 경로를 넣을 값: (`.` 으로 나눈 JSON 키 경로, 프로젝트 루트 기준
    /// 상대 경로). 신뢰한 프로젝트에서만 본다 — TS 의 `tsserver.path`(결정 M4-D7). 적힌 순서다.
    pub project_local_options: Vec<(String, RelPath)>,
}

/// 서버에 맡기는 기능의 이름(`features.disable` 의 값). 서버 관리자의 기능 라우팅(결정 M4-D12)이 쓴다.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    specta::Type,
    schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum LspFeature {
    Diagnostics,
    Hover,
    Definition,
    Implementation,
    References,
    DocumentSymbol,
    WorkspaceSymbol,
    Completion,
    SignatureHelp,
    Rename,
    CodeAction,
    Formatting,
}

impl LspFeature {
    pub const ALL: [LspFeature; 12] = [
        LspFeature::Diagnostics,
        LspFeature::Hover,
        LspFeature::Definition,
        LspFeature::Implementation,
        LspFeature::References,
        LspFeature::DocumentSymbol,
        LspFeature::WorkspaceSymbol,
        LspFeature::Completion,
        LspFeature::SignatureHelp,
        LspFeature::Rename,
        LspFeature::CodeAction,
        LspFeature::Formatting,
    ];

    /// 매니페스트 · 설정에 적는 이름.
    pub fn key(self) -> &'static str {
        match self {
            LspFeature::Diagnostics => "diagnostics",
            LspFeature::Hover => "hover",
            LspFeature::Definition => "definition",
            LspFeature::Implementation => "implementation",
            LspFeature::References => "references",
            LspFeature::DocumentSymbol => "document_symbol",
            LspFeature::WorkspaceSymbol => "workspace_symbol",
            LspFeature::Completion => "completion",
            LspFeature::SignatureHelp => "signature_help",
            LspFeature::Rename => "rename",
            LspFeature::CodeAction => "code_action",
            LspFeature::Formatting => "formatting",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|f| f.key() == key)
    }
}

/// 서버를 돌리는 런타임. 서버 관리자가 결정 M4-D6 의 순서로 찾는다 — Pitwall 은 런타임을 설치하지 않는다(D26).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum LspServerRuntime {
    Node,
    Python,
    Java,
}

impl LspServerRuntime {
    pub fn key(self) -> &'static str {
        match self {
            LspServerRuntime::Node => "node",
            LspServerRuntime::Python => "python",
            LspServerRuntime::Java => "java",
        }
    }

    fn from_key(key: &str) -> Option<Self> {
        [
            LspServerRuntime::Node,
            LspServerRuntime::Python,
            LspServerRuntime::Java,
        ]
        .into_iter()
        .find(|r| r.key() == key)
    }
}

/// `requires = { java = ">=21" }` 의 한 칸.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeRequirement {
    pub runtime: LspServerRuntime,
    pub version: VersionReq,
}

/// 실행 파일 해석의 특례(`resolve_hint`).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum ResolveHint {
    /// PATH 에서 찾은 것이 rustup 프록시면 그 디렉터리에서 `rustup which <command>` 가 성공할 때만 쓴다(결정 M4-D7).
    Rustup,
}

/// `[wasm]`. 읽기만 하고 실행하지 않는다(L2 런타임은 이후).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wasm {
    pub module: PathRef,
    pub capabilities: Vec<String>,
}

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

    /// 이미 줄 · 열로 바꾼 자리에 남긴다.
    fn at(&mut self, severity: Severity, pos: Option<Pos>, message: String) {
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
                        let builtin_grammar =
                            f.string(self, "builtin_grammar", false)
                                .map(|(name, span)| NameRef {
                                    name,
                                    pos: self.pos(&span),
                                });
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
                                builtin_grammar,
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
                        let command = f.command(self, &id, CommandKind::Project, true);
                        let file_command = f.command(self, &id, CommandKind::File, false);
                        let platforms = f.platforms(self, &id);
                        f.finish(self);
                        if let (Some((label, _)), Some(command)) = (label, command) {
                            out.external_tools.push(ExternalToolDecl {
                                pos: self.pos(&at),
                                id,
                                label,
                                command,
                                file_command,
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
                "language_servers" => {
                    for (id, entry, at) in self.entries(entries, kind, kind_span, is_segment) {
                        if let Some(decl) = self.language_server(id, entry, at) {
                            out.language_servers.push(decl);
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

    /// `[contributes.language_servers.<id>]` 하나. 형식 오류는 오류로, 모르는 키 · 기능 이름은 경고로 남긴다.
    fn language_server(
        &mut self,
        id: String,
        entry: &dyn TableLike,
        at: Span,
    ) -> Option<LanguageServerDecl> {
        let what = format!("language server `{id}`");
        let mut f = Fields::new(entry, at.clone(), &what);
        let languages = f.item(self, "languages", true).and_then(|item| {
            let names = self.name_list(item, &what, "languages");
            if names.as_ref().is_some_and(Vec::is_empty) {
                self.error(item.span(), format!("{what}: `languages` is empty"));
                return None;
            }
            names
        });
        let mason = f.string(self, "mason", false).and_then(|(raw, span)| {
            if is_package_name(&raw) {
                Some(raw)
            } else {
                self.error(
                    span,
                    format!(
                        "{what}: `mason` `{raw}` must be a registry package name (lowercase letters, digits, `.`, `_`, `-`)"
                    ),
                );
                None
            }
        });
        let command = f.item(self, "command", true).and_then(|item| {
            let Some(raw) = item.as_str() else {
                self.error(
                    item.span(),
                    format!(
                        "{what}: `command` must be one executable name like \"rust-analyzer\" — put arguments in `args`"
                    ),
                );
                return None;
            };
            match check_executable_name(raw) {
                Ok(()) => Some(raw.to_owned()),
                Err(e) => {
                    self.error(item.span(), format!("{what}: `command` {e}"));
                    None
                }
            }
        });
        let args = f.strings(self, "args").unwrap_or_default();
        let env = f
            .table(self, "env", false)
            .map(|(table, _)| self.env(table, &what))
            .unwrap_or_default();
        let root_markers = f
            .item(self, "root_markers", false)
            .and_then(|item| self.globs(item, &what))
            .unwrap_or_default();
        let initialization_options = f
            .table(self, "initialization_options", false)
            .and_then(|(table, span)| self.json_table(table, span, &what));
        let settings = f
            .table(self, "settings", false)
            .and_then(|(table, span)| self.json_table(table, span, &what));
        let disabled_features = f
            .table(self, "features", false)
            .map(|(table, span)| self.features(table, span, &what))
            .unwrap_or_default();
        let requires = f
            .table(self, "requires", false)
            .map(|(table, _)| self.requires(table, &what))
            .unwrap_or_default();
        let resolve_hint = f
            .string(self, "resolve_hint", false)
            .and_then(|(raw, span)| match raw.as_str() {
                "rustup" => Some(ResolveHint::Rustup),
                _ => {
                    self.error(
                        span,
                        format!("{what}: `resolve_hint` `{raw}` must be `rustup`"),
                    );
                    None
                }
            });
        let project_local = f
            .string(self, "project_local", false)
            .and_then(|(raw, span)| {
                RelPath::parse(&raw)
                    .map_err(|e| {
                        self.error(
                            span,
                            format!(
                                "{what}: `project_local`: {}",
                                e.replace("extension folder", "project")
                            ),
                        );
                    })
                    .ok()
            });
        let project_local_options = f
            .table(self, "project_local_options", false)
            .map(|(table, _)| self.project_local_options(table, &what))
            .unwrap_or_default();
        f.finish(self);
        Some(LanguageServerDecl {
            pos: self.pos(&at),
            id,
            languages: languages?,
            mason,
            command: command?,
            args,
            env,
            root_markers,
            initialization_options,
            settings,
            disabled_features,
            requires,
            resolve_hint,
            project_local,
            project_local_options,
        })
    }

    /// 문자열 배열을 그 자리(배열이면 각 요소의 자리)와 함께.
    fn name_list(&mut self, item: &Item, what: &str, key: &str) -> Option<Vec<NameRef>> {
        let array = item.as_array();
        let names: Option<Vec<NameRef>> = array.and_then(|array| {
            array
                .iter()
                .map(|v| {
                    v.as_str().map(|s| NameRef {
                        name: s.to_owned(),
                        pos: self.pos(&v.span()),
                    })
                })
                .collect()
        });
        if names.is_none() {
            self.error(
                item.span(),
                format!("{what}: `{key}` must be an array of strings"),
            );
        }
        names
    }

    fn env(&mut self, table: &dyn TableLike, what: &str) -> BTreeMap<String, String> {
        let mut out = BTreeMap::new();
        for (name, item) in table.iter() {
            let span = key_span(table, name).or_else(|| item.span());
            if name.is_empty() || name.contains('=') || name.contains('\0') {
                self.error(
                    span,
                    format!("{what}: `env`: `{name}` is not a variable name"),
                );
                continue;
            }
            match item.as_str() {
                Some(value) if !value.contains('\0') => {
                    out.insert(name.to_owned(), value.to_owned());
                }
                _ => self.error(
                    item.span().or(span),
                    format!("{what}: `env.{name}` must be a string"),
                ),
            }
        }
        out
    }

    fn globs(&mut self, item: &Item, what: &str) -> Option<Vec<String>> {
        let names = self.name_list(item, what, "root_markers")?;
        let mut out = Vec::new();
        for name in names {
            let problem = if name.name.trim().is_empty() {
                Some("is empty".to_owned())
            } else if name.name.contains('/') || name.name.contains('\\') {
                Some("must be a file name pattern, not a path".to_owned())
            } else {
                globset::Glob::new(&name.name)
                    .err()
                    .map(|e| format!("is not a glob: {}", e.kind()))
            };
            match problem {
                Some(problem) => self.at(
                    Severity::Error,
                    name.pos,
                    format!("{what}: root marker `{}` {problem}", name.name),
                ),
                None => out.push(name.name),
            }
        }
        Some(out)
    }

    fn json_table(&mut self, table: &dyn TableLike, span: Span, what: &str) -> Option<Json> {
        match table_to_json(table) {
            Ok(value) => Some(value),
            Err(e) => {
                self.error(span, format!("{what}: {e}"));
                None
            }
        }
    }

    fn features(&mut self, table: &dyn TableLike, span: Span, what: &str) -> Vec<LspFeature> {
        let mut f = Fields::new(table, span, &format!("{what}: `features`"));
        let disable = f
            .item(self, "disable", false)
            .and_then(|item| self.name_list(item, what, "features.disable"));
        f.finish(self);
        let mut out = Vec::new();
        for name in disable.unwrap_or_default() {
            match LspFeature::from_key(&name.name) {
                Some(feature) if !out.contains(&feature) => out.push(feature),
                Some(_) => {}
                None => self.at(
                    Severity::Warning,
                    name.pos,
                    format!(
                        "{what}: unknown feature `{}` in `features.disable` is ignored",
                        name.name
                    ),
                ),
            }
        }
        out
    }

    fn requires(&mut self, table: &dyn TableLike, what: &str) -> Vec<RuntimeRequirement> {
        let mut out = Vec::new();
        for (name, item) in table.iter() {
            let span = key_span(table, name).or_else(|| item.span());
            let Some(runtime) = LspServerRuntime::from_key(name) else {
                self.error(
                    span,
                    format!(
                        "{what}: `requires.{name}`: runtime must be `node`, `python` or `java`"
                    ),
                );
                continue;
            };
            let Some(raw) = item.as_str() else {
                self.error(
                    item.span().or(span),
                    format!("{what}: `requires.{name}` must be a version range like \">=21\""),
                );
                continue;
            };
            match VersionReq::parse(raw) {
                Ok(version) => out.push(RuntimeRequirement { runtime, version }),
                Err(e) => self.error(
                    item.span().or(span),
                    format!("{what}: `requires.{name}` `{raw}` is not a version range: {e}"),
                ),
            }
        }
        out
    }

    fn project_local_options(
        &mut self,
        table: &dyn TableLike,
        what: &str,
    ) -> Vec<(String, RelPath)> {
        let mut out = Vec::new();
        for (key, item) in table.iter() {
            let span = key_span(table, key).or_else(|| item.span());
            if key.split('.').any(str::is_empty) {
                self.error(
                    span,
                    format!("{what}: `project_local_options`: `{key}` must be dot-separated option names"),
                );
                continue;
            }
            let path = item.as_str().map(RelPath::parse);
            match path {
                Some(Ok(path)) => out.push((key.to_owned(), path)),
                Some(Err(e)) => self.error(
                    item.span().or(span),
                    format!(
                        "{what}: `project_local_options.{key}`: {}",
                        e.replace("extension folder", "project")
                    ),
                ),
                None => self.error(
                    item.span().or(span),
                    format!("{what}: `project_local_options.{key}` must be a path string"),
                ),
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

    /// 외부 도구 명령(`command` · `file_command`). 문자열 배열만 받는다 — 셸 문자열은 공백 경로가 인자를 깨고 파일 · 브랜치
    /// 이름으로 명령이 주입되므로 받지 않는다(규칙은 [`crate::template`]).
    fn command(
        &mut self,
        p: &mut Parser<'_>,
        id: &str,
        kind: CommandKind,
        required: bool,
    ) -> Option<Vec<String>> {
        let key = kind.key();
        let item = self.item(p, key, required)?;
        let argv: Option<Vec<String>> = item.as_array().and_then(|array| {
            array
                .iter()
                .map(|v| v.as_str().map(str::to_owned))
                .collect()
        });
        let Some(argv) = argv else {
            p.error(
                item.span(),
                format!(
                    "external tool `{id}`: `{key}` must be an array of strings like [\"zed\", \"{{project_root}}\"] — shell command strings are not accepted"
                ),
            );
            return None;
        };
        match check_command(kind, &argv) {
            Ok(()) => Some(argv),
            Err(e) => {
                p.error(item.span(), format!("external tool `{id}`: `{key}` {e}"));
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

/// Mason registry 패키지 이름: 소문자 · 숫자 · `.` · `_` · `-`, 소문자나 숫자로 시작하고 `..` 가 없다. 설치본 자리
/// (`$DATA/tools/<패키지>`)의 경로 마디가 되므로 폴더 밖으로 나가는 이름을 받지 않는다.
fn is_package_name(s: &str) -> bool {
    s.bytes()
        .next()
        .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        && !s.contains("..")
        && s.bytes().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b'-')
        })
}

/// 실행 파일 이름 하나인지 본다: 비지 않고, 경로 구분자 · 공백 · 제어 문자가 없고, `-` 로 시작하지 않는다.
fn check_executable_name(raw: &str) -> Result<(), String> {
    if raw.trim().is_empty() {
        return Err("is empty".to_owned());
    }
    if raw.contains('/') || raw.contains('\\') {
        return Err(format!(
            "`{raw}` must be an executable name, not a path — set a path with `[lsp.<id>] path` in settings"
        ));
    }
    if raw.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(format!(
            "`{raw}` must be one executable name — shell command strings are not accepted; put arguments in `args`"
        ));
    }
    if raw.starts_with('-') {
        return Err(format!("`{raw}` must not start with `-`"));
    }
    Ok(())
}

/// TOML 표를 JSON 객체로. 날짜 · 시각은 TOML 표기 그대로의 문자열이 된다. 유한하지 않은 실수는 JSON 에 없으므로 오류다.
pub(crate) fn table_to_json(table: &dyn TableLike) -> Result<Json, String> {
    let mut map = serde_json::Map::new();
    for (key, item) in table.iter() {
        map.insert(key.to_owned(), item_to_json(item, key)?);
    }
    Ok(Json::Object(map))
}

fn item_to_json(item: &Item, at: &str) -> Result<Json, String> {
    match item {
        Item::None => Ok(Json::Null),
        Item::Value(value) => value_to_json(value, at),
        Item::Table(table) => table_to_json(table),
        Item::ArrayOfTables(array) => array
            .iter()
            .map(|t| table_to_json(t))
            .collect::<Result<Vec<_>, _>>()
            .map(Json::Array),
    }
}

fn value_to_json(value: &Value, at: &str) -> Result<Json, String> {
    Ok(match value {
        Value::String(s) => Json::String(s.value().clone()),
        Value::Integer(i) => Json::from(*i.value()),
        Value::Float(f) => serde_json::Number::from_f64(*f.value())
            .map(Json::Number)
            .ok_or_else(|| format!("`{at}`: {} has no JSON form", f.value()))?,
        Value::Boolean(b) => Json::Bool(*b.value()),
        Value::Datetime(d) => Json::String(d.value().to_string()),
        Value::Array(array) => Json::Array(
            array
                .iter()
                .map(|v| value_to_json(v, at))
                .collect::<Result<_, _>>()?,
        ),
        Value::InlineTable(table) => table_to_json(table)?,
    })
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
command = ["zed", "{{project_root}}"]
file_command = ["zed", "{{project_root}}", "{{file}}:{{line}}:{{column}}"]
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
        let zed = &m.contributes.external_tools[0];
        assert_eq!(zed.command, ["zed", "{project_root}"]);
        assert_eq!(
            zed.file_command.as_deref(),
            Some(&["zed", "{project_root}", "{file}:{line}:{column}"].map(String::from)[..])
        );
        assert_eq!(zed.platforms, [Platform::Macos, Platform::Linux]);
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
command = ["zed", "{{root}}"]

[contributes.external_tools.code]
label = "Code"
command = ["code", "{{file"]
platforms = ["beos"]

[contributes.external_tools.idea]
label = "IDEA"
command = ["{{project_root}}/idea"]
file_command = ["idea", "{{project_root}}"]
"#
        );
        let (manifest, diagnostics) = parse(&source);
        assert!(manifest.is_none());
        let errors = errors(&diagnostics);
        assert_eq!(errors.len(), 6, "{errors:?}");
        assert!(errors[0].contains("kind `overlay` must be `base` or `bindings`"));
        assert!(errors[1].contains("unknown variable `{root}`"));
        assert!(errors[2].contains("unmatched `{`"));
        assert!(errors[3].contains("platform `beos`"));
        assert!(
            errors[4].contains("`command` program `{project_root}/idea` cannot use variables"),
            "{errors:?}"
        );
        assert!(
            errors[5].contains("`file_command` does not use `{file}`"),
            "{errors:?}"
        );
    }

    /// 셸 문자열 명령은 받지 않는다 — 형식 오류로 진단한다(공백 경로가 인자를 깨고 셸로 돌리면 이름으로 명령이 주입된다).
    #[test]
    fn rejects_shell_string_commands() {
        let source = format!(
            "{MINIMAL}\n[contributes.external_tools.zed]\nlabel = \"Zed\"\ncommand = \"zed {{project_root}}\"\n"
        );
        let (manifest, diagnostics) = parse(&source);
        assert!(manifest.is_none());
        assert_eq!(
            errors(&diagnostics),
            [format!(
                "{FILE}:9:11: error: external tool `zed`: `command` must be an array of strings like [\"zed\", \"{{project_root}}\"] — shell command strings are not accepted"
            )]
        );
    }

    /// 파일 변수는 `file_command` 에만 쓴다 — 파일이 없는 문맥(프로젝트 창)에서도 `command` 는 늘 치환된다.
    #[test]
    fn keeps_file_variables_out_of_the_project_command() {
        let source = format!(
            "{MINIMAL}\n[contributes.external_tools.zed]\nlabel = \"Zed\"\ncommand = [\"zed\", \"{{file}}:{{line}}\"]\n"
        );
        let (manifest, diagnostics) = parse(&source);
        assert!(manifest.is_none());
        let errors = errors(&diagnostics);
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(
            errors[0].contains("uses `{file}`, which only `file_command` can use"),
            "{errors:?}"
        );
    }

    const SERVER: &str = r#"
[contributes.language_servers.ts]
languages = ["typescript", "tsx"]
mason = "typescript-language-server"
command = "typescript-language-server"
args = ["--stdio"]
env = { TSS_LOG = "-level verbose" }
root_markers = ["tsconfig.json", "*.code-workspace"]
initialization_options = { preferences = { quoteStyle = "single" }, maxTsServerMemory = 4096 }
features = { disable = ["formatting", "rename", "formatting"] }
requires = { node = ">=20", python = ">=3.10" }
resolve_hint = "rustup"
project_local = "node_modules/.bin/typescript-language-server"
project_local_options = { "tsserver.path" = "node_modules/typescript/lib" }

[contributes.language_servers.ts.settings]
typescript = { inlayHints = { enabled = true }, ratio = 0.5, when = 1979-05-27 }
list = [1, "two", [false]]
"#;

    /// 언어 서버 정의의 키마다 읽은 값 — 표는 JSON 이 된다(`initialization_options` · `settings`).
    #[test]
    fn reads_a_language_server_with_every_key() {
        let (manifest, diagnostics) = parse(&format!("{MINIMAL}{SERVER}"));
        assert_eq!(diagnostics, vec![]);
        let m = manifest.unwrap();
        let s = &m.contributes.language_servers[0];
        assert_eq!(s.id, "ts");
        assert_eq!(
            s.pos,
            Some(Pos {
                line: 7,
                column: 31
            })
        );
        let languages: Vec<&str> = s.languages.iter().map(|l| l.name.as_str()).collect();
        assert_eq!(languages, ["typescript", "tsx"]);
        assert_eq!(
            s.languages[1].pos,
            Some(Pos {
                line: 8,
                column: 28
            })
        );
        assert_eq!(s.mason.as_deref(), Some("typescript-language-server"));
        assert_eq!(s.command, "typescript-language-server");
        assert_eq!(s.args, ["--stdio"]);
        assert_eq!(
            s.env,
            BTreeMap::from([("TSS_LOG".to_owned(), "-level verbose".to_owned())])
        );
        assert_eq!(s.root_markers, ["tsconfig.json", "*.code-workspace"]);
        assert_eq!(
            s.initialization_options,
            Some(
                serde_json::json!({"preferences": {"quoteStyle": "single"}, "maxTsServerMemory": 4096})
            )
        );
        assert_eq!(
            s.settings,
            Some(serde_json::json!({
                "typescript": {"inlayHints": {"enabled": true}, "ratio": 0.5, "when": "1979-05-27"},
                "list": [1, "two", [false]],
            }))
        );
        assert_eq!(
            s.disabled_features,
            [LspFeature::Formatting, LspFeature::Rename]
        );
        let requires: Vec<(LspServerRuntime, String)> = s
            .requires
            .iter()
            .map(|r| (r.runtime, r.version.to_string()))
            .collect();
        assert_eq!(
            requires,
            [
                (LspServerRuntime::Node, ">=20".to_owned()),
                (LspServerRuntime::Python, ">=3.10".to_owned())
            ]
        );
        assert_eq!(s.resolve_hint, Some(ResolveHint::Rustup));
        assert_eq!(
            s.project_local.as_ref().map(RelPath::as_str),
            Some("node_modules/.bin/typescript-language-server")
        );
        assert_eq!(
            s.project_local_options,
            [(
                "tsserver.path".to_owned(),
                RelPath::parse("node_modules/typescript/lib").unwrap()
            )]
        );
    }

    #[test]
    fn a_language_server_needs_only_languages_and_command() {
        let (manifest, diagnostics) = parse(&format!(
            "{MINIMAL}\n[contributes.language_servers.ra]\nlanguages = [\"rust\"]\ncommand = \"rust-analyzer\"\n"
        ));
        assert_eq!(diagnostics, vec![]);
        let s = &manifest.unwrap().contributes.language_servers[0];
        assert_eq!(
            (s.mason.as_deref(), s.args.len(), s.settings.as_ref()),
            (None, 0, None)
        );
        assert!(s.requires.is_empty() && s.disabled_features.is_empty());
    }

    /// 키마다의 형식 오류. 오류가 하나라도 있으면 매니페스트가 없고, 진단은 그 값의 줄 · 열을 가리킨다.
    #[test]
    fn language_server_format_errors_point_at_the_value() {
        let cases: [(&str, &str); 22] = [
            (
                "languages = [\"rust\"]\ncommand = \"x\"\nmason = \"../evil\"",
                "10:9: error: language server `s`: `mason` `../evil` must be a registry package name (lowercase letters, digits, `.`, `_`, `-`)",
            ),
            (
                "languages = [\"rust\"]\ncommand = \"x\"\nmason = \"a/b\"",
                "10:9: error: language server `s`: `mason` `a/b` must be a registry package name (lowercase letters, digits, `.`, `_`, `-`)",
            ),
            (
                "languages = [\"rust\"]\ncommand = \"x\"\nmason = \"Jdtls\"",
                "10:9: error: language server `s`: `mason` `Jdtls` must be a registry package name (lowercase letters, digits, `.`, `_`, `-`)",
            ),
            (
                "command = \"x\"",
                "7:31: error: language server `s`: missing `languages`",
            ),
            (
                "languages = []\ncommand = \"x\"",
                "8:13: error: language server `s`: `languages` is empty",
            ),
            (
                "languages = \"rust\"\ncommand = \"x\"",
                "8:13: error: language server `s`: `languages` must be an array of strings",
            ),
            (
                "languages = [\"rust\"]",
                "7:31: error: language server `s`: missing `command`",
            ),
            (
                "languages = [\"rust\"]\ncommand = [\"ra\", \"--x\"]",
                "9:11: error: language server `s`: `command` must be one executable name like \"rust-analyzer\" — put arguments in `args`",
            ),
            (
                "languages = [\"rust\"]\ncommand = \"ra --stdio\"",
                "9:11: error: language server `s`: `command` `ra --stdio` must be one executable name — shell command strings are not accepted; put arguments in `args`",
            ),
            (
                "languages = [\"rust\"]\ncommand = \"bin/ra\"",
                "9:11: error: language server `s`: `command` `bin/ra` must be an executable name, not a path — set a path with `[lsp.<id>] path` in settings",
            ),
            (
                "languages = [\"rust\"]\ncommand = \"\"",
                "9:11: error: language server `s`: `command` is empty",
            ),
            (
                "languages = [\"rust\"]\ncommand = \"-x\"",
                "9:11: error: language server `s`: `command` `-x` must not start with `-`",
            ),
            (
                "languages = [\"rust\"]\ncommand = \"x\"\nargs = \"--stdio\"",
                "10:8: error: language server `s`: `args` must be an array of strings",
            ),
            (
                "languages = [\"rust\"]\ncommand = \"x\"\nenv = { A = 1 }",
                "10:13: error: language server `s`: `env.A` must be a string",
            ),
            (
                "languages = [\"rust\"]\ncommand = \"x\"\nroot_markers = [\"a/Cargo.toml\"]",
                "10:17: error: language server `s`: root marker `a/Cargo.toml` must be a file name pattern, not a path",
            ),
            (
                "languages = [\"rust\"]\ncommand = \"x\"\nroot_markers = [\"[\"]",
                "10:17: error: language server `s`: root marker `[` is not a glob: unclosed character class; missing ']'",
            ),
            (
                "languages = [\"rust\"]\ncommand = \"x\"\nsettings = 1",
                "10:12: error: language server `s`: `settings` must be a table",
            ),
            (
                "languages = [\"rust\"]\ncommand = \"x\"\ninitialization_options = { x = nan }",
                "10:26: error: language server `s`: `x`: NaN has no JSON form",
            ),
            (
                "languages = [\"rust\"]\ncommand = \"x\"\nrequires = { ruby = \">=3\" }",
                "10:14: error: language server `s`: `requires.ruby`: runtime must be `node`, `python` or `java`",
            ),
            (
                "languages = [\"rust\"]\ncommand = \"x\"\nrequires = { java = \"twenty-one\" }",
                "10:21: error: language server `s`: `requires.java` `twenty-one` is not a version range: unexpected character 't' while parsing major version number",
            ),
            (
                "languages = [\"rust\"]\ncommand = \"x\"\nresolve_hint = \"cargo\"",
                "10:16: error: language server `s`: `resolve_hint` `cargo` must be `rustup`",
            ),
            (
                "languages = [\"rust\"]\ncommand = \"x\"\nproject_local = \"../bin/x\"",
                "10:17: error: language server `s`: `project_local`: `../bin/x` points outside the project",
            ),
        ];
        for (body, expected) in cases {
            let source = format!("{MINIMAL}\n[contributes.language_servers.s]\n{body}\n");
            let (manifest, diagnostics) = parse(&source);
            assert!(manifest.is_none(), "{body}");
            assert_eq!(
                errors(&diagnostics),
                [format!("{FILE}:{expected}")],
                "{body}"
            );
        }
    }

    #[test]
    fn warns_and_ignores_unknown_language_server_keys_and_features() {
        let source = format!(
            "{MINIMAL}\n[contributes.language_servers.s]\nlanguages = [\"rust\"]\ncommand = \"x\"\ninstall = {{ url = \"https://x\" }}\nfeatures = {{ disable = [\"hover\", \"inlay_hints\"], enable = [] }}\n"
        );
        let (manifest, diagnostics) = parse(&source);
        let s = &manifest
            .expect("warnings do not reject the extension")
            .contributes
            .language_servers[0];
        assert_eq!(s.disabled_features, [LspFeature::Hover]);
        let warnings: Vec<String> = diagnostics.iter().map(ToString::to_string).collect();
        assert_eq!(
            warnings,
            [
                format!(
                    "{FILE}:11:50: warning: language server `s`: `features`: unknown key `enable` is ignored"
                ),
                format!(
                    "{FILE}:11:34: warning: language server `s`: unknown feature `inlay_hints` in `features.disable` is ignored"
                ),
                format!(
                    "{FILE}:10:1: warning: language server `s`: unknown key `install` is ignored"
                ),
            ]
        );
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
}
