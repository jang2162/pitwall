//! 저장소의 `extensions/`(내장 확장)를 바이너리에 묶는다. `OUT_DIR/builtin.rs` 에 `BuiltinFile` 표를 쓴다.
//!
//! 파일을 손으로 적은 목록 대신 폴더를 훑는 것은 내장 확장에 파일을 더할 때 이 크레이트를 함께 고치지 않게 하려는 것이다.
//!
//! `.wasm`(언어 확장의 tree-sitter 문법)은 brotli 로 압축해 묶는다 — 원본이 합쳐 약 18 MB 라 그대로 묶으면 release 번들 증가의
//! 되돌림 조건(`docs/design/12-m1-plan.md` 결정 M1-D2, +25 MB)에 닿는다. 압축은 읽을 때 푼다(`source.rs`). 빌드 스크립트는 최적화
//! 없이 돌아 압축이 느리므로(품질 5 로 약 4초) 압축본을 `OUT_DIR` 에 두고, 원본의 크기 · 수정 시각이 같으면 다시 쓰지 않는다.

use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;
use std::{env, fs};

/// brotli 품질 · 창 크기. 품질 11 은 품질 5 보다 9% 작지만 빌드 스크립트에서 수 분이 걸린다.
const BROTLI_QUALITY: u32 = 5;
const BROTLI_WINDOW: u32 = 22;

fn main() {
    let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let root = manifest_dir.join("../../extensions");
    // 폴더를 가리키면 cargo 가 그 아래 파일의 변경을 모두 본다.
    println!("cargo:rerun-if-changed={}", root.display());

    let mut files = Vec::new();
    if root.is_dir() {
        collect(&root, &root, &mut files);
    }
    files.sort();

    let mut out = String::from("pub(crate) static FILES: &[BuiltinFile] = &[\n");
    for (rel, abs) in &files {
        let size = fs::metadata(abs).unwrap().len();
        let (bytes, brotli) = if rel.ends_with(".wasm") {
            (compressed(abs, &out_dir.join("brotli").join(rel)), true)
        } else {
            (abs.clone(), false)
        };
        writeln!(
            out,
            "    BuiltinFile {{ path: {rel:?}, bytes: include_bytes!({:?}), brotli: {brotli}, size: {size} }},",
            bytes.display().to_string()
        )
        .unwrap();
    }
    out.push_str("];\n");
    fs::write(out_dir.join("builtin.rs"), out).unwrap();
}

/// `src` 의 brotli 압축본을 `dest` 에 두고 그 경로를 돌려준다. `dest.stamp` 에 원본의 크기 · 수정 시각을 적어 두고, 같으면
/// 다시 압축하지 않는다.
fn compressed(src: &Path, dest: &Path) -> PathBuf {
    let meta = fs::metadata(src).unwrap();
    let modified = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos());
    let stamp = format!("{} {modified}", meta.len());
    let stamp_path = dest.with_extension("wasm.stamp");
    if dest.is_file() && fs::read_to_string(&stamp_path).is_ok_and(|s| s == stamp) {
        return dest.to_path_buf();
    }
    let data = fs::read(src).unwrap();
    let mut packed = Vec::new();
    {
        let mut writer =
            brotli::CompressorWriter::new(&mut packed, 1 << 16, BROTLI_QUALITY, BROTLI_WINDOW);
        writer.write_all(&data).unwrap();
    }
    fs::create_dir_all(dest.parent().unwrap()).unwrap();
    fs::write(dest, packed).unwrap();
    fs::write(stamp_path, stamp).unwrap();
    dest.to_path_buf()
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
