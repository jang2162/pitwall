//! 저장소에 넣은 문법 C 소스(`grammars/<문법>/`)를 빌드해 정적 링크한다. crates.io 의 문법 크레이트를 쓸 수 없는 문법만 여기에
//! 있다 — 지금은 Dockerfile(출처표 `scripts/grammars.toml` 의 `vendor`). 소스는 `mise run grammars` 가 복사한 원본 그대로다.

use std::env;
use std::path::PathBuf;

fn main() {
    let dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("grammars/dockerfile");
    println!("cargo:rerun-if-changed={}", dir.display());

    let mut build = cc::Build::new();
    build
        .std("c11")
        .include(&dir)
        .file(dir.join("parser.c"))
        .file(dir.join("scanner.c"))
        // 생성된 파서 · 원본 스캐너는 고치지 않으므로 그 경고는 볼 일이 없다. crates.io 의 문법 크레이트에서 나는 같은 경고는
        // cargo 가 바깥 크레이트라 보여 주지 않는다.
        .warnings(false);
    // 파서 표의 문자 리터럴이 UTF-8 이다. 빌드 스크립트의 `cfg` 는 호스트이므로 대상은 환경 변수로 본다.
    if env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        build.flag("-utf-8");
    }
    build.compile("tree-sitter-dockerfile");
}
