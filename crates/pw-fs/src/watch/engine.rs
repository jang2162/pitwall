//! 루트 하나의 감시(결정 M1-D7). OS 감시를 걸고, 받은 이벤트를 디바운스 창으로 묶어 `fs.changed` 로 낸다.
//!
//! - [`Strategy::Recursive`](macOS FSEvents · Windows ReadDirectoryChangesW): 루트 하나에 네이티브 재귀 감시. ignore 된 폴더의
//!   변경도 온다(파일 트리가 흐리게 보이는 항목도 갱신한다).
//! - [`Strategy::Folders`](Linux inotify): ignore 되지 않은 폴더마다 비재귀 감시를 건다(`node_modules` · `target` 까지 걸면
//!   `max_user_watches` 를 쉽게 넘는다). 새 폴더가 생기면 그 아래를 걸어 더 건다. 한도를 넘으면(`MaxFilesWatch`) 그 뒤의 폴더는
//!   [`Options::poll_interval`](5초) 폴링으로 보고, [`WatchMode::Polling`] 으로 바꾸고 경고를 낸다(루트마다 Host 에서 한 번).
//! - 심링크는 따르지 않는다: 재귀 감시는 실제 경로 트리만 보고, 폴더별 감시는 걷기가 링크를 따르지 않으며 폴링도
//!   `follow_symlinks(false)` 다. 그래서 루트 밖을 가리키는 링크 너머의 변경은 오지 않는다(결정 M1-D8). 링크 항목 자체의 생성 · 삭제는
//!   루트 안의 변경이라 온다.
//! - 디바운스는 고정 창이다: 쉬던 중 첫 이벤트가 창을 열고 [`Options::debounce`](75 ms) 뒤에 그동안 모은 것을 한 번에 낸다. 「조용해질
//!   때까지 기다리기」 로 하면 에이전트가 계속 쓰는 동안 아무것도 나가지 않는다.
//!
//! 감시 스레드 하나가 이벤트 받기 · 감시 추가 · 내기를 모두 한다. [`Engine`] 을 버리면 그 스레드가 멈추고 OS 감시를 푼다.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use notify::event::{CreateKind, ModifyKind, RenameMode};
use notify::{EventKind, PollWatcher, RecommendedWatcher, RecursiveMode, Watcher as _};
use pw_core::bus::{EventBus, Scope};
use pw_core::notification::{Level, Notification};

use super::batch::{Batch, MAX_PATHS};
use super::{FsChanged, TOPIC_CHANGED, WatchMode};

/// 디바운스 창(결정 M1-D7).
pub const DEBOUNCE: Duration = Duration::from_millis(75);
/// OS 감시 한도를 넘은 폴더의 폴링 간격(결정 M1-D7).
pub const POLL_INTERVAL: Duration = Duration::from_secs(5);
/// OS 가 주는 「생성」 이 그 창에서 실제로 생긴 것인가. FSEvents 는 아니다 — 같은 경로의 최근 기록(1초 남짓)을 플래그로 합쳐 주므로,
/// 방금 만든 파일을 지우거나 이름을 바꾸면 그 이벤트에도 생성이 붙는다. 그것을 믿고 「창 안에서 생겼다 사라진 경로」 를 빼면 진짜
/// 삭제를 잃는다. 그래서 macOS 에서는 빼지 않는다(에이전트식 쓰기의 임시 파일 경로가 `removed` 로 함께 온다, [`super::batch`]).
pub const CREATIONS_ARE_EXACT: bool = !cfg!(target_os = "macos");
/// 한도 초과 경고의 Fluent 메시지 id(`project.ftl`).
pub const LIMIT_MESSAGE: &str = "fs-watch-limit";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strategy {
    Recursive,
    Folders,
}

impl Strategy {
    /// 이 플랫폼의 방식. inotify 는 재귀 감시가 없어 폴더마다 건다.
    pub fn for_this_platform() -> Self {
        if cfg!(target_os = "linux") {
            Strategy::Folders
        } else {
            Strategy::Recursive
        }
    }
}

#[derive(Debug, Clone)]
pub struct Options {
    pub strategy: Strategy,
    pub debounce: Duration,
    pub poll_interval: Duration,
    /// 창 하나에 담는 경로의 상한([`MAX_PATHS`]).
    pub max_paths: usize,
    /// 시험: 이만큼 폴더를 OS 감시에 건 뒤에는 한도 초과(`MaxFilesWatch`)를 낸다.
    pub fault_limit: Option<usize>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            strategy: Strategy::for_this_platform(),
            debounce: DEBOUNCE,
            poll_interval: POLL_INTERVAL,
            max_paths: MAX_PATHS,
            fault_limit: None,
        }
    }
}

/// 내는 곳. 루트를 보는 엔진마다 하나다.
#[derive(Clone)]
pub struct Sink {
    pub bus: EventBus,
    pub project_id: String,
    pub worktree: Option<String>,
    /// 한도 초과 경고를 이미 낸 루트(Host 하나에 루트마다 한 번).
    pub warned: Arc<Mutex<HashSet<PathBuf>>>,
}

enum Msg {
    Raw(notify::Result<notify::Event>),
    Stop,
}

/// 루트 하나의 감시. 버리면 멈춘다.
pub struct Engine {
    tx: mpsc::Sender<Msg>,
    status: tokio::sync::watch::Receiver<WatchMode>,
}

impl Engine {
    /// OS 감시를 다 건 뒤에 돌아온다 — 돌아온 뒤의 변경은 놓치지 않는다. 폴더별 감시는 여기서 루트를 한 번 걷는다.
    pub fn start(root: PathBuf, sink: Sink, options: Options) -> notify::Result<Self> {
        let (tx, rx) = mpsc::channel();
        let (status_tx, status) = tokio::sync::watch::channel(WatchMode::Recursive);
        let mut backend = Backend::new(root, tx.clone(), &options)?;
        backend.start()?;
        let mut worker = Worker {
            backend,
            sink,
            options,
            status: status_tx,
        };
        worker.publish_mode();
        std::thread::Builder::new()
            .name("pw-fs-watch".into())
            .spawn(move || worker.run(&rx))
            .map_err(notify::Error::io)?;
        Ok(Self { tx, status })
    }

    pub fn status(&self) -> tokio::sync::watch::Receiver<WatchMode> {
        self.status.clone()
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        // 스레드는 기다리지 않는다 — 여는 창에서 걷기 중이면 그만큼 늦게 멈출 뿐이다. OS 감시의 콜백도 같은 채널을 들고 있어
        // 채널이 닫히기만 기다리면 멈추지 않는다.
        let _ = self.tx.send(Msg::Stop);
    }
}

struct Worker {
    backend: Backend,
    sink: Sink,
    options: Options,
    status: tokio::sync::watch::Sender<WatchMode>,
}

impl Worker {
    fn run(&mut self, rx: &mpsc::Receiver<Msg>) {
        loop {
            let first = match rx.recv() {
                Ok(Msg::Raw(raw)) => raw,
                Ok(Msg::Stop) | Err(_) => return,
            };
            let root = self.backend.root.clone();
            let mut batch = Batch::new(&root, self.options.max_paths, CREATIONS_ARE_EXACT);
            self.take(first, &mut batch);
            let deadline = Instant::now() + self.options.debounce;
            loop {
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    break;
                }
                match rx.recv_timeout(left) {
                    Ok(Msg::Raw(raw)) => self.take(raw, &mut batch),
                    Ok(Msg::Stop) | Err(RecvTimeoutError::Disconnected) => return,
                    Err(RecvTimeoutError::Timeout) => break,
                }
            }
            if let Some(parts) = batch.finish() {
                let changed = FsChanged::from_parts(&root, self.sink.worktree.as_deref(), parts);
                let scope = Scope::Project(self.sink.project_id.clone());
                if let Err(e) = self.sink.bus.publish(scope, TOPIC_CHANGED, &changed) {
                    tracing::warn!(target: "pw_fs", %e, "cannot publish fs.changed");
                }
            }
        }
    }

    fn take(&mut self, raw: notify::Result<notify::Event>, batch: &mut Batch<'_>) {
        match raw {
            Ok(event) => {
                if self.backend.strategy == Strategy::Folders {
                    let limited = self.backend.limited;
                    self.backend.follow(&event);
                    if event.need_rescan() {
                        // 큐가 넘친 사이 생긴 폴더를 놓쳤을 수 있다.
                        self.backend.add_tree(&self.backend.root.clone());
                    }
                    if self.backend.limited != limited {
                        self.publish_mode();
                    }
                }
                batch.add(&event);
            }
            Err(e) => {
                tracing::warn!(target: "pw_fs", root = %self.backend.root.display(), %e, "file watch error");
                batch.lost();
            }
        }
    }

    /// 지금 방식을 알리고, 처음 폴링으로 바뀌었으면 경고를 낸다.
    fn publish_mode(&mut self) {
        let mode = self.backend.mode();
        self.status.send_replace(mode);
        if mode != WatchMode::Polling {
            return;
        }
        let root = &self.backend.root;
        let first = self
            .sink
            .warned
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(root.clone());
        if first {
            tracing::warn!(target: "pw_fs", root = %root.display(), "file watch limit reached; polling the rest");
            Notification::new(Level::Warning, LIMIT_MESSAGE)
                .arg("root", root.display().to_string())
                .arg("seconds", self.options.poll_interval.as_secs().to_string())
                .body(root.display().to_string())
                .show(&self.sink.bus, Scope::Project(self.sink.project_id.clone()));
        }
    }
}

/// OS 감시들. 폴더별 감시는 건 폴더를 기억한다.
struct Backend {
    root: PathBuf,
    strategy: Strategy,
    tx: mpsc::Sender<Msg>,
    native: Option<RecommendedWatcher>,
    poll: Option<PollWatcher>,
    poll_interval: Duration,
    native_dirs: HashSet<PathBuf>,
    polled_dirs: HashSet<PathBuf>,
    /// OS 감시 한도를 넘었다. 이후의 폴더는 모두 폴링이다.
    limited: bool,
    fault_limit: Option<usize>,
}

impl Backend {
    fn new(root: PathBuf, tx: mpsc::Sender<Msg>, options: &Options) -> notify::Result<Self> {
        let native = match notify::recommended_watcher(sender(&tx)) {
            Ok(w) => Some(w),
            // inotify 인스턴스 한도(`max_user_instances`)도 한도다 — 모두 폴링으로 본다.
            Err(e) if options.strategy == Strategy::Folders => {
                tracing::warn!(target: "pw_fs", %e, "cannot create an inotify watcher");
                None
            }
            Err(e) => return Err(e),
        };
        Ok(Self {
            root,
            strategy: options.strategy,
            limited: native.is_none(),
            tx,
            native,
            poll: None,
            poll_interval: options.poll_interval,
            native_dirs: HashSet::new(),
            polled_dirs: HashSet::new(),
            fault_limit: options.fault_limit,
        })
    }

    fn start(&mut self) -> notify::Result<()> {
        match self.strategy {
            Strategy::Recursive => {
                let root = self.root.clone();
                self.native
                    .as_mut()
                    .expect("recursive strategy always has a native watcher")
                    .watch(&root, RecursiveMode::Recursive)
            }
            Strategy::Folders => {
                let root = self.root.clone();
                self.add_tree(&root);
                Ok(())
            }
        }
    }

    fn mode(&self) -> WatchMode {
        match (self.strategy, self.limited) {
            (Strategy::Recursive, _) => WatchMode::Recursive,
            (Strategy::Folders, false) => WatchMode::Folders,
            (Strategy::Folders, true) => WatchMode::Polling,
        }
    }

    /// 폴더별 감시: 새로 생긴(또는 이름이 바뀌어 들어온) 폴더에 감시를 더하고, 사라진 폴더를 표에서 뺀다.
    fn follow(&mut self, event: &notify::Event) {
        let arrived = matches!(
            event.kind,
            EventKind::Create(CreateKind::Folder | CreateKind::Any)
                | EventKind::Modify(ModifyKind::Name(
                    RenameMode::To | RenameMode::Both | RenameMode::Any
                ))
        );
        for path in &event.paths {
            let is_dir = std::fs::symlink_metadata(path).is_ok_and(|m| m.is_dir());
            if arrived && is_dir && !self.tracks(path) && visible(&self.root, path) {
                self.add_tree(path);
            } else if !is_dir && self.tracks(path) {
                self.forget_tree(path);
            }
        }
    }

    fn tracks(&self, dir: &Path) -> bool {
        self.native_dirs.contains(dir) || self.polled_dirs.contains(dir)
    }

    /// `start` 와 그 아래의 ignore 되지 않은 폴더(`start` 는 ignore 판정을 이미 지났다)에 감시를 건다.
    fn add_tree(&mut self, start: &Path) {
        for dir in visible_dirs(start) {
            self.add_dir(dir);
        }
    }

    fn add_dir(&mut self, dir: PathBuf) {
        if self.tracks(&dir) {
            return;
        }
        if !self.limited {
            match self.watch_native(&dir) {
                Ok(()) => {
                    self.native_dirs.insert(dir);
                    return;
                }
                Err(e) if matches!(e.kind, notify::ErrorKind::MaxFilesWatch) => self.limited = true,
                // 걷는 사이 지워진 폴더 등. 그 폴더의 변경은 부모 감시가 알린다.
                Err(_) => return,
            }
        }
        let interval = self.poll_interval;
        let tx = self.tx.clone();
        let poll = match &mut self.poll {
            Some(p) => p,
            None => {
                let config = notify::Config::default()
                    .with_poll_interval(interval)
                    .with_follow_symlinks(false);
                match PollWatcher::new(sender(&tx), config) {
                    Ok(p) => self.poll.insert(p),
                    Err(e) => {
                        tracing::warn!(target: "pw_fs", %e, "cannot create a polling watcher");
                        return;
                    }
                }
            }
        };
        if poll.watch(&dir, RecursiveMode::NonRecursive).is_ok() {
            self.polled_dirs.insert(dir);
        }
    }

    fn watch_native(&mut self, dir: &Path) -> notify::Result<()> {
        if self
            .fault_limit
            .is_some_and(|limit| self.native_dirs.len() >= limit)
        {
            return Err(notify::Error::new(notify::ErrorKind::MaxFilesWatch));
        }
        match &mut self.native {
            Some(w) => w.watch(dir, RecursiveMode::NonRecursive),
            None => Err(notify::Error::new(notify::ErrorKind::MaxFilesWatch)),
        }
    }

    /// 사라진 폴더와 그 아래를 표에서 뺀다. OS 감시는 inotify 가 스스로 풀므로 푸는 실패는 무시한다.
    fn forget_tree(&mut self, gone: &Path) {
        let native: Vec<PathBuf> = self
            .native_dirs
            .iter()
            .filter(|d| d.starts_with(gone))
            .cloned()
            .collect();
        for dir in native {
            self.native_dirs.remove(&dir);
            if let Some(w) = &mut self.native {
                let _ = w.unwatch(&dir);
            }
        }
        let polled: Vec<PathBuf> = self
            .polled_dirs
            .iter()
            .filter(|d| d.starts_with(gone))
            .cloned()
            .collect();
        for dir in polled {
            self.polled_dirs.remove(&dir);
            if let Some(p) = &mut self.poll {
                let _ = p.unwatch(&dir);
            }
        }
    }
}

fn sender(tx: &mpsc::Sender<Msg>) -> impl Fn(notify::Result<notify::Event>) + Send + 'static {
    let tx = Mutex::new(tx.clone());
    move |raw| {
        let _ = tx
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .send(Msg::Raw(raw));
    }
}

/// ignore 걷기의 공통 설정. `fs.list` 의 판정(`crate::list`)과 같다: 숨김 파일은 ignore 가 아니고, 링크를 따르지 않는다. `.git/`
/// 은 감시하지 않는다(M2 의 몫).
fn walker(start: &Path) -> ignore::WalkBuilder {
    let mut walk = ignore::WalkBuilder::new(start);
    walk.hidden(false)
        .follow_links(false)
        .filter_entry(|e| e.file_name() != ".git");
    walk
}

/// `start` 와 그 아래의 ignore 되지 않은 폴더. 시작점은 판정하지 않는다. 부모 폴더의 ignore 파일도 읽는다(`parents`).
fn visible_dirs(start: &Path) -> Vec<PathBuf> {
    walker(start)
        .build()
        .flatten()
        .filter(|e| e.file_type().is_some_and(|t| t.is_dir()))
        .map(ignore::DirEntry::into_path)
        .collect()
}

/// `dir`(루트 안) 이 ignore 되지 않는가. 루트에서 `dir` 까지만 내려가 걷는다 — 무시된 폴더 안이면 닿지 않는다.
fn visible(root: &Path, dir: &Path) -> bool {
    let Ok(rel) = dir.strip_prefix(root) else {
        return false;
    };
    let depth = rel.components().count();
    let target = dir.to_path_buf();
    let mut walk = walker(root);
    walk.max_depth(Some(depth))
        .filter_entry(move |e| e.file_name() != ".git" && target.starts_with(e.path()));
    walk.build().flatten().any(|e| e.path() == dir)
}

/// 시험이 OS 감시 상태를 들여다본다.
#[cfg(test)]
pub(crate) fn watched_dirs_of(
    options: &Options,
    root: &Path,
) -> std::collections::BTreeMap<PathBuf, bool> {
    let (tx, _rx) = mpsc::channel();
    let mut backend = Backend::new(root.to_path_buf(), tx, options).unwrap();
    backend.start().unwrap();
    backend
        .native_dirs
        .iter()
        .map(|d| (d.clone(), true))
        .chain(backend.polled_dirs.iter().map(|d| (d.clone(), false)))
        .collect()
}
