//! 설정 창 「Git」(10-ui.md S7, 13-m2-plan.md M2-20): git 실행 파일의 경로 · 찾은 버전 · 쓰기를 끈 이유 — `git.executable`.
//!
//! 설정 창은 Host 연결이고 읽기만 한다. 띄우는 것은 고정 argv `git --version` 하나뿐이고, 요청에는 인자 · 경로를 넣을 자리가 없다
//! (params `()`). 작업 디렉터리는 어느 프로젝트 안도 아닌 곳([`neutral_dir`])이다 — 03-architecture.md 「실행의 연결 범위」 의
//! 이월 범주(`cli.status` 와 같은 꼴: 고정 명령 · 고정 로그인 셸).
//!
//! 찾는 길은 쓰기 실행기([`Git::command`])와 같다: 전역 설정 `[git] path`(없으면 `git`)를 **로그인 셸**의 PATH 에서 찾고, 버전은
//! 쓰기가 쓰는 것과 같은 캐시(`Git::version`)로 읽는다 — 이 화면이 「2.39 라 꺼짐」 이라고 하면 쓰기도 그 판정으로 꺼진다.
//! 다른 것은 PATH 하나다: 신뢰한 프로젝트의 쓰기는 그 프로젝트의 런타임 환경(mise 가 PATH 를 바꿀 수 있다, D26)에서 찾는다. 설정
//! 창은 프로젝트가 없으므로 로그인 셸의 것을 보인다(화면이 그렇게 적는다).

use std::path::PathBuf;
use std::sync::Arc;

use pw_rpc::{Router, RpcError, rpc_method};
use pw_runtime::methods::RuntimeSource;
use pw_runtime::program::find_program;
use serde::{Deserialize, Serialize};

use crate::Git;
use crate::cli::{Access, GitCommand, MIN_VERSION, child_env};

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitExecutable {
    /// 전역 설정 `[git] path` 의 값. 없으면 `null`(그때 `git` 을 찾는다).
    pub configured: Option<String>,
    /// 찾은 실행 파일(절대 경로). 찾지 못했으면 `null`.
    pub program: Option<String>,
    /// `git --version` 에서 읽은 버전(`2.54.0`). 찾지 못했거나 읽지 못했으면 `null`.
    pub version: Option<String>,
    /// 쓰기를 허락하는 가장 낮은 버전(`2.40`).
    pub minimum: String,
    /// 쓰기를 끈 이유. 쓸 수 있으면 `null`.
    pub writes_off: Option<GitWritesOff>,
    /// 찾아본 PATH 를 읽은 곳(로그인 셸, 읽지 못했으면 앱 프로세스).
    pub path_source: RuntimeSource,
    /// 로그인 셸을 읽지 못했으면 그 이유(그때 앱 프로세스의 PATH 로 찾았다).
    pub shell_failure: Option<String>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum GitWritesOff {
    /// 실행 파일을 찾지 못했다. `program` 은 찾은 이름(설정값 또는 `git`).
    NotFound { program: String },
    /// 버전이 `minimum` 미만이다.
    TooOld { version: String },
    /// `git --version` 을 읽지 못했다 — 그 까닭(영어 원문).
    Unreadable { reason: String },
}

rpc_method! {
    /// 설정 창 「Git」: git 실행 파일 경로 · 찾은 버전 · 쓰기를 끈 이유. 로그인 셸의 PATH 로 찾고 고정 argv `git --version` 만
    /// 띄운다(작업 디렉터리는 어느 프로젝트 안도 아닌 곳). 처음이면 로그인 셸을 기다린다.
    pub GitExecutableMethod: "git.executable" {
        params: (),
        result: GitExecutable,
        tier: Read,
        roles: [Owner],
    }
}

/// `git --version` 의 작업 디렉터리. 어느 프로젝트 안도 아니다 — 프로젝트 루트가 파일 시스템 뿌리일 수는 없다(레지스트리가
/// 받지 않는다고 기대하지 않고, 뿌리는 어떤 프로젝트 루트의 아래도 아니다).
fn neutral_dir() -> PathBuf {
    #[cfg(unix)]
    {
        PathBuf::from("/")
    }
    #[cfg(not(unix))]
    {
        std::env::temp_dir()
    }
}

impl Git {
    /// `git.executable`.
    pub async fn executable(&self) -> GitExecutable {
        let runtime = Arc::clone(&self.runtime);
        let (layer, shell_failure) = tokio::task::spawn_blocking(move || runtime.login_shell_env())
            .await
            .unwrap_or_else(|e| std::panic::resume_unwind(e.into_panic()));
        let configured = self.configured_setting();
        let name = configured.clone().unwrap_or_else(|| "git".to_owned());
        let mut info = GitExecutable {
            configured,
            program: None,
            version: None,
            minimum: format!("{}.{}", MIN_VERSION.major, MIN_VERSION.minor),
            writes_off: None,
            path_source: layer.source.clone(),
            shell_failure,
        };
        let Some(program) = find_program(&name, &layer.env.path) else {
            info.writes_off = Some(GitWritesOff::NotFound { program: name });
            return info;
        };
        info.program = Some(program.display().to_string());
        let env = child_env(&layer.env, Access::Read, None);
        let command = GitCommand::new(program, neutral_dir(), env, Access::Read);
        match self.version(&command).await {
            Ok(version) => {
                info.version = Some(version.to_string());
                if version < MIN_VERSION {
                    info.writes_off = Some(GitWritesOff::TooOld {
                        version: version.to_string(),
                    });
                }
            }
            Err(reason) => info.writes_off = Some(GitWritesOff::Unreadable { reason }),
        }
        info
    }

    /// `[git] path` 의 값(비지 않은 문자열일 때만 — `Git::configured_program` 이 `git` 대신 쓰는 값).
    fn configured_setting(&self) -> Option<String> {
        let snapshot = self.host.get()?.global.snapshot();
        match snapshot.get("settings", crate::cli::SETTINGS_KEY) {
            Some(serde_json::Value::String(path)) if !path.is_empty() => Some(path.clone()),
            _ => None,
        }
    }
}

pub fn register(router: &mut Router, git: Arc<Git>) {
    router.unary(GitExecutableMethod, move |_ctx, _params: ()| {
        let git = Arc::clone(&git);
        async move { Ok::<_, RpcError>(git.executable().await) }
    });
}

#[cfg(all(test, unix))]
mod tests;
