//! 링크된 크레이트의 `rpc_method!` 선언 전부에서 `packages/rpc-client/src/generated/` 를 만든다.
//!
//! **메서드를 선언한 크레이트는 이 크레이트의 의존성이어야 하고, 아래 `use … as _;` 로 링크를 강제해야 한다.**
//! 둘 중 하나라도 빠지면 그 크레이트의 메서드는 오류 없이 생성물에서 빠진다(`pw_rpc::methods` 는 링크된 것만 본다).
//! 지금은 메서드를 선언한 크레이트가 없다 — 첫 크레이트를 더할 때 아래에 한 줄을 넣는다.

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
