//! 서버의 진행(`$/progress` 의 `WorkDoneProgress`)을 Host `Task` 로(15-m4-plan.md M4-05 「진행」).
//!
//! `begin` 이 오면 그 프로젝트 범위의 작업을 띄운다(제목은 서버의 `title`) — 상태바의 「인덱싱 42%」 가 이것이다. `report` 는
//! 진행률(`percentage` → `done`/100)과 문구를 알리고, `end` 나 서버가 끝나면 작업이 끝난다. 사람이 작업을 취소하면(`task.cancel`)
//! 서버가 취소를 받는다고 알린 진행(`cancellable`)에 `window/workDoneProgress/cancel` 을 보낸다.
//!
//! 진행 토큰은 서버가 정한 값(글 · 수)이라 서버마다 따로 센다 — 인스턴스마다 [`Tracker`] 하나다.

use std::collections::BTreeMap;
use std::sync::Weak;

use pw_core::bus::Scope;
use pw_core::task::{TaskProgress, TaskSpawner, TaskStatus};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::methods::LspProgress;
use crate::server::LanguageServer;

/// 진행 하나.
struct Active {
    info: LspProgress,
    /// 작업에 진행을 넘긴다. 버리면 작업이 끝난다.
    tx: mpsc::UnboundedSender<TaskProgress>,
    /// 몇 번째로 바뀌었는가(가장 최근 진행을 고른다).
    touched: u64,
}

/// 인스턴스 하나의 진행들.
#[derive(Default)]
pub(crate) struct Tracker {
    active: BTreeMap<String, Active>,
    clock: u64,
}

fn token_key(token: &Value) -> Option<String> {
    match token {
        Value::String(s) => Some(format!("s:{s}")),
        Value::Number(n) => Some(format!("n:{n}")),
        _ => None,
    }
}

fn task_progress(info: &LspProgress) -> TaskProgress {
    TaskProgress {
        done: info.percentage.map_or(0, u64::from),
        total: info.percentage.map(|_| 100),
        message: info.message.clone(),
    }
}

impl Tracker {
    /// `$/progress` 하나. 바뀌었으면 참(상태 이벤트를 다시 낸다). 진행이 아닌 것(부분 결과 등)은 무시한다.
    pub(crate) fn handle(
        &mut self,
        params: &Value,
        tasks: &TaskSpawner,
        project_id: &str,
        server: Weak<LanguageServer>,
    ) -> bool {
        let (Some(token), Some(value)) = (params.get("token"), params.get("value")) else {
            return false;
        };
        let Some(key) = token_key(token) else {
            return false;
        };
        let text = |name: &str| value.get(name).and_then(Value::as_str).map(str::to_owned);
        let percentage = value
            .get("percentage")
            .and_then(Value::as_u64)
            .map(|p| u32::try_from(p.min(100)).unwrap_or(100));
        self.clock += 1;
        match value.get("kind").and_then(Value::as_str) {
            Some("begin") => {
                let info = LspProgress {
                    title: text("title").unwrap_or_default(),
                    message: text("message"),
                    percentage,
                };
                let cancellable = value
                    .get("cancellable")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let tx = start_task(tasks, project_id, &info, token.clone(), cancellable, server);
                self.active.insert(
                    key,
                    Active {
                        info,
                        tx,
                        touched: self.clock,
                    },
                );
                true
            }
            Some("report") => {
                let Some(active) = self.active.get_mut(&key) else {
                    return false;
                };
                if let Some(message) = text("message") {
                    active.info.message = Some(message);
                }
                if percentage.is_some() {
                    active.info.percentage = percentage;
                }
                active.touched = self.clock;
                let _ = active.tx.send(task_progress(&active.info));
                true
            }
            Some("end") => self.active.remove(&key).is_some(),
            _ => false,
        }
    }

    /// 가장 최근에 바뀐 진행.
    pub(crate) fn latest(&self) -> Option<LspProgress> {
        self.active
            .values()
            .max_by_key(|a| a.touched)
            .map(|a| a.info.clone())
    }

    /// 모든 진행을 끝낸다(서버가 끝났다).
    pub(crate) fn end_all(&mut self) {
        self.active.clear();
    }
}

/// 작업을 띄우고 그 진행을 넘길 곳을 돌려준다.
fn start_task(
    tasks: &TaskSpawner,
    project_id: &str,
    info: &LspProgress,
    token: Value,
    cancellable: bool,
    server: Weak<LanguageServer>,
) -> mpsc::UnboundedSender<TaskProgress> {
    let (tx, mut rx) = mpsc::unbounded_channel::<TaskProgress>();
    let _ = tx.send(task_progress(info));
    let spawner = tasks.scoped(
        Scope::Project(project_id.to_owned()),
        CancellationToken::new(),
    );
    let mut handle = spawner.spawn(info.title.clone(), move |ctx| async move {
        while let Some(progress) = rx.recv().await {
            ctx.report(progress);
        }
        Ok(())
    });
    tokio::spawn(async move {
        if handle.wait().await == TaskStatus::Cancelled
            && cancellable
            && let Some(server) = server.upgrade()
        {
            let _ = server.notify_raw("window/workDoneProgress/cancel", json!({ "token": token }));
        }
    });
    tx
}
