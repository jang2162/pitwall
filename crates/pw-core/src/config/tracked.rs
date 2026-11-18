//! 저장소 안의 개인 덮어쓰기(`.pitwall/**/*.local.toml`)가 **git 이 추적하는 파일인가**(17-m6-plan m6-localpolicy, 05 「권한 모델」).
//!
//! 개인 파일은 「사람의 설정」 이라 넓힐 수 있다(`[mcp.policy]` · `auto_approve` · 노출 · 미신뢰 프로젝트의 언어 서버 명령). 그런데
//! 저장소가 그 파일을 강제로 커밋하면 클론 · 새 워크트리에서 남이 쓴 파일이 사람의 설정이 된다. 그래서 **인덱스에 있거나, 그 내용이
//! 객체 DB 에 있는**(git 이 받아 온 내용 — checkout 이 중간에 실패한 클론 · pull · 브랜치 전환은 파일을 쓰고 인덱스에 올리지 못한다)
//! 개인 파일은 사람의 레이어로 받지 않고 공유 레이어로 읽는다([`super::ConfigSource`] — 좁히기만). 판정하지 못하면(관리 폴더를 못
//! 찾음 · 인덱스나 객체 DB 를 읽지 못함 · SHA-1 이 아닌 저장소 · 인덱스가 없는데 HEAD 가 커밋을 가리킴 · 객체가 다 있다고 할 수 없는
//! shallow · partial clone 저장소에서 인덱스에 없는 파일) 역시 받지 않는다(fail-closed). `.git` 이 없는 폴더(저장소가 아님)는 추적
//! 파일이 없다고 확정되므로 사람의 것이다. 갓 `git init` 한 저장소(인덱스도 커밋도 없음)도 그렇다.
//!
//! 내용 판정의 오탐은 git 이 한 번이라도 저장한 내용과 바이트가 같은 개인 파일이다(실수로 `git add` 했다가 `git rm --cached` 한 직후
//! 등) — 좁히는 쪽이고, 그 파일을 고치면 풀린다.
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
    /// 인덱스에는 없지만 그 내용이 객체 DB 에 있다 — git 이 받아 온 내용이다(checkout 이 중간에 실패한 클론 · pull · 브랜치 전환).
    /// 사람의 레이어로 받지 않고, Pitwall 이 쓰지도 않는다(키 하나를 쓰면 내용이 바뀌어 남은 남의 값까지 사람의 설정이 된다 —
    /// 사람이 손으로 보고 고친다).
    Fetched,
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
    judge(checkout, file).unwrap_or_else(Tracking::Unknown)
}

fn judge(checkout: &Path, file: &Path) -> Result<Tracking, String> {
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
        return Ok(Tracking::Untracked);
    };
    let (gitdir, common) = super::gitdir::repository_dirs(top)?;
    sha1_repository(&common)?;
    let mut rels = vec![relative(top, &lexical)?];
    // 링크를 지나면(`.pitwall` 이나 파일이 링크) 실제로 닿는 파일도 본다 — 저장소 안이면.
    if let Ok(real) = fs::canonicalize(file)
        && real != lexical
        && let Ok(rel) = relative(top, &real)
    {
        rels.push(rel);
    }
    match index_paths(&gitdir.join("index"))? {
        Some(paths) if rels.iter().any(|rel| covered(&paths, rel)) => return Ok(Tracking::Tracked),
        Some(_) => {}
        // 인덱스가 없는데 HEAD 가 커밋을 가리킨다 — checkout 이 실패한 클론처럼 작업 트리가 반쯤 써졌을 수 있다.
        None if head_is_born(&gitdir, &common)? => {
            return Err(format!(
                "{}: the index is missing although HEAD points to a commit (an interrupted clone or checkout?)",
                gitdir.display()
            ));
        }
        None => {}
    }
    Ok(if content_in_object_database(&common, file)? {
        Tracking::Fetched
    } else {
        Tracking::Untracked
    })
}

/// 인덱스에 없는 `file` 의 내용이 객체 DB 에 있는가(없는 파일이면 아니다). 객체가 다 있다고 할 수 없는 저장소(shallow · partial
/// clone)와 객체 DB · 대체 저장소(alternates)를 읽지 못하면 오류(「모름」).
fn content_in_object_database(common: &Path, file: &Path) -> Result<bool, String> {
    let bytes = match fs::metadata(file) {
        Ok(meta) if meta.is_file() => {
            if meta.len() > super::robust::MAX_CONFIG_BYTES {
                return Err(format!("{}: larger than a settings file", file.display()));
            }
            fs::read(file).map_err(|e| format!("{}: {e}", file.display()))?
        }
        Ok(_) => return Ok(false),
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(format!("{}: {e}", file.display())),
    };
    incomplete_object_database(common)?;
    let objects = common.join("objects");
    // 객체 폴더가 아예 없으면(git 이 저장소로 보지 않는 `.git`) 받아 온 내용도 없다.
    if matches!(fs::symlink_metadata(&objects), Err(e) if e.kind() == io::ErrorKind::NotFound) {
        return Ok(false);
    }
    readable_object_dirs(&objects, 0)?;
    let id = gix_object::compute_hash(Kind::Sha1, gix_object::Kind::Blob, &bytes)
        .map_err(|e| format!("{}: {e}", file.display()))?;
    let odb =
        gix_odb::at(&objects, Kind::Sha1).map_err(|e| format!("{}: {e}", objects.display()))?;
    use gix_object::FindHeader as _;
    match odb.try_header(&id) {
        Ok(found) => Ok(found.is_some()),
        Err(e) => Err(format!("{}: {e}", objects.display())),
    }
}

/// shallow(`shallow` 파일) · partial clone(`partialclone` · `promisor` 설정, `.promisor` 팩) 저장소는 받아 온 내용이 객체 DB 에 다
/// 있다고 할 수 없다.
fn incomplete_object_database(common: &Path) -> Result<(), String> {
    if fs::symlink_metadata(common.join("shallow")).is_ok() {
        return Err(format!(
            "{}: a shallow repository may not hold every fetched file",
            common.display()
        ));
    }
    let config = super::gitdir::read_config_or_empty(&common.join("config"))?;
    let partial = config.lines().any(|line| {
        let Some((key, value)) = line.split_once('=') else {
            return false;
        };
        let key = key.trim();
        let value = value.split(['#', ';']).next().unwrap_or("").trim();
        key.eq_ignore_ascii_case("partialclone")
            || (key.eq_ignore_ascii_case("promisor") && !value.eq_ignore_ascii_case("false"))
    });
    let promisor_pack = fs::read_dir(common.join("objects/pack")).is_ok_and(|dir| {
        dir.filter_map(Result::ok)
            .any(|e| e.file_name().to_string_lossy().ends_with(".promisor"))
    });
    if partial || promisor_pack {
        return Err(format!(
            "{}: a partial clone may not hold every fetched file",
            common.display()
        ));
    }
    Ok(())
}

/// 객체 폴더와 그 대체 저장소(`info/alternates` — 한 줄에 하나, 상대 경로는 그 객체 폴더 기준)를 다 읽을 수 있는가. 못 읽는 것을
/// 조용히 건너뛰면 「없음」 이 되어 넓어진다.
fn readable_object_dirs(objects: &Path, depth: usize) -> Result<(), String> {
    if depth > 5 {
        return Err(format!("{}: alternates nest too deep", objects.display()));
    }
    fs::read_dir(objects).map_err(|e| format!("{}: {e}", objects.display()))?;
    let pack = objects.join("pack");
    match fs::read_dir(&pack) {
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("{}: {e}", pack.display())),
    }
    let list = objects.join("info/alternates");
    let text = match fs::read_to_string(&list) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(format!("{}: {e}", list.display())),
    };
    for line in text.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let path = Path::new(line);
        let alternate = if path.is_absolute() {
            path.to_path_buf()
        } else {
            objects.join(path)
        };
        readable_object_dirs(&alternate, depth + 1)?;
    }
    Ok(())
}

/// HEAD 가 커밋을 가리키는가(갓 `git init` 의 태어나지 않은 브랜치면 아니다). `HEAD` 가 없으면 아니다(저장소로 보이지 않는 `.git`).
/// ref 는 느슨한 파일(관리 폴더 · 공통 폴더)과 `packed-refs` 에서 찾는다. reftable 저장소 · 읽지 못하는 꼴은 오류(「모름」).
fn head_is_born(gitdir: &Path, common: &Path) -> Result<bool, String> {
    if fs::symlink_metadata(common.join("reftable")).is_ok() {
        return Err(format!(
            "{}: reftable ref storage is not supported",
            common.display()
        ));
    }
    let head = gitdir.join("HEAD");
    let mut value = match fs::read_to_string(&head) {
        Ok(text) => text.trim().to_owned(),
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(format!("{}: {e}", head.display())),
    };
    for _ in 0..5 {
        let Some(name) = value.strip_prefix("ref:").map(str::trim) else {
            return ObjectId::from_hex(value.as_bytes())
                .map(|_| true)
                .map_err(|e| format!("{}: {e}", head.display()));
        };
        if !name.starts_with("refs/") || name.split('/').any(|p| p.is_empty() || p == "..") {
            return Err(format!("{}: unexpected ref `{name}`", head.display()));
        }
        let mut next = None;
        for dir in [gitdir, common] {
            match fs::read_to_string(dir.join(name)) {
                Ok(text) => {
                    next = Some(text.trim().to_owned());
                    break;
                }
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
                    ) => {}
                Err(e) => return Err(format!("{}: {e}", dir.join(name).display())),
            }
        }
        if next.is_none() {
            let packed = common.join("packed-refs");
            match fs::read_to_string(&packed) {
                Ok(text) => {
                    next = text
                        .lines()
                        .filter(|l| !l.starts_with('#') && !l.starts_with('^'))
                        .filter_map(|l| l.split_once(' '))
                        .find(|(_, refname)| refname.trim() == name)
                        .map(|(sha, _)| sha.to_owned());
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(format!("{}: {e}", packed.display())),
            }
        }
        match next {
            Some(found) => value = found,
            // 태어나지 않은 브랜치.
            None => return Ok(false),
        }
    }
    Err(format!("{}: symbolic refs nest too deep", head.display()))
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
