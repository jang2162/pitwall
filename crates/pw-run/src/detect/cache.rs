//! 누를 때만 읽는 태스크 목록(gradle · maven — 결정 M6-D8 · D70)의 캐시: `$DATA/projects/<id>/run-tasks.json`.
//!
//! - **열쇠**([`key`]): 빌드 파일들(gradle: `build.gradle(.kts)` · `settings.gradle(.kts)` · `gradle.properties` ·
//!   `gradle-wrapper.properties` · `*.versions.toml`, maven: `pom.xml` 들 · `.mvn/` 의 파일)의 루트 기준 경로와 내용 sha256, 그리고
//!   런타임 환경의 원천 파일([`crate::seen::runtime_files`] — M6-11 F1 과 같은 목록)의 경로와 sha256. 무엇 하나 바뀌면 다른 열쇠다 —
//!   목록은 「읽기 전」(정적 목록)으로 돌아가고 창은 「다시 읽기」 를 보인다. 경로는 루트 기준이라 같은 내용의 다른 워크트리는 같은
//!   열쇠를 쓴다.
//! - **찾는 범위**: 루트 아래를 [`MAX_DEPTH`] 깊이 · [`MAX_ENTRIES`] 항목까지, 링크를 따르지 않고 숨은 폴더(`.mvn` 밖)와 산출물
//!   폴더(`build` · `target` · `node_modules` …)는 들어가지 않는다. 넘거나 빌드 파일이 [`MAX_FILE`] 을 넘으면 열쇠를 낼 수 없다 —
//!   캐시를 쓰지도 읽지도 않는다(까닭은 목록에 남는다). 빌드 파일이 부르는 다른 파일(`buildSrc` 의 소스, `apply from:` 의 스크립트)과 사람 수준의
//!   설정(`~/.gradle/gradle.properties` · `~/.gradle/init.d`, `~/.m2/settings.xml`)은 보지 않는다 — 그것만 바뀌면 「다시 읽기」 를
//!   사람이 누른다.
//! - **파일**: 판([`VERSION`])이 다르거나 깨졌으면 버린다(정적 목록으로). 감지기마다 최근 열쇠 [`KEPT`] 개를 둔다. 임시 파일 +
//!   rename 으로 쓴다. 이름만 담는다 — 태스크 설명 · 출력은 담지 않는다.

use std::collections::BTreeMap;
use std::fs;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// 캐시 파일 이름(`$DATA/projects/<id>/` 안).
pub const FILE: &str = "run-tasks.json";
/// 캐시 파일의 판. 꼴을 바꾸면 올린다 — 옛 파일은 버린다.
pub const VERSION: u32 = 1;
/// 감지기마다 두는 열쇠 수(워크트리를 오가도 다시 읽지 않게).
pub const KEPT: usize = 4;
/// 빌드 파일 찾기의 깊이 · 항목 상한.
pub const MAX_DEPTH: usize = 8;
pub const MAX_ENTRIES: usize = 20_000;
/// 빌드 파일 하나 · 캐시 파일의 크기 상한.
pub const MAX_FILE: u64 = 1024 * 1024;
const MAX_CACHE_FILE: u64 = 4 * 1024 * 1024;

/// 들어가지 않는 폴더(산출물 · 도구 캐시).
const SKIPPED_DIRS: &[&str] = &["build", "target", "node_modules", "out"];

/// 빌드 파일의 종류.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildFiles {
    Gradle,
    Maven,
}

impl BuildFiles {
    fn wants(self, rel: &Path) -> bool {
        let Some(name) = rel.file_name().and_then(|n| n.to_str()) else {
            return false;
        };
        match self {
            BuildFiles::Gradle => {
                matches!(
                    name,
                    "build.gradle"
                        | "build.gradle.kts"
                        | "settings.gradle"
                        | "settings.gradle.kts"
                        | "gradle.properties"
                        | "gradle-wrapper.properties"
                ) || name.ends_with(".versions.toml")
            }
            BuildFiles::Maven => {
                name == "pom.xml"
                    || rel
                        .components()
                        .next()
                        .is_some_and(|c| c.as_os_str() == ".mvn")
            }
        }
    }

    fn enters(self, name: &str) -> bool {
        if SKIPPED_DIRS.contains(&name) {
            return false;
        }
        !name.starts_with('.') || (self == BuildFiles::Maven && name == ".mvn")
    }
}

/// 루트의 캐시 열쇠(머리 주석). 빌드 파일이 하나도 없으면 그 빈 목록의 열쇠다.
pub fn key(root: &Path, kind: BuildFiles) -> Result<String, String> {
    let mut files = Vec::new();
    let mut seen = 0usize;
    walk(root, Path::new(""), 0, kind, &mut files, &mut seen)?;
    files.sort();
    let mut h = Sha256::new();
    h.update(format!("pitwall-run-tasks-key {VERSION}\n"));
    for rel in &files {
        let digest = digest(&root.join(rel))?;
        h.update(format!("build\0{}\0{digest}\n", slashed(rel)));
    }
    let real_root = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let home = std::env::var_os("HOME").map(PathBuf::from);
    for path in crate::seen::runtime_files(&real_root, home.as_deref()) {
        let shown = path
            .strip_prefix(&real_root)
            .map_or_else(|_| path.display().to_string(), slashed);
        let digest = digest(&path)?;
        h.update(format!("runtime\0{shown}\0{digest}\n"));
    }
    Ok(format!("sha256:{}", hex(&h.finalize())))
}

fn walk(
    root: &Path,
    rel: &Path,
    depth: usize,
    kind: BuildFiles,
    out: &mut Vec<PathBuf>,
    seen: &mut usize,
) -> Result<(), String> {
    let Ok(read) = fs::read_dir(root.join(rel)) else {
        return Ok(());
    };
    for entry in read.filter_map(Result::ok) {
        *seen += 1;
        if *seen > MAX_ENTRIES {
            return Err(format!(
                "the project has more than {MAX_ENTRIES} files where build files are looked for; Pitwall does not cache its task list"
            ));
        }
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        let child = rel.join(&name);
        // 링크는 따르지 않는다(루트 밖 · 순환).
        let Ok(meta) = fs::symlink_metadata(root.join(&child)) else {
            continue;
        };
        if meta.is_dir() {
            if depth + 1 < MAX_DEPTH && kind.enters(&name) {
                walk(root, &child, depth + 1, kind, out, seen)?;
            }
        } else if meta.is_file() && kind.wants(&child) {
            out.push(child);
        }
    }
    Ok(())
}

fn digest(path: &Path) -> Result<String, String> {
    let shown = || path.display().to_string();
    let file = fs::File::open(path).map_err(|e| format!("{}: {e}", shown()))?;
    let mut bytes = Vec::new();
    file.take(MAX_FILE + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("{}: {e}", shown()))?;
    if bytes.len() as u64 > MAX_FILE {
        return Err(format!(
            "{} is larger than {} KiB; Pitwall does not cache the task list",
            shown(),
            MAX_FILE / 1024
        ));
    }
    Ok(hex(&Sha256::digest(&bytes)))
}

fn slashed(rel: &Path) -> String {
    rel.components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct CacheFile {
    version: u32,
    /// 감지기 id → 최근 것부터.
    #[serde(default)]
    lists: BTreeMap<String, Vec<CachedList>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedList {
    key: String,
    tasks: Vec<String>,
}

/// 캐시를 찾은 결과.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lookup {
    /// 그 열쇠로 읽은 목록.
    Hit(Vec<String>),
    /// 읽은 적은 있으나 그 뒤 빌드 파일이 바뀌었다.
    Stale,
    /// 읽은 적이 없다(파일이 없거나 깨졌거나 판이 다르다).
    Missing,
}

fn read_file(dir: &Path) -> CacheFile {
    let path = dir.join(FILE);
    let Ok(file) = fs::File::open(&path) else {
        return CacheFile::default();
    };
    let mut bytes = Vec::new();
    if file
        .take(MAX_CACHE_FILE + 1)
        .read_to_end(&mut bytes)
        .is_err()
        || bytes.len() as u64 > MAX_CACHE_FILE
    {
        return CacheFile::default();
    }
    match serde_json::from_slice::<CacheFile>(&bytes) {
        Ok(file) if file.version == VERSION => file,
        Ok(_) | Err(_) => {
            tracing::info!(target: "pw_run", path = %path.display(), "discarding an unreadable or older task list cache");
            CacheFile::default()
        }
    }
}

/// `dir` 의 캐시에서 `detector` 의 목록. 그 감지기의 기록이 하나라도 있을 때만 열쇠를 계산한다(`key` — 빌드 파일을 찾는 일은
/// 목록을 부를 때마다 하지 않는다).
pub fn lookup(
    dir: &Path,
    detector: &str,
    key: impl FnOnce() -> Result<String, String>,
) -> Result<Lookup, String> {
    let file = read_file(dir);
    let Some(lists) = file.lists.get(detector).filter(|l| !l.is_empty()) else {
        return Ok(Lookup::Missing);
    };
    let key = key()?;
    Ok(lists
        .iter()
        .find(|l| l.key == key)
        .map_or(Lookup::Stale, |l| Lookup::Hit(l.tasks.clone())))
}

/// 같은 Host 의 두 쓰기가 서로의 열쇠를 지우지 않게.
static WRITING: Mutex<()> = Mutex::new(());

/// `detector` 의 `key` 목록을 쓴다(그 열쇠를 맨 앞으로, [`KEPT`] 개까지).
pub fn store(dir: &Path, detector: &str, key: &str, tasks: &[String]) -> Result<(), String> {
    let _guard = WRITING.lock().unwrap_or_else(PoisonError::into_inner);
    let mut file = read_file(dir);
    file.version = VERSION;
    let lists = file.lists.entry(detector.to_owned()).or_default();
    lists.retain(|l| l.key != key);
    lists.insert(
        0,
        CachedList {
            key: key.to_owned(),
            tasks: tasks.to_vec(),
        },
    );
    lists.truncate(KEPT);
    let text = serde_json::to_vec(&file).map_err(|e| e.to_string())?;
    // 읽는 쪽이 버릴 크기는 쓰지 않는다 — 다른 감지기의 기록까지 함께 사라진다.
    if text.len() as u64 > MAX_CACHE_FILE {
        return Err(format!(
            "the task list cache would be larger than {} MiB; Pitwall does not write it",
            MAX_CACHE_FILE / (1024 * 1024)
        ));
    }
    fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut tmp =
        tempfile::NamedTempFile::new_in(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    std::io::Write::write_all(&mut tmp, &text).map_err(|e| format!("{}: {e}", dir.display()))?;
    tmp.persist(dir.join(FILE))
        .map(drop)
        .map_err(|e| format!("{}: {}", dir.join(FILE).display(), e.error))
}
