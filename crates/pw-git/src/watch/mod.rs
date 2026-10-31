//! 저장소 감시(`docs/design/13-m2-plan.md` M2-02): 창이 [`GitWatchMethod`](`git.watch`) 스트림을 여는 동안 저장소의 바뀜을 프로젝트
//! 범위 이벤트 [`TOPIC_STATUS_CHANGED`](`git.statusChanged`) · [`TOPIC_REFS_CHANGED`](`git.refsChanged`)로 낸다. 창은 받으면
//! `git.status`(바뀐 경로만) · `git.refs` · `git.repo` 를 다시 읽는다.
//!
//! - **작업 트리**는 따로 감시하지 않고 M1-06 의 `fs.changed`(창의 `fs.watch` 가 여는 감시)를 따라간다. 바뀐 경로를 저장소 기준으로
//!   옮겨 `git.statusChanged { paths }` 로, `rescan` · 놓친 이벤트 · `.gitignore` · `.gitattributes` 변경은 전체(`paths: null`)로
//!   낸다. 프로젝트 루트가 저장소의 하위 폴더면 루트 밖(저장소 안)의 변경은 오지 않는다.
//! - **git 디렉터리**(`HEAD` · `index` · `refs/` · `packed-refs` · 진행 중 작업의 표지 — 연결 워크트리면 그 git 디렉터리와 공유 git
//!   디렉터리 둘 다)는 Host 안에서 OS 감시로 따로 본다. 그 디렉터리는 프로젝트 루트 밖일 수 있다 — 감시는 무엇이 바뀌었는지
//!   가르는 데만 쓰고 경로 · 내용을 RPC 로 내보내지 않는다(결정 M1-D8 의 ignore 파일과 같은 규칙, 루트 규칙 A 위반이 아니다).
//!   가르는 규칙은 [`classify`].
//! - 디바운스는 M1-06 과 같은 창([`pw_fs::watch::engine::DEBOUNCE`])이다. 창 하나에 모인 것을 한 번씩 낸다.
//! - 같은 (프로젝트, 활성 워크트리)를 보는 창이 여럿이면 감시는 하나다(임대 수를 센다). 마지막 임대가 풀리면(스트림이 닫힘 — 창이
//!   닫히거나 활성 워크트리를 바꿔 다시 열면) 놓는다.
//! - 감시는 연 저장소(신뢰를 든 [`crate::Opened`])를 들고 있다. 그 프로젝트의 신뢰가 바뀌면(`project.changed` 의 `trust`) 다시
//!   열고 두 이벤트를 전체로 낸다 — 신뢰하면 저장소의 필터 드라이버가 돌아 status 결과가 달라질 수 있다.
//! - 저장소가 없으면(`workTree: null`) 아무것도 보지 않는다. 나중에 `git init` 해도 따라가지 않는다 — 창이 스트림을 다시 연다
//!   (apps/desktop/src/git/git-status.ts). 그래서 「없음」 은 표에 넣지 않고 열 때마다 다시 찾는다 — 넣어 두면 같은 프로젝트를 보는
//!   다른 창이 스트림을 든 동안 다시 열어도 옛 「없음」 을 받는다.

pub mod classify;

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use notify::{RecursiveMode, Watcher};
use pw_core::bus::{EventBus, Filter, Received, Scope, Subscription};
use pw_core::registry::{ChangeKind, ProjectChanged, TOPIC_CHANGED as TOPIC_PROJECT_CHANGED};
use pw_fs::watch::{FsChanged, TOPIC_CHANGED as TOPIC_FS_CHANGED};
use pw_rpc::{Router, RpcError, rpc_method};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::methods::GitRepoParams;
use crate::status::{display, paths};
use crate::{Git, Opened};
use classify::{Dirs, Effect};

/// 작업 트리 상태가 바뀌었다. 내용은 [`GitStatusChanged`].
pub const TOPIC_STATUS_CHANGED: &str = "git.statusChanged";
/// ref · HEAD · 진행 중 작업이 바뀌었다. 내용은 [`GitRefsChanged`].
pub const TOPIC_REFS_CHANGED: &str = "git.refsChanged";

/// 이보다 많은 경로가 한 창에 모이면 전체로 낸다.
const MAX_PATHS: usize = 1000;

/// [`TOPIC_STATUS_CHANGED`] 의 내용.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitStatusChanged {
    /// 감시를 연 창의 활성 워크트리(`git.watch` 의 `worktree`) — 같은 프로젝트의 다른 워크트리 창은 무시한다.
    pub worktree: Option<String>,
    /// 바뀐 경로(저장소 작업 트리 기준, `/` 구분, 이름순). `null` 이면 전체를 다시 읽는다.
    pub paths: Option<Vec<String>>,
}

/// [`TOPIC_REFS_CHANGED`] 의 내용.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitRefsChanged {
    pub worktree: Option<String>,
}

/// `git.watch` 의 항목: 감시를 다 건 뒤 한 번.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitWatching {
    /// 감시하는 저장소의 작업 트리 꼭대기. `null` 이면 저장소가 없어 아무것도 보지 않는다.
    pub work_tree: Option<String>,
}

rpc_method! {
    /// 프로젝트(활성 워크트리)의 저장소를 감시한다. 스트림이 열려 있는 동안 바뀜이 프로젝트 범위 이벤트 `git.statusChanged` ·
    /// `git.refsChanged` 로 나가고, 닫으면 놓는다. 첫 항목이 오면 그 뒤의 바뀜은 놓치지 않는다. 작업 트리의 바뀜은 같은 창의
    /// `fs.watch` 가 열려 있어야 온다.
    pub GitWatchMethod: "git.watch" {
        params: GitRepoParams,
        stream: GitWatching,
        tier: Read,
        roles: [Owner],
    }
}

type Key = (String, Option<String>);

struct Slot {
    refs: usize,
    work_tree: String,
    /// 버리면 감시가 멈춘다.
    _engine: Engine,
}

struct Inner {
    git: Arc<Git>,
    debounce: Duration,
    slots: Mutex<HashMap<Key, Slot>>,
}

impl Inner {
    fn slots(&self) -> MutexGuard<'_, HashMap<Key, Slot>> {
        self.slots.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// 감시 서비스. 복제해도 같은 표를 가리킨다.
#[derive(Clone)]
pub struct GitWatch {
    inner: Arc<Inner>,
}

impl GitWatch {
    pub fn new(git: Arc<Git>) -> Self {
        Self::with_debounce(git, pw_fs::watch::engine::DEBOUNCE)
    }

    pub fn with_debounce(git: Arc<Git>, debounce: Duration) -> Self {
        Self {
            inner: Arc::new(Inner {
                git,
                debounce,
                slots: Mutex::default(),
            }),
        }
    }

    /// 지금 감시하는 (프로젝트, 활성 워크트리) 수.
    pub fn watched(&self) -> usize {
        self.inner.slots().len()
    }

    /// 감시한다. 돌려준 임대를 버리면 놓는다. OS 감시를 다 건 뒤에 돌아온다(블로킹 — 저장소를 연다). tokio 런타임 안에서 부른다.
    pub fn acquire(&self, params: &GitRepoParams) -> Result<WatchLease, RpcError> {
        let key = (params.project_id.clone(), params.worktree.clone());
        if let Some(work_tree) = self.retain(&key) {
            return Ok(self.lease(Some(key), Some(work_tree)));
        }
        let host = self.inner.git.host()?;
        let opened = self
            .inner
            .git
            .open(&params.project_id, params.worktree.as_deref())?;
        // 저장소가 없으면 표에 넣지 않는다(머리 주석).
        let Some(opened) = opened else {
            return Ok(self.lease(None, None));
        };
        let work_tree = display(&opened.target.work_tree);
        let engine = Engine::start(
            Arc::clone(&self.inner.git),
            host.bus.clone(),
            opened,
            params.worktree.clone(),
            self.inner.debounce,
        )?;
        let work_tree = {
            let mut slots = self.inner.slots();
            // 그 사이 다른 창이 같은 것을 걸었으면 그것을 쓰고 만든 것은 버린다.
            let slot = slots.entry(key.clone()).or_insert(Slot {
                refs: 0,
                work_tree,
                _engine: engine,
            });
            slot.refs += 1;
            slot.work_tree.clone()
        };
        Ok(self.lease(Some(key), Some(work_tree)))
    }

    fn retain(&self, key: &Key) -> Option<String> {
        let mut slots = self.inner.slots();
        let slot = slots.get_mut(key)?;
        slot.refs += 1;
        Some(slot.work_tree.clone())
    }

    fn lease(&self, key: Option<Key>, work_tree: Option<String>) -> WatchLease {
        WatchLease {
            inner: Arc::clone(&self.inner),
            key,
            work_tree,
        }
    }
}

/// 감시 하나의 임대. 버리면 그 감시를 쓰는 마지막 임대일 때 놓는다.
pub struct WatchLease {
    inner: Arc<Inner>,
    /// 표의 키. 저장소가 없으면 표에 없어 `None`.
    key: Option<Key>,
    work_tree: Option<String>,
}

impl WatchLease {
    pub fn watching(&self) -> GitWatching {
        GitWatching {
            work_tree: self.work_tree.clone(),
        }
    }
}

impl Drop for WatchLease {
    fn drop(&mut self) {
        let Some(key) = &self.key else { return };
        // 엔진은 잠금 밖에서 버린다.
        let stopped = {
            let mut slots = self.inner.slots();
            match slots.get_mut(key) {
                Some(slot) if slot.refs > 1 => {
                    slot.refs -= 1;
                    None
                }
                Some(_) => slots.remove(key),
                None => None,
            }
        };
        drop(stopped);
    }
}

/// 감시 하나. 버리면 멈춘다(작업이 곧 끝나며 OS 감시 · 구독을 놓는다).
struct Engine {
    stop: CancellationToken,
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}

impl Engine {
    fn start(
        git: Arc<Git>,
        bus: EventBus,
        opened: Opened,
        worktree: Option<String>,
        debounce: Duration,
    ) -> Result<Self, RpcError> {
        // 구독은 감시를 걸기 전에 — 돌려준 뒤의 바뀜을 놓치지 않는다.
        let project_id = opened.target.project_id.clone();
        let fs_changes = bus.subscribe(
            Filter::all()
                .scope(Scope::Project(project_id.clone()))
                .topics([TOPIC_FS_CHANGED]),
        );
        let projects = bus.subscribe(
            Filter::all()
                .scope(Scope::Host)
                .topics([TOPIC_PROJECT_CHANGED]),
        );
        let (tx, raw) = mpsc::unbounded_channel();
        let (dirs, watcher) = watch_git_dirs(&opened, tx.clone())?;
        let stop = CancellationToken::new();
        let worker = Worker {
            git,
            bus,
            project_id,
            worktree,
            work_tree: normalized(&opened.target.work_tree),
            opened,
            dirs,
            watcher,
            tx,
            debounce,
            pending: Pending::default(),
        };
        tokio::spawn(worker.run(raw, fs_changes, projects, stop.clone()));
        Ok(Self { stop })
    }
}

/// 저장소의 git 디렉터리를 재귀로 감시한다. 연결 워크트리의 git 디렉터리는 공유 git 디렉터리 안에 있어 하나로 덮인다.
fn watch_git_dirs(
    opened: &Opened,
    tx: mpsc::UnboundedSender<notify::Result<notify::Event>>,
) -> Result<(Dirs, notify::RecommendedWatcher), RpcError> {
    let dirs = Dirs {
        git_dir: normalized(opened.repo.git_dir()),
        common_dir: normalized(opened.repo.common_dir()),
    };
    let mut watcher = notify::recommended_watcher(move |event| {
        let _ = tx.send(event);
    })
    .map_err(|e| RpcError::internal(format!("cannot watch the git directory: {e}")))?;
    let mut roots = vec![dirs.common_dir.clone()];
    if !dirs.git_dir.starts_with(&dirs.common_dir) {
        roots.push(dirs.git_dir.clone());
    }
    for root in &roots {
        watcher
            .watch(root, RecursiveMode::Recursive)
            .map_err(|e| RpcError::internal(format!("cannot watch {}: {e}", root.display())))?;
    }
    Ok((dirs, watcher))
}

/// 레지스트리 · `fs.changed` 와 같은 정규화(실제 경로)를 거친 경로. 못 구하면 그대로.
fn normalized(path: &Path) -> PathBuf {
    pw_core::registry::path::normalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// 창 하나 동안 모은 것.
#[derive(Debug, Default)]
struct Pending {
    /// `None`: 상태 바뀜 없음. `Some(None)`: 전체. `Some(Some(경로들))`: 그 경로만.
    status: Option<Option<BTreeSet<String>>>,
    refs: bool,
    /// 바뀐 ref(공유 git 디렉터리 기준 이름) — HEAD 가 그 브랜치면 상태 전체.
    changed_refs: BTreeSet<String>,
    packed_refs: bool,
}

impl Pending {
    fn is_empty(&self) -> bool {
        self.status.is_none() && !self.refs
    }

    fn all_status(&mut self) {
        self.status = Some(None);
    }

    fn status_path(&mut self, path: String) {
        match &mut self.status {
            Some(None) => {}
            Some(Some(paths)) => {
                paths.insert(path);
                if paths.len() > MAX_PATHS {
                    self.status = Some(None);
                }
            }
            None => self.status = Some(Some(BTreeSet::from([path]))),
        }
    }

    /// git 디렉터리의 OS 이벤트 하나. 읽기(`Access` — inotify 는 `open` · 닫기도 낸다)는 바뀐 것이 아니라 버린다: 그러지 않으면
    /// 다른 작업 트리의 `git add` 가 `config` · `refs/` 를 읽기만 해도 이 작업 트리의 ref · 상태가 바뀐 것이 되고, 그 알림에 창이
    /// 다시 읽는 것이 또 이벤트가 된다(`pw_fs::watch` 도 같은 까닭으로 버린다).
    fn event(&mut self, dirs: &Dirs, event: &notify::Event) {
        if event.need_rescan() {
            self.effect(Effect::Both);
        }
        if matches!(event.kind, notify::EventKind::Access(_)) {
            return;
        }
        for path in &event.paths {
            self.effect(classify::classify(dirs, path));
        }
    }

    fn effect(&mut self, effect: Effect) {
        match effect {
            Effect::None => {}
            Effect::Status => self.all_status(),
            Effect::Refs => self.refs = true,
            Effect::Both => {
                self.all_status();
                self.refs = true;
            }
            Effect::Ref(name) => {
                self.refs = true;
                self.changed_refs.insert(name);
            }
            Effect::PackedRefs => {
                self.refs = true;
                self.packed_refs = true;
            }
        }
    }
}

struct Worker {
    git: Arc<Git>,
    bus: EventBus,
    project_id: String,
    worktree: Option<String>,
    /// 저장소 작업 트리(정규화).
    work_tree: PathBuf,
    opened: Opened,
    dirs: Dirs,
    watcher: notify::RecommendedWatcher,
    tx: mpsc::UnboundedSender<notify::Result<notify::Event>>,
    debounce: Duration,
    pending: Pending,
}

impl Worker {
    async fn run(
        mut self,
        mut raw: mpsc::UnboundedReceiver<notify::Result<notify::Event>>,
        mut fs_changes: Subscription,
        mut projects: Subscription,
        stop: CancellationToken,
    ) {
        let mut deadline: Option<tokio::time::Instant> = None;
        loop {
            let flush_at = deadline;
            tokio::select! {
                () = stop.cancelled() => break,
                Some(event) = raw.recv() => self.git_event(event),
                Some(received) = fs_changes.recv() => self.fs_event(&received),
                Some(received) = projects.recv() => self.project_event(&received),
                () = sleep_until(flush_at) => {
                    deadline = None;
                    self.flush();
                    continue;
                }
            }
            if deadline.is_none() && !self.pending.is_empty() {
                deadline = Some(tokio::time::Instant::now() + self.debounce);
            }
        }
        tracing::debug!(target: "pw_git", project = self.project_id, "git watch stopped");
    }

    fn git_event(&mut self, event: notify::Result<notify::Event>) {
        match event {
            Ok(event) => self.pending.event(&self.dirs, &event),
            Err(e) => {
                tracing::warn!(target: "pw_git", project = self.project_id, error = %e, "git directory watch error; reading everything again");
                self.pending.effect(Effect::Both);
            }
        }
    }

    fn fs_event(&mut self, received: &Received) {
        let changed = match received {
            Received::Lagged(_) => {
                self.pending.all_status();
                return;
            }
            Received::Event(event) => {
                match serde_json::from_value::<FsChanged>(event.payload.clone()) {
                    Ok(changed) => changed,
                    Err(e) => {
                        tracing::warn!(target: "pw_git", error = %e, "unreadable fs.changed");
                        return;
                    }
                }
            }
        };
        if changed.worktree != self.worktree {
            return;
        }
        let root = PathBuf::from(&changed.root);
        if !root.starts_with(&self.work_tree) {
            return;
        }
        if changed.rescan {
            self.pending.all_status();
            return;
        }
        for path in &changed.paths {
            let Some(rel) = paths::relative(&self.work_tree, Path::new(path)) else {
                continue;
            };
            let name = rel.rsplit('/').next().unwrap_or(&rel);
            if rel.is_empty() || name == ".gitignore" || name == ".gitattributes" {
                self.pending.all_status();
            } else {
                self.pending.status_path(rel);
            }
        }
    }

    fn project_event(&mut self, received: &Received) {
        let trust_changed = match received {
            Received::Lagged(_) => true,
            Received::Event(event) => {
                serde_json::from_value::<ProjectChanged>(event.payload.clone())
                    .is_ok_and(|c| c.kind == ChangeKind::Trust)
            }
        };
        if !trust_changed || self.git.is_current(&self.opened) {
            return;
        }
        match self.git.open(&self.project_id, self.worktree.as_deref()) {
            Ok(Some(opened)) => {
                let dirs = Dirs {
                    git_dir: normalized(opened.repo.git_dir()),
                    common_dir: normalized(opened.repo.common_dir()),
                };
                if dirs != self.dirs {
                    match watch_git_dirs(&opened, self.tx.clone()) {
                        Ok((dirs, watcher)) => {
                            self.dirs = dirs;
                            self.watcher = watcher;
                        }
                        Err(e) => {
                            tracing::warn!(target: "pw_git", project = self.project_id, error = %e.message, "cannot watch the reopened repository")
                        }
                    }
                }
                self.opened = opened;
            }
            Ok(None) => {
                tracing::warn!(target: "pw_git", project = self.project_id, "the repository is gone after a trust change");
            }
            Err(e) => {
                tracing::warn!(target: "pw_git", project = self.project_id, error = %e.message, "cannot reopen the repository after a trust change");
            }
        }
        self.pending.effect(Effect::Both);
    }

    fn flush(&mut self) {
        let mut pending = std::mem::take(&mut self.pending);
        if pending.refs && (pending.packed_refs || !pending.changed_refs.is_empty()) {
            // 지금 브랜치의 끝이 움직였으면 HEAD↔index 가 바뀐다(`git commit` 은 HEAD 파일을 건드리지 않는다).
            let head = classify::head_target(&self.dirs.git_dir);
            if pending.packed_refs || head.is_some_and(|h| pending.changed_refs.contains(&h)) {
                pending.all_status();
            }
        }
        let scope = Scope::Project(self.project_id.clone());
        if pending.refs {
            let event = GitRefsChanged {
                worktree: self.worktree.clone(),
            };
            let _ = self.bus.publish(scope.clone(), TOPIC_REFS_CHANGED, &event);
        }
        if let Some(paths) = pending.status {
            let event = GitStatusChanged {
                worktree: self.worktree.clone(),
                paths: paths.map(|p| p.into_iter().collect()),
            };
            let _ = self.bus.publish(scope, TOPIC_STATUS_CHANGED, &event);
        }
    }
}

async fn sleep_until(deadline: Option<tokio::time::Instant>) {
    match deadline {
        Some(at) => tokio::time::sleep_until(at).await,
        None => std::future::pending().await,
    }
}

/// `git.watch` 를 붙인다. 감시를 거는 일(저장소 열기)은 블로킹 스레드에서 한다. 스트림이 취소되거나 세션이 닫히면 pw-rpc 가 이
/// future 를 버리고, 그와 함께 임대가 풀린다.
pub fn register(router: &mut Router, watch: GitWatch) {
    router.stream(GitWatchMethod, move |_ctx, params: GitRepoParams, sink| {
        let watch = watch.clone();
        async move {
            let lease = tokio::task::spawn_blocking(move || watch.acquire(&params))
                .await
                .map_err(|e| RpcError::internal(e.to_string()))??;
            sink.send(lease.watching()).await?;
            std::future::pending::<()>().await;
            drop(lease);
            Ok(())
        }
    });
}

#[cfg(test)]
mod tests {
    use notify::event::{AccessKind, AccessMode, CreateKind, DataChange, ModifyKind};
    use notify::{Event, EventKind};

    use super::*;

    fn linked() -> Dirs {
        Dirs {
            git_dir: PathBuf::from("/r/.git/worktrees/wt"),
            common_dir: PathBuf::from("/r/.git"),
        }
    }

    /// 읽기는 아무것도 바꾸지 않는다 — inotify 는 다른 작업 트리의 `git add` 가 `config` · `refs/` 를 열기만 해도 낸다.
    #[test]
    fn reading_the_git_directory_changes_nothing() {
        let dirs = linked();
        let mut pending = Pending::default();
        for rel in ["config", "refs", "refs/heads/main", "worktrees/wt/HEAD"] {
            for kind in [
                AccessKind::Open(AccessMode::Any),
                AccessKind::Close(AccessMode::Read),
                AccessKind::Read,
            ] {
                let event = Event::new(EventKind::Access(kind)).add_path(dirs.common_dir.join(rel));
                pending.event(&dirs, &event);
            }
        }
        assert!(pending.is_empty(), "{pending:?}");
        // 쓰기는 그대로 받는다.
        let written = Event::new(EventKind::Modify(ModifyKind::Data(DataChange::Any)))
            .add_path(dirs.common_dir.join("config"));
        pending.event(&dirs, &written);
        assert!(pending.refs && pending.status == Some(None));
        let mut pending = Pending::default();
        let created = Event::new(EventKind::Create(CreateKind::File))
            .add_path(dirs.common_dir.join("refs/heads/x"));
        pending.event(&dirs, &created);
        assert!(pending.refs && pending.status.is_none());
    }
}
