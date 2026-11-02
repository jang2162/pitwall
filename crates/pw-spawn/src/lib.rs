//! 자식 프로세스를 띄우는(fork 하는) 전용 스레드.
//!
//! Linux 의 `PR_SET_PDEATHSIG` 는 프로세스가 아니라 **fork 한 스레드**가 끝날 때 자식에게 신호를 보낸다. tokio 런타임의 스레드는
//! 끝날 수 있다 — 블로킹 풀 스레드는 쉬다 끝나고(기본 10 s), 일꾼 스레드도 `block_in_place` 를 한 번 하면 블로킹 풀로 넘어가 같이
//! 끝난다. 그 스레드에서 띄운 자식은 앱이 살아 있는데도 조용히 끝난다(macOS 에는 없는 증상 — M4-05 리뷰 M4, M3-11 의 ssh).
//! 그래서 `PR_SET_PDEATHSIG` 를 거는 자식은 [`on_launcher`] 로만 띄운다: fork 는 프로세스 수명 동안 사는 스레드 하나
//! (`pw-launcher`)가 하고, 띄우라고 하는 쪽은 어느 스레드여도 된다.
//!
//! 그 스레드는 띄우기만 한다. 자식을 기다리고 파이프를 읽는 것은 부른 쪽 런타임의 태스크다 — 띄우기를 부른 쪽 런타임에 들어가서
//! 하므로(`Handle::enter`) tokio 의 `Child` · 파이프가 그 런타임에 붙는다.

use std::io;
use std::sync::OnceLock;
use std::sync::mpsc::Sender;

/// 띄우기 스레드의 이름.
pub const THREAD_NAME: &str = "pw-launcher";

type Job = Box<dyn FnOnce() + Send>;

/// 띄우기 스레드. 보내는 쪽을 `static` 이 쥐어 받는 고리가 끝나지 않는다 — 프로세스가 끝날 때까지 산다. 시작하지 못했으면
/// 이후의 띄우기는 모두 오류다(다른 스레드에서 fork 하는 길로 새지 않는다).
fn launcher() -> io::Result<&'static Sender<Job>> {
    static LAUNCHER: OnceLock<Option<Sender<Job>>> = OnceLock::new();
    LAUNCHER
        .get_or_init(|| {
            let (tx, rx) = std::sync::mpsc::channel::<Job>();
            let started = std::thread::Builder::new()
                .name(THREAD_NAME.to_owned())
                .spawn(move || {
                    for job in rx {
                        // 한 번의 패닉이 이 스레드를(그래서 그 뒤의 모든 띄우기를) 끝내지 않게.
                        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job));
                    }
                });
            match started {
                Ok(_) => Some(tx),
                Err(e) => {
                    tracing::error!(target: "pw_spawn", %e, "cannot start the process launcher thread");
                    None
                }
            }
        })
        .as_ref()
        .ok_or_else(|| io::Error::other("the process launcher thread is not running"))
}

/// `spawn`(자식을 띄우는 일 — 보통 `tokio::process::Command::spawn`)을 띄우기 스레드에서 부르고 그 결과를 돌려준다. 부른 쪽의
/// tokio 런타임에 들어가서 부르므로 돌려받은 `Child` 의 대기 · 파이프는 그 런타임에서 돈다. tokio 런타임 안에서 부른다.
///
/// 기다리던 쪽이 결과를 받기 전에 사라지면 결과는 띄우기 스레드에서 (그 런타임에 든 채로) 버려진다 — 자식을 남기지 않으려면
/// `kill_on_drop` 을 켠다. `spawn` 이 패닉하면 오류다(스레드는 계속 산다).
pub async fn on_launcher<T, F>(spawn: F) -> io::Result<T>
where
    F: FnOnce() -> io::Result<T> + Send + 'static,
    T: Send + 'static,
{
    let runtime = tokio::runtime::Handle::try_current()
        .map_err(|e| io::Error::other(format!("on_launcher needs a tokio runtime: {e}")))?;
    let (tx, rx) = tokio::sync::oneshot::channel();
    launcher()?
        .send(Box::new(move || {
            let _entered = runtime.enter();
            // 보내기가 실패하면(기다리던 쪽이 사라짐) 결과는 이 문장 끝에서 런타임에 든 채로 버려진다.
            let _ = tx.send(spawn());
        }))
        .map_err(|_| io::Error::other("the process launcher thread is not running"))?;
    rx.await
        .map_err(|_| io::Error::other("the process launcher thread dropped the launch"))?
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn thread_name() -> Option<String> {
        std::thread::current().name().map(str::to_owned)
    }

    /// 어느 스레드에서 부르든(일꾼 · 곧 끝나는 블로킹 풀 스레드) 띄우기는 같은 `pw-launcher` 스레드에서 돌고, 그 안에서 부른 쪽
    /// 런타임에 들어가 있다.
    #[test]
    fn every_launch_runs_on_the_one_launcher_thread_inside_the_callers_runtime() {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_keep_alive(Duration::from_millis(50))
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let probe = || {
                Ok((
                    std::thread::current().id(),
                    thread_name(),
                    tokio::runtime::Handle::try_current().is_ok(),
                ))
            };
            let first = on_launcher(probe).await.unwrap();
            let handle = tokio::runtime::Handle::current();
            let from_blocking =
                tokio::task::spawn_blocking(move || handle.block_on(on_launcher(probe)))
                    .await
                    .unwrap()
                    .unwrap();
            // 그 블로킹 스레드가 끝난 뒤에도.
            tokio::time::sleep(Duration::from_millis(300)).await;
            let later = on_launcher(probe).await.unwrap();
            assert_eq!(first.1.as_deref(), Some(THREAD_NAME));
            assert!(first.2, "the launch did not enter the caller's runtime");
            assert_eq!(first.0, from_blocking.0);
            assert_eq!(first.0, later.0);
        });
    }

    /// 띄우기 하나가 패닉해도 그 띄우기만 오류이고 스레드는 산다.
    #[tokio::test]
    async fn a_panicking_launch_fails_alone() {
        let e = on_launcher(|| -> io::Result<()> { panic!("boom") })
            .await
            .unwrap_err();
        assert!(e.to_string().contains("dropped the launch"), "{e}");
        assert_eq!(on_launcher(|| Ok(7)).await.unwrap(), 7);
        assert_eq!(
            on_launcher(|| Ok(thread_name())).await.unwrap().as_deref(),
            Some(THREAD_NAME)
        );
    }

    /// 띄우기의 오류는 그대로 돌아온다.
    #[tokio::test]
    async fn a_launch_error_comes_back_as_is() {
        let e = on_launcher(|| -> io::Result<()> {
            Err(io::Error::new(io::ErrorKind::NotFound, "no such program"))
        })
        .await
        .unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::NotFound);
    }
}
