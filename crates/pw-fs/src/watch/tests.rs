//! 실제 OS 감시로 `fs.changed` 를 본다(M1-06 완료 기준). 레지스트리에 등록한 임시 프로젝트를 감시하고 버스를 구독한다.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use pw_core::bus::{DEFAULT_CAPACITY, EventBus, Filter, Received, Scope, Subscription};
use pw_core::config::AppPaths;
use pw_core::notification::{Notification, TOPIC_SHOW};
use pw_core::registry::path::simplify;
use pw_core::registry::{NewProject, Registry as Projects};

use super::engine::{CREATIONS_ARE_EXACT, LIMIT_MESSAGE, Options, Strategy, watched_dirs_of};
use super::*;

/// 완료 기준의 지연 상한.
const LATENCY: Duration = Duration::from_millis(300);
/// 이벤트가 「온다」 만 볼 때 기다리는 시간. 지연을 재는 시험(완료 기준 ①)이 아니면 이것을 쓴다 — 부하가 큰 기계에서는 OS 가
/// 이벤트를 늦게 넘긴다(macOS 부하 160 에서 새 폴더 이벤트가 0.13~0.67초, 다른 시험이 함께 돌면 0.9초를 넘었다). 오면 곧바로
/// 돌아오므로 통과하는 시험은 늦어지지 않는다.
const ARRIVES: Duration = Duration::from_secs(10);

struct Fixture {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    fs: Arc<Fs>,
    projects: Projects,
    bus: EventBus,
}

impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let base = simplify(fs::canonicalize(tmp.path()).unwrap());
        let paths = AppPaths::under(&base.join("app"));
        let bus = EventBus::new(DEFAULT_CAPACITY);
        let projects = Projects::new(paths, None, bus.clone());
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

    fn watch(&self, options: Options) -> Watch {
        let watch = Watch::with_options(Arc::clone(&self.fs), options);
        watch.attach(self.bus.clone());
        watch
    }

    fn changes(&self, project: &str) -> Changes {
        Changes(
            self.bus.subscribe(
                Filter::all()
                    .scope(Scope::Project(project.to_owned()))
                    .topics([TOPIC_CHANGED]),
            ),
        )
    }

    fn notifications(&self) -> Subscription {
        self.bus.subscribe(Filter::all().topics([TOPIC_SHOW]))
    }
}

fn params(project: &str) -> FsWatchParams {
    FsWatchParams {
        project_id: project.to_owned(),
        worktree: None,
    }
}

fn folders(options: Options) -> Options {
    Options {
        strategy: Strategy::Folders,
        ..options
    }
}

/// 받은 `fs.changed`.
struct Changes(Subscription);

impl Changes {
    fn next(&mut self, within: Duration) -> Option<(Instant, FsChanged)> {
        let deadline = Instant::now() + within;
        loop {
            match self.0.try_recv() {
                Some(Received::Event(e)) => {
                    return Some((
                        Instant::now(),
                        serde_json::from_value(e.payload.clone()).unwrap(),
                    ));
                }
                Some(Received::Lagged(n)) => panic!("lost {n} events"),
                None if Instant::now() >= deadline => return None,
                None => std::thread::sleep(Duration::from_millis(1)),
            }
        }
    }

    /// `path` 를 담은 첫 이벤트. 그 전에 온 것도 함께 돌려준다.
    fn until(
        &mut self,
        path: &Path,
        within: Duration,
    ) -> (Option<(Instant, FsChanged)>, Vec<FsChanged>) {
        let deadline = Instant::now() + within;
        let mut before = Vec::new();
        while let Some((at, changed)) =
            self.next(deadline.saturating_duration_since(Instant::now()))
        {
            if has(&changed, path) {
                return (Some((at, changed)), before);
            }
            before.push(changed);
        }
        (None, before)
    }

    fn all(&mut self, within: Duration) -> Vec<(Instant, FsChanged)> {
        let deadline = Instant::now() + within;
        let mut out = Vec::new();
        while let Some(got) = self.next(deadline.saturating_duration_since(Instant::now())) {
            out.push(got);
        }
        out
    }
}

fn has(changed: &FsChanged, path: &Path) -> bool {
    changed.paths.iter().any(|p| Path::new(p) == path)
}

fn kind_of(changed: &FsChanged, path: &Path) -> Option<FsChangeKind> {
    let i = changed.paths.iter().position(|p| Path::new(p) == path)?;
    Some(changed.kinds[i])
}

/// 이 플랫폼의 방식과(Linux 면 그것이 폴더별 감시다) 폴더별 감시 둘 다.
fn strategies() -> Vec<Strategy> {
    let mut all = vec![Strategy::for_this_platform()];
    if !all.contains(&Strategy::Folders) {
        all.push(Strategy::Folders);
    }
    all
}

/// 완료 기준 ①: 외부에서 생성 · 수정 · 삭제 · 이름 바꾸기 · 임시 파일 rename 을 하면 300 ms 안에 그 경로를 담은 `fs.changed` 가 온다.
/// 임시 파일 rename 은 대상 경로의 변경 하나로 보이고, OS 의 「생성」 이 정확하면(macOS 밖) 임시 파일 경로는 나오지 않는다.
#[test]
fn external_changes_arrive_within_300ms() {
    for strategy in strategies() {
        let f = Fixture::new();
        let root = f.base.join("proj");
        let id = f.project(&root);
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/old.rs"), "old").unwrap();
        fs::write(root.join("src/doc.md"), "v1").unwrap();
        let mut changes = f.changes(&id);
        let watch = f.watch(Options {
            strategy,
            ..Options::default()
        });
        let _lease = watch.acquire(&params(&id)).unwrap();

        let target = root.join("src/doc.md");
        let temp = root.join("src/.doc.md.tmp-1");
        type Op<'a> = (&'a str, PathBuf, Box<dyn Fn() + 'a>, Option<FsChangeKind>);
        let ops: Vec<Op<'_>> = vec![
            (
                "create",
                root.join("src/new.rs"),
                Box::new(|| fs::write(root.join("src/new.rs"), "new").unwrap()),
                Some(FsChangeKind::Created),
            ),
            (
                "modify",
                root.join("src/new.rs"),
                Box::new(|| fs::write(root.join("src/new.rs"), "newer").unwrap()),
                None,
            ),
            (
                "rename (old name)",
                root.join("src/old.rs"),
                Box::new(|| {
                    fs::rename(root.join("src/old.rs"), root.join("src/renamed.rs")).unwrap()
                }),
                Some(FsChangeKind::Removed),
            ),
            (
                "delete",
                root.join("src/new.rs"),
                Box::new(|| fs::remove_file(root.join("src/new.rs")).unwrap()),
                Some(FsChangeKind::Removed),
            ),
            (
                "temp file rename",
                target.clone(),
                Box::new(|| {
                    fs::write(&temp, "v2").unwrap();
                    fs::rename(&temp, &target).unwrap();
                }),
                Some(FsChangeKind::Modified),
            ),
        ];
        for (name, path, op, kind) in ops {
            let started = Instant::now();
            op();
            let (got, before) = changes.until(&path, LATENCY * 3);
            let (at, changed) = got.unwrap_or_else(|| {
                panic!(
                    "{strategy:?} {name}: no fs.changed for {} (got {before:?})",
                    path.display()
                )
            });
            let elapsed = at - started;
            eprintln!("latency {strategy:?} {name}: {elapsed:?}");
            assert!(elapsed < LATENCY, "{strategy:?} {name}: {elapsed:?}");
            if let Some(kind) = kind {
                assert_eq!(
                    kind_of(&changed, &path),
                    Some(kind),
                    "{strategy:?} {name}: {changed:?}"
                );
            }
            assert_eq!(changed.root, root.display().to_string());
            assert!(!changed.rescan, "{strategy:?} {name}");
            // 다음 동작 전에 이 동작의 늦은 이벤트를 비운다. 대상 경로는 한 번이고, 임시 파일은 어느 이벤트에도 없어야 한다.
            let late: Vec<FsChanged> = changes
                .all(Duration::from_millis(150))
                .into_iter()
                .map(|(_, c)| c)
                .collect();
            let seen: Vec<&FsChanged> = before.iter().chain([&changed]).chain(&late).collect();
            if name == "temp file rename" {
                let targets = seen.iter().filter(|c| has(c, &target)).count();
                assert_eq!(targets, 1, "{strategy:?}: {seen:?}");
            }
            if CREATIONS_ARE_EXACT {
                assert!(
                    seen.iter().all(|c| !has(c, &temp)),
                    "{strategy:?} {name}: {seen:?}"
                );
            }
        }
        // 이름 바꾸기의 새 이름도 온다(위에서 비운 이벤트 안에 있었을 수 있어 따로 만들어 본다).
        let started = Instant::now();
        fs::rename(root.join("src/renamed.rs"), root.join("src/again.rs")).unwrap();
        let (got, _) = changes.until(&root.join("src/again.rs"), LATENCY * 3);
        let (at, _) =
            got.unwrap_or_else(|| panic!("{strategy:?} rename (new name): no fs.changed"));
        assert!(at - started < LATENCY, "{strategy:?} rename (new name)");
    }
}

/// 완료 기준 ②: 같은 파일을 10 ms 간격으로 20번 쓰면 이벤트가 디바운스 창마다 하나다 — 이어지는 두 이벤트는 75 ms 이상 떨어진다.
#[test]
fn twenty_writes_10ms_apart_are_one_event_per_debounce_window() {
    for strategy in strategies() {
        let f = Fixture::new();
        let root = f.base.join("proj");
        let id = f.project(&root);
        let path = root.join("busy.txt");
        fs::write(&path, "0").unwrap();
        let mut changes = f.changes(&id);
        let watch = f.watch(Options {
            strategy,
            ..Options::default()
        });
        let _lease = watch.acquire(&params(&id)).unwrap();

        // 받은 시각을 재려면 쓰는 동안에도 꺼내야 한다 — 쓰고 나서 꺼내면 쌓인 것이 한꺼번에 나와 붙어 보인다.
        let collector = std::thread::spawn(move || changes.all(Duration::from_millis(700)));
        let started = Instant::now();
        for i in 0..20 {
            fs::write(&path, i.to_string()).unwrap();
            std::thread::sleep(Duration::from_millis(10));
        }
        let writing = started.elapsed();
        let got: Vec<_> = collector
            .join()
            .unwrap()
            .into_iter()
            .filter(|(_, c)| has(c, &path))
            .collect();
        // 쓰는 동안 열린 창 수 + 마지막 창. 20번이 그대로 나가면 20이다.
        let windows = writing.as_millis().div_ceil(engine::DEBOUNCE.as_millis()) as usize + 1;
        eprintln!(
            "debounce {strategy:?}: {} events over {writing:?}",
            got.len()
        );
        assert!(
            !got.is_empty() && got.len() <= windows,
            "{strategy:?}: {} events over {writing:?} (at most {windows})",
            got.len()
        );
        for pair in got.windows(2) {
            let gap = pair[1].0 - pair[0].0;
            // 받는 쪽은 1 ms 간격으로 꺼낸다 — 그만큼 여유를 둔다.
            assert!(
                gap + Duration::from_millis(5) >= engine::DEBOUNCE,
                "{strategy:?}: two events {gap:?} apart"
            );
        }
    }
}

fn warnings(sub: &mut Subscription) -> Vec<Notification> {
    let mut out = Vec::new();
    while let Some(received) = sub.try_recv() {
        if let Received::Event(e) = received {
            out.push(serde_json::from_value(e.payload.clone()).unwrap());
        }
    }
    out
}

/// 완료 기준 ③: 한도 초과 오류를 주입하면 그 뒤의 폴더는 폴링으로 바뀌고, 경고가 한 번 나간다. 폴링 폴더의 변경도 온다.
#[test]
fn a_watch_limit_falls_back_to_polling_and_warns_once() {
    let f = Fixture::new();
    let root = f.base.join("proj");
    let id = f.project(&root);
    fs::create_dir_all(root.join("a")).unwrap();
    fs::create_dir_all(root.join("b")).unwrap();
    let mut notes = f.notifications();
    let mut changes = f.changes(&id);
    let options = folders(Options {
        // 루트 하나만 OS 감시에 걸리고 `a` · `b` 는 폴링이다.
        fault_limit: Some(1),
        poll_interval: Duration::from_millis(100),
        ..Options::default()
    });
    let watch = f.watch(options.clone());
    let mut lease = watch.acquire(&params(&id)).unwrap();
    assert_eq!(
        lease.snapshot().roots,
        vec![FsWatchedRoot {
            path: root.display().to_string(),
            mode: WatchMode::Polling
        }]
    );
    let dirs = watched_dirs_of(&options, &root);
    assert_eq!(dirs.get(&root), Some(&true), "{dirs:?}");
    assert_eq!(dirs.get(&root.join("a")), Some(&false), "{dirs:?}");

    let shown = warnings(&mut notes);
    assert_eq!(shown.len(), 1, "{shown:?}");
    assert_eq!(shown[0].message, LIMIT_MESSAGE);
    assert_eq!(shown[0].args.get("root"), Some(&root.display().to_string()));

    // 폴링 폴더 안의 변경: 폴링 간격 + 디바운스 안에 온다.
    fs::write(root.join("b/polled.txt"), "p").unwrap();
    let (got, _) = changes.until(&root.join("b/polled.txt"), Duration::from_secs(3));
    assert!(got.is_some(), "the polled folder's change did not arrive");

    // 새 폴더도 폴링으로 붙고, 같은 루트를 다시 걸거나(다른 창) 놓았다 다시 걸어도 경고는 더 나가지 않는다.
    fs::create_dir_all(root.join("c")).unwrap();
    let (got, _) = changes.until(&root.join("c"), Duration::from_secs(3));
    assert!(got.is_some());
    fs::write(root.join("c/later.txt"), "l").unwrap();
    let (got, _) = changes.until(&root.join("c/later.txt"), Duration::from_secs(3));
    assert!(got.is_some(), "a folder added after the limit is polled");
    let second = watch.acquire(&params(&id)).unwrap();
    drop((lease, second));
    let _again = watch.acquire(&params(&id)).unwrap();
    assert!(warnings(&mut notes).is_empty());
}

/// 폴더별 감시(Linux 의 방식)는 ignore 된 폴더와 `.git/` 에 감시를 걸지 않고, 새로 생긴 폴더에는 건다.
#[test]
fn folder_watches_skip_ignored_folders_and_follow_new_ones() {
    let f = Fixture::new();
    let root = f.base.join("proj");
    let id = f.project(&root);
    for dir in [".git/objects", "src/deep", "target/debug", ".github"] {
        fs::create_dir_all(root.join(dir)).unwrap();
    }
    fs::write(root.join(".gitignore"), "target/\n").unwrap();
    let options = folders(Options::default());
    let dirs: Vec<PathBuf> = watched_dirs_of(&options, &root).into_keys().collect();
    assert_eq!(
        dirs,
        vec![
            root.clone(),
            root.join(".github"),
            root.join("src"),
            root.join("src/deep")
        ]
    );

    let mut changes = f.changes(&id);
    let watch = f.watch(options);
    let _lease = watch.acquire(&params(&id)).unwrap();
    fs::create_dir_all(root.join("src/fresh")).unwrap();
    let (got, _) = changes.until(&root.join("src/fresh"), ARRIVES);
    assert!(got.is_some());
    // 폴더가 알려진 뒤에 그 안에 쓴 파일이 온다(새 폴더에 감시가 붙었다).
    fs::write(root.join("src/fresh/a.rs"), "a").unwrap();
    let (got, before) = changes.until(&root.join("src/fresh/a.rs"), ARRIVES);
    assert!(got.is_some(), "{before:?}");
    // ignore 된 폴더 안에 새로 만든 폴더에는 걸지 않는다.
    fs::create_dir_all(root.join("target/debug/build")).unwrap();
    fs::write(root.join("target/debug/build/x.o"), "x").unwrap();
    let late = changes.all(Duration::from_millis(300));
    assert!(
        late.iter()
            .all(|(_, c)| !has(c, &root.join("target/debug/build/x.o"))),
        "{late:?}"
    );
}

/// 새 폴더를 만들고 곧바로 그 안에 쓴 항목도 온다 — 폴더별 감시(Linux)에서는 감시가 그 폴더에 붙기 전에 생긴 것이라 OS 가
/// 알리지 않는다(에이전트가 새 모듈 폴더를 만들고 쓰기, `cp -r`, 압축 풀기, 브랜치 전환).
///
/// 이 플랫폼의 방식과, 한도를 넘어 새 폴더가 폴링으로 붙는 폴더별 감시(폴링은 감시를 걸 때 있던 항목을 알리지 않는다)로 본다.
/// macOS 에서 OS 감시로 폴더별 감시를 하면 notify 가 감시를 더할 때마다 FSEvents 스트림을 멈췄다 「지금부터」 로 다시 열어
/// (`fsevent.rs` 의 `watch_inner`) 그 틈의 이벤트를 잃는다 — 루트에 새로 만든 폴더 자체도 오지 않는다. macOS 의 방식은 재귀
/// 감시라 실제로는 생기지 않아 그 조합은 보지 않는다.
#[test]
fn items_written_right_after_a_new_folder_arrive() {
    let polled = folders(Options {
        fault_limit: Some(1),
        poll_interval: Duration::from_millis(100),
        ..Options::default()
    });
    let platform = Options::default();
    for options in [platform, polled] {
        let strategy = options.strategy;
        let mode = if options.fault_limit.is_some() {
            "polled"
        } else {
            "native"
        };
        let f = Fixture::new();
        let root = f.base.join("proj");
        let id = f.project(&root);
        let mut changes = f.changes(&id);
        let watch = f.watch(options);
        let _lease = watch.acquire(&params(&id)).unwrap();
        let started = Instant::now();

        let mut expected = std::collections::BTreeSet::new();
        for t in 0..10 {
            let inner = root.join(format!("new{t}/inner"));
            fs::create_dir_all(&inner).unwrap();
            expected.insert(root.join(format!("new{t}")));
            expected.insert(inner.clone());
            for i in 0..20 {
                let file = inner.join(format!("f{i}.txt"));
                fs::write(&file, "x").unwrap();
                expected.insert(file);
            }
        }
        let mut missing = expected.clone();
        let deadline = Instant::now() + Duration::from_secs(3);
        while !missing.is_empty() {
            let Some((_, changed)) =
                changes.next(deadline.saturating_duration_since(Instant::now()))
            else {
                break;
            };
            assert!(!changed.rescan, "{strategy:?}: {changed:?}");
            for p in &changed.paths {
                missing.remove(Path::new(p));
            }
        }
        let files = missing.iter().filter(|p| p.extension().is_some()).count();
        eprintln!(
            "new folders {strategy:?} {mode}: {} of {} paths in {:?}",
            expected.len() - missing.len(),
            expected.len(),
            started.elapsed()
        );
        assert!(
            missing.is_empty(),
            "{strategy:?} {mode}: {} of {} paths missing ({files} files), e.g. {:?}",
            missing.len(),
            expected.len(),
            missing.iter().take(3).collect::<Vec<_>>()
        );
    }
}

/// 재귀 감시: ignore 된 폴더 안의 변경은 그 폴더(가장 바깥의 ignore 된 폴더) 하나로 접혀 `modified` 로 온다 — `npm install` ·
/// `cargo build` 가 창마다 수만 경로를 내지 않는다. ignore 되지 않은 경로는 그대로다.
#[test]
fn changes_inside_an_ignored_folder_fold_into_that_folder() {
    let f = Fixture::new();
    let root = f.base.join("proj");
    let id = f.project(&root);
    // `.gitignore` 는 저장소 안에서만 적용된다(ripgrep 과 같다, `crate::list`).
    fs::create_dir_all(root.join(".git")).unwrap();
    fs::write(root.join(".gitignore"), "node_modules/\n").unwrap();
    let lib = root.join("node_modules/pkg/lib");
    fs::create_dir_all(&lib).unwrap();
    let mut changes = f.changes(&id);
    let watch = f.watch(Options {
        strategy: Strategy::Recursive,
        ..Options::default()
    });
    let _lease = watch.acquire(&params(&id)).unwrap();

    for i in 0..500 {
        fs::write(lib.join(format!("m{i}.js")), "x").unwrap();
    }
    fs::create_dir_all(root.join("node_modules/fresh/deep")).unwrap();
    fs::write(root.join("node_modules/fresh/deep/a.js"), "x").unwrap();
    fs::write(root.join("app.js"), "x").unwrap();
    let got = changes.all(Duration::from_millis(600));
    let all: Vec<&FsChanged> = got.iter().map(|(_, c)| c).collect();
    let paths: usize = all.iter().map(|c| c.paths.len()).sum();
    eprintln!("ignored folder: {} events, {paths} paths", all.len());
    assert!(all.iter().all(|c| !c.rescan), "{all:?}");
    let node_modules = root.join("node_modules");
    for c in &all {
        for p in &c.paths {
            assert!(
                !Path::new(p).starts_with(&node_modules) || Path::new(p) == node_modules,
                "{p} is inside the ignored folder"
            );
        }
    }
    let folded = all
        .iter()
        .find_map(|c| kind_of(c, &node_modules))
        .expect("the ignored folder itself");
    assert_eq!(folded, FsChangeKind::Modified);
    assert!(all.iter().any(|c| has(c, &root.join("app.js"))), "{all:?}");
}

/// `.git/` 안의 변경은 거른다(M2 의 몫). 같은 창의 다른 경로는 온다.
#[test]
fn git_internals_are_filtered() {
    for strategy in strategies() {
        let f = Fixture::new();
        let root = f.base.join("proj");
        let id = f.project(&root);
        fs::create_dir_all(root.join(".git")).unwrap();
        let mut changes = f.changes(&id);
        let watch = f.watch(Options {
            strategy,
            ..Options::default()
        });
        let _lease = watch.acquire(&params(&id)).unwrap();
        fs::write(root.join(".git/index"), "i").unwrap();
        fs::write(root.join("a.txt"), "a").unwrap();
        let (got, before) = changes.until(&root.join("a.txt"), ARRIVES);
        let (_, changed) = got.expect("a.txt");
        for c in before.iter().chain([&changed]) {
            assert!(
                c.paths.iter().all(|p| !p.contains("/.git/")),
                "{strategy:?}: {c:?}"
            );
        }
    }
}

/// 루트 밖을 가리키는 심링크를 따라 루트 밖 변경을 알리지 않는다(결정 M1-D8). 링크 항목 자체의 생성은 루트 안의 변경이라 온다.
#[cfg(unix)]
#[test]
fn changes_behind_a_link_to_outside_are_not_reported() {
    for strategy in strategies() {
        let f = Fixture::new();
        let root = f.base.join("proj");
        let outside = f.base.join("outside");
        let id = f.project(&root);
        fs::create_dir_all(outside.join("sub")).unwrap();
        std::os::unix::fs::symlink(&outside, root.join("link")).unwrap();
        let mut changes = f.changes(&id);
        let watch = f.watch(Options {
            strategy,
            ..Options::default()
        });
        let _lease = watch.acquire(&params(&id)).unwrap();

        fs::write(outside.join("secret.txt"), "s").unwrap();
        fs::write(outside.join("sub/deeper.txt"), "d").unwrap();
        std::os::unix::fs::symlink(&outside, root.join("link2")).unwrap();
        let (got, before) = changes.until(&root.join("link2"), ARRIVES);
        assert!(
            got.is_some(),
            "{strategy:?}: the new link itself is a change inside the root"
        );
        let late = changes.all(Duration::from_millis(300));
        for c in before
            .iter()
            .chain(got.iter().map(|(_, c)| c))
            .chain(late.iter().map(|(_, c)| c))
        {
            for p in &c.paths {
                assert!(
                    Path::new(p).starts_with(&root)
                        && !p.contains("secret")
                        && !p.contains("deeper"),
                    "{strategy:?}: {c:?}"
                );
            }
        }
    }
}

/// 감시가 ignore 파일을 읽는 두 자리 — 폴더별 감시를 걸 폴더 고르기와, 새 폴더 · 바뀐 경로가 ignore 된 폴더 안인지 보기 — 가
/// 링크된 ignore 파일(`link` 가 만든다)을 읽지 않고 제때 끝난다. `ignored` 는 `.ignore` 로 숨긴 폴더라 그 규칙은 그대로 적용된다.
#[cfg(unix)]
fn linked_ignore_files_are_not_read(link: fn(&Path, &Path)) {
    use crate::search::tests::{MIB, bounded};

    // 시한은 영영 막히는 고장에서 시험을 끝내려는 것이다 — 막히면 첫 대기(`ARRIVES`)에서 실패한다.
    bounded(64 * MIB, Duration::from_secs(60), move || {
        for strategy in strategies() {
            let f = Fixture::new();
            let root = f.base.join("proj");
            let id = f.project(&root);
            for dir in [".git", "src", "ignored"] {
                fs::create_dir_all(root.join(dir)).unwrap();
            }
            fs::write(root.join(".ignore"), "ignored/\n").unwrap();
            link(&f.base, &root);
            let options = Options {
                strategy,
                ..Options::default()
            };
            if strategy == Strategy::Folders {
                let dirs: Vec<PathBuf> = watched_dirs_of(&options, &root).into_keys().collect();
                assert_eq!(dirs, vec![root.clone(), root.join("src")]);
            }
            let mut changes = f.changes(&id);
            let watch = f.watch(options);
            let _lease = watch.acquire(&params(&id)).unwrap();
            fs::create_dir_all(root.join("src/fresh")).unwrap();
            let (got, before) = changes.until(&root.join("src/fresh"), ARRIVES);
            assert!(got.is_some(), "{strategy:?}: {before:?}");
            fs::write(root.join("src/fresh/a.rs"), "a").unwrap();
            let (got, before) = changes.until(&root.join("src/fresh/a.rs"), ARRIVES);
            assert!(got.is_some(), "{strategy:?}: {before:?}");
        }
    });
}

/// `.gitignore -> /dev/zero` 링크를 끝까지 읽으면 메모리가 끝없이 는다.
#[cfg(unix)]
#[test]
fn an_ignore_file_linked_to_dev_zero_is_not_read() {
    linked_ignore_files_are_not_read(|_, root| {
        std::os::unix::fs::symlink("/dev/zero", root.join(".gitignore")).unwrap();
    });
}

/// FIFO 를 가리키는 `.gitignore` · `.ignore` 링크를 열면 쓰는 쪽이 없어 감시 스레드가 영영 막힌다.
#[cfg(unix)]
#[test]
fn ignore_files_linked_to_a_fifo_do_not_block_the_watch() {
    linked_ignore_files_are_not_read(|base, root| {
        let fifo = base.join("fifo");
        let c = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
        // SAFETY: `c` 는 NUL 로 끝나는 살아 있는 경로 문자열이다.
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o644) }, 0);
        std::os::unix::fs::symlink(&fifo, root.join("src/.gitignore")).unwrap();
        std::os::unix::fs::symlink(&fifo, root.join("src/.ignore")).unwrap();
    });
}

/// 임대를 모두 놓으면 OS 감시를 풀고 더 내지 않는다. 같은 루트를 두 창이 보면 감시는 하나다.
#[test]
fn the_watch_is_shared_and_released_with_the_last_lease() {
    let f = Fixture::new();
    let root = f.base.join("proj");
    let id = f.project(&root);
    let mut changes = f.changes(&id);
    let watch = f.watch(Options::default());
    let first = watch.acquire(&params(&id)).unwrap();
    let second = watch.acquire(&params(&id)).unwrap();
    assert_eq!(watch.watched_roots(), 1);

    drop(first);
    assert_eq!(watch.watched_roots(), 1);
    fs::write(root.join("a.txt"), "a").unwrap();
    let (got, _) = changes.until(&root.join("a.txt"), ARRIVES);
    let (_, changed) = got.expect("still watched by the second lease");
    // 한 번 쓴 것은 이벤트 하나다(엔진이 둘이면 둘이 온다).
    let dup = changes.all(Duration::from_millis(200));
    assert!(
        dup.iter().all(|(_, c)| !has(c, &root.join("a.txt"))),
        "{changed:?} then {dup:?}"
    );

    drop(second);
    assert_eq!(watch.watched_roots(), 0);
    fs::write(root.join("b.txt"), "b").unwrap();
    let late = changes.all(LATENCY);
    assert!(late.is_empty(), "{late:?}");
}

/// 활성 워크트리를 주면 그 워크트리 안의 프로젝트 루트를 보고, 이벤트에 그 워크트리를 싣는다. 메인 체크아웃의 변경은 오지 않는다.
#[test]
fn a_worktree_lease_watches_the_worktree_root() {
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

    let mut changes = f.changes(&id);
    let watch = f.watch(Options::default());
    let worktree = wt.display().to_string();
    let mut lease = watch
        .acquire(&FsWatchParams {
            project_id: id.clone(),
            worktree: Some(worktree.clone()),
        })
        .unwrap();
    assert_eq!(
        lease.snapshot().roots[0].path,
        wt.join("app").display().to_string()
    );

    fs::write(main.join("app/main.txt"), "m").unwrap();
    fs::write(wt.join("app/wt.txt"), "w").unwrap();
    let (got, before) = changes.until(&wt.join("app/wt.txt"), ARRIVES);
    let (_, changed) = got.expect("worktree change");
    assert_eq!(changed.worktree.as_deref(), Some(worktree.as_str()));
    let late = changes.all(Duration::from_millis(200));
    for c in before
        .iter()
        .chain([&changed])
        .chain(late.iter().map(|(_, c)| c))
    {
        assert!(!has(c, &main.join("app/main.txt")), "{c:?}");
    }

    // 워크트리가 아닌 폴더는 거절한다.
    assert!(
        watch
            .acquire(&FsWatchParams {
                project_id: id,
                worktree: Some(f.base.display().to_string()),
            })
            .is_err()
    );
}

/// RPC: 첫 항목(감시 방식)이 온 뒤의 변경은 놓치지 않고, 세션이 닫히면(창이 닫힘) 감시를 놓는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_stream_holds_the_watch_until_the_session_closes() {
    use pw_rpc::{Caller, EventHub, Outbound, Role, Session};

    let f = Fixture::new();
    let root = f.base.join("proj");
    let id = f.project(&root);
    let mut changes = f.changes(&id);
    let watch = f.watch(Options::default());
    let mut router = Router::new();
    register(&mut router, watch.clone());
    let (session, mut rx) =
        Session::new(Arc::new(router), EventHub::new(), Caller::new(Role::Owner));
    session.handle_value(serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "fs.watch", "params": { "projectId": id }
    }));
    let opened = rx.recv().await.unwrap().into_response().unwrap();
    let stream_id = opened.outcome.unwrap()["streamId"].clone();
    session.handle_value(serde_json::json!({
        "jsonrpc": "2.0", "method": "stream.credit", "params": { "streamId": stream_id, "credit": 4 }
    }));
    let chunk = loop {
        match rx.recv().await.unwrap() {
            Outbound::Notification(n) if n.method == "stream.chunk" => break n,
            _ => {}
        }
    };
    let watching: FsWatching = serde_json::from_value(chunk.params["data"].clone()).unwrap();
    assert_eq!(watching.roots[0].path, root.display().to_string());
    assert_eq!(watch.watched_roots(), 1);

    let path = root.join("a.txt");
    let found = tokio::task::spawn_blocking(move || {
        fs::write(&path, "a").unwrap();
        changes.until(&path, ARRIVES).0.is_some()
    })
    .await
    .unwrap();
    assert!(found);

    session.close();
    for _ in 0..100 {
        if watch.watched_roots() == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(watch.watched_roots(), 0);
}

/// 한도 초과 경고의 메시지 id 가 내장 번역의 모든 언어에 있다(창은 없는 id 를 그대로 보인다).
#[test]
fn the_limit_message_exists_in_every_bundled_language() {
    let i18n = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../extensions/i18n-default/i18n");
    let mut languages = 0;
    for entry in fs::read_dir(&i18n).unwrap() {
        let file = entry.unwrap().path().join("project.ftl");
        let source = fs::read_to_string(&file).unwrap();
        assert!(
            source
                .lines()
                .any(|l| l.starts_with(&format!("{LIMIT_MESSAGE} ="))),
            "{} has no `{LIMIT_MESSAGE}`",
            file.display()
        );
        languages += 1;
    }
    assert!(languages >= 2, "{languages}");
}
