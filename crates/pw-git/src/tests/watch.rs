//! `git.watch`(M2-02 완료 기준 ① · ②): 실제 OS 감시 위에서 터미널 쪽 git 동작이 맞는 이벤트를 내고, 그 뒤의 `git.status` ·
//! `git.refs` 가 git 과 같다. 작업 트리 쪽은 진짜 `fs.watch`(pw-fs)가 내는 `fs.changed` 를 따른다.
//!
//! macOS FSEvents 는 감시를 걸기 직전의 기록을 늦게 주고 같은 경로의 기록을 다음 창에 합쳐 준다(M1-06). 그래서 감시를 건 직후와
//! 단언 전마다 조용해질 때까지 비우고([`Events::settle`]), 「이 이벤트가 왔다」 만 본다 — 몇 번 왔는지는 보지 않는다. 500 ms
//! 상한은 release 빌드에서만 단언한다([`RELEASE_TIMING`], `mise run test-release`). debug 에서는 지연을 찍기만 한다.

use std::time::Instant;

use pw_core::bus::{Filter, Received, Subscription};
use pw_fs::watch::{FsWatchParams, Lease as FsLease, Watch as FsWatch};

use super::status::{Fx, assert_same_refs, assert_same_status, rt};
use super::*;
use crate::status::GitChange;
use crate::testing::TestRepo;
use crate::watch::{
    GitRefsChanged, GitStatusChanged, GitWatch, TOPIC_REFS_CHANGED, TOPIC_STATUS_CHANGED,
    WatchLease,
};

/// 완료 기준의 지연 상한.
const LATENCY: Duration = Duration::from_millis(500);
/// release 빌드에서만 [`LATENCY`] 를 단언한다.
const RELEASE_TIMING: bool = !cfg!(debug_assertions);
/// 이벤트가 「온다」 만 볼 때 기다리는 시간(부하 아래에서 OS 가 이벤트를 늦게 넘긴다 — M1-06 의 `ARRIVES`).
const ARRIVES: Duration = Duration::from_secs(10);
/// 이만큼 이벤트가 없으면 조용해졌다고 본다.
const QUIET: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, PartialEq)]
enum Event {
    Status(GitStatusChanged),
    Refs(GitRefsChanged),
}

impl Event {
    fn worktree(&self) -> &Option<String> {
        match self {
            Event::Status(e) => &e.worktree,
            Event::Refs(e) => &e.worktree,
        }
    }

    fn is_status(&self) -> bool {
        matches!(self, Event::Status(_))
    }

    fn is_refs(&self) -> bool {
        matches!(self, Event::Refs(_))
    }

    /// 상태 이벤트가 `path` 를 담는다(전체 이벤트도 담는 것으로 본다).
    fn covers(&self, path: &str) -> bool {
        match self {
            Event::Status(GitStatusChanged { paths: None, .. }) => true,
            Event::Status(GitStatusChanged {
                paths: Some(paths), ..
            }) => paths.iter().any(|p| p == path),
            Event::Refs(_) => false,
        }
    }
}

/// 받은 git 이벤트.
struct Events(Subscription);

impl Events {
    fn new(bus: &EventBus, project: &str) -> Self {
        Self(
            bus.subscribe(
                Filter::all()
                    .scope(Scope::Project(project.to_owned()))
                    .topics([TOPIC_STATUS_CHANGED, TOPIC_REFS_CHANGED]),
            ),
        )
    }

    fn next(&mut self, within: Duration) -> Option<Event> {
        let deadline = Instant::now() + within;
        loop {
            match self.0.try_recv() {
                Some(Received::Event(e)) => {
                    let payload = e.payload.clone();
                    return Some(if e.topic == TOPIC_STATUS_CHANGED {
                        Event::Status(serde_json::from_value(payload).unwrap())
                    } else {
                        Event::Refs(serde_json::from_value(payload).unwrap())
                    });
                }
                Some(Received::Lagged(n)) => panic!("lost {n} events"),
                None if Instant::now() >= deadline => return None,
                None => std::thread::sleep(Duration::from_millis(1)),
            }
        }
    }

    /// `want` 가 모두 맞을 때까지 받는다. 각 조건을 처음 채운 시각을 돌려준다. 오지 않으면 받은 것과 함께 패닉한다.
    fn until(&mut self, what: &str, want: &[&dyn Fn(&Event) -> bool]) -> Vec<Instant> {
        let deadline = Instant::now() + ARRIVES;
        let mut seen: Vec<Option<Instant>> = vec![None; want.len()];
        let mut got = Vec::new();
        while seen.iter().any(Option::is_none) {
            let Some(event) = self.next(deadline.saturating_duration_since(Instant::now())) else {
                panic!("{what}: not every expected event arrived within {ARRIVES:?}: {got:?}");
            };
            let now = Instant::now();
            for (slot, pred) in seen.iter_mut().zip(want) {
                if slot.is_none() && pred(&event) {
                    *slot = Some(now);
                }
            }
            got.push(event);
        }
        seen.into_iter().map(Option::unwrap).collect()
    }

    /// 조용해질 때까지 비우고 비운 것을 돌려준다.
    fn settle(&mut self) -> Vec<Event> {
        let deadline = Instant::now() + ARRIVES * 3;
        let mut out = Vec::new();
        while let Some(event) = self.next(QUIET) {
            out.push(event);
            assert!(
                Instant::now() < deadline,
                "the watch did not settle: {out:?}"
            );
        }
        out
    }
}

/// 실제 파일 감시(`fs.watch`)와 git 감시를 건 프로젝트.
struct Watched {
    fx: Fx,
    rt: tokio::runtime::Runtime,
    fs_watch: FsWatch,
    git_watch: GitWatch,
}

impl Watched {
    fn new() -> Self {
        let fx = Fx::new();
        let fs = Arc::new(pw_fs::Fs::new());
        fs.attach(fx.projects.clone());
        let fs_watch = FsWatch::new(fs);
        fs_watch.attach(fx.bus());
        let git_watch = GitWatch::new(Arc::clone(&fx.git));
        Self {
            fx,
            rt: rt(),
            fs_watch,
            git_watch,
        }
    }

    fn fs(&self, id: &str, worktree: Option<&Path>) -> FsLease {
        self.fs_watch
            .acquire(&FsWatchParams {
                project_id: id.to_owned(),
                worktree: worktree.map(|w| w.display().to_string()),
            })
            .unwrap()
    }

    fn git(&self, id: &str, worktree: Option<&Path>) -> WatchLease {
        let _rt = self.rt.enter();
        self.git_watch
            .acquire(&GitRepoParams {
                project_id: id.to_owned(),
                worktree: worktree.map(|w| w.display().to_string()),
            })
            .unwrap()
    }
}

/// 한 동작의 지연을 찍고, release 면 상한을 단언한다.
fn latency(what: &str, started: Instant, arrived: &[Instant]) {
    for at in arrived {
        let took = at.saturating_duration_since(started);
        eprintln!("latency {what}: {took:?}");
        if RELEASE_TIMING {
            assert!(took < LATENCY, "{what}: {took:?}");
        }
    }
}

fn status_for(path: &'static str) -> impl Fn(&Event) -> bool {
    move |e: &Event| e.covers(path)
}

fn any_status(e: &Event) -> bool {
    e.is_status()
}

fn any_refs(e: &Event) -> bool {
    e.is_refs()
}

/// 완료 기준 ①: 파일 고치기 · `git add` · `git commit` · `git checkout -b` · `git stash` 가 각각 맞는 이벤트를 내고, 그 뒤의
/// `git.status` · `git.refs` 가 git 과 같다.
#[test]
fn terminal_git_actions_raise_the_matching_events_and_the_reads_follow() {
    let r = TestRepo::with_commit();
    r.commit_file("src/lib.rs", "fn a() {}\n", "lib");
    let w = Watched::new();
    let id = w.fx.project(r.root());
    let _fs = w.fs(&id, None);
    let lease = w.git(&id, None);
    assert_eq!(
        lease.watching().work_tree.as_deref(),
        Some(r.root().to_str().unwrap())
    );
    let mut events = Events::new(&w.fx.bus(), &id);
    events.settle();

    let started = Instant::now();
    r.write("src/lib.rs", "fn a() { changed() }\n");
    let at = events.until("edit", &[&status_for("src/lib.rs")]);
    latency("edit", started, &at);
    events.settle();
    assert_same_status(&w.fx, &r, &id, Some(&["src/lib.rs"]));
    assert_same_status(&w.fx, &r, &id, None);

    let started = Instant::now();
    r.git(&["add", "src/lib.rs"]);
    let at = events.until("git add", &[&any_status]);
    latency("git add", started, &at);
    events.settle();
    assert_same_status(&w.fx, &r, &id, None);

    let started = Instant::now();
    r.git(&["commit", "--quiet", "-m", "change"]);
    let at = events.until("git commit", &[&any_refs, &any_status]);
    latency("git commit", started, &at);
    events.settle();
    assert_same_status(&w.fx, &r, &id, None);
    assert_same_refs(&w.fx, &r, &id);

    let started = Instant::now();
    r.git(&["checkout", "--quiet", "-b", "feature"]);
    let at = events.until("git checkout -b", &[&any_refs]);
    latency("git checkout -b", started, &at);
    events.settle();
    assert_same_refs(&w.fx, &r, &id);

    r.write("README.md", "stash me\n");
    r.write("new.txt", "untracked stays\n");
    events.until("edit before stash", &[&status_for("README.md")]);
    events.settle();
    let started = Instant::now();
    r.git(&["stash", "--quiet"]);
    let at = events.until("git stash", &[&any_refs, &any_status]);
    latency("git stash", started, &at);
    events.settle();
    assert_same_status(&w.fx, &r, &id, None);
    assert_same_refs(&w.fx, &r, &id);
    assert!(
        w.fx.refs(&id, None)
            .branches
            .iter()
            .any(|b| b.head && b.name == "feature")
    );
}

/// index 를 건드리지 않고 지금 브랜치만 옮겨도(`git reset --soft`) 상태 전체가 바뀐 것으로 낸다 — HEAD 파일은 그대로고 브랜치 ref
/// 만 바뀐다.
#[test]
fn moving_the_current_branch_alone_reports_the_status() {
    let r = TestRepo::with_commit();
    r.commit_file("second.txt", "2\n", "second");
    let w = Watched::new();
    let id = w.fx.project(r.root());
    let _git = w.git(&id, None);
    let mut events = Events::new(&w.fx.bus(), &id);
    events.settle();
    let started = Instant::now();
    r.git(&["reset", "--quiet", "--soft", "HEAD~1"]);
    let at = events.until(
        "git reset --soft",
        &[
            &any_refs,
            &|e: &Event| matches!(e, Event::Status(s) if s.paths.is_none()),
        ],
    );
    latency("git reset --soft", started, &at);
    events.settle();
    assert_same_status(&w.fx, &r, &id, None);
}

/// 완료 기준 ②: 연결 워크트리 창은 메인 체크아웃 쪽 `git branch x` 를 `git.refsChanged` 로 받는다(공유 git 디렉터리의 `refs/`).
/// 메인 체크아웃의 `index` 는 그 창의 상태가 아니다.
#[test]
fn a_branch_made_in_the_main_checkout_reaches_a_linked_worktree() {
    let r = TestRepo::with_commit();
    let wt = r.add_worktree("wt", "feature");
    let w = Watched::new();
    let id = w.fx.project(r.root());
    let _fs = w.fs(&id, Some(&wt));
    let _git = w.git(&id, Some(&wt));
    let mut events = Events::new(&w.fx.bus(), &id);
    events.settle();
    let worktree = Some(wt.display().to_string());

    let started = Instant::now();
    r.git(&["branch", "x"]);
    let at = events.until(
        "git branch x",
        &[&|e: &Event| e.is_refs() && *e.worktree() == worktree],
    );
    latency("git branch x", started, &at);
    events.settle();
    let refs = w.fx.refs(&id, Some(&wt));
    assert!(refs.branches.iter().any(|b| b.name == "x"));

    // 메인 체크아웃의 스테이징은 연결 워크트리 창의 상태를 바꾸지 않는다.
    r.write("README.md", "main checkout change\n");
    r.git(&["add", "README.md"]);
    let late = events.settle();
    assert!(
        late.iter().all(|e| !e.is_status()),
        "the main checkout's index is not the worktree's status: {late:?}"
    );

    // 연결 워크트리 안의 커밋은 그 창의 이벤트다.
    fs::write(wt.join("wt.txt"), "x\n").unwrap();
    r.commit_all_in(&wt, "in the worktree");
    events.until("commit in the worktree", &[&any_refs, &any_status]);
}

/// 같은 (프로젝트, 워크트리)는 감시 하나를 센다. 창이 닫히면(임대가 다 풀리면) 놓고, 구독도 버스에서 빠진다. 활성 워크트리를
/// 바꾸면(놓고 새 워크트리로 다시 잡음) 옛 워크트리의 이벤트는 더 오지 않는다.
#[test]
fn the_watch_is_shared_and_released_when_the_last_window_closes() {
    let r = TestRepo::with_commit();
    let wt = r.add_worktree("wt", "feature");
    let w = Watched::new();
    let id = w.fx.project(r.root());
    let bus = w.fx.bus();
    let baseline = bus.subscriber_count();
    let a = w.git(&id, None);
    let b = w.git(&id, None);
    assert_eq!(w.git_watch.watched(), 1);
    // 감시 하나가 구독 둘(`fs.changed` · `project.changed`)을 잡는다 — 창이 둘이어도 둘이다.
    assert_eq!(bus.subscriber_count(), baseline + 2);
    drop(a);
    assert_eq!(w.git_watch.watched(), 1);
    drop(b);
    assert_eq!(w.git_watch.watched(), 0);
    wait_for(
        || bus.subscriber_count() == baseline,
        "the subscriptions to go",
    );

    // 워크트리 전환: 메인 → 연결 워크트리.
    let mut events = Events::new(&bus, &id);
    let main = w.git(&id, None);
    events.settle();
    drop(main);
    let _wt = w.git(&id, Some(&wt));
    assert_eq!(w.git_watch.watched(), 1);
    events.settle();
    r.git(&["branch", "after-switch"]);
    let worktree = Some(wt.display().to_string());
    events.until(
        "branch after the switch",
        &[&|e: &Event| e.is_refs() && *e.worktree() == worktree],
    );
    let late = events.settle();
    let all_wt = late.iter().all(|e| *e.worktree() == worktree);
    assert!(all_wt, "the old worktree's watch still publishes: {late:?}");
}

fn wait_for(mut done: impl FnMut() -> bool, what: &str) {
    let deadline = Instant::now() + ARRIVES;
    while !done() {
        assert!(Instant::now() < deadline, "waited too long for {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// `fs.changed` 를 옮기는 규칙: 경로는 저장소 기준(`/`), `rescan` · `.gitignore` · `.gitattributes` 는 전체, 다른 워크트리 창의
/// 것은 버린다. OS 감시 없이 버스에 직접 낸 `fs.changed` 로 본다.
#[test]
fn fs_changes_map_to_status_paths() {
    let r = TestRepo::with_commit();
    let w = Watched::new();
    let id = w.fx.project(r.root());
    let _git = w.git(&id, None);
    let bus = w.fx.bus();
    let mut events = Events::new(&bus, &id);
    events.settle();
    let root = r.root().display().to_string();
    let send = |worktree: Option<&str>, paths: &[&str], rescan: bool| {
        let changed = pw_fs::watch::FsChanged {
            root: root.clone(),
            worktree: worktree.map(str::to_owned),
            paths: paths
                .iter()
                .map(|p| r.root().join(p).display().to_string())
                .collect(),
            kinds: paths
                .iter()
                .map(|_| pw_fs::watch::FsChangeKind::Modified)
                .collect(),
            rescan,
        };
        bus.publish(
            Scope::Project(id.clone()),
            pw_fs::watch::TOPIC_CHANGED,
            &changed,
        )
        .unwrap();
    };
    let status = |paths: Option<&[&str]>| {
        Event::Status(GitStatusChanged {
            worktree: None,
            paths: paths.map(|p| p.iter().map(|s| (*s).to_owned()).collect()),
        })
    };

    send(None, &["src/b.rs", "a.txt"], false);
    assert_eq!(events.settle(), [status(Some(&["a.txt", "src/b.rs"]))]);
    send(None, &["sub/.gitignore"], false);
    assert_eq!(events.settle(), [status(None)]);
    send(None, &[".gitattributes"], false);
    assert_eq!(events.settle(), [status(None)]);
    send(None, &[], true);
    assert_eq!(events.settle(), [status(None)]);
    send(Some("/somewhere/else"), &["a.txt"], false);
    assert_eq!(events.settle(), []);
}

/// 신뢰가 바뀌면 감시가 저장소를 다시 열고(새 신뢰의 설정으로) 두 이벤트를 전체로 낸다.
#[test]
fn a_trust_change_reopens_and_reports_everything() {
    let r = TestRepo::with_commit();
    let w = Watched::new();
    let id = w.fx.project(r.root());
    let _git = w.git(&id, None);
    let mut events = Events::new(&w.fx.bus(), &id);
    events.settle();
    w.fx.projects.set_trust(&id, true).unwrap();
    events.until(
        "trust",
        &[
            &|e: &Event| matches!(e, Event::Status(s) if s.paths.is_none()),
            &any_refs,
        ],
    );
}

/// 저장소가 없으면 감시하지 않고 `workTree: null` 이다.
#[test]
fn no_repository_watches_nothing() {
    let r = TestRepo::new();
    let outside = r.base().join("outside");
    let w = Watched::new();
    let id = w.fx.project(&outside);
    let bus = w.fx.bus();
    let baseline = bus.subscriber_count();
    let lease = w.git(&id, None);
    assert_eq!(lease.watching().work_tree, None);
    assert_eq!(bus.subscriber_count(), baseline);
}

/// 「저장소 없음」 은 창끼리 나눠 갖지 않는다. 같은 프로젝트를 두 창이 열어 둘 다 `workTree: null` 을 받은 뒤 `git init` 하면, 다른
/// 창이 옛 스트림을 든 채로도 다시 연 창은 저장소를 받는다(창이 다시 여는 때는 apps/desktop/src/git/git-status.ts).
#[test]
fn a_window_reopening_after_git_init_gets_the_repository_while_another_still_has_none() {
    let r = TestRepo::new();
    let outside = r.base().join("outside");
    let w = Watched::new();
    let id = w.fx.project(&outside);
    let a = w.git(&id, None);
    let b = w.git(&id, None);
    assert_eq!(a.watching().work_tree, None);
    assert_eq!(b.watching().work_tree, None);

    r.git_in(&outside, &["init", "--quiet"]);
    drop(a);
    let again = w.git(&id, None);
    assert_eq!(
        again.watching().work_tree,
        Some(crate::status::display(&outside))
    );
    assert_eq!(w.git_watch.watched(), 1);
    // 옛 「없음」 임대를 놓아도 새 감시는 그대로다.
    drop(b);
    assert_eq!(w.git_watch.watched(), 1);
    drop(again);
    assert_eq!(w.git_watch.watched(), 0);
}

/// 미신뢰 저장소에 심은 프로그램(`core.fsmonitor` · `filter.<x>.clean` · `diff.external` · `diff.<x>.textconv`, include 한 파일의
/// 필터)이 `git.status` · `git.refs` · `git.watch` 어느 경로에서도 돌지 않는다. 신뢰하면 `git.status` 가 clean 필터를 부른다 —
/// 위의 「안 돈다」 가 신뢰로 갈린다(gix 가 그 파일의 내용을 필터에 넣어 비교한다).
#[test]
fn planted_programs_do_not_run_on_status_refs_or_watch_while_untrusted() {
    use std::os::unix::fs::PermissionsExt;
    let r = TestRepo::with_commit();
    let marks = r.base().join("marks");
    let bin = r.base().join("planted");
    fs::create_dir_all(&marks).unwrap();
    fs::create_dir_all(&bin).unwrap();
    let script = |name: &str, body: &str| {
        let path = bin.join(name);
        fs::write(
            &path,
            format!(
                "#!/bin/sh\necho \"$*\" >> '{}'\n{body}\n",
                marks.join(name).display()
            ),
        )
        .unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path.display().to_string()
    };
    let fsmonitor = script("fsmonitor", "exit 1");
    let clean = script("clean", "cat");
    let external = script("external", "exit 0");
    let textconv = script("textconv", "cat \"$1\"");
    let included = script("included", "cat");
    for (key, value) in [
        ("core.fsmonitor", fsmonitor.as_str()),
        ("filter.mark.clean", clean.as_str()),
        ("diff.external", external.as_str()),
        ("diff.mark.textconv", textconv.as_str()),
        ("include.path", "planted.cfg"),
    ] {
        r.git(&["config", key, value]);
    }
    fs::write(
        r.root().join(".git/planted.cfg"),
        format!("[filter \"inc\"]\n\tclean = {included}\n"),
    )
    .unwrap();
    fs::write(
        r.root().join(".git/info/attributes"),
        "* filter=mark diff=mark\n",
    )
    .unwrap();
    let names = || -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(&marks)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    };
    // 크기는 같고 내용 · 시각이 다르다 — status 가 내용을 (필터를 거쳐) 다시 읽어야 가른다.
    let dirty = |body: &str| {
        let path = r.root().join("README.md");
        fs::write(&path, body).unwrap();
        fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(std::time::SystemTime::now() + Duration::from_secs(5))
            .unwrap();
    };
    dirty("hellp\n");
    // 여기서부터 시험도 git CLI 를 띄우지 않는다 — 띄우면 그 git 이 심은 프로그램을 부른다.
    let w = Watched::new();
    let id = w.fx.project(r.root());
    assert!(!w.fx.projects.is_trusted(&id));
    let got = w.fx.status(&id, None);
    assert_eq!(got.entries.len(), 1, "{got:?}");
    assert_eq!(got.entries[0].unstaged, Some(GitChange::Modified));
    w.fx.status(&id, Some(&["README.md"]));
    w.fx.refs(&id, None);
    let _fs = w.fs(&id, None);
    let _git = w.git(&id, None);
    let mut events = Events::new(&w.fx.bus(), &id);
    events.settle();
    dirty("hellq\n");
    events.until("edit", &[&status_for("README.md")]);
    w.fx.status(&id, Some(&["README.md"]));
    assert_eq!(names(), Vec::<String>::new());

    w.fx.projects.set_trust(&id, true).unwrap();
    events.until("trust", &[&any_status]);
    dirty("hellr\n");
    w.fx.status(&id, None);
    assert!(names().contains(&"clean".to_owned()), "{:?}", names());
}

/// 미신뢰면 사람의 전역 설정(`~/.gitconfig`)에 정의된 필터 드라이버도 돌지 않는다 — 어느 파일에 어느 드라이버를 돌릴지는 저장소의
/// `.gitattributes` 가 고르기 때문이다(`filter.lfs.*` 처럼 흔히 전역에 있다). 신뢰하면 git 처럼 돈다.
///
/// 시험 프로세스의 `HOME` 을 임시 폴더로 바꾼다 — nextest 는 시험마다 프로세스가 따로라 다른 시험에 새지 않는다.
#[test]
fn global_filter_drivers_do_not_run_on_status_while_untrusted() {
    use std::os::unix::fs::PermissionsExt;
    let r = TestRepo::with_commit();
    let home = r.base().join("home");
    let marks = r.base().join("marks");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&marks).unwrap();
    let script = |name: &str, body: &str| {
        let path = r.base().join(name);
        fs::write(
            &path,
            format!(
                "#!/bin/sh\necho \"$*\" >> '{}'\n{body}\n",
                marks.join(name).display()
            ),
        )
        .unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path.display().to_string()
    };
    let clean = script("clean", "cat");
    // 긴 수명 필터(`process`)는 핸드셰이크 없이 끝난다 — 띄워지기만 해도 표식이 남는다.
    let process = script("process", "exit 1");
    fs::write(
        home.join(".gitconfig"),
        format!("[filter \"x\"]\n\tclean = {clean}\n[filter \"y\"]\n\tprocess = {process}\n"),
    )
    .unwrap();
    // SAFETY: 아직 이 프로세스에 다른 스레드가 없다(Fixture · 런타임을 만들기 전이다).
    unsafe {
        std::env::set_var("HOME", &home);
        std::env::set_var("XDG_CONFIG_HOME", home.join(".config"));
        std::env::remove_var("GIT_CONFIG_GLOBAL");
    }
    r.write("x.txt", "x\n");
    r.write("y.txt", "y\n");
    r.write(".gitattributes", "x.txt filter=x\ny.txt filter=y\n");
    r.commit_all("attributes");
    let names = || -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(&marks)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    };
    // 크기는 같고 내용 · 시각이 다르다 — status 가 내용을 (필터를 거쳐) 다시 읽어야 가른다.
    let dirty = |rel: &str, body: &str| {
        let path = r.root().join(rel);
        fs::write(&path, body).unwrap();
        fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(std::time::SystemTime::now() + Duration::from_secs(5))
            .unwrap();
    };
    dirty("x.txt", "X\n");
    dirty("y.txt", "Y\n");
    let fx = Fx::new();
    let id = fx.project(r.root());
    assert!(!fx.projects.is_trusted(&id));
    let opened = fx.git.open(&id, None).unwrap().unwrap();
    // 사람의 전역 설정은 읽혔다(드라이버가 정의돼 있다) — 아래 「안 돈다」 가 설정을 못 읽어서가 아니다.
    assert!(
        opened
            .repo
            .config_snapshot()
            .trusted_program("filter.x.clean")
            .unwrap()
            .is_some()
    );
    let got = fx.status(&id, None);
    assert_eq!(got.entries.len(), 2, "{got:?}");
    fx.status(&id, Some(&["x.txt", "y.txt"]));
    assert_eq!(names(), Vec::<String>::new());

    fx.projects.set_trust(&id, true).unwrap();
    dirty("x.txt", "Z\n");
    let _ = fx.try_status(&id, None, Some(&["x.txt"]));
    let _ = fx.try_status(&id, None, Some(&["y.txt"]));
    assert_eq!(names(), ["clean", "process"]);
}

/// 서브모듈 안에서 커밋하면(서브모듈이 가리키는 커밋이 바뀜, `.M sub`) 상태 이벤트가 온다 — 서브모듈의 git 디렉터리는 공유 git
/// 디렉터리의 `modules/sub` 다.
#[test]
fn a_commit_inside_a_submodule_reports_the_status() {
    let library = TestRepo::with_commit();
    let r = TestRepo::with_commit();
    r.git(&[
        "-c",
        "protocol.file.allow=always",
        "submodule",
        "add",
        "--quiet",
        library.root().to_str().unwrap(),
        "sub",
    ]);
    r.commit_all("add the submodule");
    let sub = r.root().join("sub");
    assert!(r.root().join(".git/modules/sub/HEAD").is_file());
    let w = Watched::new();
    let id = w.fx.project(r.root());
    let _fs = w.fs(&id, None);
    let _git = w.git(&id, None);
    let mut events = Events::new(&w.fx.bus(), &id);
    events.settle();
    assert_same_status(&w.fx, &r, &id, None);

    let started = Instant::now();
    r.git_in(
        &sub,
        &["commit", "--quiet", "--allow-empty", "-m", "inside"],
    );
    let at = events.until("commit inside the submodule", &[&status_for("sub")]);
    latency("commit inside the submodule", started, &at);
    events.settle();
    assert_same_status(&w.fx, &r, &id, None);
    assert!(
        w.fx.status(&id, None)
            .entries
            .iter()
            .any(|e| e.path == "sub" && e.unstaged == Some(GitChange::Modified))
    );

    // 분리된 HEAD 로 옮겨도(브랜치는 그대로) 온다.
    let started = Instant::now();
    r.git_in(&sub, &["checkout", "--quiet", "--detach", "HEAD~1"]);
    let at = events.until("checkout inside the submodule", &[&status_for("sub")]);
    latency("checkout inside the submodule", started, &at);
    events.settle();
    assert_same_status(&w.fx, &r, &id, None);
}
