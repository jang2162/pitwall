//! 연결을 여는 것과 그 연결에서 늘 막는 것(14-m3-plan.md M3-08, 결정 M3-D6 · M3-D13).
//!
//! - 여는 플래그: read-only 연결은 `SQLITE_OPEN_READ_ONLY` + `query_only`, 아니면 읽기 쓰기. 어느 쪽이든 `SQLITE_OPEN_CREATE` 를
//!   주지 않아 없는 경로는 오류다.
//! - 늘 끄는 것: 확장 로드(`SQLITE_DBCONFIG_ENABLE_LOAD_EXTENSION`, rusqlite 의 `load_extension` 기능도 켜지 않는다) ·
//!   `trusted_schema`. `SQLITE_DBCONFIG_DEFENSIVE` 는 켠다(`writable_schema` 로 스키마를 고치지 못한다).
//! - authorizer: `ATTACH` 대상을 [`ConnectionSpec::file_allowed`] 에 건다. `VACUUM INTO` 도 SQLite 안에서 `ATTACH '<대상>'` 을
//!   돌리므로 같은 판정을 지난다. `load_extension()` 호출, 프로세스 전역 · 늘 끄는 설정을 바꾸는 `PRAGMA`([`LOCKED_PRAGMAS`]),
//!   read-only 연결 · 읽기 전용 트랜잭션 동안 `PRAGMA query_only` 를 바꾸는 것도 거절한다.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use pw_db_types::{ConnectionSpec, DbError, Target};
use rusqlite::config::DbConfig;
use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
use rusqlite::{Connection, OpenFlags, ffi};

use crate::error::{invalid, map};

/// 다른 연결 · 프로세스의 잠금을 기다리는 시간(벽시계 — [`crate::busy`]). 넘으면 `busy` 다.
pub(crate) const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// 값을 주면 거절하는 `PRAGMA`. 앞의 둘은 프로세스 전역이다 — 메인 Host 안의 모든 SQLite 연결(로컬 저장소 포함)의 임시 · 데이터
/// 파일 자리를 바꾼다(결정 M3-D4 로 같은 프로세스다). `trusted_schema` 는 늘 끄는 설정이다(결정 M3-D13).
const LOCKED_PRAGMAS: [&str; 3] = [
    "temp_store_directory",
    "data_store_directory",
    "trusted_schema",
];

/// 연결과 그 authorizer 가 함께 보는 것.
#[derive(Clone, Default)]
pub(crate) struct Guard {
    /// authorizer 가 거절한 까닭. 오류를 만들 때 꺼낸다.
    denial: Arc<Mutex<Option<String>>>,
    /// `PRAGMA query_only` 를 바꾸지 못하게 잠갔다(read-only 연결 · 읽기 전용 트랜잭션).
    query_only_locked: Arc<AtomicBool>,
}

impl Guard {
    pub(crate) fn clear_denial(&self) {
        self.denial_slot().take();
    }

    /// rusqlite 오류를 [`DbError`] 로. authorizer 가 거절했으면 그 까닭을 싣는다.
    pub(crate) fn error(&self, error: rusqlite::Error) -> DbError {
        map(error, self.denial_slot().take())
    }

    /// `query_only` 를 켜거나 끈다. 켜면 사람이 `PRAGMA` 로 풀지 못하게 잠그고, 끄면 잠금도 푼다.
    pub(crate) fn set_query_only(&self, conn: &Connection, on: bool) -> rusqlite::Result<()> {
        self.query_only_locked.store(false, Ordering::SeqCst);
        let result = conn.pragma_update(None, "query_only", on);
        self.query_only_locked.store(on, Ordering::SeqCst);
        result
    }

    fn deny(&self, why: String) -> Authorization {
        *self.denial_slot() = Some(why);
        Authorization::Deny
    }

    fn denial_slot(&self) -> std::sync::MutexGuard<'_, Option<String>> {
        self.denial
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// 연결을 열고 늘 막는 것을 건다. 깨진 파일은 여기서 오류다(SQLite 는 첫 읽기 때 헤더를 본다).
pub(crate) fn open(spec: &ConnectionSpec) -> Result<(Connection, Guard), DbError> {
    let Target::File { path } = &spec.target else {
        return Err(invalid(
            "the SQLite driver opens a file, not a network target",
        ));
    };
    if !path.is_absolute() {
        return Err(invalid("the SQLite file path must be absolute"));
    }
    // 번들 SQLite 는 `SQLITE_USE_URI` 로 빌드돼 `file:` 로 시작하는 이름을 URI(`vfs=` · `mode=` 등)로 읽는다.
    if is_uri(path.as_os_str().to_string_lossy().as_ref()) {
        return Err(invalid("SQLite URI file names are not opened"));
    }
    let access = if spec.read_only {
        OpenFlags::SQLITE_OPEN_READ_ONLY
    } else {
        OpenFlags::SQLITE_OPEN_READ_WRITE
    };
    let conn = Connection::open_with_flags(path, access | OpenFlags::SQLITE_OPEN_NO_MUTEX)
        .map_err(|e| map(e, None))?;
    let guard = Guard::default();
    harden(&conn, spec, &guard).map_err(|e| guard.error(e))?;
    if spec.read_only {
        guard
            .set_query_only(&conn, true)
            .map_err(|e| guard.error(e))?;
    }
    conn.query_row("SELECT count(*) FROM sqlite_schema", [], |_| Ok(()))
        .map_err(|e| guard.error(e))?;
    Ok((conn, guard))
}

fn harden(conn: &Connection, spec: &ConnectionSpec, guard: &Guard) -> rusqlite::Result<()> {
    disable_load_extension(conn)?;
    conn.set_db_config(DbConfig::SQLITE_DBCONFIG_DEFENSIVE, true)?;
    conn.set_db_config(DbConfig::SQLITE_DBCONFIG_TRUSTED_SCHEMA, false)?;
    conn.busy_handler(Some(crate::busy::wait))?;
    let roots = spec.clone();
    let guard = guard.clone();
    conn.authorizer(Some(move |context: AuthContext<'_>| {
        authorize(&roots, &guard, &context)
    }))
}

/// rusqlite 는 이 설정을 `DbConfig` 로 내지 않는다(그 기능 `load_extension` 은 켜지 않는다) — C API 로 끈다.
fn disable_load_extension(conn: &Connection) -> rusqlite::Result<()> {
    let mut now: std::ffi::c_int = 1;
    // SAFETY: `handle` 은 이 함수가 도는 동안 열려 있는 연결의 포인터다. `SQLITE_DBCONFIG_ENABLE_LOAD_EXTENSION` 은 int 하나와
    // 바뀐 값을 받을 int 포인터를 받는다.
    let rc = unsafe {
        ffi::sqlite3_db_config(
            conn.handle(),
            ffi::SQLITE_DBCONFIG_ENABLE_LOAD_EXTENSION,
            0 as std::ffi::c_int,
            &raw mut now,
        )
    };
    if rc != ffi::SQLITE_OK || now != 0 {
        return Err(rusqlite::Error::SqliteFailure(
            ffi::Error::new(rc),
            Some("cannot turn off SQLite extension loading".to_owned()),
        ));
    }
    Ok(())
}

fn authorize(spec: &ConnectionSpec, guard: &Guard, context: &AuthContext<'_>) -> Authorization {
    match context.action {
        AuthAction::Attach { filename } => match attach_allowed(spec, filename) {
            Ok(()) => Authorization::Allow,
            Err(why) => guard.deny(why),
        },
        // 이름이 글자 그대로가 아닌 `ATTACH`(식 · 바인드 값)는 SQLite 가 이름을 넘기지 않는다.
        AuthAction::Unknown {
            code: ffi::SQLITE_ATTACH,
            ..
        } => guard.deny("only a literal file name can be attached".to_owned()),
        AuthAction::Function { function_name }
            if function_name.eq_ignore_ascii_case("load_extension") =>
        {
            guard.deny("loading SQLite extensions is turned off".to_owned())
        }
        AuthAction::Pragma {
            pragma_name,
            pragma_value: Some(_),
        } if LOCKED_PRAGMAS
            .iter()
            .any(|locked| pragma_name.eq_ignore_ascii_case(locked)) =>
        {
            guard.deny(format!("PRAGMA {pragma_name} cannot be changed"))
        }
        AuthAction::Pragma {
            pragma_name,
            pragma_value: Some(_),
        } if pragma_name.eq_ignore_ascii_case("query_only")
            && guard.query_only_locked.load(Ordering::SeqCst) =>
        {
            guard.deny(
                "query_only cannot be changed on a read-only connection or in a read-only transaction"
                    .to_owned(),
            )
        }
        _ => Authorization::Allow,
    }
}

/// `ATTACH` 의 파일 이름을 연결의 루트 판정에 건다. 빈 이름 · `:memory:` 는 파일이 아니라 지나간다(`VACUUM` 은 빈 이름의 임시
/// DB 를 붙인다).
///
/// 상대 경로는 거절한다 — SQLite 는 그것을 프로젝트가 아니라 앱 프로세스의 현재 폴더에서 연다(GUI 앱이면 대개 `/`).
fn attach_allowed(spec: &ConnectionSpec, filename: &str) -> Result<(), String> {
    if filename.is_empty() || filename == ":memory:" {
        return Ok(());
    }
    if is_uri(filename) {
        return Err(format!(
            "attaching a URI file name is not allowed: {filename}"
        ));
    }
    let path = Path::new(filename);
    if !path.is_absolute() {
        return Err(format!(
            "attach a file by its absolute path — SQLite opens a relative path from the app's working folder, not the project: {filename}"
        ));
    }
    let (real, exists) = real_path(path)?;
    if !spec.file_allowed(&real) {
        return Err(format!(
            "attaching a file outside the project roots is not allowed: {}",
            real.display()
        ));
    }
    // `VACUUM INTO` 는 연결의 `READONLY` 를 빼고 `CREATE` 로 붙인다 — read-only 연결은 판정을 지나도 파일을 만들지 않는다.
    if spec.read_only && !exists {
        return Err(format!(
            "a read-only connection does not create files: {}",
            real.display()
        ));
    }
    Ok(())
}

/// 링크를 끝까지 푼 경로와 그 파일이 있는지. 아직 없는 파일(`VACUUM INTO` 의 새 파일)은 폴더의 실제 경로에 이름을 붙인다.
///
/// 마지막 구성 요소가 링크인데 대상이 없으면(끊긴 링크) 거절한다 — `canonicalize` 는 그것을 「없음」 으로 답하지만 SQLite 는
/// 링크를 따라가 대상 자리(루트 밖일 수 있다)에 파일을 만든다. Windows 의 심링크 · junction 도 같다(`symlink_metadata` 가 있다).
fn real_path(path: &Path) -> Result<(PathBuf, bool), String> {
    match path.canonicalize() {
        Ok(real) => Ok((real, true)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if path.symlink_metadata().is_ok() {
                return Err(format!(
                    "attaching through a link whose target does not exist is not allowed: {}",
                    path.display()
                ));
            }
            let unresolved = || format!("cannot resolve the attached file: {}", path.display());
            let name = path.file_name().ok_or_else(unresolved)?;
            let parent = path
                .parent()
                .and_then(|p| p.canonicalize().ok())
                .ok_or_else(unresolved)?;
            Ok((parent.join(name), false))
        }
        Err(e) => Err(format!(
            "cannot resolve the attached file {}: {e}",
            path.display()
        )),
    }
}

fn is_uri(name: &str) -> bool {
    name.get(..5)
        .is_some_and(|scheme| scheme.eq_ignore_ascii_case("file:"))
}
