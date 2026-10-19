//! 자동 업데이트(docs/design/02-tech-stack.md §3-1 · D21, docs/design/04-features.md F11-11). Tauri `plugin-updater` 가 받은
//! 패키지의 서명을 앱에 든 공개 키(`tauri.conf.json` 의 `plugins.updater.pubkey`)로 검증하고 설치한다. 서명에 실린 버전이
//! 매니페스트의 버전과 다르면 거절한다(`requireSignedVersion`) — 옛 릴리스의 서명된 패키지에 높은 버전을 붙인 매니페스트로
//! 되돌리지 못하게.
//!
//! 채널은 전역 설정 `[update] channel`(`"stable"` 내장 기본값 | `"preview"`)이다. 매니페스트(`latest.json`)는 GitHub 릴리스마다
//! 그 릴리스의 것 하나만 있다(`.github/workflows/release.yml`) — preview 릴리스가 stable 매니페스트를 덮어쓸 자리가 없다.
//! - stable: [`STABLE_MANIFEST`]. GitHub 는 `releases/latest/download/…` 를 prerelease · draft 가 아닌 가장 최근 릴리스로 보낸다.
//! - preview: [`RELEASES_API`] 의 릴리스 목록에서 `latest.json` 을 가진 가장 높은 버전(stable 포함)의 그 파일([`newest_manifest`]).
//!   GitHub API 는 인증 없이 IP 당 시간당 60회다 — 넘으면 그 확인만 실패한다.
//!
//! 언제 확인하나: 시작할 때 조용히(새 버전이 있을 때만 묻는다) + 메뉴 「업데이트 확인…」. 시작 확인은 release 빌드가 기본
//! `$DATA` 로 떴을 때만 한다([`auto_check`]) — `mise run dev`(debug) · `mise run perf`(회마다 임시 `$DATA`) · 시험은 네트워크에
//! 나가지 않고 대화상자도 띄우지 않는다. debug 빌드는 메뉴로도 확인하지 않는다 — 설치가 개발 바이너리를 바꿔 끼우지 않게.
//!
//! 매니페스트 주소는 빌드할 때 `PITWALL_UPDATE_STABLE_MANIFEST` · `PITWALL_UPDATE_RELEASES_API` 로 바꿀 수 있다(로컬 업데이트
//! 왕복 시험, CONTRIBUTING.md 「릴리스」). 실행 중 환경 변수로는 바꾸지 않는다.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use pw_core::config::ConfigStore;
use pw_i18n::{FluentArgs, I18n};
use semver::Version;
use serde::Deserialize;
use tauri::{AppHandle, Manager, Runtime, Url};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
use tauri_plugin_updater::UpdaterExt;

use crate::windows;

/// 채널을 고르는 전역 설정 키. 내장 기본값은 `"stable"`(`crates/pw-core/src/config/source.rs` 의 `BUILTIN_SETTINGS`).
pub const CHANNEL_KEY: &str = "update.channel";

/// 릴리스마다 올리는 updater 매니페스트의 파일 이름.
pub const MANIFEST_ASSET: &str = "latest.json";

pub const STABLE_MANIFEST: &str = match option_env!("PITWALL_UPDATE_STABLE_MANIFEST") {
    Some(url) => url,
    None => "https://github.com/jang2162/pitwall/releases/latest/download/latest.json",
};

pub const RELEASES_API: &str = match option_env!("PITWALL_UPDATE_RELEASES_API") {
    Some(url) => url,
    None => "https://api.github.com/repos/jang2162/pitwall/releases?per_page=30",
};

/// 채널을 읽을 전역 설정. 앱 상태로 등록한다(main.rs). 설정 계층이 없으면(`None`) stable 이다.
pub struct Settings(pub Option<ConfigStore>);

/// 시작 확인을 미루는 시간. 첫 화면(콜드 스타트 목표 1.5초)과 다투지 않게.
const STARTUP_DELAY: Duration = Duration::from_secs(5);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    Stable,
    Preview,
}

/// 설정의 채널. 키가 없거나 `"stable"` · `"preview"` 가 아니면 경고를 남기고 stable 이다 — 고른 적 없는 prerelease 를
/// 받지 않게. 설정 파일이 깨져 이전 값으로 버티는 중이면 그 값을 쓴다(사람이 고른 마지막 값이다).
pub fn channel(global: &ConfigStore) -> Channel {
    parse_channel(global.snapshot().get("settings", CHANNEL_KEY))
}

fn parse_channel(value: Option<&serde_json::Value>) -> Channel {
    match value.and_then(serde_json::Value::as_str) {
        Some("stable") => Channel::Stable,
        Some("preview") => Channel::Preview,
        _ => {
            tracing::warn!(value = ?value, "{CHANNEL_KEY} is not \"stable\" or \"preview\"; using stable");
            Channel::Stable
        }
    }
}

/// 시작할 때 확인하는가: release 빌드이고 기본 `$DATA` 로 떴을 때만. `custom_data` 는 기본이 아닌 `$DATA`
/// (`PITWALL_DATA_DIR`, main.rs 의 `custom_data`)로 떴는지 — 측정(`mise run perf`) · 시험 · 확인용 실행이 그렇다.
pub fn auto_check(release: bool, custom_data: bool) -> bool {
    release && !custom_data
}

/// GitHub REST API 의 릴리스 목록 항목에서 쓰는 필드.
#[derive(Debug, Deserialize)]
pub struct GithubRelease {
    pub tag_name: String,
    #[serde(default)]
    pub draft: bool,
    #[serde(default)]
    pub assets: Vec<GithubAsset>,
}

#[derive(Debug, Deserialize)]
pub struct GithubAsset {
    pub name: String,
    pub browser_download_url: String,
}

/// preview 채널의 매니페스트: `latest.json` 을 가진 릴리스 중 태그(`v<semver>`)의 버전이 가장 높은 것. stable 이 더 높으면
/// stable 이다. draft, 태그가 semver 가 아닌 것, 매니페스트가 없는 것(올리는 중인 릴리스)은 건너뛴다.
pub fn newest_manifest(releases: &[GithubRelease]) -> Option<(Version, &str)> {
    releases
        .iter()
        .filter(|r| !r.draft)
        .filter_map(|r| {
            let version = Version::parse(r.tag_name.strip_prefix('v')?).ok()?;
            let asset = r.assets.iter().find(|a| a.name == MANIFEST_ASSET)?;
            Some((version, asset.browser_download_url.as_str()))
        })
        .max_by(|a, b| a.0.cmp(&b.0))
}

#[derive(Debug)]
enum CheckError {
    /// preview 매니페스트를 찾는 GitHub API 요청이 실패했다.
    Releases(String),
    Updater(tauri_plugin_updater::Error),
}

impl std::fmt::Display for CheckError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CheckError::Releases(e) => write!(f, "cannot list releases: {e}"),
            CheckError::Updater(e) => e.fmt(f),
        }
    }
}

impl From<tauri_plugin_updater::Error> for CheckError {
    fn from(e: tauri_plugin_updater::Error) -> Self {
        CheckError::Updater(e)
    }
}

/// 채널의 매니페스트 주소. preview 에 릴리스가 하나도 없으면 `None`(업데이트 없음).
async fn manifest_url(channel: Channel) -> Result<Option<Url>, CheckError> {
    let releases = match channel {
        Channel::Stable => {
            return STABLE_MANIFEST
                .parse()
                .map(Some)
                .map_err(|e| CheckError::Releases(format!("{STABLE_MANIFEST}: {e}")));
        }
        Channel::Preview => fetch_releases().await.map_err(CheckError::Releases)?,
    };
    let Some((version, url)) = newest_manifest(&releases) else {
        tracing::info!(api = RELEASES_API, "no release with an update manifest");
        return Ok(None);
    };
    tracing::info!(%version, url, "newest release for the preview channel");
    url.parse()
        .map(Some)
        .map_err(|e| CheckError::Releases(format!("{url}: {e}")))
}

async fn fetch_releases() -> Result<Vec<GithubRelease>, String> {
    // updater 와 같은 암호 제공자(ring). 이미 걸려 있으면 그대로 둔다.
    if rustls::crypto::CryptoProvider::get_default().is_none() {
        let _ = rustls::crypto::ring::default_provider().install_default();
    }
    let client = reqwest::Client::builder()
        .user_agent(concat!("pitwall/", env!("CARGO_PKG_VERSION")))
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|e| e.to_string())?;
    let response = client
        .get(RELEASES_API)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("{RELEASES_API} answered {status}"));
    }
    let body = response.bytes().await.map_err(|e| e.to_string())?;
    serde_json::from_slice(&body).map_err(|e| format!("{RELEASES_API}: {e}"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Trigger {
    /// 시작할 때. 새 버전이 있을 때만 대화상자를 띄운다.
    Startup,
    /// 메뉴 「업데이트 확인…」. 결과를 늘 알린다.
    Menu,
}

/// 확인은 한 번에 하나다 — 시작 확인 중에 메뉴를 고르거나 메뉴를 두 번 골라도 대화상자가 겹치지 않게.
static CHECKING: AtomicBool = AtomicBool::new(false);

/// 시작 확인(setup 에서 부른다). [`auto_check`] 가 거짓이면 아무것도 하지 않는다.
pub fn on_startup<R: Runtime>(app: &AppHandle<R>, enabled: bool) {
    let version = app.package_info().version.to_string();
    if !enabled {
        tracing::info!(%version, "update check on startup is off for this run");
        return;
    }
    tracing::info!(%version, "update check on startup is on");
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(STARTUP_DELAY).await;
        check(&app, Trigger::Startup).await;
    });
}

/// 메뉴 「업데이트 확인…」.
pub fn on_menu<R: Runtime>(app: &AppHandle<R>) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move { check(&app, Trigger::Menu).await });
}

async fn check<R: Runtime>(app: &AppHandle<R>, trigger: Trigger) {
    if cfg!(debug_assertions) {
        tracing::info!(?trigger, "update check skipped: development build");
        if trigger == Trigger::Menu {
            notify(
                app,
                MessageDialogKind::Info,
                say(app, "update-dev-build", &[]),
            )
            .await;
        }
        return;
    }
    if CHECKING.swap(true, Ordering::SeqCst) {
        tracing::info!(?trigger, "update check already running");
        return;
    }
    run(app, trigger).await;
    CHECKING.store(false, Ordering::SeqCst);
}

async fn run<R: Runtime>(app: &AppHandle<R>, trigger: Trigger) {
    let current = app.package_info().version.to_string();
    let channel = app
        .try_state::<Settings>()
        .and_then(|s| s.0.as_ref().map(channel))
        .unwrap_or(Channel::Stable);
    tracing::info!(?trigger, ?channel, %current, "checking for updates");
    let update = match find(app, channel).await {
        Ok(update) => update,
        Err(e) => {
            tracing::warn!(?trigger, error = %e, "update check failed");
            if trigger == Trigger::Menu {
                let text = say(app, "update-check-failed", &[("error", e.to_string())]);
                notify(app, MessageDialogKind::Error, text).await;
            }
            return;
        }
    };
    let Some(update) = update else {
        tracing::info!(?trigger, %current, "no update");
        if trigger == Trigger::Menu {
            let text = say(app, "update-none", &[("current", current)]);
            notify(app, MessageDialogKind::Info, text).await;
        }
        return;
    };
    tracing::info!(version = %update.version, url = %update.download_url, "update available");
    let text = say(
        app,
        "update-available",
        &[("version", update.version.clone()), ("current", current)],
    );
    if !ask(app, text).await {
        tracing::info!(version = %update.version, "update postponed");
        return;
    }
    match update.download_and_install(|_, _| {}, || {}).await {
        Ok(()) => {
            tracing::info!(version = %update.version, "update installed; restarting");
            // 새 프로세스가 아직 살아 있는 이 앱을 「기존 앱」으로 보고 인자를 넘긴 뒤 끝나지 않게 소켓을 먼저 내려놓는다.
            tauri_plugin_single_instance::destroy(app);
            app.restart();
        }
        Err(e) => {
            tracing::error!(version = %update.version, error = %e, "update not installed");
            let text = say(app, "update-install-failed", &[("error", e.to_string())]);
            notify(app, MessageDialogKind::Error, text).await;
        }
    }
}

async fn find<R: Runtime>(
    app: &AppHandle<R>,
    channel: Channel,
) -> Result<Option<tauri_plugin_updater::Update>, CheckError> {
    let Some(url) = manifest_url(channel).await? else {
        return Ok(None);
    };
    let updater = app
        .updater_builder()
        .endpoints(vec![url])?
        .timeout(REQUEST_TIMEOUT)
        .build()?;
    Ok(updater.check().await?)
}

/// UI 언어의 문장(`extensions/i18n-default/i18n/*/menu.ftl`). `$app` 은 앱 이름이다.
fn say<R: Runtime>(app: &AppHandle<R>, id: &str, args: &[(&str, String)]) -> String {
    let mut fluent = FluentArgs::new();
    fluent.set("app", app.package_info().name.clone());
    for (name, value) in args {
        fluent.set(*name, value.clone());
    }
    app.try_state::<std::sync::Arc<I18n>>().map_or_else(
        || id.to_owned(),
        |i18n| i18n.localizer().format(id, Some(&fluent)),
    )
}

fn dialog<R: Runtime>(
    app: &AppHandle<R>,
    text: String,
) -> tauri_plugin_dialog::MessageDialogBuilder<R> {
    let mut dialog = app
        .dialog()
        .message(text)
        .title(say(app, "update-title", &[]));
    if let Some(parent) = windows::dialog_parent(app) {
        dialog = dialog.parent(&parent);
    }
    dialog
}

async fn notify<R: Runtime>(app: &AppHandle<R>, kind: MessageDialogKind, text: String) {
    let (tx, rx) = tokio::sync::oneshot::channel();
    dialog(app, text).kind(kind).show(move |_| {
        let _ = tx.send(());
    });
    let _ = rx.await;
}

/// 설치하고 다시 시작할지 묻는다. 대화상자를 닫으면 「나중에」다.
async fn ask<R: Runtime>(app: &AppHandle<R>, text: String) -> bool {
    let (tx, rx) = tokio::sync::oneshot::channel();
    dialog(app, text)
        .kind(MessageDialogKind::Info)
        .buttons(MessageDialogButtons::OkCancelCustom(
            say(app, "update-install", &[]),
            say(app, "update-later", &[]),
        ))
        .show(move |ok| {
            let _ = tx.send(ok);
        });
    rx.await.unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn releases(value: serde_json::Value) -> Vec<GithubRelease> {
        serde_json::from_value(value).unwrap()
    }

    fn release(tag: &str, manifest: bool) -> serde_json::Value {
        let mut assets = vec![
            json!({"name": "Pitwall.app.tar.gz", "browser_download_url": format!("https://x/{tag}/Pitwall.app.tar.gz")}),
        ];
        if manifest {
            assets.push(json!({"name": "latest.json", "browser_download_url": format!("https://x/{tag}/latest.json")}));
        }
        json!({"tag_name": tag, "draft": false, "prerelease": tag.contains('-'), "assets": assets})
    }

    #[test]
    fn the_channel_defaults_to_stable() {
        assert_eq!(parse_channel(Some(&json!("stable"))), Channel::Stable);
        assert_eq!(parse_channel(Some(&json!("preview"))), Channel::Preview);
        assert_eq!(parse_channel(None), Channel::Stable);
        assert_eq!(parse_channel(Some(&json!("Preview"))), Channel::Stable);
        assert_eq!(parse_channel(Some(&json!(true))), Channel::Stable);
    }

    #[test]
    fn the_channel_follows_the_global_setting() {
        use pw_core::bus::DEFAULT_CAPACITY;
        use pw_core::config::{AppPaths, ConfigSource};
        use pw_core::{EventBus, Scope};

        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::under(tmp.path());
        let open = || {
            ConfigStore::open(
                ConfigSource::global(&paths),
                EventBus::new(DEFAULT_CAPACITY),
                Scope::Host,
            )
        };
        assert_eq!(channel(&open()), Channel::Stable, "내장 기본값");
        std::fs::create_dir_all(&paths.config).unwrap();
        std::fs::write(
            paths.config.join("settings.toml"),
            "schema = 1\n[update]\nchannel = \"preview\"\n",
        )
        .unwrap();
        assert_eq!(channel(&open()), Channel::Preview);
    }

    /// 측정 · 시험 · 확인용 실행(기본이 아닌 `$DATA`)과 debug 빌드는 시작할 때 네트워크에 나가지 않는다.
    #[test]
    fn only_release_builds_with_the_default_data_check_on_startup() {
        assert!(auto_check(true, false));
        assert!(!auto_check(true, true));
        assert!(!auto_check(false, false));
        assert!(!auto_check(false, true));
    }

    #[test]
    fn this_debug_build_does_not_check_on_startup() {
        assert!(!auto_check(crate::RELEASE, false));
    }

    #[test]
    fn preview_takes_the_highest_version_with_a_manifest() {
        let list = releases(json!([
            release("v0.1.0-preview.2", true),
            release("v0.1.0-preview.10", true),
            release("v0.0.9", true),
        ]));
        let (version, url) = newest_manifest(&list).unwrap();
        assert_eq!(version.to_string(), "0.1.0-preview.10");
        assert_eq!(url, "https://x/v0.1.0-preview.10/latest.json");
    }

    /// 목록 순서(만든 시각)가 아니라 버전으로 고른다 — preview 뒤에 낮은 stable 을 냈어도 preview 를 받는다.
    #[test]
    fn preview_picks_by_version_not_by_list_order() {
        let list = releases(json!([
            release("v0.0.9", true),
            release("v0.1.0-preview.1", true),
        ]));
        assert_eq!(
            newest_manifest(&list).unwrap().0.to_string(),
            "0.1.0-preview.1"
        );
        let list = releases(json!([
            release("v0.1.0-preview.3", true),
            release("v0.1.0", true),
        ]));
        assert_eq!(newest_manifest(&list).unwrap().0.to_string(), "0.1.0");
    }

    #[test]
    fn preview_skips_drafts_odd_tags_and_releases_without_a_manifest() {
        let mut draft = release("v0.3.0", true);
        draft["draft"] = json!(true);
        let list = releases(json!([
            draft,
            release("v0.2.0", false),
            release("nightly", true),
            release("v0.1.0-preview.1", true),
        ]));
        assert_eq!(
            newest_manifest(&list).unwrap().1,
            "https://x/v0.1.0-preview.1/latest.json"
        );
        assert!(newest_manifest(&releases(json!([release("v0.2.0", false)]))).is_none());
    }

    /// 배포하는 앱의 updater 설정: 공개 키가 있고, 서명에 실린 버전을 요구하고, https 만 받는다. updater 산출물(서명 필요)은
    /// 릴리스 설정(`tauri.release.conf.json`)에서만 켠다 — 기본 설정으로 만드는 번들(`mise run perf`)은 서명 키 없이 된다.
    #[test]
    fn the_shipped_updater_config_requires_signed_versions() {
        let conf: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        let updater = &conf["plugins"]["updater"];
        assert!(updater["pubkey"].as_str().is_some_and(|k| !k.is_empty()));
        assert_eq!(updater["requireSignedVersion"], json!(true));
        assert!(updater.get("dangerousInsecureTransportProtocol").is_none());
        assert!(updater.get("endpoints").is_none(), "주소는 채널이 정한다");
        assert!(conf["bundle"].get("createUpdaterArtifacts").is_none());
        let release: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.release.conf.json")).unwrap();
        assert_eq!(release["bundle"]["createUpdaterArtifacts"], json!(true));
    }

    #[test]
    fn the_default_manifest_addresses_are_the_github_releases_of_this_repository() {
        // 빌드 때 바꾸지 않았을 때만 본다(로컬 업데이트 왕복 시험 빌드는 바꾼다).
        if option_env!("PITWALL_UPDATE_STABLE_MANIFEST").is_none() {
            assert_eq!(
                STABLE_MANIFEST,
                "https://github.com/jang2162/pitwall/releases/latest/download/latest.json"
            );
        }
        if option_env!("PITWALL_UPDATE_RELEASES_API").is_none() {
            assert!(
                RELEASES_API.starts_with("https://api.github.com/repos/jang2162/pitwall/releases")
            );
        }
    }
}
