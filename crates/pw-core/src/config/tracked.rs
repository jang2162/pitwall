//! 저장소 안의 개인 덮어쓰기(`.pitwall/**/*.local.toml`)가 **git 이 추적하는 파일인가**(17-m6-plan m6-localpolicy, 05 「권한 모델」).
//!
//! 개인 파일은 「사람의 설정」 이라 넓힐 수 있다(`[mcp.policy]` · `auto_approve` · 노출 · 미신뢰 프로젝트의 언어 서버 명령). 그런데
//! 저장소가 그 파일을 강제로 커밋하면 클론 · 새 워크트리에서 남이 쓴 파일이 사람의 설정이 된다. 그래서 **인덱스에 있는** 개인 파일은
//! 사람의 레이어로 받지 않고 공유 레이어로 읽는다([`super::ConfigSource`] — 좁히기만). 판정하지 못하면(관리 폴더를 못 찾음 ·
//! 인덱스가 깨짐 · SHA-1 이 아닌 저장소) 역시 받지 않는다(fail-closed). `.git` 이 없는 폴더(저장소가 아님)는 추적 파일이 없다고
//! 확정되므로 사람의 것이다. 인덱스가 아직 없는 저장소(갓 `git init`)도 추적 파일이 없다.
//!
//! git 을 띄우지 않고 인덱스를 직접 읽는다(`gix-index` — split index · v4 · sparse 를 푼다). 경로는 대소문자를 가리지 않고 비교하고
//! (대소문자를 가리지 않는 파일 시스템), 파일 자신뿐 아니라 **조상 경로가 인덱스 항목**(커밋된 링크 `.pitwall` · sparse 폴더 항목)이어도
//! 추적으로 본다. 링크를 지나 저장소 안의 다른 파일에 닿으면 그 실제 경로도 본다.
//!
//! **캐시**: 인덱스는 수십 MB 일 수 있고 정책 읽기는 MCP 호출마다 여러 번이라, 읽은 인덱스의 경로 집합을 그 **인덱스의 체크섬**
//! (git 이 파일 끝에 적는 내용 해시 — `gix-index` 가 읽을 때 검증한다) 열쇠로 둔다. 다음 판정은 파일 끝의 체크섬만 읽어 같으면 쓴다 —
//! 내용이 다르면 체크섬이 다르므로 크기 · 수정 시각 · inode 가 같아도 낡은 집합을 쓰지 않는다. `index.skipHash`(체크섬이 0)인
//! 인덱스는 캐시하지 않는다.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};

use gix_index::hash::{Kind, ObjectId};

/// 추적 판정.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tracking {
    /// 인덱스에 없다(또는 저장소 밖). 사람의 파일이다.
    Untracked,
    /// 인덱스에 있다(또는 조상이 인덱스 항목이다). 커밋되는 파일이다.
    Tracked,
    /// 판정하지 못했다 — 까닭은 사람의 화면에 보일 글(파일 내용은 없다).
    Unknown(String),
}

impl Tracking {
    /// 사람의 레이어로 받아도 되는가.
    pub fn is_personal(&self) -> bool {
        matches!(self, Tracking::Untracked)
    }
}

/// 체크아웃 `checkout`(`.pitwall` 을 담는 폴더) 안의 파일 `file` 을 판정한다. `file` 은 `checkout` 아래 경로다(없는 파일이어도 된다 —
/// 쓰기 전에 부른다).
pub fn tracking(checkout: &Path, file: &Path) -> Tracking {
    match judge(checkout, file) {
        Ok(true) => Tracking::Tracked,
        Ok(false) => Tracking::Untracked,
        Err(reason) => Tracking::Unknown(reason),
    }
}

fn judge(checkout: &Path, file: &Path) -> Result<bool, String> {
    let real_checkout =
        fs::canonicalize(checkout).map_err(|e| format!("{}: {e}", checkout.display()))?;
    let inside = file
        .strip_prefix(checkout)
        .map_err(|_| format!("{} is not inside {}", file.display(), checkout.display()))?;
    let lexical = real_checkout.join(inside);
    let Some(top) = real_checkout
        .ancestors()
        .find(|dir| fs::metadata(dir.join(".git")).is_ok())
    else {
        // 저장소가 아니다 — 추적 파일이 없다.
        return Ok(false);
    };
    let (gitdir, common) = super::gitdir::repository_dirs(top)?;
    sha1_repository(&common)?;
    let Some(paths) = index_paths(&gitdir.join("index"))? else {
        return Ok(false);
    };
    let mut rels = vec![relative(top, &lexical)?];
    // 링크를 지나면(`.pitwall` 이나 파일이 링크) 실제로 닿는 파일도 본다 — 저장소 안이면.
    if let Ok(real) = fs::canonicalize(file)
        && real != lexical
        && let Ok(rel) = relative(top, &real)
    {
        rels.push(rel);
    }
    Ok(rels.iter().any(|rel| covered(&paths, rel)))
}

/// `rel` 또는 그 조상이 인덱스 항목인가.
fn covered(paths: &HashSet<Vec<u8>>, rel: &[u8]) -> bool {
    let mut end = 0;
    loop {
        let next = rel[end..].iter().position(|b| *b == b'/').map(|i| end + i);
        let prefix = &rel[..next.unwrap_or(rel.len())];
        if paths.contains(prefix) {
            return true;
        }
        match next {
            Some(i) => end = i + 1,
            None => return false,
        }
    }
}

/// `top` 기준 `/` 경로(소문자). `path` 가 `top` 밖이면 오류.
fn relative(top: &Path, path: &Path) -> Result<Vec<u8>, String> {
    let rel = path
        .strip_prefix(top)
        .map_err(|_| format!("{} is outside {}", path.display(), top.display()))?;
    let mut out = Vec::new();
    for component in rel.components() {
        let Component::Normal(part) = component else {
            return Err(format!("{}: unexpected path component", path.display()));
        };
        let part = part
            .to_str()
            .ok_or_else(|| format!("{}: not UTF-8", path.display()))?;
        if !out.is_empty() {
            out.push(b'/');
        }
        out.extend(part.as_bytes().to_ascii_lowercase());
    }
    Ok(out)
}

/// 공통 `config`(없으면 기본값 — SHA-1)의 `objectformat` 이 SHA-1 이 아니면(SHA-256 저장소) 판정하지 않는다 — 이 앱의 gitoxide 는 SHA-1 만 켰다.
fn sha1_repository(common: &Path) -> Result<(), String> {
    let path = common.join("config");
    let config = super::gitdir::read_config_or_empty(&path)?;
    for line in config.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.trim().eq_ignore_ascii_case("objectformat") {
            let value = value.split(['#', ';']).next().unwrap_or("");
            let value = value.trim().trim_matches('"');
            if !value.eq_ignore_ascii_case("sha1") {
                return Err(format!(
                    "{}: object format `{value}` is not supported",
                    path.display()
                ));
            }
        }
    }
    Ok(())
}

/// 캐시: 인덱스 파일 → (그 내용의 체크섬, 경로 집합).
type Cache = HashMap<PathBuf, (ObjectId, Arc<HashSet<Vec<u8>>>)>;

static CACHE: LazyLock<Mutex<Cache>> = LazyLock::new(Mutex::default);
/// 캐시에 두는 인덱스 수 — 넘으면 비운다(프로젝트 · 워크트리 수만큼).
const CACHE_LIMIT: usize = 64;

/// 인덱스의 경로 집합(소문자, 끝 `/` 없음). 인덱스가 없으면 `None`(추적 파일 없음).
fn index_paths(index: &Path) -> Result<Option<Arc<HashSet<Vec<u8>>>>, String> {
    let label = |e: &dyn std::fmt::Display| format!("{}: {e}", index.display());
    let trailer = match trailer(index) {
        Ok(trailer) => trailer,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(label(&e)),
    };
    if let Some(trailer) = trailer
        && let Some((sum, paths)) = cache().get(index)
        && *sum == trailer
    {
        return Ok(Some(paths.clone()));
    }
    let options = gix_index::decode::Options {
        thread_limit: Some(1),
        ..Default::default()
    };
    let file = gix_index::File::at(index, Kind::Sha1, false, options).map_err(|e| label(&e))?;
    let paths: HashSet<Vec<u8>> = file
        .entries()
        .iter()
        .map(|entry| {
            let path: &[u8] = entry.path(&file).as_ref();
            // sparse 폴더 항목은 끝이 `/` 다.
            path.strip_suffix(b"/").unwrap_or(path).to_ascii_lowercase()
        })
        .collect();
    let paths = Arc::new(paths);
    if let Some(sum) = file.checksum() {
        let mut cache = cache();
        if cache.len() >= CACHE_LIMIT {
            cache.clear();
        }
        cache.insert(index.to_path_buf(), (sum, paths.clone()));
    }
    Ok(Some(paths))
}

fn cache() -> MutexGuard<'static, Cache> {
    CACHE.lock().unwrap_or_else(|e| e.into_inner())
}

/// 인덱스 끝의 체크섬(SHA-1 20바이트). 0 이면(`index.skipHash`) `None`.
fn trailer(index: &Path) -> io::Result<Option<ObjectId>> {
    let mut file = fs::File::open(index)?;
    let len = file.metadata()?.len();
    let size = Kind::Sha1.len_in_bytes() as u64;
    if len < size {
        return Ok(None);
    }
    file.seek(SeekFrom::Start(len - size))?;
    let mut bytes = [0u8; 20];
    file.read_exact(&mut bytes)?;
    let id = ObjectId::from_bytes_or_panic(&bytes);
    Ok((!id.is_null()).then_some(id))
}

#[cfg(test)]
#[path = "tracked_tests.rs"]
pub(crate) mod tests;
