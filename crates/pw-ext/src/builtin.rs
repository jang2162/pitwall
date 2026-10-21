//! 바이너리에 묶인 내장 확장(저장소의 `extensions/`). 표는 `build.rs` 가 만든다.

use crate::source::ExtensionRoot;

mod generated {
    include!(concat!(env!("OUT_DIR"), "/builtin.rs"));
}

/// `(extensions/ 기준 경로, 내용)`. 경로 구분자는 OS 와 관계없이 `/` 다.
pub(crate) fn files() -> &'static [(&'static str, &'static [u8])] {
    generated::FILES
}

/// 내장 확장 폴더마다 하나. 이름순이다.
pub fn roots() -> Vec<ExtensionRoot> {
    let mut folders: Vec<&'static str> = files()
        .iter()
        .filter_map(|(path, _)| path.split_once('/').map(|(folder, _)| folder))
        .collect();
    folders.dedup();
    folders
        .into_iter()
        .map(|folder| ExtensionRoot::Builtin { folder })
        .collect()
}
