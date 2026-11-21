//! 가벼움 재기(18-m7-plan M7-04 「가벼움」 — 메인 판정 M7-P13 의 출발값과 견준다, 판정은 M7-26). 손으로 돌리는 시험이다:
//!
//! ```sh
//! cargo build --release -p pitwall-relay
//! PITWALL_RELAY_BIN=target/release/pitwall-relay \
//!   cargo nextest run -p pitwall-relay --run-ignored only --no-capture -E 'test(measure_the_relay_footprint)'
//! ```
//!
//! 데몬을 임시 `PITWALL_*_DIR` 로 띄워(이 기기 — SSH 없이) 기동 → 첫 응답(`status`), 유휴 RSS(프로젝트 없음), 링크를 붙여 프로젝트
//! 하나를 등록하고 그 창이 파일 목록 · git 상태를 읽은 뒤의 RSS(언어 서버 없이)를 잰다.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use pw_core::Host;
use pw_link::{
    DesktopIdentity, DeviceKey, Expect, LinkLimits, LinkStream, Pairing, RemoteHost, Transport,
};
use pw_rpc::{Caller, ConnectionScope, Role};
use serde_json::{Value, json};

fn bin() -> PathBuf {
    std::env::var_os("PITWALL_RELAY_BIN")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_pitwall-relay")))
}

fn with_dirs(command: &mut Command, root: &Path) {
    for (var, leaf) in [
        ("PITWALL_CONFIG_DIR", "config"),
        ("PITWALL_DATA_DIR", "data"),
        ("PITWALL_STATE_DIR", "state"),
        ("PITWALL_CACHE_DIR", "cache"),
    ] {
        command.env(var, root.join(leaf));
    }
}

fn rss_kib(pid: u32) -> u64 {
    let out = Command::new("ps")
        .args(["-o", "rss=", "-p", &pid.to_string()])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().parse().unwrap()
}

struct UnixTransport(PathBuf);

impl Transport for UnixTransport {
    fn describe(&self) -> String {
        "unix".into()
    }

    fn open(&self) -> pw_core::BoxFuture<'_, std::io::Result<LinkStream>> {
        Box::pin(async move {
            let stream = tokio::net::UnixStream::connect(&self.0).await?;
            Ok(LinkStream::split(stream))
        })
    }
}

async fn call(conn: &mut pw_core::Connection, id: i64, method: &str, params: Value) -> Value {
    conn.sink
        .send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
    loop {
        let frame = tokio::time::timeout(Duration::from_secs(60), conn.source.recv())
            .await
            .expect("no reply")
            .expect("closed");
        if frame["id"] == json!(id) {
            assert!(frame.get("error").is_none(), "{method}: {frame}");
            return frame["result"].clone();
        }
    }
}

#[test]
#[ignore = "manual measurement — see the module docs"]
fn measure_the_relay_footprint() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    // 프로젝트로 쓸 폴더: 이 저장소의 crates/pw-relay 를 복사하지 않고 그대로 가리킨다(읽기만 한다).
    let project = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../crates/pw-relay")
        .canonicalize()
        .unwrap();
    std::fs::create_dir_all(root.join("config/relay")).unwrap();
    std::fs::write(
        root.join("config/relay/relay.toml"),
        format!(
            "schema = 1\nroots = [{:?}]\n",
            project.display().to_string()
        ),
    )
    .unwrap();
    let desktop_key = Arc::new(DeviceKey::generate().unwrap());
    let data = root.join("data/relay");
    pw_link::pairing::save(
        &data.join(pw_link::pairing::PAIRINGS_FILE),
        &[Pairing {
            key: desktop_key.public(),
            role: Role::Owner,
            name: "measure".into(),
        }],
    )
    .unwrap();

    let started = Instant::now();
    let mut command = Command::new(bin());
    command
        .arg("daemon")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit());
    with_dirs(&mut command, &root);
    let mut daemon = command.spawn().unwrap();
    let pid = daemon.id();
    let first_answer = loop {
        let mut status = Command::new(bin());
        status
            .arg("status")
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        with_dirs(&mut status, &root);
        if status.output().unwrap().status.success() {
            break started.elapsed();
        }
        assert!(started.elapsed() < Duration::from_secs(10), "no answer");
        std::thread::sleep(Duration::from_millis(5));
    };
    std::thread::sleep(Duration::from_secs(3));
    let idle = rss_kib(pid);

    let socket = root
        .join("state/relay")
        .join(format!("{}.sock", env!("CARGO_PKG_VERSION")));
    let rt = tokio::runtime::Runtime::new().unwrap();
    let with_project = rt.block_on(async {
        let remote = RemoteHost::new(
            "measure",
            Arc::new(UnixTransport(socket)) as Arc<dyn Transport>,
            Arc::new(DesktopIdentity {
                key: desktop_key,
                versions: pw_link::Versions::new(
                    env!("CARGO_PKG_VERSION"),
                    option_env!("PITWALL_RELEASE_CHANNEL").unwrap_or("dev"),
                ),
            }),
            Expect::FirstContact,
            LinkLimits::default(),
        );
        remote.attach().await.unwrap();
        let mut welcome = remote.connect(Caller::new(Role::Owner)).await.unwrap();
        let created = call(&mut welcome, 1, "project.create", json!({"root": project})).await;
        let id = created["id"].as_str().unwrap().to_owned();
        let mut window = remote
            .connect(Caller {
                role: Role::Owner,
                scope: ConnectionScope::Project(id.clone()),
                window: Some("project-1".into()),
            })
            .await
            .unwrap();
        call(
            &mut window,
            2,
            "search.files",
            json!({"projectId": id, "query": "rs"}),
        )
        .await;
        call(&mut window, 3, "git.status", json!({"projectId": id})).await;
        tokio::time::sleep(Duration::from_secs(3)).await;
        let rss = rss_kib(pid);
        drop(window);
        drop(welcome);
        remote.detach();
        rss
    });
    // SAFETY: 이 시험이 띄운 데몬이다.
    unsafe { libc::kill(pid as i32, libc::SIGTERM) };
    let _ = daemon.wait();
    let size = std::fs::metadata(bin()).unwrap().len();
    eprintln!("binary: {} ({} bytes)", bin().display(), size);
    eprintln!("daemon start -> first status answer: {first_answer:?}");
    eprintln!(
        "idle RSS, no project: {idle} KiB ({:.1} MiB)",
        idle as f64 / 1024.0
    );
    eprintln!(
        "RSS with one project (file list + git status, no language server): {with_project} KiB ({:.1} MiB)",
        with_project as f64 / 1024.0
    );
}
