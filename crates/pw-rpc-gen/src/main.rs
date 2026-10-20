//! 링크된 크레이트의 `rpc_method!` 선언 전부에서 `packages/rpc-client/src/generated/` 를 만든다.
//!
//! **메서드를 선언한 크레이트는 두 곳에 더한다 — 이 크레이트의 `Cargo.toml` 의존성과, 아래의 `use <크레이트> as _;`.**
//! 의존성만 있고 `use` 가 없으면 링커가 그 크레이트를 버려 메서드가 오류 없이 생성물에서 빠지고, `--check` 도 통과한다
//! (`pw_rpc::methods` 는 링크된 것만 본다). 예: `use pw_core as _;`
//! 빠뜨리면 `apps/desktop/src-tauri` 의 「생성물 == 프로덕션 Router」 테스트가 실패한다.

use std::process::ExitCode;

// 메서드를 선언한 크레이트. 링크를 강제하는 줄이다 — 지우면 그 크레이트의 메서드가 생성물에서 조용히 빠진다.
use pw_core as _;
use pw_diag as _;
use pw_ext as _;
use pw_i18n as _;
use pw_open_with as _;
use pw_runtime as _;

use pw_rpc_gen::{Output, emit, json_schema, or_exit, typescript};

fn main() -> ExitCode {
    let methods = pw_rpc::methods();
    let ts = match or_exit(typescript(&methods)) {
        Ok(ts) => ts,
        Err(code) => return code,
    };
    emit(&[
        Output {
            path: "packages/rpc-client/src/generated/rpc.ts",
            contents: ts,
        },
        Output {
            path: "packages/rpc-client/src/generated/schema.json",
            contents: json_schema(&methods),
        },
    ])
}
