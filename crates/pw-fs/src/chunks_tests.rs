//! 조각 읽기 · 쓰기(18-m7-plan M7-13b)를 실제 RPC 세션으로 본다 — 모음이 세션(연결)에 묶이는 것까지.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use pw_core::bus::{DEFAULT_CAPACITY, EventBus};
use pw_core::config::AppPaths;
use pw_core::registry::path::simplify;
use pw_core::registry::{NewProject, Registry};
use pw_rpc::{Caller, ConnectionScope, EventHub, Outbound, Role, Router, RpcError, Session, code};
use serde_json::{Value, json};
use tokio::sync::mpsc::UnboundedReceiver;

use crate::Fs;
use crate::limits::{CHUNK_WIRE_MAX, UPLOAD_MAX, UPLOADS_MAX, UPLOADS_PER_SESSION};
use crate::write::{Stage, leftovers};

/// 응답 봉투(`jsonrpc` · `id` · 경로 · 판정 · `version`)의 몫 — 조각 내용 밖의 바이트는 이보다 작다.
const ENVELOPE: usize = 64 * 1024;
const MIB: usize = 1024 * 1024;

struct Fixture {
    _tmp: tempfile::TempDir,
    fs: Arc<Fs>,
    router: Arc<Router>,
    id: String,
    root: PathBuf,
    other: String,
}

impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let base = simplify(fs::canonicalize(tmp.path()).unwrap());
        let registry = Registry::new(
            AppPaths::under(&base.join("app")),
            None,
            EventBus::new(DEFAULT_CAPACITY),
        );
        let fs = Arc::new(Fs::new());
        fs.attach(registry.clone());
        let mut router = Router::new();
        crate::methods::register(&mut router, Arc::clone(&fs));
        let project = |name: &str| {
            let root = base.join(name);
            fs::create_dir_all(&root).unwrap();
            let id = registry
                .create(NewProject {
                    root: root.clone(),
                    ..NewProject::default()
                })
                .unwrap()
                .id;
            (id, root)
        };
        let (id, root) = project("p1");
        let (other, _) = project("p2");
        Self {
            _tmp: tmp,
            fs,
            router: Arc::new(router),
            id,
            root,
            other,
        }
    }

    fn connect(&self, scope: ConnectionScope) -> Conn {
        let (session, rx) = Session::new(
            Arc::clone(&self.router),
            EventHub::new(),
            Caller {
                role: Role::Owner,
                scope,
                window: None,
            },
        );
        Conn {
            session,
            rx,
            next_id: 0,
        }
    }

    fn own(&self) -> Conn {
        self.connect(ConnectionScope::Project(self.id.clone()))
    }

    fn file(&self, name: &str, bytes: &[u8]) -> String {
        let path = self.root.join(name);
        fs::write(&path, bytes).unwrap();
        path.display().to_string()
    }
}

struct Conn {
    session: Session,
    rx: UnboundedReceiver<Outbound>,
    next_id: i64,
}

impl Conn {
    /// 응답과 그 직렬화한 바이트 수(와이어에 실리는 메시지 하나).
    async fn call_sized(
        &mut self,
        method: &str,
        params: Value,
    ) -> (Result<Value, RpcError>, usize) {
        self.next_id += 1;
        let id = self.next_id;
        self.session
            .handle_value(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        loop {
            let msg = tokio::time::timeout(Duration::from_secs(60), self.rx.recv())
                .await
                .expect("no response")
                .expect("closed");
            let bytes = msg.to_text().len();
            if let Some(response) = msg.into_response()
                && response.id == pw_rpc::Id::Number(id)
            {
                return (response.outcome, bytes);
            }
        }
    }

    async fn call(&mut self, method: &str, params: Value) -> Result<Value, RpcError> {
        self.call_sized(method, params).await.0
    }
}

/// 조각을 처음부터 끝까지 읽어 이어 붙인다. 응답 하나의 최대 바이트도 함께.
async fn read_all(conn: &mut Conn, project: &str, path: &str, max: usize) -> (Value, usize) {
    let mut offset = 0u64;
    let mut expected = Value::Null;
    let mut text = String::new();
    let mut data = String::new();
    let mut largest = 0;
    loop {
        let (r, bytes) = conn
            .call_sized(
                "fs.readChunk",
                json!({"projectId": project, "path": path, "offset": offset, "maxBytes": max,
                       "expectedVersion": expected}),
            )
            .await;
        let r = r.unwrap();
        largest = largest.max(bytes);
        assert_eq!(r["offset"], offset);
        match r["content"]["kind"].as_str().unwrap() {
            "text" => text.push_str(r["content"]["text"].as_str().unwrap()),
            _ => data.push_str(r["content"]["data"].as_str().unwrap()),
        }
        if expected.is_null() {
            expected = r["version"].clone();
        }
        assert_eq!(
            r["version"], expected,
            "every chunk is cut from the same file"
        );
        if r["eof"].as_bool().unwrap() {
            let content = if data.is_empty() && r["content"]["kind"] == "text" {
                json!({"kind": "text", "text": text})
            } else {
                json!({"kind": "base64", "data": data})
            };
            return (
                json!({"version": expected, "content": content, "judgment": r["judgment"],
                       "size": r["size"], "path": r["path"]}),
                largest,
            );
        }
        offset = r["next"].as_u64().unwrap();
    }
}

/// 제어 문자(JSON 에서 6바이트) 줄로 채운 `bytes` 바이트 — NUL 이 없어 텍스트로 판정된다.
fn control_text(bytes: usize) -> Vec<u8> {
    let line: Vec<u8> = [[1u8; 63].as_slice(), b"\n"].concat();
    line.repeat(bytes / 64)
}

/// 이어 붙인 조각이 통째 `fs.read` 와 같은 내용 · 판정 · `version` 이고, 응답 하나가 `maxBytes` + 봉투를 넘지 않는다 — 제어 문자 ·
/// 8 KiB 뒤 NUL · 따옴표 · 여러 바이트 글자 · BOM · 바이너리 모두.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chunks_reassemble_the_whole_read_within_the_wire_budget() {
    let f = Fixture::new();
    let mut conn = f.own();
    let mut nul_late = vec![b'a'; MIB];
    nul_late[20_000] = 0;
    let mut bom = b"\xEF\xBB\xBF".to_vec();
    bom.extend("한글 \"따옴표\" \\ 😀\n".repeat(40_000).as_bytes());
    let mut binary = vec![0u8; MIB + 1];
    for (i, b) in binary.iter_mut().enumerate() {
        *b = (i % 256) as u8;
    }
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("control.txt", control_text(MIB)),
        ("nul-late.txt", nul_late),
        ("bom.txt", bom),
        ("binary.bin", binary),
        ("empty.txt", Vec::new()),
    ];
    for (name, bytes) in cases {
        let path = f.file(name, &bytes);
        let whole = conn
            .call("fs.read", json!({"projectId": f.id, "path": path}))
            .await
            .unwrap();
        let max = 256 * 1024;
        let (joined, largest) = read_all(&mut conn, &f.id, &path, max).await;
        assert_eq!(joined["content"], whole["content"], "{name}");
        assert_eq!(joined["judgment"], whole["judgment"], "{name}");
        assert_eq!(joined["version"], whole["version"], "{name}");
        assert_eq!(joined["size"], whole["size"], "{name}");
        assert!(
            largest <= max + ENVELOPE,
            "{name}: {largest} bytes on the wire"
        );
    }
}

/// 가장 큰 조각(`CHUNK_WIRE_MAX`)도 링크 틀(64 MiB) 안이다 — 와이어로 6배가 되는 글을 넘치게 둔다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_largest_chunk_of_worst_case_text_fits_the_link_frame() {
    let f = Fixture::new();
    let mut conn = f.own();
    let mut bytes = control_text(4 * MIB);
    bytes[10_000] = 0; // 8 KiB 뒤의 NUL — 바이너리가 아니다
    let path = f.file("worst.txt", &bytes);
    let max = usize::try_from(CHUNK_WIRE_MAX).unwrap();
    let (joined, largest) = read_all(&mut conn, &f.id, &path, max).await;
    assert_eq!(joined["content"]["kind"], "text");
    assert!(largest <= max + ENVELOPE, "{largest}");
    assert!(largest < 64 * MIB);
    // 상한을 넘는 예산은 받지 않는다.
    let e = conn
        .call(
            "fs.readChunk",
            json!({"projectId": f.id, "path": path, "maxBytes": CHUNK_WIRE_MAX + 1}),
        )
        .await
        .unwrap_err();
    assert_eq!(e.code, code::INVALID_PARAMS);
}

/// 완료 기준 ①: 조각 사이에 파일이 바뀌면 `version` 을 주지 않는다 — 다음 조각이 `CONFLICT` 이고 내용이 없다. 처음부터 다시 읽으면
/// 새 파일의 `version` 이다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_file_changed_between_chunks_gives_no_version() {
    let f = Fixture::new();
    let mut conn = f.own();
    let path = f.file("a.txt", "x".repeat(10_000).as_bytes());
    let first = conn
        .call(
            "fs.readChunk",
            json!({"projectId": f.id, "path": path, "maxBytes": 4096}),
        )
        .await
        .unwrap();
    assert_eq!(first["eof"], false);
    let version = first["version"].clone();
    // 같은 크기로 바꾼다 — 크기 · 시각이 같아도 내용 해시가 가른다.
    fs::write(&path, "y".repeat(10_000)).unwrap();
    let e = conn
        .call(
            "fs.readChunk",
            json!({"projectId": f.id, "path": path, "offset": first["next"], "maxBytes": 4096,
                   "expectedVersion": version}),
        )
        .await
        .unwrap_err();
    assert_eq!(e.code, code::CONFLICT);
    let current = e.data.as_ref().unwrap()["current"].clone();
    assert_ne!(current, version);
    let (joined, _) = read_all(&mut conn, &f.id, &path, 4096).await;
    assert_eq!(joined["version"], current);
    assert_eq!(joined["content"]["text"], "y".repeat(10_000));
}

/// 텍스트 자리가 아닌 `offset`(글자 중간)은 받지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_offset_inside_a_character_is_refused() {
    let f = Fixture::new();
    let mut conn = f.own();
    let path = f.file("k.txt", "한글".as_bytes());
    let e = conn
        .call(
            "fs.readChunk",
            json!({"projectId": f.id, "path": path, "offset": 1, "maxBytes": 64}),
        )
        .await
        .unwrap_err();
    assert_eq!(e.code, code::INVALID_PARAMS);
}

async fn upload(conn: &mut Conn, project: &str, path: &str, pieces: &[&str]) -> String {
    let mut id = Value::Null;
    for piece in pieces {
        let r = conn
            .call(
                "fs.writeChunk",
                json!({"projectId": project, "path": path, "upload": id,
                       "content": {"kind": "text", "text": piece}}),
            )
            .await
            .unwrap();
        id = r["upload"].clone();
    }
    id.as_str().unwrap().to_owned()
}

async fn version_of(conn: &mut Conn, project: &str, path: &str) -> Value {
    conn.call("fs.read", json!({"projectId": project, "path": path}))
        .await
        .unwrap()["version"]
        .clone()
}

/// 모은 조각은 커밋에서 한 번에 쓰인다 — 커밋 전에는 파일이 그대로다. BOM 을 붙이고 받은 `version` 이 다음 쓰기에 맞다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chunks_are_written_at_once_on_commit() {
    let f = Fixture::new();
    let mut conn = f.own();
    let path = f.file("a.txt", b"old");
    let version = version_of(&mut conn, &f.id, &path).await;
    let id = upload(&mut conn, &f.id, &path, &["one ", "two ", "셋"]).await;
    assert_eq!(fs::read(&path).unwrap(), b"old");
    let written = conn
        .call(
            "fs.writeCommit",
            json!({"projectId": f.id, "path": path, "upload": id, "bom": true,
                   "expectedVersion": version}),
        )
        .await
        .unwrap();
    assert_eq!(fs::read(&path).unwrap(), "\u{feff}one two 셋".as_bytes());
    assert_eq!(
        written["version"],
        version_of(&mut conn, &f.id, &path).await
    );
    assert_eq!(f.fs.open_uploads(), 0);
    assert!(leftovers(&f.root).is_empty());
}

/// 완료 기준 ②: 커밋의 `expectedVersion` 이 지금 파일과 다르면 아무것도 쓰지 않는다(임시 파일도 남지 않는다). 모음은 닫힌다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_commit_with_a_stale_version_writes_nothing() {
    let f = Fixture::new();
    let mut conn = f.own();
    let path = f.file("a.txt", b"old");
    let stale = version_of(&mut conn, &f.id, &path).await;
    let id = upload(&mut conn, &f.id, &path, &["new ", "content"]).await;
    fs::write(&path, b"someone else").unwrap();
    let e = conn
        .call(
            "fs.writeCommit",
            json!({"projectId": f.id, "path": path, "upload": id, "expectedVersion": stale}),
        )
        .await
        .unwrap_err();
    assert_eq!(e.code, code::CONFLICT);
    assert_ne!(e.data.as_ref().unwrap()["current"], stale);
    assert_eq!(fs::read(&path).unwrap(), b"someone else");
    assert!(leftovers(&f.root).is_empty());
    assert_eq!(f.fs.open_uploads(), 0);
    // 닫힌 모음으로는 다시 커밋하지 못한다.
    let e = conn
        .call(
            "fs.writeCommit",
            json!({"projectId": f.id, "path": path, "upload": id, "expectedVersion": null}),
        )
        .await
        .unwrap_err();
    assert_eq!(e.code, code::NOT_FOUND);
    // 새 파일을 만든다던 커밋(`expectedVersion` 없음)도 파일이 있으면 쓰지 않는다.
    let id = upload(&mut conn, &f.id, &path, &["x"]).await;
    let e = conn
        .call(
            "fs.writeCommit",
            json!({"projectId": f.id, "path": path, "upload": id}),
        )
        .await
        .unwrap_err();
    assert_eq!(e.code, code::CONFLICT);
    assert_eq!(fs::read(&path).unwrap(), b"someone else");
}

/// 커밋의 원자 쓰기가 rename 직전에 실패해도 원래 파일이 그대로다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_commit_that_fails_before_the_rename_keeps_the_original() {
    let f = Fixture::new();
    let mut conn = f.own();
    let path = f.file("a.txt", b"original");
    let version = version_of(&mut conn, &f.id, &path).await;
    let id = upload(&mut conn, &f.id, &path, &["replacement"]).await;
    let session = *f
        .fs
        .uploads
        .lock()
        .unwrap()
        .open
        .values()
        .next()
        .map(|u| &u.session)
        .unwrap();
    let scope = ConnectionScope::Project(f.id.clone());
    let params = crate::chunks::FsWriteCommitParams {
        project_id: f.id.clone(),
        worktree: None,
        path: path.clone(),
        upload: id,
        bom: false,
        expected_version: version.as_str().map(str::to_owned),
    };
    let e =
        f.fs.write_commit_with(session, &scope, &params, |s| {
            if s == Stage::Written {
                Err(std::io::Error::other("injected"))
            } else {
                Ok(())
            }
        })
        .unwrap_err();
    assert!(e.message.contains("injected") || format!("{e:?}").contains("injected"));
    assert_eq!(fs::read(&path).unwrap(), b"original");
    assert!(leftovers(&f.root).is_empty());
}

/// 공격자 쪽: 모음 id 를 알아도 다른 연결로는 이어 쓰거나 커밋하거나 버리지 못한다. 같은 연결이라도 다른 파일 · 다른 프로젝트로
/// 돌려 쓰지 못한다(엉뚱한 파일에 내용이 쓰이지 않는다).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_upload_is_bound_to_its_connection_and_file() {
    let f = Fixture::new();
    let mut conn = f.own();
    let path = f.file("a.txt", b"a");
    let other_path = f.file("b.txt", b"b");
    let version = version_of(&mut conn, &f.id, &path).await;
    let id = upload(&mut conn, &f.id, &path, &["mine"]).await;

    // 같은 프로젝트의 다른 창(다른 세션).
    let mut thief = f.own();
    let e = thief
        .call(
            "fs.writeChunk",
            json!({"projectId": f.id, "path": path, "upload": id,
                   "content": {"kind": "text", "text": "evil"}}),
        )
        .await
        .unwrap_err();
    assert_eq!(e.code, code::NOT_FOUND);
    let e = thief
        .call(
            "fs.writeCommit",
            json!({"projectId": f.id, "path": path, "upload": id, "expectedVersion": version}),
        )
        .await
        .unwrap_err();
    assert_eq!(e.code, code::NOT_FOUND);
    thief
        .call("fs.writeAbort", json!({"projectId": f.id, "upload": id}))
        .await
        .unwrap();
    assert_eq!(
        f.fs.open_uploads(),
        1,
        "another connection cannot discard it"
    );

    // 같은 연결에서 다른 파일로 돌려 쓰기 · 커밋.
    let e = conn
        .call(
            "fs.writeChunk",
            json!({"projectId": f.id, "path": other_path, "upload": id,
                   "content": {"kind": "text", "text": "x"}}),
        )
        .await
        .unwrap_err();
    assert_eq!(e.code, code::INVALID_PARAMS);
    let other_version = version_of(&mut conn, &f.id, &other_path).await;
    let e = conn
        .call(
            "fs.writeCommit",
            json!({"projectId": f.id, "path": other_path, "upload": id,
                   "expectedVersion": other_version}),
        )
        .await
        .unwrap_err();
    assert_eq!(e.code, code::INVALID_PARAMS);
    assert_eq!(fs::read(&other_path).unwrap(), b"b");
    assert_eq!(fs::read(&path).unwrap(), b"a");

    // 다른 프로젝트 창은 범위에서 먼저 막힌다.
    let mut stranger = f.connect(ConnectionScope::Project(f.other.clone()));
    let e = stranger
        .call(
            "fs.writeCommit",
            json!({"projectId": f.id, "path": path, "upload": "u1", "expectedVersion": version}),
        )
        .await
        .unwrap_err();
    assert_eq!(e.code, code::PERMISSION_DENIED);
    assert_eq!(fs::read(&path).unwrap(), b"a");

    // 다른 종류의 조각(텍스트 모음에 base64)은 받지 않는다.
    let e = conn
        .call(
            "fs.writeChunk",
            json!({"projectId": f.id, "path": path, "upload": id,
                   "content": {"kind": "base64", "data": "AA=="}}),
        )
        .await
        .unwrap_err();
    assert_eq!(e.code, code::INVALID_PARAMS);
}

/// 버리기: `fs.writeAbort` 와 연결 닫힘은 모음을 버리고 파일은 그대로다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn aborting_or_closing_the_connection_discards_the_upload() {
    let f = Fixture::new();
    let mut conn = f.own();
    let path = f.file("a.txt", b"a");
    let id = upload(&mut conn, &f.id, &path, &["half"]).await;
    conn.call("fs.writeAbort", json!({"projectId": f.id, "upload": id}))
        .await
        .unwrap();
    assert_eq!(f.fs.open_uploads(), 0);
    let e = conn
        .call(
            "fs.writeCommit",
            json!({"projectId": f.id, "path": path, "upload": id}),
        )
        .await
        .unwrap_err();
    assert_eq!(e.code, code::NOT_FOUND);

    upload(&mut conn, &f.id, &path, &["half"]).await;
    assert_eq!(f.fs.open_uploads(), 1);
    conn.session.close();
    tokio::time::timeout(Duration::from_secs(5), async {
        while f.fs.open_uploads() != 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the closed connection's upload is discarded");
    assert_eq!(fs::read(&path).unwrap(), b"a");
}

/// 상한: 연결마다 · Host 전체의 동시 모음 수, 모음 하나의 크기(넘으면 모음을 버린다).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn uploads_are_bounded() {
    let f = Fixture::new();
    let path = f.file("a.txt", b"a");
    let mut first = f.own();
    for _ in 0..UPLOADS_PER_SESSION {
        upload(&mut first, &f.id, &path, &["x"]).await;
    }
    let e = first
        .call(
            "fs.writeChunk",
            json!({"projectId": f.id, "path": path, "content": {"kind": "text", "text": "x"}}),
        )
        .await
        .unwrap_err();
    assert_eq!(e.code, code::INTERNAL_ERROR);
    assert!(
        e.data
            .as_ref()
            .and_then(Value::as_str)
            .unwrap_or(&e.message)
            .starts_with("chunked write is busy"),
        "{e:?}"
    );
    let mut conns = Vec::new();
    while f.fs.open_uploads() < UPLOADS_MAX {
        let mut c = f.own();
        upload(&mut c, &f.id, &path, &["x"]).await;
        conns.push(c);
    }
    let mut late = f.own();
    let e = late
        .call(
            "fs.writeChunk",
            json!({"projectId": f.id, "path": path, "content": {"kind": "text", "text": "x"}}),
        )
        .await
        .unwrap_err();
    assert_eq!(e.code, code::INTERNAL_ERROR);
    first.session.close();
    for c in &conns {
        c.session.close();
    }
    tokio::time::timeout(Duration::from_secs(5), async {
        while f.fs.open_uploads() != 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();

    // 크기 상한: 넘는 조각은 거절되고 그 모음은 버려진다.
    let piece = "a".repeat(usize::try_from(CHUNK_WIRE_MAX).unwrap());
    let mut id = Value::Null;
    let mut sent = 0u64;
    let e = loop {
        match late
            .call(
                "fs.writeChunk",
                json!({"projectId": f.id, "path": path, "upload": id,
                       "content": {"kind": "text", "text": piece}}),
            )
            .await
        {
            Ok(r) => {
                id = r["upload"].clone();
                sent = r["received"].as_u64().unwrap();
            }
            Err(e) => break e,
        }
    };
    assert_eq!(e.code, code::INVALID_PARAMS);
    assert!(sent <= UPLOAD_MAX && sent + CHUNK_WIRE_MAX > UPLOAD_MAX);
    assert_eq!(f.fs.open_uploads(), 0);
    assert_eq!(fs::read(&path).unwrap(), b"a");
}

/// 파일 범위 연결(단일 파일 창)은 그 파일만 조각으로 읽고 쓴다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_file_window_reads_and_writes_only_its_file_in_chunks() {
    let f = Fixture::new();
    let mine = f.file("mine.txt", b"mine");
    let other = f.file("other.txt", b"other");
    let mut window = f.connect(Caller::file(Role::Owner, PathBuf::from(&mine)).scope);
    let (joined, _) = read_all(&mut window, "", &mine, 64).await;
    assert_eq!(joined["content"]["text"], "mine");
    let e = window
        .call(
            "fs.readChunk",
            json!({"projectId": f.id, "path": other, "maxBytes": 64}),
        )
        .await
        .unwrap_err();
    assert_eq!(e.code, code::INVALID_PARAMS);
    let id = upload(&mut window, "", &mine, &["new"]).await;
    window
        .call(
            "fs.writeCommit",
            json!({"projectId": "", "path": mine, "upload": id,
                   "expectedVersion": joined["version"]}),
        )
        .await
        .unwrap();
    assert_eq!(fs::read(Path::new(&mine)).unwrap(), b"new");
    let e = window
        .call(
            "fs.writeChunk",
            json!({"projectId": f.id, "path": other, "content": {"kind": "text", "text": "x"}}),
        )
        .await
        .unwrap_err();
    assert_eq!(e.code, code::INVALID_PARAMS);
    assert_eq!(fs::read(Path::new(&other)).unwrap(), b"other");
}
