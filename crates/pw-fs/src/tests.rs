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

/// 완료 기준 ① · 결정 M1-D8: 실제 대상이 루트 밖인 심링크는 따르지 않는다 — 읽기 · 상태 · 목록 · 쓰기 모두 거절하고, 부모
/// 폴더의 목록에는 `outside` 로 표시만 한다. 리뷰의 재현: 부모를 가리키는 `up -> ..`, 절대 경로 `sys -> /etc`, 루트 밖 폴더.
/// pnpm 식으로 루트 안을 가리키는 링크는 그대로 읽히고, 그 링크로 쓰면 링크는 링크로 남고 대상 파일이 바뀐다.
#[cfg(unix)]
#[test]
fn symlinks_are_followed_only_into_the_roots() {
    let f = Fixture::new();
    let root = f.base.join("proj");
    let id = f.project(&root);
    fs::write(f.base.join("secret.txt"), "secret").unwrap();
    fs::create_dir_all(f.base.join("store")).unwrap();
    fs::write(f.base.join("store/outside.txt"), "outside").unwrap();
    std::os::unix::fs::symlink(f.base.join("store"), root.join("linked")).unwrap();
    std::os::unix::fs::symlink("..", root.join("up")).unwrap();
    std::os::unix::fs::symlink("/etc", root.join("sys")).unwrap();
    std::os::unix::fs::symlink(f.base.join("secret.txt"), root.join("secret-link")).unwrap();

    for path in [
        root.join("linked/outside.txt"),
        root.join("up/secret.txt"),
        root.join("sys/hosts"),
        root.join("secret-link"),
        root.join("up/proj/../secret.txt"),
    ] {
        let p = Fixture::at(&id, &path);
        for err in [
            f.read(&id, &path).unwrap_err(),
            f.fs.stat(&p).unwrap_err(),
            f.write(&id, &path, "pwned", None).unwrap_err(),
        ] {
            assert_eq!(err.code, code::INVALID_PARAMS, "{path:?}: {err:?}");
        }
    }
    for dir in ["linked", "up", "sys"] {
        let err = f.fs.list(&Fixture::at(&id, &root.join(dir))).unwrap_err();
        assert_eq!(err.code, code::INVALID_PARAMS, "{dir}: {err:?}");
        assert!(
            err.data
                .as_ref()
                .unwrap()
                .as_str()
                .unwrap()
                .contains("symlink"),
            "{dir}: the reason names the symlink: {err:?}"
        );
    }
    assert_eq!(
        fs::read_to_string(f.base.join("secret.txt")).unwrap(),
        "secret"
    );

    // pnpm 기본 구성: `node_modules/pkg -> node_modules/.pnpm/…`(루트 안).
    let pnpm = root.join("node_modules/.pnpm/pkg@1.0.0/node_modules/pkg");
    fs::create_dir_all(&pnpm).unwrap();
    fs::write(pnpm.join("index.js"), "module.exports = 1").unwrap();
    std::os::unix::fs::symlink(&pnpm, root.join("node_modules/pkg")).unwrap();
    assert_eq!(
        text(
            &f.read(&id, &root.join("node_modules/pkg/index.js"))
                .unwrap()
        ),
        "module.exports = 1"
    );
    assert_eq!(
        f.fs.list(&Fixture::at(&id, &root.join("node_modules/pkg")))
            .unwrap()
            .entries[0]
            .name,
        "index.js"
    );

    // 루트 목록: 밖을 가리키는 링크는 표시만 하고 대상을 알리지 않는다.
    let listed = f.fs.list(&Fixture::at(&id, &root)).unwrap();
    let get = |n: &str| listed.entries.iter().find(|e| e.name == n).unwrap();
    for name in ["linked", "up", "sys", "secret-link"] {
        let e = get(name);
        assert_eq!(
            (e.kind, e.outside, e.target, e.size),
            (EntryKind::Symlink, true, None, 0),
            "{name}"
        );
    }
    assert!(!get("node_modules").outside);
    let inner =
        f.fs.list(&Fixture::at(&id, &root.join("node_modules")))
            .unwrap();
    let pkg = inner.entries.iter().find(|e| e.name == "pkg").unwrap();
    assert_eq!((pkg.outside, pkg.target), (false, Some(EntryKind::Dir)));

    // 루트 안을 가리키는 링크로 쓰면 링크는 남고 대상이 바뀐다.
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

/// 등록한 루트 폴더를 지우고 같은 이름의 루트 밖 링크를 두면(리뷰 L7) 그 루트는 더 쓰지 않는다 — 링크 너머가 루트가 되지 않는다.
#[cfg(unix)]
#[test]
fn a_root_swapped_to_an_outside_link_is_not_a_root() {
    let f = Fixture::new();
    let root = f.base.join("proj");
    let id = f.project(&root);
    fs::write(root.join("a.txt"), "inside").unwrap();
    assert!(f.fs.list(&Fixture::at(&id, &root)).is_ok());

    let outside = f.base.join("outside");
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("root_swap_secret.rs"), "secret").unwrap();
    fs::remove_dir_all(&root).unwrap();
    std::os::unix::fs::symlink(&outside, &root).unwrap();

    let secret = root.join("root_swap_secret.rs");
    let p = Fixture::at(&id, &secret);
    for err in [
        f.fs.list(&Fixture::at(&id, &root)).unwrap_err(),
        f.read(&id, &secret).unwrap_err(),
        f.fs.stat(&p).unwrap_err(),
        f.write(&id, &secret, "pwned", None).unwrap_err(),
    ] {
        assert_eq!(err.code, code::INVALID_PARAMS, "{err:?}");
    }
    assert_eq!(
        fs::read_to_string(outside.join("root_swap_secret.rs")).unwrap(),
        "secret"
    );
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
    fs::write(
        main.join(".git/worktrees/wt/gitdir"),
        format!("{}\n", wt.join(".git").display()),
    )
    .unwrap();
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
            f.fs.write_with(
                None,
                &write_params(&id, &path, "new", version.clone()),
                |s| {
                    if s == stage {
                        Err(io::Error::other("injected"))
                    } else {
                        Ok(())
                    }
                },
            )
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
        (crate::limits::EDITABLE_MAX, SizeClass::Editable),
        (crate::limits::EDITABLE_MAX + 1, SizeClass::ReadOnly),
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

/// 같은 기대 version 으로 동시에 들어온 두 쓰기 중 하나만 성공하고, 다른 하나는 `CONFLICT` 다. 첫 쓰기를 임시 파일 단계
/// (`Stage::Created`)에 세워 두고 그동안 두 번째 쓰기를 보낸다 — 잠금이 없으면 두 번째가 먼저 끝나 둘 다 성공한다.
#[test]
fn concurrent_writes_with_the_same_version_do_not_both_succeed() {
    use std::sync::mpsc;
    use std::time::Duration;

    let f = Fixture::new();
    let root = f.base.join("proj");
    let id = f.project(&root);
    // 있는 파일을 같은 version 으로, 없는 파일을 기대 version 없이(새로 만들기) 동시에 쓴다.
    for (name, initial) in [("existing.txt", Some("base")), ("new.txt", None)] {
        let path = root.join(name);
        if let Some(initial) = initial {
            fs::write(&path, initial).unwrap();
        }
        let version = initial.and_then(|_| f.read(&id, &path).unwrap().version);
        let (entered_tx, entered_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel::<()>();
        let (a, b) = std::thread::scope(|s| {
            let a = s.spawn(|| {
                f.fs.write_with(
                    None,
                    &write_params(&id, &path, "from-A", version.clone()),
                    move |stage| {
                        if stage == write::Stage::Created {
                            entered_tx.send(()).unwrap();
                            // 두 번째 쓰기가 끝나기를 잠깐 기다린다. 잠금이 있으면 그 쓰기는 이 쓰기를 기다리므로 시간이 다 된다.
                            let _ = done_rx.recv_timeout(Duration::from_millis(500));
                        }
                        Ok(())
                    },
                )
            });
            entered_rx.recv().unwrap();
            let b = s.spawn(|| {
                let r = f.write(&id, &path, "from-B", version.clone());
                let _ = done_tx.send(());
                r
            });
            (a.join().unwrap(), b.join().unwrap())
        });
        let (won, lost) = match (&a, &b) {
            (Ok(_), Err(e)) => ("from-A", e),
            (Err(e), Ok(_)) => ("from-B", e),
            _ => panic!("{name}: exactly one write must win: A={a:?} B={b:?}"),
        };
        assert_eq!(lost.code, code::CONFLICT, "{name}: {lost:?}");
        assert_eq!(fs::read_to_string(&path).unwrap(), won, "{name}");
    }
}

/// 대상이 없는 심링크에 쓰면 링크가 일반 파일로 바뀌므로 거절한다(루트 안 · 밖 대상 모두). 링크는 그대로다.
#[cfg(unix)]
#[test]
fn dangling_symlinks_are_not_written() {
    let f = Fixture::new();
    let root = f.base.join("proj");
    let id = f.project(&root);
    fs::create_dir_all(f.base.join("outside")).unwrap();
    for (link, target) in [
        ("dang-out", f.base.join("outside/new.txt")),
        ("dang-in", root.join("missing.txt")),
    ] {
        let path = root.join(link);
        std::os::unix::fs::symlink(&target, &path).unwrap();
        let err = f.write(&id, &path, "x", None).unwrap_err();
        assert_eq!(err.code, code::INVALID_PARAMS, "{link}: {err:?}");
        assert!(
            fs::symlink_metadata(&path)
                .unwrap()
                .file_type()
                .is_symlink(),
            "{link}"
        );
        assert!(!target.exists(), "{link}");
        // 상태의 크기는 `fs.list` 와 같이 0 이다(링크 자신의 길이가 아니라).
        if link == "dang-in" {
            let stat = f.fs.stat(&Fixture::at(&id, &path)).unwrap();
            assert_eq!(
                (stat.kind, stat.target, stat.size),
                (EntryKind::Symlink, None, 0)
            );
            let listed = f.fs.list(&Fixture::at(&id, &root)).unwrap();
            let entry = listed.entries.iter().find(|e| e.name == link).unwrap();
            assert_eq!(entry.size, stat.size);
        }
    }
}

/// 심링크 고리는 내부 오류가 아니라 invalid params 다.
#[cfg(unix)]
#[test]
fn symlink_loops_are_invalid_params() {
    let f = Fixture::new();
    let root = f.base.join("proj");
    let id = f.project(&root);
    std::os::unix::fs::symlink(root.join("self"), root.join("self")).unwrap();
    std::os::unix::fs::symlink(root.join("b"), root.join("a")).unwrap();
    std::os::unix::fs::symlink(root.join("a"), root.join("b")).unwrap();
    for name in ["self", "a"] {
        let path = root.join(name);
        for err in [
            f.read(&id, &path).unwrap_err(),
            f.write(&id, &path, "x", None).unwrap_err(),
            f.fs.list(&Fixture::at(&id, &path)).unwrap_err(),
            f.read(&id, &path.join("inner.txt")).unwrap_err(),
        ] {
            assert_eq!(err.code, code::INVALID_PARAMS, "{name}: {err:?}");
        }
    }
}

/// 읽기 전용 판정은 권한 비트가 아니라 실제로 쓸 수 있는가다: 소유자 쓰기 비트만 없는 0466 파일은 쓰기 비트가 남아 있어도
/// 소유자가 쓸 수 없다.
#[cfg(unix)]
#[test]
fn read_only_means_this_process_cannot_write() {
    use std::os::unix::fs::PermissionsExt;
    // root 는 권한 비트와 무관하게 쓸 수 있어 이 시험이 볼 것이 없다.
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    let f = Fixture::new();
    let root = f.base.join("proj");
    let id = f.project(&root);
    let path = root.join("owner-locked.txt");
    fs::write(&path, "locked").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o466)).unwrap();
    assert!(!fs::metadata(&path).unwrap().permissions().readonly());
    assert!(f.fs.stat(&Fixture::at(&id, &path)).unwrap().readonly);
    let version = f.read(&id, &path).unwrap().version;
    let err = f.write(&id, &path, "changed", version).unwrap_err();
    assert_eq!(err.code, code::INVALID_PARAMS, "{err:?}");
    assert_eq!(fs::read(&path).unwrap(), b"locked");
}

/// 판정과 open 사이에 파일이 루트 밖 링크로 바뀌면 `fs.read` 는 그 내용을 돌려주지 않는다(연 핸들의 경로를 다시 판정).
#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn a_file_swapped_to_an_outside_link_while_reading_is_refused() {
    let f = Fixture::new();
    let root = f.base.join("proj");
    let id = f.project(&root);
    let secret = f.base.join("secret.txt");
    fs::write(&secret, "secret").unwrap();
    let path = root.join("a.txt");
    fs::write(&path, "inside").unwrap();
    let swap_to = secret.clone();
    *f.fs.before_open.lock().unwrap() = Some(Box::new(move |p: &Path| {
        if fs::symlink_metadata(p).is_ok_and(|m| !m.file_type().is_symlink()) {
            fs::remove_file(p).unwrap();
            std::os::unix::fs::symlink(&swap_to, p).unwrap();
        }
    }));
    let err = f.read(&id, &path).unwrap_err();
    assert_eq!(err.code, code::INVALID_PARAMS, "{err:?}");
    assert!(
        !format!("{err:?}").contains("\"secret\""),
        "the outside content must not leak: {err:?}"
    );
}

#[cfg(unix)]
fn mkfifo(path: &Path) {
    let c = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
    // SAFETY: `c` 는 NUL 로 끝나는 살아 있는 경로 문자열이다.
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o644) }, 0);
}

/// FIFO 는 열지도 않는다 — 열면 쓰는 쪽이 올 때까지 막혀 그 요청과 스레드를 영영 붙잡는다. 전체 읽기 · 범위 읽기(Markdown 의
/// 이미지도 범위 읽기다) · 쓰기 전에 지금 내용을 읽는 것 모두. 고장 나면 막히므로 시한을 두고 다른 스레드에서 돌린다.
#[cfg(unix)]
#[test]
fn fifos_are_refused_without_opening() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use crate::search::tests::{MIB, bounded};

    let f = Fixture::new();
    let root = f.base.join("proj");
    let id = f.project(&root);
    let fifo = root.join("pipe");
    mkfifo(&fifo);
    let opened = Arc::new(AtomicUsize::new(0));
    let count = Arc::clone(&opened);
    *f.fs.before_open.lock().unwrap() = Some(Box::new(move |_: &Path| {
        count.fetch_add(1, Ordering::Relaxed);
    }));
    let path = fifo.clone();
    let errors = bounded(64 * MIB, std::time::Duration::from_secs(10), move || {
        let range = FsReadParams {
            project_id: id.clone(),
            path: path.display().to_string(),
            offset: Some(0),
            length: Some(4096),
            bytes: true,
            ..FsReadParams::default()
        };
        let errors = [
            f.read(&id, &path).unwrap_err(),
            f.fs.read(&range).unwrap_err(),
            f.write(&id, &path, "x", None).unwrap_err(),
        ];
        (errors, f)
    });
    for err in errors.0 {
        assert_eq!(err.code, code::INVALID_PARAMS, "{err:?}");
        assert!(
            format!("{:?}", err.data).contains("is not a regular file"),
            "{err:?}"
        );
    }
    assert_eq!(opened.load(Ordering::Relaxed), 0);
    use std::os::unix::fs::FileTypeExt as _;
    assert!(fs::symlink_metadata(&fifo).unwrap().file_type().is_fifo());
}

/// 판정 뒤 open 전에 파일이 FIFO 로 바뀌어도 막히지 않고 거절한다(막히지 않게 열고 연 핸들의 종류를 본다).
#[cfg(unix)]
#[test]
fn a_file_swapped_to_a_fifo_before_opening_is_refused() {
    use crate::search::tests::{MIB, bounded};

    let f = Fixture::new();
    let root = f.base.join("proj");
    let id = f.project(&root);
    let path = root.join("a.txt");
    fs::write(&path, "inside").unwrap();
    *f.fs.before_open.lock().unwrap() = Some(Box::new(|p: &Path| {
        if fs::symlink_metadata(p).is_ok_and(|m| m.is_file()) {
            fs::remove_file(p).unwrap();
            mkfifo(p);
        }
    }));
    let path2 = path.clone();
    let err = bounded(64 * MIB, std::time::Duration::from_secs(10), move || {
        f.read(&id, &path2).unwrap_err()
    });
    assert_eq!(err.code, code::INVALID_PARAMS, "{err:?}");
    assert!(
        format!("{:?}", err.data).contains("is not a regular file"),
        "{err:?}"
    );
}

/// M1-23(결정 M1-D12): UTF-8 이 아닌 텍스트는 짐작한 인코딩으로 풀려 오고 편집할 수 없다.
#[test]
fn euc_kr_files_read_as_hangul_and_are_not_editable() {
    let f = Fixture::new();
    let root = f.base.join("proj");
    let id = f.project(&root);
    let hangul =
        "// 한글 주석입니다. 이 파일은 EUC-KR 로 저장되어 있습니다.\nlet 이름 = \"가나다\";\n";
    let (bytes, _, lossy) = encoding_rs::EUC_KR.encode(hangul);
    assert!(!lossy);
    let path = root.join("euc-kr.txt");
    fs::write(&path, &bytes).unwrap();

    let read = f.read(&id, &path).unwrap();
    assert_eq!(read.judgment.encoding.as_deref(), Some("EUC-KR"));
    assert!(!read.judgment.binary && !read.judgment.editable);
    assert_eq!(text(&read), hangul);
    // 바이트로 달라고 하면 원래 바이트다.
    let raw =
        f.fs.read(&FsReadParams {
            project_id: id.clone(),
            path: path.display().to_string(),
            bytes: true,
            ..FsReadParams::default()
        })
        .unwrap();
    assert_eq!(
        raw.content,
        FsContent::Base64 {
            data: BASE64.encode(&bytes)
        }
    );
}

/// M1-23(결정 M1-D9): 기준값 경계 ±1 바이트 파일의 판정. 크기 등급은 stat 으로, 편집 · 강조 여부는 전체 읽기로 본다.
#[test]
fn size_thresholds_hold_one_byte_either_side() {
    use crate::limits::{EDITABLE_MAX, LONG_LINE_CHARS};

    let f = Fixture::new();
    let root = f.base.join("proj");
    let id = f.project(&root);
    // 100자마다 줄바꿈인 텍스트.
    let text_of = |size: u64| -> Vec<u8> {
        (0..size)
            .map(|i| if i % 101 == 100 { b'\n' } else { b'a' })
            .collect()
    };
    for (size, class, editable) in [
        (EDITABLE_MAX - 1, SizeClass::Editable, true),
        (EDITABLE_MAX, SizeClass::Editable, true),
        (EDITABLE_MAX + 1, SizeClass::ReadOnly, false),
        (READ_ONLY_MAX - 1, SizeClass::ReadOnly, false),
        (READ_ONLY_MAX, SizeClass::ReadOnly, false),
    ] {
        let path = root.join(format!("{size}.txt"));
        fs::write(&path, text_of(size)).unwrap();
        let stat = f.fs.stat(&Fixture::at(&id, &path)).unwrap();
        assert_eq!(stat.size_class, Some(class), "{size}");
        let read = f.read(&id, &path).unwrap();
        let j = &read.judgment;
        assert_eq!(
            (j.size_class, j.editable, j.highlight),
            (class, editable, editable),
            "{size}"
        );
        fs::remove_file(&path).unwrap();
    }
    // 50 MiB + 1 은 stat 만으로 「열지 않음」 이고 전체 읽기는 거절한다.
    let path = root.join("over.txt");
    fs::write(&path, text_of(READ_ONLY_MAX + 1)).unwrap();
    let stat = f.fs.stat(&Fixture::at(&id, &path)).unwrap();
    assert_eq!(stat.size_class, Some(SizeClass::TooLarge));
    assert_eq!(f.read(&id, &path).unwrap_err().code, code::INVALID_PARAMS);

    // 1만 자 줄은 강조, 1만 1자 줄은 강조 · 줄바꿈 끔(편집은 된다).
    for (chars, highlight) in [(LONG_LINE_CHARS, true), (LONG_LINE_CHARS + 1, false)] {
        let path = root.join(format!("line-{chars}.js"));
        fs::write(&path, format!("a\n{}\nb\n", "x".repeat(chars))).unwrap();
        let j = f.read(&id, &path).unwrap().judgment;
        assert_eq!(
            (j.highlight, j.long_lines, j.editable),
            (highlight, !highlight, true)
        );
    }
}

/// M1-23: 1 GB 파일은 stat 만으로 「열지 않음」 이고(내용을 읽지 않는다), hex 의 끝 페이지는 그 페이지만 읽는다(`bytes.rs` 의
/// `a_page_reads_only_its_own_bytes` 가 읽은 바이트를 센다).
#[test]
fn a_1_gib_file_is_judged_by_stat_and_its_last_page_is_read_alone() {
    use std::io::{Seek as _, SeekFrom, Write as _};

    use crate::bytes::FsReadBytesParams;
    use crate::limits::HEX_PAGE_BYTES;

    const GIB: u64 = 1024 * 1024 * 1024;
    let f = Fixture::new();
    let root = f.base.join("proj");
    let id = f.project(&root);
    let path = root.join("huge.bin");
    // 희소 파일: 끝 4바이트만 디스크에 있다.
    let mut file = fs::File::create(&path).unwrap();
    file.set_len(GIB).unwrap();
    file.seek(SeekFrom::Start(GIB - 4)).unwrap();
    file.write_all(b"END\n").unwrap();
    drop(file);

    let started = std::time::Instant::now();
    let stat = f.fs.stat(&Fixture::at(&id, &path)).unwrap();
    assert_eq!(
        (stat.size, stat.size_class),
        (GIB, Some(SizeClass::TooLarge))
    );
    assert!(started.elapsed() < std::time::Duration::from_secs(1));

    let page = |offset: u64, length: u64| {
        f.fs.read_bytes(&FsReadBytesParams {
            project_id: id.clone(),
            worktree: None,
            path: path.display().to_string(),
            offset,
            length,
        })
    };
    let last = page(GIB - HEX_PAGE_BYTES, HEX_PAGE_BYTES).unwrap();
    let data = BASE64.decode(&last.data).unwrap();
    assert_eq!(data.len() as u64, HEX_PAGE_BYTES);
    assert!(data.ends_with(b"END\n"));
    assert_eq!(
        (last.offset, last.size, last.eof),
        (GIB - HEX_PAGE_BYTES, GIB, true)
    );
    let middle = page(GIB / 2, 16).unwrap();
    assert!(!middle.eof);
    assert_eq!(BASE64.decode(&middle.data).unwrap(), vec![0; 16]);
    // 끝을 넘는 범위는 빈 페이지다.
    let past = page(GIB + 10, 16).unwrap();
    assert!(past.data.is_empty() && past.eof);
    // 한 번에 64 KiB 까지.
    assert_eq!(
        page(0, HEX_PAGE_BYTES + 1).unwrap_err().code,
        code::INVALID_PARAMS
    );
}

/// M1-23: FIFO 는 바이트 페이지로 열지 않는다 — 열면 쓰는 쪽이 올 때까지 멈춘다.
#[cfg(unix)]
#[test]
fn fifos_are_not_read_as_byte_pages() {
    use crate::bytes::FsReadBytesParams;

    let f = Fixture::new();
    let root = f.base.join("proj");
    let id = f.project(&root);
    let fifo = root.join("pipe");
    mkfifo(&fifo);

    let stat = f.fs.stat(&Fixture::at(&id, &fifo)).unwrap();
    assert_eq!((stat.kind, stat.size_class), (EntryKind::Other, None));
    let (tx, rx) = std::sync::mpsc::channel();
    let params = FsReadBytesParams {
        project_id: id,
        worktree: None,
        path: fifo.display().to_string(),
        offset: 0,
        length: 16,
    };
    std::thread::spawn(move || tx.send(f.fs.read_bytes(&params)).unwrap());
    let got = rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("fs.readBytes on a FIFO must not block");
    assert_eq!(got.unwrap_err().code, code::INVALID_PARAMS);
}
