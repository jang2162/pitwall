//! 첫 실행 확인(17-m6-plan M6-07 ⑥ — M6-13 메인 판정 2026-11-18, 코디네이터 판정 2026-11-19).
//!
//! 에이전트는 `run_config_upsert` 로 구성을 묻지 않고 쓸 수 있다(`auto_approve` 의 `write` · 구성 하나에 닿는 세션 허용). 그래서
//! 창의 길([`super::Runs::start`] · 다시 실행)은 **띄울 내용이 「사람이 본 구성」(M6-11 — `Registry::human_saw`)이 아니면 띄우지
//! 않고** [`code::CONFIRM_REQUIRED`] 로 답한다. 커밋돼 온 구성 · 이 기기에서 처음 돌리는 구성도 같다(D68 — 내용이 바뀌지 않는 한
//! 한 번). 창은 [`RunConfirmRequired::shown`] 을 보이고 확인을 받은 뒤 그 `hash` 를 `confirmed` 로 실어 다시 부른다.
//!
//! - **해시는 띄우는 데 쓰는 바로 그 풀린 값에서 한 번만 낸다**(argv · 작업 폴더 · M6-03 의 env · 정의 본문 — `pw_run::seen`).
//!   확인과 띄우기 사이에 파일이 바뀌면 다시 부른 호출이 푼 값의 해시가 `confirmed` 와 달라 다시 묻는다 — 틈이 없다.
//! - 해시를 낼 수 없는 구성(정의 본문을 읽지 못한 `mise-task` 등 — `seen::content` 의 오류)은 기록할 수 없어 매번 묻는다. 그때의
//!   확인 열쇠는 [`once_token`] 이다(띄울 값의 sha256 — 기록하지 않는다, 같은 틈 막기만 한다).
//! - **보이는 내용은 실행될 글을 가리지 않는다**(M6-13 리뷰 B1 판정): argv · 정의 본문은 그대로, env 의 값만
//!   `config::shown::env_value` 로 가린다. 실행될 글에 비밀처럼 보이는 것이 있으면 [`RunShown::secret_looking`] 으로 경고만 단다.
//! - 확인을 지나 띄우면 그 해시가 「사람이 본 구성」 으로 기록된다(`SeenPlace::Run` — [`super::Runs::start`] 의 기록 그대로).
//!   이 검사는 창의 증거(`SeenBy`)가 있는 길에만 있다 — 에이전트의 길(`start_agent` · 툴의 다시 실행)은 승인 계획이 묻는다.

use std::path::Path;

use pw_rpc::RpcError;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::config::shown;

/// 실행 RPC 의 애플리케이션 오류 코드(`data` 는 객체). 다른 크레이트의 범위(DB 3xxx · 도구 4xxx · LSP 5xxx · 승인 6001)와 겹치지
/// 않는다.
pub mod code {
    /// 띄울 내용이 「사람이 본 구성」 이 아니다(`data` 는 [`super::RunConfirmRequired`]). 창은 내용을 보이고 확인을 받은 뒤 `hash` 를
    /// `confirmed` 로 실어 다시 부른다.
    pub const CONFIRM_REQUIRED: i32 = 7001;
}

/// [`code::CONFIRM_REQUIRED`] 의 `data`.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct RunConfirmRequired {
    /// 확인 열쇠 — 다시 부를 때 `confirmed` 로 그대로 돌려준다.
    pub hash: String,
    /// 띄울 구성(사슬이면 그 마지막 프로세스 단계 — 모든 단계는 `steps`).
    pub shown: RunShown,
    /// 사슬(`before` · 복합 구성 — 17-m6-plan M6-21)이면 띄울 단계 모두, 띄우는 차례로. `sql` 단계는 연결과 파일만 보인다 — 그
    /// 문장은 이 확인 뒤의 위험 확인(`3001`)이 보인다. 사슬이 아니면 비었다.
    #[serde(default)]
    pub steps: Vec<RunShown>,
    /// 사슬의 차례 한 줄(예: `seed, then (api and web together)`).
    #[serde(default)]
    pub order: Option<String>,
    /// 확인 창의 제목에 쓸 이름 — 사람이 고른 구성(사슬이면 그 구성: 복합 구성이면 복합 구성의 이름, 디버그의 `before` 면 디버그할
    /// 구성). 사슬이 아니면 없다(`shown.name` 이 그것이다).
    #[serde(default)]
    pub title: Option<String>,
}

/// 첫 실행 확인에 보이는 것 — 띄울 그 값.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct RunShown {
    /// 구성 열쇠(구성 id).
    pub key: String,
    /// 구성의 `name`.
    pub name: String,
    /// 종류(`type` — `shell` · `npm` …).
    pub kind: String,
    /// 띄울 argv 그대로(가리지 않는다).
    pub argv: Vec<String>,
    /// 작업 폴더(프로젝트 루트 기준, 루트면 `.`).
    pub cwd: String,
    pub profile: Option<String>,
    /// 얹을 env(이름순). 값은 가린 꼴이다.
    pub env: Vec<RunShownVar>,
    /// 읽은 `.env` 파일(구성의 차례).
    pub env_files: Vec<RunShownFile>,
    /// 명령 정의 본문 — 실행될 글이라 그대로. `npm` 은 그 script, `mise-task` 는 그 태스크를 정의한 파일의 글.
    pub definition: Option<String>,
    /// 정의 본문을 읽은 파일(`mise-task` — 절대 경로). `npm` 이면 `null`.
    pub definition_source: Option<String>,
    /// argv · 정의 본문에 비밀처럼 보이는 것(비밀번호가 든 URL · 비밀 이름의 `NAME=값`)이 있다 — 가리지 않고 경고만.
    pub secret_looking: bool,
    /// 확인하면 「사람이 본 구성」 으로 기록되는가. 거짓이면(해시를 낼 수 없는 구성) 다음에도 묻는다.
    pub recordable: bool,
}

/// env 하나.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct RunShownVar {
    pub name: String,
    /// 보이는 값(`config::shown::env_value` — 비밀 이름이면 `••••` 와 해시 앞 8자, URL 의 비밀번호 자리도 가린다).
    pub value: String,
    /// 값을 (일부라도) 가렸는가.
    pub masked: bool,
}

/// `.env` 파일 하나.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct RunShownFile {
    /// 구성에 적힌 경로(프로젝트 루트 기준).
    pub path: String,
    /// 내용 sha256 앞 12자.
    pub sha256: String,
}

pw_rpc::rpc_type!(RunConfirmRequired);

/// 명령 정의 본문: 해시에 드는 것과 사람에게 보이는 것.
pub(super) struct Defined {
    /// 「사람이 본 구성」 해시의 정의 본문(`seen::SeenParts::definition`).
    pub hashed: String,
    /// 확인 화면의 글.
    pub shown: String,
    /// 읽은 파일(`mise-task`).
    pub source: Option<String>,
}

/// 확인 화면에 보일 정의 파일 글의 상한.
const SHOWN_SOURCE: usize = 64 * 1024;

/// 정의 파일의 글(확인 화면) — [`SHOWN_SOURCE`] 까지, UTF-8 이 아니면 바꿔 읽는다. 읽지 못하면 그 까닭 한 줄.
pub(super) fn source_text(path: &Path) -> String {
    match std::fs::read(path) {
        Ok(bytes) => {
            let cut = bytes.len() > SHOWN_SOURCE;
            let mut text =
                String::from_utf8_lossy(&bytes[..bytes.len().min(SHOWN_SOURCE)]).into_owned();
            if cut {
                text.push_str("\n…");
            }
            text
        }
        Err(e) => format!("({}: {e})", path.display()),
    }
}

/// 띄울 값.
pub(super) struct Launching<'a> {
    /// 창의 길과 에이전트의 계획이 함께 쓰는 풀기(`Runs::resolve_process`).
    pub resolved: &'a super::Resolved,
}

impl Launching<'_> {
    /// 보일 내용.
    pub fn shown(&self, recordable: bool) -> RunShown {
        let r = self.resolved;
        let cwd = match r.cwd.strip_prefix(&r.root) {
            Ok(rel) if rel.as_os_str().is_empty() => ".".to_owned(),
            Ok(rel) => rel.display().to_string(),
            Err(_) => r.cwd.display().to_string(),
        };
        let env = r
            .run_env
            .vars()
            .iter()
            .map(|(name, value)| {
                let shown = shown::env_value(name, value);
                RunShownVar {
                    masked: shown != *value,
                    name: name.clone(),
                    value: shown,
                }
            })
            .collect();
        let env_files = r
            .run_env
            .files()
            .iter()
            .map(|f| RunShownFile {
                path: f.path.clone(),
                sha256: f.sha256.chars().take(12).collect(),
            })
            .collect();
        let secret_looking = r
            .argv
            .iter()
            .map(String::as_str)
            .chain(r.definition_shown.as_deref())
            .any(shown::secret_looking);
        RunShown {
            key: r.key.clone(),
            name: r.config.name.clone(),
            kind: r.config.kind.type_name().to_owned(),
            argv: r.argv.clone(),
            cwd,
            profile: r.run_env.profile().map(str::to_owned),
            env,
            env_files,
            definition: r.definition_shown.clone(),
            definition_source: r.definition_source.clone(),
            secret_looking,
            recordable,
        }
    }

    /// 해시를 낼 수 없는 구성의 확인 열쇠: 띄울 값(argv · 작업 폴더 · env 의 이름과 값 · `.env` 다이제스트 · 정의 본문 · 런타임 원천
    /// 파일(`mise.toml` 등 — 리뷰 F1)의 경로와 내용 sha256)의 sha256. 기록하지 않는다 — 확인과 띄우기 사이의 바뀜만 가른다.
    pub fn once_token(&self) -> String {
        let r = self.resolved;
        #[derive(Serialize)]
        struct Once<'a> {
            key: &'a str,
            config: String,
            argv: &'a [String],
            cwd: &'a Path,
            env: &'a std::collections::BTreeMap<String, String>,
            files: Vec<(&'a str, &'a str)>,
            profile: Option<&'a str>,
            definition: Option<&'a str>,
            shown_definition: Option<&'a str>,
            runtime: Vec<(String, String)>,
        }
        let once = Once {
            key: &r.key,
            config: r.config.canonical(),
            argv: &r.argv,
            cwd: &r.cwd,
            env: r.run_env.vars(),
            files: r
                .run_env
                .files()
                .iter()
                .map(|f| (f.path.as_str(), f.sha256.as_str()))
                .collect(),
            profile: r.run_env.profile(),
            definition: r.definition.as_deref(),
            shown_definition: r.definition_shown.as_deref(),
            runtime: crate::seen::runtime_files(&r.cwd, r.home.as_deref())
                .into_iter()
                .map(|path| {
                    let digest = std::fs::read(&path)
                        .map(|b| crate::env::hex(&Sha256::digest(&b)))
                        .unwrap_or_else(|e| format!("unreadable: {e}"));
                    (path.display().to_string(), digest)
                })
                .collect(),
        };
        let bytes = serde_json::to_vec(&once).unwrap_or_default();
        format!("once:{}", crate::env::hex(&Sha256::digest(&bytes)))
    }
}

/// 확인이 필요하다는 오류.
pub(super) fn confirm_required(hash: String, shown: RunShown) -> RpcError {
    confirm_required_with(RunConfirmRequired {
        hash,
        shown,
        steps: Vec::new(),
        order: None,
        title: None,
    })
}

/// [`confirm_required`] 의 꼴 그대로(사슬은 `steps` · `order` 를 채운다).
pub(super) fn confirm_required_with(data: RunConfirmRequired) -> RpcError {
    RpcError::new(
        code::CONFIRM_REQUIRED,
        format!(
            "run config `{}` has content the person has not seen; show it and confirm",
            data.shown.key
        ),
    )
    .with_data(serde_json::to_value(&data).unwrap_or_default())
}
