//! 실행기(M2-01 완료 기준 ② ③ ④): 버전 판정과 알림 한 번, 자식 환경, 프로세스 그룹째 취소. 가짜 로그인 셸이 PATH 를 정하고
//! (`bin` 이 맨 앞), 그 PATH 의 가짜 `git` 이 받은 환경 · 인자를 파일에 적는다.

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::time::Instant;

use pw_core::bus::{Filter, Received};
use pw_core::notification::TOPIC_SHOW;

use super::*;
use crate::cli::{Access, GitVersion, Output, RunError, TERM_GRACE, child_env};
use crate::testing::{FakeGit, TestRepo};

/// 가짜 로그인 셸: PATH 를 `<bin>:<extra>:/usr/bin:/bin` 으로 정하고, 실행기가 덮거나 지워야 하는 변수(`GIT_DIR` · 프롬프트 ·
/// 편집기 · 잠금)를 사람의 셸처럼 다른 값으로 둔 뒤 받은 스크립트를 돌린다. 시험 프로세스의 환경에 이미 `GIT_TERMINAL_PROMPT=0` 이
/// 있을 수 있어(에이전트 · CI) 그 값이 그대로 흘러 통과하지 않게 한다.
fn fake_shell(dir: &Path, bin: &Path, extra: &str) -> PathBuf {
    let shell = dir.join("fake-shell");
    fs::write(
        &shell,
        format!(
            "#!/bin/sh\nPATH='{}:{extra}:/usr/bin:/bin'; export PATH\nGIT_DIR=/nonexistent/stray; export GIT_DIR\nGIT_TERMINAL_PROMPT=1; export GIT_TERMINAL_PROMPT\nGIT_EDITOR=vi; export GIT_EDITOR\nGIT_OPTIONAL_LOCKS=1; export GIT_OPTIONAL_LOCKS\neval \"$4\"\n",
            bin.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&shell, fs::Permissions::from_mode(0o755)).unwrap();
    shell
}

struct Cli {
    f: Fixture,
    bin: PathBuf,
    /// 프로젝트로 등록한 폴더(가짜 git 의 작업 디렉터리).
    target: Target,
    _shell_dir: tempfile::TempDir,
}

impl Cli {
    fn new(settings: &str) -> Self {
        Self::with_path(settings, "")
    }

    /// `extra` 는 가짜 셸의 PATH 에 `bin` 다음으로 넣을 항목.
    fn with_path(settings: &str, extra: &str) -> Self {
        let shell_dir = tempfile::tempdir().unwrap();
        let bin = shell_dir.path().join("bin");
        fs::create_dir_all(&bin).unwrap();
        let shell = fake_shell(shell_dir.path(), &bin, extra);
        let f = Fixture::new(settings, Some(&shell));
        let work = f.tmp.path().join("project");
        let id = f.project(&work);
        let target = Target {
            project_id: id,
            project_root: work.clone(),
            work_tree: work,
        };
        Self {
            f,
            bin,
            target,
            _shell_dir: shell_dir,
        }
    }

    fn command(&self, access: Access) -> Result<cli::GitCommand, CliError> {
        tokio_rt().block_on(self.f.git.command(&self.target, access))
    }

    fn run(&self, access: Access, args: &[&str]) -> Output {
        let rt = tokio_rt();
        rt.block_on(async {
            let command = self.f.git.command(&self.target, access).await.unwrap();
            command
                .args(args)
                .output(&CancellationToken::new())
                .await
                .unwrap()
        })
    }
}

fn tokio_rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap()
}

/// 지금까지 버스에 나간 알림.
fn notifications(sub: &mut pw_core::bus::Subscription) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    while let Some(received) = sub.try_recv() {
        if let Received::Event(event) = received {
            assert_eq!(event.scope, Scope::Host);
            out.push(event.payload.clone());
        }
    }
    out
}

#[test]
fn versions_are_read_from_the_first_line() {
    let v = |s: &str| GitVersion::parse(s);
    assert_eq!(
        v("git version 2.54.0 (Apple Git-157)\n"),
        Some(GitVersion {
            major: 2,
            minor: 54,
            patch: 0
        })
    );
    assert_eq!(
        v("git version 2.45.1.windows.1"),
        Some(GitVersion {
            major: 2,
            minor: 45,
            patch: 1
        })
    );
    assert_eq!(
        v("git version 2.40"),
        Some(GitVersion {
            major: 2,
            minor: 40,
            patch: 0
        })
    );
    assert!(v("git version 2.39.5").unwrap() < cli::MIN_VERSION);
    assert!(v("git version 2.40.0").unwrap() >= cli::MIN_VERSION);
    for bad in ["", "hub version 2.14", "git version x.y", "git version 2"] {
        assert_eq!(v(bad), None, "{bad:?}");
    }
}

#[test]
fn the_child_env_turns_off_prompts_and_editors_and_drops_repository_overrides() {
    let mut base = RuntimeEnv {
        path: "/bin".into(),
        ..RuntimeEnv::default()
    };
    for (n, v) in [
        ("HOME", "/home/me"),
        ("GIT_DIR", "/elsewhere/.git"),
        ("GIT_WORK_TREE", "/elsewhere"),
        ("GIT_INDEX_FILE", "/elsewhere/index"),
        ("GIT_TERMINAL_PROMPT", "1"),
        ("GIT_OPTIONAL_LOCKS", "1"),
    ] {
        base.vars.insert(n.into(), v.into());
    }
    let get = |env: &[(std::ffi::OsString, std::ffi::OsString)], name: &str| {
        let found: Vec<_> = env.iter().filter(|(n, _)| n == name).collect();
        assert!(found.len() <= 1, "{name} twice: {found:?}");
        found.first().map(|(_, v)| v.to_str().unwrap().to_owned())
    };
    let read = child_env(&base, Access::Read, None);
    assert_eq!(get(&read, "PATH").as_deref(), Some("/bin"));
    assert_eq!(get(&read, "HOME").as_deref(), Some("/home/me"));
    assert_eq!(get(&read, "GIT_TERMINAL_PROMPT").as_deref(), Some("0"));
    assert_eq!(get(&read, "GIT_EDITOR").as_deref(), Some(":"));
    assert_eq!(get(&read, "GIT_SEQUENCE_EDITOR").as_deref(), Some(":"));
    assert_eq!(get(&read, "GIT_MERGE_AUTOEDIT").as_deref(), Some("no"));
    assert_eq!(get(&read, "GIT_OPTIONAL_LOCKS").as_deref(), Some("0"));
    for gone in ["GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE", "GIT_ASKPASS"] {
        assert_eq!(get(&read, gone), None, "{gone}");
    }
    let write = child_env(&base, Access::Write, Some(Path::new("/app/askpass")));
    assert_eq!(get(&write, "GIT_OPTIONAL_LOCKS"), None);
    assert_eq!(get(&write, "GIT_TERMINAL_PROMPT").as_deref(), Some("0"));
    assert_eq!(get(&write, "GIT_ASKPASS").as_deref(), Some("/app/askpass"));
    assert_eq!(get(&write, "SSH_ASKPASS").as_deref(), Some("/app/askpass"));
    assert_eq!(get(&write, "SSH_ASKPASS_REQUIRE").as_deref(), Some("force"));
}

/// ② 버전이 2.40 미만이면 쓰기를 거절하고 알림을 한 번만 낸다. 읽기는 된다.
#[test]
fn an_old_git_refuses_writes_and_notifies_once() {
    let c = Cli::new("");
    FakeGit::new(&c.bin, "git", "2.39.0");
    let mut sub = c.f.bus.subscribe(Filter::all().topics([TOPIC_SHOW]));
    for _ in 0..3 {
        assert_eq!(
            c.command(Access::Write).unwrap_err(),
            CliError::TooOld {
                program: c.bin.join("git"),
                version: "2.39.0".into(),
            }
        );
    }
    assert_eq!(
        notifications(&mut sub),
        vec![serde_json::json!({
            "level": "warning",
            "message": TOO_OLD_MESSAGE,
            "args": { "version": "2.39.0", "required": "2.40" },
            "body": c.bin.join("git").display().to_string(),
        })]
    );
    assert!(c.command(Access::Read).is_ok());
}

/// 2.40 이면 쓰기가 되고 알림이 없다(위 시험이 버전 판정으로 갈리는지).
#[test]
fn git_2_40_can_write() {
    let c = Cli::new("");
    FakeGit::new(&c.bin, "git", "2.40.0");
    let mut sub = c.f.bus.subscribe(Filter::all().topics([TOPIC_SHOW]));
    assert!(c.command(Access::Write).is_ok());
    assert_eq!(notifications(&mut sub), Vec::<serde_json::Value>::new());
}

/// 버전을 읽지 못하는 실행 파일도 쓰기를 끈다.
#[test]
fn an_unrecognized_version_refuses_writes() {
    let c = Cli::new("");
    FakeGit::new(&c.bin, "git", "x.y");
    let err = c.command(Access::Write).unwrap_err();
    assert!(
        matches!(&err, CliError::TooOld { version, .. } if version.contains("unrecognized")),
        "{err:?}"
    );
}

/// git 을 올리면(실행 파일이 바뀌면) 다시 읽는다.
#[test]
fn a_replaced_git_is_read_again() {
    let c = Cli::new("");
    FakeGit::new(&c.bin, "git", "2.39.0");
    assert!(c.command(Access::Write).is_err());
    // 크기가 다른 스크립트로 바꾼다(수정 시각이 같은 초여도 갈린다).
    FakeGit::new(&c.bin, "git", "2.41.10");
    assert!(c.command(Access::Write).is_ok());
}

/// ③ 자식 환경: 런타임 환경 + 프롬프트 끔 · 편집기 끔, 읽기에는 `GIT_OPTIONAL_LOCKS=0`. 미신뢰 프로젝트에서는 신뢰가 필요한 제공자
/// (mise 자리)가 정한 변수가 없다.
#[test]
fn the_child_gets_the_runtime_env_with_git_overrides_and_no_provider_vars_until_trusted() {
    let c = Cli::new("");
    let fake = FakeGit::new(&c.bin, "git", "2.54.0");
    c.run(Access::Read, &["status", "--porcelain=v2", "-z"]);
    assert_eq!(
        fake.received_args().unwrap(),
        ["status", "--porcelain=v2", "-z"]
    );
    assert_eq!(fake.env_var("GIT_TERMINAL_PROMPT").as_deref(), Some("0"));
    assert_eq!(fake.env_var("GIT_EDITOR").as_deref(), Some(":"));
    assert_eq!(fake.env_var("GIT_OPTIONAL_LOCKS").as_deref(), Some("0"));
    assert_eq!(fake.env_var("PW_FAKE_MISE"), None);
    // 로그인 셸이 정한 `GIT_DIR` 은 지웠다.
    assert_eq!(fake.env_var("GIT_DIR"), None);
    // PATH 는 로그인 셸의 것, cwd 는 작업 트리.
    assert!(
        fake.env_var("PATH")
            .unwrap()
            .starts_with(&c.bin.display().to_string())
    );
    assert_eq!(
        fake.record("cwd").unwrap().trim(),
        fs::canonicalize(&c.target.work_tree)
            .unwrap()
            .display()
            .to_string()
    );

    c.run(Access::Write, &["commit"]);
    assert_eq!(fake.env_var("GIT_OPTIONAL_LOCKS"), None);
    assert_eq!(fake.env_var("GIT_TERMINAL_PROMPT").as_deref(), Some("0"));
    assert_eq!(fake.env_var("PW_FAKE_MISE"), None);

    // 신뢰하면 제공자의 변수가 온다(위의 「없다」 가 신뢰로 갈리는지).
    c.f.trusted.store(true, Ordering::SeqCst);
    c.run(Access::Write, &["commit"]);
    assert_eq!(fake.env_var("PW_FAKE_MISE").as_deref(), Some("1"));
}

/// ④ 취소하면 git 과 그 자식(같은 프로세스 그룹)이 1초 안에 사라진다.
#[test]
fn cancelling_ends_the_process_group_within_a_second() {
    let c = Cli::new("");
    let fake = FakeGit::new(&c.bin, "git", "2.54.0");
    let rt = tokio_rt();
    let command = rt
        .block_on(c.f.git.command(&c.target, Access::Write))
        .unwrap();
    let cancel = CancellationToken::new();
    let token = cancel.clone();
    let running = rt.spawn(async move { command.arg("sleep").output(&token).await });
    let deadline = Instant::now() + Duration::from_secs(10);
    let (git, child) = loop {
        if let (Some(g), Some(c)) = (fake.record("git.pid"), fake.record("child.pid")) {
            break (
                g.trim().parse::<libc::pid_t>().unwrap(),
                c.trim().parse::<libc::pid_t>().unwrap(),
            );
        }
        assert!(Instant::now() < deadline, "the fake git did not start");
        std::thread::sleep(Duration::from_millis(10));
    };
    let alive = |pid: libc::pid_t| {
        // SAFETY: 신호 0 은 프로세스가 있는지만 본다.
        unsafe { libc::kill(pid, 0) == 0 }
    };
    assert!(alive(git) && alive(child));
    let cancelled = Instant::now();
    cancel.cancel();
    while alive(git) || alive(child) {
        assert!(
            cancelled.elapsed() < Duration::from_secs(1),
            "git {git} alive: {}, child {child} alive: {}",
            alive(git),
            alive(child)
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        rt.block_on(running).unwrap().unwrap_err(),
        RunError::Cancelled
    );
    // sleep 은 SIGTERM 에서 끝난다 — SIGKILL 까지 기다리지 않았다.
    assert!(cancelled.elapsed() < TERM_GRACE + Duration::from_millis(500));
}

/// `[git] path` 가 PATH 의 git 을 덮는다. 이름이면 PATH 에서, 못 찾으면 그 이름으로 「없음」.
#[test]
fn the_git_path_setting_overrides_the_path_lookup() {
    let elsewhere = tempfile::tempdir().unwrap();
    let custom = FakeGit::new(elsewhere.path(), "my-git", "2.50.0");
    let c = Cli::new(&format!("[git]\npath = \"{}\"\n", custom.path().display()));
    FakeGit::new(&c.bin, "git", "2.39.0");
    let command = c.command(Access::Write).unwrap();
    assert_eq!(command.program(), custom.path());

    let c = Cli::new("[git]\npath = \"no-such-git\"\n");
    FakeGit::new(&c.bin, "git", "2.54.0");
    assert!(matches!(
        c.command(Access::Read).unwrap_err(),
        CliError::NotFound { program, login_shell_path: true } if program == "no-such-git"
    ));
}

/// 읽는 명령은 index 를 새로 쓰지 않는다(`GIT_OPTIONAL_LOCKS=0`). 진짜 git 으로, stat 이 바뀐 파일이 있어 `git status` 가 index 를
/// 갱신하고 싶어 하는 저장소에서 본다. 같은 명령을 쓰기로 돌리면 갱신한다 — 이 시험이 그 변수로 갈리는지.
#[test]
fn read_commands_leave_the_index_alone() {
    let r = TestRepo::with_commit();
    let git = which_git();
    let c = Cli::with_path("", &git.parent().unwrap().display().to_string());
    let target = Target {
        project_id: c.target.project_id.clone(),
        project_root: r.root().to_path_buf(),
        work_tree: r.root().to_path_buf(),
    };
    let index = r.root().join(".git/index");
    let touch = || {
        let later = std::time::SystemTime::now() + Duration::from_secs(5);
        fs::File::options()
            .write(true)
            .open(r.root().join("README.md"))
            .unwrap()
            .set_modified(later)
            .unwrap();
    };
    touch();
    let before = fs::read(&index).unwrap();
    let rt = tokio_rt();
    let status = |access| {
        rt.block_on(async {
            let command = c.f.git.command(&target, access).await.unwrap();
            assert_eq!(command.program(), git);
            command
                .args(["status", "--porcelain=v2", "-z"])
                .output(&CancellationToken::new())
                .await
                .unwrap()
        })
    };
    let out = status(Access::Read);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    // 내용이 같은 파일은 바뀐 항목이 아니다.
    assert_eq!(out.records().count(), 0, "{:?}", out.stdout);
    assert_eq!(
        fs::read(&index).unwrap(),
        before,
        "a read rewrote the index"
    );
    assert!(!r.root().join(".git/index.lock").exists());

    let out = status(Access::Write);
    assert!(out.status.success());
    assert_ne!(
        fs::read(&index).unwrap(),
        before,
        "git did not refresh the index; the read check above proves nothing"
    );
}

fn which_git() -> PathBuf {
    let out = std::process::Command::new("sh")
        .args(["-c", "command -v git"])
        .output()
        .unwrap();
    let path = PathBuf::from(String::from_utf8(out.stdout).unwrap().trim());
    assert!(path.is_absolute(), "git is not on PATH");
    fs::canonicalize(path).unwrap()
}

/// 쓰기를 끈 알림의 메시지 id 가 내장 번역의 모든 언어에 있다(창은 없는 id 를 그대로 보인다).
#[test]
fn the_too_old_message_exists_in_every_bundled_language() {
    let i18n = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../extensions/i18n-default/i18n");
    let mut languages = 0;
    for entry in fs::read_dir(&i18n).unwrap() {
        let file = entry.unwrap().path().join("project.ftl");
        let source = fs::read_to_string(&file).unwrap();
        assert!(
            source
                .lines()
                .any(|l| l.starts_with(&format!("{TOO_OLD_MESSAGE} ="))),
            "{} has no `{TOO_OLD_MESSAGE}`",
            file.display()
        );
        languages += 1;
    }
    assert!(languages >= 2, "{languages}");
}
