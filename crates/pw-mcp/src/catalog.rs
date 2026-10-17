//! 툴 목록: `rpc_method!` 선언 중 `mcp: true` 인 것에서 만든다(`docs/design/05-mcp.md` 「툴 레지스트리」 절).
//!
//! 툴마다 스키마를 손으로 쓰지 않는다 — 이름 · 설명 · 등급은 [`MethodMeta`], 입출력 스키마는 선언의 Rust 타입(schemars)에서 온다.
//!
//! - 이름: RPC 이름의 `.` 을 `_` 로, camelCase 를 snake_case 로(`project.list` → `project_list`, `project.markOpened` →
//!   `project_mark_opened`). [`tool_name`].
//! - 입력: MCP 의 `inputSchema` 는 루트가 object 여야 한다. struct 입력은 그 스키마를 루트로 펼치고, `()` 입력은 빈 object 로
//!   내보내고 RPC 에는 `null` 을 보낸다. enum 등 그 밖의 루트는 툴로 만들 수 없다([`CatalogError`]).
//! - 출력: 결과 스키마의 루트가 object 일 때만 `outputSchema` 와 `structuredContent` 를 쓴다(MCP 규칙). 배열 등은 JSON 텍스트로만 준다.
//! - 프로젝트 지정: 입력에 `projectId` 가 있으면 프로젝트 툴이다. 세션에 프로젝트가 있으면(`pitwall mcp --project`, cwd) 그 키를
//!   필수에서 빼고 빠졌을 때 서버가 채운다([`Tool::input_schema_for`]).

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use pw_rpc::{MethodInfo, MethodKind, Tier};
use schemars::generate::SchemaSettings;
use serde_json::{Map, Value, json};

/// 프로젝트 툴이 받는 키(`docs/design/03-architecture.md` §4 「기능 메서드의 params 에는 projectId」).
pub const PROJECT_ID: &str = "projectId";

/// MCP 툴 이름의 최대 길이. 클라이언트가 서버 이름과 이어 붙이므로(`mcp__pitwall__project_list`) 짧게 둔다.
pub const MAX_NAME: usize = 64;

/// 툴 하나.
#[derive(Debug, Clone)]
pub struct Tool {
    pub name: String,
    /// 이 툴이 부르는 RPC 메서드.
    pub method: &'static str,
    pub tier: Tier,
    pub description: String,
    /// 입력이 `()` 라 RPC 에 `null` 을 보낸다.
    pub unit_params: bool,
    input_schema: Map<String, Value>,
    pub output_schema: Option<Arc<Map<String, Value>>>,
}

impl Tool {
    /// 입력에 [`PROJECT_ID`] 가 있는가.
    pub fn project_scoped(&self) -> bool {
        self.input_schema
            .get("properties")
            .and_then(Value::as_object)
            .is_some_and(|p| p.contains_key(PROJECT_ID))
    }

    /// 세션에 기본 프로젝트가 있으면 [`PROJECT_ID`] 를 필수에서 뺀 스키마.
    pub fn input_schema_for(&self, session_project: bool) -> Map<String, Value> {
        let mut schema = self.input_schema.clone();
        if session_project
            && self.project_scoped()
            && let Some(Value::Array(required)) = schema.get_mut("required")
        {
            required.retain(|r| r != PROJECT_ID);
            if required.is_empty() {
                schema.remove("required");
            }
        }
        schema
    }
}

#[cfg(test)]
impl Tool {
    /// 노출 · 정책 시험용 툴. 입력은 빈 object.
    pub(crate) fn fake(name: &str, tier: Tier) -> Self {
        Self {
            name: name.to_owned(),
            method: "test.fake",
            tier,
            description: String::new(),
            unit_params: true,
            input_schema: Map::new(),
            output_schema: None,
        }
    }
}

/// 툴 목록. 이름순.
#[derive(Debug, Clone, Default)]
pub struct Catalog {
    tools: Vec<Tool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CatalogError {
    /// 스트림 메서드(`rpc_method!` 가 이미 컴파일 타임에 막는다 — 손으로 만든 메타데이터용).
    Stream(&'static str),
    /// 입력 스키마의 루트가 object 도 `()` 도 아니다.
    InputNotObject(&'static str),
    /// 툴 이름이 규칙(`[a-z0-9_]`, 64자)에 맞지 않는다.
    BadName { method: &'static str, name: String },
    /// 두 메서드가 같은 툴 이름이 된다.
    Duplicate {
        name: String,
        first: &'static str,
        second: &'static str,
    },
}

impl fmt::Display for CatalogError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CatalogError::Stream(m) => write!(f, "`{m}`: stream methods cannot be MCP tools"),
            CatalogError::InputNotObject(m) => {
                write!(f, "`{m}`: MCP tool input must be a struct or `()`")
            }
            CatalogError::BadName { method, name } => {
                write!(f, "`{method}`: tool name `{name}` is not [a-z0-9_]{{1,64}}")
            }
            CatalogError::Duplicate {
                name,
                first,
                second,
            } => write!(f, "`{first}` and `{second}` both become tool `{name}`"),
        }
    }
}

impl std::error::Error for CatalogError {}

impl Catalog {
    /// 링크된 모든 크레이트의 선언에서(`pw_rpc::methods`). 실행 파일에 링크되지 않은 크레이트의 메서드는 빠진다.
    ///
    /// # Panics
    /// 선언이 툴 규칙을 어기면 패닉한다(프로그래밍 오류 — 시험이 잡는다).
    pub fn linked() -> Self {
        Self::from_methods(pw_rpc::methods()).unwrap_or_else(|e| panic!("MCP tool catalog: {e}"))
    }

    /// `mcp: true` 인 선언만 툴로 만든다. 나머지는 보지 않는다.
    pub fn from_methods<'a>(
        methods: impl IntoIterator<Item = &'a MethodInfo>,
    ) -> Result<Self, CatalogError> {
        let mut by_name: BTreeMap<String, Tool> = BTreeMap::new();
        for info in methods {
            if !info.meta.mcp {
                continue;
            }
            let tool = tool(info)?;
            if let Some(first) = by_name.get(&tool.name) {
                return Err(CatalogError::Duplicate {
                    name: tool.name,
                    first: first.method,
                    second: info.meta.name,
                });
            }
            by_name.insert(tool.name.clone(), tool);
        }
        Ok(Self {
            tools: by_name.into_values().collect(),
        })
    }

    /// Host 가 핸들러를 붙인 메서드의 툴만 남긴다. 실행 파일에 선언이 링크돼 있어도 그 Host(예: 헤드리스)가 등록하지 않은
    /// 메서드는 `tools/list` 에 내지 않는다 — 내면 부를 때 method-not-found 가 된다.
    #[must_use]
    pub fn routed_by(mut self, router: &pw_rpc::Router) -> Self {
        let routed: Vec<&str> = router.methods().iter().map(|m| m.name).collect();
        self.tools.retain(|t| routed.contains(&t.method));
        self
    }

    pub fn tools(&self) -> &[Tool] {
        &self.tools
    }

    pub fn get(&self, name: &str) -> Option<&Tool> {
        self.tools.iter().find(|t| t.name == name)
    }
}

/// RPC 이름 → 툴 이름.
pub fn tool_name(method: &str) -> String {
    let mut out = String::with_capacity(method.len() + 4);
    for c in method.chars() {
        match c {
            '.' => out.push('_'),
            c if c.is_ascii_uppercase() => {
                if !out.is_empty() && !out.ends_with('_') {
                    out.push('_');
                }
                out.push(c.to_ascii_lowercase());
            }
            c => out.push(c),
        }
    }
    out
}

fn valid_name(name: &str) -> bool {
    (1..=MAX_NAME).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

fn tool(info: &MethodInfo) -> Result<Tool, CatalogError> {
    let meta = &info.meta;
    if meta.kind == MethodKind::Stream {
        return Err(CatalogError::Stream(meta.name));
    }
    let name = tool_name(meta.name);
    if !valid_name(&name) {
        return Err(CatalogError::BadName {
            method: meta.name,
            name,
        });
    }
    let input = root_schema(|g| info.params_schema(g));
    let (unit_params, input_schema) = match input.get("type").and_then(Value::as_str) {
        Some("object") => (false, input),
        Some("null") => (
            true,
            json!({"type": "object", "properties": {}, "additionalProperties": false})
                .as_object()
                .cloned()
                .expect("object 리터럴"),
        ),
        _ => return Err(CatalogError::InputNotObject(meta.name)),
    };
    let output = root_schema(|g| info.output_schema(g));
    let output_schema =
        (output.get("type").and_then(Value::as_str) == Some("object")).then(|| Arc::new(output));
    Ok(Tool {
        name,
        method: meta.name,
        tier: meta.tier,
        description: description(meta.doc),
        unit_params,
        input_schema,
        output_schema,
    })
}

/// 스키마 하나를 루트로 펼친다. 루트가 `$ref` 면 그 정의를 루트에 놓고, 나머지 정의는 `$defs` 로 둔다.
fn root_schema(
    make: impl FnOnce(&mut schemars::SchemaGenerator) -> schemars::Schema,
) -> Map<String, Value> {
    let mut generator = SchemaSettings::draft2020_12().into_generator();
    let schema = make(&mut generator);
    let mut defs = generator.take_definitions(true);
    let mut root = match schema.as_value() {
        Value::Object(map) => map.clone(),
        // `true` 스키마(아무 값). 툴 입력으로는 쓰이지 않는다.
        _ => Map::new(),
    };
    if let Some(Value::String(reference)) = root.get("$ref").cloned()
        && let Some(name) = reference.strip_prefix("#/$defs/")
        && let Some(Value::Object(def)) = defs.remove(name)
    {
        root.remove("$ref");
        for (k, v) in def {
            root.entry(k).or_insert(v);
        }
    }
    if !defs.is_empty() {
        root.insert("$defs".into(), Value::Object(defs));
    }
    // `$schema` 는 넣지 않는다 — MCP 는 없으면 2020-12 로 본다. 툴 목록은 에이전트 컨텍스트에 들어가는 토큰이다.
    root.remove("$schema");
    root
}

/// 선언의 문서 주석(줄마다 앞에 공백 하나)을 다듬는다.
fn description(doc: &str) -> String {
    doc.lines()
        .map(str::trim)
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_turn_dots_and_camel_case_into_snake_case() {
        assert_eq!(tool_name("project.list"), "project_list");
        assert_eq!(tool_name("project.markOpened"), "project_mark_opened");
        assert_eq!(tool_name("db.query.execute"), "db_query_execute");
        assert!(valid_name("project_list"));
        assert!(!valid_name(""));
        assert!(!valid_name("dev.pitwall.x"));
        assert!(!valid_name(&"a".repeat(MAX_NAME + 1)));
    }
}
