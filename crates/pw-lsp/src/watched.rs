//! 파일 감시 전달(15-m4-plan.md M4-05b): 서버가 등록한 `workspace/didChangeWatchedFiles` 글롭에 맞는 바뀜만 그 서버에 알린다.
//!
//! **어디서 오나** — 증분은 버스의 `fs.changed`(종류 `Created` · `Modified` · `Removed` 그대로 — `LspManager::fs_changed`), 다시
//! 알리기는 파일 목록 인덱스의 다시 만든 목록(`pw_fs::index::FileListObserver::rebuilt` — 처음 · `rescan` · 버스 큐 넘침 · 취소
//! 뒤). `rescan` 이벤트 자체에는 경로가 없어 「그 폴더의 무엇이 바뀌었나」 를 모른다 — 다시 만든 목록(ignore · `.git` 이 빠진 것)의
//! 파일 중 글롭에 맞는 것을 모두 `Changed` 로 다시 알린다. 손수 걷지 않는다 — ignore 파일을 안전하게 읽는 규칙(`pw_fs` 의
//! `ignore_rules`)과 같은 제외를 다시 세우지 않으려는 것이다(메인 판정 2026-11-02).
//!
//! **무엇을 보내나**(보낼 때마다 다시 판정한다 — 그사이 등록이 지워졌거나 문서가 열렸으면 그것을 따른다):
//! - 서버의 지금 등록에 맞는 경로만. 글롭은 LSP 3.17 꼴(`*` · `?` · `**` · `{a,b}` · `[…]` · `[!…]`, `*` 는 `/` 를 넘지 않는다).
//!   `RelativePattern` 은 `baseUri` 기준 상대 경로에, 글 꼴 글롭은 절대 경로에 맞추고 — VS Code 의 규칙 — 맞지 않으면 작업 공간 루트
//!   기준 상대 경로에도 맞춘다(Neovim 처럼 글 꼴을 상대로 쓰는 서버). 종류 마스크(`Create` 1 · `Change` 2 · `Delete` 4, 없으면 7)
//!   밖의 바뀜은 보내지 않는다.
//! - 프로젝트(활성 워크트리면 그 워크트리) 루트 안의 경로만 — `fs.changed` 의 루트가 인스턴스의 루트와 같아야 하고, 경로가 그 안이며
//!   `..` 가 없어야 한다. 다중 루트 프로젝트의 다른 루트는 언어 서버의 범위(`lsp_grant` 의 루트) 밖이다.
//! - 그 서버에 열어 둔 문서의 경로는 뺀다 — 그 문서는 `didChange` 가 맡는다(저장의 원자 쓰기가 만드는 바뀜도 여기서 빠진다).
//! - 신뢰한 프로젝트의 떠 있는(`is_running`) 서버에만. 보내기 묶음마다 신뢰를 다시 본다.
//! - 등록이 모두 지워지면 멈춘다 — 다시 알리기를 보내던 중이어도 남은 묶음을 버린다.
//!
//! **상한**([`WatchLimits`]): 서버마다 글롭 수 · 글롭 길이, 쌓아 둘 바뀜 수(넘친 것은 버리고 경고 — 다음 다시 만든 목록이 메운다),
//! 다시 알릴 목록의 파일 수, 묶음 하나의 글롭 평가 수(경로 × 글롭), 알림 하나의 바뀜 수와 알림 사이 간격(속도 제한). 큰 다시 알리기는
//! 여러 알림으로 나뉘어 간격을 두고 간다.

use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::time::Duration;

use globset::{GlobBuilder, GlobMatcher};
use lsp_types::Registration;
use pw_fs::index::{FileFlags, FileListObserver};
use serde_json::{Value, json};
use tokio::sync::Notify;

use crate::server::LanguageServer;
use crate::uri::{file_uri, uri_path};

/// 감시 등록의 메서드.
pub const METHOD: &str = "workspace/didChangeWatchedFiles";

/// 파일 감시 전달의 상한. 시험이 줄인다.
#[derive(Debug, Clone)]
pub struct WatchLimits {
    /// 서버 하나가 등록한 글롭 중 쓰는 수. 넘는 것은 버리고 경고.
    pub max_watchers: usize,
    /// 글롭 하나의 바이트 수. 넘는 글롭은 버린다(글롭 하나의 정규식이 커지는 것을 막는다).
    pub max_pattern_bytes: usize,
    /// 보내기 전에 쌓아 둘 바뀜 수. 넘친 것은 버리고 경고한다.
    pub max_pending: usize,
    /// 다시 알리기에서 볼 파일 수(다시 만든 목록의 앞에서부터).
    pub max_rescan_files: usize,
    /// 보내기 묶음 하나에서 하는 글롭 평가(경로 × 글롭)의 상한. 넘으면 그 뒤 경로는 버리고 경고.
    pub max_evaluations: usize,
    /// 알림 하나에 담는 바뀜 수.
    pub batch_events: usize,
    /// 서버 하나에 보내는 알림 사이의 가장 짧은 간격.
    pub batch_interval: Duration,
}

impl Default for WatchLimits {
    fn default() -> Self {
        Self {
            max_watchers: 1_000,
            max_pattern_bytes: 4_096,
            max_pending: 65_536,
            max_rescan_files: 200_000,
            max_evaluations: 2_000_000,
            batch_events: 1_000,
            batch_interval: Duration::from_millis(100),
        }
    }
}

/// LSP `FileChangeType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    Created = 1,
    Changed = 2,
    Deleted = 3,
}

impl Change {
    /// `WatchKind` 의 비트.
    fn bit(self) -> u8 {
        match self {
            Self::Created => 1,
            Self::Changed => 2,
            Self::Deleted => 4,
        }
    }
}

impl From<pw_fs::watch::FsChangeKind> for Change {
    fn from(kind: pw_fs::watch::FsChangeKind) -> Self {
        match kind {
            pw_fs::watch::FsChangeKind::Created => Self::Created,
            pw_fs::watch::FsChangeKind::Modified => Self::Changed,
            pw_fs::watch::FsChangeKind::Removed => Self::Deleted,
        }
    }
}

/// 글롭 하나.
#[derive(Debug)]
struct Watcher {
    glob: GlobMatcher,
    /// `RelativePattern` 의 기준 폴더. 없으면 글 꼴(절대 경로 · 작업 공간 루트 기준).
    base: Option<PathBuf>,
    kind: u8,
}

/// 서버의 등록을 읽어 만든 글롭들.
#[derive(Debug, Default)]
pub(crate) struct Watchers {
    list: Vec<Watcher>,
}

/// 경로를 글롭에 맞출 글(`/` 구분).
fn slashed(path: &Path) -> String {
    let text = path.to_string_lossy();
    if cfg!(windows) {
        text.replace('\\', "/")
    } else {
        text.into_owned()
    }
}

impl Watchers {
    /// 서버의 `workspace/didChangeWatchedFiles` 등록들에서. 읽지 못하는 글롭 · 상한 밖은 버리고 경고한다.
    pub(crate) fn compile(
        server: &str,
        registrations: &[Registration],
        limits: &WatchLimits,
    ) -> Self {
        let mut list = Vec::new();
        let mut dropped = 0usize;
        for registration in registrations.iter().filter(|r| r.method == METHOD) {
            let watchers = registration
                .register_options
                .as_ref()
                .and_then(|o| o.get("watchers"))
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            for watcher in watchers {
                if list.len() >= limits.max_watchers {
                    dropped += 1;
                    continue;
                }
                match Self::one(&watcher, limits) {
                    Ok(w) => list.push(w),
                    Err(why) => {
                        tracing::warn!(target: "pw_lsp", %server, registration = %registration.id, %why, "ignoring a file watcher");
                    }
                }
            }
        }
        if dropped > 0 {
            tracing::warn!(target: "pw_lsp", %server, dropped, max = limits.max_watchers, "too many file watchers; ignoring the rest");
        }
        Self { list }
    }

    fn one(watcher: &Value, limits: &WatchLimits) -> Result<Watcher, String> {
        let kind = watcher
            .get("kind")
            .and_then(Value::as_u64)
            .map_or(7, |k| (k & 7) as u8);
        let pattern = watcher.get("globPattern").ok_or("no globPattern")?;
        let (pattern, base) = match pattern {
            Value::String(p) => (p.clone(), None),
            Value::Object(rel) => {
                let base = rel.get("baseUri").ok_or("no baseUri")?;
                // `WorkspaceFolder`(`{ uri, name }`) 이거나 URI 글.
                let uri = base
                    .get("uri")
                    .unwrap_or(base)
                    .as_str()
                    .ok_or("baseUri is not a URI")?;
                let base =
                    uri_path(uri).ok_or_else(|| format!("baseUri {uri} is not a file URI"))?;
                let pattern = rel
                    .get("pattern")
                    .and_then(Value::as_str)
                    .ok_or("no pattern")?;
                (pattern.to_owned(), Some(base))
            }
            _ => return Err("globPattern is neither a string nor a RelativePattern".into()),
        };
        if pattern.len() > limits.max_pattern_bytes {
            return Err(format!(
                "the pattern has {} bytes (max {})",
                pattern.len(),
                limits.max_pattern_bytes
            ));
        }
        let pattern = if cfg!(windows) {
            pattern.replace('\\', "/")
        } else {
            pattern
        };
        let glob = GlobBuilder::new(&pattern)
            .literal_separator(true)
            .build()
            .map_err(|e| e.to_string())?
            .compile_matcher();
        Ok(Watcher { glob, base, kind })
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    /// `path` 의 바뀜 `change` 를 알릴 글롭이 있는가. `evaluations` 에 평가 수를 더한다.
    fn wants(
        &self,
        path: &Path,
        workspace_root: &Path,
        change: Change,
        evaluations: &mut usize,
    ) -> bool {
        let absolute = slashed(path);
        let under_root = path.strip_prefix(workspace_root).ok().map(slashed);
        self.list.iter().any(|w| {
            if w.kind & change.bit() == 0 {
                return false;
            }
            *evaluations += 1;
            match &w.base {
                Some(base) => path
                    .strip_prefix(base)
                    .is_ok_and(|rel| w.glob.is_match(slashed(rel))),
                None => {
                    w.glob.is_match(&absolute)
                        || under_root.as_ref().is_some_and(|rel| w.glob.is_match(rel))
                }
            }
        })
    }
}

/// `path` 가 `root` 안인가(글자로 — `..` 가 끼면 아니다).
fn inside(root: &Path, path: &Path) -> bool {
    path.is_absolute()
        && path.starts_with(root)
        && !path.components().any(|c| matches!(c, Component::ParentDir))
}

/// 보낼 바뀜 하나.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Event {
    pub path: PathBuf,
    pub change: Change,
}

#[derive(Default)]
struct Pending {
    events: Vec<Event>,
    /// 다시 만든 목록(루트 기준 `/` 구분 상대 경로). 마지막 것만 쓴다.
    rescan: Option<Vec<Arc<str>>>,
    overflowed: bool,
}

/// 떠 있는 서버 하나로 가는 바뀜의 자리(서버가 `ready` 일 때 만들고 내려놓을 때 닫는다).
pub(crate) struct Feed {
    pending: Mutex<Pending>,
    wake: Notify,
    closed: AtomicBool,
    max_pending: usize,
}

impl Feed {
    pub(crate) fn new(max_pending: usize) -> Arc<Self> {
        Arc::new(Self {
            pending: Mutex::default(),
            wake: Notify::new(),
            closed: AtomicBool::new(false),
            max_pending,
        })
    }

    fn pending(&self) -> std::sync::MutexGuard<'_, Pending> {
        self.pending.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// 증분 바뀜을 쌓는다. 상한을 넘친 것은 버린다(다음 다시 알리기가 메운다).
    pub(crate) fn push(&self, events: &[Event]) {
        {
            let mut pending = self.pending();
            let room = self.max_pending.saturating_sub(pending.events.len());
            if events.len() > room {
                if !pending.overflowed {
                    tracing::warn!(target: "pw_lsp", dropped = events.len() - room, "too many file changes waiting for a language server; dropping some");
                }
                pending.overflowed = true;
            }
            pending.events.extend(events.iter().take(room).cloned());
        }
        self.wake.notify_one();
    }

    /// 다시 만든 목록으로 다시 알린다.
    pub(crate) fn rescan(&self, files: Vec<Arc<str>>) {
        self.pending().rescan = Some(files);
        self.wake.notify_one();
    }

    pub(crate) fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        self.wake.notify_one();
    }

    pub(crate) fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    fn take(&self) -> Option<Pending> {
        let mut pending = self.pending();
        if pending.events.is_empty() && pending.rescan.is_none() {
            return None;
        }
        Some(std::mem::take(&mut *pending))
    }
}

/// 파일 목록 인덱스에서 다시 만든 목록을 받아 그 서버의 자리로(`FileIndex::observe_queued` — 서버마다 붙이고 떼며, 그 관찰자의
/// 스레드에서 불린다).
pub(crate) struct FeedObserver {
    pub project_id: String,
    pub root: PathBuf,
    pub feed: Weak<Feed>,
}

impl FileListObserver for FeedObserver {
    fn rebuilt(&self, project_id: &str, root: &Path, files: Vec<(Arc<str>, FileFlags)>) {
        if project_id != self.project_id || root != self.root {
            return;
        }
        if let Some(feed) = self.feed.upgrade() {
            feed.rescan(files.into_iter().map(|(rel, _)| rel).collect());
        }
    }

    // 증분은 버스의 `fs.changed` 로 받는다 — 목록의 바뀜에는 종류(만듦 · 바뀜)가 없다.
    fn changed(&self, _: &str, _: &Path, _: Vec<String>, _: Vec<(Arc<str>, FileFlags)>) {}

    fn dropped(&self, _: &str, _: &Path) {}
}

/// 보내도 되는가.
pub(crate) enum Gate {
    /// 보낸다 — 그 서버에 열어 둔 문서의 URI(빼고 보낸다).
    Open(BTreeSet<String>),
    /// 지금은 보내지 않는다(미신뢰) — 이 묶음을 버린다.
    Hold,
    /// 그 인스턴스가 다른 시도로 넘어갔거나 떠 있지 않다 — 끝낸다.
    Gone,
}

/// 보내는 쪽이 관리자에게 묻는 것(`LspManager` 가 구현한다).
pub(crate) trait FeedHost: Send + Sync + 'static {
    fn gate(&self) -> impl std::future::Future<Output = Gate> + Send;
}

/// 자리 하나를 비우는 태스크: 쌓인 바뀜을 지금 등록 · 열린 문서 · 신뢰로 걸러 묶음으로 보낸다. 자리가 닫히거나 서버가 사라지면 끝난다.
pub(crate) async fn run(
    host: impl FeedHost,
    server_id: String,
    scope_root: PathBuf,
    workspace_root: PathBuf,
    server: Weak<LanguageServer>,
    feed: Arc<Feed>,
    limits: WatchLimits,
) {
    let mut cache: Option<(String, Arc<Watchers>)> = None;
    let mut last_sent: Option<tokio::time::Instant> = None;
    loop {
        let notified = feed.wake.notified();
        if feed.is_closed() {
            return;
        }
        let Some(batch) = feed.take() else {
            notified.await;
            continue;
        };
        let Some(lsp) = server.upgrade() else {
            return;
        };
        let watchers = current(&lsp, &server_id, &mut cache, &limits);
        drop(lsp);
        if watchers.is_empty() {
            continue;
        }
        let events = {
            let (watchers, scope_root, workspace_root, limits) = (
                Arc::clone(&watchers),
                scope_root.clone(),
                workspace_root.clone(),
                limits.clone(),
            );
            let server_id = server_id.clone();
            tokio::task::spawn_blocking(move || {
                select(
                    &server_id,
                    batch,
                    &watchers,
                    &scope_root,
                    &workspace_root,
                    &limits,
                )
            })
            .await
            .unwrap_or_default()
        };
        let mut events = events;
        let mut matched_with = watchers;
        let mut at = 0;
        while at < events.len() {
            if let Some(last) = last_sent {
                tokio::time::sleep_until(last + limits.batch_interval).await;
            }
            if feed.is_closed() {
                return;
            }
            let Some(lsp) = server.upgrade() else {
                return;
            };
            // 그사이 등록이 바뀌었으면 남은 것을 지금 등록으로 다시 거른다(모두 지워졌으면 멈춘다).
            let now = current(&lsp, &server_id, &mut cache, &limits);
            if !Arc::ptr_eq(&now, &matched_with) {
                let mut evaluations = 0;
                let rest = events.split_off(at);
                events.extend(
                    rest.into_iter().filter(|e| {
                        now.wants(&e.path, &workspace_root, e.change, &mut evaluations)
                    }),
                );
                matched_with = Arc::clone(&now);
                if at >= events.len() {
                    break;
                }
            }
            let open = match host.gate().await {
                Gate::Open(open) => open,
                Gate::Hold => break,
                Gate::Gone => return,
            };
            let mut changes = Vec::new();
            while at < events.len() && changes.len() < limits.batch_events.max(1) {
                let event = &events[at];
                at += 1;
                let Some(uri) = file_uri(&event.path) else {
                    continue;
                };
                if open.contains(&uri) {
                    continue;
                }
                changes.push(json!({ "uri": uri, "type": event.change as u8 }));
            }
            if changes.is_empty() {
                continue;
            }
            if lsp
                .notify_raw(METHOD, json!({ "changes": changes }))
                .is_err()
            {
                return;
            }
            last_sent = Some(tokio::time::Instant::now());
        }
    }
}

/// 서버의 지금 등록으로 만든 글롭들(등록이 그대로면 앞서 만든 것).
fn current(
    server: &LanguageServer,
    server_id: &str,
    cache: &mut Option<(String, Arc<Watchers>)>,
    limits: &WatchLimits,
) -> Arc<Watchers> {
    let registrations = server.registrations(METHOD);
    let fingerprint = serde_json::to_string(&registrations).unwrap_or_default();
    if let Some((seen, watchers)) = cache
        && *seen == fingerprint
    {
        return Arc::clone(watchers);
    }
    let watchers = Arc::new(Watchers::compile(server_id, &registrations, limits));
    *cache = Some((fingerprint, Arc::clone(&watchers)));
    watchers
}

/// 쌓인 바뀜과 다시 만든 목록에서 보낼 것을 고른다(블로킹 — 글롭 평가).
fn select(
    server_id: &str,
    batch: Pending,
    watchers: &Watchers,
    scope_root: &Path,
    workspace_root: &Path,
    limits: &WatchLimits,
) -> Vec<Event> {
    let mut evaluations = 0usize;
    let mut out = Vec::new();
    let mut over = false;
    for event in batch.events {
        if evaluations >= limits.max_evaluations {
            over = true;
            break;
        }
        if inside(scope_root, &event.path)
            && watchers.wants(&event.path, workspace_root, event.change, &mut evaluations)
        {
            out.push(event);
        }
    }
    if let Some(files) = batch.rescan {
        if files.len() > limits.max_rescan_files {
            tracing::warn!(target: "pw_lsp", server = %server_id, files = files.len(), max = limits.max_rescan_files, "the rebuilt file list is too long to announce in full");
        }
        for rel in files.iter().take(limits.max_rescan_files) {
            if evaluations >= limits.max_evaluations {
                over = true;
                break;
            }
            let path = rel
                .split('/')
                .fold(scope_root.to_path_buf(), |p, part| p.join(part));
            if inside(scope_root, &path)
                && watchers.wants(&path, workspace_root, Change::Changed, &mut evaluations)
            {
                out.push(Event {
                    path,
                    change: Change::Changed,
                });
            }
        }
    }
    if over {
        tracing::warn!(target: "pw_lsp", server = %server_id, max = limits.max_evaluations, "too many glob evaluations for one batch; dropping the rest of the file changes");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registration(watchers: Value) -> Registration {
        Registration {
            id: "w".into(),
            method: METHOD.into(),
            register_options: Some(json!({ "watchers": watchers })),
        }
    }

    fn wants(watchers: &Watchers, path: &str, change: Change) -> bool {
        let mut n = 0;
        watchers.wants(Path::new(path), Path::new("/w/root"), change, &mut n)
    }

    /// LSP 3.17 글롭 꼴: `**` · `{a,b}` · `[…]` · `[!…]` · `?`, `*` 는 `/` 를 넘지 않는다. 글 꼴은 절대 경로나 작업 공간 루트 기준
    /// 상대 경로에, `RelativePattern` 은 `baseUri` 기준에 맞춘다(`WorkspaceFolder` 꼴의 `baseUri` 도). 종류 마스크를 지킨다.
    #[cfg(unix)]
    #[test]
    fn lsp_globs_relative_patterns_and_kind_masks() {
        let w = Watchers::compile(
            "s",
            &[registration(json!([
                { "globPattern": "**/*.{rs,toml}" },
                { "globPattern": "src/?.[ch]" },
                { "globPattern": "**/x[!0-9].md", "kind": 1 },
                { "globPattern": { "baseUri": "file:///w/root/sub", "pattern": "*.json" }, "kind": 4 },
                { "globPattern": { "baseUri": { "uri": "file:///w/root/ws", "name": "ws" }, "pattern": "**/*.lock" } },
                { "globPattern": "/w/root/abs/*.txt" },
            ]))],
            &WatchLimits::default(),
        );
        assert_eq!(w.list.len(), 6);
        assert!(wants(&w, "/w/root/a/b/c.rs", Change::Changed));
        assert!(wants(&w, "/w/root/Cargo.toml", Change::Created));
        assert!(!wants(&w, "/w/root/a.rsx", Change::Changed));
        // 작업 공간 루트 기준 상대 경로에도 맞춘다. `*` · `?` 는 `/` 를 넘지 않는다.
        assert!(wants(&w, "/w/root/src/a.c", Change::Changed));
        assert!(!wants(&w, "/w/root/src/ab.c", Change::Changed));
        assert!(!wants(&w, "/w/root/src/d/a.c", Change::Changed));
        // 종류 마스크: `Create` 만.
        assert!(wants(&w, "/w/root/d/xa.md", Change::Created));
        assert!(!wants(&w, "/w/root/d/xa.md", Change::Changed));
        assert!(!wants(&w, "/w/root/d/x1.md", Change::Created));
        // `RelativePattern`: 그 폴더 바로 아래의 json 지움만.
        assert!(wants(&w, "/w/root/sub/a.json", Change::Deleted));
        assert!(!wants(&w, "/w/root/sub/a.json", Change::Created));
        assert!(!wants(&w, "/w/root/sub/deep/a.json", Change::Deleted));
        assert!(!wants(&w, "/w/root/a.json", Change::Deleted));
        assert!(wants(&w, "/w/root/ws/a/Cargo.lock", Change::Changed));
        assert!(!wants(&w, "/w/root/Cargo.lock", Change::Changed));
        // 절대 경로 글롭.
        assert!(wants(&w, "/w/root/abs/n.txt", Change::Changed));
        assert!(!wants(&w, "/w/root/abs/d/n.txt", Change::Changed));
    }

    /// 상한: 서버마다 글롭 수 · 글롭 길이. 읽지 못하는 글롭은 버리고 나머지는 쓴다. 다른 메서드의 등록은 보지 않는다.
    #[cfg(unix)]
    #[test]
    fn watcher_limits_and_bad_patterns() {
        let limits = WatchLimits {
            max_watchers: 2,
            max_pattern_bytes: 12,
            ..WatchLimits::default()
        };
        let w = Watchers::compile(
            "s",
            &[
                registration(json!([
                    { "globPattern": "src/[ab.rs" },
                    { "globPattern": "**/very/long/pattern/*.rs" },
                    { "globPattern": "**/*.rs" },
                    { "globPattern": { "baseUri": "untitled:x", "pattern": "*" } },
                    { "globPattern": "**/*.ts" },
                    { "globPattern": "**/*.md" },
                ])),
                Registration {
                    id: "other".into(),
                    method: "textDocument/formatting".into(),
                    register_options: Some(json!({ "watchers": [{ "globPattern": "**/*.py" }] })),
                },
            ],
            &limits,
        );
        assert!(wants(&w, "/w/root/a.rs", Change::Changed));
        assert!(wants(&w, "/w/root/a.ts", Change::Changed));
        assert!(
            !wants(&w, "/w/root/a.md", Change::Changed),
            "over max_watchers"
        );
        assert!(!wants(
            &w,
            "/w/root/very/long/pattern/a.rs.x",
            Change::Changed
        ));
        assert!(!wants(&w, "/w/root/a.py", Change::Changed));
    }

    /// 루트 밖 · `..` 가 낀 경로 · 상대 경로는 보내지 않는다. 평가 상한을 넘으면 그 뒤는 버린다. 다시 만든 목록은 `Changed` 로.
    #[cfg(unix)]
    #[test]
    fn selection_keeps_to_the_root_and_the_evaluation_budget() {
        let w = Watchers::compile(
            "s",
            &[registration(json!([{ "globPattern": "**/*.rs" }]))],
            &WatchLimits::default(),
        );
        let root = Path::new("/w/root");
        let event = |p: &str, change| Event {
            path: PathBuf::from(p),
            change,
        };
        let batch = Pending {
            events: vec![
                event("/w/root/a.rs", Change::Created),
                event("/w/other/b.rs", Change::Created),
                event("/w/root/../other/c.rs", Change::Created),
                event("w/root/d.rs", Change::Created),
                event("/w/root/e.txt", Change::Created),
            ],
            rescan: Some(vec!["src/f.rs".into(), "g.txt".into()]),
            overflowed: false,
        };
        let out = select("s", batch, &w, root, root, &WatchLimits::default());
        assert_eq!(
            out,
            [
                event("/w/root/a.rs", Change::Created),
                event("/w/root/src/f.rs", Change::Changed),
            ]
        );
        let limits = WatchLimits {
            max_evaluations: 3,
            ..WatchLimits::default()
        };
        let batch = Pending {
            events: (0..10)
                .map(|n| event(&format!("/w/root/{n}.rs"), Change::Changed))
                .collect(),
            ..Pending::default()
        };
        assert_eq!(select("s", batch, &w, root, root, &limits).len(), 3);
    }
}
