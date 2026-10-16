//! Pitwall 확장: `extension.toml` 매니페스트, 내장 확장, 기여 레지스트리(`docs/design/07-extensibility.md`).
//!
//! - [`manifest`]: 매니페스트 파서 · 검증. 진단에 파일 · 줄 · 열이 붙는다.
//! - [`source`]: 확장 폴더 안의 경로와 두 출처(바이너리에 묶인 내장 · 디스크).
//! - [`builtin`]: 저장소의 `extensions/` 를 묶은 내장 확장.
//! - [`registry`]: 내장과 `$DATA/extensions/` 를 읽어 테마 · 키맵 · 언어 · 외부 도구 · UI 번역 기여를 모은다.
//! - [`methods`]: `ext.list` · `theme.list` · `theme.get` · `keymap.list` · `keymap.get` RPC.
//!
//! L2 WASM 런타임은 아직 없다 — `[wasm]` 은 읽고 검사만 한다.

pub mod builtin;
pub mod manifest;
pub mod methods;
pub mod registry;
pub mod source;

pub use manifest::{Diagnostic, KeymapKind, Manifest, Pos, Severity};
pub use registry::{
    Appearance, Extension, LoadOptions, LoadedTheme, Registry, ResolvedFile, ResolvedKeymap,
};
pub use source::{ExtensionRoot, RelPath};

#[cfg(test)]
mod tests {
    use semver::Version;

    use super::*;

    fn builtin_registry() -> Registry {
        Registry::from_roots(&Version::new(0, 0, 0), builtin::roots())
    }

    #[test]
    fn builtin_extensions_are_bundled_by_folder() {
        let folders: Vec<String> = builtin::roots().iter().map(|r| r.display(None)).collect();
        assert_eq!(
            folders,
            [
                "builtin:external-tools",
                "builtin:keymap-default",
                "builtin:theme-default"
            ]
        );
    }

    #[test]
    fn the_default_theme_loads_through_the_builtin_extension() {
        let registry = builtin_registry();
        let (extension, theme) = registry.theme("pitwall-default").unwrap();
        assert_eq!(extension.manifest.id, "dev.pitwall.theme-default");
        assert!(extension.root.is_builtin());
        assert_eq!(theme.appearances, [Appearance::Light, Appearance::Dark]);
    }

    #[test]
    fn the_builtin_external_tools_cover_f8_1() {
        let registry = builtin_registry();
        let ids: Vec<&str> = registry
            .external_tools()
            .map(|(_, t)| t.id.as_str())
            .collect();
        assert_eq!(
            ids,
            [
                "orca",
                "zed",
                "idea",
                "vscode",
                "cursor",
                "finder",
                "explorer",
                "file-manager",
                "terminal"
            ]
        );
    }

    #[test]
    fn builtin_extensions_load_without_diagnostics() {
        let registry = builtin_registry();
        assert_eq!(registry.diagnostics(), []);
        assert_eq!(registry.extensions().len(), 3);
    }

    #[test]
    fn the_default_keymap_loads_through_the_builtin_extension() {
        let keymap = builtin_registry().keymap("pitwall-default").unwrap();
        assert_eq!(keymap.extension, "dev.pitwall.keymap-default");
        assert_eq!(keymap.kind, KeymapKind::Base);
        assert_eq!(
            keymap.file.display(),
            "builtin:keymap-default/keymaps/default.toml"
        );
        assert_eq!(
            keymap.file.read().unwrap().as_ref(),
            include_bytes!("../../../extensions/keymap-default/keymaps/default.toml")
        );
    }

    /// 디스크에 둔 확장에서도 같은 매니페스트가 키맵을 등록하고 그 파일의 디스크 위치를 내준다.
    #[test]
    fn the_default_keymap_manifest_resolves_its_file() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("keymap-default");
        std::fs::create_dir_all(root.join("keymaps")).unwrap();
        std::fs::write(
            root.join("extension.toml"),
            include_str!("../../../extensions/keymap-default/extension.toml"),
        )
        .unwrap();
        std::fs::write(root.join("keymaps/default.toml"), "# fixture\n").unwrap();

        let registry =
            Registry::from_roots(&Version::new(0, 0, 0), [ExtensionRoot::Dir(root.clone())]);
        assert_eq!(registry.diagnostics(), []);
        let keymap = registry.keymap("pitwall-default").unwrap();
        assert_eq!(keymap.extension, "dev.pitwall.keymap-default");
        assert_eq!(keymap.kind, KeymapKind::Base);
        assert_eq!(
            keymap.file.display(),
            root.join("keymaps/default.toml").display().to_string()
        );
        assert_eq!(keymap.file.read().unwrap().as_ref(), b"# fixture\n");
    }
}
