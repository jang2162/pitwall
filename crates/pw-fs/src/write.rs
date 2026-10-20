//! 원자 쓰기: 같은 폴더의 임시 파일에 다 쓴 뒤 rename 으로 바꿔 끼운다. 어느 단계에서 실패해도 원래 파일은 그대로다.
//!
//! - 대상은 실제 경로(심링크를 따라간 뒤, [`crate::scope::Roots::writable`])라 링크는 링크로 남고 대상 파일이 바뀐다.
//! - 원래 파일의 권한을 임시 파일에 옮긴다(0600 이 0644 가 되지 않게). 소유자 · 확장 속성 · Windows ACL 은 옮기지 않는다 —
//!   임시 파일은 폴더의 기본값을 받는다. rename 은 새 파일로 바꿔 끼우므로 하드 링크는 끊긴다.
//! - Windows 의 `fs::rename` 은 대상이 있어도 바꿔 끼운다(`MoveFileExW(MOVEFILE_REPLACE_EXISTING)`, 접근 거부면 POSIX 방식으로 다시).
//!   그 순간 다른 프로세스가 대상을 열고 있으면 잠깐 실패하므로 설정 쓰기와 같은 다시 시도([`pw_core::config::robust`])를 거친다.
//! - 임시 이름은 pid · 카운터 · 시각으로 고유하게 만든다. 같은 파일을 동시에 쓰는 두 쓰기가 서로의 임시 파일을 rename 하지 않는다.

use std::fs;
use std::io::{self, Write as _};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use pw_core::config::robust::retrying;

/// 실패를 주입하는 지점(시험).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Stage {
    /// 임시 파일을 만든 직후.
    Created,
    /// 임시 파일에 내용 · 권한을 다 쓴 뒤, rename 직전.
    Written,
}

/// `target` 을 `bytes` 로 바꾼다. `target` 이 있으면 그 권한을 지킨다. 폴더는 있어야 한다.
pub fn write_atomic(target: &Path, bytes: &[u8]) -> io::Result<()> {
    write_atomic_with(target, bytes, |_| Ok(()))
}

pub(crate) fn write_atomic_with(
    target: &Path,
    bytes: &[u8],
    fault: impl Fn(Stage) -> io::Result<()>,
) -> io::Result<()> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let permissions = match fs::metadata(target) {
        Ok(meta) => Some(meta.permissions()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => return Err(labeled("read the permissions of", target, e)),
    };
    let dir = target.parent().unwrap_or(Path::new("."));
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    loop {
        // 확장자를 원래 파일과 다르게 둔다 — 파일 감시 · 검색이 임시 파일을 원래 파일의 변경으로 보지 않게.
        let tmp = dir.join(format!(
            ".{name}.{}.{}.{nanos}.pitwall-tmp",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed),
        ));
        let mut file = match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
        {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(labeled("create the temporary file", &tmp, e)),
        };
        let written = fault(Stage::Created)
            .and_then(|()| file.write_all(bytes))
            .and_then(|()| file.sync_all())
            .map_err(|e| labeled("write the temporary file", &tmp, e))
            .and_then(|()| match &permissions {
                Some(p) => fs::set_permissions(&tmp, p.clone())
                    .map_err(|e| labeled("set the permissions of", &tmp, e)),
                None => Ok(()),
            })
            .and_then(|()| {
                drop(file);
                fault(Stage::Written)?;
                retrying(|| fs::rename(&tmp, target)).map_err(|e| {
                    io::Error::new(
                        e.kind(),
                        format!("replace {} with {}: {e}", target.display(), tmp.display()),
                    )
                })
            });
        if written.is_err() {
            let _ = fs::remove_file(&tmp);
        } else {
            sync_dir(dir);
        }
        return written;
    }
}

/// rename 을 디스크에 남긴다(전원이 나가도 새 이름이 남게). 실패해도 쓰기는 끝난 것이라 무시한다.
fn sync_dir(dir: &Path) {
    #[cfg(unix)]
    if let Ok(d) = fs::File::open(dir) {
        let _ = d.sync_all();
    }
    #[cfg(not(unix))]
    let _ = dir;
}

fn labeled(op: &str, path: &Path, e: io::Error) -> io::Error {
    io::Error::new(e.kind(), format!("{op} {}: {e}", path.display()))
}

/// `dir` 에 남은 이 모듈의 임시 파일(시험).
#[cfg(test)]
pub(crate) fn leftovers(dir: &Path) -> Vec<std::path::PathBuf> {
    fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.to_string_lossy().ends_with(".pitwall-tmp"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn injected() -> io::Error {
        io::Error::other("injected failure")
    }

    /// 대상이 이미 있을 때도 바꿔 끼운다 — Windows 의 rename 이 덮어쓰기를 하는지 3 OS CI 에서 본다.
    #[test]
    fn replaces_an_existing_file_repeatedly() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("a.txt");
        fs::write(&target, "old").unwrap();
        for content in ["one", "two", "three"] {
            write_atomic(&target, content.as_bytes()).unwrap();
            assert_eq!(fs::read_to_string(&target).unwrap(), content);
        }
        // 없는 파일은 만든다.
        let new = tmp.path().join("new.txt");
        write_atomic(&new, b"n").unwrap();
        assert_eq!(fs::read(&new).unwrap(), b"n");
        assert!(leftovers(tmp.path()).is_empty());
    }

    /// 임시 파일 단계 어디서 실패해도 원래 파일의 내용 · 권한이 그대로이고 임시 파일이 남지 않는다.
    #[test]
    fn a_failure_at_any_stage_leaves_the_original_untouched() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("keep.txt");
        fs::write(&target, "original bytes").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&target, fs::Permissions::from_mode(0o640)).unwrap();
        }
        let before = fs::metadata(&target).unwrap().permissions();
        for stage in [Stage::Created, Stage::Written] {
            let err = write_atomic_with(&target, b"new bytes", |s| {
                if s == stage { Err(injected()) } else { Ok(()) }
            })
            .unwrap_err();
            assert!(err.to_string().contains("injected"), "{err}");
            assert_eq!(fs::read(&target).unwrap(), b"original bytes", "{stage:?}");
            assert_eq!(fs::metadata(&target).unwrap().permissions(), before);
            assert!(leftovers(tmp.path()).is_empty(), "{stage:?}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn keeps_the_mode_of_the_original() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("script.sh");
        fs::write(&target, "#!/bin/sh\n").unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o751)).unwrap();
        write_atomic(&target, b"#!/bin/sh\necho hi\n").unwrap();
        assert_eq!(
            fs::metadata(&target).unwrap().permissions().mode() & 0o7777,
            0o751
        );
    }
}
