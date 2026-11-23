//! 원격 호스트의 SSH 백엔드를 앱에 붙인다(18-m7-plan M7-08). 관리자(`pw_remote::RemoteHosts`)는 Host 조립이 만들고, 앱은 주
//! 인스턴스가 된 뒤(setup) 이것을 부른다:
//!
//! - **기기 키**: OS 키체인의 `KeychainKeyStore::for_instance(<단일 인스턴스 키>)` — 다른 `$DATA` · 다른 워크트리의 앱과 키를
//!   나누지 않는다(M7-03 이 남긴 자리). 처음 붙을 때 읽는다.
//! - **판**: `pw_remote::ssh::versions()` — 워크스페이스 판과 빌드에 박은 릴리스 채널(`PITWALL_RELEASE_CHANNEL`, release.yml 의
//!   build 잡). Relay 와 같은 값이라 원격 파일 이름 · 핸드셰이크가 맞는다.
//! - **질문**: ssh 의 호스트 키 확인 · 암호는 git 과 같은 askpass 소켓(D43)으로 `remote.prompts` 를 듣는 창에 간다.
//! - **받기**: `$CACHE/relay-dist/` 와 GitHub 릴리스(`HttpsFetch` — 서명 매니페스트로 검증, `pw_link::install`).

use std::sync::Arc;

use pw_core::config::AppPaths;
use pw_link::KeychainKeyStore;
use pw_link::install::HttpsFetch;
use pw_remote::RemoteHosts;
use pw_remote::ssh::{RemoteAskpass, SshBackend, SshParts};

/// 페어링 표에 남기는 이 기기의 이름(`[A-Za-z0-9._-]` — 원격의 `pitwall-relay pair` 가 받는 글자).
pub const DEVICE_NAME: &str = "pitwall-desktop";

/// 관리자에 SSH 백엔드를 붙인다. 받기 클라이언트를 만들지 못하면 경고만 — 원격 호스트를 쓸 수 없을 뿐 앱은 뜬다.
pub fn attach(
    remote: &RemoteHosts,
    askpass: &pw_git::write::Askpass,
    paths: &AppPaths,
    instance_key: &str,
) {
    // 원격 Relay 가 청하는 DB 비밀은 이 Desktop 의 키체인(로컬과 같은 서비스, 계정에 머신 id — 18-m7-plan M7-12 ②)에서 답한다.
    remote.attach_secrets(Arc::new(pw_db::secret::KeyringStore::new()));
    let fetch = match HttpsFetch::new() {
        Ok(fetch) => fetch,
        Err(e) => {
            tracing::warn!(%e, "remote hosts disabled: cannot make the relay download client");
            return;
        }
    };
    remote.attach_backend(Arc::new(backend(
        remote,
        askpass,
        paths,
        instance_key,
        fetch,
    )));
}

fn backend(
    remote: &RemoteHosts,
    askpass: &pw_git::write::Askpass,
    paths: &AppPaths,
    instance_key: &str,
    fetch: HttpsFetch,
) -> SshBackend {
    SshBackend::new(SshParts {
        keys: Arc::new(KeychainKeyStore::for_instance(instance_key)),
        dist: paths.cache.join("relay-dist"),
        fetch: Arc::new(fetch),
        askpass: Some(Arc::new(RemoteAskpass::new(
            askpass.clone(),
            remote.prompts().clone(),
        ))),
        device_name: DEVICE_NAME.to_owned(),
        home: std::env::home_dir(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 앱의 Host 는 원격 호스트 관리자를 갖고, SSH 백엔드를 붙이면 목록을 읽을 수 있다(「쓸 수 없음」 경고가 사라진다). 키체인은
    /// 붙이기만 해서는 읽지 않는다 — 이 시험이 사람의 키체인을 건드리지 않는다.
    #[test]
    fn the_app_host_lists_remote_hosts_once_the_ssh_backend_is_attached() {
        let dir = tempfile::tempdir().unwrap();
        let (_host, _db, remote) = crate::local_host_parts(
            AppPaths::under(dir.path()),
            Arc::new(pw_diag::Diagnostics::new(dir.path().join("logs"), "0.0.0")),
            Arc::new(pw_ext::Registry::default()),
            Arc::new(pw_i18n::I18n::new(pw_i18n::Catalog::default(), Vec::new())),
            Arc::new(pw_fs::Fs::new()),
            pw_git::write::Askpass::default(),
            pw_core::open::OpenRequests::default(),
            pw_mcp::sessions::Sessions::default(),
        );
        let remote = remote.expect("the desktop host manages remote hosts");
        assert_eq!(remote.list().warnings.len(), 1);
        let paths = AppPaths::under(dir.path());
        remote.attach_backend(Arc::new(backend(
            &remote,
            &pw_git::write::Askpass::default(),
            &paths,
            "pitwall-test-instance",
            HttpsFetch::new().unwrap(),
        )));
        assert!(
            remote.list().warnings.is_empty(),
            "{:?}",
            remote.list().warnings
        );
        assert_eq!(
            pw_remote::ssh::versions().release_channel,
            pw_remote::ssh::RELEASE_CHANNEL
        );
    }
}
