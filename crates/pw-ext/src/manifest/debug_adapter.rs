//! `[contributes.debug_adapters.<id>]` — 디버그 어댑터 정의(`docs/design/17-m6-plan.md` M6-16 ①, 07-extensibility.md
//! 「매니페스트」 예). 언어 서버 정의와 같은 꼴(`languages` · `mason` · `command` · `args` · `env` · `requires` ·
//! `project_local`)에, 어댑터와 이야기하는 길(`transport`), 실행 구성 `[debug]` 를 어댑터 요청으로 옮기는 틀(`launch` · `attach`),
//! 실행 파일 없이 언어 서버가 포트를 여는 어댑터의 시작(`start`)을 더한다. 내장 확장의 것만 등록한다(레지스트리).
//!
//! 자리 표지: `{이름}`(소문자 · 숫자 · `_`, 소문자로 시작). 틀(`launch` · `attach`)의 자리는 실행 구성 `[debug]` 의 값으로 채우고
//! ([`fill_request`]), `args` 의 자리는 `{port}` · `{host}` 둘뿐이다 — 포트는 Pitwall 이 고르고 주소는 늘 [`ADAPTER_HOST`] 다
//! (17-m6-plan 메인 판정 M6-P7 — 같은 기기의 다른 프로세스가 그 포트에 붙어 대상 안에서 코드를 돌리지 못하게).

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value as Json;
use toml_edit::{Item, TableLike};

use super::{
    Fields, NameRef, Parser, Pos, RuntimeRequirement, Span, check_executable_name, is_package_name,
    is_segment,
};
use crate::source::RelPath;

/// 포트 어댑터가 묶는 주소(`args` 의 `{host}`). 다른 주소는 정의에 적을 수 없다.
pub const ADAPTER_HOST: &str = "127.0.0.1";

/// 어댑터와 이야기하는 길.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DapTransport {
    /// 어댑터 프로세스의 표준 입출력.
    Stdio,
    /// 어댑터가 연 TCP 포트(`127.0.0.1`). 실행 파일이 있는 어댑터는 `args` 의 `{port}` 로 포트를 받고, `start` 어댑터는 언어
    /// 서버가 답한 포트다.
    Tcp,
}

/// 실행 구성 `[debug] mode` 의 두 요청.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DapRequest {
    Launch,
    Attach,
}

/// `start = { lsp = "<서버 id>", command = "<명령>", bundles = [...] }` — 실행 파일 없이 언어 서버가 포트를 여는 어댑터(java-debug:
/// jdtls 에 `workspace/executeCommand` 의 `vscode.java.startDebugSession` 을 보내 받은 포트로 붙는다).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdapterStart {
    /// 명령을 받을 언어 서버 id(`[contributes.language_servers.<id>]`). 그 서버가 떠 있지 않으면 시작을 거절한다.
    pub lsp: NameRef,
    /// `workspace/executeCommand` 의 명령 이름. 답은 포트 번호다.
    pub command: String,
    /// 그 서버의 `initializationOptions.bundles` 에 넣을 것 — 어댑터 패키지(`mason`)의 `share` 이름(설치 폴더 안의 jar). 설치돼
    /// 있을 때만 넣는다.
    pub bundles: Vec<String>,
}

/// `[contributes.debug_adapters.<id>]` 하나.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DebugAdapterDecl {
    pub id: String,
    pub pos: Option<Pos>,
    /// 이 어댑터가 맡는 언어 id. 하나 이상이다.
    pub languages: Vec<NameRef>,
    /// Mason registry 패키지 이름(설치본을 찾고 설치를 제안한다 — `start` 어댑터면 `bundles` 를 찾는 곳).
    pub mason: Option<String>,
    /// 실행 파일 이름 하나(언어 서버의 `command` 와 같은 규칙). `start` 어댑터만 없다.
    pub command: Option<String>,
    /// 인자. `{port}` · `{host}` 자리는 [`DebugAdapterDecl::args_for`] 가 채운다.
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub transport: DapTransport,
    /// `launch` 요청의 틀(TOML 표를 JSON 으로). 없으면 이 어댑터로 launch 구성을 돌리지 못한다.
    pub launch: Option<Json>,
    /// `attach` 요청의 틀.
    pub attach: Option<Json>,
    /// 어댑터를 **돌리는** 런타임과 판 범위(언어 서버의 `requires` 와 같다).
    pub requires: Vec<RuntimeRequirement>,
    /// 프로젝트 안의 실행 파일(프로젝트 루트 기준). 신뢰한 프로젝트에서만 본다.
    pub project_local: Option<RelPath>,
    pub start: Option<AdapterStart>,
}

impl DebugAdapterDecl {
    /// 그 요청의 틀.
    pub fn template(&self, request: DapRequest) -> Option<&Json> {
        match request {
            DapRequest::Launch => self.launch.as_ref(),
            DapRequest::Attach => self.attach.as_ref(),
        }
    }

    /// 어댑터 프로세스의 인자: `{port}` 를 `port` 로, `{host}` 를 [`ADAPTER_HOST`] 로 채운 것. stdio 어댑터는 `port` 가 `None` 이다
    /// (그 정의의 `args` 에는 자리가 없다 — 읽을 때 본다).
    pub fn args_for(&self, port: Option<u16>) -> Vec<String> {
        let port = port.map(|p| p.to_string()).unwrap_or_default();
        self.args
            .iter()
            .map(|a| a.replace("{port}", &port).replace("{host}", ADAPTER_HOST))
            .collect()
    }
}

/// 자리 표지 이름: 소문자로 시작하고 소문자 · 숫자 · `_`.
fn is_slot_name(s: &str) -> bool {
    s.starts_with(|c: char| c.is_ascii_lowercase())
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// 글 안의 자리 표지(`{이름}`)와 그 바이트 범위. 이름 규칙에 맞지 않는 중괄호(`${workspaceFolder}` 등)는 글자 그대로다.
fn slots(s: &str) -> Vec<(std::ops::Range<usize>, &str)> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(open) = s[from..].find('{').map(|i| i + from) {
        let Some(close) = s[open + 1..].find('}').map(|i| i + open + 1) else {
            break;
        };
        let name = &s[open + 1..close];
        if is_slot_name(name) {
            out.push((open..close + 1, name));
            from = close + 1;
        } else {
            from = open + 1;
        }
    }
    out
}

/// 틀이 쓰는 자리 이름(이름순). 실행 구성 편집기가 구성에 적을 값을 고를 때 쓴다.
pub fn template_slots(template: &Json) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    collect_slots(template, &mut out);
    out
}

fn collect_slots(value: &Json, out: &mut BTreeSet<String>) {
    match value {
        Json::String(s) => out.extend(slots(s).into_iter().map(|(_, n)| n.to_owned())),
        Json::Array(items) => items.iter().for_each(|v| collect_slots(v, out)),
        Json::Object(map) => map.values().for_each(|v| collect_slots(v, out)),
        _ => {}
    }
}

/// 틀을 실행 구성의 값으로 채운다. 값(`values` — 자리 이름 → 값)은 구성에서 오고 틀에는 자리만 있다.
///
/// - 글 전체가 자리 하나(`"{port}"`)면 그 값이 형 그대로 들어간다(수 · 목록 · 표). 값이 없으면(또는 `null`) 그 키 · 목록 칸을
///   뺀다 — 구성에 적지 않은 선택 값이다.
/// - 글 안의 자리(`"--inspect={port}"`)는 글로 잇는다. 값은 글 · 수 · 참거짓이어야 하고, 없으면 오류다(반쪽짜리 글을 보내지 않는다).
pub fn fill_request(template: &Json, values: &BTreeMap<String, Json>) -> Result<Json, String> {
    Ok(fill(template, values)?.unwrap_or(Json::Null))
}

fn fill(value: &Json, values: &BTreeMap<String, Json>) -> Result<Option<Json>, String> {
    let given = |name: &str| values.get(name).filter(|v| !v.is_null());
    Ok(Some(match value {
        Json::String(s) => {
            let found = slots(s);
            if let [(range, name)] = found.as_slice()
                && *range == (0..s.len())
            {
                return Ok(given(name).cloned());
            }
            let mut out = String::with_capacity(s.len());
            let mut at = 0;
            for (range, name) in found {
                out.push_str(&s[at..range.start]);
                match given(name) {
                    Some(Json::String(v)) => out.push_str(v),
                    Some(v @ (Json::Number(_) | Json::Bool(_))) => out.push_str(&v.to_string()),
                    Some(_) => {
                        return Err(format!(
                            "`{{{name}}}` in \"{s}\" must be a string, a number or a boolean"
                        ));
                    }
                    None => return Err(format!("`{{{name}}}` in \"{s}\" has no value")),
                }
                at = range.end;
            }
            out.push_str(&s[at..]);
            Json::String(out)
        }
        Json::Array(items) => Json::Array(
            items
                .iter()
                .map(|v| fill(v, values))
                .filter_map(Result::transpose)
                .collect::<Result<_, _>>()?,
        ),
        Json::Object(map) => Json::Object(
            map.iter()
                .map(|(k, v)| Ok(fill(v, values)?.map(|v| (k.clone(), v))))
                .filter_map(Result::transpose)
                .collect::<Result<_, String>>()?,
        ),
        other => other.clone(),
    }))
}

impl Parser<'_> {
    /// `[contributes.debug_adapters.<id>]` 하나. 형식 오류는 오류로, 모르는 키는 경고로 남긴다.
    pub(super) fn debug_adapter(
        &mut self,
        id: String,
        entry: &dyn TableLike,
        at: Span,
    ) -> Option<DebugAdapterDecl> {
        let what = format!("debug adapter `{id}`");
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
        let command_item = f.item(self, "command", false);
        let command = command_item.map(|item| {
            let Some(raw) = item.as_str() else {
                self.error(
                    item.span(),
                    format!(
                        "{what}: `command` must be one executable name like \"codelldb\" — put arguments in `args`"
                    ),
                );
                return None;
            };
            match check_executable_name(raw)
                .map_err(|e| e.replace("[lsp.<id>]", "[dap.<id>]"))
            {
                Ok(()) => Some(raw.to_owned()),
                Err(e) => {
                    self.error(item.span(), format!("{what}: `command` {e}"));
                    None
                }
            }
        });
        let args_item = f.item(self, "args", false);
        let args = args_item.and_then(|item| self.adapter_args(item, &what));
        let env_item = f.table(self, "env", false);
        let env = env_item
            .as_ref()
            .map(|(table, _)| self.env(*table, &what))
            .unwrap_or_default();
        let transport = f
            .string(self, "transport", false)
            .map(|(raw, span)| match raw.as_str() {
                "stdio" => Some((DapTransport::Stdio, span)),
                "tcp" => Some((DapTransport::Tcp, span)),
                _ => {
                    self.error(
                        span,
                        format!("{what}: `transport` `{raw}` must be `stdio` or `tcp`"),
                    );
                    None
                }
            });
        let launch = self.request_template(&mut f, "launch", &what);
        let attach = self.request_template(&mut f, "attach", &what);
        let requires_item = f.table(self, "requires", false);
        let requires = requires_item
            .as_ref()
            .map(|(table, _)| self.requires(*table, &what))
            .unwrap_or_default();
        let project_local_item = f.string(self, "project_local", false);
        let project_local = project_local_item.clone().and_then(|(raw, span)| {
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
        let start = f
            .table(self, "start", false)
            .map(|(table, span)| self.adapter_start(table, span, &what, mason.is_some()));
        f.finish(self);

        // 어느 쪽으로 띄우나: 실행 파일(`command`) 또는 언어 서버(`start`) — 하나만.
        let start = match start {
            Some(start) => {
                for (key, present) in [
                    ("command", command_item.is_some()),
                    ("args", args_item.is_some()),
                    ("env", env_item.is_some()),
                    ("requires", requires_item.is_some()),
                    ("project_local", project_local_item.is_some()),
                ] {
                    if present {
                        self.error(
                            at.clone(),
                            format!(
                                "{what}: `{key}` cannot be used with `start` — a language server opens the adapter's port, there is no adapter process"
                            ),
                        );
                    }
                }
                if let Some(Some((DapTransport::Stdio, span))) = &transport {
                    self.error(
                        span.clone(),
                        format!(
                            "{what}: `transport` must be `tcp` with `start` — the language server answers a port"
                        ),
                    );
                }
                Some(start?)
            }
            None => {
                if command_item.is_none() {
                    self.error(
                        at.clone(),
                        format!(
                            "{what}: missing `command` (or `start` for an adapter a language server opens)"
                        ),
                    );
                }
                None
            }
        };
        let transport = match (&transport, &start) {
            (Some(Some((t, _))), _) => *t,
            (Some(None), _) => return None,
            (None, Some(_)) => DapTransport::Tcp,
            (None, None) => DapTransport::Stdio,
        };
        // `args` 의 형식 오류는 이미 남겼다 — 그 위에 「포트가 없다」 를 겹쳐 내지 않는다.
        let args_ok = args.is_some() || args_item.is_none();
        let args = args.unwrap_or_default();
        if start.is_none() && args_ok {
            let has_port = args
                .iter()
                .any(|a| slots(a).iter().any(|(_, n)| *n == "port"));
            let span = args_item.and_then(Item::span).or_else(|| at.clone());
            match transport {
                DapTransport::Tcp if !has_port => self.error(
                    span,
                    format!(
                        "{what}: a `tcp` adapter takes its port in `args` — write \"{{port}}\" where the port goes"
                    ),
                ),
                DapTransport::Stdio
                    if args.iter().any(|a| !slots(a).is_empty()) =>
                {
                    self.error(
                        span,
                        format!(
                            "{what}: `{{port}}` and `{{host}}` are only for a `tcp` adapter"
                        ),
                    );
                }
                _ => {}
            }
        }
        // 적었지만 형식이 틀린 틀(`Some(None)`)은 이미 오류다.
        if launch.is_none() && attach.is_none() {
            self.error(
                at.clone(),
                format!("{what}: needs a `launch` or an `attach` request template (or both)"),
            );
        }
        Some(DebugAdapterDecl {
            pos: self.pos(&at),
            id,
            languages: languages?,
            mason,
            command: command.flatten(),
            args,
            env,
            transport,
            launch: launch.flatten(),
            attach: attach.flatten(),
            requires,
            project_local,
            start,
        })
    }

    /// `args`: 문자열 배열. 자리는 `{port}` · `{host}` 만.
    fn adapter_args(&mut self, item: &Item, what: &str) -> Option<Vec<String>> {
        let Some(values) = item.as_array().and_then(|array| {
            array
                .iter()
                .map(|v| v.as_str().map(|s| (s.to_owned(), v.span())))
                .collect::<Option<Vec<_>>>()
        }) else {
            self.error(
                item.span(),
                format!("{what}: `args` must be an array of strings"),
            );
            return None;
        };
        let mut ok = true;
        for (arg, span) in &values {
            for (_, name) in slots(arg) {
                if name != "port" && name != "host" {
                    ok = false;
                    self.error(
                        span.clone().or_else(|| item.span()),
                        format!(
                            "{what}: `args` can only use `{{port}}` and `{{host}}`, not `{{{name}}}` — request values go in `launch` · `attach`"
                        ),
                    );
                }
            }
        }
        ok.then(|| values.into_iter().map(|(a, _)| a).collect())
    }

    /// `launch` · `attach`: 표. `None` 은 키가 없음, `Some(None)` 은 형식 오류.
    fn request_template(
        &mut self,
        f: &mut Fields<'_>,
        key: &'static str,
        what: &str,
    ) -> Option<Option<Json>> {
        let present = f.table.contains_key(key);
        match f.table(self, key, false) {
            Some((table, span)) => Some(self.json_table(table, span, what)),
            None => present.then_some(None),
        }
    }

    fn adapter_start(
        &mut self,
        table: &dyn TableLike,
        span: Span,
        what: &str,
        has_mason: bool,
    ) -> Option<AdapterStart> {
        let mut f = Fields::new(table, span.clone(), &format!("{what}: `start`"));
        let lsp = f.string(self, "lsp", true).and_then(|(raw, span)| {
            if is_segment(&raw) {
                Some(NameRef {
                    name: raw,
                    pos: self.pos(&span),
                })
            } else {
                self.error(
                    span,
                    format!("{what}: `start.lsp` `{raw}` must be a language server id"),
                );
                None
            }
        });
        let command = f.string(self, "command", true).and_then(|(raw, span)| {
            if raw.chars().any(|c| c.is_whitespace() || c.is_control()) {
                self.error(
                    span,
                    format!("{what}: `start.command` `{raw}` must be one command name"),
                );
                None
            } else {
                Some(raw)
            }
        });
        let bundles_item = f.item(self, "bundles", false);
        let bundles = bundles_item.and_then(|item| {
            let names = self.name_list(item, what, "start.bundles")?;
            let mut out = Vec::new();
            for name in names {
                match RelPath::parse(&name.name) {
                    Ok(path) => out.push(path.as_str().to_owned()),
                    Err(e) => self.at(
                        super::Severity::Error,
                        name.pos,
                        format!(
                            "{what}: `start.bundles`: {}",
                            e.replace("extension folder", "install folder")
                        ),
                    ),
                }
            }
            Some(out)
        });
        f.finish(self);
        if bundles.as_ref().is_some_and(|b| !b.is_empty()) && !has_mason {
            self.error(
                bundles_item.and_then(Item::span).or(span),
                format!("{what}: `start.bundles` names files of the `mason` package — set `mason`"),
            );
            return None;
        }
        Some(AdapterStart {
            lsp: lsp?,
            command: command?,
            bundles: bundles.unwrap_or_default(),
        })
    }
}

#[cfg(test)]
mod tests;
