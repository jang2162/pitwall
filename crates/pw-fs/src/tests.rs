//! 레지스트리에 등록한 임시 프로젝트로 `fs.*` 의 동작 전체를 본다.

use std::fs;
use std::path::{Path, PathBuf};

use pw_core::bus::{DEFAULT_CAPACITY, EventBus};
use pw_core::config::AppPaths;
use pw_core::registry::NewProject;
use pw_core::registry::path::simplify;

use super::*;
use crate::judge::{LineEnding, SizeClass};

struct Fixture {
    _tmp: tempfile::TempDir,
    /// 임시 폴더의 실제 경로(macOS 의 `/var` → `/private/var` 등을 푼 것).
    base: PathBuf,
    fs: Fs,
    projects: Projects,
}

impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let base = simplify(fs::canonicalize(tmp.path()).unwrap());
        let paths = AppPaths::under(&base.join("app"));
        let projects = Projects::new(paths, None, EventBus::new(DEFAULT_CAPACITY));
        let fs = Fs::new();
        fs.attach(projects.clone());
        Self {
            _tmp: tmp,
            base,
            fs,
            projects,
        }
    }

    fn project(&self, root: &Path) -> String {
        fs::create_dir_all(root).unwrap();
        self.projects
            .create(NewProject {
                root: root.to_path_buf(),
                ..NewProject::default()
            })
            .unwrap()
            .id
    }

    fn at(project: &str, path: &Path) -> FsPathParams {
        FsPathParams {
            project_id: project.to_owned(),
            worktree: None,
            path: path.display().to_string(),
        }
    }

    fn read(&self, project: &str, path: &Path) -> Result<FsRead, RpcError> {
        self.fs.read(&FsReadParams {
            project_id: project.to_owned(),
            path: path.display().to_string(),
            ..FsReadParams::default()
        })
    }

    fn write(
        &self,
        project: &str,
        path: &Path,
        text: &str,
        expected: Option<String>,
    ) -> Result<FsWritten, RpcError> {
        self.fs.write(&write_params(project, path, text, expected))
    }
}

fn write_params(project: &str, path: &Path, text: &str, expected: Option<String>) -> FsWriteParams {
    FsWriteParams {
        project_id: project.to_owned(),
        worktree: None,
        path: path.display().to_string(),
        content: FsContent::Text {
            text: text.to_owned(),
        },
        bom: false,
        expected_version: expected,
    }
}

fn text(read: &FsRead) -> &str {
    match &read.content {
        FsContent::Text { text } => text,
        other => panic!("expected text, got {other:?}"),
    }
}

/// 완료 기준 ①: 루트 밖 경로 · `..` 탈출 · 상대 경로가 모든 메서드에서 거절된다.
#[test]
fn paths_outside_the_project_roots_are_rejected() {
    let f = Fixture::new();
    let root = f.base.join("proj");
    let id = f.project(&root);
    fs::write(f.base.join("secret.txt"), "secret").unwrap();
    fs::write(root.join("ok.txt"), "ok").unwrap();

    for path in [
        f.base.join("secret.txt"),
        root.join("../secret.txt"),
        root.join("sub/../../secret.txt"),
        PathBuf::from("ok.txt"),
    ] {
        let p = Fixture::at(&id, &path);
        for err in [
            f.fs.list(&p).unwrap_err(),
            f.fs.stat(&p).unwrap_err(),
            f.read(&id, &path).unwrap_err(),
            f.write(&id, &path, "x", None).unwrap_err(),
        ] {
            assert_eq!(err.code, code::INVALID_PARAMS, "{path:?}: {err:?}");
        }
    }
    assert_eq!(
        fs::read_to_string(f.base.join("secret.txt")).unwrap(),
        "secret"
    );
    assert_eq!(text(&f.read(&id, &root.join("ok.txt")).unwrap()), "ok");

    let unknown = f
        .read("0000000000000000", &root.join("ok.txt"))
        .unwrap_err();
    assert_eq!(unknown.code, code::INVALID_PARAMS, "{unknown:?}");
}

/// 완료 기준 ①: 루트 밖을 가리키는 심링크로는 읽을 수 있어도(결정 M1-D8) 쓸 수 없다. 루트 안을 가리키는 링크로 쓰면 링크는
/// 링크로 남고 대상 파일이 바뀐다.
#[cfg(unix)]
#[test]
fn symlinks_are_read_through_and_written_only_inside() {
    let f = Fixture::new();
    let root = f.base.join("proj");
    let id = f.project(&root);
    fs::create_dir_all(f.base.join("store")).unwrap();
    fs::write(f.base.join("store/outside.txt"), "outside").unwrap();
    std::os::unix::fs::symlink(f.base.join("store"), root.join("linked")).unwrap();
    let via_link = root.join("linked/outside.txt");

    let read = f.read(&id, &via_link).unwrap();
    assert_eq!(text(&read), "outside");
    assert_eq!(
        read.path,
        f.base.join("store/outside.txt").display().to_string()
    );
    let err = f.write(&id, &via_link, "pwned", read.version).unwrap_err();
    assert_eq!(err.code, code::INVALID_PARAMS, "{err:?}");
    assert_eq!(
        fs::read_to_string(f.base.join("store/outside.txt")).unwrap(),
        "outside"
    );
    // 링크 대상의 부모로 오르는 `..` 는 읽기도 거절한다.
    fs::write(f.base.join("sibling.txt"), "sibling").unwrap();
    let err = f
        .read(&id, &root.join("linked/../sibling.txt"))
        .unwrap_err();
    assert_eq!(err.code, code::INVALID_PARAMS, "{err:?}");

    fs::write(root.join("real.txt"), "before").unwrap();
    std::os::unix::fs::symlink(root.join("real.txt"), root.join("alias.txt")).unwrap();
    let read = f.read(&id, &root.join("alias.txt")).unwrap();
    let written = f
        .write(&id, &root.join("alias.txt"), "after", read.version)
        .unwrap();
    assert_eq!(written.path, root.join("real.txt").display().to_string());
    assert!(
        fs::symlink_metadata(root.join("alias.txt"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(fs::read_to_string(root.join("real.txt")).unwrap(), "after");
}

/// 활성 워크트리 안의 프로젝트 루트도 접근 범위다. 그 워크트리를 넘기지 않으면 밖이다.
#[test]
fn the_active_worktree_is_inside_the_roots() {
    let f = Fixture::new();
    let main = f.base.join("repo");
    fs::create_dir_all(main.join(".git/worktrees/wt")).unwrap();
    let id = f.project(&main.join("app"));
    let wt = f.base.join("wt");
    fs::create_dir_all(wt.join("app")).unwrap();
    fs::write(
        wt.join(".git"),
        format!("gitdir: {}\n", main.join(".git/worktrees/wt").display()),
    )
    .unwrap();
    fs::write(main.join(".git/worktrees/wt/commondir"), "../..\n").unwrap();
    fs::write(wt.join("app/a.txt"), "in worktree").unwrap();
    fs::write(wt.join("outside-app.txt"), "x").unwrap();

    let params = |path: &Path, worktree: Option<&Path>| FsReadParams {
        project_id: id.clone(),
        worktree: worktree.map(|w| w.display().to_string()),
        path: path.display().to_string(),
        ..FsReadParams::default()
    };
    let read =
        f.fs.read(&params(&wt.join("app/a.txt"), Some(&wt)))
            .unwrap();
    assert_eq!(text(&read), "in worktree");
    // 워크트리 꼭대기라도 프로젝트 루트(`app`)에 대응하는 폴더 밖은 아니다.
    let err =
        f.fs.read(&params(&wt.join("outside-app.txt"), Some(&wt)))
            .unwrap_err();
    assert_eq!(err.code, code::INVALID_PARAMS);
    let err = f.fs.read(&params(&wt.join("app/a.txt"), None)).unwrap_err();
    assert_eq!(err.code, code::INVALID_PARAMS);
    // 다른 프로젝트의 워크트리는 받지 않는다.
    let other = f.project(&f.base.join("other"));
    let err =
        f.fs.read(&FsReadParams {
            project_id: other,
            ..params(&wt.join("app/a.txt"), Some(&wt))
        })
        .unwrap_err();
    assert_eq!(err.code, code::INVALID_PARAMS, "{err:?}");
}

/// 완료 기준 ②: 다른 `version` 으로 쓰면 `CONFLICT` 이고 파일 바이트가 그대로다.
#[test]
fn a_stale_version_is_a_conflict_and_changes_nothing() {
    let f = Fixture::new();
    let root = f.base.join("proj");
    let id = f.project(&root);
    let path = root.join("a.txt");

    // 없는 파일은 기대 version 없이 만든다.
    let created = f.write(&id, &path, "one", None).unwrap();
    let read = f.read(&id, &path).unwrap();
    assert_eq!(read.version.as_deref(), Some(created.version.as_str()));

    // 다른 프로세스(에이전트)가 바꿨다. 같은 크기로 바꿔도 내용 해시가 가른다.
    fs::write(&path, "two").unwrap();
    let err = f
        .write(&id, &path, "mine", read.version.clone())
        .unwrap_err();
    assert_eq!(err.code, code::CONFLICT, "{err:?}");
    let current = f.read(&id, &path).unwrap().version;
    assert_eq!(
        err.data.as_ref().unwrap()["current"],
        serde_json::json!(current)
    );
    assert_eq!(fs::read(&path).unwrap(), b"two");

    // 기대 version 없이 있는 파일을 쓰면 덮지 않는다.
    let err = f.write(&id, &path, "mine", None).unwrap_err();
    assert_eq!(err.code, code::CONFLICT);
    assert_eq!(fs::read(&path).unwrap(), b"two");

    // 지금 version 이면 쓰고, 그 결과의 version 이 다음 쓰기의 기대값이다.
    let w = f.write(&id, &path, "three", current).unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"three");
    f.write(&id, &path, "four", Some(w.version)).unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"four");

    // 지워진 파일에 옛 version 으로 쓰면 다시 만들지 않는다.
    let v = f.read(&id, &path).unwrap().version;
    fs::remove_file(&path).unwrap();
    let err = f.write(&id, &path, "again", v).unwrap_err();
    assert_eq!(err.code, code::CONFLICT);
    assert!(!path.exists());
}

/// 완료 기준 ③: 임시 파일 단계에서 실패를 주입하면 원래 파일의 내용 · 권한이 그대로다.
#[test]
fn an_injected_failure_keeps_the_original_content_and_permissions() {
    let f = Fixture::new();
    let root = f.base.join("proj");
    let id = f.project(&root);
    let path = root.join("keep.sh");
    fs::write(&path, "original").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o750)).unwrap();
    }
    let before = fs::metadata(&path).unwrap().permissions();
    let version = f.read(&id, &path).unwrap().version;
    for stage in [write::Stage::Created, write::Stage::Written] {
        let err =
            f.fs.write_with(&write_params(&id, &path, "new", version.clone()), |s| {
                if s == stage {
                    Err(io::Error::other("injected"))
                } else {
                    Ok(())
                }
            })
            .unwrap_err();
        assert_eq!(err.code, code::INTERNAL_ERROR, "{err:?}");
        assert_eq!(fs::read(&path).unwrap(), b"original", "{stage:?}");
        assert_eq!(fs::metadata(&path).unwrap().permissions(), before);
        assert!(write::leftovers(&root).is_empty(), "{stage:?}");
    }
    // 실패한 쓰기 뒤에도 같은 version 으로 쓸 수 있고, 권한이 지켜진다.
    f.write(&id, &path, "new", version).unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"new");
    assert_eq!(fs::metadata(&path).unwrap().permissions(), before);
}

#[test]
fn read_only_files_are_not_written() {
    let f = Fixture::new();
    let root = f.base.join("proj");
    let id = f.project(&root);
    let path = root.join("locked.txt");
    fs::write(&path, "locked").unwrap();
    let mut perms = fs::metadata(&path).unwrap().permissions();
    perms.set_readonly(true);
    fs::set_permissions(&path, perms.clone()).unwrap();
    assert!(f.fs.stat(&Fixture::at(&id, &path)).unwrap().readonly);
    let version = f.read(&id, &path).unwrap().version;
    let err = f.write(&id, &path, "changed", version).unwrap_err();
    assert_eq!(err.code, code::INVALID_PARAMS, "{err:?}");
    assert_eq!(fs::read(&path).unwrap(), b"locked");
    // 임시 폴더를 지울 수 있게 되돌린다(Windows 는 읽기 전용 파일을 지우지 못한다).
    #[allow(clippy::permissions_set_readonly_false)]
    perms.set_readonly(false);
    fs::set_permissions(&path, perms).unwrap();
}

/// 완료 기준 ④: 앞 8 KiB 에 NUL 이 있으면 바이너리, UTF-8 BOM · CRLF 가 판정에 나온다. BOM 을 지켜 다시 쓰면 바이트가 같다.
#[test]
fn reads_report_binary_bom_and_line_endings() {
    let f = Fixture::new();
    let root = f.base.join("proj");
    let id = f.project(&root);

    let bin = root.join("a.bin");
    let mut bytes = vec![b'x'; 9000];
    bytes[8000] = 0;
    fs::write(&bin, &bytes).unwrap();
    let read = f.read(&id, &bin).unwrap();
    assert!(read.judgment.binary);
    assert!(matches!(read.content, FsContent::Base64 { .. }));

    let late = root.join("late-nul.txt");
    bytes[8000] = b'x';
    bytes[8500] = 0;
    fs::write(&late, &bytes).unwrap();
    assert!(!f.read(&id, &late).unwrap().judgment.binary);

    let crlf = root.join("bom.txt");
    let original = b"\xEF\xBB\xBFline one\r\nline two\r\n";
    fs::write(&crlf, original).unwrap();
    let read = f.read(&id, &crlf).unwrap();
    assert_eq!(read.judgment.bom, Some(Bom::Utf8));
    assert_eq!(read.judgment.line_ending, LineEnding::Crlf);
    assert_eq!(read.judgment.encoding.as_deref(), Some("UTF-8"));
    assert_eq!(read.judgment.size_class, SizeClass::Editable);
    assert!(read.judgment.editable);
    assert_eq!(
        text(&read),
        "line one\r\nline two\r\n",
        "BOM 은 텍스트에서 빠진다"
    );

    // 받은 텍스트와 판정 그대로 다시 쓰면 바이트가 같다.
    f.fs.write(&FsWriteParams {
        bom: true,
        ..write_params(&id, &crlf, text(&read), read.version.clone())
    })
    .unwrap();
    assert_eq!(fs::read(&crlf).unwrap(), original);

    // 바이트로 받기(hex 뷰어).
    let read =
        f.fs.read(&FsReadParams {
            project_id: id.clone(),
            path: crlf.display().to_string(),
            bytes: true,
            ..FsReadParams::default()
        })
        .unwrap();
    match read.content {
        FsContent::Base64 { data } => assert_eq!(BASE64.decode(data).unwrap(), original),
        other => panic!("{other:?}"),
    }
}

#[test]
fn ranges_have_no_version_and_large_files_need_a_range() {
    let f = Fixture::new();
    let root = f.base.join("proj");
    let id = f.project(&root);
    let path = root.join("big.log");
    let line = "0123456789abcdef\n";
    fs::write(&path, line.repeat(1000)).unwrap();
    let range =
        f.fs.read(&FsReadParams {
            project_id: id.clone(),
            path: path.display().to_string(),
            offset: Some(17),
            length: Some(17),
            ..FsReadParams::default()
        })
        .unwrap();
    assert_eq!(text(&range), line);
    assert_eq!((range.offset, range.size, range.eof), (17, 17_000, false));
    assert_eq!(range.version, None);

    // 크기 등급은 내용을 읽지 않고 stat 으로 정해진다(희소 파일).
    let huge = root.join("huge.bin");
    let file = fs::File::create(&huge).unwrap();
    for (size, class) in [
        (judge::EDITABLE_MAX, SizeClass::Editable),
        (judge::EDITABLE_MAX + 1, SizeClass::ReadOnly),
        (READ_ONLY_MAX, SizeClass::ReadOnly),
        (READ_ONLY_MAX + 1, SizeClass::TooLarge),
    ] {
        file.set_len(size).unwrap();
        let stat = f.fs.stat(&Fixture::at(&id, &huge)).unwrap();
        assert_eq!((stat.size, stat.size_class), (size, Some(class)));
    }
    drop(file);
    // 50 MiB 를 넘는 파일의 전체 읽기는 거절하고, 앞부분 범위는 읽힌다.
    let err = f.read(&id, &huge).unwrap_err();
    assert_eq!(err.code, code::INVALID_PARAMS, "{err:?}");
    let head =
        f.fs.read(&FsReadParams {
            project_id: id.clone(),
            path: huge.display().to_string(),
            offset: Some(0),
            length: Some(1024 * 1024),
            ..FsReadParams::default()
        })
        .unwrap();
    assert_eq!(head.judgment.size_class, SizeClass::TooLarge);
    assert!(!head.judgment.editable);
}

#[test]
fn missing_targets_are_not_found_and_folders_are_listed() {
    let f = Fixture::new();
    let root = f.base.join("proj");
    let id = f.project(&root);
    let missing = root.join("nope.txt");
    assert_eq!(f.read(&id, &missing).unwrap_err().code, code::NOT_FOUND);
    assert_eq!(
        f.fs.stat(&Fixture::at(&id, &missing)).unwrap_err().code,
        code::NOT_FOUND
    );
    assert_eq!(
        f.write(&id, &root.join("no-dir/a.txt"), "x", None)
            .unwrap_err()
            .code,
        code::NOT_FOUND
    );

    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/lib.rs"), "").unwrap();
    let listed = f.fs.list(&Fixture::at(&id, &root)).unwrap();
    assert_eq!(listed.path, root.display().to_string());
    assert_eq!(
        listed
            .entries
            .iter()
            .map(|e| (e.name.as_str(), e.kind))
            .collect::<Vec<_>>(),
        [("src", EntryKind::Dir)]
    );
    let stat = f.fs.stat(&Fixture::at(&id, &root.join("src"))).unwrap();
    assert_eq!((stat.kind, stat.size_class), (EntryKind::Dir, None));
    assert_eq!(
        f.read(&id, &root.join("src")).unwrap_err().code,
        code::INVALID_PARAMS
    );
    assert_eq!(
        f.fs.list(&Fixture::at(&id, &root.join("src/lib.rs")))
            .unwrap_err()
            .code,
        code::INVALID_PARAMS
    );
}

/// 완료 기준 ⑤: 항목 1만 개 폴더의 `fs.list` 가 release 빌드에서 100 ms 안. 파일 1만 개를 만드므로 평소에는 돌지 않는다:
/// `cargo nextest run -p pw-fs --release --run-ignored only list_of_10000_entries`. debug 빌드에서는 재기만 하고 판정하지 않는다.
#[test]
#[ignore = "release 빌드 성능 측정(위 명령)"]
fn list_of_10000_entries_is_fast_in_release() {
    let f = Fixture::new();
    let root = f.base.join("proj");
    let id = f.project(&root);
    fs::create_dir_all(root.join(".git")).unwrap();
    fs::write(root.join(".gitignore"), "*.log\nbuild/\n").unwrap();
    let dir = root.join("many");
    fs::create_dir_all(&dir).unwrap();
    for i in 0..10_000 {
        let name = match i % 4 {
            0 => format!("file-{i:05}.log"),
            1 => format!("dir-{i:05}"),
            _ => format!("file-{i:05}.rs"),
        };
        if i % 4 == 1 {
            fs::create_dir(dir.join(name)).unwrap();
        } else {
            fs::write(dir.join(name), "x").unwrap();
        }
    }
    let params = Fixture::at(&id, &dir);
    // 첫 호출은 디스크 캐시를 채운다. 뒤의 11회의 중앙값으로 판정한다(다른 빌드가 도는 기기에서 한두 번 튀는 값을 거른다).
    assert_eq!(f.fs.list(&params).unwrap().entries.len(), 10_000);
    let mut times = Vec::new();
    for _ in 0..11 {
        let started = std::time::Instant::now();
        let listed = f.fs.list(&params).unwrap();
        times.push(started.elapsed());
        assert_eq!(listed.entries.iter().filter(|e| e.ignored).count(), 2_500);
    }
    times.sort();
    let median = times[times.len() / 2];
    eprintln!(
        "fs.list of 10000 entries: min {:?}, median {median:?}, max {:?}",
        times[0],
        times[times.len() - 1]
    );
    if !cfg!(debug_assertions) {
        assert!(median < std::time::Duration::from_millis(100), "{times:?}");
    }
}
