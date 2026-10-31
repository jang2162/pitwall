//! DB 드라이버 등록부를 채우는 자리(14-m3-plan.md M3-01 「공유 부품의 주인」). 코어(`pw-db`)는 드라이버 크레이트를 모르므로
//! 드라이버는 여기서만 넣는다 — 드라이버 행(M3-06 ~ M3-09)이 자기 줄을 하나씩 더하고, 아래 시험의 id 목록에도 더한다:
//!
//! ```text
//! registry.register(Arc::new(pw_db_driver_pg::PgDriver::new())).expect("db driver ids are unique");
//! ```

use std::path::PathBuf;
use std::sync::Arc;

use pw_db::config::Connections;
use pw_db::secret::{KeyringStore, RuntimeEnv};
use pw_db::tunnel::{SshTunnels, TunnelAskpass};
use pw_db::{DbSessions, DriverRegistry};
use pw_db_driver::tunnel::{AskpassGuard, Tunnels};
use pw_runtime::Runtime;

/// 연결 설정 · 비밀 · 신뢰 게이트(M3-02). 비밀은 OS 키체인(서비스 `dev.pitwall.db`), `env` 참조는 그 프로젝트의 런타임 환경에서
/// 푼다. Host 를 만든 뒤 레지스트리를 붙인다(`Connections::attach`).
pub fn connections(runtime: &Arc<Runtime>) -> Arc<Connections> {
    Arc::new(Connections::new(
        Arc::new(KeyringStore::new()),
        Arc::new(RuntimeEnv(Arc::clone(runtime))),
    ))
}

/// 세션 · 실행 파이프라인(M3-05): 연결 설정(M3-02)과 드라이버 등록부를 받는다.
pub fn sessions(connections: &Arc<Connections>) -> Arc<DbSessions> {
    Arc::new(DbSessions::new(Arc::clone(connections), drivers()))
}

/// SSH 터널(M3-11): 시스템 ssh 를 그 프로젝트의 런타임 환경으로 띄우고, 비밀번호 · 모르는 호스트 키는 git 과 같은 askpass(M2-06)로
/// 그 프로젝트 창에 묻는다. 앱이 끝날 때 남은 ssh 는 `pw_db_driver::tunnel::shutdown` 이 끝낸다(main.rs 의 `RunEvent::Exit`).
/// 앱이 강제로 끝나면 그것이 돌지 않는다 — 띄운 ssh 를 `records`(상태 디렉터리 아래)에 적고, 앞 실행이 남긴 것은 여기서 끈다(배경
/// 스레드 — 지금 실행이 띄우는 ssh 와 겹쳐도 된다, `pw_db_driver::tunnel::reap_orphans`).
pub fn attach_tunnels(
    sessions: &DbSessions,
    runtime: &Arc<Runtime>,
    askpass: &pw_git::write::Askpass,
    prompts: &pw_git::write::Prompts,
    records: PathBuf,
) {
    let previous = records.clone();
    let _ = std::thread::Builder::new()
        .name("db-ssh-reap".into())
        .spawn(move || {
            let ended = pw_db_driver::tunnel::reap_orphans(&previous);
            if ended > 0 {
                tracing::info!(target: "pw_db", ended, "ended ssh tunnels left by a previous run");
            }
        });
    let tunnels = SshTunnels::with_tunnels(
        Arc::new(RuntimeEnv(Arc::clone(runtime))),
        Tunnels::recording(records),
    );
    tunnels.attach_askpass(Arc::new(GitAskpass {
        askpass: askpass.clone(),
        prompts: prompts.clone(),
    }));
    sessions.attach_tunnels(Arc::new(tunnels));
}

/// git 의 askpass 세션을 ssh 에 준다. 질문은 그 터널을 연 프로젝트의 창으로 간다.
struct GitAskpass {
    askpass: pw_git::write::Askpass,
    prompts: pw_git::write::Prompts,
}

impl TunnelAskpass for GitAskpass {
    fn session(&self, project_id: &str) -> Option<AskpassGuard> {
        let session = self.askpass.session(
            project_id,
            &self.prompts,
            pw_git::write::prompt::ASKPASS_TIMEOUT,
        )?;
        let mut env = Vec::new();
        session.apply(&mut env);
        let session = Arc::new(session);
        let asking = Arc::clone(&session);
        Some(AskpassGuard::new(env, move || asking.asking(), session))
    }
}

/// 앱의 드라이버 등록부. 실행 파이프라인(M3-05)이 Host 를 만들 때 받는다.
pub fn drivers() -> DriverRegistry {
    let mut registry = DriverRegistry::new();
    registry
        .register(Arc::new(pw_db_driver_mysql::MysqlDriver::new()))
        .expect("db driver ids are unique");
    registry
        .register(Arc::new(pw_db_driver_pg::PgDriver::new()))
        .expect("db driver ids are unique");
    registry
        .register(Arc::new(pw_db_driver_redis::RedisDriver::new()))
        .expect("db driver ids are unique");
    registry
        .register(Arc::new(pw_db_driver_sqlite::SqliteDriver::new()))
        .expect("db driver ids are unique");
    registry
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_registry_holds_exactly_the_built_in_drivers() {
        // 등록은 갈래 검사 · 중복 검사를 지나야 한다(`DriverRegistry::register`). 드라이버 행이 자기 id 를 더한다.
        let ids: Vec<String> = super::drivers().infos().into_iter().map(|i| i.id).collect();
        assert_eq!(ids, ["mysql", "postgres", "redis", "sqlite"]);
    }
}
