//! SSH 터널을 연결 관리자에 붙이는 자리(14-m3-plan.md M3-11, 결정 M3-D11): [`DbTunnels`] 의 구현 [`SshTunnels`].
//!
//! 연결 관리자(`crate::connection`)는 관문 — 연결 범위([`crate::gate::DbGrant`]) · 신뢰 게이트 · 연결 설정 풀기(터널 목적지 ·
//! 인증서 파일의 확인 포함, [`crate::trust`]) — 을 지난 뒤에만 [`DbTunnels::open`] 을 부른다. 이 모듈은 그 뒤의 일만 한다:
//!
//! - ssh 실행 파일은 **그 프로젝트의 런타임 환경의 PATH** 에서 찾고(`pw_runtime::program::find_program`), ssh 의 환경도 그
//!   런타임 환경 전체다(`SSH_AUTH_SOCK` 등 — 사람의 에이전트를 쓴다). 앱 프로세스의 환경은 물려주지 않는다.
//! - 비밀번호 · 패스프레이즈 · 모르는 호스트 키는 M2-06 의 askpass(데스크톱이 [`TunnelAskpass`] 로 붙인다) — 질문은 그 터널을 연
//!   프로젝트의 창으로 간다. 붙이지 않았으면 ssh 는 묻지 못하고 실패한다.
//! - 터널의 수명 · argv · 준비 확인은 `pw_db_driver::tunnel`. 돌려주는 [`DbTunnelLease`] 를 연결이 쥐는 동안 터널이 산다.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use pw_db_driver::BoxFuture;
use pw_db_driver::tunnel::{AskpassGuard, Launch, TunnelKey, Tunnels};
use pw_db_types::{ConnectionSpec, DbError, DbErrorKind, Target};
use pw_rpc::RpcError;

use crate::config::validate_host_ref;
use crate::secret::RuntimeEnv;
use crate::session::{DbTunnelLease, DbTunnels, db_error};

/// ssh 의 환경: 그 프로젝트의 런타임 환경 전체(PATH 포함). 블로킹이다(로그인 셸을 기다릴 수 있다).
pub trait TunnelEnv: Send + Sync {
    fn env(&self, project_id: &str, root: &Path) -> Vec<(OsString, OsString)>;
}

impl TunnelEnv for RuntimeEnv {
    fn env(&self, project_id: &str, root: &Path) -> Vec<(OsString, OsString)> {
        let resolved = self.0.env(project_id, root);
        let mut env: Vec<(OsString, OsString)> = resolved
            .env
            .vars
            .iter()
            .filter(|(name, _)| name.as_os_str() != "PATH")
            .map(|(n, v)| (n.clone(), v.clone()))
            .collect();
        env.push(("PATH".into(), resolved.env.path.clone()));
        env
    }
}

/// ssh 에 줄 askpass(데스크톱이 `pw_git` 의 askpass 로 붙인다). 질문은 `project_id` 를 듣는 창으로 간다.
pub trait TunnelAskpass: Send + Sync {
    /// ssh 한 번의 askpass. 쓸 수 없으면(소켓을 열지 못함 · Windows) `None`.
    fn session(&self, project_id: &str) -> Option<AskpassGuard>;
}

/// 시스템 ssh 로 여는 터널.
pub struct SshTunnels {
    tunnels: Tunnels,
    env: Arc<dyn TunnelEnv>,
    askpass: OnceLock<Arc<dyn TunnelAskpass>>,
}

impl SshTunnels {
    pub fn new(env: Arc<dyn TunnelEnv>) -> Self {
        Self::with_tunnels(env, Tunnels::new())
    }

    /// 터널 표를 바꾼다(시험 — 준비 제한 시간 등).
    pub fn with_tunnels(env: Arc<dyn TunnelEnv>, tunnels: Tunnels) -> Self {
        Self {
            tunnels,
            env,
            askpass: OnceLock::new(),
        }
    }

    /// askpass 를 붙인다. 두 번째부터는 무시한다.
    pub fn attach_askpass(&self, askpass: Arc<dyn TunnelAskpass>) {
        let _ = self.askpass.set(askpass);
    }

    pub fn tunnels(&self) -> &Tunnels {
        &self.tunnels
    }

    async fn launch(&self, project_id: &str, root: &Path) -> Result<Launch, DbError> {
        let (env_source, project, root) = (
            Arc::clone(&self.env),
            project_id.to_owned(),
            root.to_path_buf(),
        );
        let env = tokio::task::spawn_blocking(move || env_source.env(&project, &root))
            .await
            .map_err(|e| DbError::new(DbErrorKind::Connect, e.to_string()))?;
        let path = env
            .iter()
            .rev()
            .find(|(name, _)| name.as_os_str() == "PATH")
            .map(|(_, v)| v.clone())
            .unwrap_or_default();
        let program: PathBuf = pw_runtime::program::find_program(ssh_name(), &path).ok_or_else(|| {
            DbError::new(
                DbErrorKind::Connect,
                "`ssh` is not on the project's PATH — install the OpenSSH client to use ssh tunnels",
            )
        })?;
        let askpass = self.askpass.get().and_then(|a| a.session(project_id));
        Ok(Launch {
            program,
            env,
            askpass,
        })
    }
}

fn ssh_name() -> &'static str {
    "ssh"
}

impl DbTunnels for SshTunnels {
    fn open<'a>(
        &'a self,
        project_id: &'a str,
        root: &'a Path,
        host_ref: &'a str,
        spec: &'a mut ConnectionSpec,
    ) -> BoxFuture<'a, Result<DbTunnelLease, RpcError>> {
        Box::pin(async move {
            // 설정 단계에서 걸렀지만 argv 앞에서 한 번 더(겹방어).
            validate_host_ref(host_ref).map_err(RpcError::invalid_params)?;
            let Target::Network {
                host,
                port,
                connect_to,
            } = &mut spec.target
            else {
                return Err(RpcError::invalid_params(
                    "only network connections go through an ssh tunnel",
                ));
            };
            let key = TunnelKey {
                project: project_id.to_owned(),
                destination: host_ref.to_owned(),
                db_host: host.clone(),
                db_port: *port,
            };
            let lease = self
                .tunnels
                .open(key, || self.launch(project_id, root))
                .await
                .map_err(db_error)?;
            *connect_to = Some(lease.local());
            let lost = lease.lost();
            Ok(DbTunnelLease::new(lease, lost))
        })
    }
}
