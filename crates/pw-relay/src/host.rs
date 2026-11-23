//! Relay 의 Host: Desktop 과 같은 조립(`pw_host::assemble_relay` — 묶음 `bundle.*` 만 뺀다)에 Relay 부품을 넣고, 레지스트리에
//! `roots` 판정을 붙인다(18-m7-plan M7-04 ① · ③).

use std::sync::Arc;

use pw_core::config::AppPaths;
use pw_db::desktop_secret::DesktopSecrets;
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
    assemble_serving(
        paths,
        config,
        tokio,
        askpass,
        open_requests,
        pw_mcp::sessions::Sessions::default(),
    )
}

/// [`assemble_with`] 에 MCP 세션 등록부를 넣은 것 — 데몬은 같은 등록부로 MCP 소켓의 세션을 연다([`crate::mcp::services`]). 링크로
/// 붙은 Desktop 의 `mcp.approvals` · `mcp.approval.answer` 가 그 세션들의 승인 허브에 닿는다(18-m7-plan M7-20).
pub fn assemble_serving(
    paths: AppPaths,
    config: &RelayConfig,
    tokio: tokio::runtime::Handle,
    askpass: pw_git::write::Askpass,
    open_requests: pw_core::open::OpenRequests,
    mcp_sessions: pw_mcp::sessions::Sessions,
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
        mcp_sessions,
        // 비밀을 디스크 · 메모리에 두지 않는다(메인 판정 M7-P5) — 링크로 붙은 Desktop 의 키체인에 묻는다(18-m7-plan M7-12 ②).
        secrets: Arc::new(DesktopSecrets::new()),
        db_drivers: pw_host::db::drivers(),
        cli: CliParts {
            target: Err("the relay installs no shell command".to_owned()),
            app: std::env::current_exe().map_err(|e| e.to_string()),
            source: pw_open_with::cli::CliSource::Ok,
        },
        // Relay 에는 창이 없다. 링크의 창 label 은 `<기기 id>/<label>` 이다(M7-03) — 붙은 Desktop 의 설정 창(그 앱의
        // `SETTINGS_LABEL`)이면 설정 창으로 본다(18-m7-plan M7-11). 접두 없는 label 은 이 머신의 창이 아니다.
        settings_window: Arc::new(is_desktop_settings_window),
        tokio,
    });
    if let Some(registry) = assembled.host.registry() {
        registry.set_root_policy(Roots::new(&config.roots).policy());
        warn_hidden(registry);
    }
    assembled
}

/// 링크로 온 창 label 이 붙은 Desktop 의 설정 창인가(`apps/desktop/src-tauri/src/windows.rs` 의 `SETTINGS_LABEL`). 도구 설치의
/// 허가(`pw_tools::install::tool_grant`)가 본다 — role(owner)은 링크가 따로 자른다.
fn is_desktop_settings_window(label: &str) -> bool {
    pw_link::link_window(label).is_some_and(|(_, window)| window == "settings")
}

/// `roots` 를 좁혀 밖에 남은 프로젝트는 레지스트리가 숨긴다(`Registry::set_root_policy` — M7-04 리뷰 권고 5). 시작할 때 그 수와
/// 루트를 경고로 남긴다 — 사람이 `relay.toml` 을 고친 뒤 왜 프로젝트가 안 보이는지 로그에서 찾게.
fn warn_hidden(registry: &pw_core::registry::Registry) {
    match registry.hidden_by_root_policy() {
        Ok(hidden) if !hidden.is_empty() => {
            let roots: Vec<&str> = hidden
                .iter()
                .flat_map(|p| p.roots.iter().map(String::as_str))
                .collect();
            tracing::warn!(target: "pw_relay", count = hidden.len(), ?roots,
                "projects registered outside `roots` in relay.toml are hidden until `roots` allows them again");
        }
        Ok(_) => {}
        Err(e) => {
            tracing::warn!(target: "pw_relay", %e, "cannot check the registry against `roots`")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 18-m7-plan M7-11(M7-03 리뷰 권고 6): 링크의 창 label 은 기기 id 가 붙어 온다. 붙은 Desktop 의 설정 창은 도구 설치의 설정 창이고,
    /// 접두 없는 label · 다른 창은 아니다.
    #[test]
    fn the_settings_window_of_a_linked_desktop_is_read_through_its_prefix() {
        assert!(is_desktop_settings_window("SHA256:ab/c+d/settings"));
        for refused in [
            "settings",
            "SHA256:ab/c+d/welcome",
            "SHA256:ab/c+d/project-1",
        ] {
            assert!(!is_desktop_settings_window(refused), "{refused}");
        }
    }
}
