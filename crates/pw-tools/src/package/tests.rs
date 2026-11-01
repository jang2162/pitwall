//! 패키지 정의 파싱 · 대상별 풀기 · 덮어쓰기.

use serde_json::json;

use super::*;
use crate::fetch::sha256_hex;
use crate::index::{Index, RegistryOrigin};
use crate::overrides::{Override, Overrides};
use crate::pinned::Pinned;
use crate::source::unzip_registry;

/// 고정 판 전체(`tests/fixtures/registry.json.zip` — bump 스크립트가 고정 판과 함께 넣는다). 고정 해시와 같은지 먼저 본다.
pub(crate) fn pinned_registry() -> Index {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/registry.json.zip"
    );
    let zip = std::fs::read(path).unwrap();
    let pinned = Pinned::builtin();
    assert_eq!(
        sha256_hex(&zip),
        pinned.registry.sha256,
        "fixture is not the pinned registry"
    );
    let json = unzip_registry(&zip).unwrap();
    assert_eq!(sha256_hex(&json), pinned.registry.json_sha256);
    Index::from_registry_json(&json, &pinned.registry.version, RegistryOrigin::Cache, None).unwrap()
}

fn pkg(value: serde_json::Value) -> Package {
    Package::parse(&value).unwrap()
}

fn files_of(r: &ResolvedTool) -> Vec<(String, String)> {
    match &r.source {
        ToolSource::Files { files } => files
            .iter()
            .map(|f| (f.name.clone(), f.url.clone()))
            .collect(),
        other => panic!("not files: {other:?}"),
    }
}

/// 완료 기준 ③: 고정 판의 모든 패키지를 읽어 대상마다 지원 · 지원하지 않음(까닭별)을 내고, 기본 번들은 모두 지원이다.
#[test]
fn every_package_of_the_pinned_registry_parses_and_the_bundled_ones_are_supported() {
    let index = pinned_registry();
    let overrides = Overrides::builtin();
    let pinned = Pinned::builtin();
    assert!(index.packages().len() > 500, "{}", index.packages().len());
    assert_eq!(index.unreadable, 0);
    println!(
        "registry {}: {} packages",
        index.version,
        index.packages().len()
    );
    for target in ToolTarget::ALL {
        let stats = index.stats(Some(target), &overrides);
        println!(
            "{target}: supported {} · unsupported {} {:?}",
            stats.supported, stats.unsupported, stats.by_reason
        );
        assert_eq!(stats.supported + stats.unsupported, index.packages().len());
        // M4 가 설치하지 않는 purl(cargo · golang · openvsx …)과 빌드 패키지는 늘 있다.
        assert!(stats.by_reason["purlType"] > 0);
        assert!(stats.by_reason["build"] > 0);
        assert!(stats.supported > stats.unsupported);
        for name in &pinned.bundled {
            let p = index.get(name).unwrap_or_else(|| panic!("{name} missing"));
            let r = p.resolve(Some(target), overrides.get(name));
            if name == "kotlin-lsp" && target == ToolTarget::LinuxArm64Gnu {
                // registry 정의의 `{{{` 오타(15-m4-plan.md M4-03 ⑤) — 그 대상만 빠진다.
                assert!(
                    matches!(r, Err(ToolUnsupported::Expression { .. })),
                    "{r:?}"
                );
            } else {
                assert!(r.is_ok(), "{name} on {target}: {r:?}");
            }
        }
    }
    // 대상이 없는 기기는 모두 지원하지 않음.
    let none = index.stats(None, &overrides);
    assert_eq!(none.supported, 0);
}

/// 완료 기준 ⑤: 번들 패키지의 자산 이름 · `download` 파일 이름(과 그 주소)이 대상마다 기대값이다.
#[test]
fn bundled_asset_and_download_names_per_target() {
    let index = Index::snapshot();
    let over = Overrides::default();
    let resolve =
        |name: &str, t: ToolTarget| index.get(name).unwrap().resolve(Some(t), over.get(name));

    let ra = [
        (
            ToolTarget::DarwinArm64,
            "rust-analyzer-aarch64-apple-darwin.gz",
        ),
        (
            ToolTarget::DarwinX64,
            "rust-analyzer-x86_64-apple-darwin.gz",
        ),
        (
            ToolTarget::LinuxX64Gnu,
            "rust-analyzer-x86_64-unknown-linux-gnu.gz",
        ),
        (
            ToolTarget::LinuxArm64Gnu,
            "rust-analyzer-aarch64-unknown-linux-gnu.gz",
        ),
        (
            ToolTarget::LinuxX64Musl,
            "rust-analyzer-x86_64-unknown-linux-musl.gz",
        ),
        (
            ToolTarget::WinX64,
            "rust-analyzer-x86_64-pc-windows-msvc.zip",
        ),
    ];
    for (t, file) in ra {
        let r = resolve("rust-analyzer", t).unwrap();
        let v = r.version.clone();
        assert_eq!(
            files_of(&r),
            vec![(
                file.to_owned(),
                format!("https://github.com/rust-lang/rust-analyzer/releases/download/{v}/{file}")
            )],
            "{t}"
        );
        let bin = match t {
            ToolTarget::WinX64 => "rust-analyzer.exe".to_owned(),
            _ => file.trim_end_matches(".gz").to_owned(),
        };
        assert_eq!(r.bin["rust-analyzer"], ToolBin::Path(bin), "{t}");
    }

    let ruff = [
        (
            ToolTarget::DarwinArm64,
            "ruff-aarch64-apple-darwin.tar.gz",
            "ruff-aarch64-apple-darwin/ruff",
        ),
        (
            ToolTarget::DarwinX64,
            "ruff-x86_64-apple-darwin.tar.gz",
            "ruff-x86_64-apple-darwin/ruff",
        ),
        // `linux_x64` 항목(musl 파일)이 gnu 에도 맞는다 — Mason 과 같다.
        (
            ToolTarget::LinuxX64Gnu,
            "ruff-x86_64-unknown-linux-musl.tar.gz",
            "ruff-x86_64-unknown-linux-musl/ruff",
        ),
        (
            ToolTarget::LinuxArm64Gnu,
            "ruff-aarch64-unknown-linux-gnu.tar.gz",
            "ruff-aarch64-unknown-linux-gnu/ruff",
        ),
        (
            ToolTarget::LinuxX64Musl,
            "ruff-x86_64-unknown-linux-musl.tar.gz",
            "ruff-x86_64-unknown-linux-musl/ruff",
        ),
        (
            ToolTarget::WinX64,
            "ruff-x86_64-pc-windows-msvc.zip",
            "ruff.exe",
        ),
    ];
    for (t, file, bin) in ruff {
        let r = resolve("ruff", t).unwrap();
        assert_eq!(files_of(&r)[0].0, file, "{t}");
        assert_eq!(r.bin["ruff"], ToolBin::Path(bin.into()), "{t}");
    }

    // kotlin-lsp: 저장 이름이 형식을 정한다(`.sit` 주소를 `kotlin-lsp.zip` 으로), 판의 `kotlin-lsp/v` 접두를 벗긴다.
    let kotlin = [
        (
            ToolTarget::DarwinArm64,
            "kotlin-lsp.zip",
            "kotlin-server-263.7084.0-aarch64.sit",
        ),
        (
            ToolTarget::DarwinX64,
            "kotlin-lsp.zip",
            "kotlin-server-263.7084.0.sit",
        ),
        (
            ToolTarget::LinuxX64Gnu,
            "kotlin-lsp.tar.gz",
            "kotlin-server-263.7084.0.tar.gz",
        ),
        (
            ToolTarget::LinuxX64Musl,
            "kotlin-lsp.tar.gz",
            "kotlin-server-263.7084.0.tar.gz",
        ),
        (
            ToolTarget::WinX64,
            "kotlin-lsp.zip",
            "kotlin-server-263.7084.0.win.zip",
        ),
    ];
    for (t, name, url_file) in kotlin {
        let r = resolve("kotlin-lsp", t).unwrap();
        let ToolSource::Files { files } = &r.source else {
            panic!()
        };
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].name, name, "{t}");
        assert_eq!(
            files[0].url,
            format!(
                "https://download.jetbrains.com/language-server/kotlin-server/263.7084.0/{url_file}"
            ),
            "{t}"
        );
        assert_eq!(
            files[0].format,
            if name.ends_with(".zip") {
                ArchiveFormat::Zip
            } else {
                ArchiveFormat::TarGz
            }
        );
        let bin = if t == ToolTarget::WinX64 {
            "bin/intellij-server.exe".to_owned()
        } else {
            "kotlin-server-263.7084.0/bin/intellij-server".to_owned()
        };
        assert_eq!(r.bin["intellij-server"], ToolBin::Path(bin), "{t}");
    }
    assert!(matches!(
        resolve("kotlin-lsp", ToolTarget::LinuxArm64Gnu),
        Err(ToolUnsupported::Expression { expression }) if expression.starts_with("{ version")
    ));

    // jdtls: `download` 의 두 파일, `{{source.download.config}}` 로 고른 설정 폴더, python 런처.
    for (t, config) in [
        (ToolTarget::DarwinArm64, "config_mac/"),
        (ToolTarget::LinuxX64Gnu, "config_linux/"),
        (ToolTarget::WinX64, "config_win/"),
    ] {
        let r = resolve("jdtls", t).unwrap();
        let names: Vec<String> = files_of(&r).into_iter().map(|(n, _)| n).collect();
        assert_eq!(names, ["jdtls.tar.gz", "lombok.jar"], "{t}");
        assert!(
            files_of(&r)[0].1.contains("jdt-language-server-1.63.0-"),
            "{t}"
        );
        assert_eq!(r.share["jdtls/config/"], config, "{t}");
        assert_eq!(r.bin["jdtls"], ToolBin::Python("bin/jdtls".into()));
    }

    // npm · PyPI 는 대상과 상관없다.
    let ts = resolve("typescript-language-server", ToolTarget::WinX64).unwrap();
    assert_eq!(
        ts.source,
        ToolSource::Npm {
            package: ToolPackageSpec {
                name: "typescript-language-server".into(),
                version: Some(ts.version.clone())
            },
            extra: vec![ToolPackageSpec {
                name: "typescript".into(),
                version: Some("6.0.3".into())
            }],
        }
    );
    assert_eq!(
        ts.bin["typescript-language-server"],
        ToolBin::Npm("typescript-language-server".into())
    );
    let bp = resolve("basedpyright", ToolTarget::LinuxX64Musl).unwrap();
    assert!(
        matches!(&bp.source, ToolSource::Pypi { package, extra } if package.name == "basedpyright" && extra.is_empty())
    );
    assert_eq!(
        bp.bin["basedpyright-langserver"],
        ToolBin::Pypi("basedpyright-langserver".into())
    );
}

fn github_def(bin: &str) -> serde_json::Value {
    json!({
        "name": "demo", "description": "d", "homepage": "https://example.com",
        "licenses": ["MIT"], "languages": [], "categories": ["LSP"],
        "source": {"id": "pkg:github/o/r@v1.2.0", "asset": [
            {"target": "darwin", "file": "demo-{{ version | strip_prefix \"v\" }}-mac.tar.gz:libexec/", "bin": "libexec/demo"},
            {"target": ["linux_x64", "win_x64"], "file": ["a.zip", "b.gz"], "bin": "a"},
        ]},
        "bin": {"demo": bin},
    })
}

#[test]
fn unknown_expressions_bins_keys_and_urls_make_the_package_unsupported() {
    // 정상: 목록 `file` · `:풀 곳` · 표현식 필터.
    let p = pkg(github_def("{{source.asset.bin}}"));
    let r = p.resolve(Some(ToolTarget::DarwinArm64), None).unwrap();
    let ToolSource::Files { files } = &r.source else {
        panic!()
    };
    assert_eq!(files[0].name, "demo-1.2.0-mac.tar.gz");
    assert_eq!(files[0].dest.as_deref(), Some("libexec/"));
    assert_eq!(
        files[0].url,
        "https://github.com/o/r/releases/download/v1.2.0/demo-1.2.0-mac.tar.gz"
    );
    assert_eq!(r.bin["demo"], ToolBin::Path("libexec/demo".into()));
    let r = p.resolve(Some(ToolTarget::WinX64), None).unwrap();
    assert_eq!(files_of(&r).len(), 2);
    assert_eq!(
        p.resolve(Some(ToolTarget::LinuxArm64Gnu), None),
        Err(ToolUnsupported::Target)
    );
    assert_eq!(p.resolve(None, None), Err(ToolUnsupported::Target));

    // 모르는 필터 · 함수 → 그 패키지가 지원하지 않음(추측해 채우지 않는다).
    let p = pkg(github_def("{{ version | upper }}"));
    assert!(matches!(
        p.resolve(Some(ToolTarget::DarwinArm64), None),
        Err(ToolUnsupported::Expression { .. })
    ));
    let p = pkg(github_def("{{ take_if_not(is_platform(\"win\"), \"x\") }}"));
    assert!(matches!(
        p.resolve(Some(ToolTarget::DarwinArm64), None),
        Err(ToolUnsupported::Expression { .. })
    ));

    // 지원하지 않는 위임 · npm 아닌 패키지의 `npm:` · 폴더 밖 경로.
    for (bin, bad) in [
        ("node:server.js", true),
        ("java-jar:x.jar", true),
        ("exec:x", true),
        ("npm:demo", true),
        ("python:../x.py", true),
        ("python:bin/x.py", false),
    ] {
        let r = pkg(github_def(bin)).resolve(Some(ToolTarget::DarwinArm64), None);
        assert_eq!(
            matches!(r, Err(ToolUnsupported::ToolBin { .. })),
            bad,
            "{bin}: {r:?}"
        );
    }
    for bin in ["../escape", "/abs/path", "a//b", "a\\b"] {
        let r = pkg(github_def(bin)).resolve(Some(ToolTarget::DarwinArm64), None);
        assert!(
            matches!(r, Err(ToolUnsupported::InvalidDefinition { .. })),
            "{bin}: {r:?}"
        );
    }

    // 모르는 키(최상위 · source) → 그 패키지만 지원하지 않음, 목록에는 남는다.
    let mut v = github_def("a");
    v["future_key"] = json!(1);
    let p = pkg(v);
    assert_eq!(
        p.problem,
        Some(ToolUnsupported::UnknownKey {
            key: "future_key".into()
        })
    );
    let mut v = github_def("a");
    v["source"]["mirror"] = json!("x");
    assert_eq!(
        pkg(v).resolve(Some(ToolTarget::DarwinArm64), None),
        Err(ToolUnsupported::UnknownKey {
            key: "source.mirror".into()
        })
    );
    // 아는 키(쓰지 않는 것)는 문제가 아니다.
    let mut v = github_def("a");
    v["neovim"] = json!({"lspconfig": "x"});
    v["schemas"] = json!({"lsp": "x"});
    assert!(pkg(v).problem.is_none());

    // http 주소 · 폴더 밖 저장 이름 · 빌드.
    let generic = |files: serde_json::Value| {
        pkg(json!({
            "name": "g", "description": "", "homepage": "", "licenses": [], "languages": [], "categories": [],
            "source": {"id": "pkg:generic/x/g@1", "download": {"files": files}},
            "bin": {"g": "g"},
        }))
    };
    assert!(matches!(
        generic(json!({"g": "http://example.com/g"})).resolve(Some(ToolTarget::DarwinArm64), None),
        Err(ToolUnsupported::InsecureUrl { .. })
    ));
    assert!(matches!(
        generic(json!({"../g": "https://example.com/g"}))
            .resolve(Some(ToolTarget::DarwinArm64), None),
        Err(ToolUnsupported::InvalidDefinition { .. })
    ));
    assert!(
        generic(json!({"g": "https://example.com/g"}))
            .resolve(Some(ToolTarget::DarwinArm64), None)
            .is_ok()
    );
    let mut v = github_def("a");
    v["source"]["build"] = json!({"run": "make"});
    assert_eq!(
        pkg(v).resolve(Some(ToolTarget::DarwinArm64), None),
        Err(ToolUnsupported::Build)
    );

    // purl 형식.
    let mut v = github_def("a");
    v["source"]["id"] = json!("pkg:cargo/ripgrep@14");
    assert_eq!(
        pkg(v).resolve(Some(ToolTarget::DarwinArm64), None),
        Err(ToolUnsupported::PurlType {
            purl_type: "cargo".into()
        })
    );
    // supported_platforms.
    let mut v = github_def("a");
    v["source"]["supported_platforms"] = json!(["linux"]);
    assert_eq!(
        pkg(v).resolve(Some(ToolTarget::DarwinArm64), None),
        Err(ToolUnsupported::Target)
    );
}

#[test]
fn untrusted_sizes_and_shapes_are_bounded() {
    let mut v = github_def("a");
    v["description"] = json!("x".repeat(MAX_TEXT_BYTES + 1));
    assert!(matches!(
        pkg(v).problem,
        Some(ToolUnsupported::InvalidDefinition { .. })
    ));
    let mut v = github_def("a");
    v["licenses"] = json!(vec!["MIT"; MAX_LIST_ITEMS + 1]);
    assert!(matches!(
        pkg(v).problem,
        Some(ToolUnsupported::InvalidDefinition { .. })
    ));
    assert!(Package::parse(&json!({"description": "no name"})).is_err());
    assert!(Package::parse(&json!({"name": "../x"})).is_err());
    assert!(Package::parse(&json!("string")).is_err());

    // 목록 수준: 배열이 아니면 · 상한을 넘으면 오류, 읽지 못한 항목 · 겹친 이름은 세고 뺀다.
    assert!(Index::from_registry_json(b"{}", "v", RegistryOrigin::Cache, None).is_err());
    assert!(Index::from_registry_json(b"[", "v", RegistryOrigin::Cache, None).is_err());
    let big = vec![b' '; crate::index::MAX_REGISTRY_JSON_BYTES + 1];
    assert!(Index::from_registry_json(&big, "v", RegistryOrigin::Cache, None).is_err());
    let many = serde_json::to_vec(&vec![json!(1); crate::index::MAX_PACKAGES + 1]).unwrap();
    assert!(Index::from_registry_json(&many, "v", RegistryOrigin::Cache, None).is_err());
    let list =
        serde_json::to_vec(&json!([github_def("a"), github_def("b"), 5, {"name": 3}])).unwrap();
    let index = Index::from_registry_json(&list, "v", RegistryOrigin::Cache, None).unwrap();
    assert_eq!(index.packages().len(), 1);
    assert_eq!(index.unreadable, 3);
}

#[test]
fn extra_package_specs_and_archive_formats() {
    let spec = |s: &str, k| ToolPackageSpec::parse(s, k).map(|p| (p.name, p.version));
    assert_eq!(
        spec("typescript@6.0.3", PurlKind::Npm),
        Ok(("typescript".into(), Some("6.0.3".into())))
    );
    assert_eq!(
        spec("@vue/ts-plugin", PurlKind::Npm),
        Ok(("@vue/ts-plugin".into(), None))
    );
    assert_eq!(
        spec("@vue/ts@1", PurlKind::Npm),
        Ok(("@vue/ts".into(), Some("1".into())))
    );
    assert_eq!(spec("toml", PurlKind::Pypi), Ok(("toml".into(), None)));
    assert_eq!(
        spec("toml==0.10", PurlKind::Pypi),
        Ok(("toml".into(), Some("0.10".into())))
    );
    assert!(spec("a b@1", PurlKind::Npm).is_err());
    assert!(spec("x@1;rm -rf", PurlKind::Npm).is_err());

    for (name, f) in [
        ("kotlin-lsp.zip", ArchiveFormat::Zip),
        ("a.tar.gz", ArchiveFormat::TarGz),
        ("a.TGZ", ArchiveFormat::TarGz),
        ("a.gz", ArchiveFormat::Gz),
        ("a.tar.xz", ArchiveFormat::TarXz),
        ("a.vsix", ArchiveFormat::Zip),
        ("lombok.jar", ArchiveFormat::Plain),
        ("server.sit", ArchiveFormat::Plain),
    ] {
        assert_eq!(ArchiveFormat::from_name(name), f, "{name}");
    }
}

/// 완료 기준 ⑧: 덮어쓰기 정의가 registry 정의의 판 · 주소 · `bin` 을 이긴다.
#[test]
fn an_override_wins_over_the_registry_definition() {
    let index = Index::snapshot();
    let jdtls = index.get("jdtls").unwrap();
    let over = Override::parse(
        r#"
reason = "test: pinned milestone instead of the snapshot URL"
version = "v1.99.0"

[[download]]
target = ["darwin", "linux"]
files = { "jdtls.tar.gz" = "https://download.eclipse.org/jdtls/milestones/{{ version | strip_prefix \"v\" }}/jdt-language-server-{{ version | strip_prefix \"v\" }}.tar.gz", "lombok.jar" = "https://projectlombok.org/downloads/lombok-1.18.40.jar" }
config = "config_unix/"

[bin]
jdtls = "plugins/org.eclipse.equinox.launcher.jar"
"#,
    )
    .unwrap();
    let r = jdtls
        .resolve(Some(ToolTarget::DarwinArm64), Some(&over))
        .unwrap();
    assert!(r.overridden);
    assert_eq!(r.version, "v1.99.0");
    assert_eq!(
        files_of(&r),
        vec![
            (
                "jdtls.tar.gz".into(),
                "https://download.eclipse.org/jdtls/milestones/1.99.0/jdt-language-server-1.99.0.tar.gz".into()
            ),
            ("lombok.jar".into(), "https://projectlombok.org/downloads/lombok-1.18.40.jar".into()),
        ]
    );
    assert_eq!(
        r.bin["jdtls"],
        ToolBin::Path("plugins/org.eclipse.equinox.launcher.jar".into())
    );
    // 덮어쓰지 않은 `share` 는 registry 의 것이고, 표현식은 덮어쓴 항목의 값을 읽는다.
    assert_eq!(r.share["jdtls/config/"], "config_unix/");
    // 덮어쓰기의 `download` 에 없는 대상은 registry 로 돌아가지 않고 빠진다.
    assert_eq!(
        jdtls.resolve(Some(ToolTarget::WinX64), Some(&over)),
        Err(ToolUnsupported::Target)
    );
    // 덮어쓰기 없이는 registry 의 스냅숏 주소.
    let plain = jdtls.resolve(Some(ToolTarget::DarwinArm64), None).unwrap();
    assert!(!plain.overridden);
    assert!(files_of(&plain)[0].1.contains("/jdtls/snapshots/"));

    // 덮어쓰기의 주소도 같은 규칙(https 만)으로 검사한다.
    let bad = Override::parse(
        "reason = \"x\"\n[[download]]\nfiles = { \"a.zip\" = \"http://example.com/a.zip\" }\n",
    )
    .unwrap();
    assert!(matches!(
        jdtls.resolve(Some(ToolTarget::DarwinArm64), Some(&bad)),
        Err(ToolUnsupported::InsecureUrl { .. })
    ));
}
