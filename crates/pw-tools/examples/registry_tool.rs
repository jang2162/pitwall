//! registry 고정 표의 도구(`scripts/tools-registry-bump.mjs` · `mise run lint` 가 부른다).
//!
//! - `plan <snapshot.json>`: 스냅숏의 기본 번들 패키지를 여섯 대상마다 풀어(덮어쓰기 포함) 받을 것을 JSON 으로 낸다 — 스크립트가
//!   이것을 받아 해시를 적는다(풀기 규칙을 JS 로 다시 쓰지 않게).
//! - `check [<registry.toml> <snapshot.json>]`: 고정 표가 번들 패키지의 (판, 대상, 파일)을 모두 덮는지(`pw_tools::pinned::check`).
//!   인자가 없으면 이 크레이트의 파일을 디스크에서 읽는다(빌드에 묶인 것이 아니라 지금 것을).
//! - `stats <registry.json> <판>`: 대상마다 지원 · 지원하지 않음(까닭별) 수.
//! - `unzip <registry.json.zip> <registry.json>`: 앱과 같은 코드(`pw_tools::source::unzip_registry`)로 꺼낸다.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use pw_tools::index::Index;
use pw_tools::overrides::Overrides;
use pw_tools::pinned::{self, Pinned};
use pw_tools::target::ToolTarget;
use serde_json::json;

fn crate_file(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(rel)
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| {
        eprintln!("{}: {e}", path.display());
        std::process::exit(2)
    })
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("plan") if args.len() == 2 => plan(Path::new(&args[1])),
        Some("check") if args.len() == 1 || args.len() == 3 => {
            let (toml, snap) = if args.len() == 3 {
                (PathBuf::from(&args[1]), PathBuf::from(&args[2]))
            } else {
                (
                    crate_file("registry.toml"),
                    crate_file("snapshot/bundled.json"),
                )
            };
            check(&toml, &snap)
        }
        Some("stats") if args.len() == 3 => stats(Path::new(&args[1]), &args[2]),
        Some("unzip") if args.len() == 3 => {
            let zip = std::fs::read(&args[1]).unwrap_or_else(|e| {
                eprintln!("{}: {e}", args[1]);
                std::process::exit(2)
            });
            match pw_tools::source::unzip_registry(&zip) {
                Ok(json) => {
                    std::fs::write(&args[2], json).unwrap();
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("{e}");
                    ExitCode::FAILURE
                }
            }
        }
        _ => {
            eprintln!(
                "usage: registry_tool plan <snapshot.json> | check [<registry.toml> <snapshot.json>] | stats <registry.json> <version> | unzip <zip> <out>"
            );
            ExitCode::from(2)
        }
    }
}

fn plan(snapshot: &Path) -> ExitCode {
    let index = Index::from_snapshot_json(&read(snapshot)).unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(2)
    });
    let overrides = Overrides::builtin();
    let mut rows = Vec::new();
    for p in index.packages() {
        for t in ToolTarget::ALL {
            let row = match p.resolve(Some(t), overrides.get(&p.name)) {
                Ok(r) => json!({"package": p.name, "target": t, "resolved": r}),
                Err(e) => json!({"package": p.name, "target": t, "unsupported": e}),
            };
            rows.push(row);
        }
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({"registry": index.version, "rows": rows})).unwrap()
    );
    ExitCode::SUCCESS
}

fn check(toml: &Path, snapshot: &Path) -> ExitCode {
    let pinned = match Pinned::parse(&read(toml)) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{}: {e}", toml.display());
            return ExitCode::FAILURE;
        }
    };
    let index = match Index::from_snapshot_json(&read(snapshot)) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("{}: {e}", snapshot.display());
            return ExitCode::FAILURE;
        }
    };
    let problems = pinned::check(
        &pinned,
        &index.version,
        index.packages(),
        &Overrides::builtin(),
    );
    if problems.is_empty() {
        println!(
            "registry {}: {} bundled packages, {} pinned files",
            pinned.registry.version,
            pinned.bundled.len(),
            pinned.artifacts.len()
        );
        ExitCode::SUCCESS
    } else {
        for p in &problems {
            eprintln!("{}: {p}", toml.display());
        }
        eprintln!(
            "fix: node scripts/tools-registry-bump.mjs bump {}",
            pinned.registry.version
        );
        ExitCode::FAILURE
    }
}

fn stats(registry_json: &Path, version: &str) -> ExitCode {
    let bytes = std::fs::read(registry_json).unwrap_or_else(|e| {
        eprintln!("{}: {e}", registry_json.display());
        std::process::exit(2)
    });
    let index =
        match Index::from_registry_json(&bytes, version, pw_tools::RegistryOrigin::Cache, None) {
            Ok(i) => i,
            Err(e) => {
                eprintln!("{e}");
                return ExitCode::FAILURE;
            }
        };
    let overrides = Overrides::builtin();
    println!(
        "registry {version}: {} packages, {} unreadable",
        index.packages().len(),
        index.unreadable
    );
    for t in ToolTarget::ALL {
        let s = index.stats(Some(t), &overrides);
        println!(
            "{t}: supported {} unsupported {} {:?}",
            s.supported, s.unsupported, s.by_reason
        );
    }
    ExitCode::SUCCESS
}
