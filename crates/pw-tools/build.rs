//! `overrides/*.toml`(덮어쓰기 정의)를 바이너리에 묶는다. `OUT_DIR/overrides.rs` 에 `(패키지 이름, 글)` 표를 쓴다.
//!
//! 폴더를 훑는 것은 덮어쓰기를 더하는 행(M4-18 의 jdtls)이 이 크레이트의 코드를 고치지 않게 하려는 것이다.

use std::fmt::Write as _;
use std::path::PathBuf;
use std::{env, fs};

fn main() {
    let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let dir = manifest_dir.join("overrides");
    println!("cargo:rerun-if-changed={}", dir.display());

    let mut files = Vec::new();
    for entry in fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            panic!("override file name is not UTF-8: {}", path.display());
        };
        if let Some(package) = name.strip_suffix(".toml") {
            files.push((package.to_owned(), path.clone()));
        }
    }
    files.sort();

    let mut out = String::from("pub(crate) static FILES: &[(&str, &str)] = &[\n");
    for (package, path) in &files {
        writeln!(
            out,
            "    ({package:?}, include_str!({:?})),",
            path.display().to_string()
        )
        .unwrap();
    }
    out.push_str("];\n");
    fs::write(
        PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("overrides.rs"),
        out,
    )
    .unwrap();
}
