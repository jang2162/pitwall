//! 파일 목록 인덱스(M1-12 완료 기준): 목록에 드는 것, 점수 · 일치 위치, 실제 OS 감시로의 갱신(500 ms), 다시 만들기, 작업.

use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use pw_core::bus::{DEFAULT_CAPACITY, EventBus, Filter, Received, Scope};
use pw_core::registry::path::simplify;
use pw_core::registry::{NewProject, Registry as Projects};
use pw_core::task::{TOPIC_FINISHED, TaskFinishedEvent};
use pw_core::{TaskSpawner, TaskStatus, config::AppPaths};
use tokio_util::sync::CancellationToken;

use super::matching::{LOWERED_DIVISOR, utf16_positions};
use super::*;
use crate::scope::Roots;
use crate::watch::{FsChanged, FsWatchParams, Watch};

/// 완료 기준: 파일을 만들거나 지우면 이 안에 결과에 반영된다.
const REFLECTED: Duration = Duration::from_millis(500);

type Hook = Box<dyn Fn() + Send>;

/// 루트별로 처음부터 걷기를 마친 뒤 · 바꿔 끼우기 전에 부를 것(시험 프로세스 안의 다른 시험과 섞이지 않게 루트로 가른다).
static AFTER_WALK: Mutex<Option<HashMap<PathBuf, Hook>>> = Mutex::new(None);

pub(super) fn after_walk(root: &Path) {
    let hooks = AFTER_WALK.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(hook) = hooks.as_ref().and_then(|h| h.get(root)) {
        hook();
    }
}

/// 루트별 감시하지 않는 걷기 횟수.
static ADHOC_WALKS: Mutex<Option<HashMap<PathBuf, usize>>> = Mutex::new(None);

pub(super) fn adhoc_walked(root: &Path) {
    *ADHOC_WALKS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get_or_insert_with(HashMap::new)
        .entry(root.to_path_buf())
        .or_default() += 1;
}

fn adhoc_walks(root: &Path) -> usize {
    ADHOC_WALKS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .as_ref()
        .and_then(|w| w.get(root).copied())
        .unwrap_or(0)
}

fn set_after_walk(root: &Path, hook: impl Fn() + Send + 'static) {
    AFTER_WALK
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get_or_insert_with(HashMap::new)
        .insert(root.to_path_buf(), Box::new(hook));
}

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

struct Fixture {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    fs: Arc<Fs>,
    projects: Projects,
    bus: EventBus,
}

impl Fixture {
    fn new() -> Self {
        Self::with_bus(EventBus::new(DEFAULT_CAPACITY))
    }

    fn with_bus(bus: EventBus) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let base = simplify(fs::canonicalize(tmp.path()).unwrap());
        let projects = Projects::new(AppPaths::under(&base.join("app")), None, bus.clone());
        let fs = Arc::new(Fs::new());
        fs.attach(projects.clone());
        Self {
            _tmp: tmp,
            base,
            fs,
            projects,
            bus,
        }
    }

    /// `name` 폴더를 저장소(`.git`)로 만들어 프로젝트로 등록한다.
    fn repo(&self, name: &str) -> (String, PathBuf) {
        let root = self.base.join(name);
        fs::create_dir_all(root.join(".git")).unwrap();
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

    /// 버스 · 작업을 붙인 인덱스. 변경을 적용하는 future 를 띄운다(tokio 런타임 안에서 부른다).
    fn index(&self) -> FileIndex {
        let index = FileIndex::new(Arc::clone(&self.fs));
        let tasks = TaskSpawner::new(self.bus.clone(), Scope::Host, CancellationToken::new());
        tokio::spawn(index.attach(&self.bus, tasks));
        index
    }

    fn watch(&self, index: &FileIndex) -> Watch {
        let watch = Watch::new(Arc::clone(&self.fs));
        watch.attach(self.bus.clone());
        watch.attach_index(index.clone());
        watch
    }
}

fn params(project: &str, query: &str) -> SearchFilesParams {
    SearchFilesParams {
        project_id: project.to_owned(),
        worktree: None,
        query: query.to_owned(),
        limit: None,
    }
}

fn relatives(found: &SearchFiles) -> Vec<&str> {
    found.files.iter().map(|f| f.relative.as_str()).collect()
}

async fn all_files(index: &FileIndex, project: &str) -> BTreeSet<String> {
    let mut p = params(project, "");
    p.limit = Some(MAX_LIMIT);
    let found = index.search(p).await.unwrap();
    found.files.into_iter().map(|f| f.relative).collect()
}

/// `query` 의 결과에 `relative` 가 있고 없음이 `present` 가 될 때까지 기다린 시간. `within` 안에 안 되면 `None`.
async fn until(
    index: &FileIndex,
    project: &str,
    query: &str,
    relative: &str,
    present: bool,
    within: Duration,
) -> Option<Duration> {
    let started = Instant::now();
    loop {
        let found = index.search(params(project, query)).await.unwrap();
        if relatives(&found).contains(&relative) == present {
            return Some(started.elapsed());
        }
        if started.elapsed() > within {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

#[cfg(unix)]
fn symlink(target: &Path, link: &Path) {
    std::os::unix::fs::symlink(target, link).unwrap();
}

#[cfg(windows)]
fn symlink(target: &Path, link: &Path) {
    if target.is_dir() {
        std::os::windows::fs::symlink_dir(target, link).unwrap();
    } else {
        std::os::windows::fs::symlink_file(target, link).unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_list_holds_files_that_are_not_ignored() {
    let f = Fixture::new();
    let (id, root) = f.repo("proj");
    write(&root.join(".gitignore"), "target/\n*.log\n");
    write(&root.join("src/lib.rs"), "");
    write(&root.join("src/main.rs"), "");
    write(&root.join(".github/workflows/ci.yml"), "");
    write(&root.join("target/out.rs"), "");
    write(&root.join("debug.log"), "");
    write(&root.join(".git/config"), "");
    let outside = f.base.join("outside");
    write(&outside.join("secret.rs"), "");
    symlink(&root.join("src"), &root.join("in_link"));
    symlink(&outside, &root.join("out_link"));
    let index = f.index();

    let listed = all_files(&index, &id).await;
    let want: BTreeSet<String> = [
        ".github/workflows/ci.yml",
        ".gitignore",
        "in_link/lib.rs",
        "in_link/main.rs",
        "src/lib.rs",
        "src/main.rs",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    assert_eq!(
        listed, want,
        "ignore 된 것 · .git · 루트 밖 링크는 빠지고 숨김 파일 · 루트 안 링크는 든다"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_file_name_match_ranks_above_a_folder_match() {
    let f = Fixture::new();
    let (id, root) = f.repo("proj");
    // 경로 점수만으로는 `lib/a.rs` 가 위다(맨 앞 일치 보너스). 파일 이름 가중이 순서를 뒤집는다.
    write(&root.join("lib/a.rs"), "");
    write(&root.join("src/pw-lib/lib.rs"), "");
    let index = f.index();

    let found = index.search(params(&id, "lib")).await.unwrap();
    assert_eq!(relatives(&found), ["src/pw-lib/lib.rs", "lib/a.rs"]);
    assert_eq!(
        found.files[0].positions,
        [11, 12, 13],
        "폴더(`pw-lib`)가 아니라 파일 이름에서 맞은 위치를 준다"
    );
    assert_eq!(found.files[1].positions, [0, 1, 2]);
    assert_eq!(
        found.files[0].path,
        root.join("src")
            .join("pw-lib")
            .join("lib.rs")
            .display()
            .to_string()
    );
    assert_eq!(found.files[0].root, root.display().to_string());
    assert_eq!((found.matched, found.indexed), (2, 2));
}

#[tokio::test(flavor = "multi_thread")]
async fn vendored_and_generated_files_are_lowered_not_dropped() {
    let f = Fixture::new();
    let (id, root) = f.repo("proj");
    // 낮추지 않으면 점수가 같고 경로순으로 `aaa/` 가 앞선다.
    write(&root.join(".gitattributes"), "aaa/** linguist-generated\n");
    write(&root.join("aaa/util.js"), "");
    write(&root.join("vendor/util.js"), "");
    write(&root.join("zzz/util.js"), "");
    write(&root.join("zzzzzz/util.js"), "");
    let index = f.index();

    let found = index.search(params(&id, "util")).await.unwrap();
    assert_eq!(
        relatives(&found),
        [
            "zzz/util.js",
            "zzzzzz/util.js",
            "aaa/util.js",
            "vendor/util.js"
        ]
    );
    let flags: Vec<_> = found
        .files
        .iter()
        .map(|f| (f.vendored, f.generated))
        .collect();
    assert_eq!(
        flags,
        [(false, false), (false, false), (false, true), (true, false)]
    );
    assert_eq!(found.files[0].score, found.files[1].score);
    assert_eq!(found.files[2].score, found.files[0].score / LOWERED_DIVISOR);
}

#[tokio::test(flavor = "multi_thread")]
async fn positions_are_utf16_units_of_the_relative_path() {
    let f = Fixture::new();
    let (id, root) = f.repo("proj");
    write(&root.join("한글/파일.txt"), "");
    write(&root.join("src/😀b.rs"), "");
    let index = f.index();

    let found = index.search(params(&id, "파일")).await.unwrap();
    assert_eq!(relatives(&found), ["한글/파일.txt"]);
    assert_eq!(found.files[0].positions, [3, 4]);
    let found = index.search(params(&id, "b.rs")).await.unwrap();
    assert_eq!(relatives(&found), ["src/😀b.rs"]);
    // `src/` 4 + 😀 2(서로게이트 쌍).
    assert_eq!(found.files[0].positions, [6, 7, 8, 9]);

    assert_eq!(utf16_positions("a😀b", vec![1, 2, 3, 4, 5]), [1, 3]);
    assert_eq!(utf16_positions("abc", vec![2, 0, 2]), [0, 2]);
}

#[tokio::test(flavor = "multi_thread")]
async fn every_word_of_the_query_must_match() {
    let f = Fixture::new();
    let (id, root) = f.repo("proj");
    write(&root.join("src/main.rs"), "");
    write(&root.join("src/lib.rs"), "");
    write(&root.join("app/main.rs"), "");
    let index = f.index();

    let found = index.search(params(&id, "src main")).await.unwrap();
    assert_eq!(relatives(&found), ["src/main.rs"]);
    assert_eq!(found.files[0].positions, [0, 1, 2, 4, 5, 6, 7]);
    let found = index.search(params(&id, "nothing")).await.unwrap();
    assert_eq!((found.files.len(), found.matched, found.indexed), (0, 0, 3));
}

#[tokio::test(flavor = "multi_thread")]
async fn limit_query_length_and_empty_query() {
    let f = Fixture::new();
    let (id, root) = f.repo("proj");
    for name in ["c.rs", "a.rs", "b.rs"] {
        write(&root.join(name), "");
    }
    let index = f.index();

    let mut p = params(&id, "rs");
    p.limit = Some(1);
    let found = index.search(p).await.unwrap();
    assert_eq!((found.files.len(), found.matched), (1, 3));
    let found = index.search(params(&id, "")).await.unwrap();
    assert_eq!(
        relatives(&found),
        ["a.rs", "b.rs", "c.rs"],
        "빈 질의는 경로순"
    );

    for limit in [0, MAX_LIMIT + 1] {
        let mut p = params(&id, "rs");
        p.limit = Some(limit);
        let e = index.search(p).await.unwrap_err();
        assert_eq!(e.code, pw_rpc::code::INVALID_PARAMS, "limit {limit}");
    }
    let long = "a".repeat(MAX_QUERY_CHARS + 1);
    let e = index.search(params(&id, &long)).await.unwrap_err();
    assert_eq!(e.code, pw_rpc::code::INVALID_PARAMS);
    let e = index.search(params("missing", "a")).await.unwrap_err();
    assert_ne!(e.code, pw_rpc::code::INTERNAL_ERROR);
}

/// 완료 기준: 실제 OS 감시로 파일을 만들거나 지우면 500 ms 안에 결과에 반영된다.
#[tokio::test(flavor = "multi_thread")]
async fn created_and_removed_files_show_up_within_500ms() {
    let f = Fixture::new();
    let (id, root) = f.repo("proj");
    write(&root.join("src/lib.rs"), "");
    let index = f.index();
    let watch = f.watch(&index);
    let _lease = watch
        .acquire(&FsWatchParams {
            project_id: id.clone(),
            worktree: None,
        })
        .unwrap();
    assert!(index.ready(&id, &root).await, "감시를 걸면 목록을 만든다");

    write(&root.join("src/fresh_file.rs"), "");
    let took = until(
        &index,
        &id,
        "fresh_file",
        "src/fresh_file.rs",
        true,
        REFLECTED,
    )
    .await;
    println!("created: {took:?}");
    assert!(took.is_some(), "만든 파일이 {REFLECTED:?} 안에 보인다");

    fs::remove_file(root.join("src/fresh_file.rs")).unwrap();
    let took = until(
        &index,
        &id,
        "fresh_file",
        "src/fresh_file.rs",
        false,
        REFLECTED,
    )
    .await;
    println!("removed: {took:?}");
    assert!(took.is_some(), "지운 파일이 {REFLECTED:?} 안에 빠진다");

    // 폴더째: 만들기(그 아래 파일까지) · 지우기.
    write(&root.join("pkg/deep/inner_file.rs"), "");
    let took = until(
        &index,
        &id,
        "inner_file",
        "pkg/deep/inner_file.rs",
        true,
        REFLECTED,
    )
    .await;
    println!("folder created: {took:?}");
    assert!(took.is_some());
    fs::remove_dir_all(root.join("pkg")).unwrap();
    let took = until(
        &index,
        &id,
        "inner_file",
        "pkg/deep/inner_file.rs",
        false,
        REFLECTED,
    )
    .await;
    println!("folder removed: {took:?}");
    assert!(took.is_some());

    // 이름 바꾸기.
    write(&root.join("src/old_name.rs"), "");
    assert!(
        until(&index, &id, "old_name", "src/old_name.rs", true, REFLECTED)
            .await
            .is_some()
    );
    fs::rename(root.join("src/old_name.rs"), root.join("src/new_name.rs")).unwrap();
    assert!(
        until(&index, &id, "new_name", "src/new_name.rs", true, REFLECTED)
            .await
            .is_some()
    );
    assert!(
        until(&index, &id, "old_name", "src/old_name.rs", false, REFLECTED)
            .await
            .is_some()
    );
}

/// 처음 보는 경로가 `modified` 로 와도 넣는다: 루트 밖에서 옮겨 온 파일, 폴더 사이 이동, 원자 쓰기(임시 파일 → rename).
#[tokio::test(flavor = "multi_thread")]
async fn moved_in_and_atomically_written_files_show_up() {
    let f = Fixture::new();
    let (id, root) = f.repo("proj");
    write(&root.join("src/a/moved_between.rs"), "");
    write(&f.base.join("elsewhere/moved_in.rs"), "");
    let index = f.index();
    let watch = f.watch(&index);
    let _lease = watch
        .acquire(&FsWatchParams {
            project_id: id.clone(),
            worktree: None,
        })
        .unwrap();
    assert!(index.ready(&id, &root).await);

    fs::create_dir_all(root.join("src/b")).unwrap();
    fs::rename(
        root.join("src/a/moved_between.rs"),
        root.join("src/b/moved_between.rs"),
    )
    .unwrap();
    fs::rename(
        f.base.join("elsewhere/moved_in.rs"),
        root.join("src/moved_in.rs"),
    )
    .unwrap();
    write(&root.join("src/.atomic.tmp"), "");
    fs::rename(
        root.join("src/.atomic.tmp"),
        root.join("src/atomic_target.rs"),
    )
    .unwrap();

    for (query, rel) in [
        ("moved_between", "src/b/moved_between.rs"),
        ("moved_in", "src/moved_in.rs"),
        ("atomic_target", "src/atomic_target.rs"),
    ] {
        let took = until(&index, &id, query, rel, true, REFLECTED).await;
        assert!(took.is_some(), "{rel} 가 {REFLECTED:?} 안에 든다");
    }
    let listed = all_files(&index, &id).await;
    assert!(!listed.contains("src/a/moved_between.rs"), "{listed:?}");
    assert!(!listed.contains("src/.atomic.tmp"), "{listed:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn changing_an_ignore_file_rewalks_its_folder() {
    let f = Fixture::new();
    let (id, root) = f.repo("proj");
    write(&root.join("web/keep.js"), "");
    write(&root.join("web/scratch.tmp"), "");
    let index = f.index();
    let watch = f.watch(&index);
    let _lease = watch
        .acquire(&FsWatchParams {
            project_id: id.clone(),
            worktree: None,
        })
        .unwrap();
    assert!(index.ready(&id, &root).await);
    assert!(all_files(&index, &id).await.contains("web/scratch.tmp"));

    write(&root.join("web/.gitignore"), "*.tmp\n");
    let took = until(&index, &id, "scratch", "web/scratch.tmp", false, REFLECTED).await;
    assert!(took.is_some(), "새로 ignore 된 파일이 빠진다");
    assert!(all_files(&index, &id).await.contains("web/keep.js"));

    fs::remove_file(root.join("web/.gitignore")).unwrap();
    let took = until(&index, &id, "scratch", "web/scratch.tmp", true, REFLECTED).await;
    assert!(took.is_some(), "ignore 가 풀리면 다시 든다");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_search_during_the_first_build_waits_for_it() {
    let f = Fixture::new();
    let (id, root) = f.repo("proj");
    write(&root.join("src/lib.rs"), "");
    set_after_walk(&root, || std::thread::sleep(Duration::from_millis(300)));
    let index = f.index();
    index.open(&id, &root, None);

    let found = index.search(params(&id, "lib")).await.unwrap();
    assert_eq!(relatives(&found), ["src/lib.rs"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn changes_during_a_build_are_applied_after_it() {
    let f = Fixture::new();
    let (id, root) = f.repo("proj");
    write(&root.join("src/lib.rs"), "");
    let index = f.index();
    let watch = f.watch(&index);
    // 걷기가 끝난 뒤 · 바꿔 끼우기 전에 파일을 만들고, 그 변경이 버스로 올 때까지 기다린다.
    let late = root.join("src/late_file.rs");
    set_after_walk(&root, move || {
        write(&late, "");
        std::thread::sleep(Duration::from_millis(400));
    });
    let _lease = watch
        .acquire(&FsWatchParams {
            project_id: id.clone(),
            worktree: None,
        })
        .unwrap();
    assert!(index.ready(&id, &root).await);
    assert!(
        all_files(&index, &id).await.contains("src/late_file.rs"),
        "만드는 동안 온 변경이 적용된다"
    );
}

/// `rescan` 이면 처음부터 다시 만든다. 감시 없이 연 목록이라 디스크를 바꿔도 `rescan` 전에는 옛 목록이다.
#[tokio::test(flavor = "multi_thread")]
async fn a_rescan_rebuilds_the_list() {
    let f = Fixture::new();
    let (id, root) = f.repo("proj");
    write(&root.join("src/lib.rs"), "");
    let index = f.index();
    index.open(&id, &root, None);
    assert!(index.ready(&id, &root).await);
    write(&root.join("src/unseen.rs"), "");
    assert!(!all_files(&index, &id).await.contains("src/unseen.rs"));

    let rescan = FsChanged {
        root: root.display().to_string(),
        worktree: None,
        paths: Vec::new(),
        kinds: Vec::new(),
        rescan: true,
    };
    f.bus
        .publish(Scope::Project(id.clone()), TOPIC_CHANGED, &rescan)
        .unwrap();
    let took = until(
        &index,
        &id,
        "unseen",
        "src/unseen.rs",
        true,
        Duration::from_secs(5),
    )
    .await;
    assert!(took.is_some(), "다시 만든 목록에 든다");
}

/// 버스 큐가 넘쳐 변경을 놓치면(`Lagged`) 모든 목록을 다시 만든다.
#[tokio::test(flavor = "current_thread")]
async fn a_lagged_subscription_rebuilds_every_list() {
    let f = Fixture::with_bus(EventBus::new(1));
    let (id, root) = f.repo("proj");
    write(&root.join("src/lib.rs"), "");
    let index = f.index();
    index.open(&id, &root, None);
    assert!(index.ready(&id, &root).await);
    write(&root.join("src/unseen.rs"), "");
    // 이 스레드가 기다리지 않는 동안 둘을 내면 큐(1)가 넘친다. 경로 없는 변경이라 그 자체로는 아무것도 바꾸지 않는다.
    let nothing = FsChanged {
        root: root.display().to_string(),
        worktree: None,
        paths: Vec::new(),
        kinds: Vec::new(),
        rescan: false,
    };
    for _ in 0..2 {
        f.bus
            .publish(Scope::Project(id.clone()), TOPIC_CHANGED, &nothing)
            .unwrap();
    }
    let took = until(
        &index,
        &id,
        "unseen",
        "src/unseen.rs",
        true,
        Duration::from_secs(5),
    )
    .await;
    assert!(took.is_some(), "다시 만든 목록에 든다");
}

#[tokio::test(flavor = "multi_thread")]
async fn releasing_the_watch_drops_the_list() {
    let f = Fixture::new();
    let (id, root) = f.repo("proj");
    write(&root.join("src/lib.rs"), "");
    let index = f.index();
    let watch = f.watch(&index);
    let lease = watch
        .acquire(&FsWatchParams {
            project_id: id.clone(),
            worktree: None,
        })
        .unwrap();
    assert!(index.ready(&id, &root).await);
    drop(lease);
    assert!(
        index.inner.get(&id, &root).is_none(),
        "마지막 임대가 풀리면 버린다"
    );
    assert!(!index.ready(&id, &root).await);

    // 감시하지 않는 루트는 요청마다 걷는다.
    write(&root.join("src/after.rs"), "");
    assert!(all_files(&index, &id).await.contains("src/after.rs"));
}

#[tokio::test(flavor = "multi_thread")]
async fn building_is_a_project_task() {
    let f = Fixture::new();
    let (id, root) = f.repo("proj");
    write(&root.join("src/lib.rs"), "");
    let mut finished = f.bus.subscribe(
        Filter::all()
            .scope(Scope::Project(id.clone()))
            .topics([TOPIC_FINISHED]),
    );
    let index = f.index();
    index.open(&id, &root, None);
    let got = tokio::time::timeout(Duration::from_secs(5), finished.recv())
        .await
        .unwrap();
    let Some(Received::Event(event)) = got else {
        panic!("no task event: {got:?}");
    };
    let event: TaskFinishedEvent = serde_json::from_value(event.payload.clone()).unwrap();
    assert_eq!(event.title, TASK_TITLE);
    assert_eq!(event.status, TaskStatus::Completed);
}

/// 작업을 취소하면(사람이 `task.cancel`) 그 목록을 버리고, 그 만들기를 기다리던 요청은 「취소됨」 으로 끝난다. 감시가 살아 있는
/// 동안 다음 요청이 다시 만든다(감시하지 않는 루트처럼 걷지 않는다).
#[tokio::test(flavor = "multi_thread")]
async fn cancelling_the_task_drops_the_list_until_the_next_request() {
    let f = Fixture::new();
    let (id, root) = f.repo("proj");
    write(&root.join("src/lib.rs"), "");
    set_after_walk(&root, || std::thread::sleep(Duration::from_millis(500)));
    let index = FileIndex::new(Arc::clone(&f.fs));
    let tasks = TaskSpawner::new(f.bus.clone(), Scope::Host, CancellationToken::new());
    tokio::spawn(index.attach(&f.bus, tasks.clone()));
    index.open(&id, &root, None);
    let running = tasks.running(Some(&id));
    assert_eq!(running.len(), 1);
    let searching = tokio::spawn({
        let index = index.clone();
        let id = id.clone();
        async move { index.search(params(&id, "lib")).await }
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(tasks.cancel(running[0].task_id));
    let e = tokio::time::timeout(Duration::from_secs(5), searching)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert_eq!(e.code, pw_rpc::code::CANCELLED, "{e:?}");
    let entry = index
        .inner
        .get(&id, &root)
        .expect("감시가 살아 있으면 자리는 남는다");
    assert!(entry.state().files.is_empty(), "목록을 버린다");
    assert!(!index.ready(&id, &root).await);

    // 다음 요청이 다시 만든다 — 다시 만들기도 작업이다.
    set_after_walk(&root, || {});
    write(&root.join("src/after.rs"), "");
    let found = tokio::time::timeout(Duration::from_secs(5), index.search(params(&id, "after")))
        .await
        .expect("다시 만들기를 기다리다 멈추지 않는다")
        .unwrap();
    assert_eq!(relatives(&found), ["src/after.rs"]);
    assert!(index.ready(&id, &root).await);
    assert_eq!(adhoc_walks(&root), 0, "감시하지 않는 루트처럼 걷지 않는다");
}

/// 취소 뒤 `rescan` 도 다시 만든다(그 전에 온 경로 변경은 버린다).
#[tokio::test(flavor = "multi_thread")]
async fn a_rescan_after_a_cancel_rebuilds_the_list() {
    let f = Fixture::new();
    let (id, root) = f.repo("proj");
    write(&root.join("src/lib.rs"), "");
    set_after_walk(&root, || std::thread::sleep(Duration::from_millis(300)));
    let index = FileIndex::new(Arc::clone(&f.fs));
    let tasks = TaskSpawner::new(f.bus.clone(), Scope::Host, CancellationToken::new());
    tokio::spawn(index.attach(&f.bus, tasks.clone()));
    index.open(&id, &root, None);
    let running = tasks.running(Some(&id));
    assert!(tasks.cancel(running[0].task_id));
    let entry = index.inner.get(&id, &root).unwrap();
    // 취소는 작업의 future 가 버려질 때 닿는다.
    let mut phase = entry.phase.subscribe();
    tokio::time::timeout(
        Duration::from_secs(5),
        phase.wait_for(|p| *p == Phase::Cancelled),
    )
    .await
    .unwrap()
    .unwrap();
    // 버린 목록에는 경로 변경을 적용하지 않는다(일부만 든 목록이 되지 않게).
    entry.changed(vec![root.join("src/lib.rs")]);
    assert!(entry.state().files.is_empty());
    set_after_walk(&root, || {});

    let rescan = FsChanged {
        root: root.display().to_string(),
        worktree: None,
        paths: Vec::new(),
        kinds: Vec::new(),
        rescan: true,
    };
    f.bus
        .publish(Scope::Project(id.clone()), TOPIC_CHANGED, &rescan)
        .unwrap();
    let rebuilt = tokio::time::timeout(Duration::from_secs(5), async {
        while !index.ready(&id, &root).await {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    assert!(rebuilt.is_ok(), "rescan 이 다시 만든다");
    assert!(all_files(&index, &id).await.contains("src/lib.rs"));
}

/// 결정 M1-D8: 감시가 알린 경로를 다시 볼 때 중간 폴더가 루트 밖 링크로 바뀌었으면 넣지 않는다(리뷰 B1 의 결정적 재현).
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn a_folder_swapped_to_an_outside_link_adds_nothing() {
    let f = Fixture::new();
    let (_id, root) = f.repo("proj");
    write(&root.join("d/x/inner.rs"), "");
    let outside = f.base.join("outside");
    write(&outside.join("x/secret_outside.rs"), "");
    write(&outside.join("top_secret.rs"), "");
    fs::remove_dir_all(root.join("d")).unwrap();
    symlink(&outside, &root.join("d"));

    let scope = Roots::new([root.clone()]);
    let stop = std::sync::atomic::AtomicBool::new(false);
    let walk = Walk::new(&root, &scope, &stop);
    let ops = walk.ops(&[root.join("d/x"), root.join("d/top_secret.rs")]);
    let inserted: Vec<_> = ops
        .iter()
        .filter_map(|op| match op {
            walk::Op::Insert(rel, _) => Some(rel.to_string()),
            walk::Op::Remove(_) => None,
        })
        .collect();
    assert_eq!(inserted, Vec::<String>::new(), "{ops:?}");
}

/// 루트를 범위 안의 다른 루트로 가는 링크로 바꿔치기해도 그 루트의 이름을 이 루트 것으로 걷지 않는다. 대상이 범위 안이라 링크
/// 판정으로는 거를 수 없고, 루트를 연 때의 실제 경로와 견준다.
#[cfg(unix)]
#[test]
fn a_root_swapped_to_another_root_walks_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let base = simplify(fs::canonicalize(tmp.path()).unwrap());
    let (a, b) = (base.join("a"), base.join("b"));
    write(&a.join("mine.rs"), "");
    write(&b.join("other_root.rs"), "");
    let scope = Roots::new([a.clone(), b.clone()]);
    let stop = std::sync::atomic::AtomicBool::new(false);
    let walk = Walk::new(&a, &scope, &stop);
    assert_eq!(walk.all().len(), 1);

    fs::remove_dir_all(&a).unwrap();
    symlink(&b, &a);
    assert_eq!(walk.all(), Vec::new());
}

/// 같은 것을 실제 OS 감시로: 폴더를 지우고 같은 이름의 루트 밖 링크를 두면 밖의 이름이 목록에 들지 않는다.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn swapping_a_folder_for_an_outside_link_leaks_no_names() {
    let f = Fixture::new();
    let (id, root) = f.repo("proj");
    write(&root.join("d/x/inner.rs"), "");
    let outside = f.base.join("outside");
    write(&outside.join("x/secret_outside.rs"), "");
    let index = f.index();
    let watch = f.watch(&index);
    let _lease = watch
        .acquire(&FsWatchParams {
            project_id: id.clone(),
            worktree: None,
        })
        .unwrap();
    assert!(index.ready(&id, &root).await);
    assert!(all_files(&index, &id).await.contains("d/x/inner.rs"));

    fs::remove_dir_all(root.join("d")).unwrap();
    symlink(&outside, &root.join("d"));
    let took = until(&index, &id, "inner", "d/x/inner.rs", false, REFLECTED).await;
    assert!(took.is_some(), "지운 폴더의 항목이 빠진다");
    // 알림이 몇 번에 나눠 와도 밖의 이름은 들지 않는다.
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(1) {
        let listed = all_files(&index, &id).await;
        assert!(
            listed.iter().all(|p| !p.contains("secret")),
            "루트 밖 이름: {listed:?}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// 루트 자체를 지우고 같은 이름의 루트 밖 링크로 바꾸면(리뷰 L7) 다시 만든 목록 · 감시하지 않는 걷기 모두 비어 있다.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn a_root_swapped_to_an_outside_link_lists_nothing() {
    let f = Fixture::new();
    let (id, root) = f.repo("proj");
    write(&root.join("src/lib.rs"), "");
    let outside = f.base.join("outside");
    write(&outside.join("root_swap_secret.rs"), "");
    let index = f.index();
    index.open(&id, &root, None);
    assert!(index.ready(&id, &root).await);

    fs::remove_dir_all(&root).unwrap();
    symlink(&outside, &root);
    let entry = index.inner.get(&id, &root).unwrap();
    // 걷기 자체가 거른다(요청이 루트를 빼는 것과 따로).
    let stop = std::sync::atomic::AtomicBool::new(false);
    assert_eq!(Walk::new(&root, &entry.scope, &stop).all(), Vec::new());
    index.rebuild(&entry);
    let rebuilt = tokio::time::timeout(Duration::from_secs(5), async {
        while entry.state().build.is_some() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    assert!(rebuilt.is_ok());
    assert_eq!(
        all_files(&index, &id).await,
        BTreeSet::new(),
        "다시 만든 목록"
    );

    index.close(&id, &root);
    assert_eq!(
        all_files(&index, &id).await,
        BTreeSet::new(),
        "감시하지 않는 걷기"
    );
}

/// 대소문자만 바꾼 이름 바꾸기(리뷰 M4): 대소문자를 가리지 않는 볼륨에서도 옛 이름이 남지 않는다.
#[tokio::test(flavor = "multi_thread")]
async fn renaming_only_the_case_leaves_no_old_name() {
    let f = Fixture::new();
    let (id, root) = f.repo("proj");
    write(&root.join("src/Foo.rs"), "");
    write(&root.join("Pkg/a.rs"), "");
    let index = f.index();
    let watch = f.watch(&index);
    let _lease = watch
        .acquire(&FsWatchParams {
            project_id: id.clone(),
            worktree: None,
        })
        .unwrap();
    assert!(index.ready(&id, &root).await);

    fs::rename(root.join("src/Foo.rs"), root.join("src/foo.rs")).unwrap();
    fs::rename(root.join("Pkg"), root.join("pkg")).unwrap();
    let want: BTreeSet<String> = ["pkg/a.rs", "src/foo.rs"]
        .into_iter()
        .map(String::from)
        .collect();
    let started = Instant::now();
    let mut listed = all_files(&index, &id).await;
    while listed != want && started.elapsed() < REFLECTED * 2 {
        tokio::time::sleep(Duration::from_millis(10)).await;
        listed = all_files(&index, &id).await;
    }
    assert_eq!(listed, want);

    // 결정적으로: 옛 이름 경로를 다시 봐도 넣지 않는다.
    let scope = Roots::new([root.clone()]);
    let stop = std::sync::atomic::AtomicBool::new(false);
    let walk = Walk::new(&root, &scope, &stop);
    let ops = walk.ops(&[
        root.join("Pkg"),
        root.join("src/Foo.rs"),
        root.join("Pkg/a.rs"),
    ]);
    assert!(
        ops.iter().all(|op| matches!(op, walk::Op::Remove(_))),
        "{ops:?}"
    );
}

/// 폴더를 만들면 그 폴더 · 그 아래가 한 묶음에 함께 온다(리뷰 L6). 가장 바깥 것만 걸어 같은 항목을 한 번 넣는다.
#[test]
fn a_batch_walks_only_the_outermost_folder() {
    let tmp = tempfile::tempdir().unwrap();
    let root = simplify(fs::canonicalize(tmp.path()).unwrap());
    write(&root.join("newdir/inner/c.rs"), "");
    write(&root.join("other/d.rs"), "");
    let scope = Roots::new([root.clone()]);
    let stop = std::sync::atomic::AtomicBool::new(false);
    let walk = Walk::new(&root, &scope, &stop);
    let ops = walk.ops(&[
        root.join("newdir/inner/c.rs"),
        root.join("newdir"),
        root.join("newdir/inner"),
        root.join("other/d.rs"),
    ]);
    assert_eq!(
        ops,
        [
            walk::Op::Remove("newdir".into()),
            walk::Op::Remove("other/d.rs".into()),
            walk::Op::Insert("other/d.rs".into(), Flags::default()),
            walk::Op::Insert("newdir/inner/c.rs".into(), Flags::default()),
        ]
    );
}

/// 만드는 동안 모은 경로가 상한을 넘으면 모으지 않고 다시 걷게 한다(리뷰 L6).
#[tokio::test(flavor = "multi_thread")]
async fn too_many_changes_during_a_build_fold_into_a_rescan() {
    let f = Fixture::new();
    let (id, root) = f.repo("proj");
    write(&root.join("src/lib.rs"), "");
    set_after_walk(&root, || std::thread::sleep(Duration::from_millis(300)));
    let index = f.index();
    index.open(&id, &root, None);
    let entry = index.inner.get(&id, &root).unwrap();
    entry.changed(vec![root.join("src/a.rs"); MAX_PENDING - 1]);
    assert_eq!(entry.state().pending.len(), MAX_PENDING - 1);
    entry.changed(vec![root.join("src/b.rs"); 2]);
    {
        let state = entry.state();
        assert!(state.rescan);
        assert!(state.pending.is_empty(), "넘치면 버린다");
    }
    set_after_walk(&root, || {});
    assert!(index.ready(&id, &root).await);
}

/// 감시하지 않는 루트(리뷰 H2): 함께 온 요청은 한 번 걸은 목록을 나눠 쓰고, [`ADHOC_KEEP`] 이 지나면 다시 걷는다.
#[tokio::test(flavor = "multi_thread")]
async fn unwatched_requests_share_one_walk_for_a_while() {
    let f = Fixture::new();
    let (id, root) = f.repo("proj");
    write(&root.join("src/lib.rs"), "");
    let index = f.index();

    let searches: Vec<_> = (0..8)
        .map(|_| {
            let index = index.clone();
            let id = id.clone();
            tokio::spawn(async move { index.search(params(&id, "lib")).await })
        })
        .collect();
    for search in searches {
        assert_eq!(relatives(&search.await.unwrap().unwrap()), ["src/lib.rs"]);
    }
    assert_eq!(adhoc_walks(&root), 1, "동시에 온 요청도 걷기는 한 번");

    write(&root.join("src/later.rs"), "");
    assert!(!all_files(&index, &id).await.contains("src/later.rs"));
    assert_eq!(adhoc_walks(&root), 1);
    tokio::time::sleep(ADHOC_KEEP + Duration::from_millis(100)).await;
    assert!(index.inner.adhoc().is_empty(), "시간이 지나면 놓는다");
    assert!(all_files(&index, &id).await.contains("src/later.rs"));
    assert_eq!(adhoc_walks(&root), 2);
}

/// 걷기 · 매칭은 Host 전체에서 [`MAX_SEARCHES`] 개까지다(리뷰 H2 · M3). 차례가 [`SEARCH_WAIT`] 안에 오지 않으면 「바쁨」.
#[tokio::test(flavor = "multi_thread")]
async fn walks_and_matches_wait_for_a_turn_then_report_busy() {
    let f = Fixture::new();
    let (id, root) = f.repo("proj");
    write(&root.join("src/lib.rs"), "");
    let (watched_id, watched_root) = f.repo("watched");
    write(&watched_root.join("src/main.rs"), "");
    let index = f.index();
    index.open(&watched_id, &watched_root, None);
    assert!(index.ready(&watched_id, &watched_root).await);

    let held = index
        .inner
        .searches
        .acquire_many(MAX_SEARCHES as u32)
        .await
        .unwrap();
    // 감시하지 않는 루트의 걷기.
    let e = index.search(params(&id, "lib")).await.unwrap_err();
    assert!(format!("{e:?}").contains("busy"), "{e:?}");
    assert_eq!(adhoc_walks(&root), 0);
    // 감시하는 루트의 매칭.
    let started = Instant::now();
    let e = index.search(params(&watched_id, "main")).await.unwrap_err();
    assert!(format!("{e:?}").contains("busy"), "{e:?}");
    assert!(started.elapsed() >= SEARCH_WAIT);

    // 기다리는 사이 차례가 나면 답한다.
    let waiting = tokio::spawn({
        let index = index.clone();
        let id = watched_id.clone();
        async move { index.search(params(&id, "main")).await }
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    drop(held);
    let found = waiting.await.unwrap().unwrap();
    assert_eq!(relatives(&found), ["src/main.rs"]);
}
