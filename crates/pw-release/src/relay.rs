//! Relay 배포물(18-m7-plan M7-05): 자산 이름, 해시 · 서명 매니페스트(`relay-manifest.json`), 개발 빌드 갈래.
//!
//! **매니페스트를 쓰는 쪽(릴리스 워크플로)과 읽는 쪽(Desktop — M7-06 의 설치)이 이 파일 하나를 쓴다.** 꼴이 바뀌면 둘이 함께
//! 바뀐다.
//!
//! - 쓰기: Relay 빌드 잡이 바이너리마다 [`describe`] 로 항목(이름 · 해시 · 크기)을 남기고, 매니페스트 잡이 [`assemble`] 로
//!   모은다. 서명은 `sign-updater` 잡이 updater 운영 키로 한다(`tauri signer sign --app-version` — updater 산출물과 같은 도구 ·
//!   같은 키, 02 「배포 · 업데이트 (D21)」). 그 `.sig` 는 minisign 서명 파일의 base64 다.
//! - 읽기: [`verify`] 가 서명(키는 앱의 updater 공개키)과 서명에 실린 판을 본 뒤에야 매니페스트를 읽는다. 원격에 올릴 바이너리는
//!   [`Manifest::check_binary`] 를 지나야 한다 — 매니페스트에 없는 해시 · 다른 아키텍처의 해시는 거절이다.
//! - 개발 빌드(결정 M7-D5): debug Desktop 은 릴리스가 아니라 이 워크트리의 `mise run relay-build` 결과(서명 없는 매니페스트)를
//!   올린다([`source`]). **이 갈래는 debug 빌드에만 있다** — release 빌드는 환경 변수가 있어도 서명 매니페스트만 쓴다.

use std::ffi::OsStr;
use std::io::Read;
use std::path::{Path, PathBuf};

use base64::Engine;
use minisign_verify::{PublicKey, Signature};
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{Error, error};

/// 매니페스트 파일 이름(릴리스 자산 · 개발 빌드 폴더 모두). 서명은 `<이름>.sig`.
pub const MANIFEST_FILE: &str = "relay-manifest.json";

/// 매니페스트 꼴의 판. 읽는 쪽은 이 값만 받는다.
pub const SCHEMA: u32 = 1;

/// Relay 바이너리 하나의 크기 상한(메인 판정 M7-P13 「원격 디스크: 바이너리 한 판 < 56 MiB」 — 처음 40 MiB 에서 M7-05 가 잰 값
/// (`relay` 프로필 x86_64 49.1 MiB) + 여유로 고쳤다). 빌드(`relay-build` · 릴리스 잡 · `relay-live`)가 [`describe`] 에서 넘으면
/// 실패한다. 값을 바꾸는 것은 M7-P13 을 바꾸는 일이다 — 계획 문서와 함께 고친다.
pub const MAX_BINARY_BYTES: u64 = 56 * 1024 * 1024;

/// debug Desktop 이 Relay 개발 빌드를 찾는 폴더를 바꾸는 환경 변수(기본은 [`DEV_DIST_DIR`]). release 빌드는 읽지 않는다.
pub const DEV_DIST_ENV: &str = "PITWALL_RELAY_DEV_DIST";

/// `mise run relay-build` 가 쓰는 폴더(워크스페이스 기준). debug Desktop 의 기본 개발 빌드 자리다.
pub const DEV_DIST_DIR: &str = "target/relay-dist";

/// 내는 바이너리 하나: 자산 이름의 끝 조각, Rust 타깃, 그 바이너리가 도는 (OS, 아키텍처)들(결정 M7-D4 — macOS 도 아키텍처별로
/// 따로 낸다(2026-11-23 바뀜): 유니버설은 상한을 넘고 원격에는 한 아키텍처만 든다). **자산 목록은 이 표 하나다** —
/// scripts/relay-build.mjs 의 `TARGETS` 와 release.yml 의 relay 매트릭스는 이 표와 같아야 한다(시험이 견준다). OS · 아키텍처 이름은 원격 `uname -sm` 을 읽는 쪽(M7-06)이 이 값으로 바꿔 [`Manifest::asset`] 에 넘긴다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Platform {
    pub asset: &'static str,
    pub rust_target: &'static str,
    pub runs_on: &'static [(&'static str, &'static str)],
}

pub const PLATFORMS: [Platform; 4] = [
    Platform {
        asset: "linux-x86_64",
        rust_target: "x86_64-unknown-linux-musl",
        runs_on: &[("linux", "x86_64")],
    },
    Platform {
        asset: "linux-aarch64",
        rust_target: "aarch64-unknown-linux-musl",
        runs_on: &[("linux", "aarch64")],
    },
    Platform {
        asset: "macos-aarch64",
        rust_target: "aarch64-apple-darwin",
        runs_on: &[("macos", "aarch64")],
    },
    Platform {
        asset: "macos-x86_64",
        rust_target: "x86_64-apple-darwin",
        runs_on: &[("macos", "x86_64")],
    },
];

/// 그 판 · 그 플랫폼의 자산 이름 — `pitwall-relay-<판>-<플랫폼>`. 릴리스 자산은 폴더 없이 올라가므로 판이 이름에 든다.
pub fn asset_name(version: &Version, platform: &Platform) -> String {
    format!("pitwall-relay-{version}-{}", platform.asset)
}

/// 바이너리 하나의 항목. 빌드 잡이 남기고([`describe`]) 매니페스트 잡이 모은다([`assemble`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub name: String,
    /// 소문자 16진 64자.
    pub sha256: String,
    pub size: u64,
}

/// 매니페스트의 한 줄 — (OS, 아키텍처)마다 하나.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Asset {
    pub os: String,
    pub arch: String,
    pub name: String,
    pub sha256: String,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub schema: u32,
    pub version: String,
    pub assets: Vec<Asset>,
}

/// 파일의 sha256(소문자 16진).
pub fn sha256_file(path: &Path) -> Result<String, Error> {
    let mut file = std::fs::File::open(path)
        .map_err(|e| error(format!("cannot open {}: {e}", path.display())))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1 << 16];
    loop {
        let n = file
            .read(&mut buffer)
            .map_err(|e| error(format!("cannot read {}: {e}", path.display())))?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }
    Ok(hex(&hasher.finalize()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn plain_sha256(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// 빌드한 바이너리의 항목. 크기가 [`MAX_BINARY_BYTES`](`max`) 를 넘으면 오류다 — 상한을 넘는 빌드는 낼 수 없다.
pub fn describe(path: &Path, name: &str, max: u64) -> Result<Entry, Error> {
    let size = std::fs::metadata(path)
        .map_err(|e| error(format!("cannot stat {}: {e}", path.display())))?
        .len();
    if size > max {
        return Err(error(format!(
            "{} is {size} bytes ({:.1} MiB), over the relay binary limit of {max} bytes ({:.1} MiB — 18-m7-plan M7-P13)",
            path.display(),
            size as f64 / 1048576.0,
            max as f64 / 1048576.0
        )));
    }
    Ok(Entry {
        name: name.to_owned(),
        sha256: sha256_file(path)?,
        size,
    })
}

/// 항목들로 그 판의 매니페스트를 만든다. `complete`(릴리스)면 [`PLATFORMS`] 의 자산이 정확히 하나씩 있어야 한다 — 빠진 플랫폼이
/// 있는 매니페스트를 내면 그 원격에 설치할 수 없다. 개발 빌드(`complete = false`)는 만든 것만 싣는다. 모르는 이름 · 같은 이름 두 번
/// · 해시 꼴이 틀린 항목은 늘 오류다.
pub fn assemble(version: &Version, entries: &[Entry], complete: bool) -> Result<Manifest, Error> {
    for entry in entries {
        if !PLATFORMS
            .iter()
            .any(|p| asset_name(version, p) == entry.name)
        {
            return Err(error(format!(
                "{} is not a relay asset of {version}",
                entry.name
            )));
        }
        if !plain_sha256(&entry.sha256) {
            return Err(error(format!(
                "{}: bad sha256 {:?}",
                entry.name, entry.sha256
            )));
        }
    }
    let mut assets = Vec::new();
    for platform in &PLATFORMS {
        let name = asset_name(version, platform);
        let found: Vec<&Entry> = entries.iter().filter(|e| e.name == name).collect();
        let entry = match found.as_slice() {
            [one] => *one,
            [] if !complete => continue,
            [] => return Err(error(format!("no entry for {name}"))),
            _ => return Err(error(format!("more than one entry for {name}"))),
        };
        for (os, arch) in platform.runs_on {
            assets.push(Asset {
                os: (*os).to_owned(),
                arch: (*arch).to_owned(),
                name: entry.name.clone(),
                sha256: entry.sha256.clone(),
                size: entry.size,
            });
        }
    }
    Ok(Manifest {
        schema: SCHEMA,
        version: version.to_string(),
        assets,
    })
}

impl Manifest {
    /// 서명할 바이트(끝에 줄바꿈). 서명은 이 바이트 그대로에 대한 것이고, 읽는 쪽은 받은 바이트를 검증한 뒤에 읽는다.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut text = serde_json::to_string_pretty(self).expect("manifest serializes");
        text.push('\n');
        text.into_bytes()
    }

    /// 그 (OS, 아키텍처)의 자산.
    pub fn asset(&self, os: &str, arch: &str) -> Option<&Asset> {
        self.assets.iter().find(|a| a.os == os && a.arch == arch)
    }

    /// 원격에 올릴 바이너리가 이 매니페스트의 그 (OS, 아키텍처) 자산과 같은가 — 크기와 sha256 이 모두 같아야 한다. 매니페스트에
    /// 없는 해시(다른 판 · 고친 바이너리 · 다른 아키텍처의 바이너리)는 거절이다.
    pub fn check_binary(&self, os: &str, arch: &str, path: &Path) -> Result<&Asset, Error> {
        let asset = self
            .asset(os, arch)
            .ok_or_else(|| error(format!("the relay manifest has no binary for {os} {arch}")))?;
        let size = std::fs::metadata(path)
            .map_err(|e| error(format!("cannot stat {}: {e}", path.display())))?
            .len();
        if size != asset.size {
            return Err(error(format!(
                "{} is {size} bytes, the manifest says {}",
                path.display(),
                asset.size
            )));
        }
        let sha256 = sha256_file(path)?;
        if sha256 != asset.sha256 {
            return Err(error(format!(
                "{} does not match the manifest's sha256 for {os} {arch}",
                path.display()
            )));
        }
        Ok(asset)
    }

    /// 꼴 검사: 판 · 스키마 · 이름 · 해시 · 줄이 겹치지 않음.
    fn validate(&self, version: &str) -> Result<(), Error> {
        if self.schema != SCHEMA {
            return Err(error(format!(
                "relay manifest schema {} is not {SCHEMA}",
                self.schema
            )));
        }
        if self.version != version {
            return Err(error(format!(
                "the relay manifest is for {}, not {version}",
                self.version
            )));
        }
        let parsed = Version::parse(&self.version)
            .map_err(|e| error(format!("the relay manifest version is not semver: {e}")))?;
        for (i, asset) in self.assets.iter().enumerate() {
            // 이름은 그 (OS, 아키텍처)의 자산 이름 그대로여야 한다 — 읽는 쪽(M7-06)이 캐시 파일 이름으로 쓰므로 `..` · `/` 같은 글자가
            // 들어올 자리를 남기지 않는다. 표에 없는 (OS, 아키텍처)도 거절이다.
            let platform = PLATFORMS
                .iter()
                .find(|p| {
                    p.runs_on
                        .contains(&(asset.os.as_str(), asset.arch.as_str()))
                })
                .ok_or_else(|| {
                    error(format!(
                        "relay manifest line {i} is for an unknown platform {:?} {:?}",
                        asset.os, asset.arch
                    ))
                })?;
            if asset.name != asset_name(&parsed, platform) {
                return Err(error(format!(
                    "relay manifest line {i} has a bad name {:?}",
                    asset.name
                )));
            }
            if !plain_sha256(&asset.sha256) {
                return Err(error(format!("relay manifest line {i} has a bad sha256")));
            }
            if self.assets[..i]
                .iter()
                .any(|a| a.os == asset.os && a.arch == asset.arch)
            {
                return Err(error(format!(
                    "relay manifest lists {} {} twice",
                    asset.os, asset.arch
                )));
            }
        }
        Ok(())
    }
}

/// 서명된 매니페스트를 검증하고 읽는다. `signature` 는 `.sig` 파일 내용(minisign 서명 파일의 base64), `public_key` 는 앱의 updater
/// 공개키(`tauri.conf.json` 의 `plugins.updater.pubkey` — minisign 공개키 파일의 base64), `version` 은 이 Desktop 의 판이다.
///
/// 순서가 판정이다: 서명(전역 서명이 덮는 trusted comment 포함)을 본 **뒤에야** trusted comment 의 판과 매니페스트를 읽는다.
/// 서명에 판이 없거나(`version:` 칸) 다르면 거절한다 — 옛 판의 서명된 매니페스트를 새 판 이름으로 내밀지 못하게(updater 의
/// `requireSignedVersion` 과 같은 까닭).
pub fn verify(
    manifest: &[u8],
    signature: &str,
    public_key: &str,
    version: &str,
) -> Result<Manifest, Error> {
    let decode = |what: &str, b64: &str| -> Result<String, Error> {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(b64.trim())
            .map_err(|e| error(format!("the relay manifest {what} is not base64: {e}")))?;
        String::from_utf8(bytes)
            .map_err(|_| error(format!("the relay manifest {what} is not text")))
    };
    let public_key = PublicKey::decode(&decode("public key", public_key)?)
        .map_err(|e| error(format!("bad updater public key: {e}")))?;
    let signature = Signature::decode(&decode("signature", signature)?)
        .map_err(|e| error(format!("bad relay manifest signature: {e}")))?;
    // 옛 꼴(prehash 없는 서명)은 받지 않는다 — Tauri 서명 도구는 prehash 꼴만 낸다.
    public_key
        .verify(manifest, &signature, false)
        .map_err(|e| error(format!("the relay manifest signature does not verify: {e}")))?;
    let field = |key: &str| {
        signature
            .trusted_comment()
            .split('\t')
            .find_map(|field| field.strip_prefix(key))
    };
    // 같은 운영 키 · 같은 판으로 서명한 updater 산출물의 `.sig` 를 매니페스트의 서명으로 내밀지 못하게 — 서명 도구가 적는 파일 이름이
    // 매니페스트의 것이어야 한다.
    if field("file:") != Some(MANIFEST_FILE) {
        return Err(error(format!(
            "the relay manifest signature is for another file ({:?})",
            field("file:").unwrap_or("")
        )));
    }
    let signed = field("version:")
        .ok_or_else(|| error("the relay manifest signature carries no version"))?;
    if signed != version {
        return Err(error(format!(
            "the relay manifest was signed for {signed}, not {version}"
        )));
    }
    let parsed: Manifest = serde_json::from_slice(manifest)
        .map_err(|e| error(format!("the relay manifest is not valid: {e}")))?;
    parsed.validate(version)?;
    Ok(parsed)
}

/// 개발 빌드의 매니페스트(서명 없음)를 읽는다. [`DevDist`] 는 [`source`] 만 만들고 `source` 는 release 빌드에서 늘
/// [`Source::Signed`] 이므로 release 빌드에서는 부를 길이 없다(판정 `source_for` 는 이 모듈 밖에 공개하지 않는다).
pub fn read_dev_manifest(dist: &DevDist, version: &str) -> Result<Manifest, Error> {
    let path = dist.0.join(MANIFEST_FILE);
    let bytes = std::fs::read(&path).map_err(|e| {
        error(format!(
            "cannot read {}: {e} (run `mise run relay-build`)",
            path.display()
        ))
    })?;
    let parsed: Manifest = serde_json::from_slice(&bytes)
        .map_err(|e| error(format!("{} is not valid: {e}", path.display())))?;
    parsed.validate(version)?;
    Ok(parsed)
}

/// 개발 빌드 폴더. 밖에서 만들 수 없다 — [`source`] 가 debug 빌드에서만 낸다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DevDist(PathBuf);

impl DevDist {
    pub fn dir(&self) -> &Path {
        &self.0
    }
}

/// Desktop 이 원격에 올릴 Relay 를 어디서 가져오는가.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// 이 워크트리의 개발 빌드(`mise run relay-build`) — debug 빌드만.
    Dev(DevDist),
    /// 릴리스 자산을 받아 서명 매니페스트로 검증한다([`verify`]).
    Signed,
}

/// 이 빌드의 Relay 출처. **release 빌드(`debug_assertions` 꺼짐)는 늘 [`Source::Signed`]** 다 — 환경 변수 · 설정으로 개발 빌드
/// 갈래를 켜는 길이 없다(desktop `instance.rs` 의 `TEST_KEY_ENV` 와 같은 꼴). debug 빌드는 [`DEV_DIST_ENV`] 가 있으면 그 폴더, 없으면
/// 이 워크스페이스의 [`DEV_DIST_DIR`].
pub fn source() -> Source {
    source_for(
        DEV_BUILDS,
        std::env::var_os(DEV_DIST_ENV).as_deref(),
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."),
    )
}

/// 이 빌드에 개발 빌드 갈래가 있는가 — debug 빌드만. release 빌드에서 이 값이 참이 되면 `relay` 통합 시험의
/// `a_release_build_ignores_the_dev_build_variable` 이 `mise run test-release` 에서 실패한다.
const DEV_BUILDS: bool = cfg!(debug_assertions);

/// [`source`] 의 판정. `debug` 는 부르는 빌드가 debug 인가. 모듈 밖에 공개하지 않는다 — `debug = true` 를 넘겨 release 빌드에서
/// [`DevDist`] 를 얻는 길이 없게.
fn source_for(debug: bool, env: Option<&OsStr>, workspace: &Path) -> Source {
    if !debug {
        return Source::Signed;
    }
    let dir = match env.filter(|v| !v.is_empty()) {
        Some(dir) => PathBuf::from(dir),
        None => workspace.join(DEV_DIST_DIR),
    };
    Source::Dev(DevDist(dir))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **release 판정은 환경 변수가 있어도 개발 빌드 갈래로 가지 않는다**(M7-05 ② — desktop `instance.rs` 의 `TEST_KEY_ENV` 와 같은
    /// 꼴). `source` 가 넘기는 값([`DEV_BUILDS`])은 release 로 도는 통합 시험이 본다.
    #[test]
    fn the_release_branch_never_takes_the_dev_build() {
        let ws = Path::new("/ws");
        let dir = Path::new("/elsewhere/relay-dist");
        assert_eq!(source_for(false, Some(dir.as_os_str()), ws), Source::Signed);
        assert_eq!(source_for(false, None, ws), Source::Signed);
        let Source::Dev(dist) = source_for(true, Some(dir.as_os_str()), ws) else {
            panic!("debug follows the variable");
        };
        assert_eq!(dist.dir(), dir);
        let Source::Dev(dist) = source_for(true, Some("".as_ref()), ws) else {
            panic!("an empty variable is unset");
        };
        assert_eq!(dist.dir(), ws.join(DEV_DIST_DIR));
        assert_eq!(DEV_BUILDS, cfg!(debug_assertions));
    }
}
