//! 실 어댑터 시험 표지(17-m6-plan.md M6-19 `dap-live` — `crates/pw-lsp/live_cfg.rs` 와 같은 방식).
//!
//! 어댑터마다 `PW_TEST_DAP_<어댑터>` 가 있으면 `cfg(pw_live_dap_<어댑터>)` 를 켠다. 실 어댑터 시험은 그 표지를 단다 — 어댑터가 없으면
//! 시험이 건너뛰어지는 것이 아니라 **목록에 없다**(그래서 `--no-tests=fail` 이 0개를 실패로 잡는다 — `scripts/dap-live.mjs`).
//!
//! 값(설치기 `tools_install` 이 받은 경로 — `scripts/dap-live.mjs` 가 준다):
//! - `PW_TEST_DAP_CODELLDB` · `PW_TEST_DAP_JS_DEBUG` · `PW_TEST_DAP_DEBUGPY`: 어댑터 실행 파일(설치본의 실행 파일 · 런처)
//! - `PW_TEST_DAP_JAVA_DEBUG`: 패치한 java-debug jar(결정 D71). 함께 `PW_TEST_DAP_JAVA_DEBUG_UPSTREAM`(상류 그대로의 jar — 구분력
//!   시험) · `PW_TEST_LSP_JDTLS`(jdtls 실행 파일)가 있어야 한다
//!
//! 어댑터를 더할 때는 아래 표, `tests/live_dap.rs`, `scripts/dap-live.mjs` 의 `TARGETS`, `scripts/dap-live.toml` 에 한 줄씩.

const ADAPTERS: [&str; 4] = ["codelldb", "js_debug", "debugpy", "java_debug"];

fn main() {
    let names: Vec<String> = ADAPTERS
        .iter()
        .map(|name| format!("pw_live_dap_{name}"))
        .collect();
    println!("cargo::rustc-check-cfg=cfg({})", names.join(", "));
    for name in ADAPTERS {
        let var = format!("PW_TEST_DAP_{}", name.to_ascii_uppercase());
        println!("cargo::rerun-if-env-changed={var}");
        if std::env::var_os(&var).is_some_and(|v| !v.is_empty()) {
            println!("cargo::rustc-cfg=pw_live_dap_{name}");
        }
    }
}
