//! `pitwall tool` 의 몸통(`docs/design/05-mcp.md` 「CLI (`pitwall tool`)」, 16-m5-plan M5-04 · 결정 M5-D12). Desktop 의
//! `pitwall tool`(`apps/desktop/src-tauri/src/tool_cli.rs`)과 원격 머신의 `pitwall-relay tool`(`apps/relay` — 18-m7-plan M7-20)이
//! 같은 것을 쓴다 — 둘이 다른 것은 세션을 여는 길(어느 소켓에 붙는가 · 헤드리스가 있는가)뿐이다.
//!
//! **CLI 는 MCP 클라이언트다.** `pitwall mcp` 와 같은 길로 세션을 연다 — 앱의 소켓이 받으면 앱에(`mcp::connect_app`), 아니면
//! 이 프로세스 안의 헤드리스 Host(`mcp::Headless`)에 프로세스 안 duplex 로 붙어 `initialize` → `tools/list` → `tools/call` 을
//! 보낸다. 서버 쪽에 CLI 전용 경로가 없다 — 카탈로그 · 노출 · 정책 · 감사가 MCP 와 한 벌이다. 클라이언트 이름은
//! `pitwall-cli <버전>`(감사 로그의 클라이언트 칸).
//!
//! - **플래그**: 입력 스키마의 최상위 필드마다 하나(camelCase → kebab-case, [`flag_name`]). 값은 스키마의 타입으로 읽는다
//!   ([`arguments`]). `--project` · `--input` · `--help` 는 예약이다([`RESERVED_FLAGS`] — 겹치는 필드는 데스크톱 크레이트의 툴
//!   목록 시험이 막는다). 툴 이름 `list` 도 명령과 겹쳐 같은 시험이 막는다.
//! - **출력**: stdout 에는 결과 JSON 문서 하나(`structuredContent`, 없으면 텍스트 내용). `list` · `--help` 만 글이다. 툴 오류 ·
//!   거절 이유 · 로그는 stderr, 로그 파일은 `$STATE/logs/cli.<날짜>.log`([`LOG_COMPONENT`]).
//! - **종료 코드**는 [`exit`]. 1 과 3 은 서버의 결과 표지(`crate::marker`)로 가른다. 앱이 있어야 하는 툴(링크된 카탈로그에는 있고
//!   헤드리스 Host 가 처리하지 않는 것)을 앱 없이 부르면 부르지 않고 3 이다(결정 M5-D3).
//! - **취소**: Ctrl+C 면 그 호출을 MCP 취소 알림(`notifications/cancelled`)으로 거둔다 — 서버가 그 호출의 Host RPC 를 취소하는
//!   M5-03 의 길이다. 그리고 130(128 + SIGINT)으로 끝난다.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::future::Future;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::time::Duration;

use crate::Catalog;
use crate::audit::Outcome;
use crate::marker::{Mark, Reason};
use rmcp::model::{
    CallToolRequest, CallToolRequestParams, CallToolResult, ClientCapabilities, ClientConfig,
    ClientRequest, Implementation, ServerResult,
};
use rmcp::service::{PeerRequestOptions, RunningService, ServiceError};
use rmcp::transport::IntoTransport;
use rmcp::{RoleClient, ServiceExt};
use serde_json::{Map, Value};

/// 로그 파일의 구성요소 이름(`cli.<날짜>.log`). 앱의 `core` · `pitwall mcp` 의 `mcp` 와 나눈다 — 호출마다 프로세스가 뜬다.
pub const LOG_COMPONENT: &str = "cli";

/// `initialize` 의 클라이언트 이름. 감사 로그에는 `pitwall-cli <버전>` 으로 남는다.
pub const CLIENT_NAME: &str = "pitwall-cli";

/// 툴 입력의 필드가 될 수 없는 플래그 이름(`--` 없이). 툴 목록 시험(`mcp_tests.rs`)이 겹치는 필드를 막는다.
#[allow(dead_code)] // 파서는 이름을 직접 가른다 — 이 목록을 읽는 것은 시험이다.
pub const RESERVED_FLAGS: [&str; 3] = ["project", "input", "help"];

/// `pitwall tool list` — 툴 이름이 될 수 없다.
pub const LIST: &str = "list";

/// 종료 코드(05-mcp.md 「CLI」).
pub mod exit {
    pub const OK: i32 = 0;
    /// 툴 오류 · 앱과의 연결 끊김.
    pub const FAILED: i32 = 1;
    /// 사용법 오류(모르는 툴 · 플래그, 타입이 안 맞는 값, 툴 없이 `pitwall tool`).
    pub const USAGE: i32 = 2;
    /// 거절(노출 안 됨 · 승인 거절 · 헤드리스의 `write` 이상 · 앱이 있어야 하는 툴을 앱 없이 부름).
    pub const DENIED: i32 = 3;
    /// Ctrl+C 로 호출을 취소했다(128 + SIGINT).
    pub const INTERRUPTED: i32 = 128 + 2;
}

/// 이 세션에 노출되지 않은 툴의 안내. 이 실행 파일에는 링크됐지만 세션 목록에 없는 툴이다 — 노출 설정이 껐거나, 붙은 앱이 다른
/// 버전이라 그 툴이 없다.
const NOT_EXPOSED: &str = "is not exposed in this session (turned off in the [mcp] settings, or not in this version of the Pitwall app)";

/// `write` 이상 툴의 답이 이만큼 오지 않으면 승인을 기다리는 중일 수 있다고 stderr 에 한 줄 쓴다(승인 경로는 16-m5-plan M5-12).
const APPROVAL_NOTE_AFTER: Duration = Duration::from_secs(1);

pub const USAGE: &str = "\
usage: pitwall tool list [--project <path>]
       pitwall tool <tool> --help [--project <path>]
       pitwall tool <tool> [--<flag> <value>]... [--input <json>|-] [--project <path>]

Calls a Pitwall MCP tool from the command line: the same tools, inputs and checks as the MCP server.
Each top-level field of the tool's input is a flag (`maxCount` is `--max-count`); strings are taken
as they are, numbers, booleans, arrays and objects as JSON. `--input` gives the whole input object
(`-` reads it from stdin); flags override its fields. `--project` picks the project (default: the
current directory). `--checkout <machine>:<path>` targets that machine's checkout of the project
(tools that take a worktree, with the Pitwall app running). The result is one JSON document on stdout; errors go to stderr.

exit codes: 0 ok, 1 tool error or lost connection, 2 usage error, 3 denied, 130 interrupted";

/// 명령줄이 고른 일.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
    /// `--project`. `tool` 뒤 어디에 와도 된다.
    pub project: Option<PathBuf>,
    pub action: Action,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// 아무것도 고르지 않았다 — 사용법과 [`exit::USAGE`].
    Usage,
    /// `pitwall tool --help` — 사용법과 [`exit::OK`].
    Help,
    /// `pitwall tool list`.
    List,
    /// `pitwall tool <툴> --help`.
    ToolHelp(String),
    Call(Call),
}

/// `pitwall tool <툴> …`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Call {
    pub tool: String,
    pub input: Option<Input>,
    /// `--<이름> <값>` 의 이름(`--` 없이)과 값. 준 순서대로.
    pub flags: Vec<(String, String)>,
}

/// `--input` 의 값.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Input {
    Json(String),
    /// `--input -`.
    Stdin,
}

/// `tool` 뒤의 인자를 가른다. 사용법 오류는 `Err`(문장).
pub fn parse(args: &[OsString]) -> Result<Invocation, String> {
    let mut project = None;
    let mut rest = Vec::new();
    let mut iter = args.iter();
    // 툴 이름 뒤 `--<이름> <값>` 의 값 자리는 가로채지 않는다 — `--message --project` 의 `--project` 는 값이다.
    let mut value_next = false;
    while let Some(arg) = iter.next() {
        let text = arg
            .to_str()
            .ok_or_else(|| format!("argument {arg:?} is not UTF-8"))?;
        if value_next {
            value_next = false;
            rest.push(text.to_owned());
            continue;
        }
        let value = if text == "--project" {
            PathBuf::from(iter.next().ok_or("--project needs a path")?)
        } else if let Some(value) = text.strip_prefix("--project=") {
            PathBuf::from(value)
        } else {
            value_next = !rest.is_empty() && takes_value(text);
            rest.push(text.to_owned());
            continue;
        };
        if project.replace(value).is_some() {
            return Err("--project given twice".into());
        }
    }
    let action = match rest.split_first() {
        None => Action::Usage,
        Some((first, _)) if is_help(first) => Action::Help,
        Some((first, more)) if first == LIST => match more {
            [] => Action::List,
            [help] if is_help(help) => Action::Help,
            [extra, ..] => return Err(format!("unexpected argument `{extra}` after `list`")),
        },
        Some((first, _)) if first.starts_with('-') => {
            return Err(format!("unknown option `{first}`"));
        }
        Some((tool, more)) => tool_action(tool, more)?,
    };
    Ok(Invocation { project, action })
}

fn is_help(arg: &str) -> bool {
    arg == "--help" || arg == "-h"
}

/// 플래그 이름 자리의 인자가 다음 인자를 값으로 받는가: `=` 없는 `--<이름>`(도움말 빼고).
fn takes_value(arg: &str) -> bool {
    arg.len() > 2 && arg.starts_with("--") && !arg.contains('=') && !is_help(arg)
}

fn tool_action(tool: &str, args: &[String]) -> Result<Action, String> {
    // 도움말은 플래그 이름 자리에서만 본다 — `--pattern -h` 의 `-h` 는 값이다.
    let mut names = args.iter();
    while let Some(arg) = names.next() {
        if is_help(arg) {
            return Ok(Action::ToolHelp(tool.to_owned()));
        }
        if takes_value(arg) {
            names.next();
        }
    }
    let mut input = None;
    let mut flags: Vec<(String, String)> = Vec::new();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        let Some(flag) = arg.strip_prefix("--").filter(|f| !f.is_empty()) else {
            return Err(format!(
                "unexpected argument `{arg}`; give the tool input as --<flag> <value> or --input"
            ));
        };
        let (name, value) = match flag.split_once('=') {
            Some((name, value)) => (name.to_owned(), value.to_owned()),
            None => (
                flag.to_owned(),
                iter.next()
                    .ok_or_else(|| format!("--{flag} needs a value"))?
                    .clone(),
            ),
        };
        if name == "input" {
            let source = if value == "-" {
                Input::Stdin
            } else {
                Input::Json(value)
            };
            if input.replace(source).is_some() {
                return Err("--input given twice".into());
            }
        } else if flags.iter().any(|(n, _)| *n == name) {
            return Err(format!("--{name} given twice"));
        } else {
            flags.push((name, value));
        }
    }
    Ok(Action::Call(Call {
        tool: tool.to_owned(),
        input,
        flags,
    }))
}

/// `--input` 을 읽는다(stdin 이면 `stdin` 을 끝까지). 입력 객체가 아니면 사용법 오류.
pub fn read_input(input: &Input, mut stdin: impl Read) -> Result<Value, String> {
    let text = match input {
        Input::Json(text) => text.clone(),
        Input::Stdin => {
            let mut text = String::new();
            stdin
                .read_to_string(&mut text)
                .map_err(|e| format!("cannot read --input from stdin: {e}"))?;
            text
        }
    };
    match serde_json::from_str(&text) {
        Ok(Value::Object(map)) => Ok(Value::Object(map)),
        Ok(_) => Err("--input must be a JSON object".into()),
        Err(e) => Err(format!("--input is not JSON: {e}")),
    }
}

/// 입력 필드 이름 → 플래그 이름(`--` 없이): camelCase 와 snake_case 를 kebab-case 로(`maxCount` · `max_count` → `max-count`).
pub fn flag_name(field: &str) -> String {
    let mut out = String::with_capacity(field.len() + 4);
    for c in field.chars() {
        if c.is_ascii_uppercase() {
            if !out.is_empty() && !out.ends_with('-') {
                out.push('-');
            }
            out.push(c.to_ascii_lowercase());
        } else if c == '_' {
            out.push('-');
        } else {
            out.push(c);
        }
    }
    out
}

/// JSON 값의 타입. 스키마의 `type` 이름과 같다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum JsonType {
    String,
    Integer,
    Number,
    Boolean,
    Array,
    Object,
    Null,
}

impl JsonType {
    fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "string" => Self::String,
            "integer" => Self::Integer,
            "number" => Self::Number,
            "boolean" => Self::Boolean,
            "array" => Self::Array,
            "object" => Self::Object,
            "null" => Self::Null,
            _ => return None,
        })
    }

    fn of(value: &Value) -> Self {
        match value {
            Value::String(_) => Self::String,
            Value::Number(n) if n.is_i64() || n.is_u64() => Self::Integer,
            Value::Number(_) => Self::Number,
            Value::Bool(_) => Self::Boolean,
            Value::Array(_) => Self::Array,
            Value::Object(_) => Self::Object,
            Value::Null => Self::Null,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::String => "string",
            Self::Integer => "integer",
            Self::Number => "number",
            Self::Boolean => "boolean",
            Self::Array => "array",
            Self::Object => "object",
            Self::Null => "null",
        }
    }

    fn accepts(self, value: &Value) -> bool {
        let actual = Self::of(value);
        actual == self || (self == Self::Number && actual == Self::Integer)
    }
}

/// 입력 스키마의 최상위 필드 하나가 만드는 플래그.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Flag {
    /// 플래그 이름(`--` 없이).
    pub name: String,
    /// 입력 필드 이름.
    pub field: String,
    /// 스키마가 허락하는 타입. 스키마로 알 수 없으면 `None`(JSON 으로 읽고, 안 되면 글자열).
    pub types: Option<BTreeSet<JsonType>>,
    pub required: bool,
    pub description: Option<String>,
}

impl Flag {
    /// 도움말의 값 자리(`<string>` · `<integer|null>` · `<json>`).
    pub fn value_label(&self) -> String {
        match &self.types {
            Some(types) => {
                let names: Vec<_> = types.iter().map(|t| t.as_str()).collect();
                format!("<{}>", names.join("|"))
            }
            None => "<json>".into(),
        }
    }

    /// 값 하나를 스키마의 타입으로 읽는다. 글자열만 받으면 그대로, 아니면 JSON 으로 읽어 타입을 본다.
    pub fn read(&self, raw: &str) -> Result<Value, String> {
        let Some(types) = &self.types else {
            return Ok(serde_json::from_str(raw).unwrap_or_else(|_| Value::String(raw.to_owned())));
        };
        let takes_string = types.contains(&JsonType::String);
        let only_string = types
            .iter()
            .all(|t| matches!(t, JsonType::String | JsonType::Null));
        if takes_string && only_string {
            return Ok(Value::String(raw.to_owned()));
        }
        let parsed = serde_json::from_str::<Value>(raw)
            .ok()
            .filter(|v| !v.is_string() && types.iter().any(|t| t.accepts(v)));
        match parsed {
            Some(value) => Ok(value),
            None if takes_string => Ok(Value::String(raw.to_owned())),
            None => Err(format!(
                "--{} expects {} (as JSON), not `{raw}`",
                self.name,
                self.value_label()
            )),
        }
    }
}

/// 입력 스키마의 플래그들. 필드 순서는 스키마의 `properties` 순서다.
pub fn flags(schema: &Map<String, Value>) -> Vec<Flag> {
    let required: BTreeSet<&str> = schema
        .get("required")
        .and_then(Value::as_array)
        .map(|r| r.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
        return Vec::new();
    };
    properties
        .iter()
        .map(|(field, property)| Flag {
            name: flag_name(field),
            field: field.clone(),
            types: json_types(property, schema, 0),
            required: required.contains(field.as_str()),
            description: property
                .get("description")
                .and_then(Value::as_str)
                .map(str::to_owned),
        })
        .collect()
}

/// 스키마 조각이 허락하는 타입. `type` · `$ref`(`#/$defs/…` · `#/definitions/…`) · `anyOf` · `oneOf` · `enum` · `const` 를
/// 본다. 알 수 없으면 `None`.
fn json_types(schema: &Value, root: &Map<String, Value>, depth: u8) -> Option<BTreeSet<JsonType>> {
    if depth > 16 {
        return None;
    }
    let object = schema.as_object()?;
    if let Some(kind) = object.get("type") {
        return match kind {
            Value::String(name) => Some(BTreeSet::from([JsonType::parse(name)?])),
            Value::Array(names) => names
                .iter()
                .map(|n| n.as_str().and_then(JsonType::parse))
                .collect(),
            _ => None,
        };
    }
    if let Some(reference) = object.get("$ref").and_then(Value::as_str) {
        let target = ["#/$defs/", "#/definitions/"].iter().find_map(|prefix| {
            let name = reference.strip_prefix(prefix)?;
            root.get(&prefix[2..prefix.len() - 1])?.get(name)
        })?;
        return json_types(target, root, depth + 1);
    }
    for union in ["anyOf", "oneOf"] {
        if let Some(Value::Array(members)) = object.get(union) {
            let mut types = BTreeSet::new();
            for member in members {
                types.extend(json_types(member, root, depth + 1)?);
            }
            return Some(types);
        }
    }
    if let Some(Value::Array(values)) = object.get("enum") {
        return Some(values.iter().map(JsonType::of).collect());
    }
    if let Some(value) = object.get("const") {
        return Some(BTreeSet::from([JsonType::of(value)]));
    }
    match object.get("allOf") {
        Some(Value::Array(members)) if members.len() == 1 => {
            json_types(&members[0], root, depth + 1)
        }
        _ => None,
    }
}

/// 툴 호출의 입력: `--input` 의 객체 위에 플래그를 덮는다. 스키마에 없는 플래그 · 타입이 안 맞는 값 · 빠진 필수 필드는 사용법
/// 오류(`Err` 문장) — 호출하지 않는다.
pub fn arguments(
    tool: &str,
    schema: &Map<String, Value>,
    input: Option<Value>,
    given: &[(String, String)],
) -> Result<Map<String, Value>, String> {
    let mut args = match input {
        None => Map::new(),
        Some(Value::Object(map)) => map,
        Some(_) => return Err("--input must be a JSON object".into()),
    };
    let known = flags(schema);
    for (name, raw) in given {
        let flag = known.iter().find(|f| f.name == *name).ok_or_else(|| {
            format!("`{tool}` has no flag --{name}; `pitwall tool {tool} --help` lists its flags")
        })?;
        let value = if flag.field == crate::checkout::CHECKOUT {
            checkout_flag(raw)?
        } else {
            flag.read(raw)?
        };
        args.insert(flag.field.clone(), value);
    }
    if let Some(missing) = known
        .iter()
        .find(|f| f.required && !args.contains_key(&f.field))
    {
        return Err(format!(
            "`{tool}` needs --{} {}",
            missing.name,
            missing.value_label()
        ));
    }
    Ok(args)
}

/// `--checkout <machine>:<path>` · `--checkout <machine>`(그 머신의 메인 체크아웃) · JSON 객체(18-m7-plan M7-18 ⑥ — D61 의 꼴).
/// 머신 id 에는 `:` 가 든다(`SHA256:…`) — 경로는 `:` 뒤가 `/` 나 `~` 로 시작하는 첫 자리부터다. 경로는 그 머신의 것이라 이 머신에서
/// 펼치지 않는다(`~` 도 그대로 보낸다).
fn checkout_flag(raw: &str) -> Result<Value, String> {
    if raw.trim_start().starts_with('{') {
        return serde_json::from_str::<Value>(raw)
            .ok()
            .filter(Value::is_object)
            .ok_or_else(|| {
                format!("--checkout expects <machine>:<path> or a JSON object, not `{raw}`")
            });
    }
    let split = raw
        .match_indices(':')
        .map(|(i, _)| i)
        .find(|&i| matches!(raw[i + 1..].chars().next(), Some('/' | '~')));
    let (machine, worktree) = match split {
        Some(i) => (&raw[..i], Some(&raw[i + 1..])),
        None => (raw, None),
    };
    if machine.is_empty() {
        return Err(format!(
            "--checkout needs a machine name before the path, not `{raw}`"
        ));
    }
    Ok(serde_json::json!({ "machine": machine, "worktree": worktree }))
}

/// 등급 이름(`read` · `write` · `exec` · `dangerous`).
fn tier_name(tier: pw_rpc::Tier) -> &'static str {
    match tier {
        pw_rpc::Tier::Read => "read",
        pw_rpc::Tier::Write => "write",
        pw_rpc::Tier::Exec => "exec",
        pw_rpc::Tier::Dangerous => "dangerous",
    }
}

/// 세션을 연 뒤 할 일에 필요한 것.
pub struct Request<'a> {
    pub action: &'a Action,
    /// 읽어 둔 `--input`([`read_input`]).
    pub input: Option<Value>,
    /// 이 실행 파일에 링크된 카탈로그(`Catalog::linked`). 등급 · 「앱이 있어야 하는 툴」 판정에 쓴다 — MCP `tools/list` 에는
    /// 등급이 없다.
    pub linked: &'a Catalog,
    /// 헤드리스 Host 에 붙었으면 그 Host 의 카탈로그. 앱에 붙었으면 `None`.
    pub headless: Option<&'a Catalog>,
}

/// 세션을 열고(`initialize`) 일을 하고 닫는다. `transport` 는 앱의 소켓 또는 헤드리스 Host 와의 duplex, `interrupt` 는 Ctrl+C.
/// 돌려주는 것은 종료 코드.
pub async fn execute<T, E, A>(
    request: Request<'_>,
    transport: T,
    interrupt: impl Future<Output = ()>,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> i32
where
    T: IntoTransport<RoleClient, E, A> + Send + 'static,
    E: std::error::Error + Send + Sync + 'static,
{
    tokio::pin!(interrupt);
    let config = ClientConfig::new(
        ClientCapabilities::default(),
        Implementation::new(CLIENT_NAME, env!("CARGO_PKG_VERSION")),
    );
    let client = tokio::select! {
        client = config.serve(transport) => match client {
            Ok(client) => client,
            Err(e) => {
                let _ = writeln!(err, "pitwall tool: the MCP session did not start: {e}");
                return exit::FAILED;
            }
        },
        () = &mut interrupt => return exit::INTERRUPTED,
    };
    let code = act(&client, request, interrupt, out, err).await;
    // 전송을 닫는다 — 서버의 세션이 끝나고, 취소 알림까지 보낸 바이트가 나간다.
    let _ = client.cancel().await;
    code
}

async fn act(
    client: &RunningService<RoleClient, ClientConfig>,
    request: Request<'_>,
    mut interrupt: std::pin::Pin<&mut impl Future<Output = ()>>,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> i32 {
    let listed = tokio::select! {
        listed = client.peer().list_all_tools() => listed,
        () = &mut interrupt => return exit::INTERRUPTED,
    };
    let listed = match listed {
        Ok(listed) => listed,
        Err(e) => return lost(&request, &e, err),
    };
    match request.action {
        Action::Usage | Action::Help => unreachable!("세션 없이 처리한다(run)"),
        Action::List => {
            list(&listed, request.linked, out);
            exit::OK
        }
        Action::ToolHelp(name) => {
            let (tool, note) = match find(name, &listed, &request) {
                Found::Listed(tool) => {
                    help(
                        name,
                        tier_of(tool, request.linked),
                        tool.description.as_deref(),
                        &tool.input_schema,
                        None,
                        out,
                    );
                    return exit::OK;
                }
                Found::NeedsApp(tool) => (
                    tool,
                    "needs the Pitwall app: start the Pitwall app to call it",
                ),
                Found::Unlisted(tool) => (tool, NOT_EXPOSED),
                Found::Unknown => return unknown(name, err),
            };
            help(
                name,
                tier_name(tool.tier),
                Some(&tool.description),
                &tool.input_schema_for(true),
                Some(note),
                out,
            );
            exit::OK
        }
        Action::Call(call) => {
            let (schema, tier) = match find(&call.tool, &listed, &request) {
                Found::Listed(tool) => (
                    tool.input_schema.as_ref().clone(),
                    request.linked.get(&call.tool).map(|t| t.tier),
                ),
                Found::NeedsApp(_) => {
                    let _ = writeln!(
                        err,
                        "pitwall tool: `{}` needs the Pitwall app; start the Pitwall app and try again",
                        call.tool
                    );
                    return exit::DENIED;
                }
                // 노출이 끈 툴: 서버가 거절과 감사 행을 남기도록 그대로 부른다(판정은 서버의 것이다).
                Found::Unlisted(tool) => (tool.input_schema_for(true), Some(tool.tier)),
                Found::Unknown => return unknown(&call.tool, err),
            };
            let args = match arguments(&call.tool, &schema, request.input.clone(), &call.flags) {
                Ok(args) => args,
                Err(e) => {
                    let _ = writeln!(err, "pitwall tool: {e}");
                    return exit::USAGE;
                }
            };
            let approval_note =
                request.headless.is_none() && tier.is_some_and(|t| t != pw_rpc::Tier::Read);
            call_tool(
                client,
                &call.tool,
                args,
                approval_note,
                &request,
                interrupt,
                out,
                err,
            )
            .await
        }
    }
}

enum Found<'a> {
    /// 이 세션의 `tools/list` 에 있다.
    Listed(&'a rmcp::model::Tool),
    /// 헤드리스 세션인데 링크된 카탈로그에만 있다 — 앱이 있어야 하는 툴.
    NeedsApp(&'a crate::Tool),
    /// 링크된 카탈로그에 있고 이 세션의 Host 도 처리하지만 노출 설정이 껐다.
    Unlisted(&'a crate::Tool),
    Unknown,
}

fn find<'a>(name: &str, listed: &'a [rmcp::model::Tool], request: &Request<'a>) -> Found<'a> {
    if let Some(tool) = listed.iter().find(|t| t.name == name) {
        return Found::Listed(tool);
    }
    let Some(linked) = request.linked.get(name) else {
        return Found::Unknown;
    };
    match request.headless {
        Some(headless) if headless.get(name).is_none() => Found::NeedsApp(linked),
        _ => Found::Unlisted(linked),
    }
}

/// 등급: 링크된 카탈로그에서. 이 실행 파일이 모르는 툴(다른 버전의 앱)이면 MCP 의 `readOnlyHint` 로 `read` 만 안다.
fn tier_of(tool: &rmcp::model::Tool, linked: &Catalog) -> &'static str {
    match linked.get(&tool.name) {
        Some(t) => tier_name(t.tier),
        None if tool
            .annotations
            .as_ref()
            .and_then(|a| a.read_only_hint)
            .unwrap_or(false) =>
        {
            "read"
        }
        None => "?",
    }
}

fn list(listed: &[rmcp::model::Tool], linked: &Catalog, out: &mut dyn Write) {
    let mut tools: Vec<_> = listed.iter().collect();
    tools.sort_by(|a, b| a.name.cmp(&b.name));
    let width = tools.iter().map(|t| t.name.len()).max().unwrap_or(0);
    for tool in tools {
        let summary = tool
            .description
            .as_deref()
            .and_then(|d| d.lines().map(str::trim).find(|l| !l.is_empty()))
            .unwrap_or("");
        let _ = writeln!(
            out,
            "{:<width$}  {:<9}  {summary}",
            tool.name,
            tier_of(tool, linked)
        );
    }
}

fn help(
    name: &str,
    tier: &str,
    description: Option<&str>,
    schema: &Map<String, Value>,
    note: Option<&str>,
    out: &mut dyn Write,
) {
    let _ = writeln!(out, "pitwall tool {name}  ({tier})");
    if let Some(note) = note {
        let _ = writeln!(out, "  `{name}` {note}");
    }
    if let Some(description) = description.filter(|d| !d.trim().is_empty()) {
        let _ = writeln!(out);
        for line in description.trim().lines() {
            let _ = writeln!(out, "{line}");
        }
    }
    let flags = flags(schema);
    let mut rows: Vec<(String, String)> = flags
        .iter()
        .map(|f| {
            let mut text = if f.required {
                "required".to_owned()
            } else {
                "optional".to_owned()
            };
            if let Some(first) = f
                .description
                .as_deref()
                .and_then(|d| d.lines().map(str::trim).find(|l| !l.is_empty()))
            {
                text.push_str(" — ");
                text.push_str(first);
            }
            (format!("--{} {}", f.name, f.value_label()), text)
        })
        .collect();
    rows.push((
        "--input <json>|-".into(),
        "the whole input object (`-` reads stdin); flags override its fields".into(),
    ));
    rows.push((
        "--project <path>".into(),
        "the project (default: the current directory)".into(),
    ));
    let width = rows
        .iter()
        .map(|(l, _)| l.chars().count())
        .max()
        .unwrap_or(0);
    let _ = writeln!(out, "\nflags:");
    for (left, right) in rows {
        let pad = width - left.chars().count();
        let _ = writeln!(out, "  {left}{:pad$}  {right}", "");
    }
}

fn unknown(name: &str, err: &mut dyn Write) -> i32 {
    let _ = writeln!(
        err,
        "pitwall tool: unknown tool `{name}`; `pitwall tool list` shows the tools"
    );
    exit::USAGE
}

fn lost(request: &Request<'_>, error: &ServiceError, err: &mut dyn Write) -> i32 {
    let peer = if request.headless.is_some() {
        "the headless MCP session"
    } else {
        "the Pitwall app"
    };
    let _ = writeln!(err, "pitwall tool: lost the connection to {peer}: {error}");
    exit::FAILED
}

#[allow(clippy::too_many_arguments)] // 호출 하나의 문맥이다.
async fn call_tool(
    client: &RunningService<RoleClient, ClientConfig>,
    name: &str,
    args: Map<String, Value>,
    approval_note: bool,
    request: &Request<'_>,
    mut interrupt: std::pin::Pin<&mut impl Future<Output = ()>>,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> i32 {
    let params = CallToolRequestParams::new(name.to_owned()).with_arguments(args);
    let handle = match client
        .peer()
        .send_cancellable_request(
            ClientRequest::CallToolRequest(CallToolRequest::new(params)),
            PeerRequestOptions::no_options(),
        )
        .await
    {
        Ok(handle) => handle,
        Err(e) => return lost(request, &e, err),
    };
    enum Next {
        Answer(Box<Result<ServerResult, ServiceError>>),
        Closed,
        Interrupted,
        Waiting,
    }
    let mut handle = handle;
    let note = tokio::time::sleep(APPROVAL_NOTE_AFTER);
    tokio::pin!(note);
    let mut noted = !approval_note;
    let answer = loop {
        let next = tokio::select! {
            answer = &mut handle.rx => match answer {
                Ok(answer) => Next::Answer(Box::new(answer)),
                Err(_) => Next::Closed,
            },
            () = &mut interrupt => Next::Interrupted,
            () = &mut note, if !noted => Next::Waiting,
        };
        match next {
            Next::Answer(answer) => break *answer,
            Next::Closed => return lost(request, &ServiceError::TransportClosed, err),
            Next::Interrupted => {
                // 서버가 그 호출의 Host RPC 를 거둔다(M5-03 의 취소 길). 알림을 보낸 뒤 끝낸다.
                if let Err(e) = handle.cancel(Some("interrupted".into())).await {
                    tracing::warn!(%e, "cannot send the cancellation");
                }
                let _ = writeln!(err, "pitwall tool: interrupted; cancelled `{name}`");
                return exit::INTERRUPTED;
            }
            Next::Waiting => {
                noted = true;
                let _ = writeln!(
                    err,
                    "pitwall tool: waiting for `{name}`; if Pitwall asks for approval, answer it in the project window"
                );
            }
        }
    };
    match answer {
        Ok(ServerResult::CallToolResult(result)) => finish(name, result, out, err),
        Ok(other) => {
            let _ = writeln!(
                err,
                "pitwall tool: unexpected answer to `{name}`: {other:?}"
            );
            exit::FAILED
        }
        Err(ServiceError::McpError(e)) => {
            let mark = e
                .data
                .as_ref()
                .and_then(Value::as_object)
                .and_then(Mark::read);
            match mark {
                Some(Mark {
                    outcome: Outcome::Denied,
                    reason,
                }) => {
                    if reason == Some(Reason::NotExposed) {
                        let _ = writeln!(err, "pitwall tool: `{name}` {NOT_EXPOSED}");
                    } else {
                        let _ = writeln!(err, "pitwall tool: {}", e.message);
                    }
                    exit::DENIED
                }
                _ => {
                    let _ = writeln!(err, "pitwall tool: {}", e.message);
                    exit::FAILED
                }
            }
        }
        Err(e) => lost(request, &e, err),
    }
}

/// `tools/call` 의 결과: 성공이면 stdout 에 JSON 문서 하나, 아니면 stderr 에 이유와 표지에 따른 코드.
fn finish(name: &str, result: CallToolResult, out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    let mark = result.meta.as_ref().and_then(|m| Mark::read(&m.0));
    let text: String = result
        .content
        .iter()
        .filter_map(|c| c.as_text().map(|t| t.text.as_str()))
        .collect();
    if result.is_error == Some(true) {
        let _ = writeln!(err, "pitwall tool: `{name}`: {text}");
        return match mark {
            Some(Mark {
                outcome: Outcome::Denied,
                ..
            }) => exit::DENIED,
            _ => exit::FAILED,
        };
    }
    let document = match result.structured_content {
        Some(value) => value,
        None => serde_json::from_str(&text).unwrap_or(Value::String(text)),
    };
    let rendered = serde_json::to_string_pretty(&document).expect("JSON 값은 직렬화된다");
    if writeln!(out, "{rendered}")
        .and_then(|()| out.flush())
        .is_err()
    {
        // stdout 이 닫혔다(`| head` 등). 결과는 받았으니 호출은 성공이다.
        tracing::debug!("stdout closed before the result was written");
    }
    exit::OK
}

/// 인자를 읽고 `--input` 을 읽는다 — 세션을 열기 전의 일. 쓰는 법 · `--help` 면 그 출력을 하고 종료 코드를 `Err` 로 돌려준다.
pub fn begin(args: &[OsString]) -> Result<(Invocation, Option<Value>), i32> {
    let invocation = match parse(args) {
        Ok(invocation) => invocation,
        Err(e) => {
            eprintln!("pitwall tool: {e}\n\n{USAGE}");
            return Err(exit::USAGE);
        }
    };
    let input = match &invocation.action {
        Action::Usage => {
            eprintln!("{USAGE}");
            return Err(exit::USAGE);
        }
        Action::Help => {
            println!("{USAGE}");
            return Err(exit::OK);
        }
        Action::Call(Call {
            input: Some(input), ..
        }) => match read_input(input, std::io::stdin()) {
            Ok(value) => Some(value),
            Err(e) => {
                eprintln!("pitwall tool: {e}");
                return Err(exit::USAGE);
            }
        },
        _ => None,
    };
    Ok((invocation, input))
}

/// Ctrl+C 를 기다린다(받을 수 없으면 영영 기다린다 — 기본 동작인 프로세스 종료가 남는다).
pub async fn interrupted() {
    if tokio::signal::ctrl_c().await.is_err() {
        std::future::pending::<()>().await;
    }
}
