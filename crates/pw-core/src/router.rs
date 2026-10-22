//! Host Router: project_id → Host. UI · MCP 가 어느 Host 에 연결할지 정한다(`docs/design/03-architecture.md` 「전체 구조」 절).
//!
//! 라우팅은 **연결 단위**다. 연결 하나는 한 Host 의 pw-rpc 세션 하나이고, 그 위의 스트림 id · 구독 id ·
//! 요청 id 는 그 세션 안에서만 유일하다. 요청마다 Host 를 고르면 `stream.credit` 처럼 projectId 가 없는 제어 메시지를
//! 어느 Host 로 보낼지와 Host 사이의 id 충돌을 transport 가 따로 풀어야 한다.
//! 계약(재바인딩하면 기존 연결을 닫고 새로 연다 등)은 `docs/design/03-architecture.md` 「RPC 설계」 절이 정본이다.
//! `bind_project` 는 이미 열린 연결을 옮기지 않는다.

use std::collections::HashMap;
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
    projects: HashMap<ProjectId, HostId>,
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

    /// 프로젝트를 Host 에 묶는다. 이미 묶여 있으면 새 Host 로 바꾼다.
    pub fn bind_project(&self, project: ProjectId, host: &str) -> Result<(), HostError> {
        let mut state = self.state.write().unwrap_or_else(PoisonError::into_inner);
        if !state.hosts.contains_key(host) {
            return Err(HostError::UnknownHost(host.to_owned()));
        }
        state.projects.insert(project, host.to_owned());
        Ok(())
    }

    pub fn unbind_project(&self, project: &str) -> bool {
        let mut state = self.state.write().unwrap_or_else(PoisonError::into_inner);
        state.projects.remove(project).is_some()
    }

    pub fn host(&self, id: &str) -> Result<Arc<dyn Host>, HostError> {
        let state = self.state.read().unwrap_or_else(PoisonError::into_inner);
        state
            .hosts
            .get(id)
            .cloned()
            .ok_or_else(|| HostError::UnknownHost(id.to_owned()))
    }

    /// 프로젝트의 Host. 모르는 프로젝트는 [`HostError::UnknownProject`].
    pub fn route(&self, project: &str) -> Result<Arc<dyn Host>, HostError> {
        let host_id = {
            let state = self.state.read().unwrap_or_else(PoisonError::into_inner);
            state
                .projects
                .get(project)
                .cloned()
                .ok_or_else(|| HostError::UnknownProject(project.to_owned()))?
        };
        self.host(&host_id)
    }

    pub fn resolve(&self, target: &Target) -> Result<Arc<dyn Host>, HostError> {
        match target {
            Target::HostId(id) => self.host(id),
            Target::ProjectId(id) => self.route(id),
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
        if let (Target::ProjectId(id), ConnectionScope::Host) = (target, &caller.scope) {
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
}
