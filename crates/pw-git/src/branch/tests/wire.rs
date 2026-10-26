//! 와이어 형식: 체크아웃 대상의 `kind` 태그 · 빠진 선택 필드, 결과 · 이전 해시의 camelCase.

use serde_json::json;

use super::super::methods::{GitBranchDeleteResult, GitCheckoutParams};
use super::super::*;

#[test]
fn checkout_targets_are_tagged_by_kind() {
    let params: GitCheckoutParams = serde_json::from_value(json!({
        "projectId": "p",
        "target": {"kind": "remote", "remote": "origin", "branch": "feat/x"}
    }))
    .unwrap();
    assert_eq!(params.worktree, None);
    assert_eq!(
        params.target,
        GitCheckoutTarget::Remote {
            remote: "origin".into(),
            branch: "feat/x".into(),
            local: None
        }
    );
    for (wire, target) in [
        (
            json!({"kind": "branch", "name": "main"}),
            GitCheckoutTarget::Branch {
                name: "main".into(),
            },
        ),
        (
            json!({"kind": "commit", "rev": "abc"}),
            GitCheckoutTarget::Commit { rev: "abc".into() },
        ),
    ] {
        assert_eq!(
            serde_json::from_value::<GitCheckoutTarget>(wire).unwrap(),
            target
        );
    }
}

#[test]
fn a_deletion_reports_the_previous_hash() {
    let result = GitBranchDeleteResult {
        work_tree: "/w".into(),
        deleted: GitRefMove {
            ref_name: "refs/heads/x".into(),
            previous: Some("0123".into()),
            current: None,
        },
    };
    assert_eq!(
        serde_json::to_value(result).unwrap(),
        json!({"workTree": "/w", "deleted": {"refName": "refs/heads/x", "previous": "0123", "current": null}})
    );
}
