//! 저장소의 `extensions/`(내장 확장)를 바이너리에 묶는다. `OUT_DIR/builtin.rs` 에 `(경로, 바이트)` 표를 쓴다.
//!
//! 파일을 손으로 적은 목록 대신 폴더를 훑는 것은 내장 확장에 파일을 더할 때 이 크레이트를 함께 고치지 않게 하려는 것이다.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::{env, fs};

fn main() {
    let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let root = manifest_dir.join("../../extensions");
    // 폴더를 가리키면 cargo 가 그 아래 파일의 변경을 모두 본다.
    println!("cargo:rerun-if-changed={}", root.display());

    let mut files = Vec::new();
    if root.is_dir() {
        collect(&root, &root, &mut files);
    }
    files.sort();

    let mut out = String::from("pub(crate) static FILES: &[(&str, &[u8])] = &[\n");
    for (rel, abs) in &files {
        writeln!(
            out,
            "    ({rel:?}, include_bytes!({:?})),",
            abs.display().to_string()
        )
        .unwrap();
    }
    out.push_str("];\n");
    let dest = PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("builtin.rs");
    fs::write(dest, out).unwrap();
}

/// 묶지 않는 것: `.` 으로 시작하는 항목(`.DS_Store` 등), `~` 로 끝나는 편집기 백업, `node_modules`(`extensions/*` 는 pnpm
/// 워크스페이스에도 들어 있어 폴더에 `package.json` 이 생기면 설치본이 그 안에 깔린다 — pnpm-workspace.yaml), 심볼릭 링크
/// (확장 폴더 밖이나 자기 자신을 가리킬 수 있다 — 레지스트리도 링크로 폴더 밖에 닿는 경로를 거절한다).
/// 이름이 UTF-8 이 아니면 빌드를 멈춘다 — 매니페스트의 경로(UTF-8 문자열)로 가리킬 수 없는 파일이다.
fn collect(root: &Path, dir: &Path, files: &mut Vec<(String, PathBuf)>) {
    for entry in fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            panic!("extension file name is not UTF-8: {}", path.display());
        };
        let kind = entry.file_type().unwrap();
        if name.starts_with('.')
            || name.ends_with('~')
            || name == "node_modules"
            || kind.is_symlink()
        {
            continue;
        }
        if kind.is_dir() {
            collect(root, &path, files);
        } else {
            let rel = path.strip_prefix(root).unwrap();
            let rel = rel
                .components()
                .map(|c| c.as_os_str().to_str().unwrap())
                .collect::<Vec<_>>()
                .join("/");
            files.push((rel, path.canonicalize().unwrap()));
        }
    }
}
