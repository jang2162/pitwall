//! 실 서버 시험 표지(15-m4-plan.md M4-01 ⑦ — M3-01 의 `crates/pw-db-driver/live_cfg.rs` 와 같은 방식).
//!
//! 서버마다 실행 파일을 찾으면 `cfg(pw_live_lsp_<서버>)` 를 켠다. 찾는 순서: ① `PW_TEST_LSP_<서버>`(실행 파일 경로) ② 설치본 폴더
//! `PW_TEST_TOOLS_DIR` 의 `bin/<실행 파일>`. 실 서버 시험은 `#[cfg(pw_live_lsp_rust_analyzer)]` 처럼 단다 — 서버가 없으면 시험이
//! 건너뛰어지는 것이 아니라 **목록에 없다**(`#[ignore]` 는 「건너뜀」 으로 남고, 런타임에 `return` 하는 시험은 통과로 센다). 그래서
//! `cargo nextest run -E 'test(/live_lsp_rust_analyzer_/)' --no-tests=fail` 은 서버 없이 돌리면 실패한다.
//!
//! 서버를 더할 때는 아래 표, `src/live.rs` 의 `LiveServer`, `scripts/lsp-live.mjs` 의 `TARGETS`, `scripts/lsp-live.toml` 에 한 줄씩.
//! 값이 바뀌면 이 스크립트가 다시 돌고 크레이트가 다시 컴파일된다.

/// (서버 이름 — 시험 이름 · 필터 조각, 실행 파일 이름).
const SERVERS: [(&str, &str); 6] = [
    ("rust_analyzer", "rust-analyzer"),
    ("typescript_language_server", "typescript-language-server"),
    ("basedpyright", "basedpyright-langserver"),
    ("ruff", "ruff"),
    ("jdtls", "jdtls"),
    ("kotlin_lsp", "kotlin-lsp"),
];

const TOOLS_DIR: &str = "PW_TEST_TOOLS_DIR";

fn main() {
    let names: Vec<String> = SERVERS
        .iter()
        .map(|(name, _)| format!("pw_live_lsp_{name}"))
        .collect();
    println!("cargo::rustc-check-cfg=cfg({})", names.join(", "));
    println!("cargo::rerun-if-env-changed={TOOLS_DIR}");
    let tools = std::env::var_os(TOOLS_DIR).filter(|v| !v.is_empty());
    for (name, exe) in SERVERS {
        let var = format!("PW_TEST_LSP_{}", name.to_ascii_uppercase());
        println!("cargo::rerun-if-env-changed={var}");
        let explicit = std::env::var_os(&var).is_some_and(|v| !v.is_empty());
        let installed = tools.as_ref().is_some_and(|dir| {
            let path = std::path::Path::new(dir).join("bin").join(exe);
            // 폴더를 줬을 때만 그 경로를 지켜본다 — 없는 경로를 늘 지켜보면 빌드마다 이 스크립트가 다시 돈다.
            println!("cargo::rerun-if-changed={}", path.display());
            path.is_file()
        });
        if explicit || installed {
            println!("cargo::rustc-cfg=pw_live_lsp_{name}");
        }
    }
}
