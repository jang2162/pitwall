//! 원격 · 브랜치 쓰기는 그 프로젝트 창의 연결만 한다(쓰기 실행기의 범위 판정, M2-06): 다른 프로젝트 창 · Host 범위(웰컴 · 설정
//! 창)는 owner 라도 `git.push` · `git.branch.delete` 가 거절되고 원격 ref · 브랜치가 그대로다. 같은 창이면 된다.

use std::sync::Arc;
use std::time::Duration;

use pw_core::{Host, LocalHost};
use pw_rpc::{Caller, ConnectionScope, Role, code as rpc_code};
use serde_json::{Value, json};

use super::with_remote;

async fn call(connection: &mut pw_core::Connection, id: i64, method: &str, params: Value) -> Value {
    connection
        .sink
        .send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
    loop {
        let frame = tokio::time::timeout(Duration::from_secs(10), connection.source.recv())
            .await
            .expect("no reply in time")
            .expect("connection closed");
        if frame["id"] == json!(id) {
            return frame;
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn only_the_project_s_own_window_may_push_or_delete_a_branch() {
    let (f, remote) = with_remote();
    f.repo.git(&["push", "--quiet", "origin", "main"]);
    let pushed = f.repo.remote_ref(&remote, "refs/heads/main");
    assert!(pushed.is_some());
    let ahead = f.repo.commit_file("a.txt", "a\n", "ahead");
    f.repo.git(&["branch", "wip"]);
    let wip = f.repo.rev_parse("refs/heads/wip");
    let other_root = f.repo.base().join("other");
    std::fs::create_dir_all(&other_root).unwrap();
    let other = f
        .projects
        .create(pw_core::registry::NewProject {
            root: other_root,
            ..Default::default()
        })
        .unwrap()
        .id;
    let (branch_write, remote_write) = (Arc::clone(&f.write), Arc::clone(&f.write));
    let host = LocalHost::builder("local")
        .methods(move |router| crate::branch::register(router, branch_write))
        .methods(move |router| crate::remote::register(router, remote_write))
        .build();
    let push = json!({"projectId": f.project, "branch": "main"});
    let delete = json!({"projectId": f.project, "name": "wip", "force": true});

    for (who, scope) in [
        ("other project window", ConnectionScope::Project(other)),
        ("host", ConnectionScope::Host),
    ] {
        let mut connection = host
            .connect(Caller {
                role: Role::Owner,
                scope,
                window: None,
            })
            .await
            .unwrap();
        for (id, method, params) in [(1, "git.push", &push), (2, "git.branch.delete", &delete)] {
            let reply = call(&mut connection, id, method, params.clone()).await;
            assert_eq!(
                reply["error"]["code"],
                rpc_code::PERMISSION_DENIED,
                "{who} {method}: {reply}"
            );
        }
        assert_eq!(
            f.repo.remote_ref(&remote, "refs/heads/main"),
            pushed,
            "{who}"
        );
        assert_eq!(f.repo.rev_parse("refs/heads/wip"), wip, "{who}");
    }

    let mut own = host
        .connect(Caller {
            role: Role::Owner,
            scope: ConnectionScope::Project(f.project.clone()),
            window: None,
        })
        .await
        .unwrap();
    let reply = call(&mut own, 1, "git.push", push).await;
    assert!(reply["error"].is_null(), "{reply}");
    assert_eq!(f.repo.remote_ref(&remote, "refs/heads/main"), Some(ahead));
    let reply = call(&mut own, 2, "git.branch.delete", delete).await;
    assert!(reply["error"].is_null(), "{reply}");
    let gone = f.repo.try_git_in(
        f.repo.root(),
        &["rev-parse", "--verify", "--quiet", "refs/heads/wip"],
    );
    assert!(!gone.status.success(), "wip is deleted");
}
