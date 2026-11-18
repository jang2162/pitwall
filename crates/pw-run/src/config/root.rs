//! 구성의 상대 경로(`cwd` · `env_file` · compose `file`)가 프로젝트 루트 안으로 풀리는가.
//!
//! 판정은 **디스크의 실제 경로**로 한다 — 루트와 경로를 모두 `canonicalize`(심링크를 따르고, 대소문자를 가리지 않는 파일
//! 시스템에서는 디스크의 대소문자로 바꾼다)한 뒤 경로 조각 단위로 앞부분을 비교한다. 그래서 `..` 로 올라가기, 루트 안의
//! 심링크가 밖을 가리키기, 대소문자만 바꿔 다른 폴더를 가리키기(`../PROJ-other`)가 모두 같은 판정을 지난다. 글자 비교(대소문자
//! 무시 포함)는 하지 않는다.
//!
//! 아직 없는 경로(빌드가 만들 폴더 · 아직 만들지 않은 `.env.local`)는 받는다 — 있는 가장 깊은 조상을 실제 경로로 바꾸고 나머지
//! 조각을 붙여 본다. 나머지에 `..` 가 있거나 나머지의 첫 조각이 대상 없는 심링크면 풀 수 없어 거절한다(나중에 대상이 생기면
//! 밖일 수 있다). 실행하는 그때(M6-02 · M6-03)도 이 함수로 다시 본다 — 읽은 뒤 링크가 바뀔 수 있다.

use std::fs;
use std::path::{Component, Path, PathBuf};

/// `rel`(프로젝트 루트 기준 상대 경로)이 `root` 안으로 풀리면 그 실제 경로. 아니면 까닭(「`<rel>` …」 꼴의 글 — 부르는 쪽이
/// 앞에 필드 이름을 붙인다).
pub fn inside(root: &Path, rel: &str) -> Result<PathBuf, String> {
    if rel.contains('\0') {
        return Err(format!("{rel:?} contains a NUL character"));
    }
    let path = Path::new(rel);
    if path.has_root() || path.components().any(|c| matches!(c, Component::Prefix(_))) {
        return Err(format!(
            "{rel:?} must be relative to the project root, not an absolute path"
        ));
    }
    let real_root = fs::canonicalize(root)
        .map_err(|e| format!("{rel:?}: cannot resolve the project root: {e}"))?;
    let parts: Vec<Component<'_>> = path.components().collect();
    let mut resolved = None;
    for keep in (0..=parts.len()).rev() {
        let prefix: PathBuf = std::iter::once(root.as_os_str())
            .chain(parts[..keep].iter().map(|c| c.as_os_str()))
            .collect();
        if let Ok(real) = fs::canonicalize(&prefix) {
            resolved = Some((real, keep));
            break;
        }
    }
    let (mut real, keep) =
        resolved.ok_or_else(|| format!("{rel:?}: cannot resolve the project root"))?;
    if keep < parts.len() {
        let first: PathBuf = std::iter::once(root.as_os_str())
            .chain(parts[..=keep].iter().map(|c| c.as_os_str()))
            .collect();
        if fs::symlink_metadata(&first).is_ok() {
            return Err(format!(
                "{rel:?} goes through {}, a link whose target does not exist",
                first.display()
            ));
        }
        for part in &parts[keep..] {
            match part {
                Component::Normal(name) => real.push(name),
                Component::CurDir => {}
                _ => {
                    return Err(format!(
                        "{rel:?} uses `..` below a folder that does not exist yet"
                    ));
                }
            }
        }
    }
    if real.starts_with(&real_root) {
        // Windows 의 `canonicalize` 는 verbatim 경로(`\\?\C:\…`)를 준다 — 작업 폴더 · `-f` 로 넘기면 `cmd.exe` 가 받지 않고, 레지스트리의
        // 루트(보통 경로)와 앞부분 비교가 어긋난다.
        Ok(pw_core::registry::path::simplify(real))
    } else {
        Err(format!(
            "{rel:?} resolves to {}, outside the project root {}",
            real.display(),
            real_root.display()
        ))
    }
}
