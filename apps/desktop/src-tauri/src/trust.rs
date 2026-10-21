//! 프로젝트 신뢰(M1-05, `crates/pw-core/src/registry/trust.rs`)를 런타임 게이트(`pw_runtime::Runtime::with_trust`)에 잇는다.
//!
//! 런타임은 Host 보다 먼저 만든다(그 메서드가 Host 를 만드는 중에 붙는다). 레지스트리는 Host 가 만드므로 판정은 나중에 채우는
//! 칸을 본다 — 채우기 전에는 어느 프로젝트도 신뢰하지 않는다. 신뢰가 바뀌면(`project.changed` 의 `trust`) 그 신뢰를 쓰는
//! 프로젝트들(신뢰를 가진 프로젝트와 그 워크트리 프로젝트)의 런타임 캐시만 버리고 `runtime.changed`(`trust`)를 낸다
//! (`Runtime::trust_changed`) — 새 터미널부터 제공자의 환경이 붙거나 빠진다. 로그인 셸과 다른 프로젝트의 캐시는 그대로다.
//! 신뢰를 가진 프로젝트가 바뀌는 경우(메인 저장소 프로젝트의 등록 · 삭제 · 루트 변경)는 이벤트가 없어도 런타임 캐시 키의 신뢰가
//! 옛 환경을 막는다. 이벤트를 놓치면(`Lagged`) 무엇이 바뀌었는지 모르므로 전역 다시 읽기(`Runtime::reload_all`)를 한다.

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

/// 구독이 닫힐 때까지 신뢰 변경마다 그 신뢰를 쓰는 프로젝트들의 런타임 환경을 다시 읽게 한다.
pub async fn follow(mut events: Subscription, registry: Registry, runtime: Arc<Runtime>) {
    while let Some(received) = events.recv().await {
        match received {
            Received::Event(event) => {
                let Ok(ProjectChanged {
                    kind: ChangeKind::Trust,
                    project_id,
                }) = serde_json::from_value(event.payload.clone())
                else {
                    continue;
                };
                // 레지스트리를 읽고 워크트리 판정으로 디스크(`.git`)를 본다.
                let registry = registry.clone();
                let runtime = Arc::clone(&runtime);
                let done = tauri::async_runtime::spawn_blocking(move || {
                    match registry.trust_followers(&project_id) {
                        Ok(projects) => runtime.trust_changed(&projects),
                        Err(e) => {
                            tracing::warn!(%e, project_id, "cannot list the projects following a trust change; reloading every runtime environment");
                            runtime.reload_all();
                        }
                    }
                });
                if let Err(e) = done.await {
                    tracing::warn!(%e, "trust change handler failed");
                }
            }
            Received::Lagged(n) => {
                tracing::warn!(
                    dropped = n,
                    "missed project registry events; reloading every runtime environment"
                );
                runtime.reload_all();
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
    use pw_runtime::{Layer, LoginShell, ProviderError, RuntimeEnv, RuntimeHost, RuntimeProvider};

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
        fn resolve(&self, _dir: &Path, base: &RuntimeEnv) -> Result<Layer, ProviderError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(Layer {
                env: base.clone(),
                source: RuntimeSource::ConfigFiles {
                    files: vec!["mise.toml".to_owned()],
                },
            })
        }
    }

    /// 로그인 셸 흉내: 받은 스크립트(`-l -i -c <스크립트>` 의 넷째 인자)를 그대로 돌리고, 돈 횟수를 `runs` 파일에 한 줄씩 남긴다.
    fn fake_shell(dir: &Path) -> std::path::PathBuf {
        let path = dir.join("fake-shell");
        let runs = dir.join("runs");
        std::fs::write(
            &path,
            format!("#!/bin/sh\necho run >> '{}'\neval \"$4\"\n", runs.display()),
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    fn shell_runs(dir: &Path) -> usize {
        std::fs::read_to_string(dir.join("runs")).map_or(0, |s| s.lines().count())
    }

    struct Fixture {
        tmp: tempfile::TempDir,
        bus: EventBus,
        registry: Registry,
        overlay: Arc<Overlay>,
        runtime: Arc<Runtime>,
        follower: tokio::task::JoinHandle<()>,
    }

    impl Fixture {
        fn new() -> Self {
            let tmp = tempfile::tempdir().unwrap();
            let bus = EventBus::new(DEFAULT_CAPACITY);
            let registry =
                Registry::new(AppPaths::under(&tmp.path().join("app")), None, bus.clone());
            let gate = TrustGate::default();
            let overlay = Arc::new(Overlay(AtomicUsize::new(0)));
            let shell = LoginShell::new(Some(fake_shell(tmp.path())), Duration::from_secs(5));
            let runtime = Arc::new(
                Runtime::new(shell)
                    .with_provider(overlay.clone())
                    .with_trust(gate.check()),
            );
            gate.attach(registry.clone());
            runtime.attach(RuntimeHost {
                projects: registry.clone(),
                bus: bus.clone(),
                paths: AppPaths::under(&tmp.path().join("app")),
            });
            let follower = tokio::spawn(follow(
                subscribe(&bus),
                registry.clone(),
                Arc::clone(&runtime),
            ));
            Self {
                tmp,
                bus,
                registry,
                overlay,
                runtime,
                follower,
            }
        }

        fn project(&self, rel: &str) -> (String, std::path::PathBuf) {
            let root = self.tmp.path().join(rel);
            std::fs::create_dir_all(&root).unwrap();
            let root = std::fs::canonicalize(root).unwrap();
            let id = self
                .registry
                .create(NewProject {
                    root: root.clone(),
                    ..NewProject::default()
                })
                .unwrap()
                .id;
            (id, root)
        }

        fn changed(&self, project: &str) -> pw_core::Subscription {
            self.bus.subscribe(
                Filter::all()
                    .scope(Scope::Project(project.to_owned()))
                    .topics([pw_runtime::TOPIC_CHANGED]),
            )
        }

        fn provider(&self, project: &str, dir: &Path) -> String {
            self.runtime.env(project, dir).info.provider.clone()
        }

        fn calls(&self) -> usize {
            self.overlay.0.load(Ordering::SeqCst)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            self.follower.abort();
        }
    }

    async fn next_reason(sub: &mut pw_core::Subscription) -> String {
        match tokio::time::timeout(Duration::from_secs(5), sub.recv())
            .await
            .expect("runtime.changed")
            .unwrap()
        {
            Received::Event(e) => e.payload["reason"].as_str().unwrap().to_owned(),
            Received::Lagged(n) => panic!("lagged {n}"),
        }
    }

    async fn nothing_arrives(sub: &mut pw_core::Subscription) {
        assert!(
            tokio::time::timeout(Duration::from_millis(300), sub.recv())
                .await
                .is_err(),
            "unexpected runtime.changed"
        );
    }

    /// 신뢰를 바꾸면 그 프로젝트의 캐시만 버려 다음 요청에서 제공자를 다시 고르고, 그 프로젝트로 `runtime.changed`(`trust`)가
    /// 나간다. 로그인 셸은 다시 돌지 않고 다른 프로젝트에는 아무것도 나가지 않는다. 같은 값으로 바꾸면 아무 일도 없다.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn changing_trust_rereads_only_that_project_and_announces_it() {
        let f = Fixture::new();
        let (id, root) = f.project("project");
        let (other, other_root) = f.project("other");
        let mut changed = f.changed(&id);
        let mut other_changed = f.changed(&other);
        assert_eq!(f.provider(&id, &root), "shell");
        assert_eq!(f.provider(&other, &other_root), "shell");
        assert_eq!(shell_runs(f.tmp.path()), 1);

        f.registry.set_trust(&id, true).unwrap();
        assert_eq!(next_reason(&mut changed).await, "trust");
        assert_eq!(f.provider(&id, &root), "overlay");
        assert_eq!(f.calls(), 1);

        f.registry.set_trust(&id, false).unwrap();
        assert_eq!(next_reason(&mut changed).await, "trust");
        assert_eq!(f.provider(&id, &root), "shell");
        // 해제할 때 신뢰 시절의 캐시도 버렸다 — 다시 신뢰하면 제공자를 다시 부른다.
        f.registry.set_trust(&id, true).unwrap();
        assert_eq!(next_reason(&mut changed).await, "trust");
        assert_eq!(f.provider(&id, &root), "overlay");
        assert_eq!(f.calls(), 2);

        // 같은 값: 쓰지 않고 이벤트도 다시 읽기도 없다.
        f.registry.set_trust(&id, true).unwrap();
        nothing_arrives(&mut changed).await;
        assert_eq!(f.provider(&id, &root), "overlay");
        assert_eq!(f.calls(), 2);

        nothing_arrives(&mut other_changed).await;
        assert_eq!(shell_runs(f.tmp.path()), 1, "login shell is not re-read");
    }

    /// 워크트리 프로젝트의 신뢰를 가진 프로젝트가 바뀌면(메인 저장소 프로젝트의 등록 · 삭제) 이벤트 없이도 옛 신뢰의 환경을 주지
    /// 않는다. 메인 저장소의 신뢰를 바꾸면 워크트리 프로젝트로도 `runtime.changed` 가 나간다.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_change_of_trust_owner_does_not_keep_the_old_environment() {
        let f = Fixture::new();
        // `git worktree add` 와 같은 모양: 관리 폴더는 `<main>/.git/worktrees/wt`.
        let admin = f.tmp.path().join("main/.git/worktrees/wt");
        std::fs::create_dir_all(&admin).unwrap();
        std::fs::create_dir_all(f.tmp.path().join("wt")).unwrap();
        let wt_dot_git = std::fs::canonicalize(f.tmp.path().join("wt"))
            .unwrap()
            .join(".git");
        std::fs::write(admin.join("commondir"), "../..\n").unwrap();
        std::fs::write(admin.join("gitdir"), format!("{}\n", wt_dot_git.display())).unwrap();
        std::fs::write(&wt_dot_git, format!("gitdir: {}\n", admin.display())).unwrap();
        let (wt, wt_root) = f.project("wt");
        f.registry.set_trust(&wt, true).unwrap();
        assert_eq!(f.provider(&wt, &wt_root), "overlay");

        // 메인 저장소 프로젝트(아직 묻지 않음 = 미신뢰)를 등록하면 워크트리는 그 신뢰를 따른다.
        let (main, _) = f.project("main");
        assert_eq!(f.registry.trust(&wt).unwrap().owner_id, main);
        assert_eq!(f.provider(&wt, &wt_root), "shell");

        let mut wt_changed = f.changed(&wt);
        f.registry.set_trust(&main, true).unwrap();
        assert_eq!(next_reason(&mut wt_changed).await, "trust");
        assert_eq!(f.provider(&wt, &wt_root), "overlay");

        f.registry.set_trust(&main, false).unwrap();
        assert_eq!(next_reason(&mut wt_changed).await, "trust");
        assert_eq!(f.provider(&wt, &wt_root), "shell");

        // 메인 저장소 프로젝트를 지우면 워크트리 프로젝트는 다시 자기 신뢰(신뢰)다.
        f.registry.delete(&main).unwrap();
        assert_eq!(f.provider(&wt, &wt_root), "overlay");
    }

    /// 레지스트리 이벤트를 놓치면 무엇이 바뀌었는지 모르므로 전역 다시 읽기를 한다.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn missed_registry_events_reload_every_runtime_environment() {
        let tmp = tempfile::tempdir().unwrap();
        // 용량 1: 두 번째 이벤트가 첫 번째를 밀어내 구독자는 `Lagged` 를 받는다.
        let bus = EventBus::new(1);
        let shell = LoginShell::new(Some(fake_shell(tmp.path())), Duration::from_secs(5));
        let runtime = Arc::new(Runtime::new(shell));
        let registry = Registry::new(AppPaths::under(&tmp.path().join("app")), None, bus.clone());
        runtime.attach(RuntimeHost {
            projects: registry.clone(),
            bus: bus.clone(),
            paths: AppPaths::under(&tmp.path().join("app")),
        });
        let dir = tmp.path().join("p");
        std::fs::create_dir_all(&dir).unwrap();
        runtime.env("p", &dir);
        assert_eq!(shell_runs(tmp.path()), 1);
        let events = subscribe(&bus);
        let mut changed = bus.subscribe(
            Filter::all()
                .scope(Scope::Project("p".to_owned()))
                .topics([pw_runtime::TOPIC_CHANGED]),
        );
        for kind in ["created", "deleted"] {
            bus.publish(
                Scope::Host,
                TOPIC_CHANGED,
                &serde_json::json!({"kind": kind, "projectId": "x"}),
            )
            .unwrap();
        }
        let follower = tokio::spawn(follow(events, registry, Arc::clone(&runtime)));
        assert_eq!(next_reason(&mut changed).await, "reload");
        runtime.env("p", &dir);
        assert_eq!(shell_runs(tmp.path()), 2, "the login shell is read again");
        follower.abort();
    }
}
