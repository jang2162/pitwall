//! RPC 경로: 창이 하는 그대로 Host 연결로 부른다 — 등록, 입력 · 결과의 와이어 형식(camelCase), 오류 코드.

use std::sync::Arc;
use std::time::Duration;

use pw_core::{Host, LocalHost};
use pw_rpc::{Caller, ConnectionScope, Role, code as rpc_code};
use serde_json::{Value, json};

use super::super::register;
use crate::testing::write::WriteFixture;

struct Client(pw_core::Connection, i64);

impl Client {
    async fn call(&mut self, method: &str, params: Value) -> Value {
        self.1 += 1;
        let id = self.1;
        self.0
            .sink
            .send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        loop {
            let frame = tokio::time::timeout(Duration::from_secs(10), self.0.source.recv())
                .await
                .expect("no frame in time")
                .expect("connection closed");
            if frame["id"] == json!(id) {
                return frame;
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_history_methods_speak_camel_case_over_rpc() {
    let f = WriteFixture::new(true);
    let first = f.repo.head();
    let second = f.repo.commit_file("a.txt", "a\n", "a");
    let write = Arc::clone(&f.write);
    let host = LocalHost::builder("local")
        .methods(move |router| register(router, write))
        .build();
    let mut window = Client(
        host.connect(Caller {
            role: Role::Owner,
            scope: ConnectionScope::Project(f.project.clone()),
        })
        .await
        .unwrap(),
        0,
    );

    let preview = window
        .call(
            "git.resetPreview",
            json!({"projectId": f.project, "commit": first}),
        )
        .await;
    assert_eq!(preview["result"]["head"], second.as_str(), "{preview}");
    assert_eq!(preview["result"]["target"], first.as_str());
    assert_eq!(
        preview["result"]["lost"],
        json!({"changes": [], "overwritten": []})
    );

    let refused = window
        .call(
            "git.reset",
            json!({"projectId": f.project, "commit": first, "mode": "soft", "expected": first}),
        )
        .await;
    assert_eq!(refused["error"]["code"], rpc_code::CONFLICT, "{refused}");
    assert_eq!(refused["error"]["data"]["head"], second.as_str());

    let done = window
        .call(
            "git.reset",
            json!({"projectId": f.project, "commit": first, "mode": "soft", "expected": second}),
        )
        .await;
    assert_eq!(
        done["result"]["moved"],
        json!({"refName": "refs/heads/main", "previous": second, "current": first}),
        "{done}"
    );
    assert_eq!(done["result"]["lost"], Value::Null);

    let merged = window
        .call(
            "git.merge",
            json!({"projectId": f.project, "rev": second, "fastForward": "only"}),
        )
        .await;
    // fast-forward 는 작업 트리를 바꾼다 — 듣는 창이 없으니 쓰기를 하지 않는다(M2-06 의 2006).
    assert_eq!(
        merged["error"]["code"],
        crate::write::code::NO_WINDOW,
        "{merged}"
    );

    let bad = window
        .call(
            "git.cherryPick",
            json!({"projectId": f.project, "commits": ["--abort"]}),
        )
        .await;
    assert_eq!(bad["error"]["code"], rpc_code::INVALID_PARAMS, "{bad}");
    assert_eq!(f.repo.head(), first);
}
