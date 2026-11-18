//! 신뢰 판정을 지나는 감지 실행기(17-m6-plan M6-05 — F1-7). 프로세스를 띄우는 감지기(mise · gradle · maven)는 이것으로만 띄운다
//! ([`super::DetectExec`]).
//!
//! - **신뢰**: 부를 때마다 그 프로젝트의 신뢰를 본다(`Runtime::with_trust` 와 같은 판정 — 레지스트리의 `is_trusted`). 미신뢰면
//!   아무것도 띄우지 않고 까닭이다 — 감지 목록에는 `detectProblems` 로 남는다.
//! - **환경**: 그 프로젝트 · 작업 폴더의 런타임 환경(로그인 셸 + mise — `pw_runtime::Runtime::env`). `mise` 는 그 PATH 에서
//!   `pw_runtime::mise::find_mise` 로, 맨 이름 프로그램(`gradle` · `mvn`)은 `pw_runtime::program::find_program` 으로 찾는다. 래퍼
//!   (`gradlew` · `mvnw`)는 감지기가 루트 안의 절대 경로로 넘긴다.
//! - **시한 · 상한**([`ExecLimits`]): 자동 감지(목록 · 파일 감시)는 [`AUTOMATIC`](30초), 사람이 「태스크 읽기」 를 누른 것은
//!   [`PRESSED`](5분 — Host 작업으로 진행 · 취소가 보인다). 출력이 상한을 넘거나 시한을 넘기면 세션째 끝내고 까닭이다.
//! - **남기지 않는다**: 자식은 새 세션에서 돌고, 끝나면(스스로 끝나도 · 시한 · 상한 · 취소) 그 세션의 프로세스와 **도는 동안 본
//!   자손**(부모 pid 를 따라 — 스스로 `setsid` 한 것도. gradle 은 `org.gradle.jvmargs` 가 있으면 `--no-daemon` 이어도 1회용 데몬을
//!   띄우고 그 데몬은 새 세션이 된다 — M6-05 리뷰 실측)을 SIGTERM → [`END_GRACE`] → SIGKILL 로 끝낸다(`pw_runtime::process::run_watched`
//!   \+ `crate::tree::Descendants`). 닿지 않는 것은 관찰 사이(약 100 ms)에 태어나 부모를 떠난 것, 그리고 Windows(세션 · 자손 찾기가
//!   없어 자식만 끝낸다)다. stdin 은 `/dev/null` 이다.
//! - **신뢰를 거두면 멈춘다**: 도는 동안 약 1 초마다 신뢰를 다시 보고, 거두어졌으면 위와 같이 끝내고 까닭이다.
//! - **까닭에 출력을 싣지 않는다**: 실패의 까닭은 프로그램 이름과 끝 코드뿐이다 — stderr 에는 설정 파일의 줄(값)이 섞인다.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use pw_runtime::process::{self, RunError};
use pw_runtime::{Cancel, RuntimeEnv};

use super::DetectExec;

/// 감지 프로세스 하나의 시한과 출력(stdout + stderr) 상한.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExecLimits {
    pub timeout: Duration,
    pub max_output: usize,
}

/// 자동 감지(목록 · 파일 감시가 부르는 `mise tasks ls`)의 상한 — 17-m6-plan M6-05 ③ 의 「시한 30초」.
pub const AUTOMATIC: ExecLimits = ExecLimits {
    timeout: Duration::from_secs(30),
    max_output: 8 * 1024 * 1024,
};

/// 사람이 「태스크 읽기」 를 누른 것(`run.tasks.refresh` — gradle · maven)의 상한. 첫 실행은 래퍼 배포판 · 의존성을 내려받아 수십
/// 초를 넘기므로 5분이다(메인 판정). 그동안 Host 작업으로 보이고 `task.cancel` 로 멈춘다.
pub const PRESSED: ExecLimits = ExecLimits {
    timeout: Duration::from_secs(5 * 60),
    max_output: 32 * 1024 * 1024,
};

/// 끝낼 때 SIGTERM 뒤 SIGKILL 까지 기다리는 시간(인터럽트를 삼키는 빌드 스크립트는 SIGKILL 로 끝난다).
pub const END_GRACE: Duration = Duration::from_secs(1);
/// 도는 동안 자손을 기록하는 간격.
const OBSERVE_EVERY: Duration = Duration::from_millis(100);
/// 도는 동안 신뢰를 다시 보는 간격.
const TRUST_EVERY: Duration = Duration::from_secs(1);

/// 그 프로젝트의 신뢰를 지금 판정한다.
pub type TrustCheck = Arc<dyn Fn(&str) -> bool + Send + Sync>;
/// 그 프로젝트 · 작업 폴더의 런타임 환경.
pub type EnvSource = Arc<dyn Fn(&str, &Path) -> RuntimeEnv + Send + Sync>;

/// 신뢰 판정을 지나는 실행기 — 프로젝트 하나에 묶인다.
pub struct TrustedExec {
    project_id: String,
    trusted: TrustCheck,
    env: EnvSource,
    limits: ExecLimits,
    cancel: Cancel,
    cache_dir: Option<PathBuf>,
}

impl TrustedExec {
    pub fn new(project_id: &str, trusted: TrustCheck, env: EnvSource, limits: ExecLimits) -> Self {
        Self {
            project_id: project_id.to_owned(),
            trusted,
            env,
            limits,
            cancel: Cancel::default(),
            cache_dir: None,
        }
    }

    /// 이 취소 신호가 오면 도는 프로세스를 세션째 끝낸다(Host 작업의 취소).
    #[must_use]
    pub fn with_cancel(mut self, cancel: Cancel) -> Self {
        self.cancel = cancel;
        self
    }

    /// 누를 때만 읽는 태스크 목록의 캐시 폴더(`$DATA/projects/<id>/` — [`super::cache`]).
    #[must_use]
    pub fn with_cache_dir(mut self, dir: PathBuf) -> Self {
        self.cache_dir = Some(dir);
        self
    }

    /// 판정 · 띄우기를 거치지 않고 신뢰만 본다.
    pub fn trusted(&self) -> bool {
        (self.trusted)(&self.project_id)
    }
}

impl TrustedExec {
    /// 띄우고 끝날 때까지: 도는 동안 자손을 기록하고 신뢰를 다시 보며, 끝나면 세션과 기록한 자손을 끝낸다(머리 주석).
    fn run(
        &self,
        command: Command,
        withdrawn: &std::cell::Cell<bool>,
    ) -> Result<process::Output, RunError> {
        #[cfg(unix)]
        let mut descendants: Option<crate::tree::Descendants> = None;
        let started = std::time::Instant::now();
        let (mut observed, mut checked) = (started, started);
        let cancel = self.cancel.clone();
        let mut watch = |event: process::Watch| match event {
            process::Watch::Started(pid) => {
                #[cfg(unix)]
                {
                    let mut d = crate::tree::Descendants::new(pid as libc::pid_t);
                    d.observe();
                    descendants = Some(d);
                }
                let _ = pid;
            }
            process::Watch::Running => {
                let now = std::time::Instant::now();
                if now.duration_since(observed) >= OBSERVE_EVERY {
                    observed = now;
                    #[cfg(unix)]
                    if let Some(d) = descendants.as_mut() {
                        d.observe();
                    }
                }
                if now.duration_since(checked) >= TRUST_EVERY {
                    checked = now;
                    if !self.trusted() {
                        withdrawn.set(true);
                        cancel.cancel();
                    }
                }
            }
            process::Watch::Ended =>
            {
                #[cfg(unix)]
                if let Some(d) = descendants.take() {
                    d.end(&[(libc::SIGTERM, END_GRACE)]);
                }
            }
        };
        process::run_watched(
            command,
            self.limits.timeout,
            self.limits.max_output,
            &self.cancel,
            &mut watch,
        )
    }
}

/// 그 프로젝트 창이 아닌 연결(Host · 다른 프로젝트 창 · 파일 창 · 에이전트)의 목록이 쓰는 실행기: 아무것도 띄우지 않고 캐시만 읽는다
/// — 프로세스를 띄우는 감지(mise)는 까닭 [`NOT_THIS_WINDOW`] 로 비고, 누를 때만 읽는 목록(gradle · maven)은 캐시 그대로 보인다. 그
/// 프로젝트의 코드는 그 프로젝트 창만 돌린다(M6-05 리뷰 권고 2 — 메인 판정).
pub struct CacheOnly {
    cache_dir: Option<PathBuf>,
}

impl CacheOnly {
    pub fn new(cache_dir: Option<PathBuf>) -> Self {
        Self { cache_dir }
    }
}

/// [`CacheOnly`] 의 까닭.
pub const NOT_THIS_WINDOW: &str =
    "tasks that need a process to list (mise) are listed only in the project's own window";

impl DetectExec for CacheOnly {
    fn output(&self, _detector: &str, _argv: &[String], _cwd: &Path) -> Result<String, String> {
        Err(NOT_THIS_WINDOW.to_owned())
    }

    fn cache_dir(&self) -> Option<&Path> {
        self.cache_dir.as_deref()
    }
}

/// 감지 실행이 거절 · 실패한 까닭의 머리 — 미신뢰.
pub fn untrusted_reason(program: &str) -> String {
    format!(
        "Pitwall runs `{program}` to list tasks only in a trusted project; trust the project first"
    )
}

impl DetectExec for TrustedExec {
    fn output(&self, detector: &str, argv: &[String], cwd: &Path) -> Result<String, String> {
        let Some(program) = argv.first() else {
            return Err(format!("the `{detector}` detector asked to run nothing"));
        };
        let shown = Path::new(program)
            .file_name()
            .map_or_else(|| program.clone(), |n| n.to_string_lossy().into_owned());
        if !self.trusted() {
            return Err(untrusted_reason(&shown));
        }
        let env = (self.env)(&self.project_id, cwd);
        let path = find(program, &env)
            .ok_or_else(|| format!("`{shown}` is not on the runtime PATH of this project"))?;
        let mut command = Command::new(&path);
        command
            .args(&argv[1..])
            .current_dir(cwd)
            .env_clear()
            .envs(&env.vars)
            .env("PATH", &env.path)
            // 색 코드 · 진행 막대가 출력에 섞이지 않게.
            .env("NO_COLOR", "1")
            .env("TERM", "dumb");
        let withdrawn = std::cell::Cell::new(false);
        let output = self.run(command, &withdrawn).map_err(|e| match e {
            RunError::Cancelled if withdrawn.get() => untrusted_reason(&shown),
            RunError::Spawn(e) => format!("cannot start `{shown}`: {e}"),
            RunError::TimedOut => format!(
                "`{shown}` did not finish within {:?}; Pitwall stopped it",
                self.limits.timeout
            ),
            RunError::TooLarge => format!(
                "`{shown}` printed more than {} MiB; Pitwall stopped it",
                self.limits.max_output / (1024 * 1024)
            ),
            RunError::Cancelled => format!("`{shown}` was cancelled"),
        })?;
        let stderr = String::from_utf8_lossy(&output.stderr);
        if detector == super::mise::ID && super::mise::not_trusted_by_mise(&stderr) {
            return Err(super::mise::NOT_TRUSTED_REASON.to_owned());
        }
        if !output.status.success() {
            return Err(format!(
                "`{shown}` failed ({}); run it in a terminal to see why",
                output.status
            ));
        }
        String::from_utf8(output.stdout)
            .map_err(|_| format!("`{shown}` printed text that is not UTF-8"))
    }

    fn cache_dir(&self) -> Option<&Path> {
        self.cache_dir.as_deref()
    }
}

/// 띄울 실행 파일: `mise` 는 런타임 PATH 에서 `find_mise`, 절대 경로(래퍼)는 그대로(실행 파일이어야), 그 밖의 맨 이름은 PATH 에서.
fn find(program: &str, env: &RuntimeEnv) -> Option<PathBuf> {
    if program == "mise" {
        return pw_runtime::mise::find_mise(env);
    }
    let path: OsString = env.path.clone();
    pw_runtime::program::find_program(program, &path)
}
