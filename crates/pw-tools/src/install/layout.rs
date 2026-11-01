//! `$DATA/tools` 배치와 설치 기록(15-m4-plan.md M4-04 ②), 설치본 찾기(`ToolLocator` — M4-02 ⑦).
//!
//! ```text
//! $DATA/tools/
//!   bin/<이름>                    실행 링크(PATH 에 넣지 않는다 — Pitwall 은 설치 기록의 절대 경로로 부른다). 유닉스만.
//!   <패키지>.lock                 설치 잠금 파일(다른 프로세스의 앱 포함 — 패키지 폴더를 지워도 남는다)
//!   <패키지>/install.json         설치 기록: 지금 판 · 이전 판(되돌리기) · 판마다 출처 · 받은 파일 해시 · 체크섬 유무 · 라이선스 · 크기 · 때
//!   <패키지>/<판>/                설치 폴더. 만드는 동안은 `.pitwall-installing` 표지가 있다
//! ```
//!
//! 새 판은 자기 폴더에 다 만든 뒤 설치 기록의 `current` 와 실행 링크만 바꾸고, 이전 판 하나(`previous`)를 남긴다. 해석은 설치 기록의
//! `current` 만 보고 표지가 있는 폴더는 고르지 않는다 — 설치 중인 판은 기록에 아직 없다.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use pw_lsp_servers::{Located, ToolLocator};
use serde::{Deserialize, Serialize};

use crate::methods::ToolChecksum;
use crate::package::PurlKind;

pub const RECORD_FILE: &str = "install.json";
/// 만드는 중인 설치 폴더의 표지.
pub const INSTALLING_MARK: &str = ".pitwall-installing";
const SCHEMA_VERSION: u32 = 1;
/// 설치 기록 파일의 상한(손상 · 남이 만든 큰 파일을 통째로 읽지 않게).
const MAX_RECORD_BYTES: u64 = 1024 * 1024;

/// `$DATA/tools`.
#[derive(Debug, Clone)]
pub struct Layout {
    root: PathBuf,
}

impl Layout {
    pub fn new(root: PathBuf) -> Self {
        Layout { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn bin_dir(&self) -> PathBuf {
        self.root.join("bin")
    }

    pub fn package_dir(&self, package: &str) -> PathBuf {
        self.root.join(package)
    }

    pub fn version_dir(&self, package: &str, version: &str) -> PathBuf {
        self.package_dir(package).join(version_dir_name(version))
    }

    pub fn lock_file(&self, package: &str) -> PathBuf {
        self.root.join(format!("{package}.lock"))
    }

    pub fn record_path(&self, package: &str) -> PathBuf {
        self.package_dir(package).join(RECORD_FILE)
    }

    /// 설치 기록. 없으면 `Ok(None)`, 읽지 못하면 오류.
    pub fn read_record(&self, package: &str) -> Result<Option<InstallRecord>, String> {
        let path = self.record_path(package);
        let meta = match fs::metadata(&path) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.to_string()),
        };
        if meta.len() > MAX_RECORD_BYTES {
            return Err(format!(
                "{RECORD_FILE} is larger than {MAX_RECORD_BYTES} bytes"
            ));
        }
        let text = fs::read(&path).map_err(|e| e.to_string())?;
        let record: InstallRecord =
            serde_json::from_slice(&text).map_err(|e| format!("{RECORD_FILE}: {e}"))?;
        if record.schema_version != SCHEMA_VERSION {
            return Err(format!(
                "{RECORD_FILE} has schema version {} (this Pitwall reads {SCHEMA_VERSION})",
                record.schema_version
            ));
        }
        Ok(Some(record))
    }

    pub fn write_record(&self, package: &str, record: &InstallRecord) -> Result<(), String> {
        let bytes = serde_json::to_vec_pretty(record).map_err(|e| e.to_string())?;
        write_atomic(&self.record_path(package), &bytes)
    }

    /// 설치한 패키지 이름들(설치 기록이 있는 폴더).
    pub fn packages(&self) -> Vec<String> {
        let Ok(entries) = fs::read_dir(&self.root) else {
            return Vec::new();
        };
        let mut out: Vec<String> = entries
            .filter_map(Result::ok)
            .filter(|e| e.path().join(RECORD_FILE).is_file())
            .filter_map(|e| e.file_name().into_string().ok())
            .collect();
        out.sort();
        out
    }

    /// 실행 링크를 지금 판으로 맞춘다(유닉스). 다른 패키지가 가진 같은 이름의 링크는 덮는다(나중 설치가 이긴다).
    pub fn link_bins(&self, package: &str, entry: &InstalledVersion) -> Result<(), String> {
        #[cfg(unix)]
        {
            let bin_dir = self.bin_dir();
            fs::create_dir_all(&bin_dir).map_err(|e| e.to_string())?;
            for (name, rel) in &entry.bins {
                let target = Path::new("..")
                    .join(package)
                    .join(version_dir_name(&entry.version))
                    .join(rel);
                let tmp = bin_dir.join(format!(".{name}.{}.tmp", std::process::id()));
                let _ = fs::remove_file(&tmp);
                std::os::unix::fs::symlink(&target, &tmp).map_err(|e| e.to_string())?;
                fs::rename(&tmp, bin_dir.join(name)).map_err(|e| e.to_string())?;
            }
        }
        #[cfg(not(unix))]
        {
            let _ = (package, entry);
        }
        Ok(())
    }

    /// 이 패키지 폴더를 가리키는 실행 링크를 지운다.
    pub fn unlink_bins(&self, package: &str) {
        let Ok(entries) = fs::read_dir(self.bin_dir()) else {
            return;
        };
        let prefix = Path::new("..").join(package);
        for entry in entries.filter_map(Result::ok) {
            if let Ok(target) = fs::read_link(entry.path())
                && target.starts_with(&prefix)
            {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
}

/// 판 이름을 폴더 이름으로: `/` 가 든 판(`kotlin-lsp/v263…`)은 `_` 로. 판은 M4-03 이 `-` 로 시작하지 않는 것만 남긴다.
pub fn version_dir_name(version: &str) -> String {
    version
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' => '_',
            c => c,
        })
        .collect()
}

/// 원자적으로 쓴다(같은 폴더의 임시 파일 → 동기화 → 이름 바꾸기).
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let dir = path.parent().ok_or("no parent folder")?;
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let name = path
        .file_name()
        .and_then(OsStr::to_str)
        .ok_or("bad file name")?;
    let tmp = dir.join(format!(".{name}.{}.tmp", std::process::id()));
    {
        let mut f = fs::File::create(&tmp).map_err(|e| e.to_string())?;
        f.write_all(bytes).map_err(|e| e.to_string())?;
        f.sync_all().map_err(|e| e.to_string())?;
    }
    fs::rename(&tmp, path).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        e.to_string()
    })
}

/// `$DATA/tools/<패키지>/install.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallRecord {
    pub schema_version: u32,
    pub name: String,
    /// 해석이 고르는 판.
    pub current: String,
    /// 되돌리기로 돌아갈 판(하나만 남긴다).
    pub previous: Option<String>,
    /// 남아 있는 판들(`current` · `previous` 만).
    pub versions: BTreeMap<String, InstalledVersion>,
}

impl InstallRecord {
    pub fn new(name: &str, entry: InstalledVersion) -> Self {
        let current = entry.version.clone();
        InstallRecord {
            schema_version: SCHEMA_VERSION,
            name: name.to_owned(),
            current: current.clone(),
            previous: None,
            versions: BTreeMap::from([(current, entry)]),
        }
    }

    pub fn current_entry(&self) -> Option<&InstalledVersion> {
        self.versions.get(&self.current)
    }
}

/// 받은 파일 하나.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FetchedFile {
    pub name: String,
    pub url: String,
    pub sha256: String,
    pub size: u64,
}

/// venv 를 만든 python(그 python 이 사라지면 「다시 설치 필요」 — M4-02 해석).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsedRuntime {
    pub path: PathBuf,
    pub version: Option<String>,
}

/// 판 하나의 설치 기록.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledVersion {
    pub version: String,
    pub kind: PurlKind,
    /// 이 정의를 읽은 registry 판.
    pub registry_version: String,
    /// 받은 곳(파일 주소 · npm · PyPI 의 고정 주소).
    pub source_urls: Vec<String>,
    /// 설치기가 직접 받은 파일(github · generic)과 그 해시. npm · pypi 는 고정 표의 행(npm · pip 이 받으며 확인한 것).
    pub files: Vec<FetchedFile>,
    /// 받은 것을 고정 해시로 확인했나(`none` 이면 「체크섬 없음」 — 결정 M4-D17).
    pub checksum: ToolChecksum,
    /// 확인 다이얼로그에 보인 라이선스(덮어쓰기 표 반영).
    pub licenses: Vec<String>,
    /// 설치 폴더의 바이트.
    pub size: u64,
    /// 유닉스 밀리초.
    pub installed_at_ms: u64,
    /// 실행 파일 이름 → 설치 폴더 기준 경로.
    pub bins: BTreeMap<String, String>,
    /// pypi: venv 를 만든 python.
    #[serde(default)]
    pub python: Option<UsedRuntime>,
    /// npm: 설치에 쓴 node.
    #[serde(default)]
    pub node: Option<UsedRuntime>,
}

/// 설치본 찾기(M4-02 의 [`ToolLocator`]).
#[derive(Debug, Clone)]
pub struct InstalledTools {
    layout: Layout,
}

impl InstalledTools {
    pub fn new(layout: Layout) -> Self {
        InstalledTools { layout }
    }
}

impl ToolLocator for InstalledTools {
    fn locate(&self, package: &str, bin: &str) -> Located {
        if !crate::package::valid_file_name(package) {
            return Located::Absent;
        }
        let record = match self.layout.read_record(package) {
            Ok(Some(r)) => r,
            Ok(None) => return Located::Absent,
            Err(e) => {
                return Located::Broken {
                    reason: format!("the install record cannot be read: {e}"),
                };
            }
        };
        let Some(entry) = record.current_entry() else {
            return Located::Broken {
                reason: "the install record has no current version".into(),
            };
        };
        let dir = self.layout.version_dir(package, &entry.version);
        if dir.join(INSTALLING_MARK).exists() || !dir.is_dir() {
            // 같은 판을 다시 설치하는 중이거나 폴더가 사라졌다 — 고르지 않는다.
            return Located::Broken {
                reason: format!(
                    "{} {} is being installed or missing",
                    package, entry.version
                ),
            };
        }
        let Some(rel) = entry.bins.get(bin) else {
            return Located::Absent;
        };
        if let Some(python) = &entry.python
            && !python.path.exists()
        {
            return Located::Broken {
                reason: format!(
                    "the python that made its environment is gone ({}) — reinstall",
                    python.path.display()
                ),
            };
        }
        let path = dir.join(rel);
        if !path.is_file() {
            return Located::Broken {
                reason: format!("{bin} is missing from the install folder — reinstall"),
            };
        }
        Located::Installed {
            path,
            version: entry.version.clone(),
        }
    }
}

/// 폴더의 바이트(링크는 따라가지 않는다).
pub fn dir_size(path: &Path) -> u64 {
    let mut total = 0;
    let mut stack = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for e in entries.filter_map(Result::ok) {
            let Ok(meta) = fs::symlink_metadata(e.path()) else {
                continue;
            };
            if meta.is_dir() {
                stack.push(e.path());
            } else if meta.is_file() {
                total += meta.len();
            }
        }
    }
    total
}
