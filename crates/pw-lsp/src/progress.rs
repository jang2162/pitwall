//! 서버의 진행(`$/progress` 의 `WorkDoneProgress`)을 Host `Task` 로(15-m4-plan.md M4-05 「진행」).
//!
//! `begin` 이 오면 그 프로젝트 범위의 작업을 띄운다(제목은 서버의 `title`) — 상태바의 「인덱싱 42%」 가 이것이다. `report` 는
//! 진행률(`percentage` → `done`/100)과 문구를 알리고, `end` 나 서버가 끝나면 작업이 끝난다. 사람이 작업을 취소하면(`task.cancel`)
//! 서버가 취소를 받는다고 알린 진행(`cancellable`)에 `window/workDoneProgress/cancel` 을 보낸다.
//!
//! 진행 토큰은 서버가 정한 값(글 · 수)이라 서버마다 따로 센다 — 인스턴스마다 [`Tracker`] 하나다.
//!
//! 상태 이벤트(`lsp.status`)는 진행이 생기고 끝날 때와 진행률이 바뀔 때만 다시 낸다 — 문구만 바뀐 `report`(큰 작업 공간을 색인하는
//! rust-analyzer 는 많이 낸다)는 `Task` 로만 나간다. 동시 진행은 서버마다 [`MAX_ACTIVE`] 개까지 — `end` 없는 `begin` 이 쌓이지
//! 않게, 넘는 `begin` 은 버린다(M4-05 리뷰 L3).
//!
//! [`WireProgress`] 는 그와 따로 서버의 읽기 태스크가 틀을 읽는 그 자리에서 진행을 센다(이벤트 채널 앞 — 버려지지도 늦지도 않는다).
//! 포맷(M4-13 「진행 중의 답」, `crate::format`)이 「요청을 보낸 뒤 답을 받기까지 어떤 진행이 열려 있었나」 를 묻는다 — 응답과 같은
//! 차례로 읽으므로 서버가 답보다 먼저 보낸 begin 은 답을 받은 때 늘 들어와 있다.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Weak;

use pw_core::bus::Scope;
use pw_core::task::{TaskProgress, TaskSpawner, TaskStatus};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::methods::LspProgress;
use crate::server::LanguageServer;

/// 서버 하나가 동시에 가질 수 있는 진행 수.
pub(crate) const MAX_ACTIVE: usize = 64;

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
    /// `$/progress` 하나. 상태 이벤트를 다시 낼 만큼 바뀌었으면 참(머리 주석). 진행이 아닌 것(부분 결과 등)은 무시한다.
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
                if !self.active.contains_key(&key) && self.active.len() >= MAX_ACTIVE {
                    tracing::debug!(target: "pw_lsp", "dropping a progress beyond {MAX_ACTIVE} at once");
                    return false;
                }
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
                let moved = percentage.is_some() && percentage != active.info.percentage;
                if moved {
                    active.info.percentage = percentage;
                }
                active.touched = self.clock;
                let _ = active.tx.send(task_progress(&active.info));
                moved
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

/// [`WireProgress`] 가 기억하는 최근 begin 수. 표지 뒤로 이보다 많이 열렸으면 그 사이의 제목을 다 알지 못한다.
pub(crate) const RECENT_BEGINS: usize = 256;

/// 읽기 태스크가 센 진행(머리 주석). 열린 진행은 [`MAX_ACTIVE`] 개까지 — 넘는 begin 은 최근 begin 에만 남는다.
#[derive(Default)]
pub(crate) struct WireProgress {
    /// 토큰 → 제목.
    open: BTreeMap<String, String>,
    /// 지금까지 받은 begin 수.
    begun: u64,
    /// 최근 begin 의 (차례 — 1 부터, 제목).
    recent: VecDeque<(u64, String)>,
}

/// 진행 표지([`crate::server::LanguageServer::progress_mark`]): 그때 열린 진행의 제목과 그때까지 받은 begin 수.
#[derive(Debug, Clone)]
pub struct ProgressMark {
    begun: u64,
    open: BTreeSet<String>,
}

impl WireProgress {
    /// 읽은 `$/progress` 하나.
    pub(crate) fn handle(&mut self, params: &Value) {
        let (Some(token), Some(value)) = (params.get("token"), params.get("value")) else {
            return;
        };
        let Some(key) = token_key(token) else {
            return;
        };
        match value.get("kind").and_then(Value::as_str) {
            Some("begin") => {
                let title = value
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                self.begun += 1;
                if self.recent.len() == RECENT_BEGINS {
                    self.recent.pop_front();
                }
                self.recent.push_back((self.begun, title.clone()));
                if self.open.contains_key(&key) || self.open.len() < MAX_ACTIVE {
                    self.open.insert(key, title);
                }
            }
            Some("end") => {
                self.open.remove(&key);
            }
            _ => {}
        }
    }

    pub(crate) fn mark(&self) -> ProgressMark {
        ProgressMark {
            begun: self.begun,
            open: self.open.values().cloned().collect(),
        }
    }

    /// `mark` 부터 지금까지 열려 있던 진행의 제목: 표지 때 열려 있던 것, 그 뒤 begin 이 온 것, 지금 열린 것. 그 뒤의 begin 을
    /// 다 기억하지 못하면(표지 뒤로 [`RECENT_BEGINS`] 개 넘게 열렸다) `None`.
    pub(crate) fn titles_since(&self, mark: &ProgressMark) -> Option<BTreeSet<String>> {
        let first_kept = self.recent.front().map_or(self.begun + 1, |(n, _)| *n);
        if self.begun > mark.begun && first_kept > mark.begun + 1 {
            return None;
        }
        let mut titles = mark.open.clone();
        titles.extend(
            self.recent
                .iter()
                .filter(|(n, _)| *n > mark.begun)
                .map(|(_, t)| t.clone()),
        );
        titles.extend(self.open.values().cloned());
        Some(titles)
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

#[cfg(test)]
mod tests {
    use pw_core::bus::{DEFAULT_CAPACITY, EventBus};

    use super::*;

    fn progress(token: u64, value: Value) -> Value {
        json!({ "token": token, "value": value })
    }

    /// 리뷰 L3: 상태 이벤트는 생기고 끝날 때 · 진행률이 바뀔 때만. 동시 진행은 [`MAX_ACTIVE`] 개까지.
    #[tokio::test]
    async fn only_begin_end_and_percentage_changes_republish_and_actives_are_capped() {
        let tasks = TaskSpawner::new(
            EventBus::new(DEFAULT_CAPACITY),
            Scope::Host,
            CancellationToken::new(),
        );
        let mut tracker = Tracker::default();
        let mut handle = |p: Value| tracker.handle(&p, &tasks, "p", Weak::new());
        assert!(handle(progress(
            1,
            json!({ "kind": "begin", "title": "index" })
        )));
        assert!(!handle(progress(
            1,
            json!({ "kind": "report", "message": "a.rs" })
        )));
        assert!(handle(progress(
            1,
            json!({ "kind": "report", "percentage": 10 })
        )));
        assert!(!handle(progress(
            1,
            json!({ "kind": "report", "percentage": 10, "message": "b.rs" })
        )));
        assert!(handle(progress(
            1,
            json!({ "kind": "report", "percentage": 11 })
        )));
        assert!(handle(progress(1, json!({ "kind": "end" }))));
        for token in 0..MAX_ACTIVE as u64 {
            assert!(handle(progress(
                100 + token,
                json!({ "kind": "begin", "title": "t" })
            )));
        }
        assert!(!handle(progress(
            9999,
            json!({ "kind": "begin", "title": "too many" })
        )));
        assert_eq!(tracker.active.len(), MAX_ACTIVE);
        assert_eq!(tracker.latest().unwrap().title, "t");
    }

    /// M4-13: 열린 진행의 제목은 모두 보인다 — 나중에 바뀐 진행이 앞의 것을 가리지 않는다. 끝난 진행은 빠진다.
    #[test]
    fn every_open_progress_title_is_listed_not_only_the_latest() {
        let mut wire = WireProgress::default();
        wire.handle(&progress(
            1,
            json!({ "kind": "begin", "title": "Roots Scanned" }),
        ));
        wire.handle(&progress(
            2,
            json!({ "kind": "begin", "title": "Indexing" }),
        ));
        wire.handle(&progress(2, json!({ "kind": "report", "percentage": 50 })));
        let both = wire.mark();
        assert_eq!(
            wire.titles_since(&both).unwrap(),
            BTreeSet::from(["Roots Scanned".to_owned(), "Indexing".to_owned()])
        );
        wire.handle(&progress(1, json!({ "kind": "end" })));
        assert_eq!(
            wire.titles_since(&wire.mark()).unwrap(),
            BTreeSet::from(["Indexing".to_owned()])
        );
        // 표지 때 열려 있던 것은 그 뒤에 닫혀도 든다.
        assert!(wire.titles_since(&both).unwrap().contains("Roots Scanned"));
    }

    /// 15-m4-plan §3 「lsp-live … 한 번씩 난 실패」 ①: 표지 뒤에 열렸다 닫힌 진행도 든다. 그 사이 begin 을 다 기억하지 못하면 `None`.
    #[test]
    fn a_progress_begun_and_ended_after_the_mark_is_listed() {
        let mut wire = WireProgress::default();
        let mark = wire.mark();
        wire.handle(&progress(
            1,
            json!({ "kind": "begin", "title": "Roots Scanned" }),
        ));
        wire.handle(&progress(1, json!({ "kind": "end" })));
        assert_eq!(
            wire.titles_since(&mark).unwrap(),
            BTreeSet::from(["Roots Scanned".to_owned()])
        );
        assert_eq!(wire.titles_since(&wire.mark()).unwrap(), BTreeSet::new());

        let mark = wire.mark();
        for n in 0..RECENT_BEGINS as u64 {
            wire.handle(&progress(
                n + 10,
                json!({ "kind": "begin", "title": "Indexing" }),
            ));
            wire.handle(&progress(n + 10, json!({ "kind": "end" })));
        }
        assert_eq!(
            wire.titles_since(&mark).unwrap(),
            BTreeSet::from(["Indexing".to_owned()])
        );
        wire.handle(&progress(
            9,
            json!({ "kind": "begin", "title": "Indexing" }),
        ));
        assert_eq!(wire.titles_since(&mark), None);
    }
}
