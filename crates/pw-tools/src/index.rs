//! 읽은 registry 한 판(패키지 목록)과 내장 스냅숏.
//!
//! 내장 스냅숏(`crates/pw-tools/snapshot/bundled.json`)은 고정 판에서 기본 번들 패키지의 정의만 뽑은 것이다(결정 M4-D16).
//! 첫 실행 · 오프라인에서도 번들 서버를 「어디서 무엇을 받는지」 안다. `node scripts/tools-registry-bump.mjs bump` 가 만든다.

use std::collections::{BTreeMap, HashMap};

use serde::Deserialize;
use serde_json::Value;

use crate::overrides::Overrides;
use crate::package::{Package, ToolUnsupported};
use crate::target::ToolTarget;

/// 바이너리에 묶인 스냅숏.
pub const SNAPSHOT_JSON: &str = include_str!("../snapshot/bundled.json");

/// `registry.json` 의 상한(바이트). 2026-11 의 판이 565 KB 다.
pub const MAX_REGISTRY_JSON_BYTES: usize = 32 * 1024 * 1024;
/// 패키지 수의 상한. 2026-11 의 판이 608 개다.
pub const MAX_PACKAGES: usize = 20_000;

/// 목록을 어디서 읽었나.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    serde::Serialize,
    serde::Deserialize,
    specta::Type,
    schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum RegistryOrigin {
    /// 바이너리에 묶인 스냅숏(기본 번들 패키지만).
    Snapshot,
    /// `$CACHE/registry/<판>/` 에 받아 둔 판 전체.
    Cache,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadError(pub String);

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for LoadError {}

#[derive(Debug, Clone)]
pub struct Index {
    pub version: String,
    pub origin: RegistryOrigin,
    /// 받은 때(유닉스 밀리초). 스냅숏이면 `None`.
    pub fetched_at_ms: Option<u64>,
    packages: Vec<Package>,
    by_name: HashMap<String, usize>,
    /// 패키지로 읽지 못해(이름이 없다 · 객체가 아니다 · 이름이 겹친다) 뺀 항목 수.
    pub unreadable: usize,
}

/// 대상 하나에서의 지원 · 지원하지 않음 수.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Stats {
    pub supported: usize,
    pub unsupported: usize,
    /// 까닭([`ToolUnsupported::reason`]) → 수.
    pub by_reason: BTreeMap<&'static str, usize>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SnapshotFile {
    registry: String,
    packages: Vec<Value>,
}

impl Index {
    /// `registry.json`(패키지 정의의 배열)을 읽는다. 상한을 넘거나 배열이 아니면 오류, 항목 하나가 이상하면 그 항목만 뺀다.
    pub fn from_registry_json(
        bytes: &[u8],
        version: &str,
        origin: RegistryOrigin,
        fetched_at_ms: Option<u64>,
    ) -> Result<Index, LoadError> {
        if bytes.len() > MAX_REGISTRY_JSON_BYTES {
            return Err(LoadError(format!(
                "registry.json is larger than {MAX_REGISTRY_JSON_BYTES} bytes"
            )));
        }
        let value: Value =
            serde_json::from_slice(bytes).map_err(|e| LoadError(format!("registry.json: {e}")))?;
        let Value::Array(items) = value else {
            return Err(LoadError("registry.json is not an array".into()));
        };
        Index::from_values(&items, version, origin, fetched_at_ms)
    }

    fn from_values(
        items: &[Value],
        version: &str,
        origin: RegistryOrigin,
        fetched_at_ms: Option<u64>,
    ) -> Result<Index, LoadError> {
        if items.len() > MAX_PACKAGES {
            return Err(LoadError(format!(
                "registry has more than {MAX_PACKAGES} packages"
            )));
        }
        let mut packages: Vec<Package> = Vec::with_capacity(items.len());
        let mut unreadable = 0;
        let mut by_name = HashMap::new();
        for item in items {
            match Package::parse(item) {
                Ok(p) if !by_name.contains_key(&p.name) => {
                    by_name.insert(p.name.clone(), packages.len());
                    packages.push(p);
                }
                _ => unreadable += 1,
            }
        }
        Ok(Index {
            version: version.to_owned(),
            origin,
            fetched_at_ms,
            packages,
            by_name,
            unreadable,
        })
    }

    /// 바이너리에 묶인 스냅숏.
    pub fn snapshot() -> Index {
        Index::from_snapshot_json(SNAPSHOT_JSON)
            .expect("crates/pw-tools/snapshot/bundled.json parses (see tests)")
    }

    pub fn from_snapshot_json(text: &str) -> Result<Index, LoadError> {
        let file: SnapshotFile =
            serde_json::from_str(text).map_err(|e| LoadError(format!("snapshot: {e}")))?;
        Index::from_values(
            &file.packages,
            &file.registry,
            RegistryOrigin::Snapshot,
            None,
        )
    }

    pub fn packages(&self) -> &[Package] {
        &self.packages
    }

    pub fn get(&self, name: &str) -> Option<&Package> {
        self.by_name.get(name).map(|&i| &self.packages[i])
    }

    /// `target` 에서 패키지마다 풀어 본 수.
    pub fn stats(&self, target: Option<ToolTarget>, overrides: &Overrides) -> Stats {
        let mut stats = Stats::default();
        for p in &self.packages {
            match p.resolve(target, overrides.get(&p.name)) {
                Ok(_) => stats.supported += 1,
                Err(e) => {
                    stats.unsupported += 1;
                    *stats.by_reason.entry(e.reason()).or_default() += 1;
                }
            }
        }
        stats
    }

    /// 대상 하나에서 지원하지 않는 까닭(지원하면 `None`).
    pub fn unsupported(
        &self,
        p: &Package,
        target: Option<ToolTarget>,
        overrides: &Overrides,
    ) -> Option<ToolUnsupported> {
        p.resolve(target, overrides.get(&p.name)).err()
    }
}
