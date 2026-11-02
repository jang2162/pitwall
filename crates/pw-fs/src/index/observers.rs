//! 목록을 따라가는 쪽들([`FileListObserver`]) — 여럿을 받는다(심볼 인덱스 `pw-index` M4-15 의 훅을 언어 서버 파일 감시 M4-05b 가
//! 넓혔다, 15-m4-plan.md 「공유 부품의 주인」).
//!
//! 받는 방식이 둘이다:
//! - **바로**([`super::FileIndex::observe`]): 목록의 잠금 안에서 적용한 순서대로 부른다 — 받은 쪽이 「만드는 중」 자리를 그 잠금 안에서
//!   두어야 할 때(`pw-index` 의 `opened`). 느리면 그동안 파일 검색 · 변경 적용 · 감시 걸기가 기다린다 — 그래서 콜백은 채널에 넣거나
//!   표에 자리 하나를 두고 곧바로 돌아와야 한다.
//! - **차례로**([`super::FileIndex::observe_queued`]): 관찰자마다 전용 스레드(`pw-fs-observer`)가 같은 순서로 부른다. 잠금 안에서는
//!   그 스레드의 큐에 넣기만 하므로, 그 관찰자가 느려도 다른 관찰자와 목록은 기다리지 않는다. 큐에는 상한이 없다 — 상한을 두어
//!   넘친 사건을 버리면 관찰자가 목록과 갈라진다. 관찰자는 받은 것을 제 표에 넣고 곧바로 돌아와야 한다.
//!
//! 어느 쪽이든 콜백의 패닉은 그 호출에서 멈춘다(로그만 남는다) — 다른 관찰자와 목록의 적용은 이어진다. 붙인 관찰자는
//! [`super::FileIndex::unobserve`] 로 뗀다(차례로 받는 쪽은 큐에 남은 것을 다 부른 뒤 스레드가 끝난다). 붙은 뒤의 사건만 받는다 —
//! 이미 있는 루트의 목록은 보내지 않는다.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, PoisonError, RwLock};

use super::{FileFlags, FileListObserver};

/// 붙인 관찰자의 손잡이([`super::FileIndex::unobserve`] 로 뗀다).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ObserverId(u64);

/// 사건 하나.
#[derive(Clone)]
enum What {
    Opened,
    Rebuilt(Vec<(Arc<str>, FileFlags)>),
    Changed(Vec<String>, Vec<(Arc<str>, FileFlags)>),
    Suspended,
    Dropped,
}

#[derive(Clone)]
struct Call {
    project_id: String,
    root: PathBuf,
    what: What,
}

#[derive(Clone)]
enum Delivery {
    Inline(Arc<dyn FileListObserver>),
    Queued(Sender<Call>),
}

struct Slot {
    id: u64,
    delivery: Delivery,
}

/// 붙은 관찰자들. 목록(`RootIndex`) 들과 함께 쓴다.
#[derive(Default)]
pub(crate) struct Observers {
    slots: RwLock<Vec<Slot>>,
    next: AtomicU64,
}

fn dispatch(observer: &dyn FileListObserver, call: Call) {
    let Call {
        project_id,
        root,
        what,
    } = call;
    match what {
        What::Opened => observer.opened(&project_id, &root),
        What::Rebuilt(files) => observer.rebuilt(&project_id, &root, files),
        What::Changed(removed, inserted) => {
            observer.changed(&project_id, &root, removed, inserted);
        }
        What::Suspended => observer.suspended(&project_id, &root),
        What::Dropped => observer.dropped(&project_id, &root),
    }
}

/// 패닉을 그 호출에서 멈춘다 — 목록의 잠금을 쥔 채 패닉이 올라가면 그 변경 적용(블로킹 태스크)이 끝나 버린다.
fn guarded(observer: &dyn FileListObserver, call: Call) {
    let root = call.root.clone();
    if catch_unwind(AssertUnwindSafe(|| dispatch(observer, call))).is_err() {
        tracing::warn!(root = %root.display(), "a file list observer panicked; the list goes on");
    }
}

impl Observers {
    fn add(&self, delivery: Delivery) -> ObserverId {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        self.slots
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .push(Slot { id, delivery });
        ObserverId(id)
    }

    pub(crate) fn add_inline(&self, observer: Arc<dyn FileListObserver>) -> ObserverId {
        self.add(Delivery::Inline(observer))
    }

    pub(crate) fn add_queued(&self, observer: Arc<dyn FileListObserver>) -> ObserverId {
        let (tx, rx) = channel::<Call>();
        let spawned = std::thread::Builder::new()
            .name("pw-fs-observer".to_owned())
            .spawn(move || {
                for call in rx {
                    guarded(observer.as_ref(), call);
                }
            });
        if let Err(e) = spawned {
            // 받는 스레드가 없으면 큐에 넣은 것이 버려질 뿐이다(목록은 막히지 않는다).
            tracing::warn!(error = %e, "file list observer thread not started");
        }
        self.add(Delivery::Queued(tx))
    }

    pub(crate) fn remove(&self, id: ObserverId) -> bool {
        let mut slots = self.slots.write().unwrap_or_else(PoisonError::into_inner);
        let before = slots.len();
        slots.retain(|s| s.id != id.0);
        slots.len() != before
    }

    pub(crate) fn len(&self) -> usize {
        self.slots
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.slots
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .is_empty()
    }

    /// 붙은 관찰자 모두에게. 목록을 읽는 잠금은 부르기 전에 놓는다 — 콜백 안에서 붙이고 떼도 막히지 않게.
    fn deliver(&self, project_id: &str, root: &Path, what: What) {
        let targets: Vec<Delivery> = self
            .slots
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .map(|s| s.delivery.clone())
            .collect();
        let call = Call {
            project_id: project_id.to_owned(),
            root: root.to_path_buf(),
            what,
        };
        for target in targets {
            match target {
                Delivery::Inline(observer) => guarded(observer.as_ref(), call.clone()),
                // 받는 스레드가 끝났으면(뗌) 버린다.
                Delivery::Queued(tx) => {
                    let _ = tx.send(call.clone());
                }
            }
        }
    }

    pub(crate) fn opened(&self, project_id: &str, root: &Path) {
        self.deliver(project_id, root, What::Opened);
    }

    pub(crate) fn rebuilt(&self, project_id: &str, root: &Path, files: Vec<(Arc<str>, FileFlags)>) {
        self.deliver(project_id, root, What::Rebuilt(files));
    }

    pub(crate) fn changed(
        &self,
        project_id: &str,
        root: &Path,
        removed: Vec<String>,
        inserted: Vec<(Arc<str>, FileFlags)>,
    ) {
        self.deliver(project_id, root, What::Changed(removed, inserted));
    }

    pub(crate) fn suspended(&self, project_id: &str, root: &Path) {
        self.deliver(project_id, root, What::Suspended);
    }

    pub(crate) fn dropped(&self, project_id: &str, root: &Path) {
        self.deliver(project_id, root, What::Dropped);
    }
}
