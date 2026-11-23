//! Relay 의 Host: Desktop 과 같은 조립(`pw_host::assemble_relay` — 묶음 `bundle.*` 만 뺀다)에 Relay 부품을 넣고, 레지스트리에
//! `roots` 판정을 붙인다(18-m7-plan M7-04 ① · ③).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};

use pw_core::config::AppPaths;
use pw_db::secret::{SecretStore, StoreError};
use pw_db_types::Secret;
use pw_host::{Assembled, CliParts, HostParts};

use crate::config::RelayConfig;
use crate::roots::Roots;

/// Relay 의 Host id. 레지스트리의 프로젝트는 그 머신에서 「이 머신의 것」(`local`)이다 — Desktop 이 고르는 원격 Host id 와 별개다.
pub const HOST_ID: &str = "local";

/// 조립한다. `paths` 는 Relay 의 앱 경로([`crate::paths::relay_paths`]).
pub fn assemble(paths: AppPaths, config: &RelayConfig, tokio: tokio::runtime::Handle) -> Assembled {
    assemble_with(
        paths,
        config,
        tokio,
        pw_git::write::Askpass::default(),
        pw_core::open::OpenRequests::default(),
    )
}

/// [`assemble`] 에 askpass · 열기 요청 자리를 넣은 것 — 데몬은 같은 것을 좁은 명령 몸통(`crate::narrow::RelayNarrow`)에도 준다.
/// 열기 요청은 붙은 Desktop 이 연다(`pw_core::open::desks` — 여는 쪽으로 붙인다, M7-15).
pub fn assemble_with(
    paths: AppPaths,
    config: &RelayConfig,
    tokio: tokio::runtime::Handle,
    askpass: pw_git::write::Askpass,
    open_requests: pw_core::open::OpenRequests,
) -> Assembled {
    open_requests.attach(Arc::new(open_requests.desks()));
    let engine = semver::Version::parse(crate::VERSION).unwrap_or(semver::Version::new(0, 0, 0));
    let extensions = Arc::new(pw_ext::Registry::load(&pw_ext::LoadOptions::with_data_dir(
        engine,
        &paths.data,
    )));
    let i18n = Arc::new(pw_i18n::I18n::new(
        pw_i18n::Catalog::from_registry(&extensions),
        pw_i18n::I18n::system_locales(),
    ));
    let assembled = pw_host::assemble_relay(HostParts {
        id: HOST_ID.into(),
        diagnostics: Arc::new(pw_diag::Diagnostics::new(
            crate::paths::logs_dir(&paths),
            crate::VERSION,
        )),
        paths,
        extensions,
        i18n,
        fs: Arc::new(pw_fs::Fs::new()),
        // 원격 git 의 암호 질문은 붙은 Desktop 의 그 프로젝트 창이 받는다(M7-15 — 데몬 소켓의 `askpass`, `Askpass::serve_through`).
        askpass,
        // 열기 요청은 붙은 Desktop 의 창이 연다(M7-15).
        open_requests,
        mcp_sessions: pw_mcp::sessions::Sessions::default(),
        // 비밀을 디스크에 두지 않는다(메인 판정 M7-P5). 붙은 Desktop 에 묻는 구현은 M7-12 가 넣는다.
        secrets: Arc::new(MemoryOnlySecrets::default()),
        db_drivers: pw_host::db::drivers(),
        cli: CliParts {
            target: Err("the relay installs no shell command".to_owned()),
            app: std::env::current_exe().map_err(|e| e.to_string()),
            source: pw_open_with::cli::CliSource::Ok,
        },
        // Relay 에는 창이 없다. 링크의 창 label 은 `<기기 id>/<label>` 이라(M7-03) 설정 창으로 읽지 않는다 — 접두를 아는 해석은
        // M7-11 이 맞춘다.
        settings_window: Arc::new(|_: &str| false),
        tokio,
    });
    if let Some(registry) = assembled.host.registry() {
        registry.set_root_policy(Roots::new(&config.roots).policy());
    }
    assembled
}

/// 데몬이 사는 동안만 쥐는 비밀(디스크 · 키체인에 쓰지 않는다).
#[derive(Default)]
pub struct MemoryOnlySecrets(Mutex<BTreeMap<String, Secret>>);

impl SecretStore for MemoryOnlySecrets {
    fn get(&self, account: &str) -> Result<Option<Secret>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(account)
            .map(|s| Secret::new(s.expose())))
    }

    fn set(&self, account: &str, value: &Secret) -> Result<(), StoreError> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(account.to_owned(), Secret::new(value.expose()));
        Ok(())
    }

    fn delete(&self, account: &str) -> Result<(), StoreError> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(account);
        Ok(())
    }
}
