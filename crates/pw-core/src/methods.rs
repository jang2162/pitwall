//! Host 가 직접 처리하는 RPC 메서드. 지금은 왕복 확인용 최소한만 둔다 — 기능 메서드는 각 모듈이 선언한다.

use pw_rpc::{JsNumber, rpc_method};
use serde::{Deserialize, Serialize};

use crate::task::{TaskId, TaskProgress, TaskStatus};

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct PingParams {
    /// 그대로 돌려받는 값. 클라이언트가 응답을 자기 요청과 맞춰 보는 데 쓴다.
    #[serde(default)]
    pub nonce: String,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct Pong {
    pub nonce: String,
    pub host_id: String,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct HostInfo {
    pub host_id: String,
    /// `local`(데스크톱 프로세스 안) 또는 `relay`(원격 데몬).
    pub kind: String,
    pub version: String,
    pub os: String,
    pub arch: String,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DemoTaskParams {
    pub steps: u32,
    #[specta(type = JsNumber)]
    pub interval_ms: u64,
}

impl DemoTaskParams {
    /// 데모가 Host 를 오래 붙잡지 못하게 묶는 범위.
    pub const MAX_STEPS: u32 = 1000;
    pub const INTERVAL_MS: std::ops::RangeInclusive<u64> = 10..=5000;
}

/// 데모 작업 스트림의 청크. 진행 알림이 여러 번, 종료 알림이 마지막에 한 번 온다.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
// rename_all 은 변형 이름만 바꾼다. 필드도 다른 메시지처럼 camelCase 로 나가게 rename_all_fields 를 함께 둔다.
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum DemoTaskUpdate {
    Progress {
        #[specta(type = JsNumber)]
        task_id: TaskId,
        progress: TaskProgress,
    },
    Finished {
        #[specta(type = JsNumber)]
        task_id: TaskId,
        // 펼치지(flatten) 않고 중첩한다. specta 는 enum 변형 안의 flatten 을 무시해 TS 타입이 와이어와 달라진다.
        outcome: TaskStatus,
    },
}

rpc_method! {
    /// 연결 확인. 받은 `nonce` 와 응답한 Host 의 id 를 돌려준다.
    pub HostPing: "host.ping" {
        params: PingParams,
        result: Pong,
        tier: Read,
    }

    /// Host 의 종류 · 버전 · OS.
    pub HostInfoMethod: "host.info" {
        params: (),
        result: HostInfo,
        tier: Read,
    }

    /// 진행률을 내는 데모 작업을 띄우고 그 진행을 스트림으로 보낸다. 스트림을 취소하면 작업도 취소된다.
    /// Task · 이벤트 버스 · 스트림 취소가 transport 끝까지 이어지는지 확인하는 용도다.
    pub HostDemoTask: "host.demoTask" {
        params: DemoTaskParams,
        stream: DemoTaskUpdate,
        tier: Read,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn demo_task_update_wire_format_is_camel_case() {
        let progress = DemoTaskUpdate::Progress {
            task_id: 7,
            progress: TaskProgress {
                done: 1,
                total: Some(2),
                message: None,
            },
        };
        assert_eq!(
            serde_json::to_value(&progress).unwrap(),
            json!({"kind": "progress", "taskId": 7, "progress": {"done": 1, "total": 2, "message": null}})
        );
        let finished = DemoTaskUpdate::Finished {
            task_id: 7,
            outcome: TaskStatus::Failed {
                error: "boom".into(),
            },
        };
        let wire = json!({"kind": "finished", "taskId": 7, "outcome": {"status": "failed", "error": "boom"}});
        assert_eq!(serde_json::to_value(&finished).unwrap(), wire);
        assert_eq!(
            serde_json::from_value::<DemoTaskUpdate>(wire).unwrap(),
            finished
        );
    }
}
