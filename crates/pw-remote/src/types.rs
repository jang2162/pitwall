//! `remote.*` 의 입출력 타입(TS 로 나간다).

use serde::{Deserialize, Serialize};

/// 호스트 항목이 어디서 왔나.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum RemoteHostSource {
    /// 전역 설정의 `[[remote_hosts]]`.
    Settings,
    /// `~/.ssh/config` 의 `Host` 줄(와일드카드 없음).
    SshConfig,
}

/// 붙는 단계(진단 칸의 차례 — 18-m7-plan M7-08 ②).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum RemoteStage {
    /// ssh 로 붙어 원격 OS · 아키텍처를 본다.
    Ssh,
    /// 같은 판의 Relay 를 올리고(이미 있으면 그대로) 이 기기를 페어링한다.
    Install,
    /// `pitwall-relay proxy` 가 데몬을 띄우고 잇는다.
    Daemon,
    /// 링크 핸드셰이크(판 · 기기 증명 · 머신 id).
    Handshake,
}

/// 실패의 종류. 화면이 사람의 말로 옮기고, 다시 붙기를 할지 가른다([`RemoteFailureKind::retryable`]).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum RemoteFailureKind {
    /// 호스트 키가 known_hosts 의 것과 다르다 — 덮는 길이 없다(사람이 known_hosts 를 고친다).
    HostKeyChanged,
    /// 호스트 키를 받지 않았다(취소 · 답 없음).
    HostKeyNotAccepted,
    /// ssh 가 붙지 못했다(연결 · 인증) 또는 ssh 를 띄우지 못했다.
    Ssh,
    /// 지원 밖 플랫폼 — 아무것도 올리지 않았다.
    Unsupported,
    /// 설치 · 페어링이 실패했다.
    Install,
    /// 판이 다르다.
    VersionMismatch,
    /// Relay 가 이 기기를 받지 않았다.
    Rejected,
    /// 링크가 서지 않았거나 끊겼다.
    Link,
    /// 이 머신이 이미 다른 호스트 이름으로 붙어 있다.
    AlreadyConnected,
    /// 원격 호스트를 쓸 수 없다(설정 · 부품 없음).
    Unavailable,
}

impl RemoteFailureKind {
    /// 끊긴 뒤 저절로 다시 붙어 볼 실패인가. 호스트 키 · 판 · 페어링 · 플랫폼처럼 사람이 손대야 하는 것은 아니다.
    pub fn retryable(self) -> bool {
        matches!(self, RemoteFailureKind::Ssh | RemoteFailureKind::Link)
    }
}

/// 실패 하나. `message` 는 영어 원문(진단), 화면은 `kind` 로 말을 고른다. 비밀(askpass 답)은 들지 않는다.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct RemoteFailure {
    pub stage: RemoteStage,
    pub kind: RemoteFailureKind,
    pub message: String,
}

/// 호스트 하나의 연결 상태(18-m7-plan M7-08 ② — 끊김 → 붙는 중 → 붙음 → 다시 붙는 중).
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(
    tag = "state",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum RemoteStatus {
    /// 링크가 없다. `byUser` 면 사람이 끊었다 — 다시 붙지 않는다. 마지막 실패가 있으면 `error`.
    Disconnected {
        by_user: bool,
        error: Option<RemoteFailure>,
    },
    /// 붙는 중(지금 단계).
    Connecting { stage: RemoteStage },
    /// 붙었다. `machine` 은 증명된 머신 id(Relay 기기 공개키 지문).
    Connected { machine: String },
    /// 끊긴 뒤 다시 붙기를 기다린다(지수 백오프). `attempt` 는 1부터.
    Reconnecting {
        attempt: u32,
        #[specta(type = pw_rpc::JsNumber)]
        retry_in_ms: u64,
        error: RemoteFailure,
    },
    /// 기억한 머신 id 와 다른 머신이 답했다(SSH 호스트 키 변경과 같은 취급 — 메인 판정 M7-P6). 사람이 「이 머신을 새로 믿기」
    /// (`remote.trustMachine`)를 고르기 전에는 붙지 않는다.
    MachineChanged { expected: String, found: String },
}

impl RemoteStatus {
    pub fn idle() -> Self {
        RemoteStatus::Disconnected {
            by_user: false,
            error: None,
        }
    }
}

/// 링크 없이 본 마지막 상태(메인 판정 M7-P6 — 마지막 링크 상태와 그 시각).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum RemoteLastLinkState {
    Connected,
    Disconnected,
    Failed,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct RemoteLastLink {
    pub state: RemoteLastLinkState,
    /// 유닉스 시각(ms).
    #[specta(type = pw_rpc::JsNumber)]
    pub at: i64,
}

/// 원격 호스트 하나(`remote.hosts.list`).
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct RemoteHostInfo {
    /// 항목 이름(별명 또는 `~/.ssh/config` 의 호스트 이름). `remote.*` 는 이것으로 고른다.
    pub name: String,
    /// ssh 목적지.
    pub host: String,
    pub user: Option<String>,
    pub port: Option<u16>,
    pub source: RemoteHostSource,
    /// 기억한 머신 id(처음 붙은 뒤).
    pub machine: Option<String>,
    pub status: RemoteStatus,
    pub last_link: Option<RemoteLastLink>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct RemoteHostList {
    pub hosts: Vec<RemoteHostInfo>,
    /// 읽지 못한 설정 항목 등(사람이 읽을 영어 문장).
    pub warnings: Vec<String>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct RemoteHostParams {
    /// 항목 이름.
    pub name: String,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct RemoteTrustMachineParams {
    pub name: String,
    /// 사람이 본 새 머신 id(`machineChanged` 의 `found`). 그사이 다른 id 가 왔으면 거절한다.
    pub machine: String,
}

/// 가벼운 상태 확인(`ssh -O check`).
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct RemoteHostCheck {
    /// 사람의 ControlMaster 가 떠 있는가. 마스터 설정이 없으면(확인할 수 없음) `null` — 그때는 `lastLink` 를 본다.
    pub master: Option<bool>,
    pub last_link: Option<RemoteLastLink>,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum RemoteLogLevel {
    Info,
    Error,
}

/// 연결 로그 한 줄(18-m7-plan M7-08 ④). 비밀(askpass 답)은 남지 않는다.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct RemoteLogEntry {
    /// 유닉스 시각(ms).
    #[specta(type = pw_rpc::JsNumber)]
    pub at: i64,
    pub stage: Option<RemoteStage>,
    pub level: RemoteLogLevel,
    /// 영어 원문(ssh stderr 의 끝부분 포함 — 제어 · 양방향 글자 제거).
    pub text: String,
}

/// `remote.status` 이벤트.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct RemoteStatusChanged {
    pub name: String,
    pub status: RemoteStatus,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct RemotePromptAnswerParams {
    #[specta(type = pw_rpc::JsNumber)]
    pub prompt_id: u64,
    pub answer: pw_git::write::GitPromptAnswer,
}
