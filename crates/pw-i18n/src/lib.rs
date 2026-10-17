//! Pitwall UI 언어(`docs/design/07-extensibility.md` §4.1): 확장 레지스트리의 UI 번역(Fluent `.ftl`)을 모으고, 언어 설정 ·
//! OS 언어로 쓸 언어를 고르고, Rust 가 그리는 문자열(네이티브 메뉴 등)을 만든다. 웹뷰는 같은 리소스를 `i18n.get` 으로 받아
//! `@fluent/react` 로 그린다(`apps/desktop/src/i18n`).
//!
//! - [`language`]: 설정 값(`settings.toml` 의 `ui.language`)과 언어 고르기.
//! - [`catalog`]: 등록된 언어와 [`Localizer`](catalog::Localizer).
//! - [`setting`]: 언어 설정 쓰기(그 키 하나만).
//! - [`methods`]: `i18n.get` · `i18n.setLanguage` RPC.

use std::sync::OnceLock;

use pw_core::config::ConfigStore;

pub mod catalog;
pub mod language;
pub mod methods;
pub mod setting;

pub use catalog::{Catalog, Language, Localizer};
pub use fluent_bundle::FluentArgs;
pub use language::{FALLBACK, LANGUAGE_KEY, LanguageSetting, SYSTEM, negotiate};

/// Host 하나의 UI 언어. 번역 목록은 시작할 때 한 번 만들고, 언어 설정은 부를 때마다 전역 설정의 지금 값을 읽는다 — 설정
/// 파일이 바뀌면(`config.changed`) 다음 호출부터 새 언어다.
pub struct I18n {
    catalog: Catalog,
    os_locales: Vec<String>,
    config: OnceLock<ConfigStore>,
}

/// 언어 설정을 바꾸지 못한 이유.
#[derive(Debug)]
pub enum SetLanguageError {
    /// `"system"` 도 등록된 언어도 아니다.
    Unknown(String),
    /// 전역 설정이 없는 Host 다.
    NoConfig,
    Write(setting::WriteError),
    /// 썼지만 설정을 다시 읽지 못했다(다른 설정 파일이 깨졌다 등). 파일 감시가 고쳐진 뒤 다시 읽는다.
    Reload(String),
}

impl std::fmt::Display for SetLanguageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SetLanguageError::Unknown(v) => {
                write!(f, "`{v}` is neither \"{SYSTEM}\" nor an installed language")
            }
            SetLanguageError::NoConfig => f.write_str("this host has no settings"),
            SetLanguageError::Write(e) => write!(f, "{e}"),
            SetLanguageError::Reload(e) => {
                write!(
                    f,
                    "the language was saved but settings could not be reloaded: {e}"
                )
            }
        }
    }
}

impl std::error::Error for SetLanguageError {}

impl I18n {
    /// `os_locales` 는 OS 선호 언어(앞이 먼저)다. 앱은 [`Self::system_locales`] 를 넘기고, 시험은 고정 값을 넘긴다.
    pub fn new(catalog: Catalog, os_locales: Vec<String>) -> Self {
        Self {
            catalog,
            os_locales,
            config: OnceLock::new(),
        }
    }

    /// OS 의 선호 언어 목록.
    pub fn system_locales() -> Vec<String> {
        sys_locale::get_locales().collect()
    }

    /// 언어 설정을 읽을 전역 설정. Host 를 만든 뒤에 붙인다(메서드는 Host 를 만드는 중에 등록된다). 두 번째부터는 무시한다.
    pub fn attach_config(&self, store: ConfigStore) {
        let _ = self.config.set(store);
    }

    pub fn catalog(&self) -> &Catalog {
        &self.catalog
    }

    /// 지금의 언어 설정. 설정이 없거나 잘못됐으면 `System`(잘못된 값은 경고로 남긴다).
    pub fn setting(&self) -> LanguageSetting {
        let Some(store) = self.config.get() else {
            return LanguageSetting::System;
        };
        let snapshot = store.snapshot();
        LanguageSetting::from_value(snapshot.get("settings", LANGUAGE_KEY)).unwrap_or_else(|e| {
            tracing::warn!(target: "pw_i18n", "{e}; following the system language");
            LanguageSetting::System
        })
    }

    /// 쓸 언어 순서(첫째가 고른 언어, 끝이 폴백).
    pub fn locales(&self) -> Vec<String> {
        negotiate(&self.setting(), &self.os_locales, &self.catalog.locales())
    }

    pub fn localizer(&self) -> Localizer {
        self.catalog.localizer(&self.locales())
    }

    /// 언어 설정을 쓰고(그 키만, [`setting`]) 전역 설정을 바로 다시 읽는다 — `config.changed` 가 나가 열린 창 · 메뉴가
    /// 손으로 파일을 고쳤을 때와 같은 길로 바뀐다. 파일 IO 를 하므로 블로킹 스레드에서 부른다.
    pub fn set_language(&self, value: &str) -> Result<(), SetLanguageError> {
        if value != SYSTEM && self.catalog.language(value).is_none() {
            return Err(SetLanguageError::Unknown(value.to_owned()));
        }
        let store = self.config.get().ok_or(SetLanguageError::NoConfig)?;
        let changed = setting::write_language(store.source().dir(), value)
            .map_err(SetLanguageError::Write)?;
        if changed {
            tracing::info!(target: "pw_i18n", language = value, "language setting written");
            store
                .reload()
                .map_err(|e| SetLanguageError::Reload(e.to_string()))?;
        }
        Ok(())
    }
}
