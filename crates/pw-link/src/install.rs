//! Relay 설치 · SSH 자동 페어링(18-m7-plan M7-06 ③ ④ ⑦ ⑧ ⑨ — 결정 M7-D5 · M7-D13). 모든 원격 일은 [`crate::ssh::Ssh`] 로 고정
//! 스크립트를 부른다.
//!
//! - **감지**(③): `uname -sm` → [`platform_of`]. 지원 밖(Windows 계열 · 32bit · 다른 OS)이면 까닭과 함께 끝나고 아무것도 올리지
//!   않는다.
//! - **설치**(④): 원격 `$DATA/relay/pitwall-relay-{채널}-{판}`(원격 셸이 `PITWALL_DATA_DIR` → `$XDG_DATA_HOME/pitwall` →
//!   `$HOME/.local/share/pitwall` 순으로 푼다 — Relay 의 `AppPaths` 와 같은 규칙)이 있고 해시가 매니페스트와 같으면 그대로
//!   (「이미 있음」). 아니면 Desktop 이 가진 바이너리를 올린다: 같은 폴더의 임시 이름(`<이름>.part.<pid>` — `pitwall-relay-*` 라 M7-07
//!   의 정리가 덮는다)에 쓰고, **원격에서 해시를 다시 본 뒤에만**(`sha256sum` · `shasum -a 256`, 둘 다 없으면 올린 바이너리의
//!   `--verify`) 0700 · 원자 rename. 다르면 임시 파일을 지우고 rename 하지 않는다.
//! - **올릴 바이트**(M7-05 리뷰 권고 2 — TOCTOU): 바이너리는 한 번 읽어 메모리에 두고([`RelayBinary`]) 그 바이트로 해시를 매니페스트와
//!   견주고 그 바이트를 그대로 보낸다 — 견준 뒤 파일을 다시 열지 않는다. 원격 rename 직전의 해시 확인이 겹방어다.
//! - **출처**: [`pw_release::relay::source`] 만 부른다. debug 빌드는 그 워크트리의 `mise run relay-build` 결과(서명 없는 매니페스트),
//!   release 빌드는 `$CACHE/relay-dist/<판>/` 의 서명 매니페스트(없거나 검증에 실패하면 릴리스에서 받는다 — [`Fetch`])다. 서명은
//!   **빌드에 박힌 updater 공개키**([`updater_pubkey`] — `tauri.conf.json` 의 `plugins.updater.pubkey` 를 컴파일 때 읽는다)로만
//!   본다. 설정 · 환경 변수로 키를 받는 길은 없다.
//! - **페어링**(⑧): `pitwall-relay pair --from-ssh <공개키> <이름>` 을 SSH 로 부른다(데몬 소켓 명령이 아니다 — 그 프로세스가 페어링
//!   표 파일에 잠금을 쥐고 쓴다). 답의 머신 id 를 M7-08 이 그 호스트 항목에 기억한다. **해제**(⑨): `pitwall-relay unpair <기기 id>`.
//! - **원격에 남기는 것**(⑦): 판마다 바이너리 하나(< 56 MiB — 메인 판정 M7-P13, 두 판까지는 M7-07 의 정리), `$DATA/relay/` 의 기기
//!   키 · 페어링 표(Relay 가 만든다). `relay.toml` 은 만들지 않는다.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use pw_core::BoxFuture;
use pw_release::relay::{self, Asset, Manifest, Source};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::device::{DeviceId, MachineId, PublicKey};
use crate::handshake::Versions;
use crate::ssh::{RemoteOutput, Ssh, SshError, SshTransport};

/// 앱 설정(updater 공개키가 든 파일). 컴파일 때 바이너리에 든다.
const TAURI_CONF: &str = include_str!("../../../apps/desktop/src-tauri/tauri.conf.json");

/// Relay 매니페스트를 검증하는 공개키 — 앱의 updater 공개키(`tauri.conf.json` 의 `plugins.updater.pubkey`, 컴파일 때 읽음). 설정 ·
/// 환경 변수로 바꾸는 길이 없다(18-m7-plan M7-06 ⑪).
pub fn updater_pubkey() -> &'static str {
    static KEY: OnceLock<String> = OnceLock::new();
    KEY.get_or_init(|| {
        let conf: serde_json::Value =
            serde_json::from_str(TAURI_CONF).expect("tauri.conf.json is JSON");
        conf["plugins"]["updater"]["pubkey"]
            .as_str()
            .expect("tauri.conf.json has plugins.updater.pubkey")
            .to_owned()
    })
}

/// 릴리스 자산을 받는 곳(태그 `v<판>`).
pub const RELEASES: &str = "https://github.com/jang2162/pitwall/releases/download";

/// 매니페스트 · 서명 파일의 크기 상한.
const MANIFEST_MAX: u64 = 64 * 1024;

/// 원격 OS · 아키텍처(`pw_release::relay::PLATFORMS` 의 이름).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemotePlatform {
    pub os: &'static str,
    pub arch: &'static str,
}

/// `uname -sm` 의 출력을 읽는다. 지원 밖이면 까닭(사람이 읽을 문장).
pub fn platform_of(uname_sm: &str) -> Result<RemotePlatform, String> {
    let mut words = uname_sm.split_whitespace();
    let (Some(kernel), Some(machine), None) = (words.next(), words.next(), words.next()) else {
        return Err(format!(
            "cannot read the remote platform from `uname -sm` ({:?})",
            crate::ssh::clean_text(uname_sm)
        ));
    };
    let os = match kernel {
        "Linux" => "linux",
        "Darwin" => "macos",
        k if k.starts_with("MINGW") || k.starts_with("MSYS") || k.starts_with("CYGWIN") => {
            return Err(format!(
                "the remote machine runs Windows ({k}); the Pitwall relay runs on macOS and Linux only"
            ));
        }
        other => {
            return Err(format!(
                "the remote machine runs {}; the Pitwall relay runs on macOS and Linux only",
                crate::ssh::clean_text(other)
            ));
        }
    };
    let arch = match machine {
        "x86_64" | "amd64" => "x86_64",
        "aarch64" | "arm64" => "aarch64",
        other => {
            return Err(format!(
                "the remote machine is {} ({}); the Pitwall relay is built for 64-bit x86_64 and aarch64 only",
                crate::ssh::clean_text(other),
                kernel
            ));
        }
    };
    Ok(RemotePlatform { os, arch })
}

/// 모든 원격 스크립트의 머리: `$1` 채널 · `$2` 판으로 Relay 바이너리 자리 `$f`(폴더 `$d` = `$DATA/relay`)를 푼다. Relay 의
/// `AppPaths` 와 같은 순서다(`PITWALL_DATA_DIR` — 절대 경로만, `$XDG_DATA_HOME` — 절대 경로만, `$HOME/.local/share`). 새로
/// 만드는 폴더 · 파일은 0700 · 0600(umask 077).
macro_rules! prelude {
    () => {
        "set -e; umask 077; \
         if [ -n \"${PITWALL_DATA_DIR:-}\" ]; then case \"$PITWALL_DATA_DIR\" in /*) b=\"$PITWALL_DATA_DIR\" ;; \
         *) echo \"pitwall: PITWALL_DATA_DIR is not an absolute path\" >&2; exit 64 ;; esac; \
         else case \"${XDG_DATA_HOME:-}\" in /*) b=\"$XDG_DATA_HOME/pitwall\" ;; \
         *) if [ -z \"${HOME:-}\" ]; then echo \"pitwall: HOME is not set\" >&2; exit 64; fi; \
         b=\"$HOME/.local/share/pitwall\" ;; esac; fi; \
         d=\"$b/relay\"; f=\"$d/pitwall-relay-$1-$2\"; "
    };
}

/// 파일 하나의 sha256(소문자 16진). 도구가 없으면 `none`. 파일 이름을 출력에 싣지 않게 stdin 으로 넘긴다.
macro_rules! hash_fn {
    () => {
        "h() { if command -v sha256sum >/dev/null 2>&1; then sha256sum < \"$1\" | cut -d \" \" -f 1; \
         elif command -v shasum >/dev/null 2>&1; then shasum -a 256 < \"$1\" | cut -d \" \" -f 1; \
         else echo none; fi; }; "
    };
}

/// 감지: 플랫폼과 설치된 바이너리의 해시(`absent` · `none` — 해시 도구 없음).
pub const PROBE: &str = concat!(
    prelude!(),
    hash_fn!(),
    "echo \"pitwall-probe uname=$(uname -sm)\"; \
     if [ -f \"$f\" ]; then echo \"pitwall-probe installed=$(h \"$f\")\"; \
     else echo \"pitwall-probe installed=absent\"; fi"
);

/// 해시 도구가 없는 원격에서 설치된 바이너리를 그 자신의 `--verify` 로 본다(`$3` 기대 해시).
pub const VERIFY: &str = concat!(
    prelude!(),
    "if \"$f\" --verify \"$f\" \"$3\" >/dev/null 2>&1; then echo \"pitwall-verify same\"; \
     else echo \"pitwall-verify different\"; fi"
);

/// 올리기: stdin 을 임시 이름에 쓰고, 원격 해시가 `$3` 과 같을 때만 0700 · rename. 다르면 임시 파일을 지우고 65 로 끝난다.
pub const UPLOAD: &str = concat!(
    prelude!(),
    hash_fn!(),
    "mkdir -p \"$d\"; t=\"$f.part.$$\"; c() { rm -f \"$t\"; }; trap c EXIT; trap \"exit 1\" HUP INT TERM; \
     cat > \"$t\"; s=$(h \"$t\"); \
     if [ \"$s\" = none ]; then chmod 700 \"$t\"; \
     if \"$t\" --verify \"$t\" \"$3\" >/dev/null 2>&1; then s=\"$3\"; else s=unverified; fi; fi; \
     if [ \"$s\" = \"$3\" ]; then chmod 700 \"$t\"; mv -f \"$t\" \"$f\"; echo \"pitwall-upload ok\"; \
     else echo \"pitwall-upload mismatch\"; exit 65; fi"
);

/// SSH 자동 페어링(`$3` 공개키 · `$4` 이름).
pub const PAIR: &str = concat!(prelude!(), "exec \"$f\" pair --from-ssh \"$3\" \"$4\"");

/// 기기 해제(`$3` 기기 id).
pub const UNPAIR: &str = concat!(prelude!(), "exec \"$f\" unpair \"$3\"");

/// 링크: 이 판의 `proxy` 의 stdio 가 링크 바이트 스트림이다.
pub const PROXY: &str = concat!(prelude!(), "exec \"$f\" proxy");

/// 업로드가 원격 해시 불일치로 끝난 종료 코드.
const MISMATCH: i32 = 65;

/// 릴리스 자산 받기(데스크톱은 [`HttpsFetch`]). `max` 바이트를 넘으면 실패다.
pub trait Fetch: Send + Sync {
    fn get<'a>(&'a self, url: &'a str, max: u64) -> BoxFuture<'a, io::Result<Vec<u8>>>;
}

/// https 로 받는다(앱의 updater 와 같은 reqwest · rustls(ring) 구성 — 크레이트가 늘지 않는다).
pub struct HttpsFetch {
    client: reqwest::Client,
}

impl HttpsFetch {
    pub fn new() -> io::Result<Self> {
        if rustls::crypto::CryptoProvider::get_default().is_none() {
            let _ = rustls::crypto::ring::default_provider().install_default();
        }
        let client = reqwest::Client::builder()
            .https_only(true)
            .timeout(std::time::Duration::from_secs(300))
            .build()
            .map_err(io::Error::other)?;
        Ok(Self { client })
    }
}

impl Fetch for HttpsFetch {
    fn get<'a>(&'a self, url: &'a str, max: u64) -> BoxFuture<'a, io::Result<Vec<u8>>> {
        Box::pin(async move {
            let mut response = self
                .client
                .get(url)
                .send()
                .await
                .and_then(reqwest::Response::error_for_status)
                .map_err(io::Error::other)?;
            if response.content_length().is_some_and(|n| n > max) {
                return Err(io::Error::other(format!("{url} is over {max} bytes")));
            }
            let mut body = Vec::new();
            while let Some(chunk) = response.chunk().await.map_err(io::Error::other)? {
                if body.len() as u64 + chunk.len() as u64 > max {
                    return Err(io::Error::other(format!("{url} is over {max} bytes")));
                }
                body.extend_from_slice(&chunk);
            }
            Ok(body)
        })
    }
}

/// 원격에 올릴 바이너리: 한 번 읽은 바이트와 그 바이트로 잰 해시. 매니페스트의 그 (OS, 아키텍처) 자산과 크기 · 해시가 같을 때만
/// 만든다([`RelayBinary::checked`]).
pub struct RelayBinary {
    bytes: Vec<u8>,
    sha256: String,
}

impl RelayBinary {
    fn checked(asset: &Asset, bytes: Vec<u8>) -> Result<Self, String> {
        if bytes.len() as u64 != asset.size {
            return Err(format!(
                "{} is {} bytes, the manifest says {}",
                asset.name,
                bytes.len(),
                asset.size
            ));
        }
        let sha256 = sha256_hex(&bytes);
        if sha256 != asset.sha256 {
            return Err(format!(
                "{} does not match the manifest's sha256",
                asset.name
            ));
        }
        Ok(Self { bytes, sha256 })
    }

    pub fn sha256(&self) -> &str {
        &self.sha256
    }

    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Desktop 이 원격에 올릴 Relay 를 어디서 가져오는가: `cache` 는 `$CACHE/relay-dist`(판마다 하위 폴더).
pub struct RelayDist {
    cache: PathBuf,
    fetch: Arc<dyn Fetch>,
    /// 시험만: debug 빌드에서도 서명 매니페스트 길로 간다(개발 빌드 갈래를 여는 쪽이 아니라 닫는 쪽).
    #[cfg(test)]
    signed_only: bool,
}

impl RelayDist {
    pub fn new(cache: PathBuf, fetch: Arc<dyn Fetch>) -> Self {
        Self {
            cache,
            fetch,
            #[cfg(test)]
            signed_only: false,
        }
    }

    /// 이 빌드의 출처에서 `version` 의 매니페스트를 읽는다([`pw_release::relay::source`] 하나만 — release 빌드는 늘 서명 매니페스트).
    async fn manifest(&self, version: &str) -> Result<(Manifest, Origin), String> {
        #[cfg(test)]
        if self.signed_only {
            return self.signed_manifest(version).await;
        }
        match relay::source() {
            Source::Dev(dist) => relay::read_dev_manifest(&dist, version)
                .map(|m| (m, Origin::Dir(dist.dir().to_path_buf())))
                .map_err(|e| e.to_string()),
            Source::Signed => self.signed_manifest(version).await,
        }
    }

    /// 그 플랫폼의 자산(매니페스트만 — 바이너리는 읽지 않는다).
    pub async fn asset(&self, version: &str, platform: RemotePlatform) -> Result<Asset, String> {
        let (manifest, _) = self.manifest(version).await?;
        manifest
            .asset(platform.os, platform.arch)
            .cloned()
            .ok_or_else(|| {
                format!(
                    "the relay manifest has no binary for {} {}",
                    platform.os, platform.arch
                )
            })
    }

    /// 그 플랫폼의 바이너리(매니페스트와 견준 바이트).
    pub async fn binary(
        &self,
        version: &str,
        platform: RemotePlatform,
    ) -> Result<RelayBinary, String> {
        let (manifest, origin) = self.manifest(version).await?;
        let asset = manifest
            .asset(platform.os, platform.arch)
            .ok_or_else(|| {
                format!(
                    "the relay manifest has no binary for {} {}",
                    platform.os, platform.arch
                )
            })?
            .clone();
        match origin {
            Origin::Dir(dir) => {
                let bytes = read_capped(&dir.join(&asset.name))?;
                RelayBinary::checked(&asset, bytes)
            }
            Origin::Signed(dir) => self.signed_binary(&dir, version, &asset).await,
        }
    }

    /// `$CACHE/relay-dist/<판>/` 의 서명 매니페스트. 없거나 검증에 실패하면 릴리스에서 다시 받아 본다. 검증 키는
    /// [`updater_pubkey`] 하나다.
    async fn signed_manifest(&self, version: &str) -> Result<(Manifest, Origin), String> {
        let dir = self.cache.join(version);
        let sig_name = format!("{}.sig", relay::MANIFEST_FILE);
        let cached = (|| -> Result<Manifest, String> {
            let manifest =
                std::fs::read(dir.join(relay::MANIFEST_FILE)).map_err(|e| e.to_string())?;
            let signature =
                std::fs::read_to_string(dir.join(&sig_name)).map_err(|e| e.to_string())?;
            relay::verify(&manifest, &signature, updater_pubkey(), version)
                .map_err(|e| e.to_string())
        })();
        let cached_error = match cached {
            Ok(manifest) => return Ok((manifest, Origin::Signed(dir))),
            Err(e) => e,
        };
        let manifest = self
            .fetch
            .get(&release_url(version, relay::MANIFEST_FILE), MANIFEST_MAX)
            .await
            .map_err(|e| {
                format!("cannot download the relay manifest ({e}); the cached one: {cached_error}")
            })?;
        let signature = self
            .fetch
            .get(&release_url(version, &sig_name), MANIFEST_MAX)
            .await
            .map_err(|e| format!("cannot download the relay manifest signature ({e})"))?;
        let signature = String::from_utf8(signature)
            .map_err(|_| "the relay manifest signature is not text".to_owned())?;
        let parsed = relay::verify(&manifest, &signature, updater_pubkey(), version)
            .map_err(|e| e.to_string())?;
        // 검증한 것만 캐시에 둔다.
        write_atomic(&dir, relay::MANIFEST_FILE, &manifest)?;
        write_atomic(&dir, &sig_name, signature.as_bytes())?;
        Ok((parsed, Origin::Signed(dir)))
    }

    /// 캐시의 바이너리를 매니페스트와 견준다. 없거나 다르면 받아서 견준 뒤 캐시에 둔다 — 견준 바이트를 그대로 돌려준다.
    async fn signed_binary(
        &self,
        dir: &Path,
        version: &str,
        asset: &Asset,
    ) -> Result<RelayBinary, String> {
        if let Ok(bytes) = read_capped(&dir.join(&asset.name))
            && let Ok(binary) = RelayBinary::checked(asset, bytes)
        {
            return Ok(binary);
        }
        let bytes = self
            .fetch
            .get(&release_url(version, &asset.name), relay::MAX_BINARY_BYTES)
            .await
            .map_err(|e| format!("cannot download {} ({e})", asset.name))?;
        let binary = RelayBinary::checked(asset, bytes)?;
        write_atomic(dir, &asset.name, &binary.bytes)?;
        Ok(binary)
    }
}

/// 매니페스트가 어디서 왔는가 — 바이너리도 같은 곳에서 읽는다.
enum Origin {
    /// 개발 빌드 폴더.
    Dir(PathBuf),
    /// `$CACHE/relay-dist/<판>`(서명 검증함).
    Signed(PathBuf),
}

fn release_url(version: &str, file: &str) -> String {
    format!("{RELEASES}/v{version}/{file}")
}

/// 파일 하나를 상한([`relay::MAX_BINARY_BYTES`]) 안에서 읽는다.
fn read_capped(path: &Path) -> Result<Vec<u8>, String> {
    let size = std::fs::metadata(path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?
        .len();
    if size > relay::MAX_BINARY_BYTES {
        return Err(format!("{} is over the relay binary limit", path.display()));
    }
    std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))
}

fn write_atomic(dir: &Path, name: &str, bytes: &[u8]) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let tmp = dir.join(format!(".{name}.{}.tmp", std::process::id()));
    std::fs::write(&tmp, bytes)
        .and_then(|()| std::fs::rename(&tmp, dir.join(name)))
        .map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            format!("cannot write {}: {e}", dir.join(name).display())
        })
}

/// 설치의 단계(진단 칸 — M7-08).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Progress {
    /// ssh 로 원격 플랫폼 · 설치 여부를 본다.
    Probing,
    Detected {
        os: &'static str,
        arch: &'static str,
    },
    /// 같은 판 · 같은 해시의 바이너리가 이미 있다.
    AlreadyInstalled,
    /// 바이너리를 올린다(바이트 수).
    Uploading {
        bytes: usize,
    },
    Installed,
    Pairing,
}

/// 설치 · 페어링 실패.
#[derive(Debug)]
pub enum InstallError {
    Ssh(SshError),
    /// 지원 밖 플랫폼. 아무것도 올리지 않았다.
    Unsupported(String),
    /// 올릴 바이너리를 얻지 못했다(매니페스트 서명 · 해시 · 받기). 아무것도 올리지 않았다.
    Dist(String),
    /// 올린 뒤 원격 해시가 매니페스트와 달랐다 — 임시 파일을 지우고 rename 하지 않았다.
    RemoteHashMismatch,
    /// 원격 스크립트 · 명령이 실패했다(단계, 종료 코드, stderr 끝부분).
    Remote {
        step: &'static str,
        code: Option<i32>,
        stderr: String,
    },
    /// 원격의 답을 읽지 못했다.
    Protocol(String),
}

impl std::fmt::Display for InstallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InstallError::Ssh(e) => write!(f, "{e}"),
            InstallError::Unsupported(why) => write!(f, "{why}"),
            InstallError::Dist(why) => write!(f, "cannot get the relay binary: {why}"),
            InstallError::RemoteHashMismatch => f.write_str(
                "the uploaded relay binary did not match the manifest on the remote machine; it was not installed",
            ),
            InstallError::Remote { step, code, stderr } => match code {
                Some(code) => write!(f, "the remote {step} failed (exit {code}): {stderr}"),
                None => write!(f, "the remote {step} was killed: {stderr}"),
            },
            InstallError::Protocol(why) => write!(f, "{why}"),
        }
    }
}

impl std::error::Error for InstallError {}

impl From<SshError> for InstallError {
    fn from(e: SshError) -> Self {
        InstallError::Ssh(e)
    }
}

/// 원격 감지 결과.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Probe {
    pub platform: RemotePlatform,
    /// 이 판의 바이너리가 있으면 그 해시(`None` — 해시 도구가 없음).
    pub installed: Installed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Installed {
    Absent,
    Sha256(String),
    /// 있으나 원격에 해시 도구가 없다 — 바이너리의 `--verify` 로 본다.
    Unknown,
}

/// [`Installer::bootstrap`] 의 결과.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bootstrapped {
    pub platform: RemotePlatform,
    /// 이번에 올렸는가(거짓 — 「이미 있음」).
    pub uploaded: bool,
    /// Relay 기기 키의 지문 — M7-08 이 그 호스트 항목에 기억하고 핸드셰이크에서 기대한다(`Expect::Machine`).
    pub machine: MachineId,
    /// 이 Desktop 키를 페어링 표에 새로 넣었는가.
    pub paired_now: bool,
}

/// 원격 호스트 하나의 설치기.
pub struct Installer {
    ssh: Arc<Ssh>,
    versions: Versions,
    dist: RelayDist,
}

#[derive(Deserialize)]
struct PairAnswer {
    machine: String,
    added: bool,
}

#[derive(Deserialize)]
struct UnpairAnswer {
    removed: bool,
}

impl Installer {
    /// `versions` 는 이 Desktop 의 판 · 릴리스 채널(원격 파일 이름 `pitwall-relay-{채널}-{판}` 과 매니페스트의 판).
    pub fn new(ssh: Arc<Ssh>, versions: Versions, dist: RelayDist) -> Result<Self, InstallError> {
        for (what, value) in [
            ("version", &versions.app),
            ("release channel", &versions.release_channel),
        ] {
            if !crate::ssh::check_arg(value) || value.contains('/') {
                return Err(InstallError::Protocol(format!(
                    "the desktop {what} has characters a remote file name does not take"
                )));
            }
        }
        Ok(Self {
            ssh,
            versions,
            dist,
        })
    }

    fn args<'a>(&'a self, more: &[&'a str]) -> Vec<&'a str> {
        let mut args = vec![
            self.versions.release_channel.as_str(),
            self.versions.app.as_str(),
        ];
        args.extend_from_slice(more);
        args
    }

    async fn run(
        &self,
        step: &'static str,
        script: &str,
        more: &[&str],
        input: Option<&[u8]>,
    ) -> Result<RemoteOutput, InstallError> {
        let output = self.ssh.run(script, &self.args(more), input).await?;
        Ok(output).and_then(|o| {
            if o.code == Some(0) || (step == "upload" && o.code == Some(MISMATCH)) {
                Ok(o)
            } else {
                Err(InstallError::Remote {
                    step,
                    code: o.code,
                    stderr: o.stderr,
                })
            }
        })
    }

    /// 원격 플랫폼과 이 판의 설치 여부. 지원 밖이면 [`InstallError::Unsupported`].
    pub async fn probe(&self) -> Result<Probe, InstallError> {
        let output = self.run("probe", PROBE, &[], None).await?;
        let uname = marked(&output.stdout, "pitwall-probe uname=")
            .ok_or_else(|| InstallError::Protocol("the remote probe printed no platform".into()))?;
        let installed = marked(&output.stdout, "pitwall-probe installed=").ok_or_else(|| {
            InstallError::Protocol("the remote probe printed no install state".into())
        })?;
        let platform = platform_of(&uname).map_err(InstallError::Unsupported)?;
        let installed = match installed.as_str() {
            "absent" => Installed::Absent,
            "none" => Installed::Unknown,
            sha if sha.len() == 64 && sha.bytes().all(|b| b.is_ascii_hexdigit()) => {
                Installed::Sha256(sha.to_ascii_lowercase())
            }
            _ => {
                return Err(InstallError::Protocol(
                    "the remote probe printed a bad hash".into(),
                ));
            }
        };
        Ok(Probe {
            platform,
            installed,
        })
    }

    /// 이 판을 설치한다(이미 같은 해시면 그대로). 올렸으면 참.
    pub async fn install(
        &self,
        probe: &Probe,
        progress: &(dyn Fn(Progress) + Send + Sync),
    ) -> Result<bool, InstallError> {
        let asset = self
            .dist
            .asset(&self.versions.app, probe.platform)
            .await
            .map_err(InstallError::Dist)?;
        let present = match &probe.installed {
            Installed::Absent => false,
            Installed::Sha256(sha) => *sha == asset.sha256,
            Installed::Unknown => {
                let output = self.run("verify", VERIFY, &[&asset.sha256], None).await?;
                marked(&output.stdout, "pitwall-verify ").as_deref() == Some("same")
            }
        };
        if present {
            progress(Progress::AlreadyInstalled);
            return Ok(false);
        }
        let binary = self
            .dist
            .binary(&self.versions.app, probe.platform)
            .await
            .map_err(InstallError::Dist)?;
        progress(Progress::Uploading {
            bytes: binary.len(),
        });
        let output = self
            .run("upload", UPLOAD, &[binary.sha256()], Some(&binary.bytes))
            .await?;
        if output.code == Some(MISMATCH) {
            return Err(InstallError::RemoteHashMismatch);
        }
        if marked(&output.stdout, "pitwall-upload ").as_deref() != Some("ok") {
            return Err(InstallError::Protocol(
                "the remote upload did not confirm".into(),
            ));
        }
        progress(Progress::Installed);
        Ok(true)
    }

    /// SSH 자동 페어링: 이 Desktop 의 기기 공개키를 Relay 페어링 표에 `owner` 로(없을 때만) 넣고 머신 id 를 받는다. `name` 은
    /// `[A-Za-z0-9._-]`(64자까지).
    pub async fn pair(
        &self,
        key: &PublicKey,
        name: &str,
    ) -> Result<(MachineId, bool), InstallError> {
        let key = key.to_string();
        let output = self.run("pairing", PAIR, &[&key, name], None).await?;
        let answer: PairAnswer = last_json(&output.stdout, "pairing")?;
        Ok((DeviceId::from_stored(answer.machine), answer.added))
    }

    /// 기기 해제: 그 기기 id 를 원격 페어링 표에서 뺀다. 데몬이 그 기기의 열린 링크를 끊는다. 있었으면 참.
    pub async fn unpair(&self, device: &DeviceId) -> Result<bool, InstallError> {
        let output = self
            .run("unpairing", UNPAIR, &[device.as_str()], None)
            .await?;
        let answer: UnpairAnswer = last_json(&output.stdout, "unpairing")?;
        Ok(answer.removed)
    }

    /// 감지 → 설치(필요하면) → 페어링. M7-08 이 「붙는 중」 의 SSH · 설치 단계로 부르고, 그 뒤 [`Self::transport`] 로 링크를 연다.
    pub async fn bootstrap(
        &self,
        key: &PublicKey,
        name: &str,
        progress: &(dyn Fn(Progress) + Send + Sync),
    ) -> Result<Bootstrapped, InstallError> {
        progress(Progress::Probing);
        let probe = self.probe().await?;
        progress(Progress::Detected {
            os: probe.platform.os,
            arch: probe.platform.arch,
        });
        let uploaded = self.install(&probe, progress).await?;
        progress(Progress::Pairing);
        let (machine, paired_now) = self.pair(key, name).await?;
        Ok(Bootstrapped {
            platform: probe.platform,
            uploaded,
            machine,
            paired_now,
        })
    }

    /// 이 판의 `proxy` 로 여는 링크 전송.
    pub fn transport(&self) -> SshTransport {
        self.ssh.transport(
            PROXY,
            vec![
                self.versions.release_channel.clone(),
                self.versions.app.clone(),
            ],
        )
    }
}

/// stdout 에서 표지 줄 하나의 값(로그인 셸이 뿌린 다른 줄은 무시한다).
fn marked(stdout: &[u8], prefix: &str) -> Option<String> {
    String::from_utf8_lossy(stdout)
        .lines()
        .rev()
        .find_map(|line| line.strip_prefix(prefix).map(|v| v.trim().to_owned()))
}

/// stdout 의 마지막 JSON 줄.
fn last_json<T: serde::de::DeserializeOwned>(stdout: &[u8], step: &str) -> Result<T, InstallError> {
    String::from_utf8_lossy(stdout)
        .lines()
        .rev()
        .find_map(|line| serde_json::from_str(line.trim()).ok())
        .ok_or_else(|| InstallError::Protocol(format!("the remote {step} printed no answer")))
}

#[cfg(test)]
mod key_tests {
    use super::*;

    /// 매니페스트 검증 키는 앱의 updater 공개키 그대로다(컴파일 때 읽음) — 시험 키가 아니다.
    #[test]
    fn the_manifest_key_is_the_app_updater_key_built_in() {
        let conf: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../apps/desktop/src-tauri/tauri.conf.json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            updater_pubkey(),
            conf["plugins"]["updater"]["pubkey"].as_str().unwrap()
        );
        let test_key = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../pw-release/tests/fixtures/relay/updater.pub"),
        )
        .unwrap();
        assert_ne!(updater_pubkey(), test_key.trim());
    }

    /// 키를 받는 길이 없다(18-m7-plan M7-06 ⑪): 이 모듈은 환경 변수를 읽지 않고, `relay::verify` 는 늘 [`updater_pubkey`] 로
    /// 부른다. 설정 · 환경 변수 · 인자로 키를 받는 길을 넣으면 이 시험이 실패한다.
    #[test]
    fn the_manifest_key_is_not_taken_from_settings_or_the_environment() {
        let source = include_str!("install.rs");
        let code = &source[..source.find("#[cfg(test)]\nmod key_tests").unwrap()];
        for forbidden in ["env::var", "var_os(", "option_env!", "env!("] {
            assert!(!code.contains(forbidden), "install.rs reads `{forbidden}`");
        }
        let calls: Vec<&str> = code
            .match_indices("relay::verify(")
            .map(|(i, _)| &code[i..])
            .collect();
        assert_eq!(calls.len(), 2, "every manifest verification is listed here");
        for call in calls {
            let args = &call[..call.find(';').unwrap()];
            assert!(args.contains("updater_pubkey()"), "{args}");
        }
        assert!(!code.contains("pubkey:"), "no struct field carries a key");
    }
}

#[cfg(test)]
mod release_tests {
    use super::*;

    struct Offline;

    impl Fetch for Offline {
        fn get<'a>(&'a self, _url: &'a str, _max: u64) -> BoxFuture<'a, io::Result<Vec<u8>>> {
            Box::pin(async { Err(io::Error::other("offline")) })
        }
    }

    /// ⑦ release 구성의 설치기는 개발 빌드 폴더를 쓰지 않는다 — 환경 변수가 좋은 개발 매니페스트를 가리켜도 서명 매니페스트 길(캐시 →
    /// 받기)로 간다. `mise run test-release` 가 돈다.
    #[tokio::test]
    #[cfg_attr(debug_assertions, ignore = "release only — mise run test-release")]
    async fn a_release_installer_never_takes_the_dev_build() {
        let dir = tempfile::tempdir().unwrap();
        let dist = dir.path().join("dev");
        std::fs::create_dir_all(&dist).unwrap();
        let version = semver::Version::parse("1.2.3").unwrap();
        let platform = &relay::PLATFORMS[0];
        let name = relay::asset_name(&version, platform);
        std::fs::write(dist.join(&name), b"relay").unwrap();
        let entry = relay::describe(&dist.join(&name), &name, relay::MAX_BINARY_BYTES).unwrap();
        let manifest = relay::assemble(&version, &[entry], false).unwrap();
        std::fs::write(dist.join(relay::MANIFEST_FILE), manifest.to_bytes()).unwrap();
        // SAFETY: nextest 는 시험마다 프로세스를 따로 띄운다.
        unsafe { std::env::set_var(relay::DEV_DIST_ENV, &dist) };
        let remote = RemotePlatform {
            os: platform.runs_on[0].0,
            arch: platform.runs_on[0].1,
        };
        let dist = RelayDist::new(dir.path().join("cache"), Arc::new(Offline));
        let err = dist.asset("1.2.3", remote).await.unwrap_err();
        assert!(err.contains("cannot download the relay manifest"), "{err}");
        assert!(dist.binary("1.2.3", remote).await.is_err());
    }
}

#[cfg(all(test, unix))]
#[path = "install_tests.rs"]
mod tests;
