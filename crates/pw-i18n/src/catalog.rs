//! 확장 레지스트리의 UI 번역을 모은 목록([`Catalog`])과, 고른 언어 순서로 메시지를 만드는 [`Localizer`].

use fluent_bundle::concurrent::FluentBundle;
use fluent_bundle::{FluentArgs, FluentResource};
use pw_ext::{Registry, TranslationFile};
use unic_langid::LanguageIdentifier;

/// 언어 이름 메시지. 언어마다 그 언어로 쓴 자기 이름이다(언어 고르기 목록).
pub const LANGUAGE_NAME: &str = "language-name";

/// 등록된 언어 하나.
#[derive(Debug, Clone)]
pub struct Language {
    pub locale: String,
    /// 기여한 확장의 id.
    pub extension: String,
    /// 그 언어의 [`LANGUAGE_NAME`]. 없으면 언어 태그.
    pub name: String,
    pub files: Vec<TranslationFile>,
}

/// 앱이 시작할 때 레지스트리에서 한 번 만든다. 레지스트리가 등록할 때 `.ftl` 문법을 이미 검사했다.
#[derive(Debug, Clone, Default)]
pub struct Catalog {
    languages: Vec<Language>,
}

impl Catalog {
    pub fn from_registry(registry: &Registry) -> Self {
        let languages = registry
            .loaded_translations()
            .map(|(extension, t)| {
                let mut language = Language {
                    locale: t.locale.clone(),
                    extension: extension.manifest.id.clone(),
                    name: t.locale.clone(),
                    files: t.files.clone(),
                };
                if let Some(name) = bundle(&language).and_then(|b| format(&b, LANGUAGE_NAME, None))
                {
                    language.name = name;
                }
                language
            })
            .collect();
        Self { languages }
    }

    /// 등록 순서(내장 → 사용자 확장).
    pub fn languages(&self) -> &[Language] {
        &self.languages
    }

    pub fn locales(&self) -> Vec<String> {
        self.languages.iter().map(|l| l.locale.clone()).collect()
    }

    pub fn language(&self, locale: &str) -> Option<&Language> {
        self.languages.iter().find(|l| l.locale == locale)
    }

    /// `locales` 순서의 [`Localizer`]. 등록되지 않은 언어는 건너뛴다.
    pub fn localizer(&self, locales: &[String]) -> Localizer {
        let bundles = locales
            .iter()
            .filter_map(|locale| self.language(locale))
            .filter_map(bundle)
            .collect();
        Localizer {
            locales: locales.to_vec(),
            bundles,
        }
    }
}

type Bundle = FluentBundle<FluentResource>;

/// 언어 하나의 번들. 양방향 격리 문자(FSI · PDI)를 넣지 않는다 — 오른쪽에서 왼쪽으로 쓰는 언어가 아직 없고, 넣으면 경로 ·
/// 이름이 낀 문자열에 보이지 않는 글자가 섞여 복사 · 비교가 어긋난다. TS 쪽(`apps/desktop/src/i18n`)도 같게 둔다.
fn bundle(language: &Language) -> Option<Bundle> {
    let id: LanguageIdentifier = match language.locale.parse() {
        Ok(id) => id,
        Err(e) => {
            tracing::warn!(target: "pw_i18n", locale = %language.locale, %e, "language tag not understood");
            return None;
        }
    };
    let mut bundle = FluentBundle::new_concurrent(vec![id]);
    bundle.set_use_isolating(false);
    for file in &language.files {
        // 레지스트리가 문법 · 중복 id 를 이미 거절했다. 여기 실패는 그 검사와 갈라진 것이라 경고로 남기고 그 파일만 뺀다.
        let resource = match FluentResource::try_new(file.source.clone()) {
            Ok(resource) => resource,
            Err((_, errors)) => {
                tracing::warn!(target: "pw_i18n", file = %file.display, ?errors, "Fluent resource not loaded");
                continue;
            }
        };
        if let Err(errors) = bundle.add_resource(resource) {
            tracing::warn!(target: "pw_i18n", file = %file.display, ?errors, "Fluent resource not added");
        }
    }
    Some(bundle)
}

fn format(bundle: &Bundle, id: &str, args: Option<&FluentArgs>) -> Option<String> {
    let pattern = bundle.get_message(id)?.value()?;
    let mut errors = Vec::new();
    let text = bundle.format_pattern(pattern, args, &mut errors);
    if !errors.is_empty() {
        tracing::debug!(target: "pw_i18n", id, ?errors, "Fluent message formatted with errors");
    }
    Some(text.into_owned())
}

/// 고른 언어 순서로 메시지를 만든다. 앞 언어에 없는 메시지는 뒤 언어(끝은 폴백 `en`)에서 찾는다.
pub struct Localizer {
    locales: Vec<String>,
    bundles: Vec<Bundle>,
}

impl Localizer {
    /// 쓰는 언어 순서. 첫째가 고른 언어다.
    pub fn locales(&self) -> &[String] {
        &self.locales
    }

    pub fn text(&self, id: &str) -> String {
        self.format(id, None)
    }

    /// 어느 언어에도 없는 메시지는 id 를 그대로 돌려준다 — 빈 칸보다 무엇이 빠졌는지 보인다.
    pub fn format(&self, id: &str, args: Option<&FluentArgs>) -> String {
        self.bundles
            .iter()
            .find_map(|b| format(b, id, args))
            .unwrap_or_else(|| id.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use pw_ext::ExtensionRoot;
    use semver::Version;

    use super::*;

    fn builtin() -> Catalog {
        Catalog::from_registry(&Registry::from_roots(
            &Version::new(0, 0, 0),
            pw_ext::builtin::roots(),
        ))
    }

    #[test]
    fn the_builtin_catalog_names_each_language_in_itself() {
        let catalog = builtin();
        let names: Vec<(&str, &str)> = catalog
            .languages()
            .iter()
            .map(|l| (l.locale.as_str(), l.name.as_str()))
            .collect();
        assert_eq!(names, [("en", "English"), ("ko", "한국어")]);
    }

    #[test]
    fn formats_with_variables_and_without_isolation_marks() {
        let ko = builtin().localizer(&["ko".into(), "en".into()]);
        let mut args = FluentArgs::new();
        args.set("app", "Pitwall");
        assert_eq!(ko.format("menu-quit", Some(&args)), "Pitwall 종료");
        let en = builtin().localizer(&["en".into()]);
        assert_eq!(en.format("menu-quit", Some(&args)), "Quit Pitwall");
    }

    /// 고른 언어에 없는 메시지는 폴백 언어에서 찾고, 어디에도 없으면 id 를 낸다.
    #[test]
    fn missing_messages_fall_back_to_the_next_language_then_the_id() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("pack");
        std::fs::create_dir_all(root.join("ko-kp")).unwrap();
        std::fs::write(
            root.join("extension.toml"),
            "id = \"dev.example.pack\"\nname = \"P\"\nversion = \"1.0.0\"\nengines = { pitwall = \"*\" }\n\
             [contributes.translations.ko-kp]\ndir = \"ko-kp\"\n",
        )
        .unwrap();
        std::fs::write(root.join("ko-kp/app.ftl"), "menu-file = 화일\n").unwrap();
        let mut roots = pw_ext::builtin::roots();
        roots.push(ExtensionRoot::Dir(root));
        let registry = Registry::from_roots(&Version::new(0, 0, 0), roots);
        assert_eq!(registry.diagnostics(), []);
        let catalog = Catalog::from_registry(&registry);
        // 언어 이름 메시지가 없는 언어는 태그로 이름을 댄다.
        assert_eq!(catalog.language("ko-kp").unwrap().name, "ko-kp");
        let l = catalog.localizer(&["ko-kp".into(), "en".into()]);
        assert_eq!(l.text("menu-file"), "화일");
        assert_eq!(l.text("menu-edit"), "Edit");
        assert_eq!(l.text("no-such-message"), "no-such-message");
    }
}
