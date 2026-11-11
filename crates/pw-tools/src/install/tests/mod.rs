//! 설치기 시험(15-m4-plan.md M4-04 완료 기준 ① ~ ⑨). 가짜 HTTP 서버(`crate::test_http`)와 가짜 npm · python 실행 파일로 돈다.
//! 정의의 `https://fake.test` 주소를 가짜 서버로 바꿔 받는다 — 정의의 「https 만」 검사는 그대로 지난다.

mod archive;
mod consent;
mod files;
mod grant;
mod launcher;
mod locator;
#[cfg(unix)]
mod npm_pip;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use pw_core::bus::{EventBus, Scope};
use pw_core::task::TaskSpawner;
use pw_runtime::RuntimeEnv;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::*;
use crate::fetch::{Fetcher, sha256_hex};
use crate::index::Index;
use crate::overrides::Overrides;
use crate::pinned::Pinned;
use crate::test_http::FakeServer;

pub const FAKE: &str = "https://fake.test";

pub fn target() -> ToolTarget {
    ToolTarget::current().expect("tests run on a supported machine")
}

/// 설정 창의 허가.
pub fn settings_grant() -> ToolGrant {
    tool_grant(
        &pw_rpc::ConnectionScope::Host,
        Some("settings"),
        &|l: &str| l == "settings",
    )
    .unwrap()
}

/// 프로젝트 창의 허가.
pub fn project_grant(id: &str) -> ToolGrant {
    tool_grant(
        &pw_rpc::ConnectionScope::Project(id.into()),
        None,
        &|_: &str| false,
    )
    .unwrap()
}

/// 고정 해시 한 줄.
#[derive(Clone)]
pub struct Row {
    pub package: String,
    pub version: String,
    pub target: String,
    pub name: String,
    pub url: String,
    pub sha256: String,
    pub size: u64,
    pub dist: Option<(String, String)>,
    pub integrity: Option<String>,
}

impl Row {
    pub fn file(package: &str, version: &str, name: &str, path: &str, body: &[u8]) -> Row {
        Row {
            package: package.into(),
            version: version.into(),
            target: target().as_str().into(),
            name: name.into(),
            url: format!("{FAKE}{path}"),
            sha256: sha256_hex(body),
            size: body.len() as u64,
            dist: None,
            integrity: None,
        }
    }
}

pub fn pinned_toml(rows: &[Row]) -> Pinned {
    let zero = "0".repeat(64);
    let mut s = format!(
        "bundled = []\n[registry]\nversion = \"test\"\nurl = \"https://x.test/registry.json.zip\"\nsha256 = \"{zero}\"\njson_sha256 = \"{zero}\"\n"
    );
    for r in rows {
        s.push_str(&format!(
            "[[artifact]]\npackage = \"{}\"\nversion = \"{}\"\ntarget = \"{}\"\nname = \"{}\"\nurl = \"{}\"\nsha256 = \"{}\"\nsize = {}\nhash_from = \"download\"\n",
            r.package, r.version, r.target, r.name, r.url, r.sha256, r.size
        ));
        if let Some((d, v)) = &r.dist {
            s.push_str(&format!("dist = \"{d}\"\ndist_version = \"{v}\"\n"));
        }
        if let Some(i) = &r.integrity {
            s.push_str(&format!("integrity = \"{i}\"\n"));
        }
    }
    Pinned::parse(&s).unwrap()
}

/// `generic` 패키지: `files` 는 (저장 이름[:풀 곳], 가짜 서버 경로).
pub fn files_pkg(
    name: &str,
    version: &str,
    files: &[(&str, &str)],
    bin: &[(&str, &str)],
    licenses: &[&str],
) -> Value {
    let files: serde_json::Map<String, Value> = files
        .iter()
        .map(|(n, p)| ((*n).to_owned(), json!(format!("{FAKE}{p}"))))
        .collect();
    let bin: serde_json::Map<String, Value> = bin
        .iter()
        .map(|(n, p)| ((*n).to_owned(), json!(p)))
        .collect();
    json!({
        "name": name, "description": "test", "homepage": "https://example.test",
        "licenses": licenses, "languages": [], "categories": ["LSP"],
        "source": {"id": format!("pkg:generic/test/{name}@{version}"), "download": {"files": files}},
        "bin": bin,
    })
}

/// 가짜 npm · python 시험(유닉스)만 쓴다.
#[cfg(unix)]
pub fn npm_pkg(name: &str, version: &str, extra: &[&str]) -> Value {
    json!({
        "name": name, "description": "test", "homepage": "https://example.test",
        "licenses": ["MIT"], "languages": [], "categories": ["LSP"],
        "source": {"id": format!("pkg:npm/{name}@{version}"), "extra_packages": extra},
        "bin": {name: format!("npm:{name}")},
    })
}

/// 가짜 npm · python 시험(유닉스)만 쓴다.
#[cfg(unix)]
pub fn pypi_pkg(name: &str, version: &str) -> Value {
    json!({
        "name": name, "description": "test", "homepage": "https://example.test",
        "licenses": ["MIT"], "languages": [], "categories": ["LSP"],
        "source": {"id": format!("pkg:pypi/{name}@{version}")},
        "bin": {name: format!("pypi:{name}")},
    })
}

pub fn registry(packages: Vec<Value>, rows: &[Row]) -> Arc<ToolsRegistry> {
    registry_with_overrides(packages, rows, &[])
}

/// 덮어쓰기 정의 `(패키지 이름, TOML 글)` 를 함께 둔 registry.
pub fn registry_with_overrides(
    packages: Vec<Value>,
    rows: &[Row],
    overrides: &[(&str, &str)],
) -> Arc<ToolsRegistry> {
    let json = json!({"registry": "test", "packages": packages}).to_string();
    let (overrides, errors) = Overrides::from_files(overrides);
    assert!(errors.is_empty(), "{errors:?}");
    Arc::new(ToolsRegistry::with_parts(
        pinned_toml(rows),
        overrides,
        Index::from_snapshot_json(&json).unwrap(),
    ))
}

/// npm · python 을 찾을 환경(가짜).
pub struct FakeChains {
    pub login: Mutex<RuntimeEnv>,
    pub project: Mutex<RuntimeEnv>,
    pub asked: Mutex<Vec<Option<String>>>,
}

impl Toolchains for FakeChains {
    fn login_shell(&self) -> RuntimeEnv {
        self.login.lock().unwrap().clone()
    }

    fn lookup(&self, project: Option<&str>) -> Result<RuntimeEnv, String> {
        self.asked.lock().unwrap().push(project.map(str::to_owned));
        Ok(match project {
            None => self.login_shell(),
            Some(_) => self.project.lock().unwrap().clone(),
        })
    }
}

/// 가짜 npm · python 시험(유닉스)만 쓴다.
#[cfg(unix)]
pub fn env_with_path(path: &[&Path]) -> RuntimeEnv {
    RuntimeEnv {
        path: std::env::join_paths(path).unwrap(),
        ..RuntimeEnv::default()
    }
}

pub struct Fx {
    pub tmp: tempfile::TempDir,
    pub server: FakeServer,
    pub tasks: TaskSpawner,
    pub chains: Arc<FakeChains>,
}

impl Fx {
    pub async fn new() -> Fx {
        let tmp = tempfile::tempdir().unwrap();
        let server = FakeServer::start().await;
        let tasks = TaskSpawner::new(EventBus::new(64), Scope::Host, CancellationToken::new());
        let chains = Arc::new(FakeChains {
            login: Mutex::new(RuntimeEnv::default()),
            project: Mutex::new(RuntimeEnv::default()),
            asked: Mutex::new(Vec::new()),
        });
        Fx {
            tmp,
            server,
            tasks,
            chains,
        }
    }

    pub fn tools(&self) -> PathBuf {
        self.tmp.path().join("data").join("tools")
    }

    pub fn state(&self) -> PathBuf {
        self.tmp.path().join("state")
    }

    pub fn layout(&self) -> Layout {
        Layout::new(self.tools())
    }

    pub fn installer(&self, registry: Arc<ToolsRegistry>, limits: Limits) -> Arc<ToolsInstaller> {
        self.installer_with(registry, limits, None)
    }

    /// 받기를 마친 뒤 · 풀기 전에 `after_download` 를 부르는 설치기.
    pub fn installer_with(
        &self,
        registry: Arc<ToolsRegistry>,
        limits: Limits,
        after_download: Option<AfterDownload>,
    ) -> Arc<ToolsInstaller> {
        let mut installer = ToolsInstaller::with_limits(registry, limits).with_test_network(
            Fetcher::for_install_loopback_tests(),
            (FAKE.to_owned(), self.server.base.clone()),
        );
        if let Some(hook) = after_download {
            installer = installer.with_after_download(hook);
        }
        installer.attach(InstallerHost {
            tools_dir: self.tools(),
            state_dir: self.state(),
            tasks: self.tasks.clone(),
            toolchains: Arc::clone(&self.chains) as Arc<dyn Toolchains>,
            settings_window: Arc::new(|l: &str| l == "settings"),
        });
        Arc::new(installer)
    }

    /// 확인을 미리 남긴다(확인 다이얼로그를 지난 것과 같다).
    pub fn consent(&self, installer: &ToolsInstaller, name: &str) {
        let resolved = installer.resolve(name).unwrap();
        let rows = installer.registry().pinned().pinned_for(&resolved.0);
        let ask = installer.confirm_values(&resolved.0, &resolved.1, rows.as_deref());
        ConsentStore::new(&self.state())
            .record(
                name,
                ConsentEntry {
                    licenses: ask.licenses,
                    source_domains: ask.source_domains,
                    version: ask.version,
                    confirmed_at_ms: 1,
                    checksum: Some(ask.checksum),
                },
            )
            .unwrap();
    }

    /// `tmp` 안 모든 파일 이름(설치 폴더 밖에 무엇이 생겼는지 볼 때).
    pub fn all_files(&self) -> Vec<PathBuf> {
        let mut out = Vec::new();
        let mut stack = vec![self.tmp.path().to_path_buf()];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).unwrap().filter_map(Result::ok) {
                let m = std::fs::symlink_metadata(e.path()).unwrap();
                if m.is_dir() {
                    stack.push(e.path());
                } else {
                    out.push(e.path());
                }
            }
        }
        out
    }
}

/// 보관 파일 항목(시험용 — 위험한 이름도 그대로 쓴다).
#[derive(Clone)]
pub enum Entry {
    File(&'static str, Vec<u8>, u32),
    Dir(&'static str),
    Symlink(&'static str, &'static str),
    Hard(&'static str, &'static str),
}

/// `.tar.gz`. 이름 · 링크 대상은 머리말에 바이트 그대로 쓴다(`tar` 크레이트의 경로 검사를 지나지 않게).
pub fn targz(entries: &[Entry]) -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());
    for e in entries {
        let mut h = tar::Header::new_old();
        let (name, data): (&str, Vec<u8>) = match e {
            Entry::File(n, body, mode) => {
                h.set_entry_type(tar::EntryType::Regular);
                h.set_mode(*mode);
                (n, body.clone())
            }
            Entry::Dir(n) => {
                h.set_entry_type(tar::EntryType::Directory);
                h.set_mode(0o755);
                (n, Vec::new())
            }
            Entry::Symlink(n, t) | Entry::Hard(n, t) => {
                h.set_entry_type(if matches!(e, Entry::Symlink(..)) {
                    tar::EntryType::Symlink
                } else {
                    tar::EntryType::Link
                });
                h.set_mode(0o777);
                let old = h.as_old_mut();
                old.linkname[..t.len()].copy_from_slice(t.as_bytes());
                (n, Vec::new())
            }
        };
        h.set_size(data.len() as u64);
        let old = h.as_old_mut();
        old.name[..name.len()].copy_from_slice(name.as_bytes());
        h.set_cksum();
        builder.append(&h, data.as_slice()).unwrap();
    }
    let tar = builder.into_inner().unwrap();
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    std::io::Write::write_all(&mut gz, &tar).unwrap();
    gz.finish().unwrap()
}

/// `.zip`. 같은 이름 두 번은 `ZipWriter` 가 거절하므로 같은 길이의 임시 이름으로 쓴 뒤 바이트를 바꿔 넣는다. 폴더(`…/`)의 임시
/// 이름도 `/` 로 끝나야 한다 — `add_directory` 가 `/` 를 붙이므로 끝의 `/` 를 바꾸면 패치 뒤 이름이 `…//` 가 된다(리뷰 M1).
pub fn zip(entries: &[Entry]) -> Vec<u8> {
    use std::io::Write as _;
    let mut out = std::io::Cursor::new(Vec::new());
    let mut seen = std::collections::BTreeSet::new();
    let mut patches: Vec<(String, String)> = Vec::new();
    let mut unique = |n: &str| -> String {
        if seen.insert(n.to_owned()) {
            return n.to_owned();
        }
        let placeholder = match n.strip_suffix('/') {
            Some(dir) => format!("{}~/", &dir[..dir.len() - 1]),
            None => format!("{}~", &n[..n.len() - 1]),
        };
        patches.push((placeholder.clone(), n.to_owned()));
        placeholder
    };
    {
        let mut w = zip::ZipWriter::new(&mut out);
        for e in entries {
            match e {
                Entry::File(n, body, mode) => {
                    let opts = zip::write::SimpleFileOptions::default()
                        .compression_method(zip::CompressionMethod::Deflated)
                        .unix_permissions(*mode);
                    w.start_file(unique(n), opts).unwrap();
                    w.write_all(body).unwrap();
                }
                Entry::Dir(n) => {
                    w.add_directory(unique(n), zip::write::SimpleFileOptions::default())
                        .unwrap();
                }
                Entry::Symlink(n, t) => {
                    w.add_symlink(unique(n), *t, zip::write::SimpleFileOptions::default())
                        .unwrap();
                }
                Entry::Hard(..) => unreachable!("zip has no hard links"),
            }
        }
        w.finish().unwrap();
    }
    let mut bytes = out.into_inner();
    for (from, to) in patches {
        let (from, to) = (from.as_bytes(), to.as_bytes());
        let mut i = 0;
        while i + from.len() <= bytes.len() {
            if &bytes[i..i + from.len()] == from {
                bytes[i..i + from.len()].copy_from_slice(to);
            }
            i += 1;
        }
    }
    bytes
}

pub fn gz(body: &[u8]) -> Vec<u8> {
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    std::io::Write::write_all(&mut gz, body).unwrap();
    gz.finish().unwrap()
}

/// 오류 코드와 `data.stage`.
pub fn code_and_stage(e: &RpcError) -> (i32, Option<String>) {
    (
        e.code,
        e.data
            .as_ref()
            .and_then(|d| d.get("stage"))
            .and_then(Value::as_str)
            .map(str::to_owned),
    )
}

#[cfg(unix)]
pub fn mode_of(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::metadata(path).unwrap().permissions().mode() & 0o7777
}
