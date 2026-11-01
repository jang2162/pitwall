//! 실 서버 시험 도우미(15-m4-plan.md M4-01 ⑦ — M3 의 `pw_db_driver::live` 와 같은 규칙).
//!
//! **시험을 다는 법**: `#[cfg(pw_live_lsp_rust_analyzer)]`(TS 는 `pw_live_lsp_typescript_language_server`)를 달고 함수 이름을
//! `live_lsp_<서버>_` 로 시작한다 — `scripts/lsp-live.mjs` 의 필터(`test(/(^|::)live_lsp_<서버>_/)`)는 이름 조각의 처음만 보고, 실
//! 서버 없이 도는 시험은 이 접두어로 시작하지 않는다. 서버가 없으면 시험이 목록에 없다(빌드 스크립트 `live_cfg.rs` 머리 주석).
//! **live 시험은 `pw-lsp` 크레이트에만 둔다** — 표지는 이 크레이트의 빌드 스크립트가 켜고, `scripts/lsp-live.mjs` 는 `pw-lsp` 만
//! 빌드해 돌린다. 실행 파일 경로는 [`LiveServer::program`], 띄울 인자는 [`LiveServer::args`].
//!
//! 서버 하한 · 기대 능력 표는 `scripts/lsp-live.toml`(값은 M4-18). 기대 능력은 `tests/live_lsp.rs` 의 initialize 시험이 본다.

use std::ffi::OsString;
use std::path::PathBuf;

/// 실 서버 시험의 서버.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveServer {
    RustAnalyzer,
    TypescriptLanguageServer,
}

impl LiveServer {
    pub const ALL: [Self; 2] = [Self::RustAnalyzer, Self::TypescriptLanguageServer];

    /// 시험 이름 · 필터 · `scripts/lsp-live.toml` 의 키.
    pub fn name(self) -> &'static str {
        match self {
            Self::RustAnalyzer => "rust_analyzer",
            Self::TypescriptLanguageServer => "typescript_language_server",
        }
    }

    /// 서버 id(정의의 id — 로그 파일 이름).
    pub fn server_id(self) -> &'static str {
        self.executable()
    }

    /// 실행 파일 이름.
    pub fn executable(self) -> &'static str {
        match self {
            Self::RustAnalyzer => "rust-analyzer",
            Self::TypescriptLanguageServer => "typescript-language-server",
        }
    }

    /// 실행 파일 경로를 주는 환경 변수 `PW_TEST_LSP_<서버>`.
    pub fn env(self) -> String {
        format!("PW_TEST_LSP_{}", self.name().to_ascii_uppercase())
    }

    /// stdio 로 띄우는 인자.
    pub fn args(self) -> Vec<OsString> {
        match self {
            Self::RustAnalyzer => Vec::new(),
            Self::TypescriptLanguageServer => vec!["--stdio".into()],
        }
    }

    /// 실행 파일 경로: `PW_TEST_LSP_<서버>`, 없으면 `PW_TEST_TOOLS_DIR/bin/<실행 파일>`. 표지(`cfg(pw_live_lsp_*)`)가 켜진 시험에서만
    /// 부른다 — 빌드 때 있던 것이 실행 때 없으면 패닉한다.
    pub fn program(self) -> PathBuf {
        if let Some(path) = std::env::var_os(self.env()).filter(|v| !v.is_empty()) {
            return PathBuf::from(path);
        }
        if let Some(dir) = std::env::var_os("PW_TEST_TOOLS_DIR").filter(|v| !v.is_empty()) {
            let path = PathBuf::from(dir).join("bin").join(self.executable());
            if path.is_file() {
                return path;
            }
        }
        panic!(
            "{} is not set and PW_TEST_TOOLS_DIR has no bin/{} (the live test was built with it)",
            self.env(),
            self.executable()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_follow_the_filter_and_env_rules() {
        assert_eq!(LiveServer::RustAnalyzer.env(), "PW_TEST_LSP_RUST_ANALYZER");
        assert_eq!(
            LiveServer::TypescriptLanguageServer.env(),
            "PW_TEST_LSP_TYPESCRIPT_LANGUAGE_SERVER"
        );
        for server in LiveServer::ALL {
            assert!(
                server
                    .name()
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b == b'_')
            );
        }
    }
}
