//! 실행 구성(17-m6-plan M6-01)의 완료 기준 ① ~ ⑦ 과 정규형 · 키 이름 표.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use pw_core::bus::{DEFAULT_CAPACITY, EventBus};
use pw_core::config::{AppPaths, LayerOrigin};
use pw_core::registry::{ConfigMode, NewProject, Project, Registry};
use pw_rpc::{Caller, ConnectionScope, EventHub, Id, Outbound, Role, Router, Session, code};
use serde_json::{Value, json};

use super::argv::{ArgvContext, argv};
use super::methods::{HostParts, RunConfigs, register};
use super::read::{from_file_value, read};
use super::*;

struct Fixture {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    paths: AppPaths,
    root: PathBuf,
    project: Project,
}

impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        // 시험의 비교가 실제 경로(`/private/var` …)로 하도록 처음부터 실제 경로로 둔다.
        let base = fs::canonicalize(tmp.path()).unwrap();
        let root = base.join("proj");
        fs::create_dir_all(&root).unwrap();
        let paths = AppPaths::under(&base.join("app"));
        let project = Project {
            id: "0123456789abcdef".into(),
            name: "proj".into(),
            host: "local".into(),
            roots: vec![root.display().to_string()],
            favorite: false,
            tags: Vec::new(),
            config_mode: ConfigMode::Repository,
            slug: "proj".into(),
            created_at: 0,
            opened_at: None,
        };
        Self {
            _tmp: tmp,
            base,
            paths,
            root,
            project,
        }
    }

    fn place(&self) -> RunConfigPlace {
        RunConfigPlace::new(&self.paths, &self.project, None).unwrap()
    }

    /// `<root>/.pitwall/run/<name>` 에 쓴다.
    fn run_file(&self, name: &str, text: &str) -> PathBuf {
        let path = self.root.join(".pitwall/run").join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, text).unwrap();
        path
    }

    fn dir(&self, rel: &str) -> PathBuf {
        let dir = self.root.join(rel);
        fs::create_dir_all(&dir).unwrap();
        dir
    }
}

/// TOML 본문(`schema` 줄 없이)을 구성으로.
fn parse(body: &str) -> RunConfig {
    let doc: toml_edit::DocumentMut = body.parse().unwrap();
    from_file_value(pw_core::config::merge::to_json(doc.as_table())).unwrap()
}

fn shell() -> Vec<String> {
    vec!["/bin/zsh".to_owned(), "-l".to_owned()]
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_owned()).collect()
}

fn python() -> &'static str {
    if cfg!(windows) { "python" } else { "python3" }
}

// --- ① 종류마다 argv ------------------------------------------------------------------------------------------

/// 종류마다 TOML 픽스처 → argv. 공백 · 따옴표 · `$` 가 든 값이 한 인자로 남는다 — 필드를 셸 글로 이어 붙이면 칸 수 · 값이 달라
/// 실패한다.
#[test]
fn every_kind_builds_its_argv_field_by_field() {
    let f = Fixture::new();
    fs::write(f.root.join("pnpm-lock.yaml"), "").unwrap();
    let wrapper_name = if cfg!(windows) {
        "gradlew.bat"
    } else {
        "gradlew"
    };
    fs::write(f.root.join(wrapper_name), "").unwrap();
    let gradlew = f.root.join(wrapper_name).display().to_string();
    let backend = f.dir("backend");
    let web = f.dir("packages/web");

    let cases: Vec<(&str, Option<&str>, &Path, Vec<String>)> = vec![
        (
            "name = 's'\ntype = 'shell'\ncommand = 'echo \"a b\" $HOME | wc -c'",
            None,
            &f.root,
            strings(&[
                "/bin/zsh",
                "-l",
                if cfg!(windows) { "-Command" } else { "-c" },
                "echo \"a b\" $HOME | wc -c",
            ]),
        ),
        (
            "name = 'n'\ntype = 'npm'\nscript = 'dev:web'",
            None,
            &web,
            strings(&["pnpm", "run", "dev:web"]),
        ),
        (
            "name = 'g'\ntype = 'gradle'\ntasks = ['bootRun', '--args=--port 8081 \"x\"']",
            None,
            &backend,
            vec![
                gradlew.clone(),
                "bootRun".into(),
                "--args=--port 8081 \"x\"".into(),
            ],
        ),
        (
            "name = 'm'\ntype = 'maven'\ntasks = ['spring-boot:run', \"-Dname=$USER it's\"]",
            None,
            &f.root,
            strings(&["mvn", "spring-boot:run", "-Dname=$USER it's"]),
        ),
        (
            "name = 'c'\ntype = 'cargo'\ncommand = 'run'\nargs = ['--', '--name', 'a b', \"it's\", '$(rm -rf /)']",
            None,
            &f.root,
            strings(&["cargo", "run", "--", "--name", "a b", "it's", "$(rm -rf /)"]),
        ),
        (
            "name = 'c'\ntype = 'cargo'\ncommand = 'run'\nargs = ['--', 'dev']\n[profiles.release]\nargs = ['--release', '--', 'prod env']",
            Some("release"),
            &f.root,
            strings(&["cargo", "run", "--release", "--", "prod env"]),
        ),
        (
            "name = 'go'\ntype = 'go'\ncommand = 'run'\npackage = './cmd/api'\nargs = ['-flag=\"q r\"']",
            None,
            &f.root,
            strings(&["go", "run", "./cmd/api", "-flag=\"q r\""]),
        ),
        (
            "name = 'p'\ntype = 'python'\nmodule = 'http.server'\nargs = ['8000']",
            None,
            &f.root,
            vec![
                python().into(),
                "-m".into(),
                "http.server".into(),
                "8000".into(),
            ],
        ),
        (
            "name = 'p'\ntype = 'python'\nscript = 'tools/run me.py'\nargs = ['a b']",
            None,
            &f.root,
            vec![python().into(), "tools/run me.py".into(), "a b".into()],
        ),
        (
            "name = 'd'\ntype = 'docker-compose'\nfile = 'deploy/compose.yaml'\nservices = ['web', 'db']",
            None,
            &backend,
            vec![
                "docker".into(),
                "compose".into(),
                "-f".into(),
                f.root.join("deploy/compose.yaml").display().to_string(),
                "up".into(),
                "web".into(),
                "db".into(),
            ],
        ),
        (
            "name = 'k'\ntype = 'mise-task'\ntask = 'build:all'",
            None,
            &f.root,
            strings(&["mise", "run", "build:all"]),
        ),
    ];
    let login = shell();
    for (body, profile, cwd, expected) in cases {
        let config = parse(body);
        validate(&config, &f.root).unwrap_or_else(|e| panic!("{body}: {e}"));
        let cx = ArgvContext {
            root: &f.root,
            cwd,
            login_shell: &login,
        };
        assert_eq!(argv(&config, profile, &cx).unwrap(), expected, "{body}");
    }
}

#[test]
fn package_manager_and_wrapper_follow_the_files() {
    let f = Fixture::new();
    let login = shell();
    let cx = ArgvContext {
        root: &f.root,
        cwd: &f.root,
        login_shell: &login,
    };
    let npm = parse("name = 'n'\ntype = 'npm'\nscript = 'dev'");
    assert_eq!(argv(&npm, None, &cx).unwrap()[0], "npm", "no lockfile");
    for (lockfile, pm) in [
        ("yarn.lock", "yarn"),
        ("bun.lock", "bun"),
        ("package-lock.json", "npm"),
    ] {
        let dir = f.dir(lockfile);
        fs::write(dir.join(lockfile), "").unwrap();
        let cx = ArgvContext { cwd: &dir, ..cx };
        assert_eq!(argv(&npm, None, &cx).unwrap()[0], pm, "{lockfile}");
    }
    let gradle = parse("name = 'g'\ntype = 'gradle'\ntasks = ['build']");
    assert_eq!(argv(&gradle, None, &cx).unwrap()[0], "gradle", "no wrapper");
    let missing = argv(&npm, Some("nope"), &cx).unwrap_err();
    assert!(missing.contains("nope"), "{missing}");
}

// --- ② 쓸 수 없음 ------------------------------------------------------------------------------------------------

/// 모르는 `type` · TOML 형식 오류 · 빠진 필드 · 새 `schema` · 틀린 id · 대상 없는 링크는 목록에서 빠지지 않고 「쓸 수 없음(까닭)」
/// 이다 — 건너뛰면 실패.
#[test]
fn broken_configs_stay_in_the_list_as_unusable() {
    let f = Fixture::new();
    f.run_file(
        "good.toml",
        "schema = 1\nname = 'Good'\ntype = 'shell'\ncommand = 'true'\n",
    );
    f.run_file(
        "future.toml",
        "schema = 1\nname = 'Future'\ntype = 'teleport'\n",
    );
    f.run_file(
        "broken.toml",
        "schema = 1\nname = 'Broken\ntype = 'shell'\n",
    );
    f.run_file(
        "missing.toml",
        "schema = 1\nname = 'Missing'\ntype = 'cargo'\n",
    );
    f.run_file(
        "newer.toml",
        "schema = 99\nname = 'Newer'\ntype = 'shell'\ncommand = 'true'\n",
    );
    f.run_file(
        "bad.id.toml",
        "schema = 1\nname = 'Dotted'\ntype = 'shell'\ncommand = 'true'\n",
    );
    f.run_file(
        "rpc-key.toml",
        "schema = 1\nname = 'K'\ntype = 'shell'\ncommand = 'x'\nenvFile = ['.env']\n",
    );
    #[cfg(unix)]
    std::os::unix::fs::symlink(
        f.base.join("nowhere.toml"),
        f.root.join(".pitwall/run/dangling.toml"),
    )
    .unwrap();

    let set = read(&f.place());
    let ids: Vec<&str> = set.configs.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(ids, ["good"]);
    let reason = |id: &str| {
        set.unusable
            .iter()
            .find(|u| u.id == id)
            .unwrap_or_else(|| panic!("{id} is missing from the unusable list: {:?}", set.unusable))
            .reason
            .clone()
    };
    assert!(
        reason("future").contains("teleport"),
        "{}",
        reason("future")
    );
    assert!(
        reason("broken").contains("broken.toml"),
        "{}",
        reason("broken")
    );
    assert!(
        reason("missing").contains("command"),
        "{}",
        reason("missing")
    );
    assert!(
        reason("newer").contains("schema = 99"),
        "{}",
        reason("newer")
    );
    assert!(reason("bad.id").contains("id"), "{}", reason("bad.id"));
    assert!(
        reason("rpc-key").contains("env_file"),
        "{}",
        reason("rpc-key")
    );
    #[cfg(unix)]
    assert!(
        reason("dangling").contains("dangling.toml"),
        "{}",
        reason("dangling")
    );
    let future = set.unusable.iter().find(|u| u.id == "future").unwrap();
    assert_eq!(future.name.as_deref(), Some("Future"));
    assert_eq!(future.layers.len(), 1);
    assert_eq!(set.problem, None);
}

/// 목록은 디스크에 쓰지 않는다 — `schema` 없는 옛 파일도 그대로다.
#[test]
fn reading_does_not_migrate_files() {
    let f = Fixture::new();
    let path = f.run_file(
        "old.toml",
        "name = 'Old'\ntype = 'shell'\ncommand = 'true'\n",
    );
    let set = read(&f.place());
    assert_eq!(set.configs.len(), 1);
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        "name = 'Old'\ntype = 'shell'\ncommand = 'true'\n"
    );
    assert!(!f.root.join(".pitwall/.backup").exists());
}

// --- ③ 루트 밖 경로 -----------------------------------------------------------------------------------------------

fn unusable_reason(f: &Fixture, body: &str) -> Option<String> {
    let _ = fs::remove_dir_all(f.root.join(".pitwall/run"));
    f.run_file(
        "x.toml",
        &format!("schema = 1\nname = 'x'\ntype = 'shell'\ncommand = 'true'\n{body}"),
    );
    let set = read(&f.place());
    set.unusable.first().map(|u| u.reason.clone())
}

/// `cwd` · `env_file` · compose `file` 이 `..` · 심링크 · 절대 경로 · 대소문자로 루트 밖이면 쓸 수 없다. 루트 안은 쓴다.
#[test]
fn paths_outside_the_root_make_the_config_unusable() {
    let f = Fixture::new();
    let outside = f.base.join("outside");
    fs::create_dir_all(outside.join("deep")).unwrap();
    // 루트 이름으로 시작하는 옆 폴더 — 경로 조각이 아니라 글 앞부분으로 비교하면 루트 안으로 보인다.
    fs::create_dir_all(f.base.join("proj-outside")).unwrap();
    fs::write(outside.join(".env"), "SECRET=1\n").unwrap();
    f.dir("app/sub");
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&outside, f.root.join("out-link")).unwrap();
        std::os::unix::fs::symlink(outside.join(".env"), f.root.join(".env.linked")).unwrap();
        std::os::unix::fs::symlink(f.base.join("gone"), f.root.join("dangling")).unwrap();
        std::os::unix::fs::symlink(f.root.join("app"), f.root.join("in-link")).unwrap();
    }

    let mut refused = vec![
        "cwd = '..'",
        "cwd = '../outside'",
        "cwd = 'app/../../outside/deep'",
        "cwd = 'app/sub/../../..'",
        "cwd = 'not-yet/../../outside'",
        "cwd = '../OUTSIDE'",
        "cwd = '../proj-outside'",
        "env_file = ['../proj-outside/.env']",
        "env_file = ['../outside/.env']",
        "env_file = ['.env', 'app/../../outside/.env']",
    ];
    let absolute = format!("cwd = '{}'", outside.display());
    refused.push(&absolute);
    #[cfg(unix)]
    refused.extend([
        "cwd = 'out-link'",
        "cwd = 'out-link/deep'",
        "env_file = ['.env.linked']",
        "cwd = 'dangling/x'",
    ]);
    for body in refused {
        let reason = unusable_reason(&f, body).unwrap_or_else(|| panic!("`{body}` was accepted"));
        let field = body.split(' ').next().unwrap();
        assert!(reason.starts_with(field), "`{body}`: {reason}");
    }

    let accepted = [
        "cwd = 'app/sub'",
        "cwd = './app/../app/sub'",
        "cwd = 'build/out'",
        "env_file = ['.env.local', 'app/.env']",
        #[cfg(unix)]
        "cwd = 'in-link/sub'",
    ];
    for body in accepted {
        assert_eq!(unusable_reason(&f, body), None, "`{body}` was refused");
    }
}

/// 대소문자: 판정은 디스크의 실제 경로로 한다. 대소문자를 가리지 않는 파일 시스템(macOS 기본)에서 루트를 다른 대소문자로 적어도
/// 같은 폴더로 보고, 다른 대소문자로 적은 밖 폴더(`../OUTSIDE`)는 밖이다. 대소문자를 가리는 파일 시스템에서는 그런 폴더가 없어
/// 루트 아래의 없는 폴더로도 풀리지 않는다(밖).
#[test]
fn case_does_not_get_around_the_root_check() {
    let f = Fixture::new();
    fs::create_dir_all(f.base.join("outside")).unwrap();
    f.dir("app");
    let upper_root = f.base.join("PROJ");
    let case_insensitive = upper_root.is_dir();
    let resolved = root::inside(&upper_root, "app");
    if case_insensitive {
        assert_eq!(resolved.unwrap(), f.root.join("app"));
    } else {
        assert!(resolved.is_err());
    }
    assert!(root::inside(&f.root, "../OUTSIDE").is_err());
    assert!(root::inside(&f.root, "../PROJ-outside").is_err());
    let back_in = root::inside(&f.root, "../PROJ/app");
    if case_insensitive {
        assert_eq!(back_in.unwrap(), f.root.join("app"));
    } else {
        assert!(back_in.is_err());
    }
}

// --- ④ 개인 파일이 덮는다 ---------------------------------------------------------------------------------------

#[test]
fn local_file_overrides_the_same_keys() {
    let f = Fixture::new();
    f.dir("web");
    f.run_file(
        "dev.toml",
        "schema = 1\nname = 'Dev'\ntype = 'shell'\ncommand = 'npm run dev'\nenv = { PORT = '3000', MODE = 'dev' }\n",
    );
    f.run_file(
        "dev.local.toml",
        "schema = 1\ncwd = 'web'\n[env]\nPORT = '4000'\n",
    );
    let set = read(&f.place());
    let dev = set.get("dev").expect("dev is usable");
    assert_eq!(dev.config.cwd.as_deref(), Some("web"));
    assert_eq!(dev.config.env["PORT"], "4000");
    assert_eq!(dev.config.env["MODE"], "dev");
    let origins: Vec<LayerOrigin> = dev.layers.iter().map(|l| l.origin).collect();
    assert_eq!(origins, [LayerOrigin::Shared, LayerOrigin::Local]);
}

/// 실제 git 워크트리와 같은 모양(`.git` 파일 → `<main>/.git/worktrees/<이름>` + `commondir`).
fn fake_worktree(main: &Path, wt: &Path) {
    let admin = main.join(".git/worktrees/wt");
    fs::create_dir_all(&admin).unwrap();
    fs::create_dir_all(wt).unwrap();
    fs::write(admin.join("commondir"), "../..\n").unwrap();
    fs::write(wt.join(".git"), format!("gitdir: {}\n", admin.display())).unwrap();
    fs::write(
        admin.join("gitdir"),
        format!("{}\n", wt.join(".git").display()),
    )
    .unwrap();
}

/// 워크트리: 공유 파일은 그 워크트리의 것, 개인 파일은 없으면 메인 체크아웃의 것.
#[test]
fn a_worktree_reads_its_shared_file_and_falls_back_to_the_main_local_file() {
    let f = Fixture::new();
    let wt = f.base.join("wt");
    fake_worktree(&f.root, &wt);
    f.run_file(
        "dev.toml",
        "schema = 1\nname = 'Main'\ntype = 'shell'\ncommand = 'main'\n",
    );
    f.run_file(
        "dev.local.toml",
        "schema = 1\nenv = { FROM = 'main local' }\n",
    );
    let wt_run = wt.join(".pitwall/run");
    fs::create_dir_all(&wt_run).unwrap();
    fs::write(
        wt_run.join("dev.toml"),
        "schema = 1\nname = 'Worktree'\ntype = 'shell'\ncommand = 'wt'\n",
    )
    .unwrap();
    let place = RunConfigPlace::new(&f.paths, &f.project, Some(&wt.display().to_string())).unwrap();
    assert_eq!(place.root, wt);
    let set = read(&place);
    let dev = set.get("dev").unwrap();
    assert_eq!(dev.config.name, "Worktree");
    assert_eq!(dev.config.env["FROM"], "main local");
    assert_eq!(
        dev.layers[1].path,
        f.root
            .join(".pitwall/run/dev.local.toml")
            .display()
            .to_string()
    );
}

// --- ⑤ 쓰기 -------------------------------------------------------------------------------------------------

/// 고치기는 주석 · 모르는 키 · 키 자리를 지키고, 종류를 바꾸면 앞 종류의 필드는 남지 않는다.
#[test]
fn writing_keeps_comments_and_unknown_keys() {
    let f = Fixture::new();
    let path = f.run_file(
        "dev.toml",
        "# Dev server for the web app\nschema = 1\nname = \"Dev\" # shown in the selector\n\
         type = \"shell\"\ncommand = \"npm run dev\"\n# added by a newer Pitwall\nfuture_key = \"keep me\"\n\
         env = { PORT = \"3000\" } # the port\n\n[debug]\n# attach to the inspector\nmode = \"attach\"\nport = 9229\n",
    );
    let place = f.place();
    let mut config = read(&place).get("dev").unwrap().config.clone();
    config.name = "Dev (web)".into();
    config.env.insert("PORT".into(), "4000".into());
    write(&place, "dev", RunConfigLocation::Shared, Some(&config)).unwrap();
    let text = fs::read_to_string(&path).unwrap();
    insta::assert_snapshot!("edited_shell", text);
    for kept in [
        "# Dev server for the web app",
        "# shown in the selector",
        "# added by a newer Pitwall\nfuture_key = \"keep me\"",
        "# the port",
        "# attach to the inspector",
    ] {
        assert!(text.contains(kept), "lost {kept:?}:\n{text}");
    }
    assert_eq!(read(&place).get("dev").unwrap().config, config);

    config.kind = RunKindConfig::Npm {
        script: "dev".into(),
    };
    write(&place, "dev", RunConfigLocation::Shared, Some(&config)).unwrap();
    let text = fs::read_to_string(&path).unwrap();
    assert!(!text.contains("command"), "the shell field stayed:\n{text}");
    assert!(text.contains("future_key"), "{text}");
    assert_eq!(read(&place).get("dev").unwrap().config, config);
}

/// 새 파일은 `schema = 1` 로 시작하고 다시 읽으면 같은 구성이다.
#[test]
fn a_new_file_round_trips() {
    let f = Fixture::new();
    f.dir("backend");
    let place = f.place();
    let config = parse(
        "name = 'API'\ntype = 'gradle'\ntasks = ['bootRun']\ncwd = 'backend'\nenv = { SPRING_PROFILES_ACTIVE = 'local' }\n\
         env_file = ['.env.local']\n[profiles.prod-like]\nenv = { SPRING_PROFILES_ACTIVE = 'prod' }\n[profiles.empty]\n\
         [debug]\nadapter = 'java-debug'\nmode = 'attach'\nport = 5005\n",
    );
    let edit = write(&place, "api", RunConfigLocation::Shared, Some(&config)).unwrap();
    assert_eq!(edit.path, f.root.join(".pitwall/run/api.toml"));
    let text = fs::read_to_string(&edit.path).unwrap();
    assert!(text.starts_with("schema = 1\n"), "{text}");
    insta::assert_snapshot!("new_gradle", text);
    assert_eq!(read(&place).get("api").unwrap().config, config);
    // 같은 내용이면 바뀌지 않는다.
    let again = plan(&place, "api", RunConfigLocation::Shared, Some(&config)).unwrap();
    assert!(!again.changes());
}

#[test]
fn writing_refuses_files_it_cannot_safely_rewrite() {
    let f = Fixture::new();
    let place = f.place();
    let config = parse("name = 'x'\ntype = 'shell'\ncommand = 'true'");
    f.run_file("half.toml", "name = 'half\n");
    f.run_file("newer.toml", "schema = 2\nname = 'n'\n");
    for id in ["half", "newer"] {
        let err = write(&place, id, RunConfigLocation::Shared, Some(&config)).unwrap_err();
        assert_eq!(err.code, code::CONFLICT, "{id}: {err:?}");
    }
    assert_eq!(
        fs::read_to_string(f.root.join(".pitwall/run/half.toml")).unwrap(),
        "name = 'half\n"
    );
    let outside = parse("name = 'x'\ntype = 'shell'\ncommand = 'true'\ncwd = '..'");
    let err = write(&place, "x", RunConfigLocation::Shared, Some(&outside)).unwrap_err();
    assert_eq!(err.code, code::INVALID_PARAMS);
    let err = write(&place, "a.b", RunConfigLocation::Shared, Some(&config)).unwrap_err();
    assert_eq!(err.code, code::INVALID_PARAMS);
}

/// 계획 뒤 파일이 바뀌면 쓰지 않는다(다시 읽어 쓰면 실패).
#[test]
fn a_planned_edit_is_not_written_over_a_changed_file() {
    let f = Fixture::new();
    let place = f.place();
    let path = f.run_file(
        "dev.toml",
        "schema = 1\nname = 'Dev'\ntype = 'shell'\ncommand = 'a'\n",
    );
    let config = parse("name = 'Dev'\ntype = 'shell'\ncommand = 'b'");
    let edit = plan(&place, "dev", RunConfigLocation::Shared, Some(&config)).unwrap();
    fs::write(
        &path,
        "schema = 1\nname = 'Dev'\ntype = 'shell'\ncommand = 'edited by hand'\n",
    )
    .unwrap();
    let err = apply(&place, &edit).unwrap_err();
    assert_eq!(err.code, code::CONFLICT);
    assert!(
        fs::read_to_string(&path)
            .unwrap()
            .contains("edited by hand")
    );

    let created = plan(&place, "new", RunConfigLocation::Shared, Some(&config)).unwrap();
    f.run_file("new.toml", "schema = 1\nname = 'raced'\n");
    assert_eq!(apply(&place, &created).unwrap_err().code, code::CONFLICT);
}

/// 개인 파일을 처음 만들면 `.gitignore` 에 `*.local.toml` 을 두고, 저장소 안의 링크는 공유 · 개인 모두 따르지 않는다.
#[test]
fn local_writes_and_links_inside_the_repository() {
    let f = Fixture::new();
    let place = f.place();
    let config = parse("name = 'x'\ntype = 'shell'\ncommand = 'true'");
    let edit = write(&place, "dev", RunConfigLocation::Local, Some(&config)).unwrap();
    assert_eq!(edit.path, f.root.join(".pitwall/run/dev.local.toml"));
    let ignore = fs::read_to_string(f.root.join(".pitwall/.gitignore")).unwrap();
    assert!(ignore.lines().any(|l| l == "*.local.toml"), "{ignore}");

    #[cfg(unix)]
    {
        let target = f.base.join("dotfile.toml");
        fs::write(
            &target,
            "schema = 1\nname = 'outside'\ntype = 'shell'\ncommand = 'x'\n",
        )
        .unwrap();
        std::os::unix::fs::symlink(&target, f.root.join(".pitwall/run/linked.toml")).unwrap();
        let err = write(&place, "linked", RunConfigLocation::Shared, Some(&config)).unwrap_err();
        assert_eq!(err.code, code::CONFLICT, "{err:?}");
        assert!(fs::read_to_string(&target).unwrap().contains("outside"));
    }
}

#[test]
fn deleting_removes_the_file() {
    let f = Fixture::new();
    let place = f.place();
    let path = f.run_file(
        "dev.toml",
        "schema = 1\nname = 'Dev'\ntype = 'shell'\ncommand = 'a'\n",
    );
    let edit = write(&place, "dev", RunConfigLocation::Shared, None).unwrap();
    assert_eq!(edit.after, None);
    assert!(!path.exists());
    assert!(read(&place).configs.is_empty());
}

// --- ⑥ RPC 범위 ------------------------------------------------------------------------------------------------

struct Host {
    _tmp: tempfile::TempDir,
    configs: Arc<RunConfigs>,
    project: String,
    root: PathBuf,
}

fn host() -> Host {
    let tmp = tempfile::tempdir().unwrap();
    let base = fs::canonicalize(tmp.path()).unwrap();
    let paths = AppPaths::under(&base.join("app"));
    let projects = Registry::new(paths.clone(), None, EventBus::new(DEFAULT_CAPACITY));
    let root = base.join("proj");
    fs::create_dir_all(&root).unwrap();
    let project = projects
        .create(NewProject {
            root: root.clone(),
            ..NewProject::default()
        })
        .unwrap();
    let configs = Arc::new(RunConfigs::new());
    configs.attach(HostParts { paths, projects });
    Host {
        _tmp: tmp,
        configs,
        project: project.id,
        root,
    }
}

async fn call(
    host: &Host,
    scope: ConnectionScope,
    method: &str,
    params: Value,
) -> Result<Value, pw_rpc::RpcError> {
    let mut router = Router::new();
    register(&mut router, Arc::clone(&host.configs));
    let (session, mut rx) = Session::new(
        Arc::new(router),
        EventHub::new(),
        Caller {
            role: Role::Owner,
            scope,
            window: None,
        },
    );
    session.handle_value(json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}));
    loop {
        let message = tokio::time::timeout(Duration::from_secs(10), rx.recv())
            .await
            .expect("timed out")
            .expect("session closed");
        if let Outbound::Response(r) = message {
            assert_eq!(r.id, Id::Number(1));
            return r.outcome;
        }
    }
}

/// 다른 프로젝트 창 · Host · 에이전트 범위 연결의 `save` · `delete` 는 permission denied 이고 파일을 만들지 않는다. 그 프로젝트
/// 창은 쓴다. 목록은 어느 연결이든 읽는다.
#[tokio::test]
async fn only_the_project_window_may_save_or_delete() {
    let h = host();
    let save = json!({
        "projectId": h.project,
        "id": "dev",
        "config": {"name": "Dev", "type": "shell", "command": "npm run dev", "envFile": [".env.local"]},
    });
    let delete = json!({"projectId": h.project, "id": "dev"});
    let file = h.root.join(".pitwall/run/dev.toml");
    for scope in [
        ConnectionScope::Project("ffffffffffffffff".into()),
        ConnectionScope::Host,
        ConnectionScope::Agent,
    ] {
        let err = call(&h, scope.clone(), "run.config.save", save.clone())
            .await
            .unwrap_err();
        assert_eq!(err.code, code::PERMISSION_DENIED, "{scope:?}");
        assert!(!file.exists(), "{scope:?} wrote the file");
    }
    let own = ConnectionScope::Project(h.project.clone());
    let saved = call(&h, own.clone(), "run.config.save", save.clone())
        .await
        .unwrap();
    assert_eq!(saved["path"], json!(file.display().to_string()));
    assert_eq!(saved["changed"], json!(true));
    assert!(
        fs::read_to_string(&file)
            .unwrap()
            .contains("env_file = [\".env.local\"]")
    );

    for scope in [ConnectionScope::Host, ConnectionScope::Agent] {
        let list = call(
            &h,
            scope,
            "run.config.list",
            json!({"projectId": h.project}),
        )
        .await
        .unwrap();
        assert_eq!(list["configs"][0]["id"], "dev");
        assert_eq!(
            list["configs"][0]["config"]["envFile"],
            json!([".env.local"])
        );
        assert_eq!(list["unusable"], json!([]));
        assert_eq!(list["detected"], json!([]));
    }
    for scope in [
        ConnectionScope::Project("ffffffffffffffff".into()),
        ConnectionScope::Host,
        ConnectionScope::Agent,
    ] {
        let err = call(&h, scope.clone(), "run.config.delete", delete.clone())
            .await
            .unwrap_err();
        assert_eq!(err.code, code::PERMISSION_DENIED, "{scope:?}");
        assert!(file.exists(), "{scope:?} deleted the file");
    }
    let deleted = call(&h, own.clone(), "run.config.delete", delete.clone())
        .await
        .unwrap();
    assert_eq!(deleted["paths"], json!([file.display().to_string()]));
    assert!(!file.exists());
    let err = call(&h, own, "run.config.delete", delete)
        .await
        .unwrap_err();
    assert_eq!(err.code, code::NOT_FOUND);
}

#[tokio::test]
async fn save_refuses_a_case_twin_and_bad_input() {
    let h = host();
    let own = ConnectionScope::Project(h.project.clone());
    let config = json!({"name": "Dev", "type": "shell", "command": "x"});
    call(
        &h,
        own.clone(),
        "run.config.save",
        json!({"projectId": h.project, "id": "Dev", "config": config}),
    )
    .await
    .unwrap();
    let err = call(
        &h,
        own.clone(),
        "run.config.save",
        json!({"projectId": h.project, "id": "dev", "config": config}),
    )
    .await
    .unwrap_err();
    assert_eq!(err.code, code::CONFLICT, "{err:?}");
    let err = call(
        &h,
        own.clone(),
        "run.config.save",
        json!({"projectId": h.project, "id": "x", "config": {"name": "x", "type": "teleport"}}),
    )
    .await
    .unwrap_err();
    assert_eq!(err.code, code::INVALID_PARAMS, "{err:?}");
    let err = call(
        &h,
        own,
        "run.config.list",
        json!({"projectId": "ffffffffffffffff"}),
    )
    .await
    .unwrap_err();
    assert_eq!(err.code, code::NOT_FOUND, "{err:?}");
}

// --- ⑦ JSON Schema ---------------------------------------------------------------------------------------------

/// 폼(M6-07)과 MCP 입력 검증(M6-13)이 같이 쓰는 스키마. 바뀌면 이 스냅숏을 새로 보고 받아들인다.
#[test]
fn json_schema_snapshot() {
    insta::assert_snapshot!(serde_json::to_string_pretty(&json_schema()).unwrap());
}

/// 스키마 설명은 에이전트도 읽는다(M6-13 의 입력 스키마) — 영어여야 한다(결정 M5-D39).
#[test]
fn schema_descriptions_are_english() {
    let text = serde_json::to_string(&json_schema()).unwrap();
    assert!(
        !text
            .chars()
            .any(|c| ('\u{AC00}'..='\u{D7A3}').contains(&c)
                || ('\u{3131}'..='\u{318E}').contains(&c)),
        "Hangul in the run config schema"
    );
}

/// 여러 낱말 키는 [`FILE_KEYS`] 로 파일 키(snake_case)와 옮긴다 — 표에 없으면 파일에 camelCase 로 쓰인다.
#[test]
fn multi_word_keys_are_mapped() {
    for key in write::known_keys() {
        assert!(
            !key.chars().any(|c| c.is_ascii_uppercase()),
            "`{key}` has no snake_case file key in FILE_KEYS"
        );
    }
}

// --- 정규형 -------------------------------------------------------------------------------------------------------

/// 주석 · 공백 · 키 순서 · 인라인 표와 `[표]` · 레이어 나눔이 달라도 정규형이 같고, 값이 하나라도 다르면 다르다.
#[test]
fn the_canonical_form_ignores_layout_but_not_values() {
    let a = parse(
        "# comment\nname = 'Dev'\ntype = 'cargo'\ncommand = 'run'\nargs = ['--', 'x']\nenv = { B = '2', A = '1' }\n\
         [debug]\nport = 9229\nmode = 'attach'\n",
    );
    let b = parse(
        "type   =   'cargo'\nargs = [ \"--\", \"x\" ]\ncommand = \"run\"\nname = \"Dev\"\n\n[env]\nA = '1' # one\nB = '2'\n\
         [debug]\nmode = 'attach'\nport = 9229\n",
    );
    assert_eq!(a.canonical(), b.canonical());
    assert!(
        a.canonical()
            .starts_with("{\"args\":[\"--\",\"x\"],\"command\":\"run\",\"cwd\":null,\"debug\":"),
        "{}",
        a.canonical()
    );

    let mut c = a.clone();
    c.env.insert("A".into(), "one".into());
    assert_ne!(a.canonical(), c.canonical());
    let mut d = a.clone();
    d.debug
        .as_mut()
        .unwrap()
        .settings
        .insert("port".into(), json!(9230));
    assert_ne!(a.canonical(), d.canonical());
}

/// 레이어로 나눠 적은 구성과 한 파일에 적은 구성은 정규형이 같다.
#[test]
fn the_canonical_form_does_not_see_layers() {
    let f = Fixture::new();
    f.run_file(
        "split.toml",
        "schema = 1\nname = 'S'\ntype = 'shell'\ncommand = 'x'\n",
    );
    f.run_file("split.local.toml", "schema = 1\nenv = { K = 'v' }\n");
    f.run_file(
        "whole.toml",
        "schema = 1\n# all in one\nenv = { K = 'v' }\ncommand = 'x'\ntype = 'shell'\nname = 'S'\n",
    );
    let set = read(&f.place());
    assert_eq!(
        set.get("split").unwrap().config.canonical(),
        set.get("whole").unwrap().config.canonical()
    );
}

// --- 리뷰(17-m6-plan M6-01 리뷰) 뒤에 더한 것 ------------------------------------------------------------------------

/// 리뷰 2: 저장소 안의 구성 파일이 체크아웃 밖을 가리키는 링크면 읽지 않고, 까닭에 그 파일 내용이 실리지 않는다. 저장소 안을
/// 가리키는 링크는 읽는다.
#[cfg(unix)]
#[test]
fn a_link_out_of_the_repository_is_not_read() {
    let f = Fixture::new();
    let secret = f.base.join("secret.ini");
    fs::write(
        &secret,
        "[default]\naws_secret_access_key = SUPERSECRETVALUE123\n",
    )
    .unwrap();
    let outside_toml = f.base.join("outside.toml");
    fs::write(
        &outside_toml,
        "name = 'OUTSIDE NAME'\ntype = 'shell'\ncommand = 'cat ~/.ssh/id_rsa'\n",
    )
    .unwrap();
    let inside = f.root.join("configs/dev.toml");
    fs::create_dir_all(inside.parent().unwrap()).unwrap();
    fs::write(
        &inside,
        "name = 'Inside'\ntype = 'shell'\ncommand = 'true'\n",
    )
    .unwrap();
    let run = f.root.join(".pitwall/run");
    fs::create_dir_all(&run).unwrap();
    std::os::unix::fs::symlink(&secret, run.join("leak.toml")).unwrap();
    std::os::unix::fs::symlink(&outside_toml, run.join("valid.toml")).unwrap();
    std::os::unix::fs::symlink(&inside, run.join("inside.toml")).unwrap();

    let set = read(&f.place());
    assert_eq!(
        set.configs
            .iter()
            .map(|c| c.id.as_str())
            .collect::<Vec<_>>(),
        ["inside"]
    );
    for id in ["leak", "valid"] {
        let u = set.unusable.iter().find(|u| u.id == id).unwrap();
        assert!(
            u.reason.contains("outside the project"),
            "{id}: {}",
            u.reason
        );
        for content in [
            "SUPERSECRETVALUE123",
            "aws_secret",
            "OUTSIDE NAME",
            "id_rsa",
        ] {
            assert!(!u.reason.contains(content), "{id}: {}", u.reason);
        }
        assert_eq!(u.name, None, "{id}");
    }

    // 개인 전용 폴더는 dotfiles 링크를 따른다.
    let mut personal = f.project.clone();
    personal.config_mode = ConfigMode::Personal;
    let dir = f.paths.personal_project_dir("proj").join("run");
    fs::create_dir_all(&dir).unwrap();
    std::os::unix::fs::symlink(&outside_toml, dir.join("dotfile.toml")).unwrap();
    let place = RunConfigPlace::new(&f.paths, &personal, None).unwrap();
    assert_eq!(
        read(&place).get("dotfile").unwrap().config.name,
        "OUTSIDE NAME"
    );
}

/// 까닭 글에는 파일의 값이 실리지 않는다 — TOML 오류는 줄 · 칸만, 꼴 오류는 값을 걷는다(env 값은 비밀일 수 있다).
#[test]
fn unusable_reasons_carry_no_file_values() {
    let f = Fixture::new();
    for (id, body, line) in [
        (
            "toml",
            "name = 'x'\ntype = 'shell'\ncommand = 'x'\nenv = { API_KEY = \"SECRET-ONE }\n",
            Some("line 5"),
        ),
        (
            "number",
            "name = 'x'\ntype = 'shell'\ncommand = 'x'\nenv = { PORT = 31337 }\n",
            None,
        ),
        (
            "string",
            "name = 'x'\ntype = 'shell'\ncommand = 'x'\nenv_file = \"SECRET-TWO\"\n",
            None,
        ),
        ("bool", "name = 'x'\ntype = 'shell'\ncommand = true\n", None),
    ] {
        f.run_file(&format!("{id}.toml"), &format!("schema = 1\n{body}"));
        let set = read(&f.place());
        let u = set.unusable.iter().find(|u| u.id == id).unwrap();
        for content in ["SECRET-ONE", "31337", "SECRET-TWO", "true"] {
            assert!(!u.reason.contains(content), "{id}: {}", u.reason);
        }
        if let Some(line) = line {
            assert!(u.reason.contains(line), "{id}: {}", u.reason);
        }
    }
    assert_eq!(
        read::redact(
            r#"invalid type: string "a \"b\" c", expected a sequence; integer `5`; unknown variant `x`"#
        ),
        "invalid type: string, expected a sequence; integer; unknown variant `x`"
    );
}

/// 리뷰 3: 쓰기는 `[profiles.<이름>]` 안의 모르는 키(새 버전이 더한 키)도 지킨다. 새 구성에 없는 프로필은 지운다.
#[test]
fn writing_keeps_unknown_keys_inside_a_profile() {
    let f = Fixture::new();
    let path = f.run_file(
        "dev.toml",
        "schema = 1\nname = 'Dev'\ntype = 'cargo'\ncommand = 'run'\n\n[profiles.fast]\n# from a newer Pitwall\n\
         future_profile_key = 'keep me'\nargs = ['--release']\n\n[profiles.gone]\nargs = []\n",
    );
    let place = f.place();
    let mut config = read(&place).get("dev").unwrap().config.clone();
    config.profiles.remove("gone");
    config.profiles.get_mut("fast").unwrap().args = Some(strings(&["--release", "--", "x"]));
    write(&place, "dev", RunConfigLocation::Shared, Some(&config)).unwrap();
    let text = fs::read_to_string(&path).unwrap();
    assert!(
        text.contains("# from a newer Pitwall\nfuture_profile_key = 'keep me'"),
        "{text}"
    );
    assert!(!text.contains("[profiles.gone]"), "{text}");
    assert_eq!(read(&place).get("dev").unwrap().config, config);
}

/// 빈자리 ②: 워크트리와 메인 체크아웃에 개인 파일이 둘 다 있으면 워크트리 것만 쓴다(파일 단위 폴백 — 병합하지 않는다).
#[test]
fn a_worktree_local_file_wins_over_the_main_one_as_a_whole() {
    let f = Fixture::new();
    let wt = f.base.join("wt");
    fake_worktree(&f.root, &wt);
    f.run_file(
        "dev.local.toml",
        "schema = 1\nenv = { FROM = 'main', ONLY_MAIN = '1' }\n",
    );
    let wt_run = wt.join(".pitwall/run");
    fs::create_dir_all(&wt_run).unwrap();
    fs::write(
        wt_run.join("dev.toml"),
        "schema = 1\nname = 'W'\ntype = 'shell'\ncommand = 'wt'\n",
    )
    .unwrap();
    fs::write(
        wt_run.join("dev.local.toml"),
        "schema = 1\nenv = { FROM = 'worktree' }\n",
    )
    .unwrap();
    let place = RunConfigPlace::new(&f.paths, &f.project, Some(&wt.display().to_string())).unwrap();
    let set = read(&place);
    let dev = set.get("dev").unwrap();
    assert_eq!(
        dev.config.env.get("FROM").map(String::as_str),
        Some("worktree")
    );
    assert_eq!(dev.config.env.get("ONLY_MAIN"), None);
    assert_eq!(
        dev.layers[1].path,
        wt_run.join("dev.local.toml").display().to_string()
    );
}

/// 빈자리 ③: `.pitwall/run` 자체가 체크아웃 밖을 가리키는 링크면 쓰지 않는다.
#[cfg(unix)]
#[test]
fn writing_refuses_a_run_folder_that_resolves_outside_the_checkout() {
    let f = Fixture::new();
    let outside = f.base.join("elsewhere");
    fs::create_dir_all(&outside).unwrap();
    fs::create_dir_all(f.root.join(".pitwall")).unwrap();
    std::os::unix::fs::symlink(&outside, f.root.join(".pitwall/run")).unwrap();
    let config = parse("name = 'x'\ntype = 'shell'\ncommand = 'true'");
    let err = write(&f.place(), "dev", RunConfigLocation::Shared, Some(&config)).unwrap_err();
    assert_eq!(err.code, code::CONFLICT, "{err:?}");
    assert!(
        fs::read_dir(&outside).unwrap().next().is_none(),
        "wrote outside the checkout"
    );
}

/// 빈자리 ⑤: 프로필의 `args` 는 `args` 가 있는 종류만, 프로필 env 이름도 검증한다.
#[test]
fn profiles_are_validated() {
    let f = Fixture::new();
    for body in [
        "[profiles.p]\nargs = ['--x']",
        "[profiles.p]\nenv = { 'BAD-NAME' = '1' }",
        "[profiles.'a.b']",
    ] {
        let reason = unusable_reason(&f, body).unwrap_or_else(|| panic!("`{body}` was accepted"));
        assert!(reason.starts_with("profile"), "`{body}`: {reason}");
    }
    assert_eq!(
        unusable_reason(&f, "[profiles.p]\nenv = { GOOD = '1' }"),
        None
    );
}

/// 권고 2: 이름 자리 값이 `-` 로 시작하면 옵션으로 읽히므로 쓸 수 없다. 옵션이 정상인 자리(`args` · `tasks`)는 받는다.
#[test]
fn a_name_that_reads_as_an_option_is_unusable() {
    let f = Fixture::new();
    let place = f.place();
    for (id, body) in [
        ("npm", "type = 'npm'\nscript = '--prefix=/tmp'"),
        ("py", "type = 'python'\nscript = '-c'\nargs = ['print(1)']"),
        (
            "pymod",
            "type = 'python'\nmodule = '-c'\nargs = ['print(1)']",
        ),
        ("gocmd", "type = 'go'\ncommand = '-toolexec=/bin/sh'"),
        (
            "go",
            "type = 'go'\ncommand = 'run'\npackage = '-exec=/bin/sh'",
        ),
        ("mise", "type = 'mise-task'\ntask = '--cd=/'"),
        (
            "compose",
            "type = 'docker-compose'\nservices = ['web', '--build']",
        ),
        ("cargo", "type = 'cargo'\ncommand = '--config=x'"),
    ] {
        f.run_file(
            &format!("{id}.toml"),
            &format!("schema = 1\nname = 'x'\n{body}\n"),
        );
        let set = read(&place);
        let u = set
            .unusable
            .iter()
            .find(|u| u.id == id)
            .unwrap_or_else(|| panic!("{id} was accepted"));
        assert!(
            u.reason.contains("must not start with `-`"),
            "{id}: {}",
            u.reason
        );
    }
    let fine = parse("name = 'x'\ntype = 'cargo'\ncommand = 'run'\nargs = ['--release']");
    validate(&fine, &f.root).unwrap();
    let fine = parse("name = 'x'\ntype = 'gradle'\ntasks = ['--offline', 'build']");
    validate(&fine, &f.root).unwrap();
}

/// 권고 1: 루트가 링크를 지난 경로(macOS 의 `/var` → `/private/var`)여도 작업 폴더의 실제 경로와 맞춰 lockfile · 래퍼를 찾는다.
#[cfg(unix)]
#[test]
fn lockfiles_are_found_when_the_root_is_reached_through_a_link() {
    let f = Fixture::new();
    fs::write(f.root.join("pnpm-lock.yaml"), "").unwrap();
    fs::write(f.root.join("gradlew"), "").unwrap();
    let web = f.dir("packages/web");
    let linked = f.base.join("linked-root");
    std::os::unix::fs::symlink(&f.root, &linked).unwrap();
    let login = shell();
    let cx = ArgvContext {
        root: &linked,
        cwd: &web,
        login_shell: &login,
    };
    let npm = parse("name = 'n'\ntype = 'npm'\nscript = 'dev'");
    assert_eq!(argv(&npm, None, &cx).unwrap()[0], "pnpm");
    let gradle = parse("name = 'g'\ntype = 'gradle'\ntasks = ['build']");
    assert_eq!(
        argv(&gradle, None, &cx).unwrap()[0],
        f.root.join("gradlew").display().to_string()
    );
}

/// 빈자리 ④: `run.config.delete` 는 정확히 그 id 의 구성만 지운다 — 대소문자를 가리지 않는 파일 시스템에서 `dev` 로 `Dev.toml`
/// 을 지우지 않는다.
#[tokio::test]
async fn delete_takes_the_exact_id_only() {
    let h = host();
    let own = ConnectionScope::Project(h.project.clone());
    let file = h.root.join(".pitwall/run/Dev.toml");
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(
        &file,
        "schema = 1\nname = 'Dev'\ntype = 'shell'\ncommand = 'x'\n",
    )
    .unwrap();
    for location in [Value::Null, json!("shared")] {
        let err = call(
            &h,
            own.clone(),
            "run.config.delete",
            json!({"projectId": h.project, "id": "dev", "location": location}),
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, code::NOT_FOUND, "{err:?}");
        assert!(file.exists(), "`dev` deleted `Dev.toml`");
    }
}

/// 재확인 리뷰 권고 4: 크기 상한(1 MiB, `pw_core::config::robust::MAX_CONFIG_BYTES`)을 넘는 구성 파일은 읽지 않고 「쓸 수 없음」 이다.
#[test]
fn an_oversized_run_file_is_unusable() {
    let f = Fixture::new();
    let filler = "x".repeat(pw_core::config::robust::MAX_CONFIG_BYTES as usize);
    f.run_file(
        "big.toml",
        &format!("schema = 1\nname = 'big'\ntype = 'shell'\ncommand = 'true'\n# {filler}\n"),
    );
    let set = read(&f.place());
    let u = set.unusable.iter().find(|u| u.id == "big").unwrap();
    assert!(u.reason.contains("larger than"), "{}", u.reason);
}

// --- `sql` 종류(M6-09) ----------------------------------------------------------------------------------------

/// `sql` 구성: 필드(`connection` · `files` · `transaction` — 기본 `all`)를 읽고, 프로세스가 아니라 argv 를 만들지 않는다. 파일이
/// 루트 밖이거나 목록이 비면 쓸 수 없다.
#[test]
fn an_sql_config_reads_its_fields_and_has_no_argv() {
    use super::{RunKindConfig, SqlTransaction};
    let config = parse(
        "name = 'seed'\ntype = 'sql'\nconnection = 'local-pg'\nfiles = ['db/seed.sql', 'db/fixtures.sql']\n",
    );
    assert_eq!(
        config.kind,
        RunKindConfig::Sql {
            connection: "local-pg".into(),
            files: strings(&["db/seed.sql", "db/fixtures.sql"]),
            transaction: SqlTransaction::All,
        }
    );
    for (text, mode) in [
        ("all", SqlTransaction::All),
        ("per_file", SqlTransaction::PerFile),
        ("none", SqlTransaction::None),
    ] {
        let config = parse(&format!(
            "name = 's'\ntype = 'sql'\nconnection = 'c'\nfiles = ['a.sql']\ntransaction = '{text}'\n"
        ));
        let RunKindConfig::Sql { transaction, .. } = config.kind else {
            panic!()
        };
        assert_eq!(transaction, mode);
        assert_eq!(transaction.name(), text);
    }
    let f = Fixture::new();
    let cx = ArgvContext {
        root: &f.root,
        cwd: &f.root,
        login_shell: &shell(),
    };
    assert_eq!(
        super::argv::argv(&config, None, &cx).unwrap_err(),
        super::argv::NOT_A_PROCESS
    );

    let sql = |body: &str| {
        let _ = fs::remove_dir_all(f.root.join(".pitwall/run"));
        f.run_file(
            "s.toml",
            &format!("schema = 1\nname = 's'\ntype = 'sql'\nconnection = 'c'\n{body}"),
        );
        let set = read(&f.place());
        set.unusable.first().map(|u| u.reason.clone())
    };
    assert_eq!(sql("files = ['db/seed.sql']"), None);
    for refused in [
        "files = []",
        "files = ['../outside.sql']",
        "files = ['db/a.sql', '../../x.sql']",
        "files = ['a.sql']\ntransaction = 'nested'",
        &format!("files = [{}]", vec!["'a.sql'"; 65].join(", ")),
    ] {
        assert!(sql(refused).is_some(), "`{refused}` was accepted");
    }
    assert!(
        sql("files = ['../outside.sql']")
            .unwrap()
            .starts_with("files[0]")
    );
}
