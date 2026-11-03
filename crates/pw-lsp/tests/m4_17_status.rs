//! 상태 항목 · 설치 안내의 Host 쪽(15-m4-plan.md M4-17 — 신뢰 뒤 기동은 m4_17_trust.rs):
//! 문서 하나를 맡는 인스턴스를 고르고(`lsp.status.document`), 「이 프로젝트에서 끄기」 · 「묻지 않기」 가 개인 파일에만 쓰인다
//! (`lsp.server.setLocal`). 설치 뒤 기동(웹뷰가 `lsp.server.restart` 를 부른다)이 그사이 신뢰를 풀었으면 띄우지 않는다.

#![cfg(unix)]

mod common;

use std::fs;
use std::path::Path;
use std::time::Duration;

use pw_lsp::fake::Scenario;
use pw_lsp::methods::LspInstanceState;
use pw_lsp_servers::methods::LspServersListParams;
use pw_rpc::{ConnectionScope, code};
use serde_json::{Value, json};

use common::env::{Env, EnvOptions};
use common::{alive, eventually};

fn incremental() -> Scenario {
    Scenario {
        capabilities: json!({ "textDocumentSync": 2, "hoverProvider": true }),
        ..Scenario::default()
    }
}

fn untrusted() -> Env {
    Env::new(EnvOptions {
        trusted: false,
        ..EnvOptions::default()
    })
}

// ---- 문서를 맡는 인스턴스 ------------------------------------------------------------------------------------------------------

/// 언어(Linguist 이름 · 언어 id)를 서버로 짝짓는 규칙은 문서 열기와 같고, 인스턴스는 루트가 그 경로를 담는 것이다. 정의가 없는
/// 언어는 서버 목록이 빈다. 다른 프로젝트의 창은 permission denied.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_document_status_names_the_instance_serving_the_path() {
    let env = Env::new(EnvOptions::default());
    env.fake("rust-analyzer", incremental());
    env.file("a/Cargo.toml", "");
    env.file("b/Cargo.toml", "");
    let a = env.file("a/src/x.rs", "");
    let b = env.file("b/src/y.rs", "");
    let mut win = env.window();
    win.open(&env, &a, "Rust", "").await;
    env.wait_state("rust-analyzer", LspInstanceState::Ready)
        .await;
    let status = |path: &Path, language: &str| json!({ "projectId": env.id, "path": path, "language": language });
    let got = win
        .call("lsp.status.document", status(&a, "Rust"))
        .await
        .unwrap();
    assert_eq!(got["language"], "rust");
    assert_eq!(got["servers"], json!(["rust-analyzer"]));
    assert_eq!(got["instances"].as_array().unwrap().len(), 1, "{got}");
    assert!(
        got["instances"][0]["root"]
            .as_str()
            .unwrap()
            .ends_with("/a"),
        "{got}"
    );
    assert_eq!(got["instances"][0]["state"]["kind"], "ready");
    // b/ 는 아직 열지 않았다 — 맡는 인스턴스가 없다(a 의 인스턴스를 고르지 않는다).
    let got = win
        .call("lsp.status.document", status(&b, "rust"))
        .await
        .unwrap();
    assert_eq!(got["instances"], json!([]), "{got}");
    let got = win
        .call("lsp.status.document", status(&a, "Python"))
        .await
        .unwrap();
    assert_eq!(got["servers"], json!(["basedpyright", "ruff"]));
    let got = win
        .call("lsp.status.document", status(&a, "COBOL"))
        .await
        .unwrap();
    assert_eq!(got["servers"], json!([]));

    // 서버 정의마다의 Mason 패키지(설정 창 — 프로젝트 없이).
    let mut settings = env.connect(ConnectionScope::Host);
    let packages = settings
        .call("lsp.servers.packages", Value::Null)
        .await
        .unwrap();
    assert!(
        packages.as_array().unwrap().contains(
            &json!({ "server": "basedpyright", "mason": "basedpyright", "languages": ["python"] })
        ),
        "{packages}"
    );

    let mut other = env.connect(ConnectionScope::Project(env.other.clone()));
    let e = other
        .call("lsp.status.document", status(&a, "Rust"))
        .await
        .unwrap_err();
    assert_eq!(e.code, code::PERMISSION_DENIED);
}

// ---- 개인 파일에만 쓴다 --------------------------------------------------------------------------------------------------------

fn set_local(env: &Env, server: &str, key: &str, value: bool) -> Value {
    json!({ "projectId": env.id, "server": server, "key": key, "value": value })
}

/// 「이 프로젝트에서 끄기」: 개인 파일 `.pitwall/project.local.toml` 에 `[lsp.<id>] enabled = false` 만 쓰고, 커밋되는
/// `project.toml` 은 바이트까지 그대로, 떠 있던 서버는 끝나 `disabled` 가 된다. 「묻지 않기」 는 `install_prompt` 로 그 서버만.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn turning_a_server_off_writes_only_the_personal_file_and_ends_it() {
    let env = Env::new(EnvOptions::default());
    let shared = "schema = 1\n# 팀 설정\n[lsp.ruff]\nargs = [\"server\"]\n";
    env.file(".pitwall/project.toml", shared);
    env.fake("rust-analyzer", incremental());
    let file = env.file("src/main.rs", "");
    let mut win = env.window();
    win.open(&env, &file, "Rust", "").await;
    env.wait_state("rust-analyzer", LspInstanceState::Ready)
        .await;
    let pid = env.manager.pids(&env.id)[0];

    let wrote = win
        .call(
            "lsp.server.setLocal",
            set_local(&env, "rust-analyzer", "enabled", false),
        )
        .await
        .unwrap();
    let local = env.root.join(".pitwall/project.local.toml");
    assert_eq!(
        fs::canonicalize(wrote["file"].as_str().unwrap()).unwrap(),
        fs::canonicalize(&local).unwrap()
    );
    assert_eq!(
        fs::read_to_string(&local).unwrap(),
        "schema = 1\n\n[lsp.rust-analyzer]\nenabled = false\n"
    );
    assert_eq!(
        fs::read(env.root.join(".pitwall/project.toml")).unwrap(),
        shared.as_bytes()
    );
    env.wait_state("rust-analyzer", LspInstanceState::Disabled)
        .await;
    assert!(eventually(Duration::from_secs(2), || !alive(pid)).await);

    win.call(
        "lsp.server.setLocal",
        set_local(&env, "basedpyright", "installPrompt", false),
    )
    .await
    .unwrap();
    let list = env
        .servers
        .list(&LspServersListParams {
            project_id: env.id.clone(),
            worktree: None,
        })
        .unwrap();
    let prompt = |id: &str| {
        list.servers
            .iter()
            .find(|s| s.id == id)
            .unwrap()
            .install_prompt
    };
    assert!(!prompt("basedpyright"));
    assert!(prompt("ruff"));
    assert!(
        !list
            .servers
            .iter()
            .find(|s| s.id == "rust-analyzer")
            .unwrap()
            .enabled
    );
    assert_eq!(
        fs::read(env.root.join(".pitwall/project.toml")).unwrap(),
        shared.as_bytes()
    );
}

/// 모르는 서버 id(점 · 따옴표가 든 것 포함)는 invalid params 이고 파일을 만들지 않는다. 다른 프로젝트의 창 · 미신뢰 프로젝트는
/// 거절한다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn set_local_refuses_unknown_servers_other_windows_and_untrusted_projects() {
    let env = Env::new(EnvOptions::default());
    let local = env.root.join(".pitwall/project.local.toml");
    let mut win = env.window();
    for id in ["nope", "rust-analyzer.x", "a\"b", "lsp"] {
        let e = win
            .call("lsp.server.setLocal", set_local(&env, id, "enabled", false))
            .await
            .unwrap_err();
        assert_eq!(e.code, code::INVALID_PARAMS, "{id}: {e:?}");
    }
    // 키 · 값의 꼴도 둘만 받는다.
    for params in [
        json!({ "projectId": env.id, "server": "ruff", "key": "path", "value": false }),
        json!({ "projectId": env.id, "server": "ruff", "key": "enabled", "value": "no" }),
    ] {
        let e = win.call("lsp.server.setLocal", params).await.unwrap_err();
        assert_eq!(e.code, code::INVALID_PARAMS, "{e:?}");
    }
    assert!(!local.exists());

    let mut other = env.connect(ConnectionScope::Project(env.other.clone()));
    let e = other
        .call(
            "lsp.server.setLocal",
            set_local(&env, "ruff", "enabled", false),
        )
        .await
        .unwrap_err();
    assert_eq!(e.code, code::PERMISSION_DENIED);
    let mut host = env.connect(ConnectionScope::Host);
    let e = host
        .call(
            "lsp.server.setLocal",
            set_local(&env, "ruff", "enabled", false),
        )
        .await
        .unwrap_err();
    assert_eq!(e.code, code::PERMISSION_DENIED);
    assert!(!local.exists());

    let env = untrusted();
    let mut win = env.window();
    let e = win
        .call(
            "lsp.server.setLocal",
            set_local(&env, "ruff", "enabled", false),
        )
        .await
        .unwrap_err();
    assert_eq!(e.code, code::PERMISSION_DENIED, "{e:?}");
    assert!(!env.root.join(".pitwall/project.local.toml").exists());
}

/// `git` 을 `dir` 에서 돌리고 표준 출력을 돌려준다. 실패하면 시험 실패.
fn git(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.com",
            "-c",
            "init.defaultBranch=main",
        ])
        .args(args)
        .current_dir(dir)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

/// 리뷰 M1: `.pitwall/` 이 없던 git 저장소에서 「끄기」 를 눌러도 git 이 커밋할 새 개인 파일이 생기지 않는다 — 개인 파일을 처음
/// 만드는 쓰기가 `.pitwall/.gitignore` 에 `*.local.toml` 을 둔다(03 「하이브리드 정책 (D9)」). 이미 있는 `.gitignore` 는 줄만 더하고
/// 있던 줄은 그대로, 줄이 이미 있으면 다시 더하지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn turning_a_server_off_leaves_no_personal_file_for_git_to_commit() {
    let env = Env::new(EnvOptions::default());
    git(&env.root, &["init", "-q"]);
    let mut win = env.window();
    win.call(
        "lsp.server.setLocal",
        set_local(&env, "ruff", "enabled", false),
    )
    .await
    .unwrap();
    assert!(env.root.join(".pitwall/project.local.toml").is_file());
    let status = git(
        &env.root,
        &["status", "--porcelain", "--untracked-files=all"],
    );
    assert!(!status.contains("local.toml"), "{status}");
    assert!(status.contains(".pitwall/.gitignore"), "{status}");
    assert_eq!(
        fs::read_to_string(env.root.join(".pitwall/.gitignore")).unwrap(),
        "*.local.toml\n"
    );
    // 두 번째 쓰기는 파일이 있으므로 `.gitignore` 를 건드리지 않는다.
    win.call(
        "lsp.server.setLocal",
        set_local(&env, "ruff", "installPrompt", false),
    )
    .await
    .unwrap();
    assert_eq!(
        fs::read_to_string(env.root.join(".pitwall/.gitignore")).unwrap(),
        "*.local.toml\n"
    );

    // 사람이 둔 `.gitignore`(끝 줄바꿈 없음)는 줄만 더한다.
    let env = Env::new(EnvOptions::default());
    git(&env.root, &["init", "-q"]);
    env.file(".pitwall/.gitignore", "# 팀\n.backup/");
    let mut win = env.window();
    win.call(
        "lsp.server.setLocal",
        set_local(&env, "ruff", "enabled", false),
    )
    .await
    .unwrap();
    assert_eq!(
        fs::read_to_string(env.root.join(".pitwall/.gitignore")).unwrap(),
        "# 팀\n.backup/\n*.local.toml\n"
    );
    let status = git(
        &env.root,
        &["status", "--porcelain", "--untracked-files=all"],
    );
    assert!(!status.contains("local.toml"), "{status}");
}

/// 리뷰 M2: 저장소 안의 개인 파일은 링크를 따르지 않는다. ① `project.local.toml` 이 커밋되는 `project.toml` 을 가리키는 링크면
/// 거절하고 `project.toml` 은 바이트까지 그대로, 링크도 그대로다. ② `.pitwall` 이 프로젝트 밖 폴더를 가리키는 링크면 거절하고 밖에
/// 아무것도 만들지 않는다. 거절은 conflict 이고 어느 파일 · 왜를 글로 싣는다(창이 알림으로 보인다).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_personal_file_behind_a_link_is_refused() {
    let env = Env::new(EnvOptions::default());
    let shared = "schema = 1\n";
    env.file(".pitwall/project.toml", shared);
    let local = env.root.join(".pitwall/project.local.toml");
    std::os::unix::fs::symlink("project.toml", &local).unwrap();
    let mut win = env.window();
    let e = win
        .call(
            "lsp.server.setLocal",
            set_local(&env, "ruff", "enabled", false),
        )
        .await
        .unwrap_err();
    assert_eq!(e.code, code::CONFLICT, "{e:?}");
    assert!(
        e.message.contains("project.local.toml") && e.message.contains("symbolic link"),
        "{e:?}"
    );
    assert_eq!(
        fs::read(env.root.join(".pitwall/project.toml")).unwrap(),
        shared.as_bytes()
    );
    assert!(
        fs::symlink_metadata(&local)
            .unwrap()
            .file_type()
            .is_symlink()
    );

    let env = Env::new(EnvOptions::default());
    let outside = env.base.join("outside");
    fs::create_dir_all(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, env.root.join(".pitwall")).unwrap();
    let mut win = env.window();
    let e = win
        .call(
            "lsp.server.setLocal",
            set_local(&env, "ruff", "installPrompt", false),
        )
        .await
        .unwrap_err();
    assert_eq!(e.code, code::CONFLICT, "{e:?}");
    assert!(
        e.message.contains("project.local.toml") && e.message.contains("outside the project"),
        "{e:?}"
    );
    assert_eq!(fs::read_dir(&outside).unwrap().count(), 0);
}

/// 리뷰 L1: 메인 체크아웃 창에서 끄면 제 개인 파일이 없어 메인 것을 폴백으로 읽는 워크트리(`<메인>/.worktrees/w` — 이 저장소의
/// 꼴)의 인스턴스도 그 워크트리 키로 다시 지나 `disabled` 로 끝난다. 제 개인 파일이 있는 워크트리는 그 파일을 따른다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn turning_a_server_off_reaches_worktrees_reading_the_same_personal_file() {
    let env = Env::new(EnvOptions::default());
    env.fake("rust-analyzer", incremental());
    let main_file = env.file("src/main.rs", "");
    git(&env.root, &["init", "-q"]);
    git(&env.root, &["add", "src/main.rs"]);
    git(&env.root, &["commit", "-q", "-m", "init"]);
    git(&env.root, &["worktree", "add", "-q", ".worktrees/w"]);
    let top = env.root.join(".worktrees/w");
    let wt_file = top.join("src/main.rs");
    let mut win = env.window();
    win.open(&env, &main_file, "Rust", "").await;
    let mut wt = env.window();
    wt.call(
        "lsp.document.open",
        json!({
            "projectId": env.id,
            "worktree": top,
            "path": wt_file,
            "language": "Rust",
            "version": 0,
            "text": "",
        }),
    )
    .await
    .unwrap();
    let state_at = |root: &Path| {
        env.manager
            .status(&env.id)
            .instances
            .into_iter()
            .find(|i| i.server == "rust-analyzer" && Path::new(&i.root) == root)
            .map(|i| i.state)
    };
    let both = |want: LspInstanceState| {
        let want = Some(want);
        let (root, top) = (env.root.clone(), top.clone());
        let state_at = &state_at;
        async move {
            eventually(Duration::from_secs(10), || {
                state_at(&root) == want && state_at(&top) == want
            })
            .await
        }
    };
    assert!(both(LspInstanceState::Ready).await, "{:?}", env.states());

    win.call(
        "lsp.server.setLocal",
        set_local(&env, "rust-analyzer", "enabled", false),
    )
    .await
    .unwrap();
    assert!(!top.join(".pitwall/project.local.toml").exists());
    assert!(both(LspInstanceState::Disabled).await, "{:?}", env.states());

    // 워크트리에 제 개인 파일(끔)이 있으면 메인에서 켜도 워크트리는 꺼진 채다.
    fs::create_dir_all(top.join(".pitwall")).unwrap();
    fs::write(
        top.join(".pitwall/project.local.toml"),
        "schema = 1\n[lsp.rust-analyzer]\nenabled = false\n",
    )
    .unwrap();
    win.call(
        "lsp.server.setLocal",
        set_local(&env, "rust-analyzer", "enabled", true),
    )
    .await
    .unwrap();
    assert!(
        eventually(Duration::from_secs(10), || state_at(&env.root)
            == Some(LspInstanceState::Ready))
        .await,
        "{:?}",
        env.states()
    );
    assert_eq!(state_at(&top), Some(LspInstanceState::Disabled));
}

// ---- 설치 뒤 기동 ------------------------------------------------------------------------------------------------------------

/// 설치 뒤 기동은 웹뷰가 `lsp.server.restart` 를 부른다: 그사이 신뢰를 풀었으면 허가가 거절돼 `untrusted` 로 남고 띄우지 않는다.
/// 신뢰한 채면 뜬다. 「설치」 는 설정 경로를 가짜 서버로 두는 것으로 흉내 낸다(설치기의 시험은 pw-tools).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_restart_after_install_asks_for_the_grant_again() {
    // 신뢰 따르기를 끈다 — 신뢰를 주는 이벤트가 스스로 띄우지 않게(이 시험은 restart 길만 본다).
    let env = Env::new(EnvOptions {
        follow: false,
        ..EnvOptions::default()
    });
    let file = env.file("tool.py", "");
    let mut win = env.window();
    let opened = win.open(&env, &file, "Python", "").await;
    let first = &opened["instances"][0];
    assert_eq!(first["server"], "basedpyright", "{opened}");
    env.wait_state(
        "basedpyright",
        LspInstanceState::NotInstalled {
            mason: Some("basedpyright".into()),
        },
    )
    .await;
    let root = first["root"].as_str().unwrap().to_owned();
    let restart = json!({ "projectId": env.id, "server": "basedpyright", "root": root });

    env.projects.set_trust(&env.id, false).unwrap();
    env.fake("basedpyright", incremental());
    let got = win
        .call("lsp.server.restart", restart.clone())
        .await
        .unwrap();
    assert_eq!(got["state"]["kind"], "untrusted");
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(env.manager.spawn_count(), 0);

    env.projects.set_trust(&env.id, true).unwrap();
    win.call("lsp.server.restart", restart).await.unwrap();
    env.wait_state("basedpyright", LspInstanceState::Ready)
        .await;
    assert_eq!(env.manager.spawn_count(), 1);
}

// ---- 로그 ---------------------------------------------------------------------------------------------------------------------

/// 「로그 열기」: 그 인스턴스의 서버 로그(stderr 포함)를 글로 돌려준다. 다른 프로젝트의 창은 permission denied, 로그가 없는
/// 인스턴스는 빈 글.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_server_log_of_an_instance_is_read_as_text() {
    use pw_lsp::fake::Action;
    let env = Env::new(EnvOptions::default());
    env.fake(
        "rust-analyzer",
        incremental().on(
            "initialized",
            vec![Action::Stderr {
                text: "hello from the server 한글".into(),
            }],
        ),
    );
    let file = env.file("src/main.rs", "");
    let mut win = env.window();
    let opened = win.open(&env, &file, "Rust", "").await;
    env.wait_state("rust-analyzer", LspInstanceState::Ready)
        .await;
    let params = json!({
        "projectId": env.id,
        "server": "rust-analyzer",
        "root": opened["instances"][0]["root"],
    });
    let mut text = String::new();
    for _ in 0..50 {
        let log = win.call("lsp.server.log", params.clone()).await.unwrap();
        text = log["text"].as_str().unwrap().to_owned();
        if text.contains("hello from the server 한글") {
            assert!(log["file"].as_str().unwrap().ends_with(".log"), "{log}");
            assert_eq!(log["truncated"], false);
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(text.contains("hello from the server 한글"), "{text}");

    let mut other = env.connect(ConnectionScope::Project(env.other.clone()));
    let e = other
        .call("lsp.server.log", params.clone())
        .await
        .unwrap_err();
    assert_eq!(e.code, code::PERMISSION_DENIED);
    let none = win
        .call(
            "lsp.server.log",
            json!({ "projectId": env.id, "server": "ruff", "root": "/nowhere" }),
        )
        .await
        .unwrap();
    assert_eq!(
        none,
        json!({ "text": "", "truncated": false, "file": null })
    );
}
