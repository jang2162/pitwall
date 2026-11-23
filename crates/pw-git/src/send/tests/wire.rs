//! RPC 로 한 번 보내기: 원본 Host 의 Router 에서 `git.send.prepare` 스트림을 받아 대상 Host 의 Router 에 `theirs` →
//! `receive.begin` · `chunk` · `finish` → `apply` 로 잇는다(Desktop 이 하는 일). 등록(`crate::write::register` 가 `send::register`
//! 를 부른다) · 와이어 이름 · camelCase · 연결에 묶인 `uploadId` 를 본다.

use pw_rpc::{Caller, ConnectionScope, EventHub, Outbound, Router, Session};
use serde_json::{Value, json};
use tokio::sync::mpsc::UnboundedReceiver;

use super::*;

pub(super) struct Conn {
    session: Session,
    rx: UnboundedReceiver<Outbound>,
    next: u64,
}

impl Conn {
    pub(super) fn open(f: &WriteFixture) -> Self {
        Self::open_as(f, ConnectionScope::Project(f.project.clone()))
    }

    /// 그 범위의 연결(MCP 세션은 `ConnectionScope::Agent`).
    pub(super) fn open_as(f: &WriteFixture, scope: ConnectionScope) -> Self {
        let mut router = Router::new();
        crate::write::register(&mut router, Arc::clone(&f.write));
        let caller = Caller {
            role: Role::Owner,
            scope,
            window: None,
        };
        let (session, rx) = Session::new(Arc::new(router), EventHub::new(), caller);
        Self {
            session,
            rx,
            next: 0,
        }
    }

    /// unary 호출 하나. `Ok(result)` 나 `Err(error)`.
    pub(super) async fn call(&mut self, method: &str, params: Value) -> Result<Value, Value> {
        self.next += 1;
        let id = self.next;
        self.session
            .handle_value(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        loop {
            let message = serde_json::to_value(self.rx.recv().await.unwrap()).unwrap();
            if message["id"] != json!(id) {
                continue;
            }
            return match message.get("error") {
                Some(error) => Err(error.clone()),
                None => Ok(message["result"].clone()),
            };
        }
    }

    /// 스트림을 열고 끝까지 받은 항목들.
    pub(super) async fn stream(&mut self, method: &str, params: Value) -> Vec<Value> {
        let opened = self.call(method, params).await.unwrap();
        let stream_id = opened["streamId"].clone();
        self.session.handle_value(
            json!({"jsonrpc": "2.0", "method": "stream.credit", "params": {"streamId": stream_id, "credit": 1000}}),
        );
        let mut items = Vec::new();
        loop {
            let message = serde_json::to_value(self.rx.recv().await.unwrap()).unwrap();
            match message["method"].as_str() {
                Some("stream.chunk") if message["params"]["streamId"] == stream_id => {
                    items.push(message["params"]["data"].clone());
                }
                Some("stream.end") if message["params"]["streamId"] == stream_id => {
                    assert_eq!(message["params"]["status"], "completed", "{message}");
                    return items;
                }
                _ => {}
            }
        }
    }
}

#[tokio::test]
async fn a_send_over_rpc_lands_on_the_target() {
    let pair = Pair::new(true);
    pair.src.repo.git(&["switch", "--quiet", "-c", "feat/x"]);
    let commit = pair.src.repo.commit_file("x.txt", "x\n", "x");
    let mut src = Conn::open(&pair.src);
    let mut dst = Conn::open(&pair.dst);

    let theirs = dst
        .call(
            "git.send.theirs",
            json!({"projectId": pair.dst.project, "branch": "feat/x"}),
        )
        .await
        .unwrap();
    assert_eq!(theirs["branchTip"], Value::Null);
    assert_eq!(theirs["theirs"], json!([pair.dst.repo.head()]));
    let items = src
        .stream(
            "git.send.prepare",
            json!({"projectId": pair.src.project, "branch": "feat/x", "commit": commit, "theirs": theirs["theirs"]}),
        )
        .await;
    assert_eq!(items[0]["kind"], "info");
    assert_eq!(items[0]["uncommitted"], false);
    let size = items[0]["size"].as_u64().unwrap();
    let begin = dst
        .call(
            "git.send.receive.begin",
            json!({"projectId": pair.dst.project, "sourceMachine": MACHINE, "branch": "feat/x", "commit": commit, "size": size}),
        )
        .await
        .unwrap();
    let upload = begin["uploadId"].clone();
    // 다른 연결은 그 id 를 쓰지 못한다.
    let mut stranger = Conn::open(&pair.dst);
    let refused = stranger
        .call(
            "git.send.receive.chunk",
            json!({"uploadId": upload, "offset": 0, "data": ""}),
        )
        .await
        .unwrap_err();
    assert_eq!(refused["code"], pw_rpc::code::INVALID_PARAMS);
    for item in &items[1..] {
        assert_eq!(item["kind"], "data");
        dst.call(
            "git.send.receive.chunk",
            json!({"uploadId": upload, "offset": item["offset"], "data": item["data"]}),
        )
        .await
        .unwrap();
    }
    let finished = dst
        .call("git.send.receive.finish", json!({"uploadId": upload}))
        .await
        .unwrap();
    assert_eq!(
        finished["incomingRef"],
        format!("refs/pitwall/incoming/{MACHINE}/feat/x")
    );
    assert_eq!(finished["commit"], commit);
    let applied = dst
        .call(
            "git.send.apply",
            json!({"projectId": pair.dst.project, "sourceMachine": MACHINE, "branch": "feat/x", "commit": commit, "expected": null}),
        )
        .await
        .unwrap();
    assert_eq!(
        applied["outcome"],
        json!({"kind": "applied", "previous": null, "current": commit, "created": true, "switched": true, "lfsPointers": 0})
    );
    assert_eq!(
        pair.dst.repo.git(&["symbolic-ref", "HEAD"]).trim(),
        "refs/heads/feat/x"
    );
    assert_eq!(pair.dst.repo.head(), commit);
}
