//! 실제로 띄운 가짜 도구(받은 argv · cwd · PATH 를 파일에 적는 스크립트)로 실행 전체를 본다. 사람의 실제 도구는 띄우지 않는다.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use pw_core::bus::{DEFAULT_CAPACITY, EventBus, Scope};
use pw_core::config::{AppPaths, ConfigSource, ConfigStore};
use pw_core::registry::{NewProject, Registry as Projects};
use semver::Version;

use super::*;

struct Fixture {
    tmp: tempfile::TempDir,
    open_with: OpenWith,
    projects: Projects,
    /// 가짜 도구가 있는 폴더(고정 PATH 의 첫 항목).
    bin: PathBuf,
}

impl Fixture {
    /// `settings` 는 전역 `settings.toml` 내용.
    fn new(settings: &str) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::under(&tmp.path().join("app"));
        fs::create_dir_all(&paths.config).unwrap();
        fs::write(paths.config.join("settings.toml"), settings).unwrap();
        let bus = EventBus::new(DEFAULT_CAPACITY);
        let global = ConfigStore::open(ConfigSource::global(&paths), bus.clone(), Scope::Host);
        assert_eq!(global.fault(), None);
        let projects = Projects::new(paths.clone(), None, bus);
        let bin = tmp.path().join("fake bin");
        fs::create_dir_all(&bin).unwrap();
        let extensions = Arc::new(Registry::from_roots(
            &Version::new(0, 0, 0),
            pw_ext::builtin::roots(),
        ));
        let path = std::env::join_paths([
            bin.clone(),
            PathBuf::from("/usr/bin"),
            PathBuf::from("/bin"),
        ])
        .unwrap();
        let open_with = OpenWith::new(extensions)
            .with_path(fixed_path(path))
            .with_early_exit(Duration::from_millis(800));
        open_with.attach(HostParts {
            paths,
            global,
            projects: projects.clone(),
        });
        Self {
            tmp,
            open_with,
            projects,
            bin,
        }
    }

    /// 받은 인자를 NUL 로 나눠 `<이름>.argv` 에, cwd 와 PATH 를 `<이름>.env` 에 적는 가짜 도구.
    fn fake_tool(&self, name: &str) -> PathBuf {
        let path = self.bin.join(name);
        let out = self.tmp.path().join(name);
        fs::write(
            &path,
            format!(
                "#!/bin/sh\nfor a in \"$@\"; do printf '%s\\0' \"$a\"; done > '{0}.argv.tmp'\nprintf '%s\\0%s' \"$(pwd -P)\" \"$PATH\" > '{0}.env'\nmv '{0}.argv.tmp' '{0}.argv'\n",
                out.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    /// 가짜 도구가 받은 argv(실행 파일 이름 제외). 도구는 떼어 띄워지므로 파일이 생길 때까지 기다린다.
    fn received(&self, name: &str) -> Vec<String> {
        let file = self.tmp.path().join(format!("{name}.argv"));
        let deadline = Instant::now() + Duration::from_secs(5);
        while !file.exists() {
            assert!(Instant::now() < deadline, "{name} was not run");
            std::thread::sleep(Duration::from_millis(20));
        }
        let raw = fs::read(&file).unwrap();
        raw.split(|&b| b == 0)
            .filter(|s| !s.is_empty())
            .map(|s| String::from_utf8(s.to_vec()).unwrap())
            .collect()
    }

    fn env(&self, name: &str) -> (String, String) {
        let text = fs::read_to_string(self.tmp.path().join(format!("{name}.env"))).unwrap();
        let (cwd, path) = text.split_once('\0').unwrap();
        (cwd.to_owned(), path.to_owned())
    }

    fn project(&self, root: &Path) -> Project {
        fs::create_dir_all(root).unwrap();
        self.projects
            .create(NewProject {
                root: root.to_path_buf(),
                ..NewProject::default()
            })
            .unwrap()
    }

    fn run(&self, params: OpenWithRunParams) -> OpenWithOutcome {
        self.open_with.run(&params).unwrap()
    }
}

fn settings_with_tool(tool: &str, command: &str, file_command: &str) -> String {
    format!(
        "schema = 1\n\n[open_with.tools.{tool}]\nlabel = \"Fake\"\ncommand = {command}\nfile_command = {file_command}\n"
    )
}

/// 경로 · 파일 이름 · 브랜치 이름에 공백, 따옴표, `$(…)`, `;`, 개행, 앞의 `-` 가 있어도 argv 가 한 칸씩 그대로 넘어간다.
#[test]
fn names_with_shell_syntax_reach_the_tool_as_exact_arguments() {
    let f = Fixture::new(&settings_with_tool(
        "fake",
        r#"["fake-tool", "{project_root}", "--branch={branch}"]"#,
        r#"["fake-tool", "{project_root}", "{file}:{line}:{column}", "{branch}"]"#,
    ));
    f.fake_tool("fake-tool");
    let pwned = f.tmp.path().join("pwned");
    let root = f.tmp.path().join(format!(
        "proj a\"b'c $(touch {0}); touch {0}\n-rf `touch {0}`",
        pwned.display()
    ));
    let project = f.project(&root);
    let root = PathBuf::from(&project.roots[0]);
    fs::create_dir_all(root.join(".git")).unwrap();
    let branch = format!("-x; touch {0} $(touch {0}) 'q' \"d\"", pwned.display());
    fs::write(
        root.join(".git/HEAD"),
        format!("ref: refs/heads/{branch}\n"),
    )
    .unwrap();
    let file = root.join("-n weird; $(rm -rf x)\n'.rs");

    let outcome = f.run(OpenWithRunParams {
        project_id: project.id.clone(),
        tool: Some("fake".to_owned()),
        file: Some(file.display().to_string()),
        line: Some(12),
        column: Some(3),
        ..OpenWithRunParams::default()
    });
    assert!(
        matches!(outcome, OpenWithOutcome::Launched { .. }),
        "{outcome:?}"
    );
    assert_eq!(
        f.received("fake-tool"),
        [
            root.display().to_string(),
            format!("{}:12:3", file.display()),
            branch.clone()
        ]
    );
    let (cwd, path) = f.env("fake-tool");
    assert_eq!(PathBuf::from(cwd), root);
    // 자식은 찾을 때 쓴 PATH 를 받는다.
    assert!(path.starts_with(&f.bin.display().to_string()), "{path}");
    assert!(!pwned.exists(), "a name was run as a command");

    // 파일이 없으면 프로젝트만 여는 command.
    fs::remove_file(f.tmp.path().join("fake-tool.argv")).unwrap();
    f.run(OpenWithRunParams {
        project_id: project.id.clone(),
        tool: Some("fake".to_owned()),
        ..OpenWithRunParams::default()
    });
    assert_eq!(
        f.received("fake-tool"),
        [root.display().to_string(), format!("--branch={branch}")]
    );
}

/// 활성 워크트리로 열었으면 `{project_root}` 와 작업 디렉터리는 그 워크트리 안의 프로젝트 루트다.
#[test]
fn the_project_root_follows_the_active_worktree() {
    let f = Fixture::new(&settings_with_tool(
        "fake",
        r#"["fake-tool", "{project_root}"]"#,
        r#"["fake-tool", "{file}"]"#,
    ));
    f.fake_tool("fake-tool");
    let main = f.tmp.path().join("repo");
    fs::create_dir_all(main.join(".git/worktrees/wt")).unwrap();
    let project = f.project(&main.join("app"));
    let wt = f.tmp.path().join("wt");
    fs::create_dir_all(wt.join("app")).unwrap();
    fs::write(
        wt.join(".git"),
        format!("gitdir: {}\n", main.join(".git/worktrees/wt").display()),
    )
    .unwrap();
    fs::write(main.join(".git/worktrees/wt/commondir"), "../..\n").unwrap();
    let wt = fs::canonicalize(&wt).unwrap();

    let outcome = f.run(OpenWithRunParams {
        project_id: project.id.clone(),
        tool: Some("fake".to_owned()),
        worktree: Some(wt.display().to_string()),
        ..OpenWithRunParams::default()
    });
    assert!(
        matches!(outcome, OpenWithOutcome::Launched { .. }),
        "{outcome:?}"
    );
    assert_eq!(
        f.received("fake-tool"),
        [wt.join("app").display().to_string()]
    );
    assert_eq!(PathBuf::from(f.env("fake-tool").0), wt.join("app"));

    // 다른 프로젝트의 워크트리는 받지 않는다.
    let other = f.project(&f.tmp.path().join("other"));
    let err = f
        .open_with
        .run(&OpenWithRunParams {
            project_id: other.id,
            tool: Some("fake".to_owned()),
            worktree: Some(wt.display().to_string()),
            ..OpenWithRunParams::default()
        })
        .unwrap_err();
    assert_eq!(err.code, code::INVALID_PARAMS, "{err:?}");
}

/// 커밋되는 프로젝트 설정은 기본 도구를 고르기만 한다 — 도구를 정의하거나 명령을 바꾸지 못한다.
#[test]
fn shared_project_config_only_picks_the_default() {
    let f = Fixture::new(&settings_with_tool(
        "fake",
        r#"["fake-tool", "{project_root}"]"#,
        r#"["fake-tool", "{file}"]"#,
    ));
    f.fake_tool("fake-tool");
    let evil = f.fake_tool("evil-tool");
    let root = f.tmp.path().join("cloned");
    let project = f.project(&root);
    fs::create_dir_all(root.join(".pitwall")).unwrap();
    fs::write(
        root.join(".pitwall/project.toml"),
        format!(
            "# no schema: an old file the app would migrate\n[open_with]\ndefault = \"fake\"\n\n[open_with.tools.fake]\ncommand = [\"{0}\", \"{{project_root}}\"]\n\n[open_with.tools.evil]\nlabel = \"Evil\"\ncommand = [\"{0}\"]\n",
            evil.display()
        ),
    )
    .unwrap();
    let before = fs::read(root.join(".pitwall/project.toml")).unwrap();

    let list = f
        .open_with
        .list(&OpenWithListParams {
            project_id: Some(project.id.clone()),
            worktree: None,
        })
        .unwrap();
    assert_eq!(list.default, DefaultToolInfo::Tool { id: "fake".into() });
    assert!(
        list.tools.iter().all(|t| t.id != "evil"),
        "{:?}",
        list.tools
    );

    let outcome = f.run(OpenWithRunParams {
        project_id: project.id.clone(),
        ..OpenWithRunParams::default()
    });
    assert!(
        matches!(&outcome, OpenWithOutcome::Launched { tool, .. } if tool == "fake"),
        "{outcome:?}"
    );
    assert_eq!(
        f.received("fake-tool"),
        [PathBuf::from(&project.roots[0]).display().to_string()]
    );
    let outcome = f.run(OpenWithRunParams {
        project_id: project.id.clone(),
        tool: Some("evil".to_owned()),
        ..OpenWithRunParams::default()
    });
    assert!(
        matches!(
            &outcome,
            OpenWithOutcome::Failed {
                problem: LaunchProblem::UnknownTool,
                ..
            }
        ),
        "{outcome:?}"
    );
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        !f.tmp.path().join("evil-tool.argv").exists(),
        "the repository's command ran"
    );
    // 읽기만 했다 — 마이그레이션 · 백업을 쓰지 않는다.
    assert_eq!(
        fs::read(root.join(".pitwall/project.toml")).unwrap(),
        before
    );
    assert!(!root.join(".pitwall/.backup").exists());
}

#[test]
fn the_global_default_applies_and_no_default_asks_for_a_choice() {
    let f = Fixture::new(&format!(
        "{}\n[open_with]\ndefault = \"fake\"\n",
        settings_with_tool(
            "fake",
            r#"["fake-tool", "{project_root}"]"#,
            r#"["fake-tool", "{file}"]"#
        )
    ));
    f.fake_tool("fake-tool");
    let project = f.project(&f.tmp.path().join("p"));
    let outcome = f.run(OpenWithRunParams {
        project_id: project.id.clone(),
        ..OpenWithRunParams::default()
    });
    assert!(
        matches!(&outcome, OpenWithOutcome::Launched { tool, .. } if tool == "fake"),
        "{outcome:?}"
    );

    let f = Fixture::new("schema = 1\n");
    let project = f.project(&f.tmp.path().join("p"));
    assert_eq!(
        f.run(OpenWithRunParams {
            project_id: project.id.clone(),
            ..OpenWithRunParams::default()
        }),
        OpenWithOutcome::NoDefault { configured: None }
    );
    let f = Fixture::new("schema = 1\n[open_with]\ndefault = \"no-such-tool\"\n");
    let project = f.project(&f.tmp.path().join("p"));
    assert_eq!(
        f.run(OpenWithRunParams {
            project_id: project.id,
            ..OpenWithRunParams::default()
        }),
        OpenWithOutcome::NoDefault {
            configured: Some("no-such-tool".into())
        }
    );
}

/// 실행하지 못하는 경우는 조용히 지나가지 않고 이유를 돌려준다.
#[test]
fn failures_come_back_with_their_reason() {
    let f = Fixture::new(
        "schema = 1\n\n[open_with.tools.missing]\nlabel = \"Missing\"\ncommand = [\"no-such-program-xyz\", \"{project_root}\"]\n\n\
         [open_with.tools.branchy]\nlabel = \"Branchy\"\ncommand = [\"fake-tool\", \"{branch}\"]\n\n\
         [open_with.tools.failing]\nlabel = \"Failing\"\ncommand = [\"failing-tool\"]\n\n\
         [open_with.tools.noexec]\nlabel = \"No exec\"\ncommand = [\"/etc/hosts\"]\n\n\
         [open_with.tools.zed]\ncommand = \"zed {project_root}\"\n",
    );
    f.fake_tool("fake-tool");
    let failing = f.bin.join("failing-tool");
    fs::write(&failing, "#!/bin/sh\nexit 3\n").unwrap();
    fs::set_permissions(&failing, fs::Permissions::from_mode(0o755)).unwrap();
    let project = f.project(&f.tmp.path().join("not a repo"));
    let problem = |tool: &str| match f.run(OpenWithRunParams {
        project_id: project.id.clone(),
        tool: Some(tool.to_owned()),
        ..OpenWithRunParams::default()
    }) {
        OpenWithOutcome::Failed { problem, .. } => problem,
        other => panic!("{tool}: {other:?}"),
    };
    assert_eq!(
        problem("missing"),
        LaunchProblem::NotFound {
            program: "no-such-program-xyz".into(),
            login_shell_path: true
        }
    );
    assert_eq!(
        problem("branchy"),
        LaunchProblem::MissingVariable {
            variable: "branch".into()
        }
    );
    assert_eq!(problem("failing"), LaunchProblem::Exited { code: Some(3) });
    assert!(matches!(problem("noexec"), LaunchProblem::NotFound { .. }));
    assert!(
        matches!(problem("zed"), LaunchProblem::InvalidTool { message } if message.contains("shell command strings")),
    );
    assert_eq!(problem("nope"), LaunchProblem::UnknownTool);

    fs::remove_dir(&project.roots[0]).unwrap();
    assert!(matches!(
        problem("branchy"),
        LaunchProblem::ProjectRootMissing { .. } | LaunchProblem::MissingVariable { .. }
    ));
    assert!(matches!(
        problem("failing"),
        LaunchProblem::ProjectRootMissing { .. }
    ));
}

#[test]
fn rejects_a_relative_file_and_an_unknown_project() {
    let f = Fixture::new("schema = 1\n");
    let project = f.project(&f.tmp.path().join("p"));
    let err = f
        .open_with
        .run(&OpenWithRunParams {
            project_id: project.id,
            tool: Some("zed".into()),
            file: Some("src/main.rs".into()),
            ..OpenWithRunParams::default()
        })
        .unwrap_err();
    assert_eq!(err.code, code::INVALID_PARAMS);
    let err = f
        .open_with
        .run(&OpenWithRunParams {
            project_id: "0000000000000000".into(),
            tool: Some("zed".into()),
            ..OpenWithRunParams::default()
        })
        .unwrap_err();
    assert_eq!(err.code, code::INVALID_PARAMS);
}

#[test]
fn lists_this_platforms_tools_with_settings_problems() {
    let f = Fixture::new(
        "schema = 1\n\n[open_with]\ndefault = \"zed\"\n\n[open_with.tools.idea]\ncommand = []\n",
    );
    let list = f.open_with.list(&OpenWithListParams::default()).unwrap();
    let ids: Vec<&str> = list.tools.iter().map(|t| t.id.as_str()).collect();
    let os_tools: &[&str] = if cfg!(target_os = "macos") {
        &["finder", "terminal"]
    } else {
        &["file-manager"]
    };
    assert_eq!(
        ids,
        [&["orca", "zed", "idea", "vscode", "cursor"][..], os_tools].concat()
    );
    assert_eq!(list.default, DefaultToolInfo::Tool { id: "zed".into() });
    let idea = &list.tools[2];
    assert!(
        idea.problem
            .as_deref()
            .is_some_and(|p| p.contains("is empty")),
        "{idea:?}"
    );
    assert!(list.tools[1].opens_files);
    let terminal = list.tools.last().unwrap();
    assert!(!terminal.opens_files, "{terminal:?} has no file_command");
}
