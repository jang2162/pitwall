//! 받은 배포물 안의 jar 를 Pitwall 이 고친 클래스로 바꾸는 패치(06-roadmap.md 결정 D71, 17-m6-plan.md M6-19).
//!
//! 지금은 하나다 — java-debug(`java-debug-adapter`)의 `JavaDebugServer` 를 127.0.0.1 에만 묶는다. 상류 서버는 모든 주소에서
//! 인증 없이 연결을 받고 jdtls 가 끝날 때까지 열려 있어, 같은 네트워크의 기기가 `launch` 로 사용자 권한의 임의 실행을 할 수 있다.
//!
//! 규칙:
//! - 바꿀 클래스 바이트는 `crates/pw-tools/patches/<패치>/classes/` 의 것을 바이너리에 묶는다. 그 바이트는 같은 폴더의 소스를
//!   `scripts/java-debug-patch.mjs` 가 고정 JDK 로 컴파일한 것이고, 그 스크립트의 `check` 가 다시 컴파일해 같은지 본다.
//! - 다시 묶기([`patch_jar`])는 결정적이다: 바꾸지 않는 항목은 압축된 바이트 그대로 옮기고(`raw_copy_file`), 바꾸는 항목은
//!   원래 자리 · 시각 · 권한으로 압축하지 않고(stored) 쓴다. 그래서 결과 jar 의 SHA-256 을 `registry.toml` 의 `[[patched]]` 에
//!   고정할 수 있고, 설치기가 그 값과 대조한다 — 다르면 설치가 실패하고 원본 jar 를 쓰지 않는다(fail-closed).
//! - 패치가 있는 패키지는 그 판의 `[[patched]]` 행이 없으면 설치하지 않는다 — 원본을 조용히 설치하는 길이 없다.

use std::io::{Cursor, Read, Write};

use sha2::{Digest, Sha256};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

/// 받은 배포물 안 jar 하나를 바꾸는 패치.
#[derive(Debug)]
pub struct PatchSet {
    /// `registry.toml` 의 `[[patched]] patch` 가 가리키는 이름.
    pub name: &'static str,
    /// Mason 패키지 이름. 이 패키지는 패치 없이 설치하지 않는다.
    pub package: &'static str,
    /// jar 안의 항목 이름 → 바꿀 바이트. jar 에 그 이름이 모두 있어야 한다(없는 이름을 더하지 않는다).
    pub entries: &'static [(&'static str, &'static [u8])],
    /// 바꾼 jar 옆에 쓰는 수정 고지의 파일 이름과 글(EPL-1.0 — 어느 파일을 무엇을 왜).
    pub notice_name: &'static str,
    pub notice: &'static str,
}

const JAVA_DEBUG_PREFIX: &str = "com/microsoft/java/debug/plugin/internal/";

/// java-debug 를 127.0.0.1 에만 묶는 패치(결정 D71).
pub static JAVA_DEBUG_LOOPBACK: PatchSet = PatchSet {
    name: "java-debug-loopback",
    package: "java-debug-adapter",
    entries: &[
        (
            "com/microsoft/java/debug/plugin/internal/JavaDebugServer.class",
            include_bytes!("../patches/java-debug/classes/JavaDebugServer.class"),
        ),
        (
            "com/microsoft/java/debug/plugin/internal/JavaDebugServer$1.class",
            include_bytes!("../patches/java-debug/classes/JavaDebugServer$1.class"),
        ),
        (
            "com/microsoft/java/debug/plugin/internal/JavaDebugServer$2.class",
            include_bytes!("../patches/java-debug/classes/JavaDebugServer$2.class"),
        ),
    ],
    notice_name: "NOTICE-pitwall-java-debug.txt",
    notice: include_str!("../patches/java-debug/NOTICE.txt"),
};

/// 모든 패치.
pub static ALL: &[&PatchSet] = &[&JAVA_DEBUG_LOOPBACK];

/// 그 패키지의 패치.
pub fn for_package(package: &str) -> Option<&'static PatchSet> {
    ALL.iter().copied().find(|p| p.package == package)
}

/// 이름으로 찾는다.
pub fn by_name(name: &str) -> Option<&'static PatchSet> {
    ALL.iter().copied().find(|p| p.name == name)
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for b in digest {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// jar 의 항목 중 패치의 이름을 바꿔 다시 묶는다. 패치의 이름이 jar 에 하나라도 없거나, 같은 클래스 가족(`JavaDebugServer$…`)의
/// 다른 항목이 jar 에 있으면 실패한다 — 상류 판이 바뀌어 클래스 구성이 달라진 것이다.
pub fn patch_jar(input: &[u8], set: &PatchSet) -> Result<Vec<u8>, String> {
    let mut archive =
        ZipArchive::new(Cursor::new(input)).map_err(|e| format!("not a zip archive: {e}"))?;
    let names: Vec<String> = archive
        .file_names()
        .map(|n| n.map(|n| n.into_owned()))
        .collect::<Result<_, _>>()
        .map_err(|e| format!("entry name: {e}"))?;
    for (name, _) in set.entries {
        if !names.iter().any(|n| n == name) {
            return Err(format!("{name} is not in the jar"));
        }
    }
    if set.name == JAVA_DEBUG_LOOPBACK.name {
        let family = format!("{JAVA_DEBUG_PREFIX}JavaDebugServer$");
        for n in &names {
            if n.starts_with(&family) && !set.entries.iter().any(|(e, _)| e == n) {
                return Err(format!(
                    "{n} is in the jar but not in the patch — the upstream class changed"
                ));
            }
        }
    }

    let mut out = ZipWriter::new(Cursor::new(Vec::with_capacity(input.len())));
    for i in 0..archive.len() {
        let raw = archive
            .by_index_raw(i)
            .map_err(|e| format!("entry {i}: {e}"))?;
        let name = raw
            .name()
            .map_err(|e| format!("entry {i}: {e}"))?
            .into_owned();
        match set.entries.iter().find(|(e, _)| *e == name) {
            None => out
                .raw_copy_file(raw)
                .map_err(|e| format!("copy {name}: {e}"))?,
            Some((_, bytes)) => {
                let mut options =
                    SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
                if let Some(t) = raw.last_modified() {
                    options = options.last_modified_time(t);
                }
                if let Some(mode) = raw.unix_mode() {
                    options = options.unix_permissions(mode);
                }
                drop(raw);
                out.start_file(name.as_str(), options)
                    .map_err(|e| format!("write {name}: {e}"))?;
                out.write_all(bytes)
                    .map_err(|e| format!("write {name}: {e}"))?;
            }
        }
    }
    let cursor = out.finish().map_err(|e| format!("finish: {e}"))?;
    Ok(cursor.into_inner())
}

/// jar 의 항목 하나를 푼다(시험 · 도구용).
pub fn read_entry(jar: &[u8], name: &str) -> Result<Vec<u8>, String> {
    let mut archive = ZipArchive::new(Cursor::new(jar)).map_err(|e| e.to_string())?;
    let mut file = archive.by_name(name).map_err(|e| format!("{name}: {e}"))?;
    let mut out = Vec::new();
    file.read_to_end(&mut out).map_err(|e| e.to_string())?;
    Ok(out)
}

#[cfg(test)]
pub(crate) mod tests;
