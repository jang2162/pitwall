//! 런타임 제공자(03 §5-1 「런타임 제공자」 표).
//!
//! 환경은 층으로 쌓인다: 앱 프로세스의 환경 → 로그인 셸([`crate::LoginShell`], 언제나) → 디렉터리의 제공자 하나(mise 등). 아래
//! 층이 실패하면 그 아래 층으로 물러난다.

use std::path::Path;

use crate::env::RuntimeEnv;
use crate::methods::{RuntimeProblem, RuntimeSource};

/// 제공자가 만든 환경과 그 출처.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layer {
    pub env: RuntimeEnv,
    pub source: RuntimeSource,
}

pub trait RuntimeProvider: Send + Sync {
    /// `runtime.get` 의 `provider` 값(`"shell"`, `"mise"` …). 프로젝트 설정 `[runtime] provider` 가 고르는 이름이기도 하다.
    fn id(&self) -> &'static str;

    /// 프로젝트의 파일 · 스크립트를 실행할 수 있는가. 그런 제공자는 신뢰한 프로젝트에서만 고른다(03 §5-1 「신뢰」).
    fn runs_project_code(&self) -> bool;

    /// `dir` 에 이 제공자를 쓸 것인가(감지). 실행 파일은 `base` 의 PATH 로 찾는다.
    fn applies_to(&self, dir: &Path, base: &RuntimeEnv) -> bool;

    /// `base` 위에 `dir` 의 환경을 만든다. 실패면 그 갈래와 이유 — 부른 쪽이 `base` 로 물러나고 결과를 캐시하지 않는다.
    fn resolve(&self, dir: &Path, base: &RuntimeEnv) -> Result<Layer, ProviderError>;
}

/// 제공자의 실패.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderError {
    pub problem: RuntimeProblem,
    /// 사람이 읽는 이유(도구의 오류 원문을 담는다).
    pub reason: String,
}

impl ProviderError {
    pub fn new(problem: RuntimeProblem, reason: impl Into<String>) -> Self {
        Self {
            problem,
            reason: reason.into(),
        }
    }
}

/// 갈래를 모르는 실패는 [`RuntimeProblem::Failed`] 다.
impl From<String> for ProviderError {
    fn from(reason: String) -> Self {
        Self::new(RuntimeProblem::Failed, reason)
    }
}
