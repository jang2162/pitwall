//! 다른 머신을 겨누는 호출(05-mcp.md 「체크아웃」 · 「다른 머신을 겨누는 호출」, 18-m7-plan M7-18).
//!
//! - **`checkout` 입력**(메인 판정 M7-P10): 세션이 입력에 `worktree` 를 받는 툴의 스키마에 선택 입력 `checkout { machine, worktree? }`
//!   를 더하고, 호출 때 떼어 낸다. **Host 는 `checkout` 을 모른다**(D57 ④) — 원격 Host 로 가는 입력은 그 머신 레지스트리의
//!   프로젝트 id 와 `worktree` 뿐이다.
//! - **묶음 풀기**: 세션의 `projectId` 는 Desktop 쪽 이름(로컬 프로젝트 id 또는 묶음 id)이다. 다른 머신의 프로젝트 id 는 받지
//!   않는다(머신 사이에 유일하지 않다 — M7-03 ⑥). 묶음과 머신 이름 · 링크는 Desktop 만 안다 — [`Machines`] 가 그것을 꽂는 자리이고,
//!   없으면(헤드리스 · 시험) 다른 머신을 겨누는 호출은 모두 거절한다(fail-closed).
//! - **연결 풀**: 세션은 머신마다 연결을 하나씩 둔다 — 처음 겨눌 때 [`Machines::connect`] 로 `Agent` 범위 연결을 열고(원격 Host 의
//!   `agent_grant` · `agent_run_grant` 가 그대로 돈다) 세션이 끝날 때까지 쥔다. 링크를 열어야 하는데 사람에게 묻는 질문(암호 ·
//!   호스트 키 · 처음 보는 머신)이 필요하면 열지 않는다(메인 판정 — 에이전트가 사람 화면에 SSH 질문을 띄우지 않는다).
//! - **승인**(결정 M7-D9): 다른 머신 호출에는 `auto_approve` · 정책 자동을 적용하지 않는다 — 원격이 낸 계획의 `policyAuto` ·
//!   `auto_approve` 는 넓히는 데 읽지 않는다(M7-P19). 「이 세션 동안 이 머신 허용」 은 그 세션의 그 머신 `write` · `exec` 에
//!   닿는다. 판정은 `server` 의 `approve_remote`.

use std::collections::HashMap;
use std::sync::Arc;

use pw_core::registry::bundle::{LOCAL_MACHINE, ProjectBundle};
use pw_core::{BoxFuture, Connection};
use serde_json::{Map, Value, json};

use crate::client::HostClient;

/// 세션이 더하는 선택 입력의 꼭대기 키.
pub const CHECKOUT: &str = "checkout";

/// 다른 머신과 묶음 · 그 머신의 링크를 아는 것(Desktop 이 꽂는다 — `Services::machines`).
pub trait Machines: Send + Sync {
    /// Desktop 의 프로젝트 이름(로컬 프로젝트 id 또는 묶음 id)의 묶음. 모르는 이름이면 `None`. 로컬 프로젝트 하나뿐이면 암묵 묶음.
    fn bundle(&self, project_id: &str) -> Result<Option<ProjectBundle>, String>;
    /// 모든 묶음(`project_list` 가 묶음을 실을 때).
    fn bundles(&self) -> Result<Vec<ProjectBundle>, String>;
    /// 에이전트가 적은 머신(별명 · 머신 id · `local`)의 머신 id. 모르면 `None`.
    fn machine_id(&self, name: &str) -> Option<String>;
    /// 머신 id 의 사람 이름(Desktop 의 별명 — Relay 가 댄 이름이 아니다, M7-P19). 모르면 `None`.
    fn machine_name(&self, machine: &str) -> Option<String>;
    /// 그 머신 Host 에 `Agent` 범위 연결을 연다. 링크가 없으면 사람에게 묻지 않고 붙을 수 있을 때만 붙인다.
    fn connect<'a>(&'a self, machine: &'a str) -> BoxFuture<'a, Result<MachineLink, MachineError>>;
}

/// 열린 다른 머신 연결. `hold` 를 버리면 그 링크를 놓는다(세션이 끝날 때).
pub struct MachineLink {
    pub connection: Connection,
    pub hold: Box<dyn Send + Sync>,
}

/// 다른 머신에 붙지 못한 까닭(에이전트에게 가는 영어 문장).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MachineError {
    /// 링크를 열려면 사람이 답할 것이 있다(암호 · 호스트 키 · 처음 보는 머신 · 사람이 끊어 둠).
    NeedsPerson(String),
    /// 닿지 못했다(끊김 · 시한 · 그 밖).
    Unreachable(String),
}

impl MachineError {
    pub fn message(&self) -> &str {
        match self {
            MachineError::NeedsPerson(m) | MachineError::Unreachable(m) => m,
        }
    }
}

/// 에이전트가 준 `checkout`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckoutInput {
    pub machine: String,
    pub worktree: Option<String>,
}

/// 입력에서 `checkout` 을 떼어 읽는다. 없거나 `null` 이면 `None`.
pub fn take(args: &mut Map<String, Value>) -> Result<Option<CheckoutInput>, String> {
    let Some(value) = args.remove(CHECKOUT) else {
        return Ok(None);
    };
    let object = match value {
        Value::Null => return Ok(None),
        Value::Object(object) => object,
        _ => return Err("`checkout` must be an object { machine, worktree? }".into()),
    };
    if let Some(extra) = object
        .keys()
        .find(|k| k.as_str() != "machine" && k.as_str() != "worktree")
    {
        return Err(format!(
            "`checkout` has an unknown field `{extra}`; it takes `machine` and `worktree`"
        ));
    }
    let machine = match object.get("machine") {
        Some(Value::String(m)) if !m.trim().is_empty() => m.trim().to_owned(),
        _ => return Err("`checkout.machine` must be a machine name".into()),
    };
    let worktree = match object.get("worktree") {
        None | Some(Value::Null) => None,
        Some(Value::String(w)) if w.is_empty() => None,
        Some(Value::String(w)) => Some(w.clone()),
        Some(_) => return Err("`checkout.worktree` must be a path".into()),
    };
    Ok(Some(CheckoutInput { machine, worktree }))
}

/// 툴 입력 스키마에 더할 `checkout` 속성.
pub fn schema() -> Value {
    json!({
        "type": "object",
        "description": "Target another checkout of this project (another machine of its bundle, see project_list). \
    `machine` is the machine name shown in Pitwall (or `local` for this computer); `worktree` is the worktree path on that \
    machine (empty for its main checkout). Without it the call targets this session's checkout. `projectId` stays this \
    Desktop's project or bundle id: another machine's project ids are not accepted. Writes and runs on another machine ask \
    for approval once per machine in a session; `auto_approve` does not apply there.",
        "properties": {
            "machine": { "type": "string", "description": "Machine name shown in Pitwall, or `local`." },
            "worktree": {
                "type": ["string", "null"],
                "description": "Worktree path on that machine. Empty or null for the main checkout."
            }
        },
        "required": ["machine"],
        "additionalProperties": false
    })
}

/// `checkout` 을 더한 스키마.
pub fn with_checkout(mut schema: Map<String, Value>) -> Map<String, Value> {
    if let Some(Value::Object(properties)) = schema.get_mut("properties") {
        properties.insert(CHECKOUT.to_owned(), self::schema());
    }
    schema
}

/// 원격 대상 하나(승인 · 감사가 쓰는 것).
#[derive(Clone)]
pub struct RemoteAim {
    /// 머신 id.
    pub machine: String,
    /// 그 머신의 프로젝트 id(멤버).
    pub project: String,
    /// 승인을 물을 창의 프로젝트(묶음의 로컬 멤버 — 창이 그 id 로 듣는다). 감사 행의 프로젝트도 이것이다.
    pub routing: String,
    pub link: Arc<RemoteLink>,
}

/// 풀의 연결 하나.
pub struct RemoteLink {
    pub client: HostClient,
    _hold: Box<dyn Send + Sync>,
}

/// 세션의 연결 풀(머신 id → 연결). 로컬 Host 연결은 세션의 `client` 이고 여기 들지 않는다.
#[derive(Default)]
pub struct Pool {
    links: tokio::sync::Mutex<HashMap<String, Arc<RemoteLink>>>,
}

impl Pool {
    /// 그 머신의 연결. 없으면 연다 — 같은 머신을 동시에 여는 호출은 하나만 연다.
    pub async fn get(
        &self,
        machines: &dyn Machines,
        machine: &str,
    ) -> Result<Arc<RemoteLink>, MachineError> {
        let mut links = self.links.lock().await;
        if let Some(link) = links.get(machine)
            && !link.client.closed()
        {
            return Ok(Arc::clone(link));
        }
        let MachineLink { connection, hold } = machines.connect(machine).await?;
        let link = Arc::new(RemoteLink {
            client: HostClient::new(connection),
            _hold: hold,
        });
        links.insert(machine.to_owned(), Arc::clone(&link));
        Ok(link)
    }

    /// 연 머신 수(시험).
    pub async fn opened(&self) -> usize {
        self.links.lock().await.len()
    }
}

/// 이 머신인가(`local` 또는 머신 이름 풀이가 이 머신).
pub fn is_local(machine: &str) -> bool {
    machine == LOCAL_MACHINE
}

/// 감사 인자 요약에 실을 대상 체크아웃(머신 id · 워크트리).
pub fn audit_value(machine: &str, worktree: Option<&str>) -> Value {
    json!({ "machine": machine, "worktree": worktree })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: Value) -> Map<String, Value> {
        v.as_object().unwrap().clone()
    }

    #[test]
    fn checkout_is_taken_out_of_the_input() {
        let mut a =
            args(json!({"projectId": "p", "checkout": {"machine": "mini", "worktree": "/w"}}));
        let c = take(&mut a).unwrap().unwrap();
        assert_eq!(c.machine, "mini");
        assert_eq!(c.worktree.as_deref(), Some("/w"));
        assert!(!a.contains_key(CHECKOUT));
    }

    #[test]
    fn a_bad_checkout_is_refused() {
        for bad in [
            json!("mini"),
            json!({}),
            json!({"machine": ""}),
            json!({"machine": "m", "host": "x"}),
            json!({"machine": "m", "worktree": 3}),
        ] {
            let mut a = args(json!({ "checkout": bad }));
            assert!(take(&mut a).is_err(), "{a:?}");
        }
        let mut a = args(json!({ "checkout": null }));
        assert_eq!(take(&mut a).unwrap(), None);
    }
}
