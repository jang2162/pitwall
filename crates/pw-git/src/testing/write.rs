//! 쓰기 실행기의 시험 도우미(unix): 진짜 git 을 PATH 에 둔 가짜 로그인 셸, 레지스트리 · 설정 · Task 를 갖춘 [`WriteFixture`],
//! 질문에 답하는 창 흉내 [`Window`], askpass 를 부르는 시험 원격 도우미 [`credential_remote`].
//!
//! 사람의 git 설정 · 셸을 읽지 않는다: 가짜 셸이 `HOME` · `XDG_CONFIG_HOME` 을 임시 폴더로 두고 전역 설정을 빈 파일로, 시스템 설정을
//! 끈다. 신원은 시험 신원이다(merge 를 계속하는 커밋 등).

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use pw_core::bus::{DEFAULT_CAPACITY, EventBus, Scope};
use pw_core::config::{AppPaths, ConfigSource, ConfigStore};
use pw_core::registry::{NewProject, Registry};
use pw_core::task::TaskSpawner;
use pw_runtime::{LoginShell, Runtime, RuntimeProvider};
use tokio_util::sync::CancellationToken;

use super::TestRepo;
use crate::write::{Askpass, GitPromptAnswer, GitPromptEvent, GitPromptRequest, GitWrite};
use crate::{Git, HostParts};

/// 시험 프로세스의 PATH 에서 찾은 진짜 git.
pub fn which_git() -> PathBuf {
    let path = std::env::var_os("PATH").unwrap_or_default();
    pw_runtime::program::find_program("git", &path).expect("git is on PATH")
}

/// 가짜 로그인 셸: PATH 를 `<bin>:<진짜 git 의 폴더>:/usr/bin:/bin` 으로, `HOME` · git 설정 자리를 `dir` 안으로 두고 받은
/// 스크립트를 돌린다(`pw_runtime::LoginShell` 은 `<셸> -l -i -c <스크립트>` 로 부른다).
pub fn fake_login_shell(dir: &Path, bin: &Path) -> PathBuf {
    let home = dir.join("home");
    fs::create_dir_all(&home).unwrap();
    let global = home.join("gitconfig");
    fs::write(&global, "").unwrap();
    let git_dir = which_git().parent().unwrap().to_path_buf();
    let script = format!(
        "#!/bin/sh\nPATH='{bin}:{git_dir}:/usr/bin:/bin'; export PATH\nHOME='{home}'; export HOME\nXDG_CONFIG_HOME='{home}/.config'; export XDG_CONFIG_HOME\nGIT_CONFIG_GLOBAL='{global}'; export GIT_CONFIG_GLOBAL\nGIT_CONFIG_NOSYSTEM=1; export GIT_CONFIG_NOSYSTEM\nGIT_AUTHOR_NAME='Pitwall Test'; export GIT_AUTHOR_NAME\nGIT_AUTHOR_EMAIL='test@pitwall.invalid'; export GIT_AUTHOR_EMAIL\nGIT_COMMITTER_NAME='Pitwall Test'; export GIT_COMMITTER_NAME\nGIT_COMMITTER_EMAIL='test@pitwall.invalid'; export GIT_COMMITTER_EMAIL\neval \"$4\"\n",
        bin = bin.display(),
        git_dir = git_dir.display(),
        home = home.display(),
        global = global.display(),
    );
    let shell = dir.join("fake-shell");
    fs::write(&shell, script).unwrap();
    fs::set_permissions(&shell, fs::Permissions::from_mode(0o755)).unwrap();
    shell
}

/// 시험 저장소 하나를 프로젝트로 둔 쓰기 실행기. tokio 런타임 안에서 만든다.
pub struct WriteFixture {
    pub repo: TestRepo,
    pub write: Arc<GitWrite>,
    pub projects: Registry,
    /// Host 버스(Task · 알림).
    pub bus: EventBus,
    pub tasks: TaskSpawner,
    pub project: String,
    /// 가짜 셸 PATH 의 맨 앞 폴더(시험 원격 도우미 등을 둔다).
    pub bin: PathBuf,
    /// 앱 폴더(`$CONFIG` · `$DATA` · `$STATE` 가 이 아래다).
    pub app: PathBuf,
    _dir: tempfile::TempDir,
}

impl WriteFixture {
    /// 첫 커밋이 있는 저장소. `trusted` 면 프로젝트를 신뢰한다.
    pub fn new(trusted: bool) -> Self {
        Self::with_repo(TestRepo::with_commit(), trusted, Askpass::default())
    }

    /// `repo` 를 프로젝트로. askpass 는 `askpass`(소켓은 [`Askpass::serve`] 로 연다).
    pub fn with_repo(repo: TestRepo, trusted: bool, askpass: Askpass) -> Self {
        Self::with_options(repo, trusted, askpass, None)
    }

    /// [`Self::with_repo`] 에 질문 시간 제한(askpass, 미저장 확인)을 더한 것.
    pub fn with_options(
        repo: TestRepo,
        trusted: bool,
        askpass: Askpass,
        timeouts: Option<(Duration, Duration)>,
    ) -> Self {
        Self::build(repo, trusted, askpass, timeouts, None)
    }

    /// [`Self::with_repo`] 의 런타임에 제공자(mise 자리)를 더한 것.
    pub fn with_provider(
        repo: TestRepo,
        trusted: bool,
        provider: Arc<dyn RuntimeProvider>,
    ) -> Self {
        Self::build(repo, trusted, Askpass::default(), None, Some(provider))
    }

    fn build(
        repo: TestRepo,
        trusted: bool,
        askpass: Askpass,
        timeouts: Option<(Duration, Duration)>,
        provider: Option<Arc<dyn RuntimeProvider>>,
    ) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        fs::create_dir_all(&bin).unwrap();
        let shell = fake_login_shell(dir.path(), &bin);
        let app = dir.path().join("app");
        let paths = AppPaths::under(&app);
        fs::create_dir_all(&paths.config).unwrap();
        let bus = EventBus::new(DEFAULT_CAPACITY);
        let global = ConfigStore::open(ConfigSource::global(&paths), bus.clone(), Scope::Host);
        let projects = Registry::new(paths, None, bus.clone());
        let gate = projects.clone();
        let mut runtime = Runtime::new(LoginShell::new(Some(shell), Duration::from_secs(10)))
            .with_trust(move |id| gate.is_trusted(id));
        if let Some(provider) = provider {
            runtime = runtime.with_provider(provider);
        }
        let git = Arc::new(Git::new(Arc::new(runtime)));
        git.attach(HostParts {
            global,
            projects: projects.clone(),
            bus: bus.clone(),
        });
        // 파일 서비스는 데스크톱처럼 같은 레지스트리를 붙인다(충돌 해결이 `fs.write` 와 쓰기 잠금을 나눈다).
        let fs = Arc::new(pw_fs::Fs::new());
        fs.attach(projects.clone());
        let mut write = GitWrite::new(git, askpass, fs);
        if let Some((askpass, confirm)) = timeouts {
            write = write.with_timeouts(askpass, confirm);
        }
        let write = Arc::new(write);
        let tasks = TaskSpawner::new(bus.clone(), Scope::Host, CancellationToken::new());
        write.attach(tasks.clone());
        let project = projects
            .create(NewProject {
                root: repo.root().to_path_buf(),
                ..NewProject::default()
            })
            .unwrap()
            .id;
        projects.set_trust(&project, trusted).unwrap();
        Self {
            repo,
            write,
            projects,
            bus,
            tasks,
            project,
            bin,
            app,
            _dir: dir,
        }
    }

    /// 이 프로젝트 창의 연결 범위(쓰기를 부를 수 있는 연결).
    pub fn caller(&self) -> pw_rpc::ConnectionScope {
        pw_rpc::ConnectionScope::Project(self.project.clone())
    }

    /// 이 프로젝트의 쓰기 요청(작업 트리를 바꾸지 않고 인증을 묻지 않는 쓰기).
    pub fn request(&self, title: &str) -> crate::write::WriteRequest {
        crate::write::WriteRequest {
            caller: self.caller(),
            project_id: self.project.clone(),
            worktree: None,
            title: title.to_owned(),
            changes: crate::write::Changes::None,
            askpass: false,
        }
    }
}

/// 이 프로젝트를 듣는 창 흉내: 받은 질문을 모으고 `answer` 로 답한다(`None` 이면 답하지 않는다). 버리면 듣기를 그만둔다.
pub struct Window {
    seen: Arc<Mutex<Vec<GitPromptRequest>>>,
    task: tokio::task::JoinHandle<()>,
}

impl Window {
    pub fn open(
        write: &Arc<GitWrite>,
        project: &str,
        answer: impl Fn(&GitPromptRequest) -> Option<GitPromptAnswer> + Send + 'static,
    ) -> Self {
        // 듣기는 여기서 건다 — 돌려준 뒤 시작한 쓰기의 질문을 놓치지 않는다.
        let mut listener = write.prompts().listen(project);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let (prompts, record, project) = (
            write.prompts().clone(),
            Arc::clone(&seen),
            project.to_owned(),
        );
        let task = tokio::spawn(async move {
            while let Some(event) = listener.recv().await {
                let GitPromptEvent::Open { prompt } = event else {
                    continue;
                };
                record
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push(prompt.request.clone());
                if let Some(reply) = answer(&prompt.request) {
                    let _ = prompts.answer(&project, prompt.prompt_id, reply);
                }
            }
        });
        Self { seen, task }
    }

    /// 지금까지 받은 질문.
    pub fn seen(&self) -> Vec<GitPromptRequest> {
        self.seen
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// `bin/git-remote-testauth` — git 이 `testauth::<무엇이든>` 원격에 쓰는 원격 도우미. 받은 `$GIT_ASKPASS` 로 암호를 묻고 그 stdout
/// 을 `out` 에, 종료 코드를 `out.code` 에, askpass 경로를 `out.askpass` 에 적은 뒤 실패로 끝난다(git 은 그 원격 작업을 실패로
/// 끝낸다). 진짜 원격 없이 「git 이 askpass 를 부른다」 를 만든다.
pub fn credential_remote(bin: &Path, out: &Path, prompt: &str) -> PathBuf {
    let path = bin.join("git-remote-testauth");
    let out = out.display();
    let prompt = prompt.replace('\'', r"'\''");
    fs::write(
        &path,
        format!(
            "#!/bin/sh\nprintf '%s' \"$GIT_ASKPASS\" > '{out}.askpass'\n\"$GIT_ASKPASS\" '{prompt}' > '{out}.partial'\necho $? > '{out}.code'\nmv '{out}.partial' '{out}'\nexit 1\n"
        ),
    )
    .unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}
