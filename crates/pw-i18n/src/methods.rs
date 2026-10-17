//! UI 언어 RPC. 웹뷰는 `i18n.get` 으로 고른 언어의 `.ftl` 리소스를 받고(Rust 와 같은 리소스 · 같은 언어 고르기), 전역 설정이
//! 바뀌면(`config.changed`) 다시 부른다.

use std::sync::Arc;

use pw_rpc::{Router, RpcError, rpc_method};
use serde::{Deserialize, Serialize};

use crate::{I18n, SetLanguageError};

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct LanguageInfo {
    /// BCP 47 언어 태그.
    pub locale: String,
    /// 그 언어로 쓴 그 언어의 이름(`language-name` 메시지, 없으면 태그).
    pub name: String,
    /// 기여한 확장의 id.
    pub extension: String,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct I18nResource {
    pub locale: String,
    pub extension: String,
    /// 사람이 읽는 위치(`builtin:i18n-default/i18n/ko/app.ftl` 또는 절대 경로).
    pub file: String,
    /// `.ftl` 파일 내용 그대로.
    pub source: String,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct I18nState {
    /// 설정 값(`"system"` 또는 언어 태그). 설정이 잘못됐으면 `"system"`.
    pub setting: String,
    /// 쓸 언어 순서. 첫째가 고른 언어, 끝이 폴백(`en`)이다.
    pub locales: Vec<String>,
    /// 등록된 언어(등록 순서). 언어 고르기 목록이다.
    pub languages: Vec<LanguageInfo>,
    /// `locales` 의 리소스. `locales` 순서, 언어 안에서는 파일 이름순이다.
    pub resources: Vec<I18nResource>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct SetLanguageParams {
    /// `"system"` 또는 등록된 언어 태그.
    pub language: String,
}

rpc_method! {
    /// 지금 쓸 UI 언어와 그 Fluent 리소스, 등록된 언어 목록.
    pub I18nGet: "i18n.get" {
        params: (),
        result: I18nState,
        tier: Read,
    }

    /// UI 언어 설정(전역 `settings.toml` 의 `ui.language`)을 바꾼다. 그 키만 고쳐 쓰고 설정을 다시 읽는다 — 열린 창 ·
    /// 메뉴는 `config.changed` 로 바뀐다. 모르는 언어면 invalid params, 파일을 고칠 수 없으면(TOML 이 깨졌다 등) 쓰지
    /// 않고 오류다.
    pub I18nSetLanguage: "i18n.setLanguage" {
        params: SetLanguageParams,
        result: (),
        tier: Write,
    }
}

pub fn register(router: &mut Router, i18n: Arc<I18n>) {
    let get = Arc::clone(&i18n);
    router
        .unary(I18nGet, move |_ctx, ()| {
            let state = state(&get);
            async move { Ok(state) }
        })
        .unary(I18nSetLanguage, move |_ctx, params: SetLanguageParams| {
            let i18n = Arc::clone(&i18n);
            async move {
                tokio::task::spawn_blocking(move || i18n.set_language(&params.language))
                    .await
                    .map_err(|e| RpcError::internal(e.to_string()))?
                    .map_err(|e| match e {
                        SetLanguageError::Unknown(_) => RpcError::invalid_params(e.to_string()),
                        _ => RpcError::internal(e.to_string()),
                    })
            }
        });
}

pub fn state(i18n: &I18n) -> I18nState {
    let catalog = i18n.catalog();
    let locales = i18n.locales();
    let resources = locales
        .iter()
        .filter_map(|locale| catalog.language(locale))
        .flat_map(|language| {
            language.files.iter().map(|file| I18nResource {
                locale: language.locale.clone(),
                extension: language.extension.clone(),
                file: file.display.clone(),
                source: file.source.clone(),
            })
        })
        .collect();
    I18nState {
        setting: i18n.setting().as_str().to_owned(),
        locales,
        languages: catalog
            .languages()
            .iter()
            .map(|l| LanguageInfo {
                locale: l.locale.clone(),
                name: l.name.clone(),
                extension: l.extension.clone(),
            })
            .collect(),
        resources,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use pw_core::bus::{DEFAULT_CAPACITY, EventBus};
    use pw_core::config::{AppPaths, ConfigSource, ConfigStore, TOPIC_CHANGED};
    use pw_core::{Filter, Received, Scope};
    use pw_ext::Registry;
    use pw_rpc::{Caller, EventHub, Outbound, Role, Session};
    use semver::Version;
    use serde_json::{Value, json};

    use super::*;
    use crate::Catalog;

    struct Fixture {
        _tmp: tempfile::TempDir,
        paths: AppPaths,
        bus: EventBus,
        i18n: Arc<I18n>,
    }

    /// 내장 번역과 임시 `$CONFIG` 의 전역 설정. OS 언어는 `os`.
    fn fixture(settings: Option<&str>, os: &[&str]) -> Fixture {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::under(tmp.path());
        if let Some(text) = settings {
            fs::create_dir_all(&paths.config).unwrap();
            fs::write(paths.config.join("settings.toml"), text).unwrap();
        }
        let bus = EventBus::new(DEFAULT_CAPACITY);
        let store = ConfigStore::open(ConfigSource::global(&paths), bus.clone(), Scope::Host);
        let registry = Registry::from_roots(&Version::new(0, 0, 0), pw_ext::builtin::roots());
        let i18n = Arc::new(I18n::new(
            Catalog::from_registry(&registry),
            os.iter().map(|s| (*s).to_owned()).collect(),
        ));
        i18n.attach_config(store);
        Fixture {
            _tmp: tmp,
            paths,
            bus,
            i18n,
        }
    }

    async fn call(i18n: &Arc<I18n>, request: Value) -> Value {
        let mut router = Router::new();
        register(&mut router, Arc::clone(i18n));
        let (session, mut rx) =
            Session::new(Arc::new(router), EventHub::new(), Caller::new(Role::Owner));
        session.handle_value(request);
        match rx.recv().await.unwrap() {
            reply @ Outbound::Response(_) => serde_json::to_value(reply).unwrap(),
            other => panic!("expected a response, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn get_returns_the_chosen_language_then_english_with_their_resources() {
        let f = fixture(Some("schema = 1\n[ui]\nlanguage = \"ko\"\n"), &["en-US"]);
        let reply = call(
            &f.i18n,
            json!({"jsonrpc": "2.0", "id": 1, "method": "i18n.get"}),
        )
        .await;
        let result = &reply["result"];
        assert_eq!(result["setting"], "ko", "{reply}");
        assert_eq!(result["locales"], json!(["ko", "en"]));
        assert_eq!(
            result["languages"],
            json!([
                {"locale": "en", "name": "English", "extension": "dev.pitwall.i18n-default"},
                {"locale": "ko", "name": "한국어", "extension": "dev.pitwall.i18n-default"}
            ])
        );
        let files: Vec<&str> = result["resources"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["file"].as_str().unwrap())
            .collect();
        assert_eq!(
            files,
            [
                "builtin:i18n-default/i18n/ko/app.ftl",
                "builtin:i18n-default/i18n/ko/menu.ftl",
                "builtin:i18n-default/i18n/ko/welcome.ftl",
                "builtin:i18n-default/i18n/en/app.ftl",
                "builtin:i18n-default/i18n/en/menu.ftl",
                "builtin:i18n-default/i18n/en/welcome.ftl",
            ]
        );
        assert_eq!(
            result["resources"][0]["source"],
            include_str!("../../../extensions/i18n-default/i18n/ko/app.ftl")
        );
    }

    #[test]
    fn system_follows_the_os_and_a_broken_setting_counts_as_system() {
        let f = fixture(None, &["ko-KR"]);
        assert_eq!(f.i18n.locales(), ["ko", "en"]);
        let f = fixture(Some("schema = 1\n[ui]\nlanguage = 7\n"), &["ko-KR"]);
        assert_eq!(f.i18n.setting(), crate::LanguageSetting::System);
        assert_eq!(f.i18n.locales(), ["ko", "en"]);
    }

    /// 언어를 바꾸면 그 키만 쓰고 설정을 바로 다시 읽어 `config.changed` 가 나간다 — 다음 `i18n.get` · 메뉴가 새 언어다.
    #[tokio::test]
    async fn set_language_writes_the_key_reloads_and_announces_the_change() {
        let f = fixture(Some("# 내 설정\nschema = 1\n"), &["en-US"]);
        let mut events = f.bus.subscribe(Filter::all());
        assert_eq!(f.i18n.locales(), ["en"]);
        let reply = call(
            &f.i18n,
            json!({"jsonrpc": "2.0", "id": 1, "method": "i18n.setLanguage", "params": {"language": "ko"}}),
        )
        .await;
        assert_eq!(reply["result"], Value::Null, "{reply}");
        assert_eq!(
            fs::read_to_string(f.paths.config.join("settings.toml")).unwrap(),
            "# 내 설정\nschema = 1\n\n[ui]\nlanguage = \"ko\"\n"
        );
        assert_eq!(f.i18n.locales(), ["ko", "en"]);
        let Some(Received::Event(event)) = events.try_recv() else {
            panic!("config.changed 가 나가지 않았다");
        };
        assert_eq!(event.topic, TOPIC_CHANGED);
        assert!(events.try_recv().is_none(), "이벤트는 한 번");
    }

    #[tokio::test]
    async fn set_language_rejects_an_unknown_language_and_a_broken_file() {
        let f = fixture(Some("schema = 1\n"), &["en-US"]);
        let reply = call(
            &f.i18n,
            json!({"jsonrpc": "2.0", "id": 1, "method": "i18n.setLanguage", "params": {"language": "ja"}}),
        )
        .await;
        assert_eq!(
            reply["error"]["code"],
            pw_rpc::code::INVALID_PARAMS,
            "{reply}"
        );

        let settings = f.paths.config.join("settings.toml");
        fs::write(&settings, "schema = = 1\n").unwrap();
        let reply = call(
            &f.i18n,
            json!({"jsonrpc": "2.0", "id": 1, "method": "i18n.setLanguage", "params": {"language": "ko"}}),
        )
        .await;
        assert_eq!(
            reply["error"]["code"],
            pw_rpc::code::INTERNAL_ERROR,
            "{reply}"
        );
        assert_eq!(fs::read_to_string(&settings).unwrap(), "schema = = 1\n");
    }
}
