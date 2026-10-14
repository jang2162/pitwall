//! 링크된 크레이트의 `rpc_method!` 선언 전부에서 `packages/rpc-client/src/generated/` 를 만든다.
//!
//! **메서드를 선언한 크레이트는 두 곳에 더한다 — 이 크레이트의 `Cargo.toml` 의존성과, 아래의 `use <크레이트> as _;`.**
//! 의존성만 있고 `use` 가 없으면 링커가 그 크레이트를 버려 메서드가 오류 없이 생성물에서 빠지고, `--check` 도 통과한다
//! (`pw_rpc::methods` 는 링크된 것만 본다). 예: `use pw_core as _;`
//! 지금은 메서드를 선언한 크레이트가 없다. 빠뜨림을 잡는 「생성물 == 프로덕션 Router」 테스트는 첫 크레이트(M0-07)와 함께 들어온다.

use std::process::ExitCode;

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
