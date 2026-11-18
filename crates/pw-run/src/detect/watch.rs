//! 감지 출처 파일이 바뀌면 그 프로젝트의 감지만 다시 하고, 결과가 달라졌으면 창에 알린다.
//!
//! 목록(`run.config.list`)은 늘 새로 감지하므로 여기서 기억하는 것은 「마지막으로 알린 결과의 지문」 뿐이다 — 목록 · 실행이 이 기억을
//! 읽지 않는다. 기억은 목록을 부른 (프로젝트, 워크트리)만 든다(창이 본 적 없는 프로젝트는 감지하지 않는다 — 가벼움).

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};

use pw_core::bus::{EventBus, Filter, Received, Scope, Subscription};
use pw_core::config::ConfigChanged;
use serde::Deserialize;

use super::{DETECTORS, DetectExec, Detection, NoExec, detect};

/// 감지가 달라졌을 때 내는 `config.changed` 의 `changed` 항목. 창은 이것을 받으면 `run.config.list` 를 다시 부른다.
pub const CHANGED_KEY: &str = "run/detected";
/// 파일 감시 토픽(`pw_fs::watch::TOPIC_CHANGED` — 이 크레이트는 pw-fs 에 기대지 않는다).
const FS_CHANGED: &str = "fs.changed";

type SeenKey = (String, Option<String>);

/// 프로젝트 id → 그 프로젝트의 감지 실행기(신뢰 판정을 지나는 것 — M6-05 `exec::TrustedExec`).
pub type ExecFor = Arc<dyn Fn(&str) -> Box<dyn DetectExec> + Send + Sync>;

/// 목록을 부른 (프로젝트, 워크트리)마다 (루트, 마지막 감지의 지문).
#[derive(Default)]
pub struct Seen {
    entries: Mutex<HashMap<SeenKey, (PathBuf, u64)>>,
    /// 다시 감지할 때 쓰는 실행기 — 목록과 같은 것이어야 지문이 같다. 붙기 전에는 [`NoExec`].
    exec: OnceLock<ExecFor>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// 감지 결과의 지문(열쇠 · 구성 정규형 · 정의 본문 · 까닭).
pub fn fingerprint(detection: &Detection) -> u64 {
    let mut h = DefaultHasher::new();
    for task in &detection.tasks {
        task.key().hash(&mut h);
        task.config.canonical().hash(&mut h);
        task.definition.hash(&mut h);
    }
    for problem in &detection.problems {
        problem.detector.hash(&mut h);
        problem.reason.hash(&mut h);
    }
    for list in &detection.lists {
        list.detector.hash(&mut h);
        list.state.hash(&mut h);
    }
    h.finish()
}

impl Seen {
    /// 다시 감지할 때의 실행기를 붙인다(목록 `run.config.list` 와 같은 것). 두 번째부터는 무시한다.
    pub fn set_exec(&self, exec: ExecFor) {
        let _ = self.exec.set(exec);
    }

    fn exec_for(&self, project_id: &str) -> Box<dyn DetectExec> {
        match self.exec.get() {
            Some(f) => f(project_id),
            None => Box::new(NoExec),
        }
    }

    /// 목록이 감지한 결과를 기억한다.
    pub fn remember(
        &self,
        project_id: &str,
        worktree: Option<&str>,
        root: &Path,
        detection: &Detection,
    ) {
        lock(&self.entries).insert(
            (project_id.to_owned(), worktree.map(str::to_owned)),
            (root.to_path_buf(), fingerprint(detection)),
        );
    }

    /// `project_id`(와 `worktree` — `None` 이면 그 프로젝트의 모든 워크트리)의 기억한 자리를 다시 감지해 지문이 달라진 프로젝트들.
    /// `project_id` 도 `None` 이면 기억한 모두.
    pub fn refresh(&self, project_id: Option<&str>, worktree: Option<Option<&str>>) -> Vec<String> {
        let targets: Vec<(SeenKey, PathBuf)> = lock(&self.entries)
            .iter()
            .filter(|((p, w), _)| {
                project_id.is_none_or(|id| p == id) && worktree.is_none_or(|wt| w.as_deref() == wt)
            })
            .map(|(k, (root, _))| (k.clone(), root.clone()))
            .collect();
        let mut changed = Vec::new();
        for (key, root) in targets {
            let exec = self.exec_for(&key.0);
            let print = fingerprint(&detect(&root, exec.as_ref()));
            let mut entries = lock(&self.entries);
            if let Some((_, old)) = entries.get_mut(&key)
                && *old != print
            {
                *old = print;
                if !changed.contains(&key.0) {
                    changed.push(key.0.clone());
                }
            }
        }
        changed
    }
}

/// 감지 출처 파일인가(감지기마다 [`super::TaskDetector::watches`] — 루트 아래의 같은 이름이면 다시 감지해 봐도 값이 같아 알리지 않는다).
pub fn is_source(path: &Path) -> bool {
    DETECTORS.iter().any(|d| d.watches(path))
}

/// 파일 감시 이벤트 구독(모든 프로젝트의 `fs.changed`). Host 를 만든 직후에 연다.
pub fn subscribe_files(bus: &EventBus) -> Subscription {
    bus.subscribe(Filter::all().topics([FS_CHANGED]))
}

#[derive(Debug, Deserialize)]
struct FsChanged {
    #[serde(default)]
    worktree: Option<String>,
    #[serde(default)]
    paths: Vec<String>,
    #[serde(default)]
    rescan: bool,
}

/// 구독이 닫힐 때까지: 감지 출처 파일이 바뀐 프로젝트의 감지를 다시 하고, 달라졌으면 `config.changed { changed: ["run/detected"] }`
/// 를 그 프로젝트 범위로 낸다. 이벤트를 놓치면 기억한 모두를 다시 감지한다.
pub async fn follow_files(mut events: Subscription, seen: Arc<Seen>, bus: EventBus) {
    while let Some(received) = events.recv().await {
        let (project, worktree) = match received {
            Received::Event(event) => {
                let Scope::Project(project) = &event.scope else {
                    continue;
                };
                let Ok(changed) = serde_json::from_value::<FsChanged>(event.payload.clone()) else {
                    continue;
                };
                if !changed.rescan && !changed.paths.iter().any(|p| is_source(Path::new(p))) {
                    continue;
                }
                (Some(project.clone()), Some(changed.worktree))
            }
            Received::Lagged(n) => {
                tracing::warn!(target: "pw_run", dropped = n, "missed file watch events");
                (None, None)
            }
        };
        let s = Arc::clone(&seen);
        let Ok(changed) = tokio::task::spawn_blocking(move || {
            s.refresh(project.as_deref(), worktree.as_ref().map(Option::as_deref))
        })
        .await
        else {
            continue;
        };
        for project in changed {
            announce(&bus, &project);
        }
    }
}

/// `config.changed { changed: ["run/detected"] }` 를 그 프로젝트 범위로 낸다.
pub fn announce(bus: &EventBus, project_id: &str) {
    let payload = ConfigChanged {
        changed: vec![CHANGED_KEY.to_owned()],
        notices: Vec::new(),
    };
    tracing::info!(target: "pw_run", project = project_id, "detected tasks changed");
    if let Err(e) = bus.publish(
        Scope::Project(project_id.to_owned()),
        pw_core::config::TOPIC_CHANGED,
        &payload,
    ) {
        tracing::warn!(target: "pw_run", %e, "cannot publish config.changed");
    }
}
