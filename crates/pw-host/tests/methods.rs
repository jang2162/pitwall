//! 18-m7-plan M7-01 완료 기준 ①: 조립한 Host 가 등록한 메서드(이름 · 종류 · 등급 · role)가 스냅숏(`methods.snapshot`)과 같다.
//! 스냅숏은 조립을 앱 크레이트에서 옮기기 전의 앱 Host 로 떴다 — 옮기면서 메서드 하나가 빠지거나 늘거나 등급 · role 이 바뀌면 실패한다.
//!
//! 메서드를 더하거나 빼거나 등급 · role 을 바꾸는 행은 `PW_HOST_UPDATE_SNAPSHOT=1` 로 이 시험을 돌려 스냅숏을 다시 쓰고, 그 diff 를
//! 커밋에 함께 싣는다.

mod common;

use std::sync::Arc;

#[test]
fn the_registered_methods_match_the_snapshot() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let assembled = common::assemble(
        tmp.path(),
        Arc::new(pw_db::secret::MemorySecretStore::new()),
        runtime.handle().clone(),
    );
    let actual: String = assembled
        .host
        .router()
        .methods()
        .into_iter()
        .map(|m| format!("{} {:?} {:?} {:?}\n", m.name, m.kind, m.tier, m.roles))
        .collect();
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/methods.snapshot");
    if std::env::var_os("PW_HOST_UPDATE_SNAPSHOT").is_some() {
        std::fs::write(&path, &actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path).unwrap();
    assert!(actual.lines().count() > 200, "{actual}");
    assert_eq!(
        actual, expected,
        "메서드 목록이 스냅숏과 다르다 — 의도한 변경이면 PW_HOST_UPDATE_SNAPSHOT=1 로 다시 쓴다"
    );
}
