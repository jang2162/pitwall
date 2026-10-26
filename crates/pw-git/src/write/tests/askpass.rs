//! askpass(④ 의 Host 쪽): 소켓 폴더 0700 · 소켓 0600 · 도우미 0700, 창의 답이 도우미에 닿는다, git 이 도우미를 질문과 함께 부른다,
//! 다른 인스턴스(다른 데이터 폴더)의 토큰과 끝난 쓰기의 토큰은 거절한다, 듣는 창이 없거나 묻는 도중 떠나면 기다리지 않는다, 비밀은
//! Task · 알림에 없다, 인증을 묻는 쓰기만 도우미를 받는다, 문장은 다듬어 보낸다, ssh 의 질문 종류가 창에 닿고 confirm 의 「예」 는
//! `yes` 다, 도우미가 먼저 끝나면 질문이 닫힌다. 진짜 `pitwall askpass` 를 거치는 것과 로그 파일은 데스크톱 시험(`apps/desktop/src-tauri/tests/askpass.rs`).

use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::super::askpass::{PROTOCOL, SOCKET_ENV, TOKEN_ENV, hint_of, request, unix};
use super::super::*;
use super::{subscribe_all, until, until_finished};
use crate::testing::TestRepo;
use crate::testing::write::{Window, WriteFixture, credential_remote};

const SECRET: &str = "pw-test-secret-6f1c";
const QUESTION: &str = "Password for 'https://someone@example.invalid': ";

/// 소켓을 `dir/run/x.sock` 에 열고(실행 파일은 `exe`) 열릴 때까지 기다린다. 돌려준 핸들을 버리면 닫는다.
async fn serve(askpass: &Askpass, dir: &Path, exe: &Path) -> tokio::task::JoinHandle<()> {
    let socket = dir.join("run").join("x.sock");
    let task = tokio::spawn(askpass.clone().serve(socket, exe.to_path_buf()));
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while askpass.socket().is_none() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the askpass socket did not open"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    task
}

fn env_value(env: &[(OsString, OsString)], name: &str) -> String {
    env.iter()
        .find(|(n, _)| n == name)
        .map(|(_, v)| v.to_string_lossy().into_owned())
        .unwrap_or_else(|| panic!("{name} is not in the git environment"))
}

/// 인증을 물을 수 있는 쓰기(fetch 등)의 요청.
fn auth_request(f: &WriteFixture) -> WriteRequest {
    WriteRequest {
        askpass: true,
        ..f.request("test git fetch")
    }
}

/// 쓰기 하나 안에서 그 쓰기의 소켓 · 토큰으로 도우미처럼 묻는다.
async fn ask_inside_a_write(f: &WriteFixture) -> (Result<String, String>, PathBuf, String) {
    ask_inside_a_write_with(f, QUESTION, AskpassHint::Secret).await
}

async fn ask_inside_a_write_with(
    f: &WriteFixture,
    question: &str,
    hint: AskpassHint,
) -> (Result<String, String>, PathBuf, String) {
    let question = question.to_owned();
    f.write
        .run(auth_request(f), move |ctx| async move {
            let env = ctx.git().env().to_vec();
            let (socket, token) = (env_value(&env, SOCKET_ENV), env_value(&env, TOKEN_ENV));
            let asked = {
                let (socket, token) = (socket.clone(), token.clone());
                tokio::task::spawn_blocking(move || {
                    request(Path::new(&socket), &token, &question, hint).map_err(|e| e.to_string())
                })
                .await
                .unwrap()
            };
            Ok((asked, PathBuf::from(socket), token))
        })
        .await
        .unwrap()
}

fn fixture(timeouts: Option<(Duration, Duration)>) -> (WriteFixture, Askpass) {
    let askpass = Askpass::default();
    let f = WriteFixture::with_options(TestRepo::with_commit(), true, askpass.clone(), timeouts);
    (f, askpass)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_socket_folder_socket_and_helper_are_private_to_the_user() {
    let (f, askpass) = fixture(None);
    let exe = PathBuf::from("/opt/Pitwall's app/pitwall");
    let server = serve(&askpass, &f.app, &exe).await;
    let socket = askpass.socket().unwrap();
    let mode = |p: &Path| fs::symlink_metadata(p).unwrap().mode() & 0o777;
    assert_eq!(mode(socket.parent().unwrap()), 0o700);
    assert_eq!(mode(&socket), 0o600);
    let helper = askpass.helper().unwrap();
    assert_eq!(mode(&helper), 0o700);
    assert_eq!(
        fs::read_to_string(&helper).unwrap(),
        "#!/bin/sh\nexec '/opt/Pitwall'\\''s app/pitwall' askpass \"$@\"\n"
    );
    unix::check_endpoint(&socket, unix::current_uid()).unwrap();

    server.abort();
    let _ = server.await;
    assert!(!socket.exists(), "closing removes the socket file");
    assert_eq!(askpass.socket(), None, "writes no longer get the helper");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_window_answer_reaches_the_helper_and_stays_out_of_tasks_and_notifications() {
    let (f, askpass) = fixture(None);
    let _server = serve(&askpass, &f.app, Path::new("/unused/pitwall")).await;
    let window = Window::open(&f.write, &f.project, |request| match request {
        GitPromptRequest::Askpass { .. } => Some(GitPromptAnswer::Askpass {
            value: SECRET.into(),
        }),
        GitPromptRequest::Unsaved { .. } => Some(GitPromptAnswer::Proceed),
    });
    let mut events = subscribe_all(&f.bus);
    let (asked, _, _) = ask_inside_a_write(&f).await;
    assert_eq!(asked.as_deref(), Ok(SECRET));
    assert_eq!(
        window.seen(),
        [GitPromptRequest::Askpass {
            text: QUESTION.into(),
            hint: AskpassHint::Secret,
        }]
    );
    let events = until_finished(&mut events).await;
    for (topic, payload) in &events {
        assert!(!payload.to_string().contains(SECRET), "{topic}: {payload}");
    }
}

/// git 이 원격 도우미의 `$GIT_ASKPASS` 로 도우미 스크립트를 주고, 스크립트가 `<실행 파일> askpass <질문>` 을 소켓 · 토큰 환경과 함께
/// 부른다. 실행 파일 자리에는 받은 것을 찍는 스크립트를 둔다(진짜 `pitwall` 은 데스크톱 시험).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn git_runs_the_helper_with_the_question_for_a_remote_that_asks() {
    let (f, askpass) = fixture(None);
    let exe = f.bin.join("fake-pitwall");
    fs::write(
        &exe,
        "#!/bin/sh\nprintf '%s|%s|%s|%s' \"$1\" \"$2\" \"$PITWALL_ASKPASS_SOCKET\" \"${#PITWALL_ASKPASS_TOKEN}\"\n",
    )
    .unwrap();
    fs::set_permissions(&exe, fs::Permissions::from_mode(0o755)).unwrap();
    let _server = serve(&askpass, &f.app, &exe).await;
    let out = f.repo.base().join("remote-out");
    credential_remote(&f.bin, &out, QUESTION);
    let result = f
        .write
        .run(auth_request(&f), |ctx| async move {
            ctx.output(ctx.git().args(["fetch", "testauth::example"]))
                .await
                .map(drop)
        })
        .await;
    assert_eq!(result, Ok(()));
    let socket = askpass.socket().unwrap();
    assert_eq!(
        fs::read_to_string(&out).unwrap(),
        format!("askpass|{QUESTION}|{}|32", socket.display())
    );
    assert_eq!(
        PathBuf::from(fs::read_to_string(out.with_extension("askpass")).unwrap()),
        askpass.helper().unwrap()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tokens_of_another_instance_or_of_a_finished_write_are_refused() {
    let (f, askpass) = fixture(None);
    let _server = serve(&askpass, &f.app, Path::new("/unused/pitwall")).await;
    let other_dir = tempfile::tempdir().unwrap();
    let other = Askpass::default();
    let _other_server = serve(&other, other_dir.path(), Path::new("/unused/pitwall")).await;
    assert_ne!(askpass.socket(), other.socket());
    let _window = Window::open(&f.write, &f.project, |_| {
        Some(GitPromptAnswer::Askpass {
            value: SECRET.into(),
        })
    });
    let other_socket = other.socket().unwrap();
    let crossed = f
        .write
        .run(auth_request(&f), move |ctx| async move {
            let token = env_value(ctx.git().env(), TOKEN_ENV);
            Ok(tokio::task::spawn_blocking(move || {
                request(&other_socket, &token, QUESTION, AskpassHint::Secret)
                    .map_err(|e| e.to_string())
            })
            .await
            .unwrap())
        })
        .await
        .unwrap();
    let error = crossed.unwrap_err();
    assert!(error.contains("does not belong"), "{error}");

    let (asked, socket, token) = ask_inside_a_write(&f).await;
    assert_eq!(asked.as_deref(), Ok(SECRET));
    assert_eq!(
        askpass.sessions(),
        0,
        "the finished write dropped its token"
    );
    let late = tokio::task::spawn_blocking(move || {
        request(&socket, &token, QUESTION, AskpassHint::Secret)
    })
    .await
    .unwrap();
    assert!(late.is_err(), "a token of a finished write is refused");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn without_a_window_or_with_cancel_the_helper_fails_without_waiting() {
    let (f, askpass) = fixture(Some((Duration::from_secs(3600), Duration::from_secs(3600))));
    let _server = serve(&askpass, &f.app, Path::new("/unused/pitwall")).await;
    let started = tokio::time::Instant::now();
    let (asked, _, _) = ask_inside_a_write(&f).await;
    let error = asked.unwrap_err();
    assert!(error.contains("no Pitwall window"), "{error}");
    assert!(
        started.elapsed() < Duration::from_secs(60),
        "it did not wait for the hour-long timeout"
    );

    let _window = Window::open(&f.write, &f.project, |_| Some(GitPromptAnswer::Cancel));
    let (asked, _, _) = ask_inside_a_write(&f).await;
    assert_eq!(asked.unwrap_err(), "cancelled");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_socket_served_by_another_app_is_not_taken_over_and_a_stale_one_is_replaced() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("run").join("x.sock");
    let uid = unix::current_uid();
    let first = unix::bind(&socket, uid).unwrap();
    let taken = unix::bind(&socket, uid);
    assert_eq!(
        taken.err().map(|e| e.kind()),
        Some(std::io::ErrorKind::AddrInUse)
    );
    drop(first);
    assert!(!socket.exists());
    // 받는 쪽 없는 소켓 파일(죽은 앱이 남긴 것) — std 의 리스너는 버려도 파일을 지우지 않는다.
    drop(std::os::unix::net::UnixListener::bind(&socket).unwrap());
    assert!(socket.exists());
    let replaced = unix::bind(&socket, uid).expect("a stale socket is replaced");
    drop(replaced);
    fs::write(&socket, "not a socket").unwrap();
    assert_eq!(
        unix::bind(&socket, uid).err().map(|e| e.kind()),
        Some(std::io::ErrorKind::PermissionDenied),
        "a file that is not a socket is left alone"
    );
    assert_eq!(fs::read_to_string(&socket).unwrap(), "not a socket");
    assert!(unix::peer_allowed(Ok(uid + 1), uid).is_err());
    assert!(unix::peer_allowed(Err(std::io::Error::other("x")), uid).is_err());
    assert!(unix::peer_allowed(Ok(uid), uid).is_ok());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_helper_refuses_a_socket_folder_open_to_others() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("run").join("x.sock");
    let _listener = unix::bind(&socket, unix::current_uid()).unwrap();
    fs::set_permissions(socket.parent().unwrap(), fs::Permissions::from_mode(0o755)).unwrap();
    let refused =
        tokio::task::spawn_blocking(move || request(&socket, "t", "q", AskpassHint::Secret))
            .await
            .unwrap();
    assert_eq!(
        refused.err().map(|e| e.kind()),
        Some(std::io::ErrorKind::PermissionDenied)
    );
}

/// 계속 · 중단 · 커밋 같은 쓰기는 도우미를 받지 않는다 — 그 훅이 `$GIT_ASKPASS` 로 진짜와 같은 암호 다이얼로그를 띄우지 못한다.
/// 인증을 묻는 쓰기(`askpass: true`)의 훅은 받는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn only_writes_that_may_authenticate_hand_the_helper_to_hooks() {
    let (f, askpass) = fixture(None);
    let _server = serve(&askpass, &f.app, Path::new("/unused/pitwall")).await;
    let seen = f.repo.base().join("hook-env");
    let hook = f.repo.root().join(".git/hooks/pre-commit");
    fs::write(
        &hook,
        format!(
            "#!/bin/sh\nprintf '%s|%s|%s|%s' \"$GIT_ASKPASS\" \"$SSH_ASKPASS\" \"$PITWALL_ASKPASS_SOCKET\" \"$PITWALL_ASKPASS_TOKEN\" > '{}'\n",
            seen.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    let commit = |request: WriteRequest| {
        f.write.run(request, |ctx| async move {
            ctx.run(ctx.git().args(["commit", "--allow-empty", "-q", "-m", "x"]))
                .await
                .map(drop)
        })
    };

    commit(f.request("test git commit")).await.unwrap();
    assert_eq!(
        fs::read_to_string(&seen).unwrap(),
        "|||",
        "no helper for the hook"
    );

    commit(auth_request(&f)).await.unwrap();
    let helper = askpass.helper().unwrap().display().to_string();
    let socket = askpass.socket().unwrap().display().to_string();
    let env = fs::read_to_string(&seen).unwrap();
    let parts: Vec<_> = env.split('|').collect();
    assert_eq!(parts[..3], [&helper, &helper, &socket], "{env}");
    assert_eq!(parts[3].len(), 32, "{env}");
}

/// 문장은 창에 보내기 전에 다듬는다(제어 문자 · 양방향 제어 문자 · 길이).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_window_gets_the_question_cleaned() {
    let (f, askpass) = fixture(None);
    let _server = serve(&askpass, &f.app, Path::new("/unused/pitwall")).await;
    let window = Window::open(&f.write, &f.project, |_| Some(GitPromptAnswer::Cancel));
    let raw = format!(
        "\u{1b}[2JPassword for 'https://\u{202E}moc.elpmaxe@x':\r {}",
        "z".repeat(600)
    );
    let (asked, _, _) = ask_inside_a_write_with(&f, &raw, AskpassHint::Secret).await;
    assert_eq!(asked.unwrap_err(), "cancelled");
    let [GitPromptRequest::Askpass { text, .. }] = &window.seen()[..] else {
        panic!("{:?}", window.seen());
    };
    assert_eq!(text, &askpass_text(&raw));
    assert!(
        text.starts_with("[2JPassword for 'https://moc.elpmaxe@x': zzz"),
        "{text}"
    );
    assert_eq!(text.chars().count(), prompt::MAX_PROMPT_CHARS + 1);
}

/// ssh 의 `SSH_ASKPASS_PROMPT` 가 창에 질문의 종류로 닿는다. confirm 에 「예」(`proceed`)면 도우미는 `yes` 를 쓰고, 「아니오」 면
/// 실패한다(ssh 는 거절로 본다).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ssh_prompt_kinds_reach_the_window_and_confirm_answers_yes() {
    assert_eq!(hint_of(None), AskpassHint::Secret);
    assert_eq!(hint_of(Some("confirm".as_ref())), AskpassHint::Confirm);
    assert_eq!(hint_of(Some("none".as_ref())), AskpassHint::Notice);
    assert_eq!(hint_of(Some("other".as_ref())), AskpassHint::Secret);

    let (f, askpass) = fixture(None);
    let _server = serve(&askpass, &f.app, Path::new("/unused/pitwall")).await;
    let window = Window::open(&f.write, &f.project, |request| match request {
        GitPromptRequest::Askpass {
            hint: AskpassHint::Confirm,
            text,
        } if text.contains("yes") => Some(GitPromptAnswer::Proceed),
        _ => Some(GitPromptAnswer::Cancel),
    });
    let (asked, _, _) =
        ask_inside_a_write_with(&f, "Allow use of key? say yes", AskpassHint::Confirm).await;
    assert_eq!(asked.as_deref(), Ok("yes"));
    let (asked, _, _) = ask_inside_a_write_with(&f, "Allow? no", AskpassHint::Confirm).await;
    assert_eq!(asked.unwrap_err(), "cancelled");
    let (asked, _, _) =
        ask_inside_a_write_with(&f, "Confirm user presence for key", AskpassHint::Notice).await;
    assert_eq!(asked.unwrap_err(), "cancelled");
    let hints: Vec<_> = window
        .seen()
        .into_iter()
        .map(|request| match request {
            GitPromptRequest::Askpass { hint, .. } => hint,
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(
        hints,
        [
            AskpassHint::Confirm,
            AskpassHint::Confirm,
            AskpassHint::Notice
        ]
    );
}

/// 도우미가 답을 받기 전에 끝나면(ssh 가 알림 도우미를 끝냄 등) 앱은 연결이 끊긴 것을 보고 그 질문을 닫는다 — 쓰기가 끝나기를
/// 기다리지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn when_the_helper_goes_away_its_question_closes() {
    use std::io::Write as _;

    let (f, askpass) = fixture(None);
    let _server = serve(&askpass, &f.app, Path::new("/unused/pitwall")).await;
    let window = Window::open(&f.write, &f.project, |_| None);
    let prompts = f.write.prompts().clone();
    f.write
        .run(auth_request(&f), move |ctx| async move {
            let env = ctx.git().env().to_vec();
            let (socket, token) = (env_value(&env, SOCKET_ENV), env_value(&env, TOKEN_ENV));
            let mut helper = std::os::unix::net::UnixStream::connect(&socket).unwrap();
            let line = serde_json::json!({
                "protocol": PROTOCOL,
                "token": token,
                "prompt": "Confirm user presence for key",
                "hint": "notice",
            });
            helper.write_all(format!("{line}\n").as_bytes()).unwrap();
            until("the window is asked", || prompts.pending() == 1).await;
            drop(helper);
            until("the question closes", || prompts.pending() == 0).await;
            Ok(())
        })
        .await
        .unwrap();
    assert_eq!(window.seen().len(), 1);
}

/// 묻는 도중 창이 닫히면 도우미는 곧 실패한다(시간 제한을 기다리지 않는다).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn when_the_window_leaves_while_asked_the_helper_fails() {
    let (f, askpass) = fixture(Some((Duration::from_secs(3600), Duration::from_secs(3600))));
    let _server = serve(&askpass, &f.app, Path::new("/unused/pitwall")).await;
    let window = Window::open(&f.write, &f.project, |_| None);
    let f = std::sync::Arc::new(f);
    let asking = tokio::spawn({
        let f = std::sync::Arc::clone(&f);
        async move { ask_inside_a_write(&f).await.0 }
    });
    until("the window is asked", || window.seen().len() == 1).await;
    drop(window);
    let error = tokio::time::timeout(Duration::from_secs(10), asking)
        .await
        .expect("the helper waited for the hour-long timeout")
        .unwrap()
        .unwrap_err();
    assert!(error.contains("closed"), "{error}");
}
