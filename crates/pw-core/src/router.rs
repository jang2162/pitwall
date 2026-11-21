//! Host Router: (Host, project_id) → Host. UI · MCP 가 어느 Host 에 연결할지 정한다(`docs/design/03-architecture.md` 「전체 구조」 절).
//!
//! 라우팅은 **연결 단위**다. 연결 하나는 한 Host 의 pw-rpc 세션 하나이고, 그 위의 스트림 id · 구독 id ·
//! 요청 id 는 그 세션 안에서만 유일하다. 요청마다 Host 를 고르면 `stream.credit` 처럼 projectId 가 없는 제어 메시지를
//! 어느 Host 로 보낼지와 Host 사이의 id 충돌을 transport 가 따로 풀어야 한다.
//! 계약(재바인딩하면 기존 연결을 닫고 새로 연다 등)은 `docs/design/03-architecture.md` 「RPC 설계」 절이 정본이다.
//! `bind_project` 는 이미 열린 연결을 옮기지 않는다.
//!
//! **표의 키는 (Host, 프로젝트 id) 다**(18-m7-plan M7-03 ⑥). 프로젝트 id 는 머신마다 레지스트리가 무작위로 만들어 머신 사이에
//! 유일하지 않다(`$DATA` 를 복사한 VM · 복원본이면 그대로 같다). 그래서 원격 프로젝트는 [`Target::Member`] 로 Host 를 함께
//! 대고, [`Target::ProjectId`] 는 이 머신(원격이 아닌 Host — [`Host::is_remote`])의 프로젝트로만 읽는다. 원격 프로젝트의 id 가
//! 로컬 프로젝트의 id 와 같아도 로컬 창의 연결이 원격으로 가지 않는다.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, PoisonError, RwLock};

use pw_rpc::{Caller, ConnectionScope};

use crate::ProjectId;
use crate::host::{Connection, Host, HostError, HostId, Target};

#[derive(Default)]
pub struct HostRouter {
    state: RwLock<State>,
}

#[derive(Default)]
struct State {
    hosts: HashMap<HostId, Arc<dyn Host>>,
    /// 묶인 (Host, 프로젝트). 원격이 아닌 Host 사이에서는 한 프로젝트가 한 Host 에만 묶인다([`HostRouter::bind_project`]).
    projects: HashSet<(HostId, ProjectId)>,
}

impl State {
    fn is_local(&self, host: &str) -> bool {
        self.hosts.get(host).is_some_and(|h| !h.is_remote())
    }

    /// 이 머신(원격이 아닌 Host)에서 그 프로젝트가 묶인 Host.
    fn local_host_of(&self, project: &str) -> Option<HostId> {
        self.projects
            .iter()
            .find(|(host, p)| p == project && self.is_local(host))
            .map(|(host, _)| host.clone())
    }
}

impl HostRouter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_host(&self, host: Arc<dyn Host>) -> Result<(), HostError> {
        let mut state = self.state.write().unwrap_or_else(PoisonError::into_inner);
        let id = host.id().to_owned();
        if state.hosts.contains_key(&id) {
            return Err(HostError::DuplicateHost(id));
        }
        state.hosts.insert(id, host);
        Ok(())
    }

    /// Host 를 뺀다. 그 Host 에 묶인 프로젝트도 함께 풀린다. 이미 열린 연결은 그대로다(연결은 Host 가 쥔다).
    pub fn remove_host(&self, id: &str) -> Option<Arc<dyn Host>> {
        let mut state = self.state.write().unwrap_or_else(PoisonError::into_inner);
        let host = state.hosts.remove(id)?;
        state.projects.retain(|(h, _)| h != id);
        Some(host)
    }

    /// 프로젝트를 Host 에 묶는다. 이 머신의 Host 끼리는 한 프로젝트가 한 Host 에만 묶인다 — 이미 다른 로컬 Host 에 묶여 있으면 새
    /// Host 로 바꾼다. 원격 Host 의 묶음은 (Host, 프로젝트)마다 따로라 다른 머신의 같은 id 를 건드리지 않는다.
    pub fn bind_project(&self, project: ProjectId, host: &str) -> Result<(), HostError> {
        let mut state = self.state.write().unwrap_or_else(PoisonError::into_inner);
        let Some(target) = state.hosts.get(host) else {
            return Err(HostError::UnknownHost(host.to_owned()));
        };
        if !target.is_remote()
            && let Some(old) = state.local_host_of(&project)
        {
            state.projects.remove(&(old, project.clone()));
        }
        state.projects.insert((host.to_owned(), project));
        Ok(())
    }

    /// 이 머신의 Host 에서 프로젝트를 푼다([`Target::ProjectId`] 의 묶음).
    pub fn unbind_project(&self, project: &str) -> bool {
        let mut state = self.state.write().unwrap_or_else(PoisonError::into_inner);
        match state.local_host_of(project) {
            Some(host) => state.projects.remove(&(host, project.to_owned())),
            None => false,
        }
    }

    /// 어느 Host 의 프로젝트 묶음을 푼다([`Target::Member`] 의 묶음).
    pub fn unbind_member(&self, host: &str, project: &str) -> bool {
        let mut state = self.state.write().unwrap_or_else(PoisonError::into_inner);
        state
            .projects
            .remove(&(host.to_owned(), project.to_owned()))
    }

    pub fn host(&self, id: &str) -> Result<Arc<dyn Host>, HostError> {
        let state = self.state.read().unwrap_or_else(PoisonError::into_inner);
        state
            .hosts
            .get(id)
            .cloned()
            .ok_or_else(|| HostError::UnknownHost(id.to_owned()))
    }

    /// 이 머신의 프로젝트의 Host. 모르는 프로젝트는 [`HostError::UnknownProject`] — 원격 Host 에만 묶인 id 도 모르는 프로젝트다.
    pub fn route(&self, project: &str) -> Result<Arc<dyn Host>, HostError> {
        let host_id = {
            let state = self.state.read().unwrap_or_else(PoisonError::into_inner);
            state
                .local_host_of(project)
                .ok_or_else(|| HostError::UnknownProject(project.to_owned()))?
        };
        self.host(&host_id)
    }

    /// 그 Host 에 묶인 프로젝트면 그 Host. 묶이지 않았으면 [`HostError::UnknownProject`].
    pub fn route_member(&self, host: &str, project: &str) -> Result<Arc<dyn Host>, HostError> {
        {
            let state = self.state.read().unwrap_or_else(PoisonError::into_inner);
            if !state
                .projects
                .contains(&(host.to_owned(), project.to_owned()))
            {
                return Err(HostError::UnknownProject(project.to_owned()));
            }
        }
        self.host(host)
    }

    pub fn resolve(&self, target: &Target) -> Result<Arc<dyn Host>, HostError> {
        match target {
            Target::HostId(id) => self.host(id),
            Target::ProjectId(id) => self.route(id),
            Target::Member { host, project } => self.route_member(host, project),
        }
    }

    /// 프로젝트로 고른 연결은 그 프로젝트를 범위로 싣는다([`ConnectionScope::Project`]). 이미 범위가 좁은 호출자(파일 하나)는
    /// 그대로다.
    pub async fn connect(
        &self,
        target: &Target,
        mut caller: Caller,
    ) -> Result<Connection, HostError> {
        let host = self.resolve(target)?;
        let project = match target {
            Target::HostId(_) => None,
            Target::ProjectId(id) | Target::Member { project: id, .. } => Some(id),
        };
        if let (Some(id), ConnectionScope::Host) = (project, &caller.scope) {
            caller.scope = ConnectionScope::Project(id.clone());
        }
        host.connect(caller).await
    }
}

#[cfg(test)]
mod tests {
    use pw_rpc::Role;

    use super::*;
    use crate::LocalHost;

    fn router_with_local() -> HostRouter {
        let router = HostRouter::new();
        router.add_host(Arc::new(LocalHost::new("local"))).unwrap();
        router
    }

    #[test]
    fn routes_a_bound_project_to_its_host() {
        let router = router_with_local();
        router.bind_project("p1".into(), "local").unwrap();
        assert_eq!(router.route("p1").unwrap().id(), "local");
        assert_eq!(
            router
                .resolve(&Target::ProjectId("p1".into()))
                .unwrap()
                .id(),
            "local"
        );
    }

    #[test]
    fn unknown_project_is_an_explicit_error() {
        let router = router_with_local();
        assert_eq!(
            router.route("nope").err(),
            Some(HostError::UnknownProject("nope".into()))
        );
        router.bind_project("p1".into(), "local").unwrap();
        assert!(router.unbind_project("p1"));
        assert_eq!(
            router.route("p1").err(),
            Some(HostError::UnknownProject("p1".into()))
        );
    }

    #[test]
    fn binding_to_an_unknown_host_is_rejected() {
        let router = router_with_local();
        assert_eq!(
            router.bind_project("p1".into(), "remote-1"),
            Err(HostError::UnknownHost("remote-1".into()))
        );
        assert_eq!(
            router.host("remote-1").err(),
            Some(HostError::UnknownHost("remote-1".into()))
        );
    }

    #[test]
    fn duplicate_host_id_is_rejected() {
        let router = router_with_local();
        assert_eq!(
            router.add_host(Arc::new(LocalHost::new("local"))),
            Err(HostError::DuplicateHost("local".into()))
        );
    }

    #[tokio::test]
    async fn connect_to_unknown_project_fails_without_touching_a_host() {
        let router = router_with_local();
        let result = router
            .connect(&Target::ProjectId("p9".into()), Caller::new(Role::Owner))
            .await;
        assert_eq!(
            result.err().map(|e| e.to_string()),
            Some("unknown project `p9`".into())
        );
    }

    /// 연결을 열 때 받은 호출자를 적어 두는 Host.
    struct Recording {
        inner: LocalHost,
        seen: std::sync::Mutex<Vec<Caller>>,
    }

    impl Host for Recording {
        fn id(&self) -> &str {
            self.inner.id()
        }

        fn connect(
            &self,
            caller: Caller,
        ) -> crate::host::BoxFuture<'_, Result<Connection, HostError>> {
            self.seen.lock().unwrap().push(caller.clone());
            self.inner.connect(caller)
        }
    }

    /// 프로젝트로 고른 연결은 그 프로젝트를 범위로 싣고(프로젝트 창의 연결 — `project.state.set` 이 `Ctx::scope` 로 가른다),
    /// Host 로 고른 연결은 Host 범위다. 이미 파일 하나로 좁힌 호출자는 프로젝트로 넓히지 않는다.
    #[tokio::test]
    async fn connecting_by_project_carries_the_project_as_the_scope() {
        let router = HostRouter::new();
        let host = Arc::new(Recording {
            inner: LocalHost::new("local"),
            seen: std::sync::Mutex::default(),
        });
        router.add_host(host.clone()).unwrap();
        router.bind_project("p1".into(), "local").unwrap();
        let by_project = Target::ProjectId("p1".into());
        let by_host = Target::HostId("local".into());
        let file = Caller::file(Role::Owner, "/x/a.txt".into());
        for (target, caller) in [
            (&by_project, Caller::new(Role::Owner)),
            (&by_host, Caller::new(Role::Readonly)),
            (&by_project, file.clone()),
        ] {
            assert!(router.connect(target, caller).await.is_ok());
        }
        let seen = host.seen.lock().unwrap().clone();
        assert_eq!(
            seen.iter().map(|c| c.scope.clone()).collect::<Vec<_>>(),
            [
                ConnectionScope::Project("p1".into()),
                ConnectionScope::Host,
                file.scope.clone(),
            ]
        );
        assert_eq!(seen[1].role, Role::Readonly);
    }

    /// 다른 머신의 Host 를 흉내 낸다(`is_remote`). 연결은 안쪽 LocalHost 가 연다.
    struct Remote(LocalHost);

    impl Host for Remote {
        fn id(&self) -> &str {
            self.0.id()
        }

        fn connect(
            &self,
            caller: Caller,
        ) -> crate::host::BoxFuture<'_, Result<Connection, HostError>> {
            self.0.connect(caller)
        }

        fn is_remote(&self) -> bool {
            true
        }
    }

    /// 프로젝트 id 는 머신마다 따로 만든다 — 원격 Host 의 프로젝트가 로컬 프로젝트와 같은 id 여도 라우팅이 갈린다(18-m7-plan M7-03
    /// ⑥). 표의 키를 프로젝트 id 하나로 되돌리면 두 번째 묶음이 첫 묶음을 덮어 한쪽이 다른 Host 로 간다.
    #[test]
    fn the_same_project_id_on_two_hosts_routes_apart() {
        let router = router_with_local();
        router
            .add_host(Arc::new(Remote(LocalHost::new("mini"))))
            .unwrap();
        router.bind_project("p1".into(), "local").unwrap();
        router.bind_project("p1".into(), "mini").unwrap();

        let local = Target::ProjectId("p1".into());
        let remote = Target::Member {
            host: "mini".into(),
            project: "p1".into(),
        };
        assert_eq!(router.resolve(&local).unwrap().id(), "local");
        assert_eq!(router.resolve(&remote).unwrap().id(), "mini");

        // 원격 묶음을 풀어도 로컬 묶음은 그대로, 그 반대도 같다.
        assert!(router.unbind_member("mini", "p1"));
        assert_eq!(router.resolve(&local).unwrap().id(), "local");
        assert_eq!(
            router.resolve(&remote).err(),
            Some(HostError::UnknownProject("p1".into()))
        );
        router.bind_project("p1".into(), "mini").unwrap();
        assert!(router.unbind_project("p1"));
        assert_eq!(router.resolve(&remote).unwrap().id(), "mini");
        assert_eq!(
            router.resolve(&local).err(),
            Some(HostError::UnknownProject("p1".into()))
        );
    }

    /// 원격 Host 에만 묶인 id 를 [`Target::ProjectId`] 로 대면 모르는 프로젝트다 — 로컬 창이 원격 프로젝트로 가는 길이 없다.
    /// 이 머신의 Host 끼리는 다시 묶으면 옮겨진다(전과 같은 계약).
    #[test]
    fn a_bare_project_id_never_reaches_a_remote_host() {
        let router = router_with_local();
        router
            .add_host(Arc::new(Remote(LocalHost::new("mini"))))
            .unwrap();
        router
            .add_host(Arc::new(LocalHost::new("local-2")))
            .unwrap();
        router.bind_project("p7".into(), "mini").unwrap();
        assert_eq!(
            router.route("p7").err(),
            Some(HostError::UnknownProject("p7".into()))
        );

        router.bind_project("p1".into(), "local").unwrap();
        router.bind_project("p1".into(), "local-2").unwrap();
        assert_eq!(router.route("p1").unwrap().id(), "local-2");
        assert_eq!(
            router
                .resolve(&Target::Member {
                    host: "local".into(),
                    project: "p1".into()
                })
                .err(),
            Some(HostError::UnknownProject("p1".into()))
        );

        // Host 를 빼면 그 묶음도 풀린다.
        assert!(router.remove_host("mini").is_some());
        router
            .add_host(Arc::new(Remote(LocalHost::new("mini"))))
            .unwrap();
        assert!(
            router
                .resolve(&Target::Member {
                    host: "mini".into(),
                    project: "p7".into()
                })
                .is_err()
        );
    }

    /// 원격 프로젝트로 고른 연결도 그 프로젝트를 범위로 싣는다.
    #[tokio::test]
    async fn connecting_to_a_member_carries_its_project_as_the_scope() {
        let router = HostRouter::new();
        let host = Arc::new(Recording {
            inner: LocalHost::new("mini"),
            seen: std::sync::Mutex::default(),
        });
        router.add_host(host.clone()).unwrap();
        router.bind_project("p1".into(), "mini").unwrap();
        let target = Target::Member {
            host: "mini".into(),
            project: "p1".into(),
        };
        assert!(
            router
                .connect(&target, Caller::new(Role::Owner))
                .await
                .is_ok()
        );
        assert_eq!(
            host.seen.lock().unwrap()[0].scope,
            ConnectionScope::Project("p1".into())
        );
    }
}
