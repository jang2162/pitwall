//! 레지스트리 시험. 워크트리 시험은 실제 `git init` · `git worktree add` 로 만든다(제품 코드는 git 을 띄우지 않는다).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use super::*;
use crate::bus::{DEFAULT_CAPACITY, Filter, Received};
use crate::config::PROJECT_DIR;

struct Fixture {
    _tmp: tempfile::TempDir,
    /// 정규화된 임시 폴더. 프로젝트 폴더는 `work/` 아래, 앱 디렉터리는 `app/` 아래에 둔다.
    base: PathBuf,
    paths: AppPaths,
    bus: EventBus,
    registry: Registry,
}

impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let base = real(tmp.path());
        let paths = AppPaths::under(&base.join("app"));
        let bus = EventBus::new(DEFAULT_CAPACITY);
        let registry = Registry::new(paths.clone(), Some(base.join("home")), bus.clone());
        Self {
            _tmp: tmp,
            base,
            paths,
            bus,
            registry,
        }
    }

    /// `work/<rel>` 폴더를 만들고 그 경로를 돌려준다.
    fn dir(&self, rel: &str) -> PathBuf {
        let dir = self.base.join("work").join(rel);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn file(&self, rel: &str) -> PathBuf {
        let file = self.base.join("work").join(rel);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, "").unwrap();
        file
    }

    fn add(&self, root: &Path) -> Project {
        self.registry
            .create(NewProject {
                root: root.to_path_buf(),
                ..NewProject::default()
            })
            .unwrap()
    }

    fn resolve(&self, path: &Path) -> Option<Resolution> {
        self.registry.resolve(path).unwrap()
    }
}

/// 실제 경로. Windows 의 canonicalize 는 verbatim 접두(`\\?\`)를 붙인다 — 레지스트리가 저장하는 꼴((`path::simplify`))로 맞춘다.
fn real(path: &Path) -> PathBuf {
    path::simplify(fs::canonicalize(path).unwrap())
}

/// 레지스트리가 내는 꼴의 경로 문자열. Windows 에서 `join("a/b")` 는 `\a/b` 로 구분자가 섞이므로 마디를 OS 구분자로 다시 잇는다.
fn s(path: &Path) -> String {
    path.components().collect::<PathBuf>().display().to_string()
}

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args([
            "-c",
            "user.name=pitwall-test",
            "-c",
            "user.email=test@pitwall.invalid",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "init.defaultBranch=main",
        ])
        .args(args)
        .current_dir(dir)
        .output()
        .expect("git 실행 파일이 있어야 워크트리 시험을 돌린다");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// `main` 에 있는 파일을 커밋한 저장소를 만들고 `wt` 에 연결 워크트리를 붙인다. 둘 다 정규화된 경로.
fn repo_with_worktree(main: &Path, wt: &Path) -> (PathBuf, PathBuf) {
    fs::create_dir_all(main).unwrap();
    git(main, &["init", "-q"]);
    git(main, &["add", "-A"]);
    git(
        main,
        &["commit", "-q", "--allow-empty", "--no-verify", "-m", "init"],
    );
    git(main, &["worktree", "add", "-q", "-b", "feature", &s(wt)]);
    (real(main), real(wt))
}

// --- CRUD · 최근 목록 · slug ---

#[test]
fn create_get_list_update_delete() {
    let f = Fixture::new();
    let beta = f.add(&f.dir("beta"));
    let alpha = f
        .registry
        .create(NewProject {
            root: f.dir("alpha-root"),
            name: Some("  Alpha  ".into()),
            config_mode: ConfigMode::Personal,
            favorite: true,
            tags: vec!["web".into(), " rust ".into(), "web".into(), "".into()],
        })
        .unwrap();
    assert_eq!(beta.name, "beta", "이름이 없으면 루트 폴더 이름");
    assert_eq!(beta.host, LOCAL_HOST);
    assert_eq!(beta.roots, vec![s(&f.base.join("work/beta"))]);
    assert_eq!(beta.opened_at, None);
    assert_eq!(alpha.name, "Alpha");
    assert_eq!(alpha.tags, vec!["rust", "web"]);
    assert!(alpha.favorite);
    assert_eq!(alpha.config_mode, ConfigMode::Personal);
    assert_eq!(alpha.id.len(), 16);
    assert_ne!(alpha.id, beta.id);

    assert_eq!(f.registry.get(&alpha.id).unwrap(), alpha);
    let names: Vec<String> = f
        .registry
        .list()
        .unwrap()
        .into_iter()
        .map(|p| p.name)
        .collect();
    assert_eq!(names, ["Alpha", "beta"]);

    let updated = f
        .registry
        .update(
            &beta.id,
            ProjectPatch {
                name: Some("Beta 2".into()),
                favorite: Some(true),
                tags: Some(vec!["b".into(), "a".into()]),
                config_mode: Some(ConfigMode::Personal),
            },
        )
        .unwrap();
    assert_eq!(updated.name, "Beta 2");
    assert!(updated.favorite);
    assert_eq!(updated.tags, vec!["a", "b"]);
    assert_eq!(updated.config_mode, ConfigMode::Personal);
    assert_eq!(updated.slug, beta.slug, "이름을 바꿔도 slug 는 그대로");
    assert_eq!(
        f.registry.update(
            &beta.id,
            ProjectPatch {
                name: Some(" ".into()),
                ..ProjectPatch::default()
            }
        ),
        Err(RegistryError::InvalidName)
    );

    f.registry.delete(&beta.id).unwrap();
    assert_eq!(
        f.registry.get(&beta.id),
        Err(RegistryError::NotFound(beta.id.clone()))
    );
    assert_eq!(
        f.registry.delete(&beta.id),
        Err(RegistryError::NotFound(beta.id.clone()))
    );
    assert!(f.base.join("work/beta").is_dir(), "폴더는 지우지 않는다");
    // 같은 루트를 다시 등록할 수 있다(태그도 함께 지워졌다).
    assert_eq!(f.add(&f.dir("beta")).tags, Vec::<String>::new());
}

#[test]
fn create_rejects_bad_roots_and_names() {
    let f = Fixture::new();
    let file = f.file("plain.txt");
    let bad = |root: PathBuf| {
        f.registry.create(NewProject {
            root,
            ..NewProject::default()
        })
    };
    assert!(matches!(
        bad(PathBuf::from("relative/dir")),
        Err(RegistryError::InvalidPath { .. })
    ));
    assert!(matches!(
        bad(f.base.join("work/missing")),
        Err(RegistryError::InvalidPath { .. })
    ));
    assert!(matches!(bad(file), Err(RegistryError::InvalidPath { .. })));
    let empty_name = f.registry.create(NewProject {
        root: f.dir("x"),
        name: Some("   ".into()),
        ..NewProject::default()
    });
    assert_eq!(empty_name, Err(RegistryError::InvalidName));

    let first = f.add(&f.dir("taken"));
    // `..` 로 돌아 들어와도 같은 루트다.
    let again = bad(f.base.join("work/x/../taken"));
    assert_eq!(
        again,
        Err(RegistryError::RootTaken {
            root: f.base.join("work/taken"),
            project_id: first.id
        })
    );
}

#[cfg(unix)]
#[test]
fn root_registered_through_a_symlink_is_stored_as_the_real_path() {
    let f = Fixture::new();
    let real = f.dir("real");
    let link = f.base.join("work/link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let project = f.add(&link);
    assert_eq!(project.roots, vec![s(&real)]);
    assert!(matches!(
        f.registry.create(NewProject {
            root: real,
            ..NewProject::default()
        }),
        Err(RegistryError::RootTaken { .. })
    ));
}

#[test]
fn recent_lists_opened_projects_newest_first() {
    let f = Fixture::new();
    let a = f.add(&f.dir("a"));
    let b = f.add(&f.dir("b"));
    let c = f.add(&f.dir("c"));
    f.add(&f.dir("never-opened"));
    // 같은 밀리초 안에 연달아 열어도 순서가 남는다.
    for id in [&a.id, &b.id, &c.id, &a.id] {
        f.registry.mark_opened(id).unwrap();
    }
    let recent: Vec<String> = f
        .registry
        .recent(10)
        .unwrap()
        .into_iter()
        .map(|p| p.name)
        .collect();
    assert_eq!(recent, ["a", "c", "b"]);
    assert_eq!(f.registry.recent(2).unwrap().len(), 2);
    assert_eq!(
        f.registry.mark_opened("nope"),
        Err(RegistryError::NotFound("nope".into()))
    );
}

#[test]
fn slugs_avoid_collisions_in_the_registry_and_on_disk() {
    let f = Fixture::new();
    let first = f.add(&f.dir("one/App"));
    let second = f.add(&f.dir("two/app"));
    assert_eq!(first.slug, "app");
    assert_eq!(second.slug, "app-2");
    // 레지스트리에는 없지만 개인 전용 폴더가 남아 있는 slug(지운 프로젝트의 것 등)는 건너뛴다.
    fs::create_dir_all(f.paths.personal_project_dir("app-3")).unwrap();
    assert_eq!(f.add(&f.dir("three/APP")).slug, "app-4");
    // 지운 프로젝트의 slug 는 그 폴더가 없으면 다시 쓰인다.
    f.registry.delete(&second.id).unwrap();
    assert_eq!(f.add(&f.dir("four/app")).slug, "app-2");
}

#[test]
fn config_source_follows_the_config_mode() {
    let f = Fixture::new();
    let root = f.dir("proj");
    let project = f.add(&root);
    let source = project.config_source(&f.paths, &root).unwrap();
    assert_eq!(source.dir(), root.join(PROJECT_DIR));

    let personal = f
        .registry
        .update(
            &project.id,
            ProjectPatch {
                config_mode: Some(ConfigMode::Personal),
                ..ProjectPatch::default()
            },
        )
        .unwrap();
    let source = personal.config_source(&f.paths, &root).unwrap();
    assert_eq!(source.dir(), f.paths.personal_project_dir(&personal.slug));
}

#[test]
fn changes_are_published_on_the_host_scope() {
    let f = Fixture::new();
    let mut sub = f
        .bus
        .subscribe(Filter::all().scope(Scope::Host).topics([TOPIC_CHANGED]));
    let p = f.add(&f.dir("p"));
    f.registry.update(&p.id, ProjectPatch::default()).unwrap();
    f.registry.mark_opened(&p.id).unwrap();
    f.registry.delete(&p.id).unwrap();
    // 실패한 변경은 이벤트를 내지 않는다.
    let _ = f.registry.delete(&p.id);
    let mut kinds = Vec::new();
    while let Some(Received::Event(event)) = sub.try_recv() {
        let change: ProjectChanged = serde_json::from_value(event.payload.clone()).unwrap();
        assert_eq!(change.project_id, p.id);
        kinds.push(change.kind);
    }
    assert_eq!(
        kinds,
        [
            ChangeKind::Created,
            ChangeKind::Updated,
            ChangeKind::Opened,
            ChangeKind::Deleted
        ]
    );
}

#[test]
fn registry_persists_and_opens_lazily() {
    let f = Fixture::new();
    let file = f.paths.data.join(store::FILE_NAME);
    assert!(!file.exists(), "만들기만 해서는 파일을 열지 않는다");
    let p = f.add(&f.dir("p"));
    assert!(file.exists());
    let reopened = Registry::new(f.paths.clone(), None, EventBus::new(DEFAULT_CAPACITY));
    assert_eq!(reopened.get(&p.id).unwrap(), p);
    assert_eq!(reopened.status(), StorageStatus::Ok { path: s(&file) });
}

#[test]
fn corrupt_registry_is_set_aside_and_the_app_keeps_working() {
    let f = Fixture::new();
    fs::create_dir_all(&f.paths.data).unwrap();
    let file = f.paths.data.join(store::FILE_NAME);
    fs::write(&file, b"garbage".repeat(1000)).unwrap();
    assert!(matches!(
        f.registry.status(),
        StorageStatus::Recovered { .. }
    ));
    assert_eq!(f.registry.list().unwrap(), Vec::new());
    let p = f.add(&f.dir("p"));
    assert_eq!(f.registry.get(&p.id).unwrap(), p);
}

/// 「열었음」 시각은 앞서 기록한 어느 것보다 크다. 시계가 뒤로 가거나(다른 기기에서 동기화된 파일 등) 같은 밀리초여도
/// 최근 목록의 맨 앞은 방금 연 프로젝트다.
#[test]
fn opened_at_always_moves_past_the_latest_record() {
    let f = Fixture::new();
    let ahead = f.add(&f.dir("ahead"));
    let now = f.add(&f.dir("now"));
    let future = now_ms() + 3_600_000;
    f.registry
        .write(|tx| {
            tx.execute(
                "UPDATE projects SET opened_at = ?2 WHERE id = ?1",
                params![ahead.id, future],
            )?;
            Ok(())
        })
        .unwrap();
    let opened = f.registry.mark_opened(&now.id).unwrap();
    assert!(opened.opened_at.unwrap() > future);
    assert_eq!(f.registry.recent(1).unwrap()[0].id, now.id);
}

/// 같은 파일을 연 레지스트리 여럿(= 프로세스 여럿)이 동시에 써도 `database is locked` 로 실패하지 않는다.
/// 쓰기가 deferred 트랜잭션이면 읽은 뒤 쓰기로 올라갈 때 busy timeout 을 거치지 않고 바로 실패한다. 그 경쟁은 타이밍에 달려
/// 한 라운드로는 놓칠 수 있어 라운드를 되풀이한다(deferred 로 되돌린 코드에서 한 라운드 놓칠 확률 약 1/4 — 4 연결 × 50 쓰기로 잼).
#[test]
fn concurrent_writers_on_one_file_wait_instead_of_failing() {
    use std::sync::{Arc, Barrier};

    const ROUNDS: usize = 4;
    const REGISTRIES: usize = 4;
    const CREATES: usize = 50;
    for round in 0..ROUNDS {
        let f = Fixture::new();
        let barrier = Arc::new(Barrier::new(REGISTRIES));
        let handles: Vec<_> = (0..REGISTRIES)
            .map(|r| {
                // 연결은 레지스트리마다 하나다 — 스레드가 같은 레지스트리를 나누면 그 사이에는 잠금 경쟁이 없다.
                let registry =
                    Registry::new(f.paths.clone(), None, EventBus::new(DEFAULT_CAPACITY));
                let barrier = Arc::clone(&barrier);
                let roots: Vec<PathBuf> =
                    (0..CREATES).map(|i| f.dir(&format!("c/{r}-{i}"))).collect();
                std::thread::spawn(move || {
                    barrier.wait();
                    roots
                        .into_iter()
                        .filter_map(|root| {
                            registry
                                .create(NewProject {
                                    root,
                                    ..NewProject::default()
                                })
                                .and_then(|p| registry.mark_opened(&p.id))
                                .err()
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        let errors: Vec<RegistryError> = handles
            .into_iter()
            .flat_map(|h| h.join().unwrap())
            .collect();
        assert_eq!(errors, Vec::new(), "round {round}");
        assert_eq!(f.registry.list().unwrap().len(), REGISTRIES * CREATES);
    }
}

/// 다른 프로세스가 파일을 통째로 잠근 상태. WAL 에서는 쓰기 트랜잭션이 읽기를 막지 않으므로 `locking_mode = EXCLUSIVE` 로 파일
/// 잠금을 쥔다. 잠금은 연결을 닫아야 풀린다.
fn lock_file(file: &Path) -> Connection {
    let holder = Connection::open(file).unwrap();
    holder
        .execute_batch("PRAGMA locking_mode = EXCLUSIVE; BEGIN EXCLUSIVE")
        .unwrap();
    holder
}

/// `f.registry` 를 열지 않고 프로젝트 하나를 파일에 써 둔다 — 열린 WAL 연결은 파일에 공유 잠금을 쥐어 [`lock_file`] 을 막는다.
fn seed(f: &Fixture, rel: &str) -> Project {
    let seeding = Registry::new(
        f.paths.clone(),
        Some(f.base.join("home")),
        EventBus::new(DEFAULT_CAPACITY),
    );
    seeding
        .create(NewProject {
            root: f.dir(rel),
            ..NewProject::default()
        })
        .unwrap()
}

/// 잠김으로 메모리에 떨어지면 목록은 비고 쓰기는 거절된다. 잠김이 풀리면 다음 호출에서 파일로 돌아간다.
#[test]
fn locked_registry_refuses_writes_and_recovers_once_unlocked() {
    let f = Fixture::new();
    let kept = seed(&f, "kept");
    let file = f.paths.data.join(store::FILE_NAME);
    let holder = lock_file(&file);

    let locked = Registry::with_timings(
        f.paths.clone(),
        None,
        EventBus::new(DEFAULT_CAPACITY),
        Timings {
            busy: Duration::from_millis(50),
            retry_interval: Duration::ZERO,
            retry_busy: Duration::from_millis(50),
        },
    );
    assert!(matches!(locked.status(), StorageStatus::Memory { .. }));
    assert_eq!(locked.list().unwrap(), Vec::new());
    let refused = locked.create(NewProject {
        root: f.dir("lost"),
        ..NewProject::default()
    });
    assert!(
        matches!(refused, Err(RegistryError::Unavailable(_))),
        "{refused:?}"
    );

    drop(holder);
    assert_eq!(locked.list().unwrap(), vec![kept.clone()]);
    assert!(matches!(locked.status(), StorageStatus::Ok { .. }));
    assert!(
        locked
            .create(NewProject {
                root: f.dir("saved"),
                ..NewProject::default()
            })
            .is_ok()
    );
}

/// 잠김이 이어지는 동안 호출마다 처음 열기만큼 기다리지 않는다: 재시도 간격 안의 호출은 메모리 결과를 바로 돌려주고,
/// 재시도 열기는 짧게 기다리며, 다시 연 연결의 쓰기는 처음과 같이 오래 기다린다.
#[test]
fn locked_registry_retries_cheaply_and_restores_the_write_timeout() {
    use std::time::Instant;

    let f = Fixture::new();
    seed(&f, "kept");
    let file = f.paths.data.join(store::FILE_NAME);
    let lock = || lock_file(&file);
    let timings = |retry_interval| Timings {
        busy: Duration::from_millis(1000),
        retry_interval,
        retry_busy: Duration::from_millis(20),
    };
    let quick = Duration::from_millis(500);

    // 간격 안: 다시 열지 않는다(잠김이 풀려도 간격이 지나기 전까지는 메모리다).
    let holder = lock();
    let waiting = Registry::with_timings(
        f.paths.clone(),
        None,
        EventBus::new(DEFAULT_CAPACITY),
        timings(Duration::from_secs(3600)),
    );
    assert!(matches!(waiting.status(), StorageStatus::Memory { .. }));
    for _ in 0..3 {
        let started = Instant::now();
        assert_eq!(waiting.list().unwrap(), Vec::new());
        assert!(started.elapsed() < quick, "{:?}", started.elapsed());
    }
    drop(holder);
    assert!(matches!(waiting.status(), StorageStatus::Memory { .. }));

    // 간격이 지나면: 재시도는 retry_busy 만큼만 기다린다.
    let holder = lock();
    let retrying = Registry::with_timings(
        f.paths.clone(),
        None,
        EventBus::new(DEFAULT_CAPACITY),
        timings(Duration::ZERO),
    );
    assert!(matches!(retrying.status(), StorageStatus::Memory { .. }));
    let started = Instant::now();
    assert_eq!(retrying.list().unwrap(), Vec::new());
    assert!(started.elapsed() < quick, "{:?}", started.elapsed());
    drop(holder);
    assert_eq!(retrying.list().unwrap().len(), 1, "풀리면 파일로 돌아온다");

    // 다시 연 연결의 쓰기는 busy(1초)까지 기다린다 — 300ms 쥐었다 놓는 쓰기에 실패하지 않는다. `retrying` 이 파일을 열고
    // 있으므로 파일 잠금 대신 다른 연결의 쓰기 트랜잭션으로 막는다.
    let holder = Connection::open(&file).unwrap();
    holder.execute_batch("BEGIN IMMEDIATE").unwrap();
    let release = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        holder.execute_batch("ROLLBACK").unwrap();
    });
    let created = retrying.create(NewProject {
        root: f.dir("after-retry"),
        ..NewProject::default()
    });
    release.join().unwrap();
    assert!(created.is_ok(), "{created:?}");
}

/// 앱을 내려 새 스키마 파일을 만나면 목록 · 해석은 되고 쓰기는 거절된다. 파일은 그대로다.
#[test]
fn newer_registry_is_readable_but_not_writable() {
    let f = Fixture::new();
    let kept = f.add(&f.dir("kept"));
    let file = f.paths.data.join(store::FILE_NAME);
    Connection::open(&file)
        .unwrap()
        .pragma_update(None, "user_version", store::supported_version() + 1)
        .unwrap();
    let before = fs::read(&file).unwrap();

    let older_app = Registry::new(f.paths.clone(), None, EventBus::new(DEFAULT_CAPACITY));
    assert!(matches!(older_app.status(), StorageStatus::ReadOnly { .. }));
    assert_eq!(older_app.list().unwrap(), vec![kept.clone()]);
    assert_eq!(
        older_app
            .resolve(&f.base.join("work/kept/x.rs"))
            .unwrap()
            .unwrap()
            .project
            .id,
        kept.id
    );
    for refused in [
        older_app.mark_opened(&kept.id).map(|_| ()),
        older_app.delete(&kept.id),
        older_app
            .create(NewProject {
                root: f.dir("new"),
                ..NewProject::default()
            })
            .map(|_| ()),
    ] {
        assert!(
            matches!(refused, Err(RegistryError::Unavailable(ref r)) if r.contains("newer")),
            "{refused:?}"
        );
    }
    assert_eq!(fs::read(&file).unwrap(), before);
}

// --- 경로 → 프로젝트 해석 ---

#[test]
fn nested_roots_resolve_to_the_deepest() {
    let f = Fixture::new();
    let outer = f.add(&f.dir("mono"));
    let inner = f.add(&f.dir("mono/packages/web"));
    f.add(&f.dir("mono/packages/web-admin"));

    let hit = f
        .resolve(&f.file("mono/packages/web/src/index.ts"))
        .unwrap();
    assert_eq!(hit.project.id, inner.id);
    assert_eq!(hit.root, s(&f.base.join("work/mono/packages/web")));
    assert_eq!(hit.worktree, None);
    // 루트 자체도 그 프로젝트다.
    assert_eq!(
        f.resolve(&f.base.join("work/mono/packages/web"))
            .unwrap()
            .project
            .id,
        inner.id
    );
    // 구성 요소 단위 비교다 — `web` 은 `web-admin` 의 접두사가 아니다.
    assert_ne!(
        f.resolve(&f.file("mono/packages/web-admin/a.ts"))
            .unwrap()
            .project
            .id,
        inner.id
    );
    assert_eq!(
        f.resolve(&f.file("mono/README.md")).unwrap().project.id,
        outer.id
    );
    assert_eq!(f.resolve(&f.file("elsewhere/x.rs")), None);
}

#[test]
fn missing_paths_resolve_by_their_existing_ancestors() {
    let f = Fixture::new();
    let p = f.add(&f.dir("proj"));
    let hit = f
        .resolve(&f.base.join("work/proj/not/yet/created.rs"))
        .unwrap();
    assert_eq!(hit.project.id, p.id);
    assert_eq!(hit.path, s(&f.base.join("work/proj/not/yet/created.rs")));
    // 없는 폴더 뒤의 `..` 로 프로젝트 밖으로 나가면 그 프로젝트가 아니다.
    assert_eq!(
        f.resolve(&f.base.join("work/proj/missing/../../other.rs")),
        None
    );
}

#[test]
fn relative_paths_are_rejected() {
    let f = Fixture::new();
    assert!(matches!(
        f.registry.resolve(Path::new("src/main.rs")),
        Err(RegistryError::InvalidPath { .. })
    ));
    assert!(matches!(
        f.registry.candidates(Path::new("src")),
        Err(RegistryError::InvalidPath { .. })
    ));
}

#[cfg(unix)]
#[test]
fn paths_entering_through_symlinks_resolve_to_the_real_project() {
    let f = Fixture::new();
    let p = f.add(&f.dir("real/proj"));
    f.file("real/proj/src/lib.rs");
    let link = f.base.join("work/shortcut");
    std::os::unix::fs::symlink(f.base.join("work/real/proj/src"), &link).unwrap();
    let hit = f.resolve(&link.join("lib.rs")).unwrap();
    assert_eq!(hit.project.id, p.id);
    assert_eq!(hit.path, s(&f.base.join("work/real/proj/src/lib.rs")));
    // 링크가 등록 루트 밖을 가리키면 링크가 놓인 자리의 프로젝트가 아니다.
    let around = f.add(&f.dir("around"));
    let out_link = f.base.join("work/around/to-real");
    std::os::unix::fs::symlink(f.base.join("work/real/proj"), &out_link).unwrap();
    assert_eq!(
        f.resolve(&out_link.join("src/lib.rs")).unwrap().project.id,
        p.id
    );
    assert_ne!(p.id, around.id);
}

#[test]
fn unregistered_worktree_resolves_to_the_main_project() {
    let f = Fixture::new();
    let (main, wt) = repo_with_worktree(&f.dir("repo"), &f.base.join("work/wt-feature"));
    fs::create_dir_all(main.join("app")).unwrap();
    let project = f.add(&main);
    let hit = f.resolve(&wt.join("app/main.rs")).unwrap();
    assert_eq!(hit.project.id, project.id);
    assert_eq!(hit.root, s(&main));
    assert_eq!(
        hit.path,
        s(&wt.join("app/main.rs")),
        "경로는 워크트리 쪽 그대로"
    );
    assert_eq!(
        hit.worktree,
        Some(ActiveWorktree {
            top: s(&wt),
            root: s(&wt)
        })
    );
    // 메인 체크아웃 안의 경로는 워크트리 없이.
    assert_eq!(f.resolve(&main.join("app")).unwrap().worktree, None);
}

#[test]
fn worktree_of_a_subfolder_project_maps_the_root_into_the_worktree() {
    let f = Fixture::new();
    let (main, wt) = repo_with_worktree(&f.dir("mono"), &f.base.join("work/mono-wt"));
    fs::create_dir_all(main.join("services/api")).unwrap();
    let api = f.add(&main.join("services/api"));
    let hit = f.resolve(&wt.join("services/api/handler.go")).unwrap();
    assert_eq!(hit.project.id, api.id);
    assert_eq!(
        hit.worktree,
        Some(ActiveWorktree {
            top: s(&wt),
            root: s(&wt.join("services/api"))
        })
    );
    // 워크트리 안이지만 그 프로젝트 밖(메인 쪽에도 등록 루트가 없음)이면 없다.
    assert_eq!(f.resolve(&wt.join("README.md")), None);
}

/// Orca 처럼 워크트리를 메인 체크아웃 안(`.worktrees/`)에 두는 경우: 메인 루트가 경로를 이미 포함하지만, 활성 워크트리를 알아야 한다.
#[test]
fn worktree_nested_inside_the_main_checkout_reports_the_worktree() {
    let f = Fixture::new();
    let main_dir = f.dir("host");
    let (main, wt) = repo_with_worktree(&main_dir, &main_dir.join(".worktrees/feat"));
    let project = f.add(&main);
    let hit = f.resolve(&wt.join("src/lib.rs")).unwrap();
    assert_eq!(hit.project.id, project.id);
    assert_eq!(
        hit.worktree,
        Some(ActiveWorktree {
            top: s(&wt),
            root: s(&wt)
        })
    );
}

#[test]
fn registered_worktree_is_its_own_project() {
    let f = Fixture::new();
    let (main, wt) = repo_with_worktree(&f.dir("repo"), &f.base.join("work/wt"));
    f.add(&main);
    let own = f.add(&wt);
    let hit = f.resolve(&wt.join("src/x.rs")).unwrap();
    assert_eq!(hit.project.id, own.id);
    assert_eq!(hit.worktree, None);
}

#[test]
fn worktree_whose_main_is_unregistered_does_not_resolve() {
    let f = Fixture::new();
    let (_main, wt) = repo_with_worktree(&f.dir("repo"), &f.base.join("work/wt"));
    assert_eq!(f.resolve(&wt.join("x.rs")), None);
    // 워크트리와 메인 체크아웃을 함께 감싸는 폴더가 등록돼 있으면 그 프로젝트다. 그 폴더는 메인 저장소의 프로젝트가 아니므로
    // 워크트리로 옮겨 열지 않는다.
    let outer = f.add(&f.base.join("work"));
    let hit = f.resolve(&wt.join("x.rs")).unwrap();
    assert_eq!(hit.project.id, outer.id);
    assert_eq!(hit.worktree, None);
}

// --- 루트 후보 ---

fn candidate_paths(list: &[RootCandidate]) -> Vec<String> {
    list.iter().map(|c| c.path.clone()).collect()
}

#[test]
fn candidates_put_the_vcs_root_first_then_build_folders_nearest_first() {
    let f = Fixture::new();
    let repo = f.dir("repo");
    fs::create_dir_all(repo.join(".git")).unwrap();
    f.file("repo/Cargo.toml");
    f.file("repo/packages/web/package.json");
    f.file("repo/packages/web/src/deep/package.json");
    let file = f.file("repo/packages/web/src/deep/x/index.ts");
    // 저장소 밖의 빌드 파일은 보지 않는다.
    f.file("package.json");

    let list = f.registry.candidates(&file).unwrap();
    assert_eq!(
        candidate_paths(&list),
        [
            s(&repo),
            s(&repo.join("packages/web/src/deep")),
            s(&repo.join("packages/web")),
        ]
    );
    assert!(list[0].vcs && !list[0].worktree_main);
    assert_eq!(
        list[0].build_files,
        ["Cargo.toml"],
        "VCS 루트의 빌드 파일은 같은 후보에 모은다"
    );
    assert_eq!(list[2].build_files, ["package.json"]);
    assert!(!list[2].vcs);
}

#[test]
fn candidates_never_include_home_or_its_ancestors() {
    let f = Fixture::new();
    // 홈 자체가 dotfiles 저장소고 빌드 파일도 있다.
    let home = f.base.join("home");
    fs::create_dir_all(home.join(".git")).unwrap();
    fs::write(home.join("package.json"), "").unwrap();
    fs::write(f.base.join("package.json"), "").unwrap();
    fs::create_dir_all(home.join("code/tool")).unwrap();
    fs::write(home.join("code/tool/go.mod"), "").unwrap();

    let list = f
        .registry
        .candidates(&home.join("code/tool/main.go"))
        .unwrap();
    assert_eq!(candidate_paths(&list), [s(&home.join("code/tool"))]);
    assert_eq!(f.registry.candidates(&home).unwrap(), Vec::new());
    assert_eq!(f.registry.candidates(&f.base).unwrap(), Vec::new());
}

/// 워크트리 안에서 나온 후보는 메인 체크아웃 쪽 경로로 옮긴다 — 워크트리 경로가 프로젝트로 등록되지 않게(F2-5).
#[test]
fn candidates_in_a_worktree_point_into_the_main_checkout() {
    let f = Fixture::new();
    f.file("repo/Cargo.toml");
    f.file("repo/pkg/package.json");
    let (main, wt) = repo_with_worktree(&f.base.join("work/repo"), &f.base.join("work/wt"));
    fs::create_dir_all(wt.join("pkg/src")).unwrap();
    // 워크트리에만 있는 새 폴더는 메인 쪽에 대응하는 곳이 없다.
    fs::create_dir_all(wt.join("scratch")).unwrap();
    fs::write(wt.join("scratch/go.mod"), "").unwrap();

    let list = f.registry.candidates(&wt.join("pkg/src")).unwrap();
    assert_eq!(candidate_paths(&list), [s(&main), s(&main.join("pkg"))]);
    assert!(
        list[0].worktree_main && list[0].vcs,
        "워크트리 꼭대기는 메인 꼭대기와 합쳐진다"
    );
    assert_eq!(list[0].build_files, ["Cargo.toml"]);
    assert_eq!(list[1].build_files, ["package.json"]);

    let list = f.registry.candidates(&wt.join("scratch")).unwrap();
    assert_eq!(candidate_paths(&list), [s(&main)]);
}

#[test]
fn candidates_without_a_repository_list_build_folders_up_to_home() {
    let f = Fixture::new();
    let home = f.base.join("home");
    fs::create_dir_all(home.join("scratch/a/b")).unwrap();
    fs::write(home.join("scratch/pom.xml"), "").unwrap();
    fs::write(home.join("scratch/a/build.gradle.kts"), "").unwrap();
    fs::write(home.join("scratch/a/settings.gradle.kts"), "").unwrap();
    let list = f.registry.candidates(&home.join("scratch/a/b")).unwrap();
    assert_eq!(
        candidate_paths(&list),
        [s(&home.join("scratch/a")), s(&home.join("scratch"))]
    );
    assert_eq!(
        list[0].build_files,
        ["build.gradle.kts", "settings.gradle.kts"]
    );
    assert!(list.iter().all(|c| !c.vcs));
}

/// 앱 없이 뜬 `pitwall mcp` 의 레지스트리: 있는 파일은 읽고, 파일 · 폴더를 만들거나 고치지 않는다.
#[test]
fn a_read_only_registry_reads_without_creating_or_writing() {
    let f = Fixture::new();
    // 파일이 없으면 메모리로 뜨고 `$DATA` 도 만들지 않는다.
    let absent = Registry::read_only(f.paths.clone(), None, f.bus.clone());
    assert!(absent.list().unwrap().is_empty());
    assert!(matches!(absent.status(), StorageStatus::Memory { .. }));
    assert!(!f.paths.data.exists(), "읽기 전용 열기가 폴더를 만들었다");

    let root = f.dir("proj");
    let created = f.add(&root);
    let file = f.registry.file().to_path_buf();
    let before = fs::read(&file).unwrap();

    let reader = Registry::read_only(f.paths.clone(), None, f.bus.clone());
    assert_eq!(reader.list().unwrap(), vec![created.clone()]);
    assert_eq!(
        reader.status(),
        StorageStatus::ReadOnly {
            path: file.display().to_string(),
            reason: store::READ_ONLY_BY_REQUEST.into(),
        }
    );
    assert!(matches!(
        reader.mark_opened(&created.id),
        Err(RegistryError::Unavailable(_))
    ));
    assert!(matches!(
        reader.create(NewProject {
            root: f.dir("other"),
            ..NewProject::default()
        }),
        Err(RegistryError::Unavailable(_))
    ));
    assert_eq!(fs::read(&file).unwrap(), before, "파일이 바뀌었다");
}

/// 손상된 파일을 앱처럼 옮겨 복구하지 않는다 — 복구는 레지스트리를 소유한 앱의 일이다.
#[test]
fn a_read_only_registry_leaves_a_corrupt_file_alone() {
    let f = Fixture::new();
    fs::create_dir_all(&f.paths.data).unwrap();
    let file = f.paths.data.join(store::FILE_NAME);
    fs::write(&file, b"not a database").unwrap();
    let reader = Registry::read_only(f.paths.clone(), None, f.bus.clone());
    assert!(matches!(reader.status(), StorageStatus::Memory { .. }));
    assert!(reader.list().unwrap().is_empty());
    assert_eq!(fs::read(&file).unwrap(), b"not a database");
    let entries: Vec<_> = fs::read_dir(&f.paths.data)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(entries, [std::ffi::OsString::from(store::FILE_NAME)]);
}

#[test]
fn info_summarises_a_registered_project() {
    let f = Fixture::new();
    let root = f.dir("proj");
    f.file("proj/Cargo.toml");
    let created = f.add(&root);
    let info = f.registry.info(&created.id).unwrap();
    assert_eq!(info.project, created);
    assert_eq!(info.languages, ["rust"]);
    assert!(matches!(
        f.registry.info("nope"),
        Err(RegistryError::NotFound(_))
    ));
}

// --- 신뢰 ---

/// v1 파일(신뢰 표가 없던 빌드)의 기존 프로젝트는 v2 로 올라오며 미신뢰(아직 묻지 않음)다.
#[test]
fn projects_from_a_v1_registry_are_untrusted_after_migration() {
    let f = Fixture::new();
    let root = f.dir("old");
    let file = f.paths.data.join(store::FILE_NAME);
    {
        let v1 = store::open_with(&file, store::BUSY_TIMEOUT, &store::MIGRATIONS[..1]);
        assert!(matches!(v1.status, StorageStatus::Ok { .. }));
        v1.conn
            .execute_batch(&format!(
                "INSERT INTO projects (id, name, host, config_mode, slug, created_at)
                 VALUES ('0123456789abcdef', 'old', 'local', 'repository', 'old', 1);
                 INSERT INTO project_roots (project_id, position, path)
                 VALUES ('0123456789abcdef', 0, '{}');",
                s(&root).replace('\'', "''")
            ))
            .unwrap();
        assert!(
            v1.conn.prepare(store::SELECT_TRUST).is_err(),
            "v1 에는 표가 없다"
        );
    }

    assert!(matches!(f.registry.status(), StorageStatus::Ok { .. }));
    let version: u32 = Connection::open(&file)
        .unwrap()
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(version, 2);
    let trust = f.registry.trust("0123456789abcdef").unwrap();
    assert_eq!(trust.state, TrustState::Unknown);
    assert!(!f.registry.is_trusted("0123456789abcdef"));
    // 기존 프로젝트도 처음 열면 한 번 묻는다.
    assert!(f.registry.claim_trust_prompt("0123456789abcdef").unwrap());
}

/// 묻기는 한 번: 처음 잡은 호출만 `true`, 그 기록은 미신뢰다. 신뢰는 파일에 남아 다시 열어도 그대로고 묻지 않는다.
#[test]
fn the_prompt_is_claimed_once_and_trust_survives_reopening() {
    let f = Fixture::new();
    let p = f.add(&f.dir("p"));
    assert_eq!(f.registry.trust(&p.id).unwrap().state, TrustState::Unknown);
    assert!(f.registry.claim_trust_prompt(&p.id).unwrap());
    assert!(!f.registry.claim_trust_prompt(&p.id).unwrap());
    assert_eq!(
        f.registry.trust(&p.id).unwrap().state,
        TrustState::Untrusted
    );
    assert!(!f.registry.is_trusted(&p.id));

    let trusted = f.registry.set_trust(&p.id, true).unwrap();
    assert_eq!(
        trusted,
        ProjectTrust {
            project_id: p.id.clone(),
            owner_id: p.id.clone(),
            state: TrustState::Trusted
        }
    );
    let reopened = Registry::new(f.paths.clone(), None, EventBus::new(DEFAULT_CAPACITY));
    assert!(reopened.is_trusted(&p.id));
    assert!(!reopened.claim_trust_prompt(&p.id).unwrap());

    assert_eq!(
        reopened.set_trust(&p.id, false).unwrap().state,
        TrustState::Untrusted
    );
    assert!(!f.registry.is_trusted(&p.id));

    // 지운 프로젝트의 신뢰는 행과 함께 사라진다 — 같은 폴더를 다시 등록하면 새로 묻는다.
    f.registry.delete(&p.id).unwrap();
    let again = f.add(&f.dir("p"));
    assert_eq!(
        f.registry.trust(&again.id).unwrap().state,
        TrustState::Unknown
    );
    assert!(matches!(
        f.registry.trust(&p.id),
        Err(RegistryError::NotFound(_))
    ));
    assert!(!f.registry.is_trusted("nope"));
}

/// 같은 프로젝트의 창 여럿(다른 프로세스 포함 — 연결이 따로인 레지스트리)이 동시에 열려도 다이얼로그는 하나만 뜬다.
#[test]
fn concurrent_prompt_claims_let_only_one_window_ask() {
    const WINDOWS: usize = 4;
    for round in 0..10 {
        let f = Fixture::new();
        let p = f.add(&f.dir("p"));
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(WINDOWS));
        let threads: Vec<_> = (0..WINDOWS)
            .map(|_| {
                let registry =
                    Registry::new(f.paths.clone(), None, EventBus::new(DEFAULT_CAPACITY));
                let barrier = std::sync::Arc::clone(&barrier);
                let id = p.id.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    registry.claim_trust_prompt(&id)
                })
            })
            .collect();
        let claims: Vec<bool> = threads
            .into_iter()
            .map(|t| t.join().unwrap().unwrap())
            .collect();
        assert_eq!(
            claims.iter().filter(|&&c| c).count(),
            1,
            "round {round}: {claims:?}"
        );
    }
}

/// 워크트리를 따로 등록한 프로젝트는 메인 저장소 프로젝트의 신뢰를 읽고 바꾸고 묻는다.
#[test]
fn a_worktree_project_follows_the_main_repository_trust() {
    let f = Fixture::new();
    let (main, wt) = repo_with_worktree(&f.dir("repo"), &f.base.join("work/wt"));
    fs::create_dir_all(main.join("app")).unwrap();
    fs::create_dir_all(wt.join("app")).unwrap();
    let main_project = f.add(&main);
    let wt_project = f.add(&wt);
    let wt_sub = f.add(&wt.join("app"));
    let other = f.add(&f.dir("other"));

    let trust = f.registry.trust(&wt_project.id).unwrap();
    assert_eq!(trust.owner_id, main_project.id);
    assert_eq!(
        f.registry.trust(&wt_sub.id).unwrap().owner_id,
        main_project.id
    );
    assert_eq!(f.registry.trust(&other.id).unwrap().owner_id, other.id);

    // 워크트리 창이 먼저 물었으면 메인 창은 묻지 않는다.
    assert!(f.registry.claim_trust_prompt(&wt_project.id).unwrap());
    assert!(!f.registry.claim_trust_prompt(&main_project.id).unwrap());

    let set = f.registry.set_trust(&wt_project.id, true).unwrap();
    assert_eq!(set.owner_id, main_project.id);
    assert!(f.registry.is_trusted(&main_project.id));
    assert!(f.registry.is_trusted(&wt_sub.id));
    assert!(!f.registry.is_trusted(&other.id));
    f.registry.set_trust(&main_project.id, false).unwrap();
    assert!(!f.registry.is_trusted(&wt_project.id));
}

/// 메인 저장소가 등록되지 않은 워크트리 프로젝트는 자기 신뢰를 가진다.
#[test]
fn a_worktree_project_without_its_main_repository_owns_its_trust() {
    let f = Fixture::new();
    let (_main, wt) = repo_with_worktree(&f.dir("repo"), &f.base.join("work/wt"));
    let wt_project = f.add(&wt);
    assert_eq!(
        f.registry.trust(&wt_project.id).unwrap().owner_id,
        wt_project.id
    );
    f.registry.set_trust(&wt_project.id, true).unwrap();
    assert!(f.registry.is_trusted(&wt_project.id));
}

/// 바꾸기는 신뢰를 가진 프로젝트로 `trust` 변경을 낸다. 묻기 차례 잡기는 상태 표시가 그대로라 내지 않는다.
#[test]
fn trust_changes_are_published_for_the_owner() {
    let f = Fixture::new();
    let (main, wt) = repo_with_worktree(&f.dir("repo"), &f.base.join("work/wt"));
    let main_project = f.add(&main);
    let wt_project = f.add(&wt);
    let mut sub = f
        .bus
        .subscribe(Filter::all().scope(Scope::Host).topics([TOPIC_CHANGED]));
    f.registry.claim_trust_prompt(&wt_project.id).unwrap();
    f.registry.set_trust(&wt_project.id, true).unwrap();
    let mut changes = Vec::new();
    while let Some(Received::Event(event)) = sub.try_recv() {
        changes.push(serde_json::from_value::<ProjectChanged>(event.payload.clone()).unwrap());
    }
    assert_eq!(
        changes,
        [ProjectChanged {
            kind: ChangeKind::Trust,
            project_id: main_project.id
        }]
    );
}

/// `memory` · `readOnly` 에서는 신뢰를 바꾸지도 묻지도 않는다 — 「없는 프로젝트」 가 아니라 저장소 오류로 거절한다.
#[test]
fn trust_writes_are_refused_when_the_registry_cannot_be_saved() {
    let f = Fixture::new();
    let kept = seed(&f, "kept");
    let file = f.paths.data.join(store::FILE_NAME);

    let holder = lock_file(&file);
    let locked = Registry::with_timings(
        f.paths.clone(),
        None,
        EventBus::new(DEFAULT_CAPACITY),
        Timings {
            busy: Duration::from_millis(50),
            retry_interval: Duration::from_secs(60),
            retry_busy: Duration::from_millis(50),
        },
    );
    assert!(matches!(locked.status(), StorageStatus::Memory { .. }));
    for refused in [
        locked.set_trust(&kept.id, true).map(|_| ()),
        locked.claim_trust_prompt(&kept.id).map(|_| ()),
    ] {
        assert!(
            matches!(refused, Err(RegistryError::Unavailable(_))),
            "{refused:?}"
        );
    }
    assert!(!locked.is_trusted(&kept.id));
    drop(holder);

    Connection::open(&file)
        .unwrap()
        .pragma_update(None, "user_version", store::supported_version() + 1)
        .unwrap();
    let older_app = Registry::new(f.paths.clone(), None, EventBus::new(DEFAULT_CAPACITY));
    assert!(matches!(older_app.status(), StorageStatus::ReadOnly { .. }));
    assert_eq!(
        older_app.trust(&kept.id).unwrap().state,
        TrustState::Unknown
    );
    for refused in [
        older_app.set_trust(&kept.id, true).map(|_| ()),
        older_app.claim_trust_prompt(&kept.id).map(|_| ()),
    ] {
        assert!(
            matches!(refused, Err(RegistryError::Unavailable(ref r)) if r.contains("newer")),
            "{refused:?}"
        );
    }
}
