//! 덮어쓰기 정의 `crates/pw-tools/overrides/<패키지>.toml` (15-m4-plan.md M4-03 ⑦).
//!
//! Pitwall 이 registry 정의 위에 받을 판 · 주소 · `bin` 을 덮어 고정하는 틀이다. 덮어쓰기가 있는 패키지는 [`Package::resolve`]
//! 에서 이 값이 registry 의 것을 이긴다(판은 바꾸고, `download` 는 registry 의 `asset` · `download` 를 통째로 바꾸고, `bin` ·
//! `share` 는 키마다 덮는다). 내용(jdtls 의 정식 마일스톤 · 판 고정 lombok)은 쓰는 행이 더한다(M4-18 — 결정 M4-D4).
//!
//! 형식(모르는 키는 오류 — 우리 파일이라 조용히 버리지 않는다):
//!
//! ```toml
//! reason = "registry 의 스냅숏 주소는 서버에서 지워진다"   # 필수 — 왜 덮는가
//! version = "v1.63.0"                                     # 받을 판(purl 의 판 대신). 표현식의 `{{version}}` 도 이 값이다
//!
//! [[download]]                                            # registry 의 `source.asset` · `source.download` 대신
//! target = ["darwin_arm64", "darwin_x64"]                 # Mason 대상 이름(없으면 모든 대상)
//! files = { "jdtls.tar.gz" = "https://download.eclipse.org/jdtls/milestones/{{ version | strip_prefix \"v\" }}/…" }
//! config = "config_mac/"                                  # 그 밖의 키는 표현식 `{{source.download.<키>}}` 의 값
//!
//! [bin]
//! jdtls = "bin/jdtls"
//!
//! [share]
//! "jdtls/lombok.jar" = "lombok.jar"
//!
//! [bin_alias]                                             # 새 이름 → 이미 있는 `bin` 이름(대상별로 풀린 경로를 따른다)
//! kotlin-lsp = "intellij-server"
//! ```
//!
//! `reason` 만 쓰고 `version` · `download` 가 없는 덮어쓰기(별칭만 등)는 받을 것을 바꾸지 않으므로 고정 해시도 그대로다.
//!
//! [`Package::resolve`]: crate::package::Package::resolve

use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::{Map, Value};

mod generated {
    include!(concat!(env!("OUT_DIR"), "/overrides.rs"));
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Override {
    pub reason: String,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub download: Option<Vec<Map<String, Value>>>,
    #[serde(default)]
    pub bin: BTreeMap<String, String>,
    #[serde(default)]
    pub share: BTreeMap<String, String>,
    /// 새 실행 파일 이름 → 이미 있는(registry 또는 위 `bin` 의) 실행 파일 이름. 언어 확장의 `command` 가 registry 의 `bin`
    /// 이름과 다를 때 쓴다(kotlin-lsp: `kotlin-lsp` → `intellij-server` — 메인 결정, 리뷰 X1). 대상마다 다른 경로를 다시 적지
    /// 않고 그 대상에서 풀린 경로를 그대로 따른다.
    #[serde(default)]
    pub bin_alias: BTreeMap<String, String>,
}

impl Override {
    /// TOML 글을 읽는다.
    pub fn parse(text: &str) -> Result<Override, String> {
        let doc: toml_edit::DocumentMut = text.parse().map_err(|e| format!("{e}"))?;
        let json = Value::Object(pw_core::config::merge::to_json(doc.as_table()));
        let over: Override = serde_json::from_value(json).map_err(|e| e.to_string())?;
        if over.reason.trim().is_empty() {
            return Err("`reason` is empty".into());
        }
        if let Some(entries) = &over.download {
            if entries.is_empty() {
                return Err("`download` is empty".into());
            }
            for entry in entries {
                if !entry.get("files").is_some_and(Value::is_object) {
                    return Err("a `download` entry has no `files` table".into());
                }
            }
        }
        Ok(over)
    }

    /// `download` 를 registry 의 `source.download` 와 같은 꼴(목록)로.
    pub(crate) fn download_json(&self) -> Option<Value> {
        self.download
            .as_ref()
            .map(|entries| Value::Array(entries.iter().cloned().map(Value::Object).collect()))
    }
}

/// 패키지 이름 → 덮어쓰기.
#[derive(Debug, Clone, Default)]
pub struct Overrides {
    by_name: BTreeMap<String, Override>,
}

impl Overrides {
    /// 바이너리에 묶인 `overrides/*.toml`. 읽지 못한 파일은 경고하고 뺀다(시험 `builtin_overrides_all_parse` 가 막는다).
    pub fn builtin() -> Overrides {
        let (overrides, errors) = Overrides::from_files(generated::FILES);
        for (name, e) in errors {
            tracing::warn!(target: "pw_tools", "override {name}.toml: {e}");
        }
        overrides
    }

    /// `(패키지 이름, TOML 글)` 들. 읽지 못한 것은 `(이름, 오류)` 로 돌려준다.
    pub fn from_files(files: &[(&str, &str)]) -> (Overrides, Vec<(String, String)>) {
        let mut by_name = BTreeMap::new();
        let mut errors = Vec::new();
        for (name, text) in files {
            match Override::parse(text) {
                Ok(o) => {
                    by_name.insert((*name).to_owned(), o);
                }
                Err(e) => errors.push(((*name).to_owned(), e)),
            }
        }
        (Overrides { by_name }, errors)
    }

    pub fn get(&self, name: &str) -> Option<&Override> {
        self.by_name.get(name)
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.by_name.keys().map(String::as_str)
    }
}

/// 묶인 덮어쓰기 파일(시험 · 검사용).
#[cfg(test)]
pub(crate) fn builtin_files() -> &'static [(&'static str, &'static str)] {
    generated::FILES
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_overrides_all_parse() {
        let (_, errors) = Overrides::from_files(builtin_files());
        assert!(errors.is_empty(), "{errors:?}");
    }

    #[test]
    fn parse_rejects_unknown_keys_and_missing_reason() {
        assert!(Override::parse("reason = \"x\"\nversion = \"1\"\n").is_ok());
        assert!(Override::parse("version = \"1\"\n").is_err());
        assert!(Override::parse("reason = \" \"\n").is_err());
        assert!(Override::parse("reason = \"x\"\nurl = \"https://a\"\n").is_err());
        assert!(Override::parse("reason = \"x\"\n[[download]]\ntarget = \"linux\"\n").is_err());
        assert!(Override::parse("reason = \"x\"\ndownload = []\n").is_err());
        assert!(Override::parse("reason = \"x\"\n[bin]\na = 1\n").is_err());
        assert!(Override::parse("reason = \"x\"\n[bin_alias]\na = \"b\"\n").is_ok());
        assert!(Override::parse("reason = \"x\"\n[bin_alias]\na = 1\n").is_err());
    }
}
