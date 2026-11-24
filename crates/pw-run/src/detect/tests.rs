//! 태스크 감지(17-m6-plan M6-04)의 완료 기준 ① ~ ⑤ 와 읽기의 경계(큰 파일 · 이상한 꼴 · 루트 밖 링크).

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use pw_core::bus::{DEFAULT_CAPACITY, EventBus, Filter, Received, Scope};
use pw_core::config::AppPaths;
use pw_core::registry::{NewProject, Registry};
use pw_rpc::{Caller, ConnectionScope, EventHub, Id, Outbound, Role, Router, Session};
use serde_json::{Value, json};

use super::*;
use crate::config::argv::{ArgvContext, argv};
use crate::config::methods::{HostParts, RunConfigs, register};

const PACKAGE_JSON: &str = r#"{
  "name": "web",
  "scripts": {
    "dev": "vite --port 5173",
    "predev": "node scripts/check.js",
    "build:prod": "vite build --mode production",
    "postbuild:prod": "node scripts/size.js",
    "test": "vitest run",
    "--weird": "echo option",
    "broken": 5
  }
}
"#;

const MAKEFILE: &str = "\
# 주석
CC := cc
FLAGS = -O2 -Wall: x
OUT ?= build
.PHONY: build test clean
.DEFAULT_GOAL := build

build: deps ## 빌드
\t$(CC) $(FLAGS) -o $(OUT)/app \\
\t  main.c

# 레시피 사이 주석

test: build
\t./run-tests.sh

%.o: %.c
\t$(CC) -c $<

$(OUT):
\tmkdir -p $@

.c.o:
\t$(CC) -c $<

clean::
\trm -rf $(OUT)

debug: FLAGS = -g

define TEMPLATE
fake: target
\techo inside define
endef

ifeq ($(OS),Windows_NT)
win-only: ; echo windows
endif

a;rm: x
\techo unsafe
deps docs/site:
\techo deps
";

const JUSTFILE: &str = "\
set shell := [\"bash\", \"-c\"]
alias b := build
version := `git describe`
export RUST_LOG := \"info\"

# 빌드한다
build target='debug:fast':
    cargo build --profile {{target}}

    echo done

[private]
helper:
    echo hidden

_secret:
    echo hidden

[group('ci')]
@test *args: build
    cargo test {{args}}

serve port:
\tpython3 -m http.server {{port}}

import 'other.just'
mod tools
";

const COMPOSE: &str = "\
x-env: &env
  RUST_LOG: info
  TOKEN: \"abc\"
services:
  db:
    image: postgres:16
    environment: *env
  web:
    <<: &web
      build: .
    command: [\"npm\", \"run\", \"dev\"]
    ports:
      - \"5173:5173\"
  'cache':
    image: redis
volumes:
  data: {}
";

/// 가짜 실행기 — 불린 수를 센다.
#[derive(Default)]
struct CountingExec(AtomicUsize);

impl DetectExec for CountingExec {
    fn output(&self, _detector: &str, _argv: &[String], _cwd: &Path) -> Result<String, String> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(String::new())
    }
}

fn root() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let root =
        pw_core::registry::path::simplify(fs::canonicalize(tmp.path()).unwrap()).join("proj");
    fs::create_dir_all(&root).unwrap();
    (tmp, root)
}

fn all_fixtures(root: &Path) {
    fs::write(root.join("package.json"), PACKAGE_JSON).unwrap();
    fs::write(root.join("pnpm-lock.yaml"), "lockfileVersion: '9.0'\n").unwrap();
    fs::write(root.join("Makefile"), MAKEFILE).unwrap();
    fs::write(root.join("justfile"), JUSTFILE).unwrap();
    fs::write(root.join("compose.yaml"), COMPOSE).unwrap();
}

/// 스냅숏용 글: 감지기 · 열쇠 · argv · 정의 본문, 그리고 까닭.
fn render(detection: &Detection, root: &Path) -> String {
    let login = vec!["/bin/zsh".to_owned(), "-l".to_owned()];
    let cx = ArgvContext {
        root,
        cwd: root,
        login_shell: &login,
    };
    let root_text = root.display().to_string();
    let mut out = String::new();
    for task in &detection.tasks {
        // 스냅숏은 OS 마다 같은 글이다 — 루트 안 경로는 `<root>/…`(`/` 로), Windows 의 셸 옵션(`-Command`)은 `-c` 로 적는다. 그
        // 옵션 자체는 `config::tests::every_kind_builds_its_argv_field_by_field` 가 OS 마다 본다.
        let argv: Vec<String> = argv(&task.config, None, &cx)
            .unwrap()
            .into_iter()
            .map(|arg| match arg.strip_prefix(&root_text) {
                Some(rest) => format!("<root>{}", rest.replace('\\', "/")),
                None if cfg!(windows) && arg == "-Command" => "-c".to_owned(),
                None => arg,
            })
            .collect();
        out.push_str(&format!(
            "{key}\n  source: {source}\n  config: {config}\n  argv: {argv:?}\n  definition: {definition:?}\n",
            key = task.key(),
            source = task.source.strip_prefix(root).unwrap().display(),
            config = task.config.canonical(),
            definition = task.definition,
        ));
    }
    for problem in &detection.problems {
        out.push_str(&format!(
            "problem[{}]: {}\n",
            problem.detector, problem.reason
        ));
    }
    out.replace(&root_text, "<root>")
}

// --- ① 감지기마다 픽스처 → 태스크 목록 스냅숏 ---------------------------------------------------------------------------

#[test]
fn npm_scripts_snapshot() {
    let (_tmp, root) = root();
    fs::write(root.join("package.json"), PACKAGE_JSON).unwrap();
    fs::write(root.join("pnpm-lock.yaml"), "").unwrap();
    insta::assert_snapshot!(render(&detect(&root, &NoExec), &root));
}

#[test]
fn makefile_targets_snapshot() {
    let (_tmp, root) = root();
    fs::write(root.join("Makefile"), MAKEFILE).unwrap();
    insta::assert_snapshot!(render(&detect(&root, &NoExec), &root));
}

#[test]
fn justfile_recipes_snapshot() {
    let (_tmp, root) = root();
    fs::write(root.join("justfile"), JUSTFILE).unwrap();
    insta::assert_snapshot!(render(&detect(&root, &NoExec), &root));
}

#[test]
fn compose_services_snapshot() {
    let (_tmp, root) = root();
    fs::write(root.join("compose.yaml"), COMPOSE).unwrap();
    insta::assert_snapshot!(render(&detect(&root, &NoExec), &root));
}

#[test]
fn empty_project_detects_nothing() {
    let (_tmp, root) = root();
    assert_eq!(detect(&root, &NoExec), Detection::default());
}

// --- ② Makefile 의 .PHONY · 패턴 규칙 · 변수 대상은 빠진다 -----------------------------------------------------------

#[test]
fn makefile_leaves_out_special_pattern_and_variable_targets() {
    let parsed = make::parse(MAKEFILE);
    let names: Vec<&str> = parsed.targets.iter().map(String::as_str).collect();
    assert_eq!(
        names,
        ["build", "clean", "deps", "docs/site", "test", "win-only"]
    );
    for gone in [
        ".PHONY",
        ".DEFAULT_GOAL",
        "%.o",
        "$(OUT)",
        ".c.o",
        "debug",
        "CC",
        "FLAGS",
        "OUT",
        "fake",
    ] {
        assert!(!parsed.targets.contains(gone), "{gone} was detected");
    }
    // `a;rm` 은 셸에 안전하지 않아 빠지고 줄 번호가 남는다.
    assert_eq!(parsed.unsafe_lines, [40]);
}

#[test]
fn makefile_variants_follow_gnu_make_order() {
    let (_tmp, root) = root();
    fs::write(root.join("makefile"), "lower:\n\ttrue\n").unwrap();
    fs::write(root.join("GNUmakefile"), "gnu:\n\ttrue\n").unwrap();
    let detection = detect(&root, &NoExec);
    let names: Vec<&str> = detection.tasks.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, ["gnu"]);
    assert!(detection.tasks[0].source.ends_with("GNUmakefile"));
}

// --- ③ 감지가 어떤 프로세스도 띄우지 않는다 ------------------------------------------------------------------------

#[test]
fn file_detectors_start_no_process() {
    let (_tmp, root) = root();
    all_fixtures(&root);
    let exec = CountingExec::default();
    let detection = detect(&root, &exec);
    assert!(detection.tasks.len() > 10, "{detection:#?}");
    assert_eq!(
        exec.0.load(Ordering::SeqCst),
        0,
        "a file-only detector called the executor"
    );
}

/// 감지기는 프로세스를 [`DetectExec`] 로만 띄운다 — `std::process` · `tokio::process` · `pw_spawn` 을 직접 부르면 가짜 실행기로는
/// 잡히지 않으므로 소스에서 본다(감지기에 `just --dump` 를 `Command` 로 넣으면 실패).
#[test]
fn detectors_do_not_spawn_directly() {
    let sources = [
        ("mod.rs", include_str!("mod.rs")),
        ("file.rs", include_str!("file.rs")),
        ("npm.rs", include_str!("npm.rs")),
        ("make.rs", include_str!("make.rs")),
        ("just.rs", include_str!("just.rs")),
        ("compose.rs", include_str!("compose.rs")),
        ("watch.rs", include_str!("watch.rs")),
        // 프로세스를 띄우는 감지기(M6-05)도 `DetectExec` 로만 띄운다 — 띄우는 곳은 `exec.rs` 하나다.
        ("mise.rs", include_str!("mise.rs")),
        ("gradle.rs", include_str!("gradle.rs")),
        ("maven.rs", include_str!("maven.rs")),
        ("cache.rs", include_str!("cache.rs")),
        ("refresh.rs", include_str!("refresh.rs")),
    ];
    for (name, text) in sources {
        // 주석(머리 주석이 「`std::process` 를 부르지 않는다」 고 적는다)은 보지 않는다.
        let code: String = text
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        for banned in [
            "std::process",
            "process::Command",
            "Command::new",
            "tokio::process",
            "pw_spawn",
            "portable_pty",
        ] {
            assert!(!code.contains(banned), "{name} uses {banned}");
        }
    }
}

// --- ④ package.json 을 고치면 run.config.list 의 감지가 바뀐다 · ⑤ 「구성으로 저장」 -----------------------------------

struct Host {
    _tmp: tempfile::TempDir,
    configs: Arc<RunConfigs>,
    project: String,
    root: PathBuf,
}

fn host() -> Host {
    let tmp = tempfile::tempdir().unwrap();
    let base = pw_core::registry::path::simplify(fs::canonicalize(tmp.path()).unwrap());
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
    assert!(
        !projects.is_trusted(&project.id),
        "the fixture project starts untrusted"
    );
    let configs = Arc::new(RunConfigs::new());
    configs.attach(HostParts { paths, projects });
    Host {
        _tmp: tmp,
        configs,
        project: project.id,
        root,
    }
}

async fn call(host: &Host, scope: ConnectionScope, method: &str, params: Value) -> Value {
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
            return r.outcome.unwrap();
        }
    }
}

fn detected_keys(list: &Value) -> Vec<String> {
    list["detected"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["key"].as_str().unwrap().to_owned())
        .collect()
}

/// 미신뢰 프로젝트에서도 파일만 읽는 감지는 보이고, `package.json` 을 고치면 다음 목록이 바뀐다(캐시하지 않는다).
#[tokio::test]
async fn editing_package_json_changes_the_listed_detection() {
    let h = host();
    let list_params = json!({"projectId": h.project});
    let scope = ConnectionScope::Project(h.project.clone());
    fs::write(
        h.root.join("package.json"),
        r#"{"scripts": {"dev": "vite"}}"#,
    )
    .unwrap();
    let list = call(&h, scope.clone(), "run.config.list", list_params.clone()).await;
    assert_eq!(detected_keys(&list), ["detected:npm:dev"]);
    assert_eq!(list["detected"][0]["detector"], "npm");
    assert_eq!(
        list["detected"][0]["config"],
        json!({"name": "dev", "type": "npm", "script": "dev", "cwd": null, "env": {}, "envFile": [], "profiles": {}, "debug": null, "before": []})
    );
    assert_eq!(list["detectProblems"], json!([]));

    fs::write(
        h.root.join("package.json"),
        r#"{"scripts": {"dev": "vite", "lint": "oxlint"}}"#,
    )
    .unwrap();
    let list = call(&h, scope.clone(), "run.config.list", list_params.clone()).await;
    assert_eq!(
        detected_keys(&list),
        ["detected:npm:dev", "detected:npm:lint"]
    );

    fs::write(h.root.join("package.json"), "{ not json").unwrap();
    let list = call(&h, scope, "run.config.list", list_params).await;
    assert_eq!(detected_keys(&list), Vec::<String>::new());
    assert_eq!(
        list["detectProblems"],
        json!([{"detector": "npm", "reason": "package.json: not valid JSON at line 1, column 3"}])
    );
}

/// 파일 감시: 감지 출처가 바뀌면 그 프로젝트만 다시 감지하고, 달라졌을 때만 `config.changed { changed: ["run/detected"] }` 를 낸다.
#[tokio::test]
async fn a_changed_source_file_announces_the_project() {
    let h = host();
    let bus = EventBus::new(DEFAULT_CAPACITY);
    let mut announced = bus.subscribe(Filter::all().topics([pw_core::config::TOPIC_CHANGED]));
    tokio::spawn(follow_files(
        subscribe_files(&bus),
        h.configs.seen(),
        bus.clone(),
    ));
    fs::write(h.root.join("Makefile"), "build:\n\tcc\n").unwrap();
    call(
        &h,
        ConnectionScope::Project(h.project.clone()),
        "run.config.list",
        json!({"projectId": h.project}),
    )
    .await;
    let fs_changed = |path: &Path| json!({"root": h.root.display().to_string(), "worktree": null, "paths": [path.display().to_string()], "kinds": ["modified"], "rescan": false});
    // 감지 출처가 아닌 파일 · 바뀌지 않은 출처는 알리지 않는다.
    fs::write(h.root.join("README.md"), "x").unwrap();
    bus.publish(
        Scope::Project(h.project.clone()),
        "fs.changed",
        &fs_changed(&h.root.join("README.md")),
    )
    .unwrap();
    bus.publish(
        Scope::Project(h.project.clone()),
        "fs.changed",
        &fs_changed(&h.root.join("Makefile")),
    )
    .unwrap();
    // 바뀐 출처는 알린다.
    fs::write(h.root.join("Makefile"), "build:\n\tcc -O2\n").unwrap();
    bus.publish(
        Scope::Project(h.project.clone()),
        "fs.changed",
        &fs_changed(&h.root.join("Makefile")),
    )
    .unwrap();
    let received = tokio::time::timeout(Duration::from_secs(10), announced.recv())
        .await
        .expect("no config.changed")
        .unwrap();
    let Received::Event(event) = received else {
        panic!("{received:?}")
    };
    assert_eq!(event.scope, Scope::Project(h.project.clone()));
    assert_eq!(event.payload["changed"], json!([watch::CHANGED_KEY]));
    // 앞의 두 이벤트는 아무것도 내지 않았다 — 하나만 왔다.
    assert!(
        tokio::time::timeout(Duration::from_millis(200), announced.recv())
            .await
            .is_err()
    );
}

/// 「구성으로 저장」: 감지된 `config` 를 `run.config.save` 로 쓰고 다시 읽으면 같은 argv 다(감지기마다).
#[tokio::test]
async fn saving_a_detected_task_keeps_its_argv() {
    let h = host();
    all_fixtures(&h.root);
    let scope = ConnectionScope::Project(h.project.clone());
    let list = call(
        &h,
        scope.clone(),
        "run.config.list",
        json!({"projectId": h.project}),
    )
    .await;
    let login = vec!["/bin/sh".to_owned(), "-l".to_owned()];
    let cx = ArgvContext {
        root: &h.root,
        cwd: &h.root,
        login_shell: &login,
    };
    let detected = list["detected"].as_array().unwrap().clone();
    let mut seen_detectors = std::collections::BTreeSet::new();
    for (i, task) in detected.iter().enumerate() {
        let config: crate::config::RunConfig =
            serde_json::from_value(task["config"].clone()).unwrap();
        let id = format!("saved-{i}");
        call(
            &h,
            scope.clone(),
            "run.config.save",
            json!({"projectId": h.project, "id": id, "config": task["config"]}),
        )
        .await;
        let list = call(
            &h,
            scope.clone(),
            "run.config.list",
            json!({"projectId": h.project}),
        )
        .await;
        let saved = list["configs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["id"] == json!(id))
            .unwrap_or_else(|| panic!("{id} not listed: {list:#}"));
        let reread: crate::config::RunConfig =
            serde_json::from_value(saved["config"].clone()).unwrap();
        assert_eq!(
            argv(&reread, None, &cx).unwrap(),
            argv(&config, None, &cx).unwrap(),
            "{}",
            task["key"]
        );
        seen_detectors.insert(task["detector"].as_str().unwrap().to_owned());
    }
    assert_eq!(
        seen_detectors.into_iter().collect::<Vec<_>>(),
        ["compose", "just", "make", "npm"]
    );
}

// --- 셸 글 · 열쇠 -----------------------------------------------------------------------------------------------

/// 셸 종류로 만든 `make '<대상>'` 을 실제 POSIX 셸이 나누면 이름이 그대로 한 인자다(글 잇기가 아니라 따옴표 규칙).
/// 안전하지 않은 이름은 감지에서 빠진다.
#[cfg(unix)]
#[test]
fn shell_commands_quote_the_name_as_one_argument() {
    for name in ["build", "a.b", "x+y", "dir/sub", "a-b_c", "_x", "9lives"] {
        assert!(shell_safe_name(name), "{name}");
        let command = shell_command("printf '%s\\n'", name);
        let out = std::process::Command::new("/bin/sh")
            .args(["-c", &command])
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8(out.stdout).unwrap(),
            format!("{name}\n"),
            "{command}"
        );
    }
    for name in [
        "a b", "a;rm", "$(x)", "`x`", "a'b", "a\"b", "-rf", ".hidden", "", "a\\b", "a*", "~x",
        "a\nb",
    ] {
        assert!(!shell_safe_name(name), "{name:?}");
    }
    let parsed = just::parse("ok:\n  true\nfo;o:\n  true\n");
    assert_eq!(parsed.recipes.iter().collect::<Vec<_>>(), ["ok"]);
    assert_eq!(parsed.unsafe_lines, [3]);
}

#[test]
fn keys_split_at_the_detector() {
    assert_eq!(key("npm", "build:prod"), "detected:npm:build:prod");
    assert_eq!(
        parse_key("detected:npm:build:prod"),
        Some(("npm", "build:prod"))
    );
    assert_eq!(parse_key("dev"), None);
    let (_tmp, root) = root();
    fs::write(root.join("package.json"), PACKAGE_JSON).unwrap();
    let detection = detect(&root, &NoExec);
    let task = detection.get("detected:npm:build:prod").unwrap();
    assert_eq!(task.name, "build:prod");
}

// --- 정의 본문(M6-11 이 해시한다) -------------------------------------------------------------------------------

/// 정의 본문은 그 태스크를 정의한 본문만큼 바뀐다 — 이름 · 구성만 보면 실패한다.
#[test]
fn definitions_follow_the_body() {
    let (_tmp, root) = root();
    let def = |root: &Path, key: &str| detect(root, &NoExec).get(key).unwrap().definition.clone();

    fs::write(
        root.join("package.json"),
        r#"{"scripts": {"dev": "vite", "postdev": "a"}}"#,
    )
    .unwrap();
    let before = def(&root, "detected:npm:dev");
    fs::write(
        root.join("package.json"),
        r#"{"scripts": {"dev": "vite", "postdev": "b"}}"#,
    )
    .unwrap();
    assert_ne!(def(&root, "detected:npm:dev"), before, "post script");

    // make · just 는 파일 전체 — 그 태스크의 레시피 밖(선행 대상 · `SHELL` · `set shell`)을 고쳐도 바뀐다(리뷰 「고쳐야 하는 것」 2).
    for (file, key, edits) in [
        (
            "justfile",
            "detected:just:run",
            [
                "prep:\n    echo a\nrun: prep\n    echo run\n",
                "prep:\n    echo EVIL\nrun: prep\n    echo run\n",
                "set shell := [\"sh\", \"-c\"]\nprep:\n    echo EVIL\nrun: prep\n    echo run\n",
            ],
        ),
        (
            "Makefile",
            "detected:make:run",
            [
                "prep:\n\techo a\nrun: prep\n\techo run\n",
                "prep:\n\techo EVIL\nrun: prep\n\techo run\n",
                "SHELL := /tmp/evil\nprep:\n\techo EVIL\nrun: prep\n\techo run\n",
            ],
        ),
    ] {
        let mut seen = std::collections::BTreeSet::new();
        for text in edits {
            fs::write(root.join(file), text).unwrap();
            let definition = def(&root, key);
            assert!(definition.ends_with(" run"), "{definition}");
            assert!(
                seen.insert(definition),
                "{file}: {text:?} kept the definition"
            );
        }
    }

    fs::write(
        root.join("compose.yaml"),
        "services:\n  web:\n    command: a\n",
    )
    .unwrap();
    let before = def(&root, "detected:compose:web");
    fs::write(
        root.join("compose.yaml"),
        "# 주석\nservices:\n  web:\n    command:   a   # x\n",
    )
    .unwrap();
    assert_eq!(
        def(&root, "detected:compose:web"),
        before,
        "comments and spacing do not count"
    );
    fs::write(
        root.join("compose.yaml"),
        "x: &c b\nservices:\n  web:\n    command: *c\n",
    )
    .unwrap();
    assert_ne!(
        def(&root, "detected:compose:web"),
        before,
        "an alias is expanded"
    );
}

/// `npm` 종류 구성의 정의 본문은 작업 폴더에서 위로 처음 만나는 `package.json` 의 것이고, 감지된 태스크와 같은 꼴이다.
#[test]
fn npm_config_definition_matches_detection() {
    let (_tmp, root) = root();
    fs::write(root.join("package.json"), PACKAGE_JSON).unwrap();
    fs::create_dir_all(root.join("packages/app/src")).unwrap();
    fs::write(
        root.join("packages/app/package.json"),
        r#"{"scripts": {"dev": "next dev"}}"#,
    )
    .unwrap();
    let detected = detect(&root, &NoExec)
        .get("detected:npm:dev")
        .unwrap()
        .definition
        .clone();
    assert_eq!(
        npm::script_definition(&root, &root, "dev").unwrap(),
        Some(detected)
    );
    assert_eq!(
        npm::script_definition(&root, &root.join("packages/app/src"), "dev").unwrap(),
        Some(r#"{"dev":"next dev"}"#.to_owned())
    );
    assert_eq!(npm::script_definition(&root, &root, "nope").unwrap(), None);
    let (_other, outside) = self::root();
    assert!(npm::script_definition(&root, &outside, "dev").is_err());
}

// --- 읽기의 경계 ------------------------------------------------------------------------------------------------

/// 루트 밖을 가리키는 링크는 읽지 않고 그 감지기만 빈다(까닭에 내용이 없다). 루트 안을 가리키는 링크는 읽는다.
#[cfg(unix)]
#[test]
fn a_link_outside_the_root_is_not_read() {
    let (_tmp, root) = root();
    let secret = root.parent().unwrap().join("secret.json");
    fs::write(&secret, r#"{"scripts": {"leak": "TOKEN=hunter2"}}"#).unwrap();
    std::os::unix::fs::symlink(&secret, root.join("package.json")).unwrap();
    fs::write(root.join("Makefile"), "build:\n\tcc\n").unwrap();
    let detection = detect(&root, &NoExec);
    assert_eq!(
        detection
            .tasks
            .iter()
            .map(DetectedTask::key)
            .collect::<Vec<_>>(),
        ["detected:make:build"]
    );
    assert_eq!(detection.problems.len(), 1, "{detection:#?}");
    assert_eq!(detection.problems[0].detector, "npm");
    assert!(
        detection.problems[0].reason.contains("outside the project"),
        "{detection:#?}"
    );
    assert!(!detection.problems[0].reason.contains("hunter2"));

    fs::remove_file(root.join("package.json")).unwrap();
    fs::create_dir_all(root.join("config")).unwrap();
    fs::write(
        root.join("config/pkg.json"),
        r#"{"scripts": {"dev": "vite"}}"#,
    )
    .unwrap();
    std::os::unix::fs::symlink("config/pkg.json", root.join("package.json")).unwrap();
    assert!(detect(&root, &NoExec).get("detected:npm:dev").is_some());
}

/// 1 MiB 를 넘는 파일 · 정규 파일이 아닌 것(FIFO — 열면 멈춘다) · UTF-8 이 아닌 것은 읽지 않고 까닭이다.
#[cfg(unix)]
#[test]
fn oversized_and_odd_files_are_refused() {
    let (_tmp, root) = root();
    let mut big = String::from(r#"{"scripts": {"dev": "vite"}, "pad": ""#);
    big.push_str(&"x".repeat(file::MAX_BYTES as usize));
    big.push_str("\"}");
    fs::write(root.join("package.json"), big).unwrap();
    let fifo = root.join("Makefile");
    let c = std::ffi::CString::new(fifo.display().to_string()).unwrap();
    // SAFETY: 경로는 NUL 로 끝나는 C 문자열이다.
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
    fs::write(root.join("justfile"), [0xff, 0xfe, b'\n']).unwrap();
    // 회귀하면(FIFO 를 열어 기다리면) 멈추지 않고 시한에서 실패한다.
    let detection = within(Duration::from_secs(10), move || detect(&root, &NoExec));
    assert!(detection.tasks.is_empty(), "{detection:#?}");
    let reasons: Vec<(&str, &str)> = detection
        .problems
        .iter()
        .map(|p| (p.detector.as_str(), p.reason.as_str()))
        .collect();
    assert_eq!(
        reasons,
        [
            (
                "npm",
                "package.json is larger than 1024 KiB; Pitwall does not read a file that large for task detection"
            ),
            ("make", "Makefile is not a regular file"),
            ("just", "justfile is not UTF-8 text"),
        ]
    );
}

/// 큰 `package.json`(상한 아래): 태스크는 [`MAX_TASKS`] 까지만 두고 까닭을 남긴다.
#[test]
fn too_many_tasks_are_capped() {
    let (_tmp, root) = root();
    let scripts: serde_json::Map<String, Value> = (0..MAX_TASKS + 20)
        .map(|i| (format!("s{i:04}"), json!("true")))
        .collect();
    fs::write(
        root.join("package.json"),
        json!({"scripts": scripts}).to_string(),
    )
    .unwrap();
    let detection = detect(&root, &NoExec);
    assert_eq!(detection.tasks.len(), MAX_TASKS);
    assert_eq!(detection.tasks[0].name, "s0000");
    assert_eq!(
        detection.problems,
        [DetectProblem {
            detector: "npm".into(),
            reason: format!(
                "package.json: found {} tasks; only the first {MAX_TASKS} by name are listed",
                MAX_TASKS + 20
            ),
        }]
    );
}

/// 이상한 꼴: `scripts` 가 객체가 아님 · compose 의 top 이 목록 · 잘못된 YAML · 둘인 justfile.
#[test]
fn odd_shapes_empty_only_that_detector() {
    let (_tmp, root) = root();
    fs::write(root.join("package.json"), r#"{"scripts": ["dev"]}"#).unwrap();
    fs::write(root.join("compose.yaml"), "- a\n- b\n").unwrap();
    fs::write(root.join("Makefile"), "ok:\n\ttrue\n").unwrap();
    let detection = detect(&root, &NoExec);
    assert_eq!(
        detection
            .tasks
            .iter()
            .map(DetectedTask::key)
            .collect::<Vec<_>>(),
        ["detected:make:ok"]
    );
    assert_eq!(
        detection.problems,
        [
            DetectProblem {
                detector: "npm".into(),
                reason: "package.json: `scripts` is not an object".into()
            },
            DetectProblem {
                detector: "compose".into(),
                reason: "compose.yaml: the top level is not a mapping".into()
            },
        ]
    );
    fs::write(root.join("compose.yaml"), "services:\n  web: [\n").unwrap();
    let problems = detect(&root, &NoExec).problems;
    assert!(
        problems[1]
            .reason
            .starts_with("compose.yaml: not valid YAML at line"),
        "{problems:?}"
    );
    fs::write(root.join("justfile"), "a:\n  true\n").unwrap();
    fs::write(root.join(".justfile"), "b:\n  true\n").unwrap();
    let problems = detect(&root, &NoExec).problems;
    assert!(
        problems
            .iter()
            .any(|p| p.detector == "just" && p.reason.contains("more than one justfile")),
        "{problems:?}"
    );
}

/// 별칭을 겹친 compose 파일(billion laughs)은 펼친 길이 상한에서 멈추고, 스스로를 가리키는 별칭은 깊이 상한에서 멈춘다.
#[test]
fn compose_alias_bombs_are_bounded() {
    let mut bomb = String::from(
        "a: &a [\"lol\",\"lol\",\"lol\",\"lol\",\"lol\",\"lol\",\"lol\",\"lol\",\"lol\"]\n",
    );
    let mut previous = 'a';
    for name in 'b'..='j' {
        bomb.push_str(&format!(
            "{name}: &{name} [*{previous},*{previous},*{previous},*{previous},*{previous},*{previous},*{previous},*{previous},*{previous}]\n"
        ));
        previous = name;
    }
    bomb.push_str("services:\n  web:\n    command: *j\n");
    // 회귀하면(상한을 걷으면) 멈추지 않고 시한에서 실패한다.
    let err = within(Duration::from_secs(5), move || {
        compose::services(&bomb, "compose.yaml", &mut Vec::new()).unwrap_err()
    });
    assert!(err.contains("expands to more than 256 KiB"), "{err}");

    let err = within(Duration::from_secs(5), || {
        compose::services(
            "services:\n  web: &w\n    again: *w\n",
            "compose.yaml",
            &mut Vec::new(),
        )
        .unwrap_err()
    });
    assert!(err.contains("nested deeper than"), "{err}");
}

/// `f` 를 스레드에서 돌려 `limit` 안에 끝나지 않으면 실패한다 — 회귀가 「멈춤」 이 아니라 「실패」 로 드러나게.
fn within<T: Send + 'static>(limit: Duration, f: impl FnOnce() -> T + Send + 'static) -> T {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    rx.recv_timeout(limit)
        .unwrap_or_else(|e| panic!("did not finish within {limit:?}: {e}"))
}

/// 감지 하나를 시한 안에 돌리고 정의 본문 합계를 본다.
fn bounded_detect(root: PathBuf) -> (Detection, usize, Duration) {
    let started = std::time::Instant::now();
    let detection = within(Duration::from_secs(5), move || detect(&root, &NoExec));
    let elapsed = started.elapsed();
    let total = detection.tasks.iter().map(|t| t.definition.len()).sum();
    (detection, total, elapsed)
}

/// 증폭 막기(리뷰 「막는 것」 1): 대상 4000 개를 한 규칙 줄에 적고 250 KB 레시피를 단 273 KB `Makefile` — 정의를 대상마다
/// 복사하면 1 GB 를 넘었다. 고른 500 개의 정의만, 합계 수 MB 안 · 2 초 안.
#[test]
fn many_targets_on_one_rule_do_not_multiply_the_recipe() {
    let (_tmp, root) = root();
    let targets: Vec<String> = (0..4000).map(|i| format!("t{i:04}")).collect();
    let mut text = format!("{}:\n", targets.join(" "));
    for _ in 0..2500 {
        text.push_str(&format!("\techo {}\n", "x".repeat(94)));
    }
    assert!(text.len() > 270_000, "{}", text.len());
    fs::write(root.join("Makefile"), &text).unwrap();
    let (detection, total, elapsed) = bounded_detect(root.clone());
    assert_eq!(detection.tasks.len(), MAX_TASKS);
    assert!(total < 64 * 1024, "definitions add up to {total} bytes");
    assert!(elapsed < Duration::from_secs(2), "{elapsed:?}");
    assert_eq!(
        detection.problems,
        [DetectProblem {
            detector: "make".into(),
            reason: format!(
                "Makefile: found 4000 tasks; only the first {MAX_TASKS} by name are listed"
            ),
        }]
    );

    // 같은 대상을 한 줄에 2000 번 적은 104 KB 파일은 대상 하나다.
    let mut text = format!("{}:\n", ["a"; 2000].join(" "));
    text.push_str(&format!("\techo {}\n", "x".repeat(100_000)));
    fs::write(root.join("Makefile"), &text).unwrap();
    let (detection, total, _) = bounded_detect(root);
    assert_eq!(detection.tasks.len(), 1);
    assert!(total < 1024, "{total}");
}

/// 증폭 막기: 250 KB 앵커 하나를 서비스 3000 개가 가리키는 285 KB compose 파일 — 서비스마다 펼치면 800 MB · 15 초였다. 합계
/// 상한([`MAX_DEFINITION_BYTES`])에 닿는 순간 멈추고 감지기를 비운다.
#[test]
fn many_services_on_one_anchor_stop_at_the_budget() {
    let (_tmp, root) = root();
    let mut text = format!("x-big: &big \"{}\"\nservices:\n", "y".repeat(250_000));
    for i in 0..3000 {
        text.push_str(&format!("  s{i:04}: {{command: *big}}\n"));
    }
    assert!(text.len() > 280_000, "{}", text.len());
    fs::write(root.join("compose.yaml"), &text).unwrap();
    let (detection, total, elapsed) = bounded_detect(root);
    assert!(
        detection.tasks.is_empty(),
        "{} tasks",
        detection.tasks.len()
    );
    assert_eq!(total, 0);
    assert!(elapsed < Duration::from_secs(2), "{elapsed:?}");
    assert_eq!(
        detection.problems,
        [DetectProblem {
            detector: "compose".into(),
            reason: format!("compose.yaml: {}", over_budget()),
        }]
    );
}

/// 이름 길이 상한: npm · compose 이름도 [`MAX_NAME_LEN`] 을 넘으면 빼고 까닭을 남긴다.
#[test]
fn overlong_names_are_left_out() {
    let (_tmp, root) = root();
    let long = "n".repeat(MAX_NAME_LEN + 1);
    fs::write(
        root.join("package.json"),
        json!({"scripts": {"dev": "vite", long.clone(): "x"}}).to_string(),
    )
    .unwrap();
    let detection = detect(&root, &NoExec);
    assert_eq!(
        detection
            .tasks
            .iter()
            .map(DetectedTask::key)
            .collect::<Vec<_>>(),
        ["detected:npm:dev"]
    );
    assert_eq!(
        detection.problems,
        [DetectProblem {
            detector: "npm".into(),
            reason: format!(
                "package.json: skipped 1 task(s) whose names are longer than {MAX_NAME_LEN} bytes"
            ),
        }]
    );
}
