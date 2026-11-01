//! 라이선스 · 출처 확인(D29 「최초 1회」 — 15-m4-plan.md M4-04 ⑤).
//!
//! - 확인 기록 `$STATE/tools/consent.json`: 패키지마다 확인한 라이선스 목록 · 출처 도메인. 라이선스나 출처 도메인이 바뀐 판은
//!   다시 묻는다(판만 바뀐 것은 묻지 않는다). 메인 판정(2026-11-02): `state.db` 가 아니라 이 파일이다 — `state.db` 는
//!   `$DATA` 의 프로젝트 레지스트리이고 그 마이그레이션은 `pw-core` 가 가진다(M3-02 의 `db.json` 과 같은 꼴).
//! - 쓰기는 원자적이다. 파일이 깨졌으면 **확인 없음**으로 본다 — 묻지 않고 설치하는 쪽으로 떨어지지 않는다. 깨진 파일은 다음
//!   쓰기 전에 `consent.json.corrupt` 로 옮긴다.
//! - 라이선스는 Mason `licenses` 위에 Pitwall 의 덮어쓰기 표(`crates/pw-tools/license-notes.toml`)를 먼저 본다.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::layout::write_atomic;

/// 바이너리에 묶인 라이선스 덮어쓰기 표.
pub const LICENSE_NOTES_TOML: &str = include_str!("../../license-notes.toml");
const MAX_CONSENT_BYTES: u64 = 4 * 1024 * 1024;
const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LicenseNote {
    pub licenses: Vec<String>,
    pub note: String,
    pub source: String,
    pub checked: String,
}

/// `license-notes.toml`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LicenseNotes {
    by_name: BTreeMap<String, LicenseNote>,
}

impl LicenseNotes {
    pub fn parse(text: &str) -> Result<LicenseNotes, String> {
        let doc: toml_edit::DocumentMut = text.parse().map_err(|e| format!("{e}"))?;
        let json = serde_json::Value::Object(pw_core::config::merge::to_json(doc.as_table()));
        let by_name: BTreeMap<String, LicenseNote> =
            serde_json::from_value(json).map_err(|e| e.to_string())?;
        for (name, note) in &by_name {
            if note.licenses.is_empty() {
                return Err(format!("{name}: licenses is empty"));
            }
        }
        Ok(LicenseNotes { by_name })
    }

    /// 묶인 표. 시험이 읽히는 것을 보므로 실패하지 않는다.
    pub fn builtin() -> LicenseNotes {
        LicenseNotes::parse(LICENSE_NOTES_TOML)
            .expect("crates/pw-tools/license-notes.toml parses (see tests)")
    }

    pub fn get(&self, name: &str) -> Option<&LicenseNote> {
        self.by_name.get(name)
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.by_name.keys().map(String::as_str)
    }

    /// 다이얼로그 · 기록에 쓸 라이선스: 덮어쓰기가 있으면 그것, 없으면 Mason 의 것.
    pub fn effective(&self, name: &str, mason: &[String]) -> Vec<String> {
        match self.by_name.get(name) {
            Some(n) => n.licenses.clone(),
            None => mason.to_vec(),
        }
    }
}

/// 패키지 하나의 확인.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConsentEntry {
    pub licenses: Vec<String>,
    pub source_domains: Vec<String>,
    /// 확인할 때의 판(참고 — 판만 바뀐 것은 다시 묻지 않는다).
    pub version: String,
    pub confirmed_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConsentFile {
    schema_version: u32,
    packages: BTreeMap<String, ConsentEntry>,
}

/// `$STATE/tools/consent.json`.
#[derive(Debug, Clone)]
pub struct ConsentStore {
    path: PathBuf,
}

impl ConsentStore {
    pub fn new(state_dir: &Path) -> Self {
        ConsentStore {
            path: state_dir.join("tools").join("consent.json"),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 읽은 기록. 없거나 깨졌으면 빈 기록(확인 없음)과, 깨졌으면 `true`.
    fn load(&self) -> (BTreeMap<String, ConsentEntry>, bool) {
        let meta = match fs::metadata(&self.path) {
            Ok(m) => m,
            Err(_) => return (BTreeMap::new(), false),
        };
        if meta.len() > MAX_CONSENT_BYTES {
            tracing::warn!(target: "pw_tools", "consent.json is too large — asking again");
            return (BTreeMap::new(), true);
        }
        let parsed = fs::read(&self.path)
            .map_err(|e| e.to_string())
            .and_then(|b| serde_json::from_slice::<ConsentFile>(&b).map_err(|e| e.to_string()));
        match parsed {
            Ok(f) if f.schema_version == SCHEMA_VERSION => (f.packages, false),
            Ok(f) => {
                tracing::warn!(target: "pw_tools", version = f.schema_version, "consent.json has an unknown schema — asking again");
                (BTreeMap::new(), true)
            }
            Err(e) => {
                tracing::warn!(target: "pw_tools", "consent.json cannot be read ({e}) — asking again");
                (BTreeMap::new(), true)
            }
        }
    }

    pub fn get(&self, package: &str) -> Option<ConsentEntry> {
        self.load().0.remove(package)
    }

    /// 확인을 남긴다.
    pub fn record(&self, package: &str, entry: ConsentEntry) -> Result<(), String> {
        let (mut packages, corrupt) = self.load();
        if corrupt {
            let _ = fs::rename(&self.path, self.path.with_extension("json.corrupt"));
        }
        packages.insert(package.to_owned(), entry);
        let file = ConsentFile {
            schema_version: SCHEMA_VERSION,
            packages,
        };
        let bytes = serde_json::to_vec_pretty(&file).map_err(|e| e.to_string())?;
        write_atomic(&self.path, &bytes)
    }
}

/// 확인 기록이 지금 판의 라이선스 · 출처를 덮는가(같은 목록 — 순서는 따지지 않는다).
pub fn covers(entry: &ConsentEntry, licenses: &[String], domains: &[String]) -> bool {
    sorted(&entry.licenses) == sorted(licenses) && sorted(&entry.source_domains) == sorted(domains)
}

fn sorted(list: &[String]) -> Vec<String> {
    let mut v = list.to_vec();
    v.sort();
    v.dedup();
    v
}
