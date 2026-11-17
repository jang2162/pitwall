//! 툴 메서드의 **계획**과 승인 해시(결정 M5-D30 — 16-m5-plan.md M5-12 ②).
//!
//! 승인을 청하는 툴 메서드(`write` 이상 등급의 툴 전부, 그리고 호출마다 승인을 청하는 툴 — 「이 연결 열기」 · 프로덕션 `ask` 의
//! 조회)는 실행 전에 [`ToolPlan`] 을 만들고 [`approve`] 를 지난다. 승인 경로(`pw_mcp::approval`) · 정책은 `pw-mcp` 에 있고, 타입이
//! 여기 있는 것은 툴 메서드를 가진 크레이트(pw-core · pw-open-with · pw-db)가 `pw-mcp` 에 의존하지 않아서다(메인 판정 2026-11-17).
//!
//! # 두 단계(D45 의 `confirmed` 와 같은 꼴)
//!
//! 1. MCP 세션이 승인 표 없이 툴을 부른다. 툴은 계획을 만들어 [`APPROVAL_REQUIRED`] 오류(`data` 는 [`ApprovalRequired`])로
//!    답하고 **아무 부작용 없이** 돌아온다 — 계획을 만드는 것 말고는 하지 않는다(파일 · 레지스트리 · DB 에 쓰지 않는다).
//!    입력이 틀려 계획을 만들 수 없으면 그 오류를 그대로 낸다(승인을 묻기 전에 거절).
//! 2. 세션이 사람에게 묻고(또는 `auto_approve` · 세션 허용) 승인 표([`ApprovalTicket`] — 해시)를 입력의 예약 키
//!    [`APPROVAL_KEY`] 에 실어 다시 부른다. 툴은 계획을 **다시 만들고** 해시가 같을 때만 돈다. 다르면(승인 뒤 파일 · 대상이 바뀜)
//!    다시 [`APPROVAL_REQUIRED`] 이고 `mismatch` 가 `true` 다 — 세션은 다시 묻지 않고 거절한다(`plan_changed`).
//!    **[`approve`] 를 지난 뒤의 부작용은 계획을 만들 때 읽은 값(해시에 넣은 값)으로 한다 — 다시 읽지 않는다.** 다시 읽으면
//!    `approve` 와 부작용 사이에 바뀐 것이 돈다. 파일을 고치는 툴은 쓰기 직전에 파일 내용이 계획 때 읽은 그 값인지 비교하고, DB
//!    툴은 계획 때 푼 대상으로 연다(16-m5-plan M5-16 · 17 · 18 이 이 규약을 따른다).
//!
//! # 해시가 덮는 것
//!
//! 툴의 RPC 이름 · 등급 · **서버가 채운 뒤의 입력 전체**(`projectId` · `worktree` 를 채운 typed 입력) · 툴이 더하는 **대상 값**
//! ([`PlanBuilder::target`] — DB 는 연결의 푼 대상 · 방언 · 프로덕션 · read-only 표지, 파일을 고치는 툴은 고치기 전 파일 내용) ·
//! 보이는 요약 · 바뀔 것 · 승인 규칙. 키 순서와 무관하게 같은 값은 같은 해시다(키를 정렬해 직렬화한다).
//!
//! # 예약 키
//!
//! 승인 표를 싣는 자리는 툴 입력 스키마에 없다. 툴은 입력 타입을 [`Planned`] 로 감싸고, [`Planned`] 의 JSON Schema 는 감싼
//! 타입의 것 그대로에 표지 [`PLANNED_MARK`] 만 더한다 — 카탈로그(`pw_mcp::catalog`)가 그 표지를 읽어 지우고, 표지 없는 `write`
//! 이상 툴은 시작 때 실패한다(fail-closed). 세션은 에이전트가 보낸 [`APPROVAL_KEY`] 를 늘 버린다 — 에이전트가 승인을 스스로 실을
//! 수 없다. 창의 `confirmed`(DB 의 위험 쿼리 확인)는 MCP 툴 입력에 두지 않는다(14-m3-plan.md 결정 M3-D19).

use std::borrow::Cow;

use pw_rpc::{RpcError, Tier};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

/// 승인 표를 싣는 입력의 예약 키. 세션만 싣고, 에이전트가 보낸 같은 키는 세션이 버린다.
pub const APPROVAL_KEY: &str = "pitwall/approval";

/// [`Planned`] 입력의 JSON Schema 루트에 붙는 표지. 카탈로그가 읽고 툴 목록에 보이는 스키마에서는 지운다.
pub const PLANNED_MARK: &str = "x-pitwall-planned";

/// 「승인이 있어야 한다」 — 계획을 실은 오류(`data` 는 [`ApprovalRequired`]). `pw-mcp` 의 범위(6001~)다. MCP 세션 안에서만
/// 쓰이고 에이전트에게 그대로 나가지 않는다(세션이 묻지 못하면 정해진 거절 까닭으로 바꾼다).
pub const APPROVAL_REQUIRED: i32 = 6001;

/// 툴이 만든 계획. 승인 요청이 그대로 보인다.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct ToolPlan {
    /// 툴의 RPC 이름.
    pub method: String,
    pub tier: Tier,
    /// 한 줄 요약(영어가 아니어도 된다 — 사람의 화면이 보인다. 에이전트가 보낸 글이 섞이면 화면이 거른다).
    pub summary: String,
    /// 바뀔 것(파일 diff · 문장 목록 등). 빈 목록이면 요약만 보인다.
    pub changes: Vec<PlanChange>,
    pub rules: PlanRules,
    /// `sha256:<hex>`. 승인은 이것에 묶인다.
    pub hash: String,
}

/// 바뀔 것 하나. 다른 꼴이 필요한 툴 행이 갈래를 더한다.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum PlanChange {
    /// 파일 하나의 앞뒤 내용. 없던 파일이면 `before` 가, 지우는 파일이면 `after` 가 `null`.
    File {
        path: String,
        before: Option<String>,
        after: Option<String>,
    },
    /// 이름 붙은 글(SQL 문장 · 명령 정의 등).
    Text { label: String, text: String },
}

/// 그 계획에 어떤 답이 열리는가.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct PlanRules {
    /// `[mcp] auto_approve` 를 적용하는가. `open_with_upsert` 와 「이 연결 열기」 는 `false`(결정 M5-D31). `dangerous` 는 이것과
    /// 상관없이 적용하지 않는다.
    pub auto_approve: bool,
    /// 「이 세션 동안 허용」(CLI 면 「창이 열려 있는 동안 CLI 허용」)이 무엇을 기억하는가. `dangerous` 는 이것과 상관없이 없다.
    pub remember: Remember,
}

impl Default for PlanRules {
    fn default() -> Self {
        Self {
            auto_approve: true,
            remember: Remember::Tool,
        }
    }
}

/// 세션 허용이 기억하는 단위.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Remember {
    /// 그 세션 · 그 프로젝트 · 그 툴(05 「권한 모델」).
    Tool,
    /// 그 세션 · 그 프로젝트 · 이 열쇠(「이 연결 열기」 — 그 연결 id 와 푼 대상).
    Key { key: String },
    /// 기억하지 않는다 — 셋째 버튼이 없다(`open_with_upsert`).
    Never,
}

/// 세션이 [`APPROVAL_KEY`] 에 싣는 승인 표.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalTicket {
    /// 사람(또는 정책)이 승인한 계획의 해시.
    pub hash: String,
}

/// [`APPROVAL_REQUIRED`] 오류의 `data`.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalRequired {
    /// 지금 입력 · 대상으로 다시 만든 계획.
    pub plan: ToolPlan,
    /// 승인 표가 있었는데 그 해시가 이 계획과 다르다(승인 뒤 계획이 바뀌었다).
    pub mismatch: bool,
}

/// 계획을 만든다. [`ToolPlan::builder`].
#[derive(Debug, Clone)]
pub struct PlanBuilder {
    method: String,
    tier: Tier,
    input: Value,
    summary: String,
    changes: Vec<PlanChange>,
    targets: Map<String, Value>,
    rules: PlanRules,
}

impl ToolPlan {
    /// `input` 은 **서버가 채운 뒤의 typed 입력**이다(예약 키는 [`Planned`] 가 이미 떼었다).
    pub fn builder(method: &str, tier: Tier, input: &impl Serialize) -> PlanBuilder {
        PlanBuilder {
            method: method.to_owned(),
            tier,
            input: to_value(input),
            summary: String::new(),
            changes: Vec::new(),
            targets: Map::new(),
            rules: PlanRules::default(),
        }
    }
}

impl PlanBuilder {
    #[must_use]
    pub fn summary(mut self, summary: impl Into<String>) -> Self {
        self.summary = summary.into();
        self
    }

    #[must_use]
    pub fn change(mut self, change: PlanChange) -> Self {
        self.changes.push(change);
        self
    }

    /// 해시에 더하는 대상 값(보이지 않는다). 같은 키를 두 번 주면 뒤의 것이다.
    #[must_use]
    pub fn target(mut self, key: &str, value: &impl Serialize) -> Self {
        self.targets.insert(key.to_owned(), to_value(value));
        self
    }

    #[must_use]
    pub fn rules(mut self, rules: PlanRules) -> Self {
        self.rules = rules;
        self
    }

    pub fn build(self) -> ToolPlan {
        let covered = serde_json::json!({
            "method": self.method,
            "tier": self.tier,
            "input": self.input,
            "targets": self.targets,
            "summary": self.summary,
            "changes": self.changes,
            "rules": self.rules,
        });
        let mut hasher = Sha256::new();
        hasher.update(canonical(&covered).as_bytes());
        let hash = format!("sha256:{}", hex(&hasher.finalize()));
        ToolPlan {
            method: self.method,
            tier: self.tier,
            summary: self.summary,
            changes: self.changes,
            rules: self.rules,
            hash,
        }
    }
}

/// [`approve`] 를 지난 증거 — 이것이 있어야 툴이 돈다. 이 모듈 밖에서는 만들 수 없다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Approved {
    hash: String,
}

impl Approved {
    /// 승인된 계획의 해시(감사 상세 등).
    pub fn hash(&self) -> &str {
        &self.hash
    }
}

/// 승인 표가 지금 계획과 맞으면 [`Approved`], 아니면 계획을 실은 [`APPROVAL_REQUIRED`] 오류. 툴 메서드는 계획을 만든 뒤 부작용
/// 전에 이것을 지난다.
pub fn approve(ticket: Option<&ApprovalTicket>, plan: ToolPlan) -> Result<Approved, RpcError> {
    match ticket {
        Some(ticket) if ticket.hash == plan.hash => Ok(Approved { hash: plan.hash }),
        _ => Err(approval_required(plan, ticket.is_some())),
    }
}

/// [`APPROVAL_REQUIRED`] 오류.
pub fn approval_required(plan: ToolPlan, mismatch: bool) -> RpcError {
    RpcError::new(
        APPROVAL_REQUIRED,
        "this call needs approval in Pitwall before it runs",
    )
    .with_data(to_value(&ApprovalRequired { plan, mismatch }))
}

/// 승인을 청하는 툴 메서드의 입력: 감싼 입력 `params` 와 세션이 실은 승인 표. JSON 은 감싼 입력과 같은 object 에 [`APPROVAL_KEY`]
/// 가 더해진 것이고, 스키마 · TS 타입은 감싼 입력의 것이다(표는 스키마에 없다 — 머리 주석).
#[derive(Debug, Clone, PartialEq)]
pub struct Planned<P> {
    pub params: P,
    pub ticket: Option<ApprovalTicket>,
}

impl<P> Planned<P> {
    /// [`approve`] 를 이 입력의 표로.
    pub fn approve(&self, plan: ToolPlan) -> Result<Approved, RpcError> {
        approve(self.ticket.as_ref(), plan)
    }
}

impl<'de, P: DeserializeOwned> Deserialize<'de> for Planned<P> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        let mut map = Map::<String, Value>::deserialize(deserializer)?;
        let ticket = match map.remove(APPROVAL_KEY) {
            None | Some(Value::Null) => None,
            Some(value) => Some(serde_json::from_value(value).map_err(D::Error::custom)?),
        };
        let params = serde_json::from_value(Value::Object(map)).map_err(D::Error::custom)?;
        Ok(Self { params, ticket })
    }
}

impl<P: Serialize> Serialize for Planned<P> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::Error;
        let mut map = match serde_json::to_value(&self.params).map_err(S::Error::custom)? {
            Value::Object(map) => map,
            _ => return Err(S::Error::custom("Planned params must be an object")),
        };
        if let Some(ticket) = &self.ticket {
            map.insert(
                APPROVAL_KEY.to_owned(),
                serde_json::to_value(ticket).map_err(S::Error::custom)?,
            );
        }
        map.serialize(serializer)
    }
}

impl<P: schemars::JsonSchema> schemars::JsonSchema for Planned<P> {
    fn inline_schema() -> bool {
        true
    }

    fn schema_name() -> Cow<'static, str> {
        P::schema_name()
    }

    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        // 감싼 타입의 정의 그 자체(`$ref` 가 아니다)에 표지만 더한다.
        let mut schema = P::json_schema(generator);
        schema.insert(PLANNED_MARK.to_owned(), Value::Bool(true));
        schema
    }
}

impl<P: specta::Type> specta::Type for Planned<P> {
    fn definition(types: &mut specta::Types) -> specta::datatype::DataType {
        P::definition(types)
    }
}

fn to_value(value: &impl Serialize) -> Value {
    serde_json::to_value(value).expect("계획에 넣는 값은 JSON 으로 직렬화된다")
}

/// 키를 정렬한 JSON(공백 없음). 직렬화 라이브러리의 키 순서 설정과 무관하게 같은 값은 같은 글이다.
fn canonical(value: &Value) -> String {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let body: Vec<String> = keys
                .into_iter()
                .map(|k| format!("{}:{}", Value::String(k.clone()), canonical(&map[k])))
                .collect();
            format!("{{{}}}", body.join(","))
        }
        Value::Array(items) => {
            let body: Vec<String> = items.iter().map(canonical).collect();
            format!("[{}]", body.join(","))
        }
        other => other.to_string(),
    }
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::new(), |mut out, b| {
        let _ = write!(out, "{b:02x}");
        out
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
    #[serde(rename_all = "camelCase")]
    struct Input {
        project_id: String,
        #[serde(default)]
        worktree: Option<String>,
    }

    fn plan(input: &Input, content: &str) -> ToolPlan {
        ToolPlan::builder("test.write", Tier::Write, input)
            .summary("write a file")
            .target("content", &content)
            .build()
    }

    #[test]
    fn the_hash_covers_the_filled_input_and_the_target_values() {
        let a = Input {
            project_id: "p".into(),
            worktree: Some("/w1".into()),
        };
        let base = plan(&a, "x");
        assert!(base.hash.starts_with("sha256:"), "{}", base.hash);
        assert_eq!(plan(&a, "x").hash, base.hash, "같은 계획은 같은 해시");
        assert_ne!(plan(&a, "y").hash, base.hash, "대상 값(파일 내용)");
        let b = Input {
            worktree: Some("/w2".into()),
            ..a.clone()
        };
        assert_ne!(plan(&b, "x").hash, base.hash, "서버가 채운 worktree");
        let other_tool = ToolPlan::builder("test.other", Tier::Write, &a)
            .summary("write a file")
            .target("content", &"x")
            .build();
        assert_ne!(other_tool.hash, base.hash, "툴 이름");
        let rules = ToolPlan::builder("test.write", Tier::Write, &a)
            .summary("write a file")
            .target("content", &"x")
            .rules(PlanRules {
                auto_approve: false,
                remember: Remember::Never,
            })
            .build();
        assert_ne!(rules.hash, base.hash, "승인 규칙");
    }

    #[test]
    fn key_order_does_not_change_the_hash() {
        let one = canonical(&json!({"b": 1, "a": {"y": [1, {"d": 2, "c": 3}], "x": "s"}}));
        let two = canonical(&json!({"a": {"x": "s", "y": [1, {"c": 3, "d": 2}]}, "b": 1}));
        assert_eq!(one, two);
        assert_eq!(one, r#"{"a":{"x":"s","y":[1,{"c":3,"d":2}]},"b":1}"#);
    }

    #[test]
    fn approve_needs_a_ticket_with_the_same_hash() {
        let input = Input {
            project_id: "p".into(),
            worktree: None,
        };
        let p = plan(&input, "x");
        let err = approve(None, p.clone()).unwrap_err();
        assert_eq!(err.code, APPROVAL_REQUIRED);
        let data: ApprovalRequired = serde_json::from_value(err.data.unwrap()).unwrap();
        assert_eq!(
            data,
            ApprovalRequired {
                plan: p.clone(),
                mismatch: false
            }
        );

        let stale = ApprovalTicket {
            hash: plan(&input, "old").hash,
        };
        let err = approve(Some(&stale), p.clone()).unwrap_err();
        let data: ApprovalRequired = serde_json::from_value(err.data.unwrap()).unwrap();
        assert!(data.mismatch);

        let ok = approve(
            Some(&ApprovalTicket {
                hash: p.hash.clone(),
            }),
            p.clone(),
        )
        .unwrap();
        assert_eq!(ok.hash(), p.hash);
    }

    #[test]
    fn planned_takes_the_ticket_out_of_the_input_and_keeps_it_out_of_the_schema() {
        let planned: Planned<Input> = serde_json::from_value(json!({
            "projectId": "p",
            APPROVAL_KEY: {"hash": "sha256:1"},
        }))
        .unwrap();
        assert_eq!(planned.params.project_id, "p");
        assert_eq!(
            planned.ticket,
            Some(ApprovalTicket {
                hash: "sha256:1".into()
            })
        );
        let bare: Planned<Input> = serde_json::from_value(json!({"projectId": "p"})).unwrap();
        assert_eq!(bare.ticket, None);
        // 직렬화하면 같은 object 로 돌아온다.
        assert_eq!(
            serde_json::to_value(&planned).unwrap(),
            json!({"projectId": "p", "worktree": null, APPROVAL_KEY: {"hash": "sha256:1"}})
        );

        let schema = schemars::SchemaGenerator::default()
            .into_root_schema_for::<Planned<Input>>()
            .to_value();
        assert_eq!(schema[PLANNED_MARK], json!(true));
        let properties = schema["properties"].as_object().unwrap();
        assert!(properties.contains_key("projectId"));
        assert!(!properties.contains_key(APPROVAL_KEY), "{schema}");
        let plain = schemars::SchemaGenerator::default()
            .into_root_schema_for::<Input>()
            .to_value();
        assert!(plain.get(PLANNED_MARK).is_none());
    }
}
