//! 파일 감시 전달(15-m4-plan.md M4-05b): 서버가 등록한 `workspace/didChangeWatchedFiles` 글롭에 맞는 바뀜만 그 서버에 알린다.
//!
//! **어디서 오나** — 증분은 버스의 `fs.changed`(종류 `Created` · `Modified` · `Removed` 그대로 — `LspManager::fs_changed`), 다시
//! 알리기는 파일 목록 인덱스가 **다시** 만든 목록(`pw_fs::index::FileListObserver::rebuilt` — `rescan` · 인덱스의 버스 큐 넘침 · 목록이
//! 있던 뒤의 취소 다음 — 그 사이의 바뀜을 놓쳤을 수 있는 때). `rescan` 이벤트 자체에는 경로가 없어 「무엇이 바뀌었나」 를 모른다 —
//! 다시 만든 목록(ignore · `.git` 이 빠진 것)의 파일 중 글롭에 맞는 것을 모두 `Changed` 로 다시 알린다. 루트를 열 때마다의 **첫
//! 목록**(`listed`)은 기준선이라 알리지 않는다 — 서버는 막 디스크를 스스로 읽었다(M4-05b 리뷰 M1). 늦게 붙은 서버(그 루트의 목록이
//! 이미 있음)는 목록을 받지 않는다 — 다음 다시 만든 목록부터 받는다. 손수 걷지 않는다 — ignore 파일을 안전하게 읽는 규칙(`pw_fs`
//! 의 `ignore_rules`)과 같은 제외를 다시 세우지 않으려는 것이다(메인 판정 2026-11-02).
//!
//! **무엇을 보내나**(보낼 묶음마다 다시 판정한다 — 그사이 등록이 바뀌었거나 문서가 열렸으면 그것을 따른다):
//! - 서버의 지금 등록에 맞는 경로만. 글롭은 LSP 3.17 꼴(`*` · `?` · `**` · `{a,b}` · `[…]` · `[!…]`, `*` 는 `/` 를 넘지 않는다).
//!   `RelativePattern` 은 `baseUri` 기준 상대 경로에, 글 꼴 글롭은 절대 경로에 맞추고 — VS Code 의 규칙 — 맞지 않으면 작업 공간 루트
//!   기준 상대 경로에도 맞춘다(Neovim 처럼 글 꼴을 상대로 쓰는 서버). 종류 마스크(`Create` 1 · `Change` 2 · `Delete` 4, 없으면 7)
//!   밖의 바뀜은 보내지 않는다.
//! - 글롭은 (기준 폴더, 종류 마스크)마다 `GlobSet` 하나로 묶어 경로마다 한 번에 맞춘다 — 기준 폴더는 경로의 조상을 표에서 찾는다.
//!   비용이 글롭 수에 거의 비례하지 않는다(rust-analyzer 는 패키지마다 글롭 셋을 등록한다 — 패키지 수백이면 글롭 수백 ~ 수천, 리뷰 H1).
//! - 프로젝트(활성 워크트리면 그 워크트리) 루트 안의 경로만 — `fs.changed` 의 루트가 인스턴스의 루트와 같아야 하고, 경로가 그 안이며
//!   `..` 가 없어야 한다. 다중 루트 프로젝트의 다른 루트, 루트 밖의 경로 의존성(`path = "../shared"`)은 언어 서버의 범위(`lsp_grant`
//!   의 루트) 밖이라 보내지 않는다 — 서버가 감시를 클라이언트에 맡겼으면 그 파일의 바뀜은 서버에 가지 않는다(알려진 한계).
//! - 그 서버에 열어 둔 문서의 경로는 뺀다 — 그 문서는 `didChange` 가 맡는다(저장의 원자 쓰기가 만드는 바뀜도 여기서 빠진다).
//! - 신뢰한 프로젝트의 떠 있는(`is_running`) 서버에만. 보내기 묶음마다 신뢰를 다시 본다.
//! - 등록이 모두 지워지면 멈춘다 — 다시 알리기를 보내던 중이어도 남은 것을 버린다.
//!
//! **상한과 넘칠 때**([`WatchLimits`]) — 맞는 바뀜을 조용히 버리는 길을 두지 않는다(리뷰 H1):
//! - 글롭 수 · 글롭 길이를 넘거나 읽지 못하는 글롭은 버리지 않고 **모두 맞음**으로 둔다(그 기준 폴더 · 종류 안에서) — 넘치게 보내는
//!   것이 빠뜨리는 것보다 낫다(서버는 내용이 같으면 무시한다).
//! - 보내기 전에 쌓아 둘 바뀜 수를 넘치면 그 뒤 것은 버리되, 그 자리에만 그 루트를 **다시 알리게** 한다 — 관리자가 파일 목록
//!   인덱스에 이 자리의 관찰자 하나에만 지금 목록을 청하고(`FileIndex::replay` — 다시 만들지 않는다) 그 목록으로 메운다(지운 파일은
//!   목록에 없어 알리지 못한다). 관리자의 버스 구독이 넘쳐도(`Lagged`) 자리마다 같은 길로 메운다. 버스에 `rescan` 을 내지 않는다 —
//!   내면 그 토픽의 구독자 전부(런타임 → 서버마다 「다시 시작 권함」, git · 트리 · 열린 문서 · 심볼 인덱스)가 움직인다(재리뷰 R1).
//!   글롭 바이트 합([`WatchLimits::max_total_pattern_bytes`])을 넘는 등록도 「모두 맞음」 이다(재리뷰 R3).
//! - 보내는 속도: 알림 하나에 [`WatchLimits::batch_events`] 바뀜, 알림 사이 [`WatchLimits::batch_interval`]. 글롭 맞추기는 블로킹
//!   스레드에서 한 번에 [`WatchLimits::scan_step`] 경로까지 — 큰 다시 알리기도 목록을 앞에서부터 차례로 걷는다(버리지 않는다).

use std::collections::{BTreeSet, HashMap};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError, Weak};
use std::time::Duration;

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use lsp_types::Registration;
use pw_fs::index::{FileFlags, FileListObserver, ObserverId};
use serde_json::{Value, json};
use tokio::sync::Notify;

use crate::server::LanguageServer;
use crate::uri::{file_uri, uri_path};

/// 감시 등록의 메서드.
pub const METHOD: &str = "workspace/didChangeWatchedFiles";

/// 파일 감시 전달의 상한. 시험이 줄인다.
#[derive(Debug, Clone)]
pub struct WatchLimits {
    /// 서버 하나가 등록한 글롭 중 따로 맞추는 수. 넘으면 그 서버는 모든 경로 · 종류를 받는다(경고).
    pub max_watchers: usize,
    /// 글롭 하나의 바이트 수. 넘는 글롭은 그 기준 폴더 · 종류의 모든 경로로 본다(정규식이 커지는 것을 막는다).
    pub max_pattern_bytes: usize,
    /// 서버 하나가 등록한 (쓸 수 있는) 글롭의 바이트 합. 넘으면 그 서버는 모든 경로 · 종류를 받는다(경고) — 컴파일 비용은 글롭 수가
    /// 아니라 정규식 크기를 따른다(긴 글롭 1만 개는 블로킹 스레드를 수십 초 쥔다 · 재리뷰 R3).
    pub max_total_pattern_bytes: usize,
    /// 보내기 전에 쌓아 둘 바뀜 수. 넘친 것은 버리고 그 루트를 다시 알리게 한다.
    pub max_pending: usize,
    /// 블로킹 스레드에서 한 번에 글롭에 맞춰 볼 경로 수(그 사이 런타임 · 다른 서버가 기다리지 않게 나눈다).
    pub scan_step: usize,
    /// 알림 하나에 담는 바뀜 수.
    pub batch_events: usize,
    /// 서버 하나에 보내는 알림 사이의 가장 짧은 간격.
    pub batch_interval: Duration,
}

impl Default for WatchLimits {
    fn default() -> Self {
        Self {
            max_watchers: 10_000,
            max_pattern_bytes: 4_096,
            max_total_pattern_bytes: 256 * 1024,
            max_pending: 65_536,
            scan_step: 20_000,
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

/// (기준 폴더, 종류 마스크)가 같은 글롭들. `set` 이 없으면 모든 경로가 맞는다(넘친 · 읽지 못하는 글롭).
#[derive(Debug)]
struct Group {
    kind: u8,
    set: Option<GlobSet>,
}

impl Group {
    fn hits(&self, bit: u8, text: &str) -> bool {
        self.kind & bit != 0 && self.set.as_ref().is_none_or(|s| s.is_match(text))
    }
}

/// 서버의 등록을 읽어 만든 글롭들.
#[derive(Debug, Default)]
pub(crate) struct Watchers {
    /// 모든 경로가 맞는 종류(글롭 수를 넘었거나 기준 없는 글롭을 읽지 못함).
    everything: u8,
    /// 글 꼴(기준 폴더 없음).
    strings: Vec<Group>,
    /// `RelativePattern` — 기준 폴더마다.
    based: HashMap<PathBuf, Vec<Group>>,
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

/// 등록 옵션의 글롭 하나: (기준 폴더, 종류 마스크, 글롭 글 — 쓸 수 없으면 그 까닭).
type Parsed = (Option<PathBuf>, u8, Result<String, String>);

impl Watchers {
    /// 서버의 `workspace/didChangeWatchedFiles` 등록들에서. 상한 밖 · 읽지 못하는 글롭은 「모두 맞음」 으로 두고 경고한다.
    pub(crate) fn compile(
        server: &str,
        registrations: &[Registration],
        limits: &WatchLimits,
    ) -> Self {
        let mut out = Self::default();
        let mut parsed: Vec<Parsed> = Vec::new();
        let mut total = 0usize;
        for registration in registrations.iter().filter(|r| r.method == METHOD) {
            let watchers = registration
                .register_options
                .as_ref()
                .and_then(|o| o.get("watchers"))
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            total += watchers.len();
            for watcher in &watchers {
                match Self::one(watcher, limits) {
                    Ok(p) => parsed.push(p),
                    // 기준 폴더도 읽지 못했다 — 어느 경로에 맞추려던 것인지 모르니 모두 맞음.
                    Err(why) => {
                        tracing::warn!(target: "pw_lsp", %server, registration = %registration.id, %why, "cannot read a file watcher; sending every change instead");
                        out.everything = 7;
                    }
                }
            }
        }
        if total > limits.max_watchers {
            tracing::warn!(target: "pw_lsp", %server, total, max = limits.max_watchers, "too many file watchers; sending every change instead");
            out.everything = 7;
            return out;
        }
        let bytes: usize = parsed
            .iter()
            .filter_map(|(_, _, glob)| glob.as_ref().ok().map(String::len))
            .sum();
        if bytes > limits.max_total_pattern_bytes {
            tracing::warn!(target: "pw_lsp", %server, bytes, max = limits.max_total_pattern_bytes, "file watcher globs are too large; sending every change instead");
            out.everything = 7;
            return out;
        }
        let mut groups: HashMap<(Option<PathBuf>, u8), (GlobSetBuilder, bool)> = HashMap::new();
        for (base, kind, glob) in parsed {
            let slot = groups
                .entry((base, kind))
                .or_insert_with(|| (GlobSetBuilder::new(), false));
            match glob.and_then(|g| {
                GlobBuilder::new(&g)
                    .literal_separator(true)
                    .build()
                    .map_err(|e| e.to_string())
            }) {
                Ok(glob) => {
                    slot.0.add(glob);
                }
                Err(why) => {
                    tracing::warn!(target: "pw_lsp", %server, %why, "cannot use a file watcher's glob; sending every change under its base instead");
                    slot.1 = true;
                }
            }
        }
        for ((base, kind), (builder, all)) in groups {
            let set = if all {
                None
            } else {
                builder
                    .build()
                    .map_err(|e| {
                        tracing::warn!(target: "pw_lsp", %server, %e, "cannot build the file watcher globs; sending every change under their base instead");
                    })
                    .ok()
            };
            let group = Group { kind, set };
            match base {
                None if group.set.is_none() => out.everything |= kind,
                None => out.strings.push(group),
                Some(base) => out.based.entry(base).or_default().push(group),
            }
        }
        out
    }

    fn one(watcher: &Value, limits: &WatchLimits) -> Result<Parsed, String> {
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
                let Some(pattern) = rel.get("pattern").and_then(Value::as_str) else {
                    return Ok((Some(base), kind, Err("no pattern".into())));
                };
                (pattern.to_owned(), Some(base))
            }
            _ => return Err("globPattern is neither a string nor a RelativePattern".into()),
        };
        let glob = if pattern.len() > limits.max_pattern_bytes {
            Err(format!(
                "the pattern has {} bytes (max {})",
                pattern.len(),
                limits.max_pattern_bytes
            ))
        } else if cfg!(windows) {
            Ok(pattern.replace('\\', "/"))
        } else {
            Ok(pattern)
        };
        Ok((base, kind, glob))
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.everything == 0 && self.strings.is_empty() && self.based.is_empty()
    }

    /// `path` 의 바뀜 `change` 를 알릴 글롭이 있는가.
    fn wants(&self, path: &Path, workspace_root: &Path, change: Change) -> bool {
        let bit = change.bit();
        if self.everything & bit != 0 {
            return true;
        }
        if self.strings.iter().any(|g| g.kind & bit != 0) {
            let absolute = slashed(path);
            let under_root = path.strip_prefix(workspace_root).ok().map(slashed);
            if self.strings.iter().any(|g| {
                g.hits(bit, &absolute) || under_root.as_ref().is_some_and(|rel| g.hits(bit, rel))
            }) {
                return true;
            }
        }
        if self.based.is_empty() {
            return false;
        }
        path.ancestors().skip(1).any(|base| {
            self.based.get(base).is_some_and(|groups| {
                let rel = path.strip_prefix(base).map(slashed).unwrap_or_default();
                groups.iter().any(|g| g.hits(bit, &rel))
            })
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
    rescan: Option<Arc<Vec<Arc<str>>>>,
}

/// 떠 있는 서버 하나로 가는 바뀜의 자리(서버가 `ready` 일 때 만들고 내려놓을 때 닫는다).
pub(crate) struct Feed {
    pending: Mutex<Pending>,
    wake: Notify,
    closed: AtomicBool,
    /// 쌓기가 넘쳐 그 루트의 지금 목록을 다시 청했다 — 그 목록이 올 때까지 다시 청하지 않는다.
    refilling: AtomicBool,
    max_pending: usize,
    /// 파일 목록 인덱스에 붙인 이 자리의 관찰자(`FileIndex::replay` 로 이 자리에만 지금 목록을 다시 청한다).
    observer: OnceLock<ObserverId>,
}

impl Feed {
    pub(crate) fn new(max_pending: usize) -> Arc<Self> {
        Arc::new(Self {
            pending: Mutex::default(),
            wake: Notify::new(),
            closed: AtomicBool::new(false),
            refilling: AtomicBool::new(false),
            max_pending,
            observer: OnceLock::new(),
        })
    }

    fn pending(&self) -> std::sync::MutexGuard<'_, Pending> {
        self.pending.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// 증분 바뀜을 쌓는다. 상한을 넘친 것은 버리고, 이번에 처음 넘쳤으면 `true` — 부른 쪽이 그 루트를 다시 알리게 한다.
    pub(crate) fn push(&self, events: &[Event]) -> bool {
        let overflowed = {
            let mut pending = self.pending();
            let room = self.max_pending.saturating_sub(pending.events.len());
            pending.events.extend(events.iter().take(room).cloned());
            events.len() > room
        };
        self.wake.notify_one();
        overflowed && !self.refilling.swap(true, Ordering::SeqCst)
    }

    /// 다시 청할 수 없었다(인덱스에 그 루트 · 관찰자가 없다) — 다음 넘침에 다시 청한다.
    pub(crate) fn refill_failed(&self) {
        self.refilling.store(false, Ordering::SeqCst);
    }

    pub(crate) fn set_observer(&self, id: ObserverId) {
        let _ = self.observer.set(id);
    }

    pub(crate) fn observer(&self) -> Option<ObserverId> {
        self.observer.get().copied()
    }

    /// 다시 만든 목록(또는 다시 청한 지금 목록)으로 다시 알린다.
    pub(crate) fn rescan(&self, files: Vec<Arc<str>>) {
        self.refilling.store(false, Ordering::SeqCst);
        self.pending().rescan = Some(Arc::new(files));
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
/// 스레드에서 불린다). 제 (프로젝트, 루트)만 받는다(`wants` — 다른 루트의 목록을 복제해 받지 않게, 리뷰 L3).
pub(crate) struct FeedObserver {
    pub project_id: String,
    pub root: PathBuf,
    pub feed: Weak<Feed>,
}

impl FileListObserver for FeedObserver {
    fn wants(&self, project_id: &str, root: &Path) -> bool {
        project_id == self.project_id && root == self.root
    }

    /// 루트를 열 때의 첫 목록은 기준선이다 — 알리지 않는다(리뷰 M1).
    fn listed(&self, _: &str, _: &Path, _: Vec<(Arc<str>, FileFlags)>) {}

    fn rebuilt(&self, project_id: &str, root: &Path, files: Vec<(Arc<str>, FileFlags)>) {
        if !self.wants(project_id, root) {
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

/// 보낼 것: 쌓인 증분 다음 다시 만든 목록 — 앞에서부터 차례로 맞춰 본다.
struct Work {
    events: Vec<Event>,
    at: usize,
    list: Option<Arc<Vec<Arc<str>>>>,
    list_at: usize,
}

impl Work {
    fn done(&self) -> bool {
        self.at >= self.events.len() && self.list.as_ref().is_none_or(|l| self.list_at >= l.len())
    }
}

/// 맞춰 볼 때 쓰는 것(블로킹 스레드로 넘긴다).
struct Scope {
    scope_root: PathBuf,
    workspace_root: PathBuf,
    limits: WatchLimits,
}

/// 다음 알림 하나에 담을 바뀜: 지금 등록 · 열린 문서로 걸러 [`WatchLimits::batch_events`] 개까지, 맞춰 보는 경로는
/// [`WatchLimits::scan_step`] 개까지(블로킹).
fn step(
    work: &mut Work,
    watchers: &Watchers,
    scope: &Scope,
    open: &BTreeSet<String>,
) -> Vec<Value> {
    let limits = &scope.limits;
    let mut changes = Vec::new();
    let mut scanned = 0usize;
    let take = |path: &Path, change: Change, changes: &mut Vec<Value>| {
        if !inside(&scope.scope_root, path) || !watchers.wants(path, &scope.workspace_root, change)
        {
            return;
        }
        if let Some(uri) = file_uri(path)
            && !open.contains(&uri)
        {
            changes.push(json!({ "uri": uri, "type": change as u8 }));
        }
    };
    let full = |changes: &Vec<Value>, scanned: usize| {
        changes.len() >= limits.batch_events.max(1) || scanned >= limits.scan_step.max(1)
    };
    while work.at < work.events.len() && !full(&changes, scanned) {
        let event = &work.events[work.at];
        work.at += 1;
        scanned += 1;
        take(&event.path, event.change, &mut changes);
    }
    if let Some(list) = &work.list {
        while work.list_at < list.len() && !full(&changes, scanned) {
            let rel = &list[work.list_at];
            work.list_at += 1;
            scanned += 1;
            let path = rel
                .split('/')
                .fold(scope.scope_root.clone(), |p, part| p.join(part));
            take(&path, Change::Changed, &mut changes);
        }
    }
    changes
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
    let scope = Arc::new(Scope {
        scope_root,
        workspace_root,
        limits,
    });
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
        let mut work = Work {
            events: batch.events,
            at: 0,
            list: batch.rescan,
            list_at: 0,
        };
        while !work.done() {
            if let Some(last) = last_sent {
                tokio::time::sleep_until(last + scope.limits.batch_interval).await;
            }
            if feed.is_closed() {
                return;
            }
            let open = match host.gate().await {
                Gate::Open(open) => open,
                Gate::Hold => break,
                Gate::Gone => return,
            };
            let Some(lsp) = server.upgrade() else {
                return;
            };
            // 묶음마다 서버의 지금 등록으로 — 그사이 바뀌었으면 남은 것도 새 등록으로 거른다(모두 지워졌으면 멈춘다).
            let registrations = lsp.registrations(METHOD);
            drop(lsp);
            let watchers = current(&server_id, registrations, &mut cache, &scope.limits).await;
            if watchers.is_empty() {
                break;
            }
            let scope = Arc::clone(&scope);
            let Ok((back, changes)) = tokio::task::spawn_blocking(move || {
                let changes = step(&mut work, &watchers, &scope, &open);
                (work, changes)
            })
            .await
            else {
                break;
            };
            work = back;
            if changes.is_empty() {
                continue;
            }
            let Some(lsp) = server.upgrade() else {
                return;
            };
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

/// 서버의 지금 등록으로 만든 글롭들(등록이 그대로면 앞서 만든 것). 컴파일은 블로킹 스레드에서(리뷰 L2).
async fn current(
    server_id: &str,
    registrations: Vec<Registration>,
    cache: &mut Option<(String, Arc<Watchers>)>,
    limits: &WatchLimits,
) -> Arc<Watchers> {
    let fingerprint = serde_json::to_string(&registrations).unwrap_or_default();
    if let Some((seen, watchers)) = cache
        && *seen == fingerprint
    {
        return Arc::clone(watchers);
    }
    let (id, limits) = (server_id.to_owned(), limits.clone());
    let watchers = tokio::task::spawn_blocking(move || {
        Arc::new(Watchers::compile(&id, &registrations, &limits))
    })
    .await
    .unwrap_or_default();
    *cache = Some((fingerprint, Arc::clone(&watchers)));
    watchers
}

// 시험은 유닉스 경로(`/w/root/…`)로 쓴다.
#[cfg(all(test, unix))]
mod tests {
    use std::time::Instant;

    use super::*;

    fn registration(watchers: Value) -> Registration {
        Registration {
            id: "w".into(),
            method: METHOD.into(),
            register_options: Some(json!({ "watchers": watchers })),
        }
    }

    fn wants(watchers: &Watchers, path: &str, change: Change) -> bool {
        watchers.wants(Path::new(path), Path::new("/w/root"), change)
    }

    fn scope() -> Scope {
        Scope {
            scope_root: PathBuf::from("/w/root"),
            workspace_root: PathBuf::from("/w/root"),
            limits: WatchLimits::default(),
        }
    }

    /// 다 보낼 때까지 `step` 을 돌려 보낼 바뀜 전부(경로 끝, 종류).
    fn drain(mut work: Work, watchers: &Watchers, scope: &Scope) -> Vec<(String, u64)> {
        let mut out = Vec::new();
        while !work.done() {
            for c in step(&mut work, watchers, scope, &BTreeSet::new()) {
                let uri = c["uri"].as_str().unwrap();
                out.push((
                    uri.strip_prefix("file:///w/root/")
                        .unwrap_or(uri)
                        .to_owned(),
                    c["type"].as_u64().unwrap(),
                ));
            }
        }
        out
    }

    /// LSP 3.17 글롭 꼴: `**` · `{a,b}` · `[…]` · `[!…]` · `?`, `*` 는 `/` 를 넘지 않는다. 글 꼴은 절대 경로나 작업 공간 루트 기준
    /// 상대 경로에, `RelativePattern` 은 `baseUri` 기준에 맞춘다(`WorkspaceFolder` 꼴의 `baseUri` 도). 종류 마스크를 지킨다.
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
        assert_eq!(w.everything, 0);
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

    /// 상한 · 읽지 못하는 글롭은 버리지 않고 「모두 맞음」 으로 둔다(리뷰 H1): 기준 폴더가 있으면 그 폴더 · 종류 안에서, 기준 없는
    /// 글롭이나 글롭 수 · 글롭 바이트 합(재리뷰 R3)을 넘으면 모든 경로 · 종류. 다른 메서드의 등록은 보지 않는다.
    #[test]
    fn watchers_over_the_limits_or_unreadable_match_everything_they_could() {
        let limits = WatchLimits {
            max_pattern_bytes: 12,
            ..WatchLimits::default()
        };
        let other = Registration {
            id: "other".into(),
            method: "textDocument/formatting".into(),
            register_options: Some(json!({ "watchers": [{ "globPattern": "**/*.py" }] })),
        };
        // 기준 폴더의 글롭을 쓸 수 없으면(길이 · 문법) 그 폴더 · 종류의 모든 경로.
        let w = Watchers::compile(
            "s",
            &[
                registration(json!([
                    { "globPattern": "**/*.rs" },
                    { "globPattern": { "baseUri": "file:///w/root/a", "pattern": "**/very/long/pattern/*.rs" }, "kind": 4 },
                    { "globPattern": { "baseUri": "file:///w/root/b", "pattern": "src/[ab.rs" } },
                ])),
                other.clone(),
            ],
            &limits,
        );
        assert_eq!(w.everything, 0);
        assert!(wants(&w, "/w/root/a.rs", Change::Changed));
        assert!(!wants(&w, "/w/root/a.py", Change::Changed));
        assert!(wants(&w, "/w/root/a/x.txt", Change::Deleted));
        assert!(!wants(&w, "/w/root/a/x.txt", Change::Changed));
        assert!(wants(&w, "/w/root/b/deep/x.txt", Change::Created));
        assert!(!wants(&w, "/w/root/c/x.txt", Change::Changed));
        // 기준 없는 글롭을 쓸 수 없으면 그 종류의 모든 경로, 기준 폴더도 못 읽으면 모든 경로 · 종류.
        let w = Watchers::compile(
            "s",
            &[registration(
                json!([{ "globPattern": "src/[ab.rs", "kind": 2 }]),
            )],
            &limits,
        );
        assert!(wants(&w, "/w/root/x.txt", Change::Changed));
        assert!(!wants(&w, "/w/root/x.txt", Change::Created));
        let w = Watchers::compile(
            "s",
            &[registration(
                json!([{ "globPattern": { "baseUri": "untitled:x", "pattern": "*" } }]),
            )],
            &limits,
        );
        assert!(wants(&w, "/w/root/x.txt", Change::Created));
        // 글롭 수 상한을 넘으면 모든 경로 · 종류.
        let w = Watchers::compile(
            "s",
            &[registration(json!([
                { "globPattern": "**/*.rs" },
                { "globPattern": "**/*.ts" },
                { "globPattern": "**/*.md" },
            ]))],
            &WatchLimits {
                max_watchers: 2,
                ..WatchLimits::default()
            },
        );
        assert!(wants(&w, "/w/root/x.txt", Change::Deleted));
        // 글롭 바이트 합 상한을 넘으면(글롭 하나하나는 길이 상한 안) 모든 경로 · 종류. 상한 안이면 글롭대로.
        let three = [registration(json!([
            { "globPattern": "**/*.rs" },
            { "globPattern": "**/*.ts" },
            { "globPattern": "**/*.md" },
        ]))];
        let w = Watchers::compile(
            "s",
            &three,
            &WatchLimits {
                max_total_pattern_bytes: 20,
                ..WatchLimits::default()
            },
        );
        assert!(wants(&w, "/w/root/x.txt", Change::Deleted));
        let w = Watchers::compile(
            "s",
            &three,
            &WatchLimits {
                max_total_pattern_bytes: 21,
                ..WatchLimits::default()
            },
        );
        assert!(!wants(&w, "/w/root/x.txt", Change::Deleted));
        assert!(wants(&w, "/w/root/x.md", Change::Deleted));
    }

    /// 리뷰 H1 재현: rust-analyzer 꼴로 패키지 300 개(기준 폴더마다 글롭 셋 — 900 개)를 등록하고 다시 만든 목록 30,000 파일(모두
    /// 맞음)을 넣으면 하나도 버리지 않고 모두 알린다.
    #[test]
    fn many_packages_lose_no_matching_file() {
        let mut watchers = Vec::new();
        let mut files: Vec<Arc<str>> = Vec::new();
        for p in 0..300 {
            let base = format!("file:///w/root/crates/p{p}");
            for pattern in ["**/*.rs", "**/Cargo.{lock,toml}", "**/rust-analyzer.toml"] {
                watchers.push(json!({ "globPattern": { "baseUri": base, "pattern": pattern } }));
            }
            for n in 0..100 {
                files.push(format!("crates/p{p}/src/m{n}.rs").into());
            }
        }
        let w = Watchers::compile(
            "s",
            &[registration(Value::Array(watchers))],
            &WatchLimits::default(),
        );
        assert_eq!(w.everything, 0, "within the limits");
        let started = Instant::now();
        let sent = drain(
            Work {
                events: Vec::new(),
                at: 0,
                list: Some(Arc::new(files)),
                list_at: 0,
            },
            &w,
            &scope(),
        );
        eprintln!(
            "30,000 paths × 900 globs matched in {:?}",
            started.elapsed()
        );
        assert_eq!(sent.len(), 30_000);
        assert!(sent.iter().all(|(_, t)| *t == 2));
    }

    /// 루트 밖 · `..` 가 낀 경로 · 상대 경로는 보내지 않는다. 증분 다음 다시 만든 목록(`Changed`)이 차례로, 알림 하나에
    /// `batch_events` 까지 · 한 번에 `scan_step` 경로까지 맞춰 본다.
    #[test]
    fn steps_keep_to_the_root_and_go_through_everything_in_order() {
        let w = Watchers::compile(
            "s",
            &[registration(json!([{ "globPattern": "**/*.rs" }]))],
            &WatchLimits::default(),
        );
        let event = |p: &str, change| Event {
            path: PathBuf::from(p),
            change,
        };
        let work = Work {
            events: vec![
                event("/w/root/a.rs", Change::Created),
                event("/w/other/b.rs", Change::Created),
                event("/w/root/../other/c.rs", Change::Created),
                event("w/root/d.rs", Change::Created),
                event("/w/root/e.txt", Change::Created),
            ],
            at: 0,
            list: Some(Arc::new(vec!["src/f.rs".into(), "g.txt".into()])),
            list_at: 0,
        };
        assert_eq!(
            drain(work, &w, &scope()),
            [("a.rs".to_owned(), 1), ("src/f.rs".to_owned(), 2)]
        );
        let mut work = Work {
            events: (0..10)
                .map(|n| event(&format!("/w/root/{n}.rs"), Change::Changed))
                .collect(),
            at: 0,
            list: None,
            list_at: 0,
        };
        let mut small = scope();
        small.limits.batch_events = 4;
        assert_eq!(step(&mut work, &w, &small, &BTreeSet::new()).len(), 4);
        small.limits.batch_events = 100;
        small.limits.scan_step = 3;
        assert_eq!(step(&mut work, &w, &small, &BTreeSet::new()).len(), 3);
        // 열린 문서는 뺀다.
        let open = BTreeSet::from(["file:///w/root/7.rs".to_owned()]);
        small.limits.scan_step = 100;
        assert_eq!(step(&mut work, &w, &small, &open).len(), 2);
        assert!(work.done());
    }

    /// 첫 목록(`listed`)은 알리지 않고 다시 만든 목록(`rebuilt`)만 알린다. 제 (프로젝트, 루트)만 받는다(리뷰 M1 · L3).
    #[test]
    fn the_observer_announces_rebuilt_lists_only() {
        let feed = Feed::new(10);
        let observer = FeedObserver {
            project_id: "p".into(),
            root: PathBuf::from("/w/root"),
            feed: Arc::downgrade(&feed),
        };
        let files = || vec![(Arc::<str>::from("a.rs"), FileFlags::default())];
        assert!(!observer.wants("p", Path::new("/w/other")));
        assert!(!observer.wants("q", Path::new("/w/root")));
        observer.listed("p", Path::new("/w/root"), files());
        assert!(feed.take().is_none());
        observer.rebuilt("p", Path::new("/w/root"), files());
        assert_eq!(feed.take().unwrap().rescan.unwrap().len(), 1);
    }

    /// 쌓기 상한을 처음 넘칠 때 한 번만 다시 알리기를 청한다 — 목록이 오거나 청하지 못했으면 다시 청할 수 있다.
    #[test]
    fn an_overflow_asks_for_one_refill() {
        let feed = Feed::new(2);
        let e = |n: u32| Event {
            path: PathBuf::from(format!("/w/root/{n}.rs")),
            change: Change::Created,
        };
        assert!(!feed.push(&[e(0)]));
        assert!(feed.push(&[e(1), e(2)]));
        assert!(!feed.push(&[e(3)]));
        assert_eq!(feed.take().unwrap().events.len(), 2);
        feed.rescan(Vec::new());
        assert!(!feed.push(&[e(4), e(5)]));
        assert!(feed.push(&[e(6)]));
        // 청하지 못했으면(인덱스에 그 루트가 없음) 다음 넘침에 다시 청한다.
        feed.refill_failed();
        assert!(feed.push(&[e(7)]));
    }
}
