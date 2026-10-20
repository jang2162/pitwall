//! 키마다 한 번만 계산하고, 성공한 결과만 남기는 캐시.
//!
//! - 같은 키를 동시에 부르면 계산은 한 번 돈다. 나머지는 그 결과를 기다려 함께 받는다(셸을 여러 번 띄우지 않는다).
//! - 계산이 끝나면 그 시도는 사라진다. `keep` 이 거른 결과(실패 · 시간 초과)는 그 시도를 기다린 쪽만 받고, 다음 호출은 다시
//!   계산한다 — 셸을 고친 뒤 앱을 다시 띄우지 않아도 된다.
//! - [`Cache::clear`] 는 남긴 결과와 진행 중인 시도를 함께 잊는다. 지우기 전에 시작한 계산은 끝나도 남기지 않는다(옛 설정으로
//!   계산했을 수 있다) — 끝난 계산은 자기 시도가 아직 표에 있을 때만 결과를 남긴다.

use std::collections::HashMap;
use std::hash::Hash;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};

pub struct Cache<K, V> {
    state: Mutex<State<K, V>>,
}

type Flight<V> = Arc<OnceLock<Arc<V>>>;

struct State<K, V> {
    ready: HashMap<K, Arc<V>>,
    flights: HashMap<K, Flight<V>>,
}

impl<K, V> Default for Cache<K, V> {
    fn default() -> Self {
        Self {
            state: Mutex::new(State {
                ready: HashMap::new(),
                flights: HashMap::new(),
            }),
        }
    }
}

impl<K: Clone + Eq + Hash, V> Cache<K, V> {
    fn lock(&self) -> MutexGuard<'_, State<K, V>> {
        // 잠금 안에서는 표에 넣고 빼기만 하므로 패닉으로 오염돼도 표가 어긋나지 않는다.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// 남긴 결과, 없으면 계산한 결과. 계산은 잠금 밖에서 돈다.
    pub fn get_or_compute(
        &self,
        key: &K,
        compute: impl FnOnce() -> V,
        keep: impl FnOnce(&V) -> bool,
    ) -> Arc<V> {
        let flight = {
            let mut state = self.lock();
            if let Some(value) = state.ready.get(key) {
                return Arc::clone(value);
            }
            Arc::clone(state.flights.entry(key.clone()).or_default())
        };
        // OnceLock 은 한 쪽만 계산하게 하고 나머지는 그 결과를 기다리게 한다.
        let value = Arc::clone(flight.get_or_init(|| Arc::new(compute())));
        let mut state = self.lock();
        if state
            .flights
            .get(key)
            .is_some_and(|f| Arc::ptr_eq(f, &flight))
        {
            state.flights.remove(key);
            if keep(&value) {
                state.ready.insert(key.clone(), Arc::clone(&value));
            }
        }
        value
    }

    /// 남긴 결과가 있는가.
    pub fn contains(&self, key: &K) -> bool {
        self.lock().ready.contains_key(key)
    }

    /// 모두 잊는다. 남겼던 결과와 진행 중이던 시도의 키를 돌려준다(중복 없이).
    pub fn clear(&self) -> Vec<K> {
        let mut state = self.lock();
        let mut keys: Vec<K> = state.ready.drain().map(|(k, _)| k).collect();
        for (k, _) in state.flights.drain() {
            if !keys.contains(&k) {
                keys.push(k);
            }
        }
        keys
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use super::*;

    /// 동시에 여러 번 불러도 계산은 한 번 돈다(첫 호출이 끝날 때까지 나머지가 기다린다).
    #[test]
    fn concurrent_callers_share_one_computation() {
        let cache = Arc::new(Cache::<(), usize>::default());
        let calls = Arc::new(AtomicUsize::new(0));
        let threads: Vec<_> = (0..4)
            .map(|_| {
                let cache = Arc::clone(&cache);
                let calls = Arc::clone(&calls);
                std::thread::spawn(move || {
                    *cache.get_or_compute(
                        &(),
                        || {
                            std::thread::sleep(Duration::from_millis(100));
                            calls.fetch_add(1, Ordering::SeqCst) + 7
                        },
                        |_| true,
                    )
                })
            })
            .collect();
        for t in threads {
            assert_eq!(t.join().unwrap(), 7);
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(cache.contains(&()));
    }

    /// 거른 결과는 남기지 않는다 — 다음 호출이 다시 계산한다.
    #[test]
    fn rejected_results_are_computed_again() {
        let cache = Cache::<&str, Result<u32, String>>::default();
        let failed = cache.get_or_compute(&"k", || Err("timeout".to_owned()), Result::is_ok);
        assert_eq!(*failed, Err("timeout".to_owned()));
        assert!(!cache.contains(&"k"));
        let ok = cache.get_or_compute(&"k", || Ok(1), Result::is_ok);
        assert_eq!(*ok, Ok(1));
        let again = cache.get_or_compute(&"k", || Ok(2), Result::is_ok);
        assert_eq!(*again, Ok(1));
    }

    /// 지우기 전에 시작한 계산은 끝나도 남기지 않는다.
    #[test]
    fn a_computation_started_before_clear_is_not_kept() {
        let cache = Arc::new(Cache::<(), u32>::default());
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (finish_tx, finish_rx) = std::sync::mpsc::channel::<()>();
        let worker = {
            let cache = Arc::clone(&cache);
            std::thread::spawn(move || {
                *cache.get_or_compute(
                    &(),
                    || {
                        started_tx.send(()).unwrap();
                        finish_rx.recv().unwrap();
                        1
                    },
                    |_| true,
                )
            })
        };
        started_rx.recv().unwrap();
        assert_eq!(cache.clear(), vec![()]);
        finish_tx.send(()).unwrap();
        assert_eq!(worker.join().unwrap(), 1);
        assert!(!cache.contains(&()));
        assert_eq!(*cache.get_or_compute(&(), || 2, |_| true), 2);
    }
}
