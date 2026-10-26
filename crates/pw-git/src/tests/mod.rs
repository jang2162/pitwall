//! 저장소는 진짜 git 으로 임시 폴더에 만든다(`crate::testing`). 실행기 시험은 가짜 `git`(`testing::fake`)과 가짜 로그인 셸로
//! 환경을 정해 본다(unix). 미신뢰 저장소의 설정이 코드를 돌리지 않는지는 진짜 git 과 표식을 남기는 sh 스크립트로 본다(unix).

mod repo;

#[cfg(unix)]
mod cli;
#[cfg(unix)]
mod untrusted;

use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use pw_core::bus::{DEFAULT_CAPACITY, EventBus, Scope};
use pw_core::config::{AppPaths, ConfigSource, ConfigStore};
use pw_core::registry::{NewProject, Registry as Projects};
use pw_runtime::methods::RuntimeSource;
use pw_runtime::{Cancel, Layer, LoginShell, ProviderError, Runtime, RuntimeEnv, RuntimeProvider};

use super::*;

/// 신뢰한 프로젝트에서만 고르는 제공자(mise 자리). 환경에 `PW_FAKE_MISE=1` 을 더한다.
struct FakeMise;

impl RuntimeProvider for FakeMise {
    fn id(&self) -> &'static str {
        "mise"
    }

    fn runs_project_code(&self) -> bool {
        true
    }

    fn applies_to(&self, _dir: &Path, _base: &RuntimeEnv) -> bool {
        true
    }

    fn resolve(
        &self,
        _dir: &Path,
        base: &RuntimeEnv,
        _cancel: &Cancel,
    ) -> Result<Layer, ProviderError> {
        let mut env = base.clone();
        env.vars.insert("PW_FAKE_MISE".into(), "1".into());
        Ok(Layer {
            env,
            source: RuntimeSource::ConfigFiles { files: Vec::new() },
        })
    }
}

struct Fixture {
    /// 앱 데이터 폴더(설정 · 레지스트리)를 시험 동안 살려 둔다.
    _tmp: tempfile::TempDir,
    git: Git,
    projects: Projects,
    /// 알림을 받아 보는 시험(unix 의 실행기 시험)만 쓴다.
    #[cfg(unix)]
    bus: EventBus,
}

impl Fixture {
    /// `settings` 는 전역 `settings.toml` 내용. `shell` 이 있으면 그것이 로그인 셸이다(없으면 셸을 읽지 못해 앱 프로세스의
    /// 환경으로 물러난다).
    fn new(settings: &str, shell: Option<&Path>) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::under(&tmp.path().join("app"));
        fs::create_dir_all(&paths.config).unwrap();
        fs::write(paths.config.join("settings.toml"), settings).unwrap();
        let bus = EventBus::new(DEFAULT_CAPACITY);
        let global = ConfigStore::open(ConfigSource::global(&paths), bus.clone(), Scope::Host);
        assert_eq!(global.fault(), None);
        let projects = Projects::new(paths, None, bus.clone());
        // 앱과 같이 런타임 게이트가 레지스트리의 신뢰(M1-05)를 본다 — 실행기 · 저장소 열기와 같은 판정이다.
        let gate = projects.clone();
        let runtime = Runtime::new(LoginShell::new(
            shell.map(Path::to_path_buf),
            Duration::from_secs(5),
        ))
        .with_provider(Arc::new(FakeMise))
        .with_trust(move |id| gate.is_trusted(id));
        let git = Git::new(Arc::new(runtime));
        git.attach(HostParts {
            global,
            projects: projects.clone(),
            bus: bus.clone(),
        });
        Self {
            _tmp: tmp,
            git,
            projects,
            #[cfg(unix)]
            bus,
        }
    }

    /// 프로젝트를 신뢰하거나 해제한다(신뢰 확인 창의 답).
    fn trust(&self, project: &str, trusted: bool) {
        self.projects.set_trust(project, trusted).unwrap();
    }

    /// `root` 를 프로젝트로 등록하고 그 id 를 돌려준다.
    fn project(&self, root: &Path) -> String {
        fs::create_dir_all(root).unwrap();
        self.projects
            .create(NewProject {
                root: root.to_path_buf(),
                ..NewProject::default()
            })
            .unwrap()
            .id
    }

    fn repo(&self, project: &str, worktree: Option<&Path>) -> Result<GitRepo, RpcError> {
        self.git.repo(&GitRepoParams {
            project_id: project.to_owned(),
            worktree: worktree.map(|w| w.display().to_string()),
        })
    }
}
