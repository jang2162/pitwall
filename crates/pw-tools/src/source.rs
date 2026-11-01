//! registry 가져오기와 캐시(15-m4-plan.md M4-03 ① · 결정 M4-D16).
//!
//! 릴리스 산출물(확인 2026-11-02): `registry.json.zip`(그 안에 `registry.json` 하나)과 `checksums.txt`(`sha256sum` 꼴 —
//! `registry.json` · `registry.json.zip` 두 줄). 태그는 `YYYY-MM-DD-<낱말>-<낱말>` 이다.
//!
//! - **고정**(`[tools] registry = "pinned"`, 기본): `registry.toml` 의 판 · 주소 · 해시로 받는다. zip 과 그 안 `registry.json` 의
//!   해시가 고정 값과 같을 때만 캐시에 넣는다.
//! - **최신**(`"latest"`): 하루 한 번 `/releases/latest` 의 태그를 보고, 새 판이면 그 판의 `checksums.txt` 로 확인한다.
//! - 받기 · 확인이 실패하면(오프라인 포함) 캐시에 넣지 않는다. 물러나는 곳은 모드마다 다르다(메인 결정, 리뷰 F2): **고정**
//!   모드는 앱에 든 내장 스냅숏(고정 판의 번들 정의 — 고정 해시가 다 있다)으로만 물러난다 — 다른 판의 캐시(옛 Pitwall 의 고정 판 ·
//!   최신 모드의 잔재)로 가면 번들 패키지가 고정 해시 없는 판으로 풀린다. **최신** 모드만 마지막 캐시(받은 때가 가장 늦은 온전한
//!   판)로, 그것도 없으면 내장 스냅숏으로 물러난다.
//!
//! 캐시: `$CACHE/registry/<판>/registry.json` + `meta.json`(받은 때 · 해시). 읽을 때 `registry.json` 의 해시를 `meta.json` 과
//! 다시 견준다(깨진 캐시를 쓰지 않는다). 쓸 때는 임시 폴더에 다 쓴 뒤 이름을 바꾼다. 받은 태그는 폴더 이름이 되므로 꼴을 검사한다.

use std::io::Read as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::fetch::{Fetcher, sha256_hex};
use crate::index::{Index, MAX_REGISTRY_JSON_BYTES, RegistryOrigin};
use crate::pinned::PinnedRegistry;

/// mason-org/mason-registry 의 최신 릴리스(GitHub API).
pub const LATEST_API: &str =
    "https://api.github.com/repos/mason-org/mason-registry/releases/latest";
/// 릴리스 산출물 주소의 앞(`<앞>/<태그>/registry.json.zip`).
pub const RELEASE_BASE: &str = "https://github.com/mason-org/mason-registry/releases/download";

/// `registry.json.zip` 의 상한(바이트). 2026-11 의 판이 76 KB 다.
pub const MAX_REGISTRY_ZIP_BYTES: usize = 8 * 1024 * 1024;
/// `checksums.txt` · API 답의 상한.
const MAX_SMALL_BYTES: usize = 256 * 1024;
/// 최신 판을 다시 보는 간격.
pub const LATEST_CHECK_INTERVAL_MS: u64 = 24 * 60 * 60 * 1000;

/// 받을 곳(시험은 가짜 HTTP 서버로 바꾼다).
#[derive(Debug, Clone)]
pub struct Endpoints {
    pub latest_api: String,
    pub release_base: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Endpoints {
            latest_api: LATEST_API.into(),
            release_base: RELEASE_BASE.into(),
        }
    }
}

/// 설정 `[tools] registry`.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum RegistryMode {
    Pinned,
    Latest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Meta {
    version: String,
    fetched_at_ms: u64,
    zip_sha256: String,
    json_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LatestState {
    pub checked_at_ms: u64,
    pub version: String,
}

/// 받은 태그가 폴더 이름 · 주소에 넣어도 되는 꼴인가(`2026-11-01-batty-dew`).
pub fn valid_version(tag: &str) -> bool {
    let b = tag.as_bytes();
    tag.len() <= 64
        && b.len() > 11
        && b[..10].iter().enumerate().all(|(i, c)| {
            if i == 4 || i == 7 {
                *c == b'-'
            } else {
                c.is_ascii_digit()
            }
        })
        && b[10] == b'-'
        && tag[11..].split('-').all(|w| {
            !w.is_empty()
                && w.bytes()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        })
}

/// `registry.json.zip` 안 항목 수의 상한(지금은 하나다).
pub const MAX_ZIP_ENTRIES: usize = 16;

/// `registry.json.zip` 에서 `registry.json` 을 꺼낸다. 받은 것이라 미신뢰다 — 디스크에 항목 이름으로 풀지 않고(`registry.json`
/// 하나만 메모리로 읽는다) 그래도 보관 파일 전체를 먼저 거른다: 항목 수 상한, 이름이 폴더 밖을 가리키거나(`..` · 절대 경로 ·
/// 드라이브 · `\`) 심볼릭 링크인 항목이 있으면 거절, 적힌 풀린 크기의 합이 상한을 넘으면 거절(압축 폭탄), 적힌 크기가 거짓이어도
/// 상한까지만 읽는다.
pub fn unzip_registry(zip_bytes: &[u8]) -> Result<Vec<u8>, String> {
    unzip_with(zip_bytes, MAX_REGISTRY_JSON_BYTES, MAX_ZIP_ENTRIES)
}

pub(crate) fn unzip_with(
    zip_bytes: &[u8],
    max_bytes: usize,
    max_entries: usize,
) -> Result<Vec<u8>, String> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(zip_bytes))
        .map_err(|e| format!("registry.json.zip: {e}"))?;
    if archive.len() > max_entries {
        return Err(format!(
            "registry.json.zip has {} entries (limit {max_entries})",
            archive.len()
        ));
    }
    let mut declared: u64 = 0;
    for i in 0..archive.len() {
        let entry = archive
            .by_index_raw(i)
            .map_err(|e| format!("registry.json.zip: {e}"))?;
        let name = entry.name().to_owned();
        let unsafe_name = name.is_empty()
            || name.starts_with('/')
            || name.contains('\\')
            || name.contains(':')
            || name.split('/').any(|seg| seg == "..");
        if unsafe_name {
            return Err(format!(
                "registry.json.zip has an entry outside the folder: {name:?}"
            ));
        }
        if entry.is_symlink() {
            return Err(format!("registry.json.zip has a symbolic link: {name:?}"));
        }
        declared = declared.saturating_add(entry.size());
    }
    if declared > max_bytes as u64 {
        return Err(format!(
            "registry.json.zip unpacks to {declared} bytes (limit {max_bytes})"
        ));
    }
    let entry = archive
        .by_name("registry.json")
        .map_err(|e| format!("registry.json.zip has no registry.json: {e}"))?;
    let mut out = Vec::new();
    entry
        .take(max_bytes as u64 + 1)
        .read_to_end(&mut out)
        .map_err(|e| format!("registry.json.zip: {e}"))?;
    if out.len() > max_bytes {
        return Err(format!("registry.json is larger than {max_bytes} bytes"));
    }
    Ok(out)
}

/// `checksums.txt` 에서 `name` 의 해시(소문자).
pub fn checksum_for(text: &str, name: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let (hash, file) = line.split_once(char::is_whitespace)?;
        let file = file.trim_start().trim_start_matches('*').trim_end();
        let hash = hash.to_ascii_lowercase();
        (file == name && hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()))
            .then_some(hash)
    })
}

/// `$CACHE/registry`.
#[derive(Debug, Clone)]
pub struct Cache {
    root: PathBuf,
}

impl Cache {
    pub fn new(root: PathBuf) -> Cache {
        Cache { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// 그 판의 캐시(온전할 때만).
    pub fn load(&self, version: &str) -> Result<Index, String> {
        self.load_expecting(version, None)
    }

    /// 고정 판의 캐시. `meta.json` 만이 아니라 바이너리에 든 고정 해시(`registry.toml` 의 `json_sha256`)와도 같아야 한다 —
    /// `meta.json` 대조는 손상 감지일 뿐 변조 감지가 아니다(리뷰 F1).
    pub fn load_pinned(&self, pinned: &PinnedRegistry) -> Result<Index, String> {
        self.load_expecting(&pinned.version, Some(&pinned.json_sha256))
    }

    fn load_expecting(&self, version: &str, expected_json: Option<&str>) -> Result<Index, String> {
        if !valid_version(version) {
            return Err(format!("not a registry version: {version}"));
        }
        let dir = self.root.join(version);
        let meta: Meta = serde_json::from_slice(
            &std::fs::read(dir.join("meta.json")).map_err(|e| format!("{version}: {e}"))?,
        )
        .map_err(|e| format!("{version}/meta.json: {e}"))?;
        if meta.version != version {
            return Err(format!("{version}/meta.json names {}", meta.version));
        }
        let json = read_limited(&dir.join("registry.json"), MAX_REGISTRY_JSON_BYTES)?;
        let got = sha256_hex(&json);
        if expected_json.is_some_and(|want| want != got) {
            return Err(format!(
                "{version}/registry.json does not match the pinned SHA-256"
            ));
        }
        if got != meta.json_sha256 {
            return Err(format!(
                "{version}/registry.json does not match its meta.json"
            ));
        }
        Index::from_registry_json(
            &json,
            version,
            RegistryOrigin::Cache,
            Some(meta.fetched_at_ms),
        )
        .map_err(|e| e.to_string())
    }

    /// 받은 때가 가장 늦은 온전한 판. 없으면 `None`.
    pub fn load_last(&self) -> Option<Index> {
        let entries = std::fs::read_dir(&self.root).ok()?;
        let mut candidates: Vec<(u64, String)> = entries
            .filter_map(Result::ok)
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|n| valid_version(n))
            .filter_map(|n| {
                let meta: Meta = serde_json::from_slice(
                    &std::fs::read(self.root.join(&n).join("meta.json")).ok()?,
                )
                .ok()?;
                Some((meta.fetched_at_ms, n))
            })
            .collect();
        candidates.sort();
        candidates
            .into_iter()
            .rev()
            .find_map(|(_, version)| match self.load(&version) {
                Ok(index) => Some(index),
                Err(e) => {
                    tracing::warn!(target: "pw_tools", "skipping registry cache: {e}");
                    None
                }
            })
    }

    /// 확인한 판을 넣는다(임시 폴더에 쓴 뒤 이름 바꾸기). 같은 판이 있으면 바꾼다.
    fn store(&self, meta: &Meta, json: &[u8]) -> Result<(), String> {
        std::fs::create_dir_all(&self.root).map_err(|e| format!("{}: {e}", self.root.display()))?;
        let tmp = self
            .root
            .join(format!(".tmp-{}-{}", meta.version, std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
        std::fs::write(tmp.join("registry.json"), json).map_err(|e| e.to_string())?;
        std::fs::write(
            tmp.join("meta.json"),
            serde_json::to_vec_pretty(meta).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        let dest = self.root.join(&meta.version);
        let _ = std::fs::remove_dir_all(&dest);
        std::fs::rename(&tmp, &dest).map_err(|e| format!("{}: {e}", dest.display()))
    }

    pub fn latest_state(&self) -> Option<LatestState> {
        serde_json::from_slice(&std::fs::read(self.root.join("latest.json")).ok()?).ok()
    }

    fn write_latest_state(&self, state: &LatestState) -> Result<(), String> {
        std::fs::create_dir_all(&self.root).map_err(|e| e.to_string())?;
        let tmp = self
            .root
            .join(format!(".latest.json.{}", std::process::id()));
        std::fs::write(
            &tmp,
            serde_json::to_vec_pretty(state).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, self.root.join("latest.json")).map_err(|e| e.to_string())
    }
}

fn read_limited(path: &Path, limit: usize) -> Result<Vec<u8>, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut out = Vec::new();
    file.take(limit as u64 + 1)
        .read_to_end(&mut out)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    if out.len() > limit {
        return Err(format!("{} is larger than {limit} bytes", path.display()));
    }
    Ok(out)
}

/// 확인한 바이트를 캐시에 넣고 읽는다.
fn verify_and_store(
    cache: &Cache,
    version: &str,
    zip_bytes: &[u8],
    zip_sha256: &str,
    json_sha256: &str,
    now_ms: u64,
) -> Result<Index, String> {
    let got = sha256_hex(zip_bytes);
    if got != zip_sha256 {
        return Err(format!(
            "registry {version}: registry.json.zip SHA-256 is {got}, expected {zip_sha256}"
        ));
    }
    let json = unzip_registry(zip_bytes)?;
    let got = sha256_hex(&json);
    if got != json_sha256 {
        return Err(format!(
            "registry {version}: registry.json SHA-256 is {got}, expected {json_sha256}"
        ));
    }
    // 읽히는 것을 먼저 보고 넣는다 — 배열이 아니거나 상한을 넘는 판은 캐시에 남기지 않는다.
    let index = Index::from_registry_json(&json, version, RegistryOrigin::Cache, Some(now_ms))
        .map_err(|e| format!("registry {version}: {e}"))?;
    cache.store(
        &Meta {
            version: version.to_owned(),
            fetched_at_ms: now_ms,
            zip_sha256: zip_sha256.to_owned(),
            json_sha256: json_sha256.to_owned(),
        },
        &json,
    )?;
    Ok(index)
}

/// 고정 판을 받아 확인하고 캐시에 넣는다.
pub async fn fetch_pinned(
    fetcher: &Fetcher,
    pinned: &PinnedRegistry,
    cache: &Cache,
    now_ms: u64,
) -> Result<Index, String> {
    let zip = fetcher
        .get(&pinned.url, MAX_REGISTRY_ZIP_BYTES, None)
        .await
        .map_err(|e| format!("registry {}: {e}", pinned.version))?;
    let cache = cache.clone();
    let pinned = pinned.clone();
    tokio::task::spawn_blocking(move || {
        verify_and_store(
            &cache,
            &pinned.version,
            &zip,
            &pinned.sha256,
            &pinned.json_sha256,
            now_ms,
        )
    })
    .await
    .map_err(|e| e.to_string())?
}

#[derive(Deserialize)]
struct LatestRelease {
    tag_name: String,
}

/// 최신 판을 본다. 이미 캐시에 있으면 그것을, 아니면 받아 그 판의 `checksums.txt` 로 확인하고 넣는다. 본 때를 기록한다.
pub async fn fetch_latest(
    fetcher: &Fetcher,
    endpoints: &Endpoints,
    cache: &Cache,
    now_ms: u64,
) -> Result<Index, String> {
    let body = fetcher
        .get(
            &endpoints.latest_api,
            MAX_SMALL_BYTES,
            Some("application/vnd.github+json"),
        )
        .await
        .map_err(|e| format!("latest registry: {e}"))?;
    let release: LatestRelease =
        serde_json::from_slice(&body).map_err(|e| format!("latest registry: {e}"))?;
    let version = release.tag_name;
    if !valid_version(&version) {
        return Err(format!("latest registry: unexpected tag {version:?}"));
    }
    let index = match cache.load(&version) {
        Ok(index) => index,
        Err(_) => {
            let base = format!("{}/{version}", endpoints.release_base.trim_end_matches('/'));
            let sums = fetcher
                .get(&format!("{base}/checksums.txt"), MAX_SMALL_BYTES, None)
                .await
                .map_err(|e| format!("registry {version} checksums.txt: {e}"))?;
            let sums = String::from_utf8_lossy(&sums).into_owned();
            let zip_sha = checksum_for(&sums, "registry.json.zip").ok_or_else(|| {
                format!("registry {version}: checksums.txt has no registry.json.zip")
            })?;
            let json_sha = checksum_for(&sums, "registry.json")
                .ok_or_else(|| format!("registry {version}: checksums.txt has no registry.json"))?;
            let zip = fetcher
                .get(
                    &format!("{base}/registry.json.zip"),
                    MAX_REGISTRY_ZIP_BYTES,
                    None,
                )
                .await
                .map_err(|e| format!("registry {version}: {e}"))?;
            let c = cache.clone();
            let v = version.clone();
            tokio::task::spawn_blocking(move || {
                verify_and_store(&c, &v, &zip, &zip_sha, &json_sha, now_ms)
            })
            .await
            .map_err(|e| e.to_string())??
        }
    };
    if let Err(e) = cache.write_latest_state(&LatestState {
        checked_at_ms: now_ms,
        version,
    }) {
        tracing::warn!(target: "pw_tools", "could not record the latest registry check: {e}");
    }
    Ok(index)
}

#[cfg(test)]
mod tests;
