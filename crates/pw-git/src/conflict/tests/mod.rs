//! 충돌 데이터 · 해결 시험. 구간 계산은 `git merge-file -p --diff3` 와 견주고([`merge`]), 저장소는 진짜 git 으로 임시 폴더에
//! 만든다(`crate::testing`). 읽기([`read`])는 모든 OS 에서, 해결([`resolve`])과 미신뢰 저장소([`untrusted`])는 쓰기 실행기의
//! 시험 도우미(가짜 로그인 셸)로 unix 에서 돈다.
//!
//! gix 가 사람의 전역 · 시스템 git 설정을 읽지 않게 시험마다 런타임을 띄우기 전에 [`isolate`] 한다(`#[tokio::test]` 대신
//! [`block_on`] — 환경을 바꾼 뒤에 스레드를 띄운다).

mod merge;
mod read;
#[cfg(unix)]
mod resolve;
#[cfg(unix)]
mod untrusted;

#[cfg(unix)]
use std::future::Future;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use pw_core::bus::{DEFAULT_CAPACITY, EventBus, Scope};
use pw_core::config::{AppPaths, ConfigSource, ConfigStore};
use pw_core::registry::{NewProject, Registry};
use pw_runtime::{LoginShell, Runtime};

use super::merge::Kind;
use crate::diff::hunks::lines;
use crate::testing::TestRepo;
use crate::testing::repo::git_command;
use crate::{Git, HostParts};

/// HOME · XDG · `GIT_CONFIG_GLOBAL` 을 `<base>/home` 으로 돌리고 시스템 설정을 끈다.
fn isolate(base: &Path) {
    let home = base.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let config = home.join(".gitconfig");
    if !config.exists() {
        std::fs::write(&config, "").unwrap();
    }
    // SAFETY: nextest 는 시험마다 프로세스가 따로이고, 이것은 다른 스레드(런타임)를 띄우기 전에 부른다.
    unsafe {
        std::env::set_var("HOME", &home);
        std::env::set_var("XDG_CONFIG_HOME", home.join(".config"));
        std::env::set_var("GIT_CONFIG_GLOBAL", &config);
        std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
    }
}

/// 다중 스레드 런타임에서 `f` 를 끝까지 돌린다(해결 시험 — unix).
#[cfg(unix)]
fn block_on<F: Future>(f: F) -> F::Output {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap()
        .block_on(f)
}

/// 시험 저장소 하나를 프로젝트로 둔 Host 의 git(읽기 시험). 로그인 셸은 없다 — 읽기는 git CLI 를 띄우지 않는다.
struct Host {
    repo: TestRepo,
    git: Git,
    project: String,
    _app: tempfile::TempDir,
}

impl Host {
    fn new(repo: TestRepo) -> Self {
        isolate(repo.base());
        let app = tempfile::tempdir().unwrap();
        let paths = AppPaths::under(&app.path().join("app"));
        std::fs::create_dir_all(&paths.config).unwrap();
        let bus = EventBus::new(DEFAULT_CAPACITY);
        let global = ConfigStore::open(ConfigSource::global(&paths), bus.clone(), Scope::Host);
        let projects = Registry::new(paths, None, bus.clone());
        let gate = projects.clone();
        let runtime = Runtime::new(LoginShell::new(None, Duration::from_secs(5)))
            .with_trust(move |id| gate.is_trusted(id));
        let git = Git::new(Arc::new(runtime));
        git.attach(HostParts {
            global,
            projects: projects.clone(),
            bus,
        });
        let project = projects
            .create(NewProject {
                root: repo.root().to_path_buf(),
                ..NewProject::default()
            })
            .unwrap()
            .id;
        projects.set_trust(&project, true).unwrap();
        Self {
            repo,
            git,
            project,
            _app: app,
        }
    }
}

/// git 의 충돌 표시로 나눈 병합 결과의 한 부분.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Section {
    Plain(Vec<u8>),
    Conflict {
        ours: Vec<u8>,
        base: Vec<u8>,
        theirs: Vec<u8>,
    },
}

/// `git merge-file -p --diff3 -L O -L B -L T` 의 출력을 나눈다. 표시 줄의 `\r` 은 뗀다(CRLF 파일이면 git 이 표시에도 붙인다).
fn parse(out: &[u8]) -> Vec<Section> {
    let mut sections = Vec::new();
    let (mut plain, mut parts) = (Vec::new(), [Vec::new(), Vec::new(), Vec::new()]);
    let mut state = 0;
    for line in lines(out) {
        let bare = line.strip_suffix(b"\n").unwrap_or(line);
        let bare = bare.strip_suffix(b"\r").unwrap_or(bare);
        match (state, bare) {
            (0, b"<<<<<<< O") => {
                sections.push(Section::Plain(std::mem::take(&mut plain)));
                state = 1;
            }
            (1, b"||||||| B") => state = 2,
            (2, b"=======") => state = 3,
            (3, b">>>>>>> T") => {
                let [ours, base, theirs] = std::mem::take(&mut parts);
                sections.push(Section::Conflict { ours, base, theirs });
                state = 0;
            }
            (0, _) => plain.extend_from_slice(line),
            // 1 · 2 · 3 = ours · base · theirs
            (n, _) => parts[n - 1].extend_from_slice(line),
        }
    }
    assert_eq!(
        state,
        0,
        "unterminated conflict in {}",
        String::from_utf8_lossy(out)
    );
    sections.push(Section::Plain(plain));
    sections.retain(|s| !matches!(s, Section::Plain(p) if p.is_empty()));
    sections
}

/// 병합 결과 하나(줄 범위는 0 기준 · 끝 제외)를 git 의 꼴로: 충돌 사이는 결과 줄, 충돌은 세 쪽의 그 줄. git 은 충돌 쪽 글이 개행
/// 없이 끝나면 표시 앞에 개행을 붙인다.
fn sections(
    merged: &[u8],
    regions: &[(Kind, [std::ops::Range<u32>; 4])],
    base: &[u8],
    ours: &[u8],
    theirs: &[u8],
) -> Vec<Section> {
    let (merged, base, ours, theirs) = (lines(merged), lines(base), lines(ours), lines(theirs));
    let slice = |side: &[&[u8]], r: &std::ops::Range<u32>| {
        let mut text = side[r.start as usize..r.end as usize].concat();
        if !text.is_empty() && !text.ends_with(b"\n") {
            text.push(b'\n');
        }
        text
    };
    let mut out = Vec::new();
    let mut at = 0;
    for (kind, [b, o, t, m]) in regions {
        if *kind != Kind::Conflict {
            continue;
        }
        assert!(m.is_empty(), "a conflict is empty in the result");
        out.push(Section::Plain(merged[at..m.start as usize].concat()));
        out.push(Section::Conflict {
            ours: slice(&ours, o),
            base: slice(&base, b),
            theirs: slice(&theirs, t),
        });
        at = m.start as usize;
    }
    out.push(Section::Plain(merged[at..].concat()));
    out.retain(|s| !matches!(s, Section::Plain(p) if p.is_empty()));
    out
}

/// `dir` 에 세 쪽을 쓰고 `git merge-file -p --diff3` 의 출력을 나눈다.
fn merge_file(dir: &Path, base: &[u8], ours: &[u8], theirs: &[u8]) -> Vec<Section> {
    for (name, data) in [("base", base), ("ours", ours), ("theirs", theirs)] {
        std::fs::write(dir.join(name), data).unwrap();
    }
    let out = git_command(dir, dir)
        .args([
            "merge-file",
            "-p",
            "--diff3",
            "-L",
            "O",
            "-L",
            "B",
            "-L",
            "T",
            "ours",
            "base",
            "theirs",
        ])
        .output()
        .expect("run git merge-file");
    // 종료 코드는 충돌 수다(음수면 오류).
    assert!(
        out.status.code().is_some_and(|c| (0..=127).contains(&c)),
        "git merge-file failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    parse(&out.stdout)
}
