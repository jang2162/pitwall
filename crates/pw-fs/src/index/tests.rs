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
        root.join("src/pw-lib/lib.rs").display().to_string()
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

/// 작업을 취소하면(사람이 `task.cancel`) 그 목록을 버리고, 기다리던 요청은 직접 걸어 답한다.
#[tokio::test(flavor = "multi_thread")]
async fn cancelling_the_task_drops_the_list() {
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
    assert!(tasks.cancel(running[0].task_id));
    let found = tokio::time::timeout(Duration::from_secs(5), searching)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(relatives(&found), ["src/lib.rs"]);
    assert!(index.inner.get(&id, &root).is_none());
}
