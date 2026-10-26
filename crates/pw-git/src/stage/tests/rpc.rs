//! RPC 경로: 확인이 필요한 쓰기(`confirmed`)와 모든 쓰기는 owner 연결만 부른다 — mobile · readonly 연결은 `confirmed: true` 를
//! 보내도 거절된다. 어느 메서드도 MCP 툴이 아니다(결정 M2-D7). 와이어 꼴은 camelCase 다.

use std::sync::Arc;
use std::time::Duration;

use pw_core::{Host, LocalHost};
use pw_rpc::{Caller, ConnectionScope, Role, Router, Tier, code as rpc_code};
use serde_json::{Value, json};

use crate::stage::register;
use crate::testing::write::WriteFixture;

struct Client(pw_core::Connection, i64);

impl Client {
    async fn window(host: &LocalHost, role: Role, project: &str) -> Self {
        let caller = Caller {
            role,
            scope: ConnectionScope::Project(project.to_owned()),
        };
        Self(host.connect(caller).await.unwrap(), 0)
    }

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

fn host(f: &WriteFixture) -> LocalHost {
    let write = Arc::clone(&f.write);
    LocalHost::builder("local")
        .methods(move |router| register(router, write))
        .build()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn only_an_owner_connection_may_discard_or_drop_even_when_confirmed() {
    let f = WriteFixture::new(true);
    f.repo.write("README.md", "changed\n");
    f.repo.git(&["stash", "push", "--quiet"]);
    let stash = f.repo.rev_parse("stash@{0}");
    f.repo.write("README.md", "changed again\n");
    let host = host(&f);
    let hunk = json!({"oldStart": 1, "oldLines": 1, "newStart": 1, "newLines": 1, "header": "", "lines": []});
    let calls = [
        (
            "git.discard",
            json!({"projectId": f.project, "paths": ["README.md"], "confirmed": true}),
        ),
        (
            "git.discardLines",
            json!({"projectId": f.project, "path": "README.md", "hunks": [{"hunk": hunk}], "confirmed": true}),
        ),
        (
            "git.stash.drop",
            json!({"projectId": f.project, "index": 0, "commit": stash, "confirmed": true}),
        ),
        (
            "git.stage",
            json!({"projectId": f.project, "paths": ["README.md"]}),
        ),
        ("git.stash.list", json!({"projectId": f.project})),
    ];
    for role in [Role::Mobile, Role::Readonly] {
        let mut client = Client::window(&host, role, &f.project).await;
        for (method, params) in &calls {
            let reply = client.call(method, params.clone()).await;
            assert_eq!(
                reply["error"]["code"],
                rpc_code::PERMISSION_DENIED,
                "{role:?} {method}: {reply}"
            );
        }
    }
    assert_eq!(
        std::fs::read_to_string(f.repo.root().join("README.md")).unwrap(),
        "changed again\n"
    );
    assert_eq!(f.repo.rev_parse("stash@{0}"), stash);

    let mut owner = Client::window(&host, Role::Owner, &f.project).await;
    for (method, params) in &calls[..3] {
        let mut unconfirmed = params.clone();
        unconfirmed["confirmed"] = json!(false);
        let reply = owner.call(method, unconfirmed).await;
        assert_eq!(
            reply["error"]["code"],
            rpc_code::INVALID_PARAMS,
            "{method}: {reply}"
        );
        let mut missing = params.clone();
        missing.as_object_mut().unwrap().remove("confirmed");
        let reply = owner.call(method, missing).await;
        assert_eq!(
            reply["error"]["code"],
            rpc_code::INVALID_PARAMS,
            "{method}: {reply}"
        );
    }
    assert_eq!(f.repo.rev_parse("stash@{0}"), stash);

    let list = owner
        .call("git.stash.list", json!({"projectId": f.project}))
        .await;
    assert_eq!(
        list["result"]["workTree"],
        f.repo.root().display().to_string(),
        "{list}"
    );
    assert_eq!(list["result"]["stashes"][0]["commit"], stash);
    assert_eq!(list["result"]["stashes"][0]["index"], 0);
}

#[tokio::test]
async fn no_method_is_an_mcp_tool_and_every_method_is_owner_only() {
    let f = WriteFixture::new(true);
    let mut router = Router::new();
    register(&mut router, Arc::clone(&f.write));
    let metas = router.methods();
    let mut names: Vec<&str> = metas.iter().map(|m| m.name).collect();
    names.sort_unstable();
    assert_eq!(
        names,
        [
            "git.discard",
            "git.discardLines",
            "git.stage",
            "git.stageLines",
            "git.stash.apply",
            "git.stash.drop",
            "git.stash.list",
            "git.stash.pop",
            "git.stash.push",
            "git.unstage",
            "git.unstageLines",
        ]
    );
    for meta in metas {
        assert!(!meta.mcp, "{} must not be an MCP tool", meta.name);
        assert!(meta.roles.contains(Role::Owner), "{}", meta.name);
        assert!(
            !meta.roles.contains(Role::Mobile) && !meta.roles.contains(Role::Readonly),
            "{} is owner only",
            meta.name
        );
        let read = meta.name == "git.stash.list";
        assert_eq!(matches!(meta.tier, Tier::Read), read, "{}", meta.name);
    }
}
