//! askpass(④ 의 Host 쪽): 소켓 폴더 0700 · 소켓 0600 · 도우미 0700, 창의 답이 도우미에 닿는다, git 이 도우미를 질문과 함께 부른다,
//! 다른 인스턴스(다른 데이터 폴더)의 토큰과 끝난 쓰기의 토큰은 거절한다, 듣는 창이 없으면 기다리지 않는다, 비밀은 Task · 알림에
//! 없다. 진짜 `pitwall askpass` 를 거치는 것과 로그 파일은 데스크톱 시험(`apps/desktop/src-tauri/tests/askpass.rs`).

use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::super::askpass::{SOCKET_ENV, TOKEN_ENV, request, unix};
use super::super::*;
use super::{subscribe_all, until_finished};
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

/// 쓰기 하나 안에서 그 쓰기의 소켓 · 토큰으로 도우미처럼 묻는다.
async fn ask_inside_a_write(f: &WriteFixture) -> (Result<String, String>, PathBuf, String) {
    f.write
        .run(f.request("test git write"), |ctx| async move {
            let env = ctx.git().env().to_vec();
            let (socket, token) = (env_value(&env, SOCKET_ENV), env_value(&env, TOKEN_ENV));
            let asked = {
                let (socket, token) = (socket.clone(), token.clone());
                tokio::task::spawn_blocking(move || {
                    request(Path::new(&socket), &token, QUESTION).map_err(|e| e.to_string())
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
            text: QUESTION.into()
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
        .run(f.request("test git write"), |ctx| async move {
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
        .run(f.request("test git write"), move |ctx| async move {
            let token = env_value(ctx.git().env(), TOKEN_ENV);
            Ok(tokio::task::spawn_blocking(move || {
                request(&other_socket, &token, QUESTION).map_err(|e| e.to_string())
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
    let late = tokio::task::spawn_blocking(move || request(&socket, &token, QUESTION))
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
    let refused = tokio::task::spawn_blocking(move || request(&socket, "t", "q"))
        .await
        .unwrap();
    assert_eq!(
        refused.err().map(|e| e.kind()),
        Some(std::io::ErrorKind::PermissionDenied)
    );
}
