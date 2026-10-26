//! RPC 경로: 창이 하는 그대로 Host 연결로 `git.prompts` 스트림을 열어 질문을 받고 `git.prompt.answer` 로 답한다. `git.operation`
//! 의 와이어 형식, 질문 · 답은 owner 만 · 그 프로젝트 창의 연결만, 없는 프로젝트 · 끝난 질문의 오류.

use std::sync::Arc;
use std::time::Duration;

use pw_core::{Host, LocalHost};
use pw_rpc::{Caller, ConnectionScope, Role, code as rpc_code};
use serde_json::{Value, json};

use super::super::*;
use crate::testing::write::WriteFixture;

struct Client(pw_core::Connection, i64);

impl Client {
    async fn connect(host: &LocalHost, caller: Caller) -> Self {
        Self(host.connect(caller).await.unwrap(), 0)
    }

    /// 프로젝트 창의 연결(연결 범위가 그 프로젝트 — Host Router 가 `projectId` 연결에 싣는 것).
    async fn window(host: &LocalHost, role: Role, project: &str) -> Self {
        Self::connect(
            host,
            Caller {
                role,
                scope: ConnectionScope::Project(project.to_owned()),
            },
        )
        .await
    }

    /// `git.prompts` 를 열고 Host 가 듣기 시작할 때까지 기다린다.
    async fn listen(&mut self, write: &GitWrite, project: &str) {
        let before = write.prompts().listeners(project);
        let opened = self
            .call("git.prompts", json!({"projectId": project}))
            .await;
        let stream = opened["result"]["streamId"].clone();
        assert!(!stream.is_null(), "{opened}");
        self.notify("stream.credit", json!({"streamId": stream, "credit": 16}));
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while write.prompts().listeners(project) == before {
            assert!(
                tokio::time::Instant::now() < deadline,
                "the stream did not start listening"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// `git.prompts` 를 열면 스트림이 그 오류로 끝난다(열기 응답 뒤 `stream.end`).
    async fn refused_stream(&mut self, project: &str) -> Value {
        let opened = self
            .call("git.prompts", json!({"projectId": project}))
            .await;
        if !opened["error"].is_null() {
            return opened["error"].clone();
        }
        let stream = opened["result"]["streamId"].clone();
        self.notify("stream.credit", json!({"streamId": stream, "credit": 1}));
        loop {
            let frame = self.next().await;
            if frame["method"] == "stream.end" {
                assert_eq!(frame["params"]["streamId"], stream);
                return frame["params"]["error"].clone();
            }
            assert_ne!(
                frame["method"], "stream.chunk",
                "a refused stream sent {frame}"
            );
        }
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
    let mut window = Client::window(&host, Role::Owner, &f.project).await;

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

    window.listen(&f.write, &f.project).await;

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

    let mut readonly = Client::window(&host, Role::Readonly, &f.project).await;
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
    let mut unknown = Client::window(&host, Role::Owner, "no-such-project").await;
    let end = unknown.refused_stream("no-such-project").await;
    assert_eq!(end["code"], rpc_code::INVALID_PARAMS, "{end}");
    assert_eq!(f.write.prompts().listeners("no-such-project"), 0);
    let mut window = Client::window(&host, Role::Owner, &f.project).await;
    let none = window
        .call("git.operation", json!({"projectId": f.project}))
        .await;
    assert_eq!(none["result"], json!({"operation": null}));
    let failed = window
        .call("git.operation.continue", json!({"projectId": f.project}))
        .await;
    assert_eq!(failed["error"]["code"], code::NO_OPERATION, "{failed}");
}

/// 다른 프로젝트 창 · 웰컴 · 설정 창(Host 범위) · 파일 창은 이 프로젝트의 질문을 듣지도 답하지도 못한다. 듣지 못하므로 쓰기는 「창
/// 없음」 이다 — 그 프로젝트 창이 열리면 그 창만 질문을 받고 답한다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn only_the_projects_own_window_hears_and_answers_its_prompts() {
    let f = WriteFixture::new(true);
    f.repo.start_conflicting_merge();
    let elsewhere = f.repo.base().join("another-project");
    std::fs::create_dir_all(&elsewhere).unwrap();
    let other_project = f
        .projects
        .create(pw_core::registry::NewProject {
            root: elsewhere,
            ..Default::default()
        })
        .unwrap()
        .id;
    let host = host(&f);
    let file = f.repo.root().join("README.md");
    let outsiders = [
        (
            "another project's window",
            Caller {
                role: Role::Owner,
                scope: ConnectionScope::Project(other_project),
            },
        ),
        ("the welcome or settings window", Caller::new(Role::Owner)),
        ("a file window", Caller::file(Role::Owner, file)),
    ];
    let mut clients = Vec::new();
    for (who, caller) in outsiders {
        let mut client = Client::connect(&host, caller).await;
        let refused = client.refused_stream(&f.project).await;
        assert_eq!(
            refused["code"],
            rpc_code::PERMISSION_DENIED,
            "{who}: {refused}"
        );
        clients.push((who, client));
    }
    assert_eq!(f.write.prompts().listeners(&f.project), 0);
    assert_eq!(
        f.write.act(&f.project, None, OperationAction::Abort).await,
        Err(WriteError::NoWindow),
        "no window of the project heard the confirmation"
    );

    let mut window = Client::window(&host, Role::Owner, &f.project).await;
    window.listen(&f.write, &f.project).await;
    let abort = tokio::spawn({
        let write = Arc::clone(&f.write);
        let project = f.project.clone();
        async move { write.act(&project, None, OperationAction::Abort).await }
    });
    let open = window.chunk().await;
    let prompt_id = open["prompt"]["promptId"].clone();
    for (who, client) in &mut clients {
        let denied = client
            .call(
                "git.prompt.answer",
                json!({"projectId": f.project, "promptId": prompt_id, "answer": {"kind": "proceed"}}),
            )
            .await;
        assert_eq!(
            denied["error"]["code"],
            rpc_code::PERMISSION_DENIED,
            "{who}: {denied}"
        );
    }
    assert_eq!(f.write.prompts().pending(), 1, "nobody else answered it");
    let answered = window
        .call(
            "git.prompt.answer",
            json!({"projectId": f.project, "promptId": prompt_id, "answer": {"kind": "cancel"}}),
        )
        .await;
    assert_eq!(answered["result"], Value::Null, "{answered}");
    assert_eq!(abort.await.unwrap(), Err(WriteError::Cancelled));
}
