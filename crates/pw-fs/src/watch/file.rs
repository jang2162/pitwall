//! 파일 하나의 감시(단일 파일 창, `docs/design/12-m1-plan.md` M1-24). 파일 범위 호출자의 `fs.watch` 가 쓴다.
//!
//! - 그 파일이 든 폴더를 **비재귀**로 본다 — 파일 자체에 걸면 에디터 · 에이전트의 원자 저장(임시 파일 → rename)이 감시를 끊는다
//!   (inotify 는 옛 inode 를 본다). 폴더 안의 다른 항목의 변경은 버리고 그 파일의 것만 낸다. 폴더 아래는 보지 않는다.
//! - 디바운스는 프로젝트 감시와 같은 고정 창([`super::engine::DEBOUNCE`])이다. 창 하나에 한 번, 그 파일 하나의 [`FsChanged`] 를
//!   그 호출자의 범위([`pw_rpc::FileScope::event_scope`])로 낸다 — 종류는 낼 때의 디스크(있으면 `modified`, 처음 생겼으면
//!   `created`, 없으면 `removed`)다.
//! - 프로젝트 감시와 달리 루트끼리 나누지 않는다: 같은 파일의 창은 하나다(데스크톱 앱이 실제 경로로 고른다).

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher as _};
use pw_core::bus::{EventBus, Scope};

use super::{FsChangeKind, FsChanged, TOPIC_CHANGED};

enum Msg {
    Raw(notify::Result<notify::Event>),
    Stop,
}

/// 파일 하나의 감시. 버리면 멈춘다.
pub struct FileWatch {
    tx: mpsc::Sender<Msg>,
    _watcher: RecommendedWatcher,
}

impl FileWatch {
    /// OS 감시를 다 건 뒤에 돌아온다 — 돌아온 뒤의 변경은 놓치지 않는다. 바뀌면 `bus` 의 `scope` 로 낸다.
    pub fn start(
        file: PathBuf,
        bus: EventBus,
        scope: String,
        debounce: Duration,
    ) -> notify::Result<Self> {
        let parent = file
            .parent()
            .ok_or_else(|| notify::Error::path_not_found().add_path(file.clone()))?
            .to_path_buf();
        let (tx, rx) = mpsc::channel();
        let raw = tx.clone();
        let mut watcher = notify::recommended_watcher(move |event| {
            let _ = raw.send(Msg::Raw(event));
        })?;
        watcher.watch(&parent, RecursiveMode::NonRecursive)?;
        let mut existed = file.exists();
        std::thread::Builder::new()
            .name("pw-fs-watch-file".into())
            .spawn(move || {
                while next(&rx, &file, None) == Next::Change {
                    let deadline = Instant::now() + debounce;
                    // 창 안의 나머지는 모으기만 한다 — 디스크는 낼 때 한 번 본다.
                    loop {
                        match next(&rx, &file, Some(deadline)) {
                            Next::Change => {}
                            Next::Quiet => break,
                            Next::Stop => return,
                        }
                    }
                    let exists = file.exists();
                    let kind = match (existed, exists) {
                        (_, false) => FsChangeKind::Removed,
                        (false, true) => FsChangeKind::Created,
                        (true, true) => FsChangeKind::Modified,
                    };
                    existed = exists;
                    publish(&bus, &scope, &file, kind);
                }
            })
            .map_err(notify::Error::io)?;
        Ok(Self {
            tx,
            _watcher: watcher,
        })
    }
}

impl Drop for FileWatch {
    fn drop(&mut self) {
        let _ = self.tx.send(Msg::Stop);
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Next {
    /// 그 파일의 변경이 왔다.
    Change,
    /// `deadline` 까지 오지 않았다.
    Quiet,
    Stop,
}

/// 그 파일의 다음 변경을 기다린다. `deadline` 이 없으면 올 때까지.
fn next(rx: &mpsc::Receiver<Msg>, file: &Path, deadline: Option<Instant>) -> Next {
    loop {
        let msg = match deadline {
            None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
            Some(deadline) => {
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    return Next::Quiet;
                }
                rx.recv_timeout(left)
            }
        };
        match msg {
            Ok(Msg::Raw(Ok(event))) if touches(&event, file) => return Next::Change,
            Ok(Msg::Raw(Err(e))) => {
                // 놓친 변경이 있을 수 있다 — 그 파일을 다시 보게 한다.
                tracing::warn!(target: "pw_fs", %e, file = %file.display(), "file watch error");
                return Next::Change;
            }
            Ok(Msg::Raw(Ok(_))) => {}
            Err(RecvTimeoutError::Timeout) => return Next::Quiet,
            Ok(Msg::Stop) | Err(RecvTimeoutError::Disconnected) => return Next::Stop,
        }
    }
}

/// 그 파일의 변경인가. 읽기는 버린다. 큐가 넘쳤으면(경로 없음) 그 파일도 바뀌었을 수 있다.
fn touches(event: &notify::Event, file: &Path) -> bool {
    !matches!(event.kind, EventKind::Access(_))
        && (event.need_rescan() || event.paths.iter().any(|p| p == file))
}

fn publish(bus: &EventBus, scope: &str, file: &Path, kind: FsChangeKind) {
    let path = file.display().to_string();
    let changed = FsChanged {
        root: path.clone(),
        worktree: None,
        paths: vec![path],
        kinds: vec![kind],
        rescan: false,
    };
    if let Err(e) = bus.publish(Scope::Project(scope.to_owned()), TOPIC_CHANGED, &changed) {
        tracing::warn!(target: "pw_fs", %e, "cannot publish fs.changed");
    }
}
