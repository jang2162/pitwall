//! 색인의 수명 · 갱신 · 취소 · 질의. 실제 파일 목록 인덱스(`pw_fs::index`)와 파일 감시를 붙여, 창이 `fs.watch` 를 연 것처럼 임대를
//! 잡고 돈다. 심볼 표 스냅숏(완료 기준 ①)은 `tests/symbols.rs`, 성능(④)은 `tests/perf.rs`.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use pw_core::TaskSpawner;
use pw_core::bus::{DEFAULT_CAPACITY, EventBus, Scope};
use pw_core::config::AppPaths;
use pw_core::registry::path::simplify;
use pw_core::registry::{NewProject, Registry as Projects};
use pw_fs::index::FileIndex;
use pw_fs::watch::{FsChangeKind, FsChanged, FsWatchParams, Lease, TOPIC_CHANGED, Watch};
use semver::Version;

use super::*;

type Hook = Box<dyn Fn() + Send>;

/// 루트별로 파일 하나를 파싱하기 전에 부를 것(시험 프로세스 안의 다른 시험과 섞이지 않게 루트로 가른다).
static BEFORE_PARSE: Mutex<Option<HashMap<PathBuf, Hook>>> = Mutex::new(None);
/// 루트별로 목록 사건 하나를 적용하기 전에 부를 것(관찰자 쪽이 느린 경우).
static BEFORE_EVENT: Mutex<Option<HashMap<PathBuf, Hook>>> = Mutex::new(None);
/// (루트, 상대 경로)별 파싱(읽기) 횟수.
static PARSED: Mutex<Option<HashMap<(PathBuf, String), usize>>> = Mutex::new(None);

pub(crate) fn before_parse(root: &Path, _rel: &str) {
    let hooks = BEFORE_PARSE.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(hook) = hooks.as_ref().and_then(|h| h.get(root)) {
        hook();
    }
}

pub(crate) fn before_event(root: &Path) {
    let hooks = BEFORE_EVENT.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(hook) = hooks.as_ref().and_then(|h| h.get(root)) {
        hook();
    }
}

fn set_before_event(root: &Path, hook: impl Fn() + Send + 'static) {
    BEFORE_EVENT
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get_or_insert_with(HashMap::new)
        .insert(root.to_path_buf(), Box::new(hook));
}

pub(crate) fn parsed(root: &Path, rel: &str) {
    *PARSED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get_or_insert_with(HashMap::new)
        .entry((root.to_path_buf(), rel.to_owned()))
        .or_default() += 1;
}

fn parses(root: &Path, rel: &str) -> usize {
    PARSED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .as_ref()
        .and_then(|p| p.get(&(root.to_path_buf(), rel.to_owned())).copied())
        .unwrap_or(0)
}

fn set_before_parse(root: &Path, hook: impl Fn() + Send + 'static) {
    BEFORE_PARSE
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get_or_insert_with(HashMap::new)
        .insert(root.to_path_buf(), Box::new(hook));
}

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

/// 창 없이 Host 의 부품만: 레지스트리 · 파일 서비스 · 파일 목록 인덱스 · 감시 · 심볼 인덱스 · 작업.
struct Fixture {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    projects: Projects,
    bus: EventBus,
    tasks: TaskSpawner,
    watch: Watch,
    files: FileIndex,
    symbols: SymbolIndex,
}

impl Fixture {
    /// tokio 런타임 안에서 부른다.
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let base = simplify(fs::canonicalize(tmp.path()).unwrap());
        let bus = EventBus::new(DEFAULT_CAPACITY);
        let projects = Projects::new(AppPaths::under(&base.join("app")), None, bus.clone());
        let fs = Arc::new(Fs::new());
        fs.attach(projects.clone());
        let registry = Registry::from_roots(&Version::new(0, 0, 0), pw_ext::builtin::roots());
        let symbols = SymbolIndex::new(Arc::clone(&fs), Arc::new(registry));
        let tasks = TaskSpawner::new(bus.clone(), Scope::Host, CancellationToken::new());
        let files = FileIndex::new(Arc::clone(&fs));
        files.observe(symbols.observer());
        tokio::spawn(files.attach(&bus, tasks.clone()));
        symbols.attach(tasks.clone());
        let watch = Watch::new(fs);
        watch.attach(bus.clone());
        watch.attach_index(files.clone());
        Self {
            _tmp: tmp,
            base,
            projects,
            bus,
            tasks,
            watch,
            files,
            symbols,
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

    /// 창이 `fs.watch` 를 연 것처럼 감시를 잡는다(파일 목록 → 심볼 색인이 만들어지기 시작한다).
    fn open(&self, id: &str) -> Lease {
        self.watch
            .acquire(&FsWatchParams {
                project_id: id.to_owned(),
                worktree: None,
            })
            .unwrap()
    }

    /// 감시가 알린 것처럼 `fs.changed` 를 낸다(실제 감시의 알림보다 먼저 닿게 — 실제 알림이 뒤에 와도 결과는 같다).
    fn changed(&self, id: &str, root: &Path, paths: &[&Path]) {
        let event = FsChanged {
            root: root.display().to_string(),
            worktree: None,
            paths: paths.iter().map(|p| p.display().to_string()).collect(),
            kinds: paths.iter().map(|_| FsChangeKind::Modified).collect(),
            rescan: false,
        };
        self.bus
            .publish(Scope::Project(id.to_owned()), TOPIC_CHANGED, &event)
            .unwrap();
    }

    async fn ready(&self, id: &str, root: &Path) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !self.symbols.ready(id, root).await {
            assert!(
                Instant::now() < deadline,
                "the symbol index never became ready"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(self.symbols.idle(id, root).await);
    }

    async fn search(&self, id: &str, query: &str) -> Result<IndexSymbolSearch, RpcError> {
        self.symbols
            .search(IndexSymbolSearchParams {
                project_id: id.to_owned(),
                query: query.to_owned(),
                ..IndexSymbolSearchParams::default()
            })
            .await
    }

    /// 프로젝트 범위에서 도는 심볼 색인 작업들.
    fn index_tasks(&self, id: &str) -> Vec<pw_core::task::TaskInfo> {
        self.tasks
            .running(Some(id))
            .into_iter()
            .filter(|t| t.title == TASK_TITLE)
            .collect()
    }
}

fn names(found: &IndexSymbolSearch) -> Vec<(String, String)> {
    found
        .symbols
        .iter()
        .map(|s| (s.name.clone(), s.relative.clone()))
        .collect()
}

/// 기다리던 결과가 나올 때까지(많아야 5초) 질의를 되풀이한다.
async fn eventually(
    f: &Fixture,
    id: &str,
    query: &str,
    want: impl Fn(&IndexSymbolSearch) -> bool,
) -> IndexSymbolSearch {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let found = f.search(id, query).await.unwrap();
        if want(&found) {
            return found;
        }
        assert!(
            Instant::now() < deadline,
            "never matched: {:?}",
            names(&found)
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// 감시를 잡으면 색인이 만들어지고(작업 하나), 이름으로 퍼지 매칭된다. 벤더 · 생성 파일과 정의 쿼리가 없는 언어는 빠진다.
#[tokio::test(flavor = "multi_thread")]
async fn watching_a_project_indexes_its_symbols() {
    let f = Fixture::new();
    let (id, root) = f.repo("proj");
    write(
        &root.join("src/lib.rs"),
        "pub struct Widget;\nimpl Widget { pub fn render(&self) {} }\n",
    );
    write(&root.join("web/app.ts"), "export function renderApp() {}\n");
    write(&root.join("vendor/dep.rs"), "pub fn render_vendored() {}\n");
    write(&root.join("gen/out.rs"), "pub fn render_generated() {}\n");
    write(&root.join(".gitattributes"), "gen/** linguist-generated\n");
    write(&root.join("notes.md"), "# render\n");
    let _lease = f.open(&id);
    f.ready(&id, &root).await;

    let found = f.search(&id, "render").await.unwrap();
    assert_eq!(
        names(&found),
        [
            ("render".to_owned(), "src/lib.rs".to_owned()),
            ("renderApp".to_owned(), "web/app.ts".to_owned())
        ]
    );
    let render = &found.symbols[0];
    assert_eq!(render.kind, IndexSymbolKind::Method);
    assert_eq!(render.container.as_deref(), Some("Widget"));
    assert_eq!(render.language, "rust");
    // 경로는 OS 의 구분자로 온다(Windows 는 `\\`) — `join("src/lib.rs")` 는 Windows 에서 `/` 를 그대로 둔다.
    assert_eq!(
        render.path,
        root.join("src").join("lib.rs").display().to_string()
    );
    assert_eq!(render.selection_range.start.line, 1);
    assert_eq!(render.positions, [0, 1, 2, 3, 4, 5]);
    assert!(found.unindexed.is_empty());

    let status = f
        .symbols
        .status(IndexStatusParams {
            project_id: id.clone(),
            worktree: None,
        })
        .await
        .unwrap();
    assert_eq!(status.roots.len(), 1);
    assert_eq!(status.roots[0].state, IndexRootState::Ready);
    assert_eq!(
        status.roots[0].files, 2,
        "lib.rs · app.ts — 벤더 · 생성 · 마크다운은 빠진다"
    );
}

/// 색인이 준비되면 후보 버퍼가 이미 심볼 수만큼 잡혀 있다 — 창을 연 뒤 첫 질의가 그 페이지를 처음 만지는 비용을 내지 않는다
/// (search.rs 머리 주석). 질의 뒤에도 그 버퍼가 쉬는 자리로 돌아온다.
#[tokio::test(flavor = "multi_thread")]
async fn the_candidate_buffer_is_warm_when_the_index_is_ready() {
    let f = Fixture::new();
    let (id, root) = f.repo("proj");
    let mut text = String::new();
    for i in 0..40 {
        text.push_str(&format!("pub fn warm_{i:02}() {{}}\n"));
    }
    write(&root.join("src/lib.rs"), &text);
    let _lease = f.open(&id);
    f.ready(&id, &root).await;
    let scratches = &f.symbols.inner.scratches;
    assert!(
        scratches.resting_capacity() >= 40,
        "ready before the buffer was filled: {}",
        scratches.resting_capacity()
    );
    assert_eq!(f.search(&id, "warm").await.unwrap().matched, 40);
    assert!(scratches.resting_capacity() >= 40);
}

/// 점수 · 이름 길이가 같으면 경로 바이트 순서, 같은 파일 안에서는 위치 순서다. 순서는 파일 번호로 견주므로(search.rs `order`) 파일을
/// 쓴 차례 · 폴더 깊이와 무관하게 경로 문자열 순서와 같아야 한다 — 대문자(`C`)는 소문자보다 앞, `a/z` 는 `a_` 보다 앞(`/` < `_`).
#[tokio::test(flavor = "multi_thread")]
async fn ties_follow_the_path_byte_order_then_the_position() {
    let f = Fixture::new();
    let (id, root) = f.repo("proj");
    let files = [
        "src/b.rs",
        "src/a_x.rs",
        "src/a/z.rs",
        "src/C.rs",
        "src/a/b/c.rs",
    ];
    for rel in files {
        write(
            &root.join(rel),
            "pub fn tied_one() {}
pub fn tied_two() {}
",
        );
    }
    let _lease = f.open(&id);
    f.ready(&id, &root).await;
    let found = f.search(&id, "tied").await.unwrap();
    let mut want: Vec<&str> = files.to_vec();
    want.sort_unstable();
    let got: Vec<(String, String)> = names(&found);
    let expected: Vec<(String, String)> = want
        .iter()
        .flat_map(|rel| ["tied_one", "tied_two"].map(|n| (n.to_owned(), (*rel).to_owned())))
        .collect();
    // 이름 길이가 같고 점수가 같다 — 경로 순, 그 안에서 위치 순.
    assert_eq!(got, expected);
}

/// 쪽 넘기기 · 종류 거르기 · 빈 질의 · impl 은 고르지 않으면 빠진다. 입력 상한.
#[tokio::test(flavor = "multi_thread")]
async fn search_pages_filters_kinds_and_checks_its_input() {
    let f = Fixture::new();
    let (id, root) = f.repo("proj");
    let mut text = String::from("pub struct Item;\nimpl Item {}\n");
    for i in 0..30 {
        text.push_str(&format!("pub fn item_{i:02}() {{}}\n"));
    }
    write(&root.join("src/lib.rs"), &text);
    let _lease = f.open(&id);
    f.ready(&id, &root).await;

    let all = f.search(&id, "item").await.unwrap();
    assert_eq!(all.matched, 31, "impl 은 빠진다");
    assert_eq!(all.indexed, 31);
    let top: Vec<_> = all
        .symbols
        .iter()
        .take(3)
        .map(|s| s.name.as_str())
        .collect();
    assert_eq!(
        top,
        ["item_00", "item_01", "item_02"],
        "같은 점수는 짧은 이름 · 경로 · 위치 순"
    );
    // 대문자가 있으면 대소문자를 가린다.
    let exact = f.search(&id, "Item").await.unwrap();
    assert_eq!(
        names(&exact),
        [("Item".to_owned(), "src/lib.rs".to_owned())]
    );

    let page = |offset, limit| IndexSymbolSearchParams {
        project_id: id.clone(),
        query: "item".to_owned(),
        offset: Some(offset),
        limit: Some(limit),
        ..IndexSymbolSearchParams::default()
    };
    let first = f.symbols.search(page(0, 10)).await.unwrap();
    let second = f.symbols.search(page(10, 10)).await.unwrap();
    assert_eq!(first.symbols.len(), 10);
    assert_eq!(second.symbols.len(), 10);
    assert_eq!(first.matched, 31);
    let joined: Vec<_> = first
        .symbols
        .iter()
        .chain(&second.symbols)
        .map(|s| s.name.clone())
        .collect();
    let whole: Vec<_> = all
        .symbols
        .iter()
        .take(20)
        .map(|s| s.name.clone())
        .collect();
    assert_eq!(joined, whole, "쪽을 이으면 한 번에 받은 것과 같다");

    let types = f
        .symbols
        .search(IndexSymbolSearchParams {
            project_id: id.clone(),
            query: String::new(),
            kinds: Some(vec![IndexSymbolKind::Struct, IndexSymbolKind::Impl]),
            ..IndexSymbolSearchParams::default()
        })
        .await
        .unwrap();
    let kinds: Vec<_> = types.symbols.iter().map(|s| s.kind).collect();
    assert_eq!(kinds, [IndexSymbolKind::Struct, IndexSymbolKind::Impl]);

    for bad in [
        IndexSymbolSearchParams {
            limit: Some(0),
            ..page(0, 1)
        },
        IndexSymbolSearchParams {
            limit: Some(MAX_LIMIT + 1),
            ..page(0, 1)
        },
        IndexSymbolSearchParams {
            offset: Some(MAX_OFFSET + 1),
            ..page(0, 1)
        },
        IndexSymbolSearchParams {
            query: "x".repeat(MAX_QUERY_CHARS + 1),
            ..page(0, 1)
        },
    ] {
        let e = f.symbols.search(bad).await.unwrap_err();
        assert_eq!(e.code, code::INVALID_PARAMS, "{e:?}");
    }
}

/// 완료 기준 ②: `fs.changed` 로 파일 하나를 고치면 그 파일의 심볼만 바뀐다 — 다른 파일은 다시 읽지 않고 같은 심볼(같은 할당)을 쓴다.
#[tokio::test(flavor = "multi_thread")]
async fn a_changed_file_reparses_only_that_file() {
    let f = Fixture::new();
    let (id, root) = f.repo("proj");
    let a = root.join("src/a.rs");
    let b = root.join("src/b.rs");
    write(&a, "pub fn alpha() {}\n");
    write(&b, "pub fn beta() {}\n");
    let _lease = f.open(&id);
    f.ready(&id, &root).await;
    assert_eq!(parses(&root, "src/a.rs"), 1);
    assert_eq!(parses(&root, "src/b.rs"), 1);
    let entry = f.symbols.inner.get(&id, &root).unwrap();
    let before_b = entry
        .state()
        .files
        .get("src/b.rs")
        .cloned()
        .flatten()
        .unwrap();

    // 크기가 달라지게 고친다(같은 초 안의 수정 시각만으로는 갈리지 않을 수 있다).
    write(&a, "pub fn alpha() {}\npub fn gamma_added() {}\n");
    f.changed(&id, &root, &[&a]);
    eventually(&f, &id, "gamma", |r| {
        r.symbols.iter().any(|s| s.name == "gamma_added")
    })
    .await;
    assert!(f.symbols.idle(&id, &root).await);

    let after_b = entry
        .state()
        .files
        .get("src/b.rs")
        .cloned()
        .flatten()
        .unwrap();
    assert!(
        Arc::ptr_eq(&before_b, &after_b),
        "고치지 않은 파일의 심볼은 그대로다"
    );
    assert_eq!(
        parses(&root, "src/b.rs"),
        1,
        "고치지 않은 파일은 다시 읽지 않는다"
    );
    assert!(parses(&root, "src/a.rs") >= 2);
    let alpha = f.search(&id, "alpha").await.unwrap();
    assert_eq!(names(&alpha), [("alpha".to_owned(), "src/a.rs".to_owned())]);

    // 지우면 그 파일의 심볼이 빠진다.
    fs::remove_file(&a).unwrap();
    f.changed(&id, &root, &[&a]);
    eventually(&f, &id, "alpha", |r| r.symbols.is_empty()).await;
    let beta = f.search(&id, "beta").await.unwrap();
    assert_eq!(names(&beta), [("beta".to_owned(), "src/b.rs".to_owned())]);
}

/// 완료 기준 ③: 미신뢰 프로젝트에서도 색인이 된다(파싱만 하고 프로젝트 코드를 돌리지 않는다 — 결정 M4-D8).
#[tokio::test(flavor = "multi_thread")]
async fn an_untrusted_project_is_indexed() {
    let f = Fixture::new();
    let (id, root) = f.repo("cloned");
    f.projects.set_trust(&id, false).unwrap();
    assert!(!f.projects.is_trusted(&id));
    write(
        &root.join("build.rs"),
        "fn main() { std::process::exit(1) }\n",
    );
    write(&root.join("src/main.rs"), "fn untrusted_entry() {}\n");
    let _lease = f.open(&id);
    f.ready(&id, &root).await;
    let found = f.search(&id, "untrusted").await.unwrap();
    assert_eq!(
        names(&found),
        [("untrusted_entry".to_owned(), "src/main.rs".to_owned())]
    );
}

/// 완료 기준 ⑤: 만드는 작업을 취소하면 색인을 버리고, 그 만들기를 기다리던 요청은 `CANCELLED` 로 끝난다. 감시가 살아 있으면
/// 다음 요청이 다시 만든다(그 사이의 파일 변경도 든다).
#[tokio::test(flavor = "multi_thread")]
async fn cancelling_the_task_drops_the_index_and_the_waiting_request_is_cancelled() {
    let f = Fixture::new();
    let (id, root) = f.repo("proj");
    for i in 0..20 {
        write(
            &root.join(format!("src/m{i}.rs")),
            &format!("pub fn func_{i}() {{}}\n"),
        );
    }
    set_before_parse(&root, || std::thread::sleep(Duration::from_millis(50)));
    let _lease = f.open(&id);
    let deadline = Instant::now() + Duration::from_secs(10);
    let task = loop {
        if let Some(task) = f.index_tasks(&id).pop() {
            break task;
        }
        assert!(Instant::now() < deadline, "the indexing task never started");
        tokio::time::sleep(Duration::from_millis(5)).await;
    };
    let searching = tokio::spawn({
        let symbols = f.symbols.clone();
        let id = id.clone();
        async move {
            symbols
                .search(IndexSymbolSearchParams {
                    project_id: id,
                    query: "func".to_owned(),
                    ..IndexSymbolSearchParams::default()
                })
                .await
        }
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(f.tasks.cancel(task.task_id));
    let e = tokio::time::timeout(Duration::from_secs(5), searching)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert_eq!(e.code, code::CANCELLED, "{e:?}");
    let entry = f
        .symbols
        .inner
        .get(&id, &root)
        .expect("감시가 살아 있으면 자리는 남는다");
    assert!(
        entry.state().files.values().all(Option::is_none),
        "심볼을 버린다"
    );
    assert!(!f.symbols.ready(&id, &root).await);
    let status = f
        .symbols
        .status(IndexStatusParams {
            project_id: id.clone(),
            worktree: None,
        })
        .await
        .unwrap();
    assert_eq!(status.roots[0].state, IndexRootState::Cancelled);
    assert_eq!(status.roots[0].symbols, 0);

    // 취소된 동안의 파일 추가도 다음 만들기에 든다.
    set_before_parse(&root, || {});
    let added = root.join("src/after.rs");
    write(&added, "pub fn func_after() {}\n");
    f.changed(&id, &root, &[&added]);
    let deadline = Instant::now() + Duration::from_secs(5);
    while !entry.state().files.contains_key("src/after.rs") {
        assert!(
            Instant::now() < deadline,
            "the file list never got the new file"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let found = tokio::time::timeout(Duration::from_secs(10), f.search(&id, "func"))
        .await
        .expect("다시 만들기를 기다리다 멈추지 않는다")
        .unwrap();
    assert_eq!(found.matched, 21);
    assert!(f.symbols.ready(&id, &root).await);
}

/// 수명: 마지막 감시가 풀리면 색인을 버린다(파일 목록 인덱스와 같다). 감시하지 않는 루트는 찾지 않고 `unindexed` 로 알린다.
#[tokio::test(flavor = "multi_thread")]
async fn releasing_the_last_watch_drops_the_index() {
    let f = Fixture::new();
    let (id, root) = f.repo("proj");
    write(&root.join("src/lib.rs"), "pub fn kept() {}\n");
    let first = f.open(&id);
    let second = f.open(&id);
    f.ready(&id, &root).await;
    drop(first);
    assert_eq!(
        f.search(&id, "kept").await.unwrap().symbols.len(),
        1,
        "창 하나가 남았다"
    );
    drop(second);
    let deadline = Instant::now() + Duration::from_secs(5);
    while f.symbols.inner.get(&id, &root).is_some() {
        assert!(Instant::now() < deadline, "the index was never dropped");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let found = f.search(&id, "kept").await.unwrap();
    assert!(found.symbols.is_empty());
    assert_eq!(found.unindexed, [root.display().to_string()]);
    let status = f
        .symbols
        .status(IndexStatusParams {
            project_id: id.clone(),
            worktree: None,
        })
        .await
        .unwrap();
    assert_eq!(status.roots[0].state, IndexRootState::None);
}

/// `index.file.symbols`: 색인에 있는 것 · 저장하지 않은 텍스트 · 감시하지 않는 루트의 디스크 파일. 프로젝트 밖 경로는 거절한다.
#[tokio::test(flavor = "multi_thread")]
async fn file_symbols_come_from_the_index_the_text_or_the_disk() {
    let f = Fixture::new();
    let (id, root) = f.repo("proj");
    let lib = root.join("src/lib.rs");
    write(
        &lib,
        "pub struct Outer;\nimpl Outer {\n    pub fn inner(&self) {}\n}\n",
    );
    let params = |text: Option<&str>| IndexFileSymbolsParams {
        project_id: id.clone(),
        worktree: None,
        path: lib.display().to_string(),
        text: text.map(str::to_owned),
    };

    // 감시하지 않는 루트: 디스크에서 읽어 파싱한다.
    let disk = f.symbols.file_symbols(params(None)).await.unwrap();
    assert_eq!(disk.language.as_deref(), Some("rust"));
    let tree: Vec<_> = disk
        .symbols
        .iter()
        .map(|s| (s.name.as_str(), s.kind, s.parent))
        .collect();
    assert_eq!(
        tree,
        [
            ("Outer", IndexSymbolKind::Struct, None),
            ("Outer", IndexSymbolKind::Impl, None),
            ("inner", IndexSymbolKind::Method, Some(1)),
        ]
    );

    // 색인에서: 디스크와 같은 것.
    let _lease = f.open(&id);
    f.ready(&id, &root).await;
    assert_eq!(parses(&root, "src/lib.rs"), 1);
    let indexed = f.symbols.file_symbols(params(None)).await.unwrap();
    assert_eq!(indexed, disk);

    // 저장하지 않은 텍스트.
    let edited = f
        .symbols
        .file_symbols(params(Some("fn unsaved_edit() {}\n")))
        .await
        .unwrap();
    assert_eq!(edited.symbols.len(), 1);
    assert_eq!(edited.symbols[0].name, "unsaved_edit");

    // 코드가 아닌 파일 · 프로젝트 밖.
    write(&root.join("README.md"), "# title\n");
    let md = f
        .symbols
        .file_symbols(IndexFileSymbolsParams {
            path: root.join("README.md").display().to_string(),
            ..params(None)
        })
        .await
        .unwrap();
    assert_eq!(md.language, None);
    assert!(md.symbols.is_empty());
    let outside = f.base.join("elsewhere.rs");
    write(&outside, "fn secret() {}\n");
    let e = f
        .symbols
        .file_symbols(IndexFileSymbolsParams {
            path: outside.display().to_string(),
            ..params(None)
        })
        .await
        .unwrap_err();
    assert_eq!(e.code, code::INVALID_PARAMS, "{e:?}");
}

/// 관찰자(심볼 색인) 쪽이 느려도 파일 목록 인덱스는 막히지 않는다 — 파일 검색과 `fs.changed` 적용이 곧바로 된다(관찰자 콜백은
/// 채널에 넣기만 한다). 느린 쪽이 다 따라오면 심볼 색인에도 든다.
#[tokio::test(flavor = "multi_thread")]
async fn a_slow_symbol_index_does_not_block_the_file_list() {
    let f = Fixture::new();
    let (id, root) = f.repo("proj");
    for i in 0..10 {
        write(
            &root.join(format!("src/m{i}.rs")),
            &format!("pub fn slow_{i}() {{}}\n"),
        );
    }
    // 목록 사건 하나를 적용하는 데 1.5초 — 그동안 파일 목록 인덱스의 잠금을 쥐고 있었다면 아래가 모두 그만큼 늦는다.
    set_before_event(&root, || std::thread::sleep(Duration::from_millis(1500)));
    let _lease = f.open(&id);
    let started = Instant::now();
    assert!(f.files.ready(&id, &root).await);
    let file_search = |query: &str| pw_fs::index::SearchFilesParams {
        project_id: id.clone(),
        worktree: None,
        query: query.to_owned(),
        limit: None,
    };
    let found = f.files.search(file_search("m3")).await.unwrap();
    assert_eq!(found.files[0].relative, "src/m3.rs");
    assert!(
        started.elapsed() < Duration::from_millis(1000),
        "{:?}",
        started.elapsed()
    );

    let added = root.join("src/added.rs");
    write(&added, "pub fn slow_added() {}\n");
    let changed_at = Instant::now();
    f.changed(&id, &root, &[&added]);
    loop {
        let found = f.files.search(file_search("added")).await.unwrap();
        if found.files.iter().any(|m| m.relative == "src/added.rs") {
            break;
        }
        assert!(
            changed_at.elapsed() < Duration::from_millis(1000),
            "fs.changed was not applied to the file list while the symbol index was slow"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        f.symbols
            .inner
            .get(&id, &root)
            .is_none_or(|e| !e.state().files.contains_key("src/added.rs")),
        "심볼 색인은 아직 따라오는 중이다"
    );

    set_before_event(&root, || {});
    let found = eventually(&f, &id, "slow", |r| r.matched == 11).await;
    assert!(found.symbols.iter().any(|s| s.name == "slow_added"));
}

/// 파일이 없는 루트도 곧바로 준비된다(요청이 「만드는 중」 을 영영 기다리지 않는다).
#[tokio::test(flavor = "multi_thread")]
async fn an_empty_root_is_ready_at_once() {
    let f = Fixture::new();
    let (id, root) = f.repo("empty");
    let _lease = f.open(&id);
    let found = tokio::time::timeout(Duration::from_secs(5), f.search(&id, "anything"))
        .await
        .expect("빈 루트의 요청이 멈추지 않는다")
        .unwrap();
    assert!(found.symbols.is_empty());
    assert!(found.unindexed.is_empty());
    assert!(f.symbols.ready(&id, &root).await);
}

/// 폴더를 지우면 그 아래 파일의 심볼만 빠진다 — 이름이 그 폴더 이름으로 시작하는 옆 파일(`a-b.rs` 는 `a` 와 `a/` 사이에 정렬된다)은
/// 남는다.
#[tokio::test(flavor = "multi_thread")]
async fn removing_a_folder_drops_only_the_files_under_it() {
    let f = Fixture::new();
    let (id, root) = f.repo("proj");
    write(&root.join("src/a/x.rs"), "pub fn under_folder() {}\n");
    write(&root.join("src/a-b.rs"), "pub fn beside_folder() {}\n");
    let _lease = f.open(&id);
    f.ready(&id, &root).await;
    fs::remove_dir_all(root.join("src/a")).unwrap();
    f.changed(&id, &root, &[&root.join("src/a")]);
    eventually(&f, &id, "folder", |r| r.matched == 1).await;
    let found = f.search(&id, "folder").await.unwrap();
    assert_eq!(
        names(&found),
        [("beside_folder".to_owned(), "src/a-b.rs".to_owned())]
    );
}

/// 한 줄이 긴 1 MiB 파일이 있어도 첫 색인이 그 파일 하나에 붙잡히지 않는다(리뷰 H1 — 파일 하나의 시한 안에 끝나거나 심볼 없이
/// 건너뛴다). JS 번들은 minified 판정으로 빠지므로(M2) 판정에 걸리지 않는 Rust 로 잰다.
#[tokio::test(flavor = "multi_thread")]
async fn a_one_line_megabyte_file_does_not_hold_the_first_index() {
    let f = Fixture::new();
    let (id, root) = f.repo("proj");
    let mut text = String::from("const S: &str = \"é\";");
    let mut i = 0;
    while text.len() < 1_000_000 {
        text.push_str(&format!("fn long_{i}(){{}}"));
        i += 1;
    }
    write(&root.join("src/one.rs"), &text);
    write(
        &root.join("src/lib.rs"),
        "pub fn beside_the_long_file() {}\n",
    );
    let started = Instant::now();
    let _lease = f.open(&id);
    f.ready(&id, &root).await;
    let took = started.elapsed();
    // CI 에서는 한도가 두 배(`PARSE_BUDGET` × 4)다: 공유 Windows 러너의 첫 색인이 한도 근처에서 흔들린다(실측 1.75 · 3.10 ·
    // 4.21 s — 2026-11-11, run 42910504824 · 42916693969 · 42932668158). CI 의 한도는 첫 색인이 두 배를 넘게 느려지는 회귀만
    // 잡는다. 로컬은 × 2 그대로다.
    let limit = if std::env::var_os("CI").is_some_and(|v| !v.is_empty()) {
        extract::PARSE_BUDGET * 4
    } else {
        extract::PARSE_BUDGET * 2
    };
    assert!(took < limit, "first index took {took:?} (limit {limit:?})");
    let found = f.search(&id, "beside").await.unwrap();
    assert_eq!(found.matched, 1);
}

/// 시한은 파싱 뒤의 쿼리 매칭에도 걸린다 — 시한이 지나면 그 파일은 심볼이 없다(일부만 내지 않는다). 작은 입력은 파싱의 진행
/// 콜백이 불리기 전에 끝나므로 이 시험은 쿼리 쪽 확인을 가른다.
#[test]
fn the_budget_also_covers_query_matching() {
    let registry = Registry::from_roots(&Version::new(0, 0, 0), pw_ext::builtin::roots());
    let languages = lang::Languages::new(Arc::new(registry));
    let rust = languages.get("Rust").unwrap();
    let stop = std::sync::atomic::AtomicBool::new(false);
    let text = "fn a() {}\nfn b() {}\n";
    let mut extractor = Extractor::default();
    assert_eq!(
        extractor
            .extract_within(&rust, text, &stop, Duration::from_secs(5))
            .map(|e| e.symbols.len()),
        Some(2)
    );
    assert!(
        extractor
            .extract_within(&rust, text, &stop, Duration::ZERO)
            .is_none()
    );
}

/// 내용으로 판정하는 생성 파일(minified 번들 · `Code generated … DO NOT EDIT` Go · `@generated` 머리)은 색인에서 빠진다(리뷰 M2). 경로 규칙에는 걸리지
/// 않는 이름이다.
#[tokio::test(flavor = "multi_thread")]
async fn content_generated_files_are_not_indexed() {
    let f = Fixture::new();
    let (id, root) = f.repo("proj");
    let mut bundle = String::from("var s=\"x\";");
    for i in 0..200 {
        bundle.push_str(&format!("function gen_bundle_{i}(){{return {i}}}"));
    }
    write(&root.join("public/app.js"), &bundle);
    write(
        &root.join("api/api.go"),
        "// Code generated by protoc-gen-go. DO NOT EDIT.\npackage api\n\nfunc GenGoFunc() {}\n",
    );
    write(
        &root.join("src/bindings.rs"),
        "// @generated by the binding generator\npub fn gen_marked() {}\n",
    );
    write(&root.join("src/main.rs"), "fn gen_hand_written() {}\n");
    let _lease = f.open(&id);
    f.ready(&id, &root).await;
    let found = f.search(&id, "gen").await.unwrap();
    assert_eq!(
        names(&found),
        [("gen_hand_written".to_owned(), "src/main.rs".to_owned())]
    );
}

/// 파일 목록 작업을 취소하면 심볼 색인도 그 루트를 낡은 채 「준비」 로 두지 않는다(리뷰 M1): 파일 목록을 다시 만들 때까지
/// 찾지 않고(`unindexed` · 상태 `suspended`), 다시 만들면 그사이 고친 · 더한 파일이 든다.
#[tokio::test(flavor = "multi_thread")]
async fn cancelling_the_file_list_task_suspends_the_symbols() {
    let f = Fixture::new();
    let (id, root) = f.repo("proj");
    // 걷기가 곧바로 끝나지 않게 파일을 넉넉히(작업이 보이는 동안 취소한다).
    for i in 0..400 {
        write(&root.join(format!("pad/p{i}.txt")), "");
    }
    write(&root.join("src/a.rs"), "pub fn old_a() {}\n");
    let _lease = f.open(&id);
    f.ready(&id, &root).await;
    assert_eq!(f.search(&id, "old_a").await.unwrap().matched, 1);

    // `rescan` 으로 파일 목록을 다시 만들게 하고, 그 작업을 취소한다(놓치면 다시).
    let rescan = FsChanged {
        root: root.display().to_string(),
        worktree: None,
        paths: Vec::new(),
        kinds: Vec::new(),
        rescan: true,
    };
    let mut cancelled = false;
    for _ in 0..50 {
        f.bus
            .publish(Scope::Project(id.clone()), TOPIC_CHANGED, &rescan)
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(1);
        while Instant::now() < deadline {
            let task = f
                .tasks
                .running(Some(&id))
                .into_iter()
                .find(|t| t.title == pw_fs::index::TASK_TITLE);
            if let Some(task) = task {
                cancelled = f.tasks.cancel(task.task_id);
                break;
            }
            tokio::task::yield_now().await;
        }
        if cancelled {
            break;
        }
    }
    assert!(cancelled, "never caught the file list task");
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let status = f
            .symbols
            .status(IndexStatusParams {
                project_id: id.clone(),
                worktree: None,
            })
            .await
            .unwrap();
        if status.roots[0].state == IndexRootState::Suspended {
            break;
        }
        assert!(Instant::now() < deadline, "never suspended: {status:?}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    // 목록이 없는 동안 고친 · 더한 파일: 옛 심볼로 답하지 않는다.
    write(
        &root.join("src/a.rs"),
        "pub fn new_a() {}\npub fn new_a2() {}\n",
    );
    write(&root.join("src/b.rs"), "pub fn new_b() {}\n");
    f.changed(
        &id,
        &root,
        &[&root.join("src/a.rs"), &root.join("src/b.rs")],
    );
    tokio::time::sleep(Duration::from_millis(200)).await;
    let stale = f.search(&id, "old_a").await.unwrap();
    assert!(stale.symbols.is_empty(), "{:?}", names(&stale));
    assert_eq!(stale.unindexed, [root.display().to_string()]);

    // 파일 검색이 목록을 다시 만들면 심볼도 따라온다.
    f.files
        .search(pw_fs::index::SearchFilesParams {
            project_id: id.clone(),
            worktree: None,
            query: "a.rs".to_owned(),
            limit: None,
        })
        .await
        .unwrap();
    let found = eventually(&f, &id, "new", |r| r.matched == 3).await;
    assert!(found.unindexed.is_empty());
    assert_eq!(f.search(&id, "old_a").await.unwrap().matched, 0);
}

/// 화면은 `packages/rpc-client/src/client.ts` 의 `isBusyError` 로 「바쁨 → 다시」 를 고른다: 코드 INTERNAL_ERROR 에
/// 사유 문자열이 그 정규식에 맞아야 한다. 문구를 바꾸면 그 정규식과 rpc-client 시험도 함께 고친다.
fn assert_busy(e: &RpcError) {
    assert_eq!(e.code, pw_rpc::code::INTERNAL_ERROR, "{e:?}");
    let detail = e.data.as_ref().and_then(|d| d.as_str()).unwrap_or_default();
    assert!(detail.starts_with("the symbol index is busy:"), "{e:?}");
}

/// 매칭 · 텍스트 파싱은 Host 전체에서 [`MAX_REQUESTS`] 개까지 함께 돈다 — 그 위는 [`REQUEST_WAIT`] 를 기다리다 「바쁨」(리뷰 M3).
/// 색인을 기다리는 동안은 차례를 쥐지 않는다.
#[tokio::test(flavor = "multi_thread")]
async fn requests_beyond_the_limit_are_busy() {
    let f = Fixture::new();
    let (id, root) = f.repo("proj");
    write(&root.join("src/lib.rs"), "pub fn limited() {}\n");
    let _lease = f.open(&id);
    f.ready(&id, &root).await;
    let held = f
        .symbols
        .inner
        .turns
        .acquire_many(MAX_REQUESTS as u32)
        .await
        .unwrap();
    let started = Instant::now();
    let e = f.search(&id, "limited").await.unwrap_err();
    assert_busy(&e);
    assert!(started.elapsed() >= REQUEST_WAIT);
    let e = f
        .symbols
        .file_symbols(IndexFileSymbolsParams {
            project_id: id.clone(),
            worktree: None,
            path: root.join("src/lib.rs").display().to_string(),
            text: Some("fn x() {}".to_owned()),
        })
        .await
        .unwrap_err();
    assert_busy(&e);
    drop(held);
    assert_eq!(f.search(&id, "limited").await.unwrap().matched, 1);
}

/// 바뀐 파일을 다시 읽는 동안은 옛 심볼로 답한다(빈 결과가 아니다) — 다 읽으면 새 심볼로 바뀐다(리뷰 L1, 03-architecture.md 의
/// 심볼 인덱스 단락).
#[tokio::test(flavor = "multi_thread")]
async fn old_symbols_answer_while_the_file_is_reparsed() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let f = Fixture::new();
    let (id, root) = f.repo("proj");
    let a = root.join("src/a.rs");
    write(&a, "pub fn before_edit() {}\n");
    let _lease = f.open(&id);
    f.ready(&id, &root).await;

    let open = Arc::new(AtomicBool::new(false));
    let started = Arc::new(AtomicBool::new(false));
    set_before_parse(&root, {
        let (open, started) = (Arc::clone(&open), Arc::clone(&started));
        move || {
            started.store(true, Ordering::SeqCst);
            while !open.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    });
    write(&a, "pub fn after_edit() {}\npub fn after_edit_two() {}\n");
    f.changed(&id, &root, &[&a]);
    let deadline = Instant::now() + Duration::from_secs(5);
    while !started.load(Ordering::SeqCst) {
        assert!(Instant::now() < deadline, "the reparse never started");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let during = f.search(&id, "edit").await.unwrap();
    assert_eq!(
        names(&during),
        [("before_edit".to_owned(), "src/a.rs".to_owned())]
    );
    let status = f
        .symbols
        .status(IndexStatusParams {
            project_id: id.clone(),
            worktree: None,
        })
        .await
        .unwrap();
    assert_eq!(status.roots[0].state, IndexRootState::Ready);

    open.store(true, Ordering::SeqCst);
    set_before_parse(&root, || {});
    eventually(&f, &id, "edit", |r| r.matched == 2).await;
    assert_eq!(f.search(&id, "before_edit").await.unwrap().matched, 0);
}

/// 프로젝트 안의 FIFO 를 `index.file.symbols` 로 불러도 막히지 않고 심볼 없이 답한다(리뷰 L2). 되돌렸을 때 시험이 영영 서지 않게,
/// 3초 뒤 쓰는 쪽을 열어 막힌 읽기를 풀어 준다.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn a_fifo_does_not_block_file_symbols() {
    let f = Fixture::new();
    let (id, root) = f.repo("proj");
    let fifo = root.join("pipe.rs");
    let status = std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .unwrap();
    assert!(status.success());
    let unblock = fifo.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(3));
        use std::os::unix::fs::OpenOptionsExt as _;
        let _ = std::fs::OpenOptions::new()
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&unblock);
    });
    let started = Instant::now();
    let result = f
        .symbols
        .file_symbols(IndexFileSymbolsParams {
            project_id: id.clone(),
            worktree: None,
            path: fifo.display().to_string(),
            text: None,
        })
        .await
        .unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(result.language, None);
    assert!(result.symbols.is_empty());
}

/// 감시를 풀 때(`dropped` — 감시 표의 잠금 안에서 불린다) 그 루트의 잠금을 기다리지 않는다: 질의 · 적용이 루트 잠금을 오래 쥐어도
/// 감시 해제가 곧바로 끝난다(리뷰 L4).
#[tokio::test(flavor = "multi_thread")]
async fn releasing_a_watch_does_not_wait_for_the_root_lock() {
    let f = Fixture::new();
    let (id, root) = f.repo("proj");
    write(&root.join("src/lib.rs"), "pub fn held() {}\n");
    let lease = f.open(&id);
    f.ready(&id, &root).await;
    let entry = f.symbols.inner.get(&id, &root).unwrap();
    let (locked_tx, locked_rx) = std::sync::mpsc::channel();
    let holder = std::thread::spawn({
        let entry = Arc::clone(&entry);
        move || {
            let _guard = entry.state();
            locked_tx.send(()).unwrap();
            std::thread::sleep(Duration::from_millis(1500));
        }
    });
    locked_rx.recv().unwrap();
    let started = Instant::now();
    tokio::task::spawn_blocking(move || drop(lease))
        .await
        .unwrap();
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "{:?}",
        started.elapsed()
    );
    assert!(f.symbols.inner.get(&id, &root).is_none());
    holder.join().unwrap();
}

/// 파일 목록 작업 취소(`suspended`)를 사건 스레드가 적용하기 전에 감시가 풀리고 곧바로 다시 걸리면, 그 취소는 옛 루트의 것이다 —
/// 새로 건 루트(만드는 중)를 멈추지 않는다.
#[tokio::test(flavor = "multi_thread")]
async fn a_late_suspension_does_not_stop_a_reopened_root() {
    let f = Fixture::new();
    let root = f.base.join("reopened");
    let sentinel = f.base.join("sentinel");
    let observer = f.symbols.observer();

    let (entered_tx, entered_rx) = std::sync::mpsc::channel::<()>();
    let (go_tx, go_rx) = std::sync::mpsc::channel::<()>();
    let (entered_tx, go_rx) = (Mutex::new(entered_tx), Mutex::new(go_rx));
    set_before_event(&root, move || {
        let _ = entered_tx.lock().unwrap().send(());
        let _ = go_rx.lock().unwrap().recv_timeout(Duration::from_secs(5));
    });
    let (drained_tx, drained_rx) = std::sync::mpsc::channel::<()>();
    let drained_tx = Mutex::new(drained_tx);
    set_before_event(&sentinel, move || {
        let _ = drained_tx.lock().unwrap().send(());
    });

    observer.opened("proj", &root);
    observer.suspended("proj", &root);
    // 사건 스레드가 취소를 집어 들고 멈춘 동안 감시가 풀리고 다시 걸린다.
    entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    observer.dropped("proj", &root);
    observer.opened("proj", &root);
    go_tx.send(()).unwrap();
    // 앞의 사건을 다 적용했다는 표지(이 루트는 자리가 없어 아무것도 바꾸지 않는다).
    observer.changed("proj", &sentinel, Vec::new(), Vec::new());
    drained_rx.recv_timeout(Duration::from_secs(5)).unwrap();

    let entry = f.symbols.inner.get("proj", &root).unwrap();
    assert_eq!(*entry.phase.borrow(), Phase::Building);
    assert!(!entry.state().suspended);
}

/// MCP 툴 `symbol_search`(16-m5-plan.md M5-08 ②). 앱처럼 `methods::register` 로 붙인 라우터를 MCP 세션의 호출자
/// (`Caller::agent`)로 부른다. 이름 · 그룹 · 등급 · 스키마는 진짜 MCP 세션으로 데스크톱 크레이트의 `mcp_tests::code` 가 본다.
mod tool {
    use pw_rpc::{Caller, ConnectionScope, EventHub, Outbound, Role, Router, Session, code};
    use serde_json::{Value, json};

    use super::*;

    const TOOL: &str = "index.tool.symbolSearch";

    async fn call_as(f: &Fixture, caller: Caller, params: Value) -> Result<Value, RpcError> {
        let mut router = Router::new();
        crate::methods::register(&mut router, f.symbols.clone());
        let (session, mut rx) = Session::new(Arc::new(router), EventHub::new(), caller);
        session.handle_value(json!({"jsonrpc": "2.0", "id": 1, "method": TOOL, "params": params}));
        let out: Outbound = tokio::time::timeout(Duration::from_secs(30), rx.recv())
            .await
            .expect("an outbound message")
            .expect("open channel");
        out.into_response().unwrap().result_as()
    }

    async fn call(f: &Fixture, params: Value) -> Result<Value, RpcError> {
        call_as(f, Caller::agent(Role::Owner), params).await
    }

    fn found(result: &Value) -> Vec<(String, String)> {
        result["symbols"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| {
                (
                    s["name"].as_str().unwrap().to_owned(),
                    s["kind"].as_str().unwrap().to_owned(),
                )
            })
            .collect()
    }

    /// 감시하는 프로젝트를 만들고 색인이 다 될 때까지 기다린다.
    async fn indexed(f: &Fixture, name: &str, files: &[(&str, &str)]) -> (String, PathBuf, Lease) {
        let (id, root) = f.repo(name);
        for (rel, text) in files {
            write(&root.join(rel), text);
        }
        let lease = f.open(&id);
        f.ready(&id, &root).await;
        (id, root, lease)
    }

    const WIDGET: &str = "pub struct Widget;\nimpl Widget {\n    pub fn render(&self) {}\n}\npub fn render_all() {}\n";

    /// 완료 기준: 픽스처 저장소에서 이름으로 찾고 `kinds` 로 거른다. 위치는 1 부터, 화면용 칸(일치 위치 · 점수)은 없다.
    #[tokio::test(flavor = "multi_thread")]
    async fn finds_by_name_and_filters_by_kind() {
        let f = Fixture::new();
        let (id, root, _lease) = indexed(&f, "app", &[("src/lib.rs", WIDGET)]).await;
        let result = call(&f, json!({ "projectId": id, "query": "render" }))
            .await
            .unwrap();
        let mut got = found(&result);
        got.sort();
        assert_eq!(
            got,
            [
                ("render".to_owned(), "method".to_owned()),
                ("render_all".to_owned(), "function".to_owned()),
            ],
            "{result}"
        );
        let method = result["symbols"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["name"] == "render")
            .unwrap();
        assert_eq!(method["container"], "Widget");
        assert_eq!(method["relative"], "src/lib.rs");
        assert_eq!(method["path"], json!(root.join("src/lib.rs")));
        assert_eq!(method["language"], "rust");
        // `    pub fn render` — 이름은 3째 줄 12째 열(1 부터).
        assert_eq!(
            (&method["line"], &method["column"]),
            (&json!(3), &json!(12))
        );
        assert_eq!(
            (&method["startLine"], &method["endLine"]),
            (&json!(3), &json!(3))
        );
        assert!(method.get("positions").is_none() && method.get("score").is_none());
        assert_eq!(result["matched"], 2);
        assert_eq!(result["nextCursor"], Value::Null);
        assert_eq!(result["unindexed"], json!([]));
        assert_eq!(result["hint"], Value::Null);

        let functions = call(
            &f,
            json!({ "projectId": id, "query": "render", "kinds": ["function"] }),
        )
        .await
        .unwrap();
        assert_eq!(
            found(&functions),
            [("render_all".to_owned(), "function".to_owned())]
        );
        let types = call(
            &f,
            json!({ "projectId": id, "query": "Widget", "kinds": ["struct"] }),
        )
        .await
        .unwrap();
        assert_eq!(found(&types), [("Widget".to_owned(), "struct".to_owned())]);
    }

    /// 완료 기준: 미신뢰 프로젝트에서도 찾는다(파싱만 한다 — 결정 M4-D8).
    #[tokio::test(flavor = "multi_thread")]
    async fn an_untrusted_project_is_searched() {
        let f = Fixture::new();
        let (id, _root, _lease) = indexed(&f, "cloned", &[("src/lib.rs", WIDGET)]).await;
        f.projects.set_trust(&id, false).unwrap();
        assert!(!f.projects.is_trusted(&id));
        let result = call(
            &f,
            json!({ "projectId": id, "query": "Widget", "kinds": ["struct"] }),
        )
        .await
        .unwrap();
        assert_eq!(found(&result), [("Widget".to_owned(), "struct".to_owned())]);
    }

    /// 완료 기준: 감시하지 않는(창이 열려 있지 않은) 프로젝트는 `unindexed` 와 「Pitwall 에서 열어라」. 툴이 색인을 만들지 않는다.
    #[tokio::test(flavor = "multi_thread")]
    async fn an_unwatched_project_is_unindexed_with_a_hint() {
        let f = Fixture::new();
        let (id, root) = f.repo("closed");
        write(&root.join("src/lib.rs"), WIDGET);
        let result = call(&f, json!({ "projectId": id, "query": "Widget" }))
            .await
            .unwrap();
        assert_eq!(result["symbols"], json!([]));
        assert_eq!(result["unindexed"], json!([root]));
        assert!(
            result["hint"]
                .as_str()
                .unwrap()
                .contains("open the project in Pitwall"),
            "{result}"
        );
        assert!(f.index_tasks(&id).is_empty());
    }

    /// 쪽: 기본 50, `cursor` 로 빠짐 · 겹침 없이 이어지고 마지막 쪽의 `nextCursor` 는 `null`. 못 읽는 커서 · 범위 밖 `limit` 은
    /// invalid params.
    #[tokio::test(flavor = "multi_thread")]
    async fn pages_follow_the_cursor() {
        let f = Fixture::new();
        let mut text = String::new();
        for i in 0..60 {
            text.push_str(&format!("pub fn item_{i:02}() {{}}\n"));
        }
        let (id, _root, _lease) = indexed(&f, "many", &[("src/lib.rs", &text)]).await;
        let first = call(&f, json!({ "projectId": id, "query": "item" }))
            .await
            .unwrap();
        assert_eq!(first["symbols"].as_array().unwrap().len(), 50);
        assert_eq!(first["matched"], 60);
        let mut seen: Vec<String> = found(&first).into_iter().map(|(n, _)| n).collect();
        let mut cursor = first["nextCursor"].clone();
        assert!(cursor.is_string(), "{first}");
        while !cursor.is_null() {
            let page = call(
                &f,
                json!({ "projectId": id, "query": "item", "limit": 7, "cursor": cursor }),
            )
            .await
            .unwrap();
            seen.extend(found(&page).into_iter().map(|(n, _)| n));
            cursor = page["nextCursor"].clone();
            assert!(seen.len() <= 60);
        }
        seen.sort();
        let want: Vec<String> = (0..60).map(|i| format!("item_{i:02}")).collect();
        assert_eq!(seen, want);
        for bad in [
            json!({ "projectId": id, "query": "item", "cursor": "12" }),
            json!({ "projectId": id, "query": "item", "cursor": "" }),
            json!({ "projectId": id, "query": "item", "limit": 0 }),
            json!({ "projectId": id, "query": "item", "limit": 1001 }),
        ] {
            let e = call(&f, bad.clone()).await.unwrap_err();
            assert_eq!(e.code, code::INVALID_PARAMS, "{bad}");
        }
    }

    /// 응답 예산(리뷰 F1): 심볼 하나가 무거우면(깊은 경로 · 긴 이름) `limit` 1000 보다 먼저 [`crate::tools::RESPONSE_BUDGET`] 에서
    /// 끊고, 그 자리부터 다음 쪽이 빠짐 · 겹침 없이 이어진다.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_page_stops_at_the_response_budget_and_resumes_there() {
        let f = Fixture::new();
        let deep = format!("src/{}", ["deeply_nested_folder_name"; 6].join("/"));
        let pad = "x".repeat(60);
        let count = 1200;
        let files: Vec<(String, String)> = (0..count / 100)
            .map(|n| {
                let text: String = (0..100)
                    .map(|i| format!("pub fn budget_{:04}_{pad}() {{}}\n", n * 100 + i))
                    .collect();
                (format!("{deep}/f{n:02}.rs"), text)
            })
            .collect();
        let files: Vec<(&str, &str)> = files
            .iter()
            .map(|(p, t)| (p.as_str(), t.as_str()))
            .collect();
        let (id, _root, _lease) = indexed(&f, "heavy", &files).await;
        let mut seen = Vec::new();
        let mut cursor = Value::Null;
        let mut sizes = Vec::new();
        loop {
            let page = call(
                &f,
                json!({ "projectId": id, "query": "budget", "limit": 1000, "cursor": cursor }),
            )
            .await
            .unwrap();
            let size = serde_json::to_vec(&page).unwrap().len();
            assert!(size <= crate::tools::RESPONSE_BUDGET + 1024, "{size}");
            sizes.push((page["symbols"].as_array().unwrap().len(), size));
            seen.extend(found(&page).into_iter().map(|(n, _)| n));
            cursor = page["nextCursor"].clone();
            if cursor.is_null() {
                break;
            }
            assert!(sizes.len() < 20, "{sizes:?}");
        }
        assert!(
            sizes[0].0 < 1000,
            "the budget never cut the first page: {sizes:?}"
        );
        let mut sorted = seen.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), seen.len(), "a symbol came twice: {sizes:?}");
        let want: Vec<String> = (0..count).map(|i| format!("budget_{i:04}_{pad}")).collect();
        assert_eq!(sorted, want);
    }

    /// 관문은 `agent_grant` 다: 창 · Host 의 연결은 툴을 부르지 못하고(`notAgentSession`), 없는 프로젝트는 그 관문의 오류 그대로
    /// (invalid params).
    #[tokio::test(flavor = "multi_thread")]
    async fn only_an_mcp_session_calls_the_tool() {
        let f = Fixture::new();
        let (id, _root, _lease) = indexed(&f, "app", &[("src/lib.rs", WIDGET)]).await;
        for scope in [ConnectionScope::Project(id.clone()), ConnectionScope::Host] {
            let caller = Caller {
                role: Role::Owner,
                scope,
                window: None,
            };
            let e = call_as(&f, caller, json!({ "projectId": id, "query": "Widget" }))
                .await
                .unwrap_err();
            assert_eq!(e.code, code::PERMISSION_DENIED);
            assert_eq!(e.data.unwrap()["reason"], "notAgentSession");
        }
        let e = call(&f, json!({ "projectId": "nope", "query": "Widget" }))
            .await
            .unwrap_err();
        assert_eq!(e.code, code::INVALID_PARAMS);
    }
}
