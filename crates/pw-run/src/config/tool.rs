//! 실행 구성 쓰기 툴 메서드(17-m6-plan M6-13, `docs/design/05-mcp.md` 「실행 구성」 표). 그룹 `run`, 둘 다 `write`, 앱 Host 만
//! (헤드리스 Host 는 실행 구성 메서드를 붙이지 않는다).
//!
//! - `run_config_upsert` ← `run.tool.configUpsert`: `.pitwall/run/<id>.toml`(공유, 기본) 또는 `<id>.local.toml`(개인)을 만들거나
//!   고친다. `run_config_delete` ← `run.tool.configDelete`: 그 파일(들)을 지운다. 쓰기는 화면(`run.config.save` · `delete`)과 같은
//!   [`super::plan`] → [`super::apply`] 하나다(공유 부품 표 — M6-01).
//! - **두 단계**(`pw_core::registry::plan`): 계획은 그 파일의 앞뒤 내용(`PlanChange::File`)이고 계획 단계는 아무것도 쓰지 않는다.
//!   승인 뒤에는 계획 때 만든 [`super::RunConfigEdit`] 를 그대로 [`super::apply`] 한다 — 다시 만들지 않고, `apply` 가 지금 파일이
//!   계획 때 읽은 내용인지 견준다(다르면 conflict, 쓰지 않는다).
//! - **입력 검증은 구성 스키마 그대로**: 입력의 `config` 는 [`RunConfig`](JSON Schema 를 내는 바로 그 타입 — `config::json_schema`)로
//!   읽고, 그 타입이 모르는 키(`env_file` 처럼 파일 키로 적은 것, 오타)는 거절한다(serde 는 모르는 키를 말없이 버린다 — 고치려던
//!   필드 대신 빈 값이 저장되면 안 된다). 그 뒤 [`super::validate`](`cwd` · `envFile` 이 루트 안 등)를 지난다. 모두 승인을 묻기
//!   전에 거절한다.
//! - **비밀은 받지 않는다**(메인 판정 M6-P18): `env` · 프로필 `env` 의 키가 비밀 단어면([`shown::is_secret_name`]) 거절하고
//!   `envFile` 로 가리키게 한다 — 공유 구성은 커밋된다. 값 안의 비밀(이름이 평범한 키의 토큰)은 가르지 않는다(`pw_core::secret`
//!   머리 주석 — 첫 방어선은 설계 규칙).
//! - **보이는 계획은 가린다**([`shown`] — M6-14 ② 와 같은 규칙): 파일 diff 의 앞뒤 글에서 비밀 단어 키의 값 · URL 비밀번호를
//!   가리고(사람이 개인 파일에 적어 둔 값이 앞 글에 있을 수 있다), `envFile` 마다 그 파일이 낼 변수를 이름과 값(같은 가림)으로
//!   보인다. 해시는 가리기 전의 앞뒤 내용을 대상 값으로 덮는다.
//! - **git 이 추적하는 `.local.toml` 은 쓰지 않는다**(m6-localpolicy — 커밋된 파일은 사람의 개인 설정이 아니다): `location: local`
//!   의 그 파일이 추적(또는 받아 온 내용 · 판정 못 함)이면 묻기 전에 거절한다. `apply` 도 거절하지만(`ConfigSource::
//!   prepare_local_write`) 그것은 승인 뒤다 — 사람에게 쓸 수 없는 쓰기를 묻지 않는다.
//! - **승인은 「사람이 본 구성」 이 아니다**(결정 D68 · M6-D6 ① — 「`run_config_upsert` 승인은 넣지 않는다」): 이 툴은
//!   `run_seen` 을 기록하지 않는다. 사람이 본 것은 파일 diff 이지 띄울 명령의 풀린 내용(argv · `.env` 값 · 런타임 원천 파일)이
//!   아니고, 에이전트가 만든 구성은 그 첫 `run_start` 에서 다시 묻는다. 에이전트 범위라 `SeenBy` 를 만들 수도 없다.
//! - **계획 규칙**([`rules`] — 메인 판정 2026-11-18): `auto_approve` 의 `write` 가 닿고, 「이 세션 동안 허용」 은 그 구성 하나
//!   (`Remember::Key` — 구성 id)에 닿는다. `open_with_upsert`(결정 M5-D31 — 늘 묻는다)와 다른 까닭: 에이전트는 파일 도구로 같은
//!   파일을 고칠 수 있어 이 쓰기는 파일 쓰기와 같은 무게이고, 실행은 따로 묻는다(`run_start` 는 사람이 본 적 없는 구성을 묻고,
//!   사람이 화면에서 처음 띄울 때도 내용을 먼저 보인다 — M6-07).
//!
//! 둘 다 MCP 세션의 연결 범위만 받는다(`agent_grant` — 없는 프로젝트는 그 오류 그대로).

use std::path::Path;
use std::sync::Arc;

use pw_core::config::tracked::{Tracking, tracking};
use pw_core::registry::agent::agent_grant;
use pw_core::registry::plan::{PlanChange, PlanRules, Planned, Remember, ToolPlan};
use pw_rpc::{ConnectionScope, Router, RpcError, Tier, code, rpc_method};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Value, json};

use super::RunConfig;
use super::methods::RunConfigs;
use super::read::{RunConfigPlace, read};
use super::shown;
use super::write::{RunConfigEdit, RunConfigLocation, apply, check_id, file_of, plan};

/// `run.tool.configUpsert` 의 RPC 이름.
pub const UPSERT_METHOD: &str = "run.tool.configUpsert";
/// `run.tool.configDelete` 의 RPC 이름.
pub const DELETE_METHOD: &str = "run.tool.configDelete";

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunToolConfigUpsertParams {
    pub project_id: String,
    /// Top of the worktree (absolute path). The shared file is written in that worktree and `cwd` · `envFile` are relative to
    /// the project root inside it. Empty means the session's worktree, or else the main checkout.
    #[serde(default)]
    pub worktree: Option<String>,
    /// Run configuration id (= file name): letters, digits, `-`, `_`, 1-64 characters.
    pub id: String,
    /// The whole configuration of that file: keys left out are removed from it (comments and keys Pitwall does not know are
    /// kept). Keys are camelCase (`envFile`); unknown keys are refused. `env` (and a profile's `env`) must not hold secrets: a
    /// variable whose name has a secret word (`API_TOKEN`, `DB_PASSWORD`) is refused — put it in a `.env` file that is not
    /// committed and list that file in `envFile`.
    #[serde(deserialize_with = "strict_config")]
    #[schemars(with = "RunConfig")]
    #[specta(type = RunConfig)]
    pub config: RunConfig,
    /// `shared` (the default): `.pitwall/run/<id>.toml`, committed with the project. `local`: `.pitwall/run/<id>.local.toml`,
    /// personal and not committed (the main checkout's for a linked worktree).
    #[serde(default)]
    pub location: RunConfigLocation,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct RunToolConfigUpsert {
    pub id: String,
    /// The file written (absolute path).
    pub file: String,
    /// False when the file already had this content (nothing was written).
    pub changed: bool,
    /// Why the configuration cannot be used after this write (the shared and personal files together), if it cannot.
    pub unusable: Option<String>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunToolConfigDeleteParams {
    pub project_id: String,
    /// Top of the worktree (absolute path). Empty means the session's worktree, or else the main checkout.
    #[serde(default)]
    pub worktree: Option<String>,
    /// Run configuration id (see `run_config_list`).
    pub id: String,
    /// The file to delete: `shared` or `local`. Without it both files are deleted and the configuration is gone; deleting only
    /// the personal file brings back the shared file's configuration.
    #[serde(default)]
    pub location: Option<RunConfigLocation>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct RunToolConfigDelete {
    pub id: String,
    /// The files deleted (absolute paths).
    pub files: Vec<String>,
}

rpc_method! {
    /// Create or change a run configuration — `.pitwall/run/<id>.toml` (shared, committed; the default) or `<id>.local.toml`
    /// (personal, `location: "local"`). `config` is the whole configuration of that file, checked against the run
    /// configuration schema (`type`: shell, npm, gradle, maven, cargo, go, python, docker-compose, mise-task) before anything is
    /// asked: `cwd` and `envFile` must stay inside the project root, and `env` takes no secrets (names with a secret word are
    /// refused — use `envFile`). The person sees the file's diff and approves it in Pitwall first. Saving does not approve
    /// running: `run_start` still asks the person for a configuration an agent created or changed.
    pub RunToolConfigUpsertMethod: "run.tool.configUpsert" {
        params: Planned<RunToolConfigUpsertParams>,
        result: RunToolConfigUpsert,
        tier: Write,
        roles: [Owner],
        mcp: { group: "run", name: "run_config_upsert" },
    }

    /// Delete a run configuration's file — the shared one, the personal one (`location`), or both (no `location`). The person
    /// sees what is deleted and approves it in Pitwall first. Detected tasks (`detected:…`) have no file and cannot be deleted.
    pub RunToolConfigDeleteMethod: "run.tool.configDelete" {
        params: Planned<RunToolConfigDeleteParams>,
        result: RunToolConfigDelete,
        tier: Write,
        roles: [Owner],
        mcp: { group: "run", name: "run_config_delete" },
    }
}

/// 입력의 `config`: [`RunConfig`] 로 읽고, 그 타입이 모르는 키가 있으면 거절한다(모듈 문서). 판정은 읽은 값을 다시 JSON 으로 쓴
/// 것에 입력의 키가 모두 있는가다 — `[debug]` 의 키 · `env` 의 이름처럼 값으로 들고 다니는 키는 다시 쓴 것에도 있다.
fn strict_config<'de, D: Deserializer<'de>>(deserializer: D) -> Result<RunConfig, D::Error> {
    use serde::de::Error;
    let input = Value::deserialize(deserializer)?;
    let config: RunConfig = serde_json::from_value(input.clone()).map_err(D::Error::custom)?;
    let known = serde_json::to_value(&config).map_err(D::Error::custom)?;
    match unknown_key(&input, &known, "config") {
        Some(at) => Err(D::Error::custom(format!(
            "unknown field `{at}` (run configuration keys are camelCase, e.g. `envFile`)"
        ))),
        None => Ok(config),
    }
}

/// `input` 에 있고 `known` 에 없는 키의 자리(첫 하나).
fn unknown_key(input: &Value, known: &Value, at: &str) -> Option<String> {
    let (Value::Object(input), Value::Object(known)) = (input, known) else {
        return None;
    };
    input.iter().find_map(|(key, value)| {
        let here = format!("{at}.{key}");
        match known.get(key) {
            None => Some(here),
            Some(known) => unknown_key(value, known, &here),
        }
    })
}

/// `env` · 프로필 `env` 의 비밀 이름을 거절한다(메인 판정 M6-P18).
fn refuse_secret_env(config: &RunConfig) -> Result<(), RpcError> {
    let names = config.env.keys().map(|n| (n, None)).chain(
        config
            .profiles
            .iter()
            .flat_map(|(p, profile)| profile.env.keys().map(move |n| (n, Some(p.as_str())))),
    );
    for (name, profile) in names {
        if shown::is_secret_name(name) {
            let at = match profile {
                None => "env".to_owned(),
                Some(p) => format!("profiles.{p}.env"),
            };
            return Err(RpcError::new(
                code::INVALID_PARAMS,
                format!(
                    "`{at}.{name}` looks like a secret (its name has a secret word); Pitwall does not write secrets into \
                     run configuration files — put it in a `.env` file that is not committed and list that file in `envFile`"
                ),
            )
            .with_data(json!({ "reason": "secretEnv", "name": name })));
        }
    }
    Ok(())
}

/// 개인 파일(`location: local`) 이 git 이 추적하는 파일이면(또는 받아 온 내용 · 판정 못 함) 묻기 전에 거절한다(모듈 문서).
fn refuse_tracked_local(
    place: &RunConfigPlace,
    location: RunConfigLocation,
    edit: &RunConfigEdit,
) -> Result<(), RpcError> {
    if !place.repository || location != RunConfigLocation::Local {
        return Ok(());
    }
    let checkout = [Some(place.source.dir()), place.source.local_fallback_dir()]
        .into_iter()
        .flatten()
        .find(|dir| edit.path.starts_with(dir))
        .and_then(Path::parent)
        .ok_or_else(|| {
            RpcError::internal(format!(
                "{} is not inside the project's config folder",
                edit.path.display()
            ))
        })?;
    let why = match tracking(checkout, &edit.path) {
        Tracking::Untracked => return Ok(()),
        Tracking::Tracked => {
            "is tracked by git, so it is a committed file and not a personal setting".to_owned()
        }
        Tracking::Fetched => {
            "holds content git fetched, so it is not a personal setting".to_owned()
        }
        Tracking::Unknown(reason) => format!("cannot tell whether git tracks it ({reason})"),
    };
    Err(RpcError::new(
        code::CONFLICT,
        format!(
            "{} {why}; Pitwall does not write it — the person has to fix it by hand",
            edit.path.display()
        ),
    )
    .with_data(json!({ "reason": "trackedLocalFile" })))
}

/// 계획에 보이는 파일 diff(가린 글 — [`shown::config_text`]).
fn file_change(edit: &RunConfigEdit) -> PlanChange {
    PlanChange::File {
        path: edit.path.display().to_string(),
        before: edit.before.as_deref().map(shown::config_text),
        after: edit.after.as_deref().map(shown::config_text),
    }
}

/// 계획 규칙: 기본값(`auto_approve` 가 닿는다)에 「이 세션 동안 허용」 을 그 구성 id 하나로(모듈 문서).
fn rules(id: &str) -> PlanRules {
    PlanRules {
        remember: Remember::Key { key: id.to_owned() },
        ..PlanRules::default()
    }
}

fn not_found(id: &str) -> RpcError {
    RpcError::new(code::NOT_FOUND, format!("run config `{id}` does not exist"))
}

impl RunConfigs {
    /// `run_config_upsert`.
    pub fn tool_upsert(
        &self,
        scope: &ConnectionScope,
        planned: &Planned<RunToolConfigUpsertParams>,
    ) -> Result<RunToolConfigUpsert, RpcError> {
        self.upsert_then(scope, planned, || {})
    }

    /// [`Self::tool_upsert`]. `approved` 는 승인을 지난 뒤 · 쓰기 전에 돈다(시험이 그 틈에 파일을 바꾼다).
    pub(crate) fn upsert_then(
        &self,
        scope: &ConnectionScope,
        planned: &Planned<RunToolConfigUpsertParams>,
        approved: impl FnOnce(),
    ) -> Result<RunToolConfigUpsert, RpcError> {
        let p = &planned.params;
        let agent = agent_grant(scope, &self.host()?.projects, &p.project_id)?;
        check_id(&p.id)?;
        refuse_secret_env(&p.config)?;
        let place = self.place(agent.project_id(), p.worktree.as_deref())?;
        if let Some(twin) = read(&place).case_twin(&p.id) {
            return Err(RpcError::new(
                code::CONFLICT,
                format!(
                    "run config `{twin}` differs from `{}` only in case; change `{twin}` instead",
                    p.id
                ),
            ));
        }
        let edit = plan(&place, &p.id, p.location, Some(&p.config))?;
        refuse_tracked_local(&place, p.location, &edit)?;
        let mut builder = ToolPlan::builder(UPSERT_METHOD, Tier::Write, p)
            .summary(format!(
                "{} run configuration `{}` ({}) — running it will still ask",
                if edit.before.is_some() {
                    "Change"
                } else {
                    "Create"
                },
                p.id,
                p.config.name
            ))
            // 해시는 가리기 전의 내용을 덮는다 — 보이는 diff 는 가린 글이다.
            .target("before", &edit.before)
            .target("after", &edit.after)
            .rules(rules(&p.id))
            .change(file_change(&edit));
        if let Some(text) = shown::env_files_text(&p.config, &place.root) {
            builder = builder.change(PlanChange::Text {
                label: "Environment files".to_owned(),
                text,
            });
        }
        planned.approve(builder.build())?;
        approved();
        // 승인 뒤에는 계획 때 만든 그 내용만 쓴다(`apply` 가 계획 때 읽은 내용과 견준다).
        apply(&place, &edit)?;
        let unusable = read(&place)
            .unusable
            .into_iter()
            .find(|u| u.id == p.id)
            .map(|u| u.reason);
        Ok(RunToolConfigUpsert {
            id: p.id.clone(),
            file: edit.path.display().to_string(),
            changed: edit.changes(),
            unusable,
        })
    }

    /// `run_config_delete`.
    pub fn tool_delete(
        &self,
        scope: &ConnectionScope,
        planned: &Planned<RunToolConfigDeleteParams>,
    ) -> Result<RunToolConfigDelete, RpcError> {
        self.delete_then(scope, planned, || {})
    }

    /// [`Self::tool_delete`]. `approved` 는 [`Self::upsert_then`] 과 같다.
    pub(crate) fn delete_then(
        &self,
        scope: &ConnectionScope,
        planned: &Planned<RunToolConfigDeleteParams>,
        approved: impl FnOnce(),
    ) -> Result<RunToolConfigDelete, RpcError> {
        let p = &planned.params;
        let agent = agent_grant(scope, &self.host()?.projects, &p.project_id)?;
        check_id(&p.id)?;
        let place = self.place(agent.project_id(), p.worktree.as_deref())?;
        // 정확히 그 id 의 구성만(대소문자를 가리지 않는 파일 시스템에서 `dev` 로 `Dev.toml` 을 지우지 않는다 — `run.config.delete`).
        if !read(&place).contains(&p.id) {
            return Err(not_found(&p.id));
        }
        let locations = match p.location {
            Some(location) => vec![location],
            None => vec![RunConfigLocation::Shared, RunConfigLocation::Local],
        };
        let mut edits = Vec::new();
        for location in locations {
            if std::fs::symlink_metadata(file_of(&place, &p.id, location)).is_err() {
                continue;
            }
            let edit = plan(&place, &p.id, location, None)?;
            refuse_tracked_local(&place, location, &edit)?;
            edits.push(edit);
        }
        if edits.is_empty() {
            return Err(not_found(&p.id));
        }
        let befores: Vec<&Option<String>> = edits.iter().map(|e| &e.before).collect();
        let mut builder = ToolPlan::builder(DELETE_METHOD, Tier::Write, p)
            .summary(format!("Delete run configuration `{}`", p.id))
            .target("before", &befores)
            .rules(rules(&p.id));
        for edit in &edits {
            builder = builder.change(file_change(edit));
        }
        planned.approve(builder.build())?;
        approved();
        let mut files = Vec::new();
        for edit in &edits {
            apply(&place, edit)?;
            files.push(edit.path.display().to_string());
        }
        Ok(RunToolConfigDelete {
            id: p.id.clone(),
            files,
        })
    }
}

/// 블로킹 스레드에서 돌린다(설정 파일 · 레지스트리 · git 인덱스 읽기).
async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, RpcError> + Send + 'static,
) -> Result<T, RpcError> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| RpcError::internal(e.to_string()))?
}

/// 핸들러를 붙인다([`super::methods::register`] 가 부른다).
pub(crate) fn register(router: &mut Router, configs: Arc<RunConfigs>) {
    let upsert = Arc::clone(&configs);
    router
        .unary(
            RunToolConfigUpsertMethod,
            move |ctx, planned: Planned<RunToolConfigUpsertParams>| {
                let (c, scope) = (Arc::clone(&upsert), ctx.scope().clone());
                blocking(move || c.tool_upsert(&scope, &planned))
            },
        )
        .unary(
            RunToolConfigDeleteMethod,
            move |ctx, planned: Planned<RunToolConfigDeleteParams>| {
                let (c, scope) = (Arc::clone(&configs), ctx.scope().clone());
                blocking(move || c.tool_delete(&scope, &planned))
            },
        );
}

#[cfg(test)]
mod tests;
