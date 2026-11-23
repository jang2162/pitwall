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
    /// ssh 가 붙지 못했다(연결 · 시간 초과) 또는 ssh 를 띄우지 못했다.
    Ssh,
    /// ssh 가 로그인하지 못했다(암호 질문을 취소했거나 틀린 답 · 받는 키가 없음). 저절로 다시 하지 않는다 — 다시 하면 같은 질문을
    /// 되풀이한다.
    Auth,
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

/// 떠 있는 옛 판 Relay 데몬 하나(18-m7-plan M7-07b ② — 사람이 본 그대로). 이 값 그대로 `remote.stopOld` 에 실어 보낸다 — 그사이
/// pid · 실행 · 터미널 · 붙은 링크 수가 바뀌었으면 끝내지 않는다.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct RemoteOldRelay {
    /// 원격 바이너리 이름(`pitwall-relay-{채널}-{판}`).
    pub file: String,
    /// 이름에서 읽은 판(읽지 못하면 `null`).
    pub version: Option<String>,
    pub channel: Option<String>,
    pub pid: u32,
    pub runs: u32,
    pub terminals: u32,
    /// 그 데몬에 붙은 링크 수(아직 옛 판인 다른 Desktop). 0 보다 크면 끝낼 때 그 연결도 끊긴다(M7-07 리뷰 R2).
    pub links: u32,
}

/// 답하지 않는 옛 판 Relay(데몬이 있는지 모른다 — `status` 가 3 이 아닌 오류). 끝내기 단추가 없다.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct RemoteSilentRelay {
    pub file: String,
    pub version: Option<String>,
}

/// 원격의 `~/.local/bin/pitwall`(메인 판정 M7-P12 — 사람이 고를 때만 둔다).
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Default,
    Serialize,
    Deserialize,
    specta::Type,
    schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum RemoteCommandLink {
    /// 없다.
    #[default]
    Absent,
    /// Relay 가 둔 링크(이 판으로 — 판이 바뀌면 붙을 때 옮긴다).
    Linked,
    /// 사람의 다른 파일 · 링크 — 건드리지 않는다.
    Other,
}

/// 붙은 뒤 본 원격 Relay 들(`remote.relays` — 붙을 때마다 조사 → 링크 옮기기 → 정리가 한 번 돈다).
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct RemoteRelays {
    /// 조사한 링크의 머신 id(증명된 것). `remote.stopOld` · `remote.linkCommand` 는 이 머신에만 간다.
    pub machine: String,
    pub old: Vec<RemoteOldRelay>,
    pub silent: Vec<RemoteSilentRelay>,
    pub command: RemoteCommandLink,
    /// 이번 정리에서 지운 옛 판 바이너리 이름.
    pub removed: Vec<String>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct RemoteStopOldParams {
    pub name: String,
    /// 사람이 본 머신(`RemoteRelays::machine`). 그 호스트가 지금 다른 머신에 붙어 있으면 거절한다.
    pub machine: String,
    /// 사람이 본 그 데몬(`RemoteRelays::old` 의 하나 그대로).
    pub seen: RemoteOldRelay,
}

/// [옛 판 끝내기] 의 결과.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(
    tag = "outcome",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum RemoteStopOutcome {
    /// 끝났다.
    Stopped,
    /// SIGTERM 은 보냈고 20초 안에 끝나지 않았다(실행 트리를 끝내는 중일 수 있다).
    Stopping,
    /// 사람이 본 뒤 바뀌었다 — 보내지 않았다. `now` 를 다시 보이고 다시 묻는다.
    Changed { now: RemoteOldRelay },
    /// 이미 없다.
    Gone,
    /// 답하지 않아 있는지 모른다 — 보내지 않았다.
    NoAnswer,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct RemoteLinkCommandParams {
    pub name: String,
    /// 사람이 본 머신(`RemoteRelays::machine`).
    pub machine: String,
    /// 켜기(이 판 바이너리로 링크) · 끄기(Relay 의 링크를 걷음).
    pub on: bool,
}

/// `remote.relays` 이벤트(그 호스트의 `remote.relays` 가 바뀌었다 — 다시 읽는다).
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct RemoteRelaysChanged {
    pub name: String,
}
