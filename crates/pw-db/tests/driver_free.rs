//! 완료 기준 ③(14-m3-plan.md M3-01): 코어는 드라이버 크레이트에 의존하지 않는다 — trait 로만 부른다(결정 M3-D3).
//!
//! `cargo tree -p pw-db -e normal` 의 패키지 이름을 본다. 개발 의존성(시험이 쓰는 가짜 드라이버)은 세지 않는다.

use std::path::Path;
use std::process::Command;

#[test]
fn pw_db_does_not_depend_on_any_driver_crate() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let output = Command::new(env!("CARGO"))
        .current_dir(&workspace)
        .args([
            "tree", "--locked", "-p", "pw-db", "-e", "normal", "--prefix", "none", "--format",
            "{p}",
        ])
        .output()
        .expect("cargo runs");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "cargo tree failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let packages: Vec<&str> = stdout
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .collect();
    // 읽는 법이 맞는지: 코어가 쓰는 trait 크레이트는 보여야 한다.
    assert!(packages.contains(&"pw-db-driver"), "{stdout}");
    assert!(packages.contains(&"pw-db-types"), "{stdout}");
    let drivers: Vec<&&str> = packages
        .iter()
        .filter(|p| p.starts_with("pw-db-driver-"))
        .collect();
    assert!(
        drivers.is_empty(),
        "pw-db depends on driver crates: {drivers:?}"
    );
}
