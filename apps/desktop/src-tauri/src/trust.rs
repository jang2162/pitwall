//! 프로젝트 신뢰(M1-05, `crates/pw-core/src/registry/trust.rs`)를 런타임 게이트(`pw_runtime::Runtime::with_trust`)에 잇는다.
//!
//! 런타임은 Host 보다 먼저 만든다(그 메서드가 Host 를 만드는 중에 붙는다). 레지스트리는 Host 가 만드므로 판정은 나중에 채우는
//! 칸을 본다 — 채우기 전에는 어느 프로젝트도 신뢰하지 않는다. 신뢰가 바뀌면(`project.changed` 의 `trust`) 그 프로젝트의
//! 「환경 다시 읽기」(`Runtime::reload`)로 캐시를 버리고 `runtime.changed` 를 낸다 — 새 터미널부터 제공자의 환경이 붙거나 빠진다.
//! 다시 읽기는 모든 디렉터리의 캐시를 버리므로 그 신뢰를 따르는 워크트리 프로젝트의 캐시도 함께 다시 읽힌다.

use std::sync::{Arc, OnceLock};

use pw_core::bus::{Received, Subscription};
use pw_core::registry::{ChangeKind, ProjectChanged, Registry, TOPIC_CHANGED};
use pw_core::{EventBus, Filter, Scope};
use pw_runtime::Runtime;

/// 런타임이 묻는 신뢰 판정. 레지스트리는 Host 를 만든 뒤 [`TrustGate::attach`] 로 채운다.
#[derive(Clone, Default)]
pub struct TrustGate(Arc<OnceLock<Registry>>);

impl TrustGate {
    /// `Runtime::with_trust` 에 넘기는 판정. 레지스트리가 없거나 읽지 못하면 미신뢰다.
    pub fn check(&self) -> impl Fn(&str) -> bool + Send + Sync + 'static {
        let slot = Arc::clone(&self.0);
        move |project| slot.get().is_some_and(|r| r.is_trusted(project))
    }

    /// 두 번째부터는 무시한다.
    pub fn attach(&self, registry: Registry) {
        let _ = self.0.set(registry);
    }
}

/// 레지스트리 변경 구독. Host 를 만든 직후에 연다.
pub fn subscribe(bus: &EventBus) -> Subscription {
    bus.subscribe(Filter::all().scope(Scope::Host).topics([TOPIC_CHANGED]))
}

/// 구독이 닫힐 때까지 신뢰 변경마다 그 프로젝트의 런타임 환경을 다시 읽게 한다.
pub async fn follow(mut events: Subscription, runtime: Arc<Runtime>) {
    while let Some(received) = events.recv().await {
        match received {
            Received::Event(event) => {
                if let Ok(ProjectChanged {
                    kind: ChangeKind::Trust,
                    project_id,
                }) = serde_json::from_value(event.payload.clone())
                {
                    runtime.reload(&project_id);
                }
            }
            Received::Lagged(n) => {
                tracing::warn!(
                    dropped = n,
                    "missed project registry events; a trust change may not reach the runtime cache until the next reload"
                );
            }
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::PermissionsExt as _;
    use std::path::Path;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use pw_core::bus::DEFAULT_CAPACITY;
    use pw_core::config::AppPaths;
    use pw_core::registry::NewProject;
    use pw_runtime::methods::RuntimeSource;
    use pw_runtime::{Layer, LoginShell, RuntimeEnv, RuntimeHost, RuntimeProvider};

    use super::*;

    /// 프로젝트 코드를 돌리는 제공자 흉내(mise 자리). 부른 횟수를 센다.
    struct Overlay(AtomicUsize);

    impl RuntimeProvider for Overlay {
        fn id(&self) -> &'static str {
            "overlay"
        }
        fn runs_project_code(&self) -> bool {
            true
        }
        fn applies_to(&self, _dir: &Path, _base: &RuntimeEnv) -> bool {
            true
        }
        fn resolve(&self, _dir: &Path, base: &RuntimeEnv) -> Result<Layer, String> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(Layer {
                env: base.clone(),
                source: RuntimeSource::ConfigFiles {
                    files: vec!["mise.toml".to_owned()],
                },
            })
        }
    }

    /// 로그인 셸 흉내: 받은 스크립트(`-l -i -c <스크립트>` 의 넷째 인자)를 그대로 돌린다.
    fn fake_shell(dir: &Path) -> std::path::PathBuf {
        let path = dir.join("fake-shell");
        std::fs::write(&path, "#!/bin/sh\neval \"$4\"\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    /// 신뢰를 바꾸면 그 프로젝트의 캐시가 버려져 다음 요청에서 제공자를 다시 고르고, 그 프로젝트로 `runtime.changed` 가 나간다.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn changing_trust_rereads_the_runtime_cache_and_announces_it() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("project");
        std::fs::create_dir_all(&root).unwrap();
        let bus = EventBus::new(DEFAULT_CAPACITY);
        let registry = Registry::new(AppPaths::under(&tmp.path().join("app")), None, bus.clone());
        let project = registry
            .create(NewProject {
                root: root.clone(),
                ..NewProject::default()
            })
            .unwrap();

        let gate = TrustGate::default();
        let overlay = Arc::new(Overlay(AtomicUsize::new(0)));
        let shell = LoginShell::new(Some(fake_shell(tmp.path())), Duration::from_secs(5));
        let runtime = Arc::new(
            Runtime::new(shell)
                .with_provider(overlay.clone())
                .with_trust(gate.check()),
        );
        // 붙이기 전에는 아무것도 신뢰하지 않는다.
        assert_eq!(runtime.env(&project.id, &root).info.provider, "shell");
        gate.attach(registry.clone());
        runtime.attach(RuntimeHost {
            projects: registry.clone(),
            bus: bus.clone(),
        });
        let follower = tokio::spawn(follow(subscribe(&bus), Arc::clone(&runtime)));
        let mut changed = bus.subscribe(
            Filter::all()
                .scope(Scope::Project(project.id.clone()))
                .topics([pw_runtime::TOPIC_CHANGED]),
        );

        // 캐시에 남은 미신뢰 결과: 신뢰를 바꾸지 않으면 그대로다.
        assert_eq!(runtime.env(&project.id, &root).info.provider, "shell");
        assert_eq!(overlay.0.load(Ordering::SeqCst), 0);

        registry.set_trust(&project.id, true).unwrap();
        tokio::time::timeout(Duration::from_secs(5), changed.recv())
            .await
            .expect("runtime.changed after trusting")
            .unwrap();
        assert_eq!(runtime.env(&project.id, &root).info.provider, "overlay");
        assert_eq!(overlay.0.load(Ordering::SeqCst), 1);

        registry.set_trust(&project.id, false).unwrap();
        tokio::time::timeout(Duration::from_secs(5), changed.recv())
            .await
            .expect("runtime.changed after untrusting")
            .unwrap();
        assert_eq!(runtime.env(&project.id, &root).info.provider, "shell");
        assert_eq!(overlay.0.load(Ordering::SeqCst), 1);
        follower.abort();
    }
}
