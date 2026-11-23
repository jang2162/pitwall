//! Relay 의 기기 관리 RPC(18-m7-plan M7-23 ③ — 메인 판정 2026-11-24): 페어링 표 보기 · 기기 해제 · 다른 기기를 붙일 코드 띄우기.
//!
//! - **owner 기기만**(`roles: [Owner]`) — 낮은 role 기기(`mobile` · `readonly`)는 다른 기기를 페어링 · 해제하지 못한다. Desktop 쪽
//!   중계(`remote.devices` 등 — `pw_remote`)는 role 을 대신 판정하지 않는다 — 이 판정과 두 겹이다.
//! - Host 범위 연결만 — 프로젝트 · 에이전트 연결(원격 에이전트의 MCP 세션 포함)은 거절한다. MCP 툴이 아니다(`mcp` 를 달지 않는다).
//! - 해제는 표를 곧바로 바꾼다 — 그 기기의 열린 링크 · 구독 · 터미널 붙기 · 멈춘 링크의 이어 붙기 토큰까지 끊긴다
//!   (`crate::PairingTable::watch`). 자기 자신(이 링크의 기기)을 해제하면 이 답이 가기 전에 링크가 끊길 수 있다.
//! - 코드는 `relay.pairCode` 의 답으로만 나간다 — 로그 · 감사에 싣지 않는다(`crate::pair` 머리 주석).
//! - Relay 만 이 메서드를 낸다 — Desktop 의 Host 조립은 `relay.` 갈래를 걷는다(`pw_host`). 몸통([`DeviceAdmin`])은 데몬이
//!   붙인다(`pw_relay::devices`). 붙이기 전에는 「쓸 수 없음」 이다.

use std::sync::{Arc, OnceLock};

use pw_rpc::{ConnectionScope, Ctx, Role, Router, RpcError, code, rpc_method};
use serde::{Deserialize, Serialize};

/// 페어링된 기기 하나(기기 화면의 한 줄).
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct RelayDevice {
    /// 기기 id(공개키의 지문 — `SHA256:…`).
    pub device: String,
    /// 기기 공개키(base64).
    pub key: String,
    pub name: String,
    pub role: Role,
    /// 페어링한 시각(유닉스 ms). 이 행 전에 페어링한 기기는 모른다.
    #[serde(default)]
    #[specta(type = Option<pw_rpc::JsNumber>)]
    pub paired_at_ms: Option<u64>,
    /// 그 기기의 링크가 마지막으로 증명된 시각(유닉스 ms).
    #[serde(default)]
    #[specta(type = Option<pw_rpc::JsNumber>)]
    pub last_seen_ms: Option<u64>,
    /// 이 호출을 한 기기인가.
    pub you: bool,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
pub struct RelayDeviceList {
    pub devices: Vec<RelayDevice>,
    /// 이 Relay 의 직접 연결 듣기(18-m7-plan M7-24 ②). 듣지 않거나 모르면 없다.
    #[serde(default)]
    pub listen: Option<RelayListen>,
}

/// Relay 의 직접 연결 듣기(`relay.toml [listen]`). `wide` 면 `"all"` 을 골랐다 — 기기 화면이 경고한다.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct RelayListen {
    pub port: u16,
    /// 고른 값(`tailscale` · `lan` · `all`).
    pub interfaces: Vec<String>,
    pub wide: bool,
    /// 지금 듣는 주소(`ip:port`).
    pub addresses: Vec<String>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
pub struct RelayUnpairParams {
    /// 해제할 기기 id.
    pub device: String,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
pub struct RelayUnpairResult {
    /// 표에 있었는가.
    pub removed: bool,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
pub struct RelayPairCodeParams {
    /// 그 코드로 페어링되는 기기가 받을 role.
    pub role: Role,
    /// 그 기기의 이름(`[A-Za-z0-9._-]`, 64자까지). 없으면 기기가 댄 이름.
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct RelayPairCode {
    /// 6자리 코드(사람에게만 보인다 — 새 코드를 만들면 앞 코드는 끝난다).
    pub code: String,
    /// 만료 시각(유닉스 ms — 5분 뒤).
    #[specta(type = pw_rpc::JsNumber)]
    pub expires_at_ms: u64,
}

rpc_method! {
    /// 이 Relay 의 페어링 표: 기기 id · 이름 · role · 페어링 시각 · 마지막 접속. owner 기기만.
    pub RelayDevices: "relay.devices" {
        params: (),
        result: RelayDeviceList,
        tier: Read,
        roles: [Owner],
    }

    /// 기기 하나를 해제한다 — 그 기기의 열린 링크 · 구독 · 터미널 붙기 · 이어 붙기 토큰이 곧바로 끊긴다. owner 기기만.
    pub RelayUnpair: "relay.unpair" {
        params: RelayUnpairParams,
        result: RelayUnpairResult,
        tier: Write,
        roles: [Owner],
    }

    /// 다른 기기를 이 Relay 에 붙일 6자리 코드를 만든다(5분, 틀린 시도 세 번이면 끝, 한 번 쓰면 끝). 앞의 대기 코드는 끝난다. owner
    /// 기기만.
    pub RelayPairCodeIssue: "relay.pairCode" {
        params: RelayPairCodeParams,
        result: RelayPairCode,
        tier: Write,
        roles: [Owner],
    }
}

/// 몸통(데몬 — `pw_relay::devices`). `caller` 는 이 호출을 한 기기 id(링크 창 label 의 접두)다.
pub trait DeviceAdmin: Send + Sync + 'static {
    fn list(&self, caller: Option<&str>) -> std::io::Result<Vec<RelayDevice>>;
    fn unpair(&self, device: &str) -> std::io::Result<bool>;
    fn pair_code(&self, role: Role, name: Option<&str>) -> std::io::Result<RelayPairCode>;
    /// 직접 연결 듣기(M7-24). 기본은 모름.
    fn listen(&self) -> Option<RelayListen> {
        None
    }
}

/// 몸통을 붙일 자리. 복제는 같은 자리를 본다.
#[derive(Clone, Default)]
pub struct Devices(Arc<OnceLock<Arc<dyn DeviceAdmin>>>);

impl Devices {
    /// 몸통을 붙인다(한 번만 — 두 번째는 무시한다).
    pub fn attach(&self, admin: Arc<dyn DeviceAdmin>) {
        let _ = self.0.set(admin);
    }

    fn admin(&self) -> Result<Arc<dyn DeviceAdmin>, RpcError> {
        self.0
            .get()
            .cloned()
            .ok_or_else(|| RpcError::internal("device management is not available on this host"))
    }
}

fn host_scope(ctx: &Ctx) -> Result<(), RpcError> {
    match ctx.scope() {
        ConnectionScope::Host => Ok(()),
        _ => Err(RpcError::new(
            code::PERMISSION_DENIED,
            "devices are managed from a host connection",
        )),
    }
}

fn io(e: std::io::Error) -> RpcError {
    match e.kind() {
        std::io::ErrorKind::InvalidInput => RpcError::invalid_params(e.to_string()),
        _ => RpcError::internal(e.to_string()),
    }
}

async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> std::io::Result<T> + Send + 'static,
) -> Result<T, RpcError> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| RpcError::internal(e.to_string()))?
        .map_err(io)
}

/// 핸들러를 붙인다.
pub fn register(router: &mut Router, devices: &Devices) {
    let d = devices.clone();
    router.unary(RelayDevices, move |ctx, ()| {
        let d = d.clone();
        async move {
            host_scope(&ctx)?;
            let admin = d.admin()?;
            let caller = ctx
                .caller()
                .window
                .as_deref()
                .and_then(crate::link_window)
                .map(|(device, _)| device.to_owned());
            let listen = admin.listen();
            let devices = blocking(move || admin.list(caller.as_deref())).await?;
            Ok(RelayDeviceList { devices, listen })
        }
    });
    let d = devices.clone();
    router.unary(RelayUnpair, move |ctx, p: RelayUnpairParams| {
        let d = d.clone();
        async move {
            host_scope(&ctx)?;
            let admin = d.admin()?;
            let removed = blocking(move || admin.unpair(&p.device)).await?;
            Ok(RelayUnpairResult { removed })
        }
    });
    let d = devices.clone();
    router.unary(RelayPairCodeIssue, move |ctx, p: RelayPairCodeParams| {
        let d = d.clone();
        async move {
            host_scope(&ctx)?;
            let admin = d.admin()?;
            blocking(move || admin.pair_code(p.role, p.name.as_deref())).await
        }
    });
}
