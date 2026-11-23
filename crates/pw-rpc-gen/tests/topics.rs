//! 토픽 목록과 `rpc_event!` 선언이 맞는지 보는 표 시험(`docs/design/18-m7-plan.md` M7-02 ④).
//!
//! 토픽 목록은 소스에서 읽는다: `crates/` 와 `apps/desktop/src-tauri/src` 의 `const TOPIC_<이름>: &str = "<토픽>"` 줄.
//! 선언은 생성기와 같은 크레이트를 링크한 [`pw_rpc::events`] 다. 두 집합이 같아야 한다 — 토픽 상수를 더하고 선언을
//! 빠뜨리면(그 토픽은 아무도 구독할 수 없다) 실패하고, 상수 없이 선언만 남아도 실패한다.
//!
//! **이 시험이 보지 않는 것**: 이름이 `TOPIC_` 으로 시작하지 않는 상수, 리터럴로 바로 낸 토픽(`publish(scope, "x.y", …)`),
//! 실행 중에 조립한 토픽 이름, 시험 파일(`tests/` 아래 · `*tests.rs`)의 상수. 그런 토픽은 선언이 없으면 구독이 거절될 뿐
//! 여기서는 잡히지 않는다 — 토픽은 `TOPIC_` 상수로 두고 그 옆에 선언한다(`pw_rpc::events` 머리 주석).

use std::collections::BTreeSet;
use std::path::Path;

// 생성기(`src/main.rs`)와 같은 링크 목록 — 빠지면 그 크레이트의 선언이 보이지 않아 이 시험이 실패한다.
use pw_core as _;
use pw_db as _;
use pw_db_sql as _;
use pw_db_types as _;
use pw_debug as _;
use pw_diag as _;
use pw_ext as _;
use pw_fs as _;
use pw_git as _;
use pw_i18n as _;
use pw_index as _;
use pw_lang as _;
use pw_lsp as _;
use pw_lsp_servers as _;
use pw_mcp as _;
use pw_open_with as _;
use pw_remote as _;
use pw_run as _;
use pw_runtime as _;
use pw_syntax as _;
use pw_tools as _;

fn is_test_file(path: &Path) -> bool {
    path.components().any(|c| c.as_os_str() == "tests")
        || path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.ends_with("tests.rs"))
}

fn scan(dir: &Path, found: &mut BTreeSet<(String, String)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        if name == "target" || name == "node_modules" || name == "tests" {
            continue;
        }
        if path.is_dir() {
            scan(&path, found);
        } else if path.extension().is_some_and(|e| e == "rs") && !is_test_file(&path) {
            let text = std::fs::read_to_string(&path).unwrap();
            for line in text.lines() {
                let Some(rest) = line.split_once("const TOPIC_").map(|(_, r)| r) else {
                    continue;
                };
                let Some((_, value)) = rest.split_once(": &str = \"") else {
                    continue;
                };
                let topic = value.split_once('"').expect("closing quote").0;
                found.insert((topic.to_owned(), path.display().to_string()));
            }
        }
    }
}

#[test]
fn every_topic_constant_is_declared_and_every_declaration_has_a_constant() {
    let root = pw_rpc_gen::repo_root();
    let mut found = BTreeSet::new();
    scan(&root.join("crates"), &mut found);
    scan(&root.join("apps/desktop/src-tauri/src"), &mut found);
    let constants: BTreeSet<&str> = found.iter().map(|(t, _)| t.as_str()).collect();
    // 표가 비면 스캔이 길을 잃은 것이다 — 비교가 공허하게 통과하지 않게.
    assert!(
        constants.len() >= 29,
        "scan found too few topics: {found:?}"
    );

    let declared: BTreeSet<&str> = pw_rpc::events().iter().map(|m| m.name).collect();

    let undeclared: Vec<_> = found
        .iter()
        .filter(|(t, _)| !declared.contains(t.as_str()))
        .collect();
    assert!(
        undeclared.is_empty(),
        "topic constants without `pw_rpc::rpc_event!` (nobody can subscribe): {undeclared:?}"
    );
    let orphan: Vec<_> = declared.difference(&constants).collect();
    assert!(
        orphan.is_empty(),
        "`rpc_event!` declarations without a TOPIC_ constant: {orphan:?}"
    );
}
