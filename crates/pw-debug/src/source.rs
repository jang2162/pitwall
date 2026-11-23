//! 프레임 소스의 읽기 토큰(17-m6-plan M6-18 ② — 결정 M4-D13 과 같은 규칙, 메인 판정 2026-11-19).
//!
//! 디버그 대상이 멈춘 프레임의 소스가 프로젝트 루트 밖(표준 라이브러리 · 의존성 · 어댑터만 쥔 `sourceReference`)이면 창은 그 파일을
//! `fs.*` 로 읽을 수 없다(루트 판정은 그대로다). 그래서 `debug.stack` 이 그런 프레임에 토큰을 달고, 창은 그 토큰으로만
//! `debug.source` 를 부른다 — 읽기 전용 탭(외부 소스 탭과 같은 자리)으로 연다.
//!
//! - 토큰은 **어댑터가 스택 프레임으로 보고한 자리에만** 나온다. 디버그 대상은 믿을 수 없는 글을 보낼 수 있으므로(`/etc` 아래를
//!   프레임이라고 보고) 읽기를 좁힌다: 일반 파일만 · 5 MiB 까지 · 쓰기 없음. 경로는 토큰을 만들 때 실제 경로와 그 파일의 정체
//!   (unix 는 장치 · inode)로 풀어 두고, 읽을 때는 **그 실제 경로를 한 번 열어**(unix 는 `O_NOFOLLOW | O_NONBLOCK` — 마지막
//!   구성 요소가 링크 · FIFO 로 바뀌어도 따라가지 않고 멈추지 않는다) 연 핸들의 정보로 판정하고(일반 파일 · 같은 정체 · 크기) 그
//!   핸들에서 상한까지만 읽는다 — 확인과 읽기 사이에 파일을 바꿔 끼울 틈이 없다. 화면에는 그 실제 경로를 보인다(보고한 경로가
//!   다르면 함께 — `DebugSource.reported`).
//! - 어댑터가 `sourceReference > 0` 을 주면 경로가 있어도 내용은 `source` 요청으로 받는다(DAP 규약 — 디스크 글이 아니다).
//! - 실행 id 는 다시 쓰이지 않는다(실행 관리자) — 끝난 실행의 토큰은 `Debugger::find` 가 먼저 거절하고(끝이 알려진 실행 — 세션 표에서
//!   빠지기 전이라도, 그 까닭은 `find` 의 주석), `forget_run` 은 표를 비우는 메모리 정리다.
//! - 토큰은 128비트 글이고 (연결, 프로젝트, 실행, 자리)에 묶인다. 다른 연결 · 다른 실행에서는 없는 토큰과 같은 not found 다.
//! - 실행이 끝나면(`Debugger::finish`) 그 실행의 토큰이, 연결이 닫히면 그 연결의 토큰이 지워진다. 연결마다
//!   [`TOKENS_PER_CONNECTION`] 을 넘으면 가장 오래 쓰지 않은 것부터 지운다.
//! - `debug.source` 는 창 연결만 부른다(그 프로젝트 창 — 에이전트 · Host 범위는 `PERMISSION_DENIED`). 에이전트가 이것으로 루트 밖
//!   파일을 읽는 길이 되지 않게 한다.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::hash::{BuildHasher, Hasher};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use pw_rpc::{Ctx, RpcError, SessionId as Connection, code};
use pw_run::runs::RunId;

use crate::run::lock;

/// 토큰으로 읽는 글의 상한(바이트) — 결정 M4-D13 의 루트 밖 문서와 같다.
pub const SOURCE_LIMIT: u64 = 5 * 1024 * 1024;
/// 연결 하나가 들고 있는 토큰의 상한.
pub const TOKENS_PER_CONNECTION: usize = 1000;

/// 토큰이 가리키는 자리.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum SourceAt {
    /// 어댑터가 보고한 경로와 토큰을 만들 때 푼 실제 경로 · 그 파일의 정체.
    File {
        reported: String,
        real: PathBuf,
        id: FileId,
    },
    /// 어댑터가 내용을 쥔 소스(`source` 요청) — 그 세션의 참조와 어댑터가 준 이름.
    Reference {
        session: pw_dap::SessionId,
        reference: i64,
        name: Option<String>,
    },
}

/// 파일의 정체(unix: 장치 · inode). 다른 곳은 비교하지 않는다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct FileId(Option<(u64, u64)>);

impl FileId {
    fn of(meta: &std::fs::Metadata) -> Self {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            Self(Some((meta.dev(), meta.ino())))
        }
        #[cfg(not(unix))]
        {
            let _ = meta;
            Self(None)
        }
    }
}

#[derive(Clone)]
pub(crate) struct Entry {
    connection: Connection,
    pub(crate) project: String,
    pub(crate) run: RunId,
    pub(crate) at: SourceAt,
    used: u64,
}

#[derive(Default)]
struct Table {
    by_token: HashMap<String, Entry>,
    by_place: HashMap<(Connection, RunId, SourceAt), String>,
    /// 연결마다 쓴 차례 → 토큰(가장 오래 쓰지 않은 것이 처음).
    ages: HashMap<Connection, BTreeMap<u64, String>>,
    watched: HashSet<Connection>,
    clock: u64,
}

impl Table {
    fn kill(&mut self, token: &str) {
        let Some(e) = self.by_token.remove(token) else {
            return;
        };
        self.by_place.remove(&(e.connection, e.run.clone(), e.at));
        if let Some(ages) = self.ages.get_mut(&e.connection) {
            ages.remove(&e.used);
        }
    }

    fn touch(&mut self, token: &str) {
        let Some(e) = self.by_token.get_mut(token) else {
            return;
        };
        let ages = self.ages.entry(e.connection).or_default();
        ages.remove(&e.used);
        self.clock += 1;
        e.used = self.clock;
        ages.insert(self.clock, token.to_owned());
    }
}

/// 토큰 표. 복제는 같은 표를 본다.
#[derive(Clone, Default)]
pub(crate) struct SourceTokens {
    table: Arc<Mutex<Table>>,
}

/// 토큰 글 하나(128비트). 프로세스마다 OS 난수로 씨를 받은 해시로 만든다(`pw-lsp` 의 읽기 토큰과 같은 꼴) — 연결에 묶이므로
/// 맞혀도 다른 연결에서는 쓸 수 없다.
fn new_token() -> String {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let state = std::collections::hash_map::RandomState::new();
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let mut a = state.build_hasher();
    a.write_u64(n);
    let mut b = state.build_hasher();
    b.write_u64(!n);
    format!("{:016x}{:016x}", a.finish(), b.finish())
}

/// 어댑터가 보고한 경로가 루트 밖 일반 파일이면 그 실제 경로. 루트 안 · 없는 파일 · 상대 경로면 `None`(토큰을 달지 않는다 —
/// 루트 안이면 창이 `fs.*` 로 연다).
pub(crate) fn outside_file(root: &Path, reported: &str) -> Option<(PathBuf, FileId)> {
    let path = Path::new(reported);
    if !path.is_absolute() {
        return None;
    }
    let real = std::fs::canonicalize(path).ok()?;
    if real.starts_with(root) {
        return None;
    }
    let meta = std::fs::metadata(&real).ok()?;
    meta.is_file().then(|| (real, FileId::of(&meta)))
}

fn no_such_token() -> RpcError {
    RpcError::new(code::NOT_FOUND, "no such source token")
}

impl SourceTokens {
    /// 그 자리의 토큰(같은 연결 · 실행 · 자리면 같은 토큰). 처음 받은 연결이면 닫힘을 지켜본다.
    pub(crate) fn mint(&self, ctx: &Ctx, project: &str, run: &str, at: SourceAt) -> String {
        let connection = ctx.session();
        let mut t = lock(&self.table);
        let place = (connection, run.to_owned(), at.clone());
        if let Some(token) = t.by_place.get(&place).cloned() {
            t.touch(&token);
            return token;
        }
        let token = new_token();
        t.clock += 1;
        let used = t.clock;
        t.by_place.insert(place, token.clone());
        t.by_token.insert(
            token.clone(),
            Entry {
                connection,
                project: project.to_owned(),
                run: run.to_owned(),
                at,
                used,
            },
        );
        t.ages
            .entry(connection)
            .or_default()
            .insert(used, token.clone());
        while t.ages.get(&connection).map_or(0, BTreeMap::len) > TOKENS_PER_CONNECTION {
            let Some((_, old)) = t.ages.get_mut(&connection).and_then(BTreeMap::pop_first) else {
                break;
            };
            t.kill(&old);
        }
        if t.watched.insert(connection) {
            let closed = ctx.connection_closed();
            let table = Arc::clone(&self.table);
            tokio::spawn(async move {
                closed.cancelled().await;
                let mut t = lock(&table);
                let dead: Vec<String> = t
                    .by_token
                    .iter()
                    .filter(|(_, e)| e.connection == connection)
                    .map(|(k, _)| k.clone())
                    .collect();
                for token in dead {
                    t.kill(&token);
                }
                t.ages.remove(&connection);
                t.watched.remove(&connection);
            });
        }
        token
    }

    /// 이 연결 · 이 프로젝트의 토큰. 아니면(없음 · 다른 연결 · 다른 프로젝트) 같은 not found.
    pub(crate) fn entry(&self, ctx: &Ctx, project: &str, token: &str) -> Result<Entry, RpcError> {
        let mut t = lock(&self.table);
        let entry = t
            .by_token
            .get(token)
            .filter(|e| e.connection == ctx.session() && e.project == project)
            .cloned()
            .ok_or_else(no_such_token)?;
        t.touch(token);
        Ok(entry)
    }

    /// 실행이 끝났다 — 그 실행의 토큰을 지운다.
    pub(crate) fn forget_run(&self, run: &str) {
        let mut t = lock(&self.table);
        let dead: Vec<String> = t
            .by_token
            .iter()
            .filter(|(_, e)| e.run == run)
            .map(|(k, _)| k.clone())
            .collect();
        for token in dead {
            t.kill(&token);
        }
    }
}

/// 토큰의 파일을 읽는다(블로킹, 머리 주석): 실제 경로를 한 번 열고 그 핸들로 판정한 뒤 그 핸들에서 상한까지만 읽는다.
pub(crate) fn read_file(real: &Path, id: FileId) -> Result<String, RpcError> {
    use std::io::Read;
    let mut open = std::fs::OpenOptions::new();
    open.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        open.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    // 바뀐 자리(링크 · 지움)는 없는 토큰과 같다.
    let file = open.open(real).map_err(|_| no_such_token())?;
    let meta = file
        .metadata()
        .map_err(|e| RpcError::internal(e.to_string()))?;
    if !meta.is_file() {
        return Err(RpcError::invalid_params("only regular files can be read"));
    }
    if FileId::of(&meta) != id {
        return Err(no_such_token());
    }
    if meta.len() > SOURCE_LIMIT {
        return Err(too_large(meta.len()));
    }
    let mut bytes = Vec::new();
    file.take(SOURCE_LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| RpcError::internal(e.to_string()))?;
    if bytes.len() as u64 > SOURCE_LIMIT {
        return Err(too_large(bytes.len() as u64));
    }
    String::from_utf8(bytes).map_err(|_| RpcError::invalid_params("the source is not UTF-8 text"))
}

pub(crate) fn too_large(bytes: u64) -> RpcError {
    RpcError::invalid_params(format!(
        "the source takes {bytes} bytes, over the {SOURCE_LIMIT} byte limit"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn place(root: &Path, path: &Path) -> Option<(PathBuf, FileId)> {
        outside_file(root, path.to_str().unwrap())
    }

    #[test]
    fn only_files_outside_the_root_get_a_place() {
        let tmp = tempfile::tempdir().unwrap();
        let base = std::fs::canonicalize(tmp.path()).unwrap();
        let root = base.join("root");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/main.rs"), "fn main() {}").unwrap();
        std::fs::write(base.join("lib.rs"), "pub fn f() {}").unwrap();
        assert_eq!(place(&root, &root.join("src/main.rs")), None);
        assert_eq!(
            place(&root, &base.join("lib.rs")).map(|(p, _)| p),
            Some(base.join("lib.rs"))
        );
        assert_eq!(outside_file(&root, "lib.rs"), None, "relative");
        assert_eq!(place(&root, &base), None, "a folder");
        assert_eq!(place(&root, &base.join("missing.rs")), None);
    }

    /// 토큰을 만든 뒤 그 실제 경로의 파일을 바꿔 끼우면(다른 파일 · FIFO · `/dev/zero` 로 가는 링크) 읽지 않는다 — 멈추거나 끝없이
    /// 읽지 않고 곧바로 거절한다(연 핸들로 판정, `O_NOFOLLOW | O_NONBLOCK`).
    #[cfg(unix)]
    #[test]
    fn a_file_swapped_after_the_token_is_refused_without_blocking() {
        let tmp = tempfile::tempdir().unwrap();
        let base = std::fs::canonicalize(tmp.path()).unwrap();
        let root = base.join("root");
        std::fs::create_dir_all(&root).unwrap();
        let a = base.join("a.rs");
        std::fs::write(&a, "a").unwrap();
        let (real, id) = place(&root, &a).unwrap();
        assert_eq!(read_file(&real, id).unwrap(), "a");

        // 다른 파일을 그 자리로.
        std::fs::write(base.join("b.rs"), "b").unwrap();
        std::fs::rename(base.join("b.rs"), &a).unwrap();
        assert_eq!(read_file(&real, id).unwrap_err().code, code::NOT_FOUND);

        // FIFO — 막는 열기면 여기서 멈춘다.
        std::fs::remove_file(&a).unwrap();
        let c = std::ffi::CString::new(a.to_str().unwrap()).unwrap();
        // SAFETY: 경로는 NUL 로 끝나는 글이다.
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        let e = read_file(&real, id).unwrap_err();
        assert!(
            matches!(e.code, code::NOT_FOUND | code::INVALID_PARAMS),
            "{e:?}"
        );

        // `/dev/zero` 로 가는 링크 — 따라가면 끝없이 읽는다.
        std::fs::remove_file(&a).unwrap();
        std::os::unix::fs::symlink("/dev/zero", &a).unwrap();
        assert_eq!(read_file(&real, id).unwrap_err().code, code::NOT_FOUND);
    }

    #[test]
    fn a_large_or_binary_file_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let base = std::fs::canonicalize(tmp.path()).unwrap();
        let root = base.join("root");
        let big = base.join("big.txt");
        std::fs::write(&big, vec![b'a'; SOURCE_LIMIT as usize + 1]).unwrap();
        let (real, id) = place(&root, &big).unwrap();
        assert_eq!(read_file(&real, id).unwrap_err().code, code::INVALID_PARAMS);
        let bin = base.join("bin.dat");
        std::fs::write(&bin, [0xff, 0xfe, 0x00]).unwrap();
        let (real, id) = place(&root, &bin).unwrap();
        assert_eq!(read_file(&real, id).unwrap_err().code, code::INVALID_PARAMS);
    }
}
