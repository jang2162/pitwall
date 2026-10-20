//! 구문 분석 서비스: 언어 확장의 tree-sitter 문법(WASM)으로 문서를 파싱하고 화면 범위의 강조 구간을 낸다
//! (`docs/design/12-m1-plan.md` M1-10, 결정 M1-D2 · M1-D3).
//!
//! - 문법 · 쿼리: 언어 확장의 `[contributes.languages.<id>]` 의 `grammar`(WASM) · `queries`(폴더의 `highlights.scm`). Linguist
//!   이름(`linguist`)으로 언어 판별(`pw-lang` 의 `lang.detect`)과 잇는다. 내장 언어 확장은 `extensions/lang-<문법>/` 이고 그 WASM ·
//!   쿼리의 출처는 `scripts/grammars.toml` 이다.
//! - [`grammar`]: wasmtime 엔진 · 파서와 올린 문법. 처음 필요할 때 만든다.
//! - [`token`]: 캡처 이름 → 구문 토큰(테마 토큰은 `syntax.<토큰>`).
//! - [`methods`]: 문서 세션 RPC(`syntax.open` · `syntax.edit` · `syntax.highlights` · `syntax.close`). 화면의 강조(CodeMirror
//!   decoration)는 M1-17 이다.
//!
//! 언어 주입(`injections.scm` — HTML 안의 JS, Markdown 의 코드 블록 · 인라인 문법)은 아직 없다. 주입될 자리는 바깥 문법의
//! 강조만 받는다.

pub mod grammar;
mod highlight;
pub mod methods;
mod service;
mod text;
pub mod token;

pub use service::{MAX_DOCUMENTS, MAX_EARLY_EDITS, Opened, Syntax};
pub use token::SyntaxToken;

#[cfg(test)]
mod tests;
