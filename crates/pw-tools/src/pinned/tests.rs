//! 고정 표(`registry.toml`)의 검사.

use super::*;
use crate::index::Index;

fn builtin_problems(pinned: &Pinned) -> Vec<String> {
    let snapshot = Index::snapshot();
    check(
        pinned,
        &snapshot.version,
        snapshot.packages(),
        &Overrides::builtin(),
    )
}

/// 완료 기준 ⑦: 고정 판의 번들 패키지마다 `registry.toml` 에 해시가 있다.
#[test]
fn the_builtin_pinned_table_covers_every_bundled_package() {
    let pinned = Pinned::builtin();
    assert_eq!(builtin_problems(&pinned), Vec::<String>::new());
    // 번들 여섯이 모두 고정 표에 행을 가진다.
    for name in &pinned.bundled {
        assert!(
            pinned.artifacts.iter().any(|a| &a.package == name),
            "{name}"
        );
    }
    assert_eq!(
        pinned.bundled,
        [
            "basedpyright",
            "jdtls",
            "kotlin-lsp",
            "ruff",
            "rust-analyzer",
            "typescript-language-server"
        ]
    );
}

/// 완료 기준 ⑦: 하나를 지우면 검사가 실패한다(`mise run lint` 가 같은 검사를 돈다).
#[test]
fn removing_or_breaking_a_pinned_row_is_caught() {
    let pinned = Pinned::builtin();
    // 행마다 하나씩 지워 본다 — 어느 것을 지워도 잡힌다.
    for i in 0..pinned.artifacts.len() {
        let mut p = pinned.clone();
        let removed = p.artifacts.remove(i);
        let problems = builtin_problems(&p);
        assert!(
            !problems.is_empty(),
            "removing {} ({}) went unnoticed",
            removed.name,
            removed.target
        );
    }

    let mut p = pinned.clone();
    p.artifacts[0].sha256 = "ABC".into();
    assert!(builtin_problems(&p).iter().any(|e| e.contains("SHA-256")));

    let mut p = pinned.clone();
    let row = p
        .artifacts
        .iter_mut()
        .find(|a| a.package == "ruff")
        .unwrap();
    row.url = "https://example.com/other.tar.gz".into();
    assert!(
        builtin_problems(&p)
            .iter()
            .any(|e| e.contains("but the definition downloads"))
    );

    let mut p = pinned.clone();
    let mut extra = p.artifacts[0].clone();
    extra.package = "gopls".into();
    p.artifacts.push(extra);
    assert!(
        builtin_problems(&p)
            .iter()
            .any(|e| e.contains("not bundled"))
    );

    let mut p = pinned.clone();
    let mut stale = p
        .artifacts
        .iter()
        .find(|a| a.package == "ruff")
        .unwrap()
        .clone();
    stale.version = "0.0.1".into();
    p.artifacts.push(stale);
    assert!(
        builtin_problems(&p)
            .iter()
            .any(|e| e.contains("another version"))
    );

    let mut p = pinned.clone();
    p.registry.version = "2020-01-01-old-one".into();
    assert!(
        builtin_problems(&p)
            .iter()
            .any(|e| e.contains("snapshot is registry"))
    );

    let mut p = pinned.clone();
    p.bundled.push("gopls".into());
    assert!(
        builtin_problems(&p)
            .iter()
            .any(|e| e.contains("not in the snapshot"))
    );

    let mut p = pinned.clone();
    let npm = p
        .artifacts
        .iter_mut()
        .find(|a| a.dist.as_deref() == Some("typescript"))
        .unwrap();
    npm.integrity = None;
    assert!(builtin_problems(&p).iter().any(|e| e.contains("integrity")));

    let mut p = pinned.clone();
    p.artifacts[0].hash_from = "guess".into();
    assert!(builtin_problems(&p).iter().any(|e| e.contains("hash_from")));

    let mut p = pinned;
    p.artifacts[0].url = "http://example.com/x".into();
    assert!(builtin_problems(&p).iter().any(|e| e.contains("not https")));
}

#[test]
fn lookups_and_parsing() {
    let pinned = Pinned::builtin();
    let ra = Index::snapshot()
        .get("rust-analyzer")
        .unwrap()
        .version()
        .to_owned();
    let rows = pinned.artifacts_for("rust-analyzer", &ra, ToolTarget::DarwinArm64);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].name, "rust-analyzer-aarch64-apple-darwin.gz");
    // `any` 행(순수 파이썬 휠)과 그 대상의 행(Node 휠)이 함께 나온다.
    let bp = pinned.artifacts_for("basedpyright", "1.40.2", ToolTarget::LinuxX64Musl);
    assert!(
        bp.iter()
            .any(|a| a.target == "any" && a.dist.as_deref() == Some("basedpyright"))
    );
    assert!(
        bp.iter()
            .any(|a| a.target == "linux_x64_musl" && a.name.contains("musllinux"))
    );
    assert!(
        pinned
            .artifacts_for("rust-analyzer", "old", ToolTarget::DarwinArm64)
            .is_empty()
    );

    assert!(Pinned::parse("bundled = []\n[registry]\nversion = \"v\"\n").is_err());
    assert!(Pinned::parse(&format!("{PINNED_TOML}\nsurprise = 1\n")).is_err());
    assert_eq!(
        normalize_pypi("Nodejs_Wheel.Binaries"),
        "nodejs-wheel-binaries"
    );
    assert_eq!(normalize_pypi("a--b"), "a-b");
}
