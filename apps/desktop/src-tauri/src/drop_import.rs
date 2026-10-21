//! Finder(다른 앱)에서 파일 트리로 드롭한 항목을 그 폴더로 복사한다(`docs/design/12-m1-plan.md` M1-21, 10-ui §5.4 「Finder에서
//! 드롭」).
//!
//! 드롭한 항목은 루트 밖에 있으므로 Host RPC `fs.copy`(원본도 루트 안이어야 한다)로는 못 한다. 그렇다고 RPC 에 「원본 검사 없음」
//! 을 열면 원격 · MCP 연결이 디스크의 아무 파일이나 프로젝트로 끌어올 수 있다. 그래서 이 데스크톱 command 만 하고 원본은 **그 창의
//! 네이티브 드롭 이벤트가 준 경로뿐**이다:
//!
//! - 창이 드롭을 받으면(`WindowEvent::DragDrop::Drop`) 경로를 기록하고 번호를 붙여 그 창에 [`DROP_EVENT`] 로 알린다(번호 · 경로 ·
//!   위치). 웹뷰는 위치로 트리의 대상 폴더를 고르고 `import_drop` 에 **번호와 인덱스**를 넘긴다 — 경로 글자를 넘겨 임의의 파일을
//!   고르지 못한다.
//! - 기록은 창마다 마지막 드롭 하나이고, 한 번 꺼내면 버리고, [`DROP_TTL`] 이 지나면 버린다. 번호가 다르거나(위조 · 옛 드롭) 다른
//!   창의 번호면 거절한다.
//! - 대상 폴더는 pw-fs 의 쓰기 규칙으로 판정하고, 같은 이름은 덮지 않는다([`pw_fs::Fs::import`] — 링크는 링크로, 특수 파일은 건너뛴다).
//! - 복사는 그 프로젝트의 `Task` 로 돈다(상태바의 진행 · 취소, M1-04). 취소하면 지금 항목을 치우고 멈춘다.
//!
//! 로컬 창은 owner 다(ipc.rs). MCP · 원격 연결은 Tauri command 를 부를 수 없다.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use pw_core::task::{TaskProgress, TaskSpawner};
use pw_core::{Scope, task};
use pw_fs::Fs;
use pw_fs::ops::Imported;
use pw_rpc::RpcError;
use serde::Serialize;
use tauri::{DragDropEvent, Emitter, EventTarget, Runtime, State, Window, WindowEvent};
use tokio_util::sync::CancellationToken;

/// 드롭을 기록했다고 그 창에 알리는 이벤트(src/file-tree/tree-drop.ts 가 듣는다).
pub const DROP_EVENT: &str = "pitwall://file-drop";

/// 드롭한 뒤 들여오기를 부를 수 있는 시간.
pub const DROP_TTL: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DropRecorded {
    pub id: u64,
    pub paths: Vec<String>,
    /// 창 안의 물리 픽셀 위치(Tauri `onDragDropEvent` 의 `position` 과 같다).
    pub x: f64,
    pub y: f64,
}

struct Recorded {
    id: u64,
    paths: Vec<PathBuf>,
    at: Instant,
}

/// 들여오기를 거절한 이유.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refused {
    /// 이 창의 마지막 드롭이 아니다(위조 · 다른 창 · 이미 꺼냈다).
    Unknown,
    Expired,
    /// 그 드롭에 없는 인덱스.
    Index,
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Refused::Unknown => "the drop is unknown or was already used",
            Refused::Expired => "the drop expired",
            Refused::Index => "the drop has no such item",
        })
    }
}

/// 창마다 마지막 드롭.
#[derive(Default)]
pub struct Drops {
    next: AtomicU64,
    by_window: Mutex<HashMap<String, Recorded>>,
}

impl Drops {
    /// 창 `label` 의 드롭을 기록하고 번호를 돌려준다. 그 창의 앞선 드롭은 버린다.
    pub fn record(&self, label: &str, paths: Vec<PathBuf>, now: Instant) -> u64 {
        let id = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        self.lock()
            .insert(label.to_owned(), Recorded { id, paths, at: now });
        id
    }

    /// 창 `label` 의 드롭 `id` 에서 `indices`(없으면 전부)의 경로를 꺼낸다. 맞든 틀리든 그 창의 기록은 버린다 — 한 번만 쓴다.
    pub fn take(
        &self,
        label: &str,
        id: u64,
        indices: Option<&[usize]>,
        now: Instant,
    ) -> Result<Vec<PathBuf>, Refused> {
        let mut by_window = self.lock();
        let Some(recorded) = by_window.remove(label) else {
            return Err(Refused::Unknown);
        };
        if recorded.id != id {
            // 다른 번호로는 기록을 쓰지 않는다. 같은 창의 맞는 번호는 아직 쓸 수 있게 되돌린다.
            by_window.insert(label.to_owned(), recorded);
            return Err(Refused::Unknown);
        }
        if now.saturating_duration_since(recorded.at) > DROP_TTL {
            return Err(Refused::Expired);
        }
        match indices {
            None => Ok(recorded.paths),
            Some(indices) => indices
                .iter()
                .map(|&i| recorded.paths.get(i).cloned().ok_or(Refused::Index))
                .collect(),
        }
    }

    /// 창이 닫혔다.
    pub fn forget(&self, label: &str) {
        self.lock().remove(label);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Recorded>> {
        self.by_window
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

/// 앱 상태(`app.manage`): 드롭 기록 · 파일 서비스 · Host 의 작업 spawner.
pub struct DropImport {
    pub drops: Drops,
    fs: Arc<Fs>,
    tasks: TaskSpawner,
}

impl DropImport {
    pub fn new(fs: Arc<Fs>, tasks: TaskSpawner) -> Self {
        Self {
            drops: Drops::default(),
            fs,
            tasks,
        }
    }
}

/// 창 이벤트(ipc.rs 의 `on_window_event` 가 부른다): 드롭을 기록해 그 창에 알리고, 창이 닫히면 기록을 버린다.
pub fn on_window_event<R: Runtime>(window: &Window<R>, event: &WindowEvent, state: &DropImport) {
    match event {
        WindowEvent::DragDrop(DragDropEvent::Drop { paths, position }) if !paths.is_empty() => {
            let id = state
                .drops
                .record(window.label(), paths.clone(), Instant::now());
            let payload = DropRecorded {
                id,
                paths: paths.iter().map(|p| p.display().to_string()).collect(),
                x: position.x,
                y: position.y,
            };
            if let Err(e) = window.emit_to(
                EventTarget::webview_window(window.label()),
                DROP_EVENT,
                payload,
            ) {
                tracing::warn!(%e, "cannot announce a file drop");
            }
        }
        WindowEvent::Destroyed => state.drops.forget(window.label()),
        _ => {}
    }
}

/// 드롭 `dropId` 의 항목(`indices`, 없으면 전부)을 프로젝트의 폴더 `dir` 로 복사한다. `title` 은 작업 표시줄의 이름(웹뷰의 UI 언어).
#[allow(clippy::too_many_arguments)] // Tauri command 의 인자는 웹뷰가 넘기는 필드 그대로다.
#[tauri::command]
pub async fn import_drop<R: Runtime>(
    window: Window<R>,
    state: State<'_, DropImport>,
    drop_id: u64,
    indices: Option<Vec<usize>>,
    project_id: String,
    worktree: Option<String>,
    dir: String,
    title: String,
) -> Result<Imported, RpcError> {
    // 단일 파일 창(M1-24)은 그 파일 하나에만 닿는다 — 그 창의 드롭으로 프로젝트에 들여오지 않는다.
    if crate::windows::WindowKind::of_label(window.label())
        == Some(crate::windows::WindowKind::File)
    {
        return Err(RpcError::outside_file_scope("import_drop"));
    }
    let sources = state
        .drops
        .take(window.label(), drop_id, indices.as_deref(), Instant::now())
        .map_err(|r| RpcError::invalid_params(r.to_string()))?;
    let total = sources.len() as u64;
    let fs = Arc::clone(&state.fs);
    let (tx, rx) = tokio::sync::oneshot::channel();
    let tasks = state
        .tasks
        .scoped(Scope::Project(project_id.clone()), CancellationToken::new());
    let handle = tasks.spawn(title, move |ctx: task::TaskCtx| async move {
        let ctx = Arc::new(ctx);
        let worker = Arc::clone(&ctx);
        let outcome = tokio::task::spawn_blocking(move || {
            let cancel = worker.cancellation().clone();
            worker.report(TaskProgress {
                done: 0,
                total: Some(total),
                message: None,
            });
            fs.import(
                &project_id,
                worktree.as_deref(),
                Path::new(&dir),
                &sources,
                &|| cancel.is_cancelled(),
                &mut |done| {
                    worker.report(TaskProgress {
                        done: done as u64,
                        total: Some(total),
                        message: None,
                    });
                },
            )
        })
        .await
        .map_err(|e| RpcError::internal(e.to_string()))
        .and_then(|r| r);
        let failed = outcome.as_ref().err().map(|e| e.message.clone());
        let _ = tx.send(outcome);
        drop(ctx);
        failed.map_or(Ok(()), Err)
    });
    // 취소하면 작업 future 가 버려져 `tx` 가 닫힌다. 복사 스레드는 다음 항목에서 멈추고 지금 항목을 치운다.
    let _keep = handle;
    match rx.await {
        Ok(outcome) => outcome,
        Err(_) => Ok(Imported {
            cancelled: true,
            ..Imported::default()
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths() -> Vec<PathBuf> {
        vec![
            PathBuf::from("/Users/me/a.txt"),
            PathBuf::from("/Users/me/b"),
        ]
    }

    #[test]
    fn a_drop_is_taken_once_by_its_window_and_id() {
        let drops = Drops::default();
        let now = Instant::now();
        let id = drops.record("project-1", paths(), now);
        // 위조한 번호 · 다른 창은 거절하고, 맞는 번호는 그 뒤에도 쓸 수 있다.
        assert_eq!(
            drops.take("project-1", id + 1, None, now),
            Err(Refused::Unknown)
        );
        assert_eq!(
            drops.take("project-2", id, None, now),
            Err(Refused::Unknown)
        );
        assert_eq!(
            drops.take("project-1", id, Some(&[1]), now),
            Ok(vec![PathBuf::from("/Users/me/b")])
        );
        // 다시 쓰지 못한다.
        assert_eq!(
            drops.take("project-1", id, None, now),
            Err(Refused::Unknown)
        );
    }

    #[test]
    fn a_drop_expires_and_a_newer_drop_replaces_it() {
        let drops = Drops::default();
        let then = Instant::now();
        let old = drops.record("w", paths(), then);
        let newer = drops.record("w", vec![PathBuf::from("/x")], then);
        assert_eq!(drops.take("w", old, None, then), Err(Refused::Unknown));
        assert_eq!(
            drops.take("w", newer, None, then + DROP_TTL + Duration::from_secs(1)),
            Err(Refused::Expired)
        );
        let id = drops.record("w", paths(), then);
        assert_eq!(drops.take("w", id, Some(&[2]), then), Err(Refused::Index));
        let id = drops.record("w", paths(), then);
        drops.forget("w");
        assert_eq!(drops.take("w", id, None, then), Err(Refused::Unknown));
    }
}
