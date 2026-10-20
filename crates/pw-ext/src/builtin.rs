//! 바이너리에 묶인 내장 확장(저장소의 `extensions/`). 표는 `build.rs` 가 만든다.

use crate::source::ExtensionRoot;

/// 묶인 파일 하나. 경로 구분자는 OS 와 관계없이 `/` 다.
pub(crate) struct BuiltinFile {
    /// `extensions/` 기준 경로.
    pub path: &'static str,
    /// 내용. `brotli` 면 압축본이다(`.wasm` — 이유는 `build.rs` 머리 주석).
    pub bytes: &'static [u8],
    pub brotli: bool,
    /// 원본 크기(바이트).
    pub size: u64,
}

mod generated {
    use super::BuiltinFile;

    include!(concat!(env!("OUT_DIR"), "/builtin.rs"));
}

pub(crate) fn files() -> &'static [BuiltinFile] {
    generated::FILES
}

/// 내장 확장 폴더마다 하나. 이름순이다.
pub fn roots() -> Vec<ExtensionRoot> {
    let mut folders: Vec<&'static str> = files()
        .iter()
        .filter_map(|f| f.path.split_once('/').map(|(folder, _)| folder))
        .collect();
    folders.dedup();
    folders
        .into_iter()
        .map(|folder| ExtensionRoot::Builtin { folder })
        .collect()
}
