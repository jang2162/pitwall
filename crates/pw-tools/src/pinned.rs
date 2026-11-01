//! `crates/pw-tools/registry.toml` — Pitwall 판이 고정한 registry 판과 기본 번들 패키지의 고정 해시(결정 M4-D16 · D17).
//!
//! 손으로 고치지 않는다. `node scripts/tools-registry-bump.mjs bump` 가 registry 판을 올리며 다시 쓴다(CONTRIBUTING.md 「도구
//! registry」). `mise run lint` 의 `check` 가 [`check`] 로 기본 번들 패키지의 (판, 대상, 파일)마다 해시가 있는지 본다.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::overrides::Overrides;
use crate::package::{Package, ResolvedTool, ToolSource, ToolUnsupported};
use crate::target::ToolTarget;

/// 바이너리에 묶인 `registry.toml`.
pub const PINNED_TOML: &str = include_str!("../registry.toml");

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pinned {
    pub registry: PinnedRegistry,
    /// 기본 번들 패키지(결정 M4-D1 ~ D5). 내장 스냅숏에 묶이고 해시를 고정한다.
    pub bundled: Vec<String>,
    #[serde(default, rename = "artifact")]
    pub artifacts: Vec<Artifact>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PinnedRegistry {
    /// 릴리스 태그(`2026-11-01-batty-dew`).
    pub version: String,
    /// `registry.json.zip` 의 주소.
    pub url: String,
    /// `registry.json.zip` 의 SHA-256(소문자 16진수).
    pub sha256: String,
    /// 그 안 `registry.json` 의 SHA-256(릴리스의 `checksums.txt` 와 같다).
    pub json_sha256: String,
}

/// 받을 파일 하나의 고정 해시.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Artifact {
    /// Mason 패키지 이름.
    pub package: String,
    /// 그 패키지의 판(덮어쓰기가 있으면 그 판).
    pub version: String,
    /// [`ToolTarget`] 이름 또는 `any`(npm 타르볼 · 순수 파이썬 휠처럼 대상과 상관없는 파일).
    pub target: String,
    /// 저장 이름(github · generic) 또는 받은 파일 이름(npm 타르볼 · 휠).
    pub name: String,
    pub url: String,
    pub sha256: String,
    /// 바이트.
    pub size: u64,
    /// 해시를 어디서 얻었나: `download`(받아서 계산) · `github-api`(GitHub 이 올릴 때 계산한 자산 `digest`) · `pypi`(PyPI 의
    /// `digests.sha256`) · `vendor-sha256`(배포처가 함께 올린 `<주소>.sha256`). 모두 HTTPS 로 받은 값이다.
    pub hash_from: String,
    /// npm 의 `dist.integrity`(SRI, `sha512-…`) — 설치기가 npm 이 받은 것과 대조한다.
    #[serde(default)]
    pub integrity: Option<String>,
    /// npm · PyPI 의 배포 이름과 판(본 패키지 · `extra_packages` · PyPI 의존 휠). github · generic 은 없다.
    #[serde(default)]
    pub dist: Option<String>,
    #[serde(default)]
    pub dist_version: Option<String>,
}

impl Pinned {
    pub fn parse(text: &str) -> Result<Pinned, String> {
        let doc: toml_edit::DocumentMut = text.parse().map_err(|e| format!("{e}"))?;
        let json = Value::Object(pw_core::config::merge::to_json(doc.as_table()));
        serde_json::from_value(json).map_err(|e| e.to_string())
    }

    /// 바이너리에 묶인 것. 묶을 때 시험이 읽히는 것을 보므로 실패하지 않는다.
    pub fn builtin() -> Pinned {
        Pinned::parse(PINNED_TOML).expect("crates/pw-tools/registry.toml parses (see tests)")
    }

    pub fn is_bundled(&self, name: &str) -> bool {
        self.bundled.iter().any(|b| b == name)
    }

    /// 그 패키지 · 판 · 대상의 고정 해시(대상이 `any` 인 것 포함).
    pub fn artifacts_for(
        &self,
        package: &str,
        version: &str,
        target: ToolTarget,
    ) -> Vec<&Artifact> {
        self.artifacts
            .iter()
            .filter(|a| {
                a.package == package
                    && a.version == version
                    && (a.target == "any" || a.target == target.as_str())
            })
            .collect()
    }
}

impl Pinned {
    /// 풀린 패키지가 받을 것 전부에 고정 해시가 있으면 그 행들(받을 파일과 (이름, 주소)로 짝지은 것), 하나라도 없으면 `None`
    /// (리뷰 F4 — 판만 같고 주소가 바뀐 정의에 고정 해시를 내보이지 않는다). 설치기(M4-04)도 이것으로 찾는다.
    /// - 파일(`github` · `generic`): 파일마다 (패키지, 판, 대상, 저장 이름, 주소)가 같은 행.
    /// - npm: 본 패키지와 `extra_packages` 마다 (배포 이름, 판)이 같은 `any` 행.
    /// - PyPI: 본 배포와 표에 적힌 의존 배포마다 그 대상(또는 `any`)의 휠 행 — 그 행 전부.
    pub fn pinned_for(&self, resolved: &ResolvedTool) -> Option<Vec<&Artifact>> {
        let rows = || {
            self.artifacts
                .iter()
                .filter(|a| a.package == resolved.name && a.version == resolved.version)
        };
        match &resolved.source {
            ToolSource::Files { files } => files
                .iter()
                .map(|f| {
                    rows().find(|a| {
                        a.target == resolved.target.as_str() && a.name == f.name && a.url == f.url
                    })
                })
                .collect(),
            ToolSource::Npm { package, extra } => std::iter::once(package)
                .chain(extra)
                .map(|spec| {
                    rows().find(|a| {
                        a.target == "any"
                            && a.dist.as_deref() == Some(spec.name.as_str())
                            && spec
                                .version
                                .as_ref()
                                .is_none_or(|v| a.dist_version.as_ref() == Some(v))
                    })
                })
                .collect(),
            ToolSource::Pypi { package, .. } => {
                let for_target: Vec<&Artifact> = rows()
                    .filter(|a| a.target == "any" || a.target == resolved.target.as_str())
                    .collect();
                let main = normalize_pypi(&package.name);
                let mut dists: BTreeSet<String> = rows()
                    .filter_map(|a| a.dist.as_deref().map(normalize_pypi))
                    .collect();
                dists.insert(main);
                let covered = dists.iter().all(|d| {
                    for_target
                        .iter()
                        .any(|a| a.dist.as_deref().map(normalize_pypi).as_deref() == Some(d))
                });
                covered.then_some(for_target)
            }
        }
    }
}

fn is_sha256(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// PyPI 이름 정규화(PEP 503).
pub fn normalize_pypi(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut dash = false;
    for c in name.chars() {
        if matches!(c, '-' | '_' | '.') {
            dash = true;
        } else {
            if dash && !out.is_empty() {
                out.push('-');
            }
            dash = false;
            out.push(c.to_ascii_lowercase());
        }
    }
    out
}

/// 고정 표가 스냅숏의 번들 패키지를 모두 덮는가. 문제를 모두 모아 돌려준다(비면 통과).
///
/// 보는 것: 스냅숏의 registry 판 = 고정 판, 번들 패키지가 스냅숏에 있고 패키지 전체로는 지원된다, (패키지, 판, 대상)마다
/// 받을 파일 각각에 해시 · 크기 · 주소가 맞는 줄이 있다(npm 은 본 패키지와 `extra_packages` 의 타르볼 + `integrity`, PyPI 는 본
/// 배포와 표에 적힌 의존 배포마다 그 대상의 휠), 번들 밖 · 옛 판을 가리키는 줄이 없다. 대상 항목이 없거나 표현식을 모르는 대상(kotlin-lsp 의 `linux_arm64`)은
/// 해시가 필요 없다 — 그 대상에서는 설치를 제안하지 않는다.
///
/// 보지 않는 것: 해시가 실제 파일과 맞는가(받지 않는다 — `bump` 가 받아 적고 설치기가 받을 때 대조한다), PyPI 의존 배포의 목록이
/// 빠짐없는가(의존은 PyPI 메타데이터에서 오고 registry 정의에는 없다 — `bump` 가 푼다. 한 배포의 행을 통째로 지우면 못 잡는다).
pub fn check(
    pinned: &Pinned,
    snapshot_version: &str,
    snapshot: &[Package],
    overrides: &Overrides,
) -> Vec<String> {
    let mut problems = Vec::new();
    if pinned.registry.version != snapshot_version {
        problems.push(format!(
            "snapshot is registry {snapshot_version} but registry.toml pins {}",
            pinned.registry.version
        ));
    }
    for (what, h) in [
        ("registry.sha256", &pinned.registry.sha256),
        ("registry.json_sha256", &pinned.registry.json_sha256),
    ] {
        if !is_sha256(h) {
            problems.push(format!("{what} is not a lowercase SHA-256"));
        }
    }
    if !pinned.registry.url.starts_with("https://") {
        problems.push("registry.url is not https".into());
    }
    let mut versions: BTreeSet<(String, String)> = BTreeSet::new();
    for name in &pinned.bundled {
        let Some(pkg) = snapshot.iter().find(|p| &p.name == name) else {
            problems.push(format!("bundled package {name} is not in the snapshot"));
            continue;
        };
        for target in ToolTarget::ALL {
            let resolved = match pkg.resolve(Some(target), overrides.get(name)) {
                Ok(r) => r,
                Err(ToolUnsupported::Target | ToolUnsupported::Expression { .. }) => continue,
                Err(e) => {
                    problems.push(format!("bundled package {name} is unsupported: {e}"));
                    break;
                }
            };
            versions.insert((name.clone(), resolved.version.clone()));
            match &resolved.source {
                ToolSource::Files { files } => {
                    for f in files {
                        let row = pinned.artifacts.iter().find(|a| {
                            a.package == *name
                                && a.version == resolved.version
                                && a.target == target.as_str()
                                && a.name == f.name
                        });
                        match row {
                            None => problems.push(format!(
                                "{name} {} {target}: no pinned hash for {}",
                                resolved.version, f.name
                            )),
                            Some(a) if a.url != f.url => problems.push(format!(
                                "{name} {} {target}: {} is pinned for {} but the definition downloads {}",
                                resolved.version, f.name, a.url, f.url
                            )),
                            Some(_) => {}
                        }
                    }
                }
                ToolSource::Npm { package, extra } => {
                    for spec in std::iter::once(package).chain(extra) {
                        let row = pinned.artifacts.iter().find(|a| {
                            a.package == *name
                                && a.version == resolved.version
                                && a.target == "any"
                                && a.dist.as_deref() == Some(&spec.name)
                                && spec
                                    .version
                                    .as_ref()
                                    .is_none_or(|v| a.dist_version.as_ref() == Some(v))
                        });
                        match row {
                            None => problems.push(format!(
                                "{name} {}: no pinned npm tarball for {}",
                                resolved.version, spec.name
                            )),
                            Some(a) => {
                                if !a
                                    .integrity
                                    .as_deref()
                                    .is_some_and(|i| i.starts_with("sha512-"))
                                {
                                    problems.push(format!(
                                        "{name} {}: npm tarball {} has no sha512 integrity",
                                        resolved.version, a.name
                                    ));
                                }
                            }
                        }
                    }
                }
                ToolSource::Pypi { package, .. } => {
                    // 본 배포와, 고정 표에 적힌 의존 배포(bump 가 PyPI 메타데이터로 푼 것)마다 이 대상에서 설치할 휠이 있어야 한다.
                    let rows: Vec<&Artifact> = pinned
                        .artifacts
                        .iter()
                        .filter(|a| a.package == *name && a.version == resolved.version)
                        .collect();
                    let mut dists: BTreeSet<String> = rows
                        .iter()
                        .filter_map(|a| a.dist.as_deref().map(normalize_pypi))
                        .collect();
                    dists.insert(normalize_pypi(&package.name));
                    for dist in dists {
                        let covered = rows.iter().any(|a| {
                            (a.target == "any" || a.target == target.as_str())
                                && a.dist.as_deref().map(normalize_pypi).as_deref() == Some(&dist)
                        });
                        if !covered {
                            problems.push(format!(
                                "{name} {} {target}: no pinned wheel for {dist}",
                                resolved.version
                            ));
                        }
                    }
                }
            }
        }
    }
    for a in &pinned.artifacts {
        if !pinned.is_bundled(&a.package) {
            problems.push(format!(
                "artifact {} is for {} which is not bundled",
                a.name, a.package
            ));
        } else if !versions.contains(&(a.package.clone(), a.version.clone())) {
            problems.push(format!(
                "artifact {} pins {} {} but the snapshot resolves another version",
                a.name, a.package, a.version
            ));
        }
        if a.target != "any" && ToolTarget::parse(&a.target).is_none() {
            problems.push(format!(
                "artifact {} has unknown target {}",
                a.name, a.target
            ));
        }
        if !is_sha256(&a.sha256) {
            problems.push(format!(
                "artifact {} ({}) has no lowercase SHA-256",
                a.name, a.target
            ));
        }
        if a.size == 0 {
            problems.push(format!("artifact {} ({}) has size 0", a.name, a.target));
        }
        if !["download", "github-api", "pypi", "vendor-sha256"].contains(&a.hash_from.as_str()) {
            problems.push(format!(
                "artifact {} ({}) has unknown hash_from {}",
                a.name, a.target, a.hash_from
            ));
        }
        if !a.url.starts_with("https://") {
            problems.push(format!("artifact {} ({}) is not https", a.name, a.target));
        }
    }
    problems
}

#[cfg(test)]
mod tests;
