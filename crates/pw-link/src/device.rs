//! 기기 키(18-m7-plan M7-03 ⑧ · 결정 M7-D13 · 메인 판정 M7-P6).
//!
//! - 키는 Ed25519 다. 공개키의 SHA-256 지문([`DeviceId`] — `SHA256:<base64>`, ssh 의 지문과 같은 꼴)이 기기 id 이고, Relay 의
//!   것은 **머신 id** 다([`MachineId`]). Desktop 은 핸드셰이크마다 Relay 에 도전값 서명을 청해 이 id 를 확인한다.
//! - Desktop 의 키는 OS 키체인([`KeychainKeyStore`]) — 항목 이름은 그 Desktop 의 `$DATA` 로 가른다(단일 인스턴스 키와 같은 열쇠,
//!   앱의 `instance.rs`). 같은 기기의 두 Desktop(다른 `PITWALL_*_DIR`)은 키가 다르다(계획 재리뷰 F-b).
//! - Relay 의 키는 `$DATA/relay/device.key`([`RelayKeyFile`] — 파일 0600 · 폴더 0700).
//!
//! 키 · 씨앗 · 서명은 로그 · 감사에 쓰지 않는다 — `Debug` 는 지문만 보인다.

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD};
use ed25519_dalek::{Signer, SigningKey, VerifyingKey};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

/// 기기 id: 공개키의 지문.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DeviceId(String);

impl DeviceId {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// 저장해 둔 글(M7-08 의 `remote_hosts` · M7-10 의 묶음 멤버)에서. 꼴은 검사하지 않는다 — 비교만 한다.
    pub fn from_stored(text: impl Into<String>) -> Self {
        Self(text.into())
    }
}

impl fmt::Display for DeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// 머신 id(메인 판정 M7-P6): Relay 기기 공개키의 지문.
pub type MachineId = DeviceId;

/// 기기 공개키.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct PublicKey([u8; 32]);

impl PublicKey {
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// 지문 — 기기 id.
    pub fn id(&self) -> DeviceId {
        DeviceId(format!(
            "SHA256:{}",
            STANDARD_NO_PAD.encode(Sha256::digest(self.0))
        ))
    }

    /// `signature` 가 이 키로 `message` 에 한 서명인가. 꼴이 틀린 키 · 약한 키는 거짓이다(`verify_strict`).
    pub fn verifies(&self, message: &[u8], signature: &Signature) -> bool {
        let Ok(key) = VerifyingKey::from_bytes(&self.0) else {
            return false;
        };
        key.verify_strict(message, &ed25519_dalek::Signature::from_bytes(&signature.0))
            .is_ok()
    }

    /// base64 글에서(페어링 표 · 틀).
    pub fn parse(text: &str) -> Option<Self> {
        let bytes = STANDARD.decode(text.trim()).ok()?;
        Some(Self(bytes.try_into().ok()?))
    }
}

impl fmt::Display for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&STANDARD.encode(self.0))
    }
}

impl fmt::Debug for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PublicKey({})", self.id())
    }
}

impl Serialize for PublicKey {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for PublicKey {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let text = String::deserialize(d)?;
        Self::parse(&text).ok_or_else(|| serde::de::Error::custom("not a 32-byte base64 key"))
    }
}

/// 서명 64바이트.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Signature([u8; 64]);

impl Signature {
    pub fn from_bytes(bytes: [u8; 64]) -> Self {
        Self(bytes)
    }

    pub fn to_bytes(self) -> [u8; 64] {
        self.0
    }
}

impl fmt::Debug for Signature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Signature(..)")
    }
}

impl Serialize for Signature {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&STANDARD.encode(self.0))
    }
}

impl<'de> Deserialize<'de> for Signature {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let text = String::deserialize(d)?;
        let bytes = STANDARD
            .decode(text)
            .map_err(|_| serde::de::Error::custom("signature is not base64"))?;
        Ok(Self(bytes.try_into().map_err(|_| {
            serde::de::Error::custom("signature is not 64 bytes")
        })?))
    }
}

/// 기기의 비밀키. 버리면 씨앗을 지운다(ed25519-dalek 의 `zeroize`).
pub struct DeviceKey {
    key: SigningKey,
}

impl DeviceKey {
    /// OS 난수로 새 키.
    pub fn generate() -> io::Result<Self> {
        let mut seed = Zeroizing::new([0u8; 32]);
        getrandom::fill(seed.as_mut()).map_err(|e| io::Error::other(e.to_string()))?;
        Ok(Self::from_seed(&seed))
    }

    pub fn from_seed(seed: &[u8; 32]) -> Self {
        Self {
            key: SigningKey::from_bytes(seed),
        }
    }

    /// 저장할 씨앗(키체인 · 키 파일). 쓴 뒤 지워진다.
    fn seed(&self) -> Zeroizing<[u8; 32]> {
        Zeroizing::new(self.key.to_bytes())
    }

    pub fn public(&self) -> PublicKey {
        PublicKey(self.key.verifying_key().to_bytes())
    }

    pub fn id(&self) -> DeviceId {
        self.public().id()
    }

    pub fn sign(&self, message: &[u8]) -> Signature {
        Signature(self.key.sign(message).to_bytes())
    }

    fn encode_seed(&self) -> Zeroizing<String> {
        Zeroizing::new(STANDARD.encode(self.seed().as_ref()))
    }

    fn decode_seed(text: &str) -> Option<Self> {
        let bytes = Zeroizing::new(STANDARD.decode(text.trim()).ok()?);
        let seed: Zeroizing<[u8; 32]> = Zeroizing::new(bytes.as_slice().try_into().ok()?);
        Some(Self::from_seed(&seed))
    }
}

impl fmt::Debug for DeviceKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "DeviceKey({})", self.id())
    }
}

/// 32바이트 난수(도전값).
pub(crate) fn random_challenge() -> io::Result<[u8; 32]> {
    let mut out = [0u8; 32];
    getrandom::fill(&mut out).map_err(|e| io::Error::other(e.to_string()))?;
    Ok(out)
}

#[derive(Debug)]
pub struct KeyStoreError(pub String);

impl fmt::Display for KeyStoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "device key store: {}", self.0)
    }
}

impl std::error::Error for KeyStoreError {}

/// Desktop 기기 키를 두는 곳. 없으면 만들어 둔다.
pub trait DeviceKeyStore: Send + Sync {
    fn load_or_create(&self) -> Result<DeviceKey, KeyStoreError>;
}

/// 키체인 서비스 이름. 항목(계정)은 [`KeychainKeyStore::account_for`] 가 인스턴스마다 가른다.
pub const KEYCHAIN_SERVICE: &str = "Pitwall Device Key";

/// OS 키체인의 Desktop 기기 키(`keyring` — macOS Keychain · Windows Credential Manager · Secret Service).
pub struct KeychainKeyStore {
    service: String,
    account: String,
}

impl KeychainKeyStore {
    /// 단일 인스턴스 키(앱의 `single_instance_key` — `$DATA` · 빌드 종류로 갈린다)로 가른 항목.
    pub fn for_instance(instance_key: &str) -> Self {
        Self {
            service: KEYCHAIN_SERVICE.to_owned(),
            account: Self::account_for(instance_key),
        }
    }

    /// 인스턴스 키의 항목 이름. 다른 `$DATA` 의 Desktop 은 다른 키를 읽는다(계획 재리뷰 F-b).
    pub fn account_for(instance_key: &str) -> String {
        format!("device-key:{instance_key}")
    }

    pub fn account(&self) -> &str {
        &self.account
    }
}

impl DeviceKeyStore for KeychainKeyStore {
    fn load_or_create(&self) -> Result<DeviceKey, KeyStoreError> {
        let entry = keyring::Entry::new(&self.service, &self.account)
            .map_err(|e| KeyStoreError(e.to_string()))?;
        match entry.get_password() {
            Ok(text) => {
                let text = Zeroizing::new(text);
                DeviceKey::decode_seed(&text)
                    .ok_or_else(|| KeyStoreError("the stored device key is malformed".to_owned()))
            }
            Err(keyring::Error::NoEntry) => {
                let key = DeviceKey::generate().map_err(|e| KeyStoreError(e.to_string()))?;
                entry
                    .set_password(&key.encode_seed())
                    .map_err(|e| KeyStoreError(e.to_string()))?;
                Ok(key)
            }
            Err(e) => Err(KeyStoreError(e.to_string())),
        }
    }
}

/// 메모리 저장소(시험 — 사람의 키체인을 건드리지 않는다).
#[derive(Default)]
pub struct MemoryKeyStore {
    seed: Mutex<Option<Zeroizing<[u8; 32]>>>,
}

impl DeviceKeyStore for MemoryKeyStore {
    fn load_or_create(&self) -> Result<DeviceKey, KeyStoreError> {
        let mut seed = self
            .seed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(seed) = seed.as_ref() {
            return Ok(DeviceKey::from_seed(seed));
        }
        let key = DeviceKey::generate().map_err(|e| KeyStoreError(e.to_string()))?;
        *seed = Some(key.seed());
        Ok(key)
    }
}

/// Relay 기기 키 파일의 이름(`$DATA/relay/` 아래).
pub const RELAY_KEY_FILE: &str = "device.key";

/// Relay 의 기기 키 파일(`<dir>/device.key`, `dir` 은 `$DATA/relay`). 폴더는 0700, 파일은 0600 으로 만든다. 이미 있는 폴더 · 파일이
/// 다른 사용자에게 열려 있거나 심링크면 읽지 않는다(fail-closed — 남이 바꿔 넣은 키로 머신 id 를 대지 않게).
pub struct RelayKeyFile {
    path: PathBuf,
}

impl RelayKeyFile {
    pub fn in_dir(dir: &Path) -> Self {
        Self {
            path: dir.join(RELAY_KEY_FILE),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn load_or_create(&self) -> io::Result<DeviceKey> {
        let dir = self
            .path
            .parent()
            .ok_or_else(|| io::Error::other("the key file has no folder"))?;
        private_dir(dir)?;
        match read_private(&self.path) {
            Ok(text) => DeviceKey::decode_seed(&text).ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "the device key file is malformed",
                )
            }),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                let key = DeviceKey::generate()?;
                write_private_new(&self.path, key.encode_seed().as_bytes())?;
                Ok(key)
            }
            Err(e) => Err(e),
        }
    }
}

/// 폴더를 0700 으로 만들거나, 있는 폴더가 이 사용자 전용인지 본다.
pub(crate) fn private_dir(dir: &Path) -> io::Result<()> {
    match std::fs::symlink_metadata(dir) {
        Ok(meta) => {
            if meta.file_type().is_symlink() || !meta.is_dir() {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    format!("{} is not a plain folder", dir.display()),
                ));
            }
            check_private(&meta, dir)
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            let mut builder = std::fs::DirBuilder::new();
            builder.recursive(true);
            #[cfg(unix)]
            std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
            builder.create(dir)
        }
        Err(e) => Err(e),
    }
}

/// 다른 사용자(그룹 · 그 밖)에게 열린 권한이 없는가. 유닉스가 아니면 사용자 폴더의 ACL 에 맡긴다.
#[cfg_attr(not(unix), allow(clippy::unnecessary_wraps))]
fn check_private(meta: &std::fs::Metadata, path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o077 != 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("{} is open to other users", path.display()),
            ));
        }
    }
    #[cfg(not(unix))]
    let _ = (meta, path);
    Ok(())
}

/// 이 사용자 전용 파일을 읽는다(심링크 · 다른 사용자에게 열린 파일은 거절).
pub(crate) fn read_private(path: &Path) -> io::Result<Zeroizing<String>> {
    let meta = std::fs::symlink_metadata(path)?;
    if !meta.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("{} is not a plain file", path.display()),
        ));
    }
    check_private(&meta, path)?;
    Ok(Zeroizing::new(std::fs::read_to_string(path)?))
}

/// 없는 파일을 0600 으로 새로 만든다(있으면 실패 — 두 프로세스가 동시에 만들어도 하나만 이긴다).
fn write_private_new(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_signature_verifies_only_for_its_key_and_message() {
        let a = DeviceKey::generate().unwrap();
        let b = DeviceKey::generate().unwrap();
        let sig = a.sign(b"hello");
        assert!(a.public().verifies(b"hello", &sig));
        assert!(!a.public().verifies(b"hellO", &sig));
        assert!(!b.public().verifies(b"hello", &sig));
        assert_ne!(a.id(), b.id());
        assert!(a.id().as_str().starts_with("SHA256:"));
        // 지문 · Debug 에 비밀이 실리지 않는다.
        assert!(!format!("{a:?}").contains(&STANDARD.encode(a.seed().as_ref())));
    }

    #[test]
    fn the_keychain_item_is_per_instance() {
        let one = KeychainKeyStore::for_instance("dev.pitwall:/Users/a/data-1");
        let two = KeychainKeyStore::for_instance("dev.pitwall:/Users/a/data-2");
        assert_ne!(one.account(), two.account());
        assert_eq!(
            one.account(),
            KeychainKeyStore::account_for("dev.pitwall:/Users/a/data-1")
        );
    }

    #[test]
    fn the_memory_store_keeps_one_key() {
        let store = MemoryKeyStore::default();
        assert_eq!(
            store.load_or_create().unwrap().id(),
            store.load_or_create().unwrap().id()
        );
    }

    #[test]
    fn the_relay_key_file_is_created_private_and_read_back() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("relay");
        let file = RelayKeyFile::in_dir(&dir);
        let first = file.load_or_create().unwrap();
        let again = file.load_or_create().unwrap();
        assert_eq!(first.id(), again.id());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&dir), 0o700);
            assert_eq!(mode(file.path()), 0o600);
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_key_file_open_to_others_is_not_read() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("relay");
        let file = RelayKeyFile::in_dir(&dir);
        file.load_or_create().unwrap();
        std::fs::set_permissions(file.path(), std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(
            file.load_or_create().unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
        std::fs::set_permissions(file.path(), std::fs::Permissions::from_mode(0o600)).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            file.load_or_create().unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
    }
}
