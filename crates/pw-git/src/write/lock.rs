//! 저장소마다 쓰기를 한 줄로 세우는 잠금. 읽기(gix · `git.repo` · `git.operation`)는 이 잠금을 보지 않는다.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError, Weak};

use tokio::sync::Mutex as AsyncMutex;

/// 공유 git 디렉터리(`commonDir`)마다 잠금 하나 — 연결 워크트리끼리 refs · objects 를 함께 쓰므로 한 저장소의 워크트리들도 같은
/// 줄에 선다. 먼저 기다린 쪽이 먼저 받는다(tokio 의 `Mutex` 는 공정하다). 아무도 쥐거나 기다리지 않는 잠금은 표에서 걷는다.
#[derive(Default)]
pub(crate) struct RepoLocks(Mutex<HashMap<PathBuf, Weak<AsyncMutex<()>>>>);

impl RepoLocks {
    pub(crate) fn get(&self, common_dir: &Path) -> Arc<AsyncMutex<()>> {
        let mut map = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        map.retain(|_, lock| lock.strong_count() > 0);
        if let Some(lock) = map.get(common_dir).and_then(Weak::upgrade) {
            return lock;
        }
        let lock = Arc::new(AsyncMutex::new(()));
        map.insert(common_dir.to_path_buf(), Arc::downgrade(&lock));
        lock
    }
}
