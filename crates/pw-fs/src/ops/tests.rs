//! 파일 조작(`fs.create` · `fs.rename` · `fs.copy` · `fs.trash`, Finder 드롭 들여오기). 휴지통은 가짜를 넘긴다 — 사람의 휴지통에
//! 시험 파일이 쌓이지 않게. 진짜 휴지통은 macOS 시험 하나만 쓰고, 그 파일 이름에 `pitwall-test` 를 단다(휴지통을 비울 방법이 없어
//! 사람이 알아보게).

use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use pw_core::bus::{DEFAULT_CAPACITY, EventBus};
use pw_core::config::AppPaths;
use pw_core::registry::path::simplify;
use pw_core::registry::{NewProject, Registry as Projects};
use pw_rpc::{Caller, EventHub, Method as _, Role, Session};
use serde_json::json;

use super::*;

struct Fixture {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    fs: Arc<Fs>,
    projects: Projects,
}

impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let base = simplify(fs::canonicalize(tmp.path()).unwrap());
        let projects = Projects::new(
            AppPaths::under(&base.join("app")),
            None,
            EventBus::new(DEFAULT_CAPACITY),
        );
        let fs = Arc::new(Fs::new());
        fs.attach(projects.clone());
        Self {
            _tmp: tmp,
            base,
            fs,
            projects,
        }
    }

    /// `proj` 를 루트로 둔 프로젝트.
    fn project(&self) -> (String, PathBuf) {
        let root = self.base.join("proj");
        fs::create_dir_all(&root).unwrap();
        let id = self
            .projects
            .create(NewProject {
                root: root.clone(),
                ..NewProject::default()
            })
            .unwrap()
            .id;
        (id, root)
    }

    fn create(&self, id: &str, path: &Path, kind: FsCreateKind) -> Result<FsCreated, RpcError> {
        self.fs.create(&FsCreateParams {
            project_id: id.to_owned(),
            worktree: None,
            path: path.display().to_string(),
            kind,
        })
    }

    fn rename(&self, id: &str, from: &Path, to: &Path) -> Result<FsMoved, RpcError> {
        self.fs.rename(&from_to(id, from, to))
    }

    fn copy(&self, id: &str, from: &Path, to: &Path) -> Result<FsCopied, RpcError> {
        self.fs.copy(&from_to(id, from, to))
    }

    fn trash(
        &self,
        id: &str,
        paths: &[&Path],
        permanent: bool,
        trasher: Trasher<'_>,
    ) -> Result<FsTrashed, RpcError> {
        self.fs.trash_with(
            &FsTrashParams {
                project_id: id.to_owned(),
                worktree: None,
                paths: paths.iter().map(|p| p.display().to_string()).collect(),
                permanent,
            },
            trasher,
        )
    }
}

fn from_to(id: &str, from: &Path, to: &Path) -> FsFromToParams {
    FsFromToParams {
        project_id: id.to_owned(),
        worktree: None,
        from: from.display().to_string(),
        to: to.display().to_string(),
    }
}

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap()
}

/// 이 모듈의 임시 이름이 남지 않았다.
fn no_leftovers(dir: &Path) {
    let left: Vec<_> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".pitwall-tmp"))
        .collect();
    assert!(left.is_empty(), "{left:?}");
}

fn assert_code(result: Result<impl std::fmt::Debug, RpcError>, expected: i32) -> RpcError {
    let e = result.expect_err("an error");
    assert_eq!(e.code, expected, "{e:?}");
    e
}

/// 휴지통에 넣은 것을 적고 지우는 가짜(진짜 휴지통처럼 원래 자리에서 사라진다).
fn recording_trash(log: &RefCell<Vec<PathBuf>>) -> impl Fn(&Path) -> Result<(), trash::Error> + '_ {
    move |path| {
        log.borrow_mut().push(path.to_path_buf());
        remove_entry(path).map_err(|e| trash::Error::Unknown {
            description: e.to_string(),
        })
    }
}

#[test]
fn create_makes_files_and_folders_and_never_overwrites() {
    let fx = Fixture::new();
    let (id, root) = fx.project();
    let file = root.join("new.txt");
    let made = fx.create(&id, &file, FsCreateKind::File).unwrap();
    assert_eq!(made.path, file.display().to_string());
    assert_eq!(fs::read(&file).unwrap(), b"");
    let dir = root.join("src");
    fx.create(&id, &dir, FsCreateKind::Dir).unwrap();
    assert!(dir.is_dir());

    // 같은 이름: 파일 · 폴더 어느 쪽이든 `CONFLICT` 이고 있던 것은 그대로다.
    write(&file, "keep");
    for kind in [FsCreateKind::File, FsCreateKind::Dir] {
        let e = assert_code(fx.create(&id, &file, kind), code::CONFLICT);
        assert_eq!(e.data, Some(json!({ "path": file.display().to_string() })));
        assert_eq!(read(&file), "keep");
        assert_code(fx.create(&id, &dir, kind), code::CONFLICT);
    }
    // 들어갈 폴더가 없다 · 루트 밖 · 상대 경로 · 루트 자체.
    assert_code(
        fx.create(&id, &root.join("missing/a.txt"), FsCreateKind::File),
        code::NOT_FOUND,
    );
    assert_code(
        fx.create(&id, &fx.base.join("outside.txt"), FsCreateKind::File),
        code::INVALID_PARAMS,
    );
    assert!(!fx.base.join("outside.txt").exists());
    assert_code(
        fx.create(&id, Path::new("rel.txt"), FsCreateKind::File),
        code::INVALID_PARAMS,
    );
    assert_code(
        fx.create(&id, &root, FsCreateKind::Dir),
        code::INVALID_PARAMS,
    );
}

#[test]
fn rename_moves_files_and_folders_without_overwriting() {
    let fx = Fixture::new();
    let (id, root) = fx.project();
    write(&root.join("a.txt"), "a");
    write(&root.join("dir/inner.txt"), "inner");
    fs::create_dir_all(root.join("dest")).unwrap();

    let moved = fx
        .rename(&id, &root.join("a.txt"), &root.join("b.txt"))
        .unwrap();
    assert_eq!(moved.from, root.join("a.txt").display().to_string());
    assert_eq!(moved.to, root.join("b.txt").display().to_string());
    assert!(!root.join("a.txt").exists());
    assert_eq!(read(&root.join("b.txt")), "a");

    // 폴더를 다른 폴더로 옮긴다(드래그 이동).
    fx.rename(&id, &root.join("dir"), &root.join("dest/dir"))
        .unwrap();
    assert_eq!(read(&root.join("dest/dir/inner.txt")), "inner");
    assert!(!root.join("dir").exists());

    // 대상이 있으면 아무것도 바꾸지 않는다(파일 위로 · 폴더 위로).
    write(&root.join("c.txt"), "c");
    let e = assert_code(
        fx.rename(&id, &root.join("b.txt"), &root.join("c.txt")),
        code::CONFLICT,
    );
    assert_eq!(
        e.data,
        Some(json!({ "path": root.join("c.txt").display().to_string() }))
    );
    assert_eq!(read(&root.join("b.txt")), "a");
    assert_eq!(read(&root.join("c.txt")), "c");
    fs::create_dir_all(root.join("empty")).unwrap();
    assert_code(
        fx.rename(&id, &root.join("dest"), &root.join("empty")),
        code::CONFLICT,
    );
    assert!(root.join("dest/dir/inner.txt").exists());

    // 자기 안으로 · 같은 이름 · 루트 밖 · 루트 자체 · 없는 원본.
    assert_code(
        fx.rename(&id, &root.join("dest"), &root.join("dest/dir/x")),
        code::INVALID_PARAMS,
    );
    assert_code(
        fx.rename(&id, &root.join("c.txt"), &root.join("c.txt")),
        code::INVALID_PARAMS,
    );
    assert_code(
        fx.rename(&id, &root.join("c.txt"), &fx.base.join("c.txt")),
        code::INVALID_PARAMS,
    );
    assert_code(
        fx.rename(&id, &fx.base.join("app"), &root.join("app")),
        code::INVALID_PARAMS,
    );
    assert_code(
        fx.rename(&id, &root, &fx.base.join("renamed")),
        code::INVALID_PARAMS,
    );
    assert_code(
        fx.rename(&id, &root.join("none.txt"), &root.join("x.txt")),
        code::NOT_FOUND,
    );
    assert!(root.join("c.txt").exists());
}

/// 대소문자를 무시하는 볼륨(macOS 기본 APFS · NTFS)에서 대소문자만 바꾸는 이름은 같은 항목이라 그대로 바꾼다. 구분하는 볼륨이면
/// 다른 이름이라 보통 이름 바꾸기다.
#[test]
fn rename_changes_only_the_case() {
    let fx = Fixture::new();
    let (id, root) = fx.project();
    write(&root.join("readme.md"), "r");
    fx.rename(&id, &root.join("readme.md"), &root.join("README.md"))
        .unwrap();
    let names: Vec<_> = fs::read_dir(&root)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["README.md"]);
}

/// 「없을 때만」 은 확인 뒤가 아니라 마지막 OS 호출이 지킨다 — 확인과 쓰기 사이에 생긴 대상(파일 · 폴더)도 덮지 않는다.
#[test]
fn the_final_rename_does_not_replace_a_target_created_after_the_check() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    write(&dir.join("from.txt"), "from");
    write(&dir.join("to.txt"), "raced in");
    let e = rename_noreplace(&dir.join("from.txt"), &dir.join("to.txt")).unwrap_err();
    assert_eq!(e.kind(), io::ErrorKind::AlreadyExists, "{e}");
    assert_eq!(read(&dir.join("from.txt")), "from");
    assert_eq!(read(&dir.join("to.txt")), "raced in");

    // 폴더 위로 · 빈 폴더 위로(보통 rename 은 빈 폴더를 바꿔 끼운다).
    fs::create_dir_all(dir.join("a")).unwrap();
    fs::create_dir_all(dir.join("b")).unwrap();
    assert!(rename_noreplace(&dir.join("a"), &dir.join("b")).is_err());
    assert!(dir.join("a").is_dir());
    assert!(rename_noreplace(&dir.join("from.txt"), &dir.join("b")).is_err());
    assert_eq!(read(&dir.join("from.txt")), "from");

    rename_noreplace(&dir.join("from.txt"), &dir.join("new.txt")).unwrap();
    assert_eq!(read(&dir.join("new.txt")), "from");
}

/// 규칙 A: 원본이 루트 밖을 가리키는 링크면 거절한다. 루트 안을 가리키는 링크는 링크 자체를 바꾼다(대상은 그대로). 들어갈 폴더가
/// 루트 밖 링크면 대상도 거절한다.
#[cfg(unix)]
#[test]
fn symlinks_are_judged_by_their_real_target_and_moved_as_links() {
    use std::os::unix::fs::symlink;
    let fx = Fixture::new();
    let (id, root) = fx.project();
    write(&fx.base.join("secret/s.txt"), "s");
    write(&root.join("real.txt"), "real");
    symlink(fx.base.join("secret"), root.join("out")).unwrap();
    symlink(root.join("real.txt"), root.join("in.txt")).unwrap();

    for result in [
        fx.rename(&id, &root.join("out"), &root.join("out2"))
            .map(drop),
        fx.copy(&id, &root.join("out"), &root.join("copy"))
            .map(drop),
        // 들어 있는 폴더가 루트 밖 링크.
        fx.rename(&id, &root.join("out/s.txt"), &root.join("s.txt"))
            .map(drop),
        // 들어갈 폴더가 루트 밖 링크.
        fx.rename(&id, &root.join("real.txt"), &root.join("out/real.txt"))
            .map(drop),
        fx.copy(&id, &root.join("real.txt"), &root.join("out/real.txt"))
            .map(drop),
        fx.create(&id, &root.join("out/new.txt"), FsCreateKind::File)
            .map(drop),
    ] {
        assert_code(result, code::INVALID_PARAMS);
    }
    assert!(root.join("out").is_symlink());
    assert_eq!(fs::read_dir(fx.base.join("secret")).unwrap().count(), 1);

    let log = RefCell::new(Vec::new());
    let trashed = fx.trash(&id, &[&root.join("out")], false, &recording_trash(&log));
    assert_code(trashed, code::INVALID_PARAMS);
    assert!(log.borrow().is_empty());

    fx.rename(&id, &root.join("in.txt"), &root.join("link.txt"))
        .unwrap();
    assert!(root.join("link.txt").is_symlink());
    assert_eq!(read(&root.join("real.txt")), "real");
}

#[cfg(unix)]
#[test]
fn copy_copies_trees_without_following_links_and_skips_special_files() {
    use std::os::unix::fs::{PermissionsExt as _, symlink};
    let fx = Fixture::new();
    let (id, root) = fx.project();
    write(&root.join("tree/a.txt"), "a");
    write(&root.join("tree/sub/b.sh"), "#!/bin/sh\n");
    fs::set_permissions(
        root.join("tree/sub/b.sh"),
        fs::Permissions::from_mode(0o751),
    )
    .unwrap();
    write(&fx.base.join("outside/secret.txt"), "secret");
    symlink(fx.base.join("outside"), root.join("tree/link-out")).unwrap();
    symlink("a.txt", root.join("tree/link-rel")).unwrap();
    let fifo = root.join("tree/pipe");
    let c = std::ffi::CString::new(fifo.display().to_string()).unwrap();
    // SAFETY: `c` 는 NUL 로 끝나는 살아 있는 경로다.
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o644) }, 0);

    let copied = fx
        .copy(&id, &root.join("tree"), &root.join("copy"))
        .unwrap();
    assert_eq!(
        (copied.files, copied.dirs, copied.links),
        (2, 2, 2),
        "{copied:?}"
    );
    assert_eq!(copied.skipped, [fifo.display().to_string()]);
    assert_eq!(read(&root.join("copy/a.txt")), "a");
    assert_eq!(
        fs::metadata(root.join("copy/sub/b.sh"))
            .unwrap()
            .permissions()
            .mode()
            & 0o7777,
        0o751
    );
    // 링크는 같은 대상 글자의 링크다 — 루트 밖 내용은 복사되지 않았다.
    assert_eq!(
        fs::read_link(root.join("copy/link-out")).unwrap(),
        fx.base.join("outside")
    );
    assert_eq!(
        fs::read_link(root.join("copy/link-rel")).unwrap(),
        Path::new("a.txt")
    );
    assert!(!root.join("copy/pipe").exists());
    no_leftovers(&root);

    // 파일 하나.
    fx.copy(&id, &root.join("tree/a.txt"), &root.join("a copy.txt"))
        .unwrap();
    assert_eq!(read(&root.join("a copy.txt")), "a");
}

#[test]
fn copy_never_overwrites_and_leaves_no_partial_copy() {
    let fx = Fixture::new();
    let (id, root) = fx.project();
    write(&root.join("tree/a.txt"), "a");
    write(&root.join("taken/keep.txt"), "keep");
    write(&root.join("file.txt"), "file");

    assert_code(
        fx.copy(&id, &root.join("tree"), &root.join("taken")),
        code::CONFLICT,
    );
    assert_code(
        fx.copy(&id, &root.join("tree/a.txt"), &root.join("file.txt")),
        code::CONFLICT,
    );
    assert_eq!(read(&root.join("taken/keep.txt")), "keep");
    assert!(!root.join("taken/a.txt").exists());
    assert_eq!(read(&root.join("file.txt")), "file");
    // 자기 안으로.
    assert_code(
        fx.copy(&id, &root.join("tree"), &root.join("tree/inside")),
        code::INVALID_PARAMS,
    );
    assert!(!root.join("tree/inside").exists());

    // 자리를 잡기 직전에 같은 이름이 생기면(경합) 덮지 않고 임시 복사본을 치운다.
    let mut tally = Tally::default();
    let raced = place_copy(&root.join("tree"), &root.join("raced"), &mut tally, &|| {
        // 복사가 도는 중에(항목마다 부른다) 대상 이름을 만든다.
        let _ = fs::create_dir(root.join("raced"));
        false
    });
    let e = raced.unwrap_err();
    assert!(op_error(&root.join("raced"), e).code == code::CONFLICT);
    assert_eq!(fs::read_dir(root.join("raced")).unwrap().count(), 0);
    no_leftovers(&root);
}

#[test]
fn trash_judges_everything_first_and_reports_what_it_could_not_remove() {
    let fx = Fixture::new();
    let (id, root) = fx.project();
    write(&root.join("a.txt"), "a");
    write(&root.join("dir/b.txt"), "b");
    write(&root.join("keep.txt"), "keep");

    // 하나라도 루트 밖이면 아무것도 지우지 않는다.
    let log = RefCell::new(Vec::new());
    assert_code(
        fx.trash(
            &id,
            &[&root.join("a.txt"), &fx.base.join("app")],
            false,
            &recording_trash(&log),
        ),
        code::INVALID_PARAMS,
    );
    assert_code(
        fx.trash(&id, &[&root], false, &recording_trash(&log)),
        code::INVALID_PARAMS,
    );
    assert!(log.borrow().is_empty());
    assert!(root.join("a.txt").exists());

    // 폴더와 그 안의 파일을 함께 고르면 폴더 하나만 넣는다. 없는 것은 `missing`.
    let done = fx
        .trash(
            &id,
            &[
                &root.join("dir/b.txt"),
                &root.join("dir"),
                &root.join("a.txt"),
                &root.join("gone"),
            ],
            false,
            &recording_trash(&log),
        )
        .unwrap();
    assert_eq!(*log.borrow(), [root.join("a.txt"), root.join("dir")]);
    assert_eq!(
        done.removed,
        [
            root.join("a.txt").display().to_string(),
            root.join("dir").display().to_string(),
            root.join("dir/b.txt").display().to_string(),
        ]
    );
    assert_eq!(done.failed.len(), 1);
    assert_eq!(done.failed[0].reason, FsTrashReason::Missing);
    assert!(root.join("keep.txt").exists());

    // 휴지통이 받지 않으면 `unavailable` 이고 파일은 그대로다. 확인 뒤의 `permanent` 는 휴지통을 거치지 않고 지운다.
    let refuse = |_: &Path| -> Result<(), trash::Error> {
        Err(trash::Error::Unknown {
            description: "no trash on this volume".into(),
        })
    };
    let done = fx
        .trash(&id, &[&root.join("keep.txt")], false, &refuse)
        .unwrap();
    assert!(done.removed.is_empty());
    assert_eq!(done.failed[0].reason, FsTrashReason::Unavailable);
    assert!(
        done.failed[0].message.contains("no trash"),
        "{:?}",
        done.failed
    );
    assert!(root.join("keep.txt").exists());
    let done = fx
        .trash(&id, &[&root.join("keep.txt")], true, &refuse)
        .unwrap();
    assert_eq!(done.removed, [root.join("keep.txt").display().to_string()]);
    assert!(!root.join("keep.txt").exists());
}

/// 들어 있는 폴더에 쓰기 권한이 없으면 휴지통 · 영구 삭제 모두 `permission` 이다(휴지통 없음과 구분한다).
#[cfg(unix)]
#[test]
fn trash_tells_a_permission_problem_apart_from_a_missing_trash() {
    use std::os::unix::fs::PermissionsExt as _;
    // SAFETY: geteuid 는 인자 없이 값만 돌려준다.
    if unsafe { libc::geteuid() } == 0 {
        return; // root 는 권한 비트를 넘는다.
    }
    let fx = Fixture::new();
    let (id, root) = fx.project();
    write(&root.join("locked/a.txt"), "a");
    fs::set_permissions(root.join("locked"), fs::Permissions::from_mode(0o555)).unwrap();
    let log = RefCell::new(Vec::new());
    for permanent in [false, true] {
        let done = fx
            .trash(
                &id,
                &[&root.join("locked/a.txt")],
                permanent,
                &recording_trash(&log),
            )
            .unwrap();
        assert_eq!(
            done.failed[0].reason,
            FsTrashReason::Permission,
            "{permanent}"
        );
    }
    assert!(log.borrow().is_empty());
    fs::set_permissions(root.join("locked"), fs::Permissions::from_mode(0o755)).unwrap();
    assert!(root.join("locked/a.txt").exists());
}

/// 영구 삭제는 폴더 아래의 링크를 따르지 않는다 — 루트 밖 대상은 남는다.
#[cfg(unix)]
#[test]
fn permanent_delete_does_not_follow_links() {
    use std::os::unix::fs::symlink;
    let fx = Fixture::new();
    let (id, root) = fx.project();
    write(&fx.base.join("outside/o.txt"), "o");
    write(&root.join("dir/a.txt"), "a");
    symlink(fx.base.join("outside"), root.join("dir/link")).unwrap();
    let unused = |_: &Path| -> Result<(), trash::Error> { unreachable!() };
    let done = fx.trash(&id, &[&root.join("dir")], true, &unused).unwrap();
    assert!(done.failed.is_empty(), "{:?}", done.failed);
    assert!(!root.join("dir").exists());
    assert_eq!(read(&fx.base.join("outside/o.txt")), "o");
}

/// 진짜 macOS 휴지통: 임시 폴더 안의 `pitwall-test-…` 파일 하나만 넣는다. 원래 자리에서 사라진 것을 본다(휴지통 안은 사람이
/// Finder 로 확인한다 — 12-m1-plan M1-21 의 「macOS 수동 확인」).
#[cfg(target_os = "macos")]
#[test]
fn the_os_trash_takes_a_test_file_on_macos() {
    let fx = Fixture::new();
    let (id, root) = fx.project();
    let name = format!("pitwall-test-trash-{}.txt", std::process::id());
    write(&root.join(&name), "pitwall test file, safe to delete");
    let done = fx
        .trash(&id, &[&root.join(&name)], false, &os_trash)
        .unwrap();
    assert!(done.failed.is_empty(), "{:?}", done.failed);
    assert!(!root.join(&name).exists());
}

/// 네 메서드 모두 write 등급이라 owner 만 부른다(`permanent` 도) — readonly · mobile 연결은 막힌다. MCP 툴이 아니다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn only_the_owner_may_call_file_operations_and_none_is_an_mcp_tool() {
    let fx = Fixture::new();
    let (id, root) = fx.project();
    write(&root.join("a.txt"), "a");
    let mut router = Router::new();
    register(&mut router, Arc::clone(&fx.fs));
    let router = Arc::new(router);
    let a = root.join("a.txt").display().to_string();
    let b = root.join("b.txt").display().to_string();
    let calls = [
        (
            "fs.create",
            json!({"projectId": id, "path": b, "kind": "file"}),
        ),
        ("fs.rename", json!({"projectId": id, "from": a, "to": b})),
        ("fs.copy", json!({"projectId": id, "from": a, "to": b})),
        ("fs.trash", json!({"projectId": id, "paths": [a]})),
        (
            "fs.trash",
            json!({"projectId": id, "paths": [a], "permanent": true}),
        ),
    ];
    for role in [Role::Readonly, Role::Mobile] {
        let (session, mut rx) =
            Session::new(Arc::clone(&router), EventHub::new(), Caller::new(role));
        for (n, (method, params)) in calls.iter().enumerate() {
            session.handle_value(
                json!({"jsonrpc": "2.0", "id": n, "method": method, "params": params}),
            );
            let response = tokio::time::timeout(Duration::from_secs(5), rx.recv())
                .await
                .unwrap()
                .unwrap()
                .into_response()
                .unwrap();
            let e = response.outcome.expect_err("denied");
            assert_eq!(e.code, code::PERMISSION_DENIED, "{role:?} {method}");
        }
    }
    assert_eq!(read(&root.join("a.txt")), "a");
    assert!(!root.join("b.txt").exists());

    // owner 는 부른다.
    let (session, mut rx) = Session::new(router, EventHub::new(), Caller::new(Role::Owner));
    session.handle_value(
        json!({"jsonrpc": "2.0", "id": 1, "method": "fs.copy", "params": calls[2].1}),
    );
    let response = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .unwrap()
        .unwrap()
        .into_response()
        .unwrap();
    assert!(response.outcome.is_ok(), "{:?}", response.outcome);
    assert_eq!(read(&root.join("b.txt")), "a");

    for meta in [
        FsCreateMethod::META,
        FsRenameMethod::META,
        FsCopyMethod::META,
        FsTrashMethod::META,
    ] {
        assert_eq!(meta.tier, pw_rpc::Tier::Write, "{}", meta.name);
        assert!(!meta.mcp, "{}", meta.name);
    }
}

/// Finder 드롭 들여오기: 루트 밖 원본을 대상 폴더에 복사하고, 같은 이름은 덮지 않고 알리며, 링크는 링크로 · 특수 파일은 건너뛴다.
/// 대상 폴더는 쓰기 규칙으로 판정한다.
#[cfg(unix)]
#[test]
fn import_copies_dropped_items_into_a_folder_inside_the_roots() {
    use std::os::unix::fs::symlink;
    let fx = Fixture::new();
    let (id, root) = fx.project();
    let finder = fx.base.join("Desktop");
    write(&finder.join("one.txt"), "one");
    write(&finder.join("folder/two.txt"), "two");
    symlink("/etc", finder.join("folder/etc")).unwrap();
    let fifo = finder.join("folder/pipe");
    let c = std::ffi::CString::new(fifo.display().to_string()).unwrap();
    // SAFETY: `c` 는 NUL 로 끝나는 살아 있는 경로다.
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o644) }, 0);
    write(&finder.join("taken.txt"), "new");
    write(&root.join("in/taken.txt"), "old");

    let sources = [
        finder.join("one.txt"),
        finder.join("folder"),
        finder.join("taken.txt"),
    ];
    let mut seen = Vec::new();
    let out = fx
        .fs
        .import(&id, None, &root.join("in"), &sources, &|| false, &mut |n| {
            seen.push(n)
        })
        .unwrap();
    assert_eq!(
        out.copied,
        [
            root.join("in/one.txt").display().to_string(),
            root.join("in/folder").display().to_string(),
        ]
    );
    assert_eq!(
        out.conflicts,
        [finder.join("taken.txt").display().to_string()]
    );
    assert_eq!(out.skipped, [fifo.display().to_string()]);
    assert_eq!(out.links, 1);
    assert!(!out.cancelled);
    assert_eq!(seen, [1, 2, 3]);
    assert_eq!(read(&root.join("in/folder/two.txt")), "two");
    assert_eq!(
        fs::read_link(root.join("in/folder/etc")).unwrap(),
        Path::new("/etc")
    );
    assert_eq!(read(&root.join("in/taken.txt")), "old");
    no_leftovers(&root.join("in"));

    // 대상 폴더가 루트 밖 · 루트 밖 링크 · 파일이면 거절한다.
    symlink(&finder, root.join("finder-link")).unwrap();
    for dir in [
        finder.clone(),
        root.join("finder-link"),
        root.join("in/one.txt"),
    ] {
        let result = fx
            .fs
            .import(&id, None, &dir, &sources[..1], &|| false, &mut |_| {});
        assert_code(result, code::INVALID_PARAMS);
    }
    // 루트 자체에는 넣을 수 있다.
    fx.fs
        .import(&id, None, &root, &sources[..1], &|| false, &mut |_| {})
        .unwrap();
    assert_eq!(read(&root.join("one.txt")), "one");
}

#[test]
fn import_stops_when_cancelled_and_cleans_up_the_current_item() {
    let fx = Fixture::new();
    let (id, root) = fx.project();
    let finder = fx.base.join("Desktop");
    for i in 0..20 {
        write(&finder.join(format!("big/f{i:02}.txt")), "x");
    }
    write(&finder.join("after.txt"), "after");
    let calls = std::cell::Cell::new(0);
    // 큰 폴더를 복사하는 도중(다섯 번째 항목)에 취소한다.
    let cancelled = || {
        calls.set(calls.get() + 1);
        calls.get() > 5
    };
    let out = fx
        .fs
        .import(
            &id,
            None,
            &root,
            &[finder.join("big"), finder.join("after.txt")],
            &cancelled,
            &mut |_| {},
        )
        .unwrap();
    assert!(out.cancelled);
    assert!(out.copied.is_empty(), "{out:?}");
    assert!(!root.join("big").exists());
    assert!(!root.join("after.txt").exists());
    no_leftovers(&root);
}
