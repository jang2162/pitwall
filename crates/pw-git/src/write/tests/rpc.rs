//! RPC 경로: 창이 하는 그대로 Host 연결로 `git.prompts` 스트림을 열어 질문을 받고 `git.prompt.answer` 로 답한다. `git.operation`
//! 의 와이어 형식, 질문 · 답은 owner 만, 없는 프로젝트 · 끝난 질문의 오류.

use std::sync::Arc;
use std::time::Duration;

use pw_core::{Host, LocalHost};
use pw_rpc::{Caller, Role, code as rpc_code};
use serde_json::{Value, json};

use super::super::*;
use crate::testing::write::WriteFixture;

struct Client(pw_core::Connection, i64);

impl Client {
    async fn connect(host: &LocalHost, role: Role) -> Self {
        Self(host.connect(Caller::new(role)).await.unwrap(), 0)
    }

    fn send(&mut self, method: &str, params: Value) -> i64 {
        self.1 += 1;
        self.0
            .sink
            .send(json!({"jsonrpc": "2.0", "id": self.1, "method": method, "params": params}));
        self.1
    }

    fn notify(&self, method: &str, params: Value) {
        self.0
            .sink
            .send(json!({"jsonrpc": "2.0", "method": method, "params": params}));
    }

    async fn next(&mut self) -> Value {
        tokio::time::timeout(Duration::from_secs(10), self.0.source.recv())
            .await
            .expect("no frame in time")
            .expect("connection closed")
    }

    /// 응답 `id` 를 기다린다(그 사이의 청크는 `chunks` 에 모은다).
    async fn reply(&mut self, id: i64, chunks: &mut Vec<Value>) -> Value {
        loop {
            let frame = self.next().await;
            if frame["id"] == json!(id) {
                return frame;
            }
            if frame["method"] == "stream.chunk" {
                chunks.push(frame["params"]["data"].clone());
            }
        }
    }

    async fn chunk(&mut self) -> Value {
        loop {
            let frame = self.next().await;
            if frame["method"] == "stream.chunk" {
                return frame["params"]["data"].clone();
            }
        }
    }

    async fn call(&mut self, method: &str, params: Value) -> Value {
        let id = self.send(method, params);
        self.reply(id, &mut Vec::new()).await
    }
}

fn host(f: &WriteFixture) -> LocalHost {
    let write = Arc::clone(&f.write);
    LocalHost::builder("local")
        .methods(move |router| register(router, write))
        .build()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_window_hears_the_question_over_rpc_and_its_answer_decides_the_write() {
    let f = WriteFixture::new(true);
    f.repo.start_conflicting_merge();
    let host = host(&f);
    let mut window = Client::connect(&host, Role::Owner).await;

    let operation = window
        .call("git.operation", json!({"projectId": f.project}))
        .await;
    assert_eq!(
        operation["result"]["operation"]["kind"], "merge",
        "{operation}"
    );
    assert_eq!(
        operation["result"]["operation"]["actions"],
        json!(["continue", "abort"])
    );

    let opened = window
        .call("git.prompts", json!({"projectId": f.project}))
        .await;
    let stream = opened["result"]["streamId"].clone();
    window.notify("stream.credit", json!({"streamId": stream, "credit": 16}));
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while f.write.prompts().listeners(&f.project) == 0 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the stream did not start listening"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    let abort = tokio::spawn({
        let write = Arc::clone(&f.write);
        let project = f.project.clone();
        async move { write.act(&project, None, OperationAction::Abort).await }
    });
    let open = window.chunk().await;
    assert_eq!(open["kind"], "open", "{open}");
    assert_eq!(open["prompt"]["request"]["kind"], "unsaved");
    assert_eq!(
        open["prompt"]["request"]["paths"],
        Value::Null,
        "the whole work tree"
    );
    assert_eq!(
        open["prompt"]["request"]["workTree"],
        f.repo.root().display().to_string()
    );
    let prompt_id = open["prompt"]["promptId"].clone();

    let mut readonly = Client::connect(&host, Role::Readonly).await;
    let denied = readonly
        .call(
            "git.prompt.answer",
            json!({"projectId": f.project, "promptId": prompt_id, "answer": {"kind": "proceed"}}),
        )
        .await;
    assert_eq!(denied["error"]["code"], rpc_code::PERMISSION_DENIED);
    let denied = readonly
        .call("git.prompts", json!({"projectId": f.project}))
        .await;
    assert_eq!(denied["error"]["code"], rpc_code::PERMISSION_DENIED);

    let mismatch = window
        .call(
            "git.prompt.answer",
            json!({"projectId": f.project, "promptId": prompt_id, "answer": {"kind": "askpass", "value": "x"}}),
        )
        .await;
    assert_eq!(mismatch["error"]["code"], rpc_code::INVALID_PARAMS);

    let mut chunks = Vec::new();
    let id = window.send(
        "git.prompt.answer",
        json!({"projectId": f.project, "promptId": prompt_id, "answer": {"kind": "cancel"}}),
    );
    let answered = window.reply(id, &mut chunks).await;
    assert_eq!(answered["result"], Value::Null, "{answered}");
    assert_eq!(abort.await.unwrap(), Err(WriteError::Cancelled));
    let closed = match chunks.pop() {
        Some(chunk) => chunk,
        None => window.chunk().await,
    };
    assert_eq!(closed, json!({"kind": "closed", "promptId": prompt_id}));

    let late = window
        .call(
            "git.prompt.answer",
            json!({"projectId": f.project, "promptId": prompt_id, "answer": {"kind": "cancel"}}),
        )
        .await;
    assert_eq!(late["error"]["code"], rpc_code::NOT_FOUND);
    let still = window
        .call("git.operation", json!({"projectId": f.project}))
        .await;
    assert_eq!(
        still["result"]["operation"]["kind"], "merge",
        "the cancelled abort left the merge as it was"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn prompts_of_an_unknown_project_are_refused_and_no_repository_has_no_operation() {
    let f = WriteFixture::new(true);
    let host = host(&f);
    let mut window = Client::connect(&host, Role::Owner).await;
    let opened = window
        .call("git.prompts", json!({"projectId": "no-such-project"}))
        .await;
    let stream = opened["result"]["streamId"].clone();
    window.notify("stream.credit", json!({"streamId": stream, "credit": 1}));
    let end = loop {
        let frame = window.next().await;
        if frame["method"] == "stream.end" {
            break frame["params"].clone();
        }
    };
    assert_eq!(end["streamId"], stream);
    assert_eq!(end["error"]["code"], rpc_code::INVALID_PARAMS, "{end}");
    assert_eq!(f.write.prompts().listeners("no-such-project"), 0);
    let none = window
        .call("git.operation", json!({"projectId": f.project}))
        .await;
    assert_eq!(none["result"], json!({"operation": null}));
    let failed = window
        .call("git.operation.continue", json!({"projectId": f.project}))
        .await;
    assert_eq!(failed["error"]["code"], code::NO_OPERATION, "{failed}");
}
