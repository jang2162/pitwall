//! 기본 번들 디버거가 쓰는 갈래(17-m6-plan.md M6-16 ③ · ④): `openvsx` 받기의 고정 해시 · `share`, `exec:` 의 실행 비트, `node:` ·
//! `pyvenv:` 의 런처(가짜 node · python 이 받은 argv 를 적는다), node 가 없을 때의 거절.

use std::os::unix::fs::PermissionsExt as _;
use std::process::Command;

use pw_lsp_servers::{Located, ToolLocator as _};

use crate::test_http::Reply;

use super::*;

const VSIX_PATH: &str =
    "/api/vscjava/vscode-java-debug/0.59.0/file/vscjava.vscode-java-debug-0.59.0.vsix";
const JAR: &str = "java-debug-adapter/com.microsoft.java.debug.plugin.jar";

fn java_debug_pkg() -> Value {
    json!({
        "name": "java-debug-adapter", "description": "d", "homepage": "https://example.test",
        "licenses": ["EPL-1.0"], "languages": ["Java"], "categories": ["DAP"],
        "source": {
            "id": "pkg:openvsx/vscjava/vscode-java-debug@0.59.0",
            "download": {"file": "vscjava.vscode-java-debug-{{version}}.vsix"},
        },
        "share": {
            "java-debug-adapter/": "extension/server/",
            JAR: "extension/server/com.microsoft.java.debug.plugin-0.53.2.jar",
        },
    })
}

const JAR_FILE: &str = "extension/server/com.microsoft.java.debug.plugin-0.53.2.jar";

fn vsix() -> Vec<u8> {
    zip(&[
        Entry::File("extension/package.json", b"{}".to_vec(), 0o644),
        Entry::File(JAR_FILE, crate::patches::tests::upstream_like_jar(), 0o644),
    ])
}

/// 그 jar 의 패치 고정 행(결정 D71) — 설치기와 같은 함수로 계산한 결과 해시.
fn patched_row() -> crate::pinned::PatchedFile {
    let jar = crate::patches::tests::upstream_like_jar();
    let out = crate::patches::patch_jar(&jar, &crate::patches::JAVA_DEBUG_LOOPBACK).unwrap();
    crate::pinned::PatchedFile {
        package: "java-debug-adapter".into(),
        version: "0.59.0".into(),
        file: JAR_FILE.into(),
        patch: "java-debug-loopback".into(),
        sha256: sha256_hex(&jar),
        patched_sha256: sha256_hex(&out),
    }
}

fn java_debug_registry(body: &[u8]) -> Arc<ToolsRegistry> {
    registry_patched(
        vec![java_debug_pkg()],
        &[vsix_row(body)],
        vec![patched_row()],
    )
}

fn vsix_row(body: &[u8]) -> Row {
    Row {
        url: format!("https://open-vsx.org{VSIX_PATH}"),
        ..Row::file(
            "java-debug-adapter",
            "0.59.0",
            "vscjava.vscode-java-debug-0.59.0.vsix",
            VSIX_PATH,
            body,
        )
    }
}

/// 주소 앞 `from` 을 가짜 서버로 바꿔 받는 설치기. 설치 폴더는 공백 · 홑따옴표가 든 경로다(런처가 경로를 따옴표로 싼다).
fn installer_at(
    fx: &Fx,
    registry: Arc<ToolsRegistry>,
    from: &str,
) -> (Arc<ToolsInstaller>, Layout) {
    let tools = fx.tmp.path().join("Application Support/it's/tools");
    let installer = ToolsInstaller::with_limits(registry, Limits::default()).with_test_network(
        Fetcher::for_install_loopback_tests(),
        (from.to_owned(), fx.server.base.clone()),
    );
    installer.attach(InstallerHost {
        tools_dir: tools.clone(),
        state_dir: fx.state(),
        tasks: fx.tasks.clone(),
        toolchains: Arc::clone(&fx.chains) as Arc<dyn Toolchains>,
        settings_window: Arc::new(|l: &str| l == "settings"),
    });
    (Arc::new(installer), Layout::new(tools))
}

fn openvsx_installer(fx: &Fx, registry: Arc<ToolsRegistry>) -> (Arc<ToolsInstaller>, Layout) {
    installer_at(fx, registry, "https://open-vsx.org")
}

/// 완료 기준 ④: Open VSX 에서 받은 `.vsix` 의 해시가 고정 값과 다르면 설치 전체가 실패하고(그 판의 폴더 · 기록이 남지 않는다),
/// 같으면 zip 으로 풀어 `share` 를 기록한다 — jar 는 `locate_share` 로 찾고, 사라지면 「다시 설치」 다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_openvsx_download_must_match_its_pinned_hash() {
    let fx = Fx::new().await;
    let body = vsix();
    fx.server.route(VSIX_PATH, Reply::Body(body.clone()));

    let mut wrong = vsix_row(&body);
    wrong.sha256 = "f".repeat(64);
    let (installer, layout) = openvsx_installer(
        &fx,
        registry_patched(vec![java_debug_pkg()], &[wrong], vec![patched_row()]),
    );
    fx.consent(&installer, "java-debug-adapter");
    let err = installer
        .install(&settings_grant(), "java-debug-adapter", None, None, false)
        .await
        .unwrap_err();
    assert_eq!(
        code_and_stage(&err),
        (code::INSTALL_FAILED, Some("checksum".to_owned()))
    );
    assert_eq!(fx.server.hits(VSIX_PATH), 1);
    assert!(!layout.version_dir("java-debug-adapter", "0.59.0").exists());
    assert_eq!(layout.read_record("java-debug-adapter").unwrap(), None);

    let (installer, layout) = openvsx_installer(&fx, java_debug_registry(&body));
    fx.consent(&installer, "java-debug-adapter");
    let result = installer
        .install(&settings_grant(), "java-debug-adapter", None, None, false)
        .await
        .unwrap();
    assert_eq!(result.checksum, ToolChecksum::Pinned);
    let record = layout.read_record("java-debug-adapter").unwrap().unwrap();
    let entry = record.current_entry().unwrap();
    assert_eq!(entry.kind, PurlKind::Openvsx);
    assert_eq!(
        entry.source_urls,
        [format!("https://open-vsx.org{VSIX_PATH}")]
    );
    assert!(entry.bins.is_empty());
    let dir = layout.version_dir("java-debug-adapter", "0.59.0");
    let locator = InstalledTools::new(layout.clone());
    let jar = dir.join("extension/server/com.microsoft.java.debug.plugin-0.53.2.jar");
    assert_eq!(
        locator.locate_share("java-debug-adapter", JAR),
        Located::Installed {
            path: jar.clone(),
            version: "0.59.0".into()
        }
    );
    assert_eq!(
        locator.locate_share("java-debug-adapter", "no/such/share"),
        Located::Absent
    );
    assert_eq!(locator.locate_share("absent-package", JAR), Located::Absent);
    // 설치된 jar 는 패치한 것이고(결정 D71) 옆에 수정 고지가 있다.
    assert_eq!(
        sha256_hex(&std::fs::read(&jar).unwrap()),
        patched_row().patched_sha256
    );
    let notice =
        std::fs::read_to_string(dir.join("extension/server/NOTICE-pitwall-java-debug.txt"))
            .unwrap();
    assert!(notice.contains("Eclipse Public License v1.0"), "{notice}");
    std::fs::remove_file(&jar).unwrap();
    assert!(matches!(
        locator.locate_share("java-debug-adapter", JAR),
        Located::Broken { .. }
    ));
}

/// 결정 D71: java-debug 는 패치한 jar 로만 설치된다 — 그 판의 고정 행이 없거나, 받은 jar · 바꾼 결과의 해시가 행과 다르면 설치
/// 전체가 실패하고(단계 `patch`) 그 판의 폴더 · 기록이 남지 않는다. 원본 jar 로 설치를 끝내는 길이 없다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn java_debug_installs_only_with_its_pinned_patch() {
    let fx = Fx::new().await;
    let body = vsix();
    fx.server.route(VSIX_PATH, Reply::Body(body.clone()));
    let no_row = registry(vec![java_debug_pkg()], &[vsix_row(&body)]);
    let mut bad_result = patched_row();
    bad_result.patched_sha256 = "e".repeat(64);
    let mut bad_input = patched_row();
    bad_input.sha256 = "d".repeat(64);
    for (what, reg) in [
        ("no row", no_row),
        (
            "result",
            registry_patched(vec![java_debug_pkg()], &[vsix_row(&body)], vec![bad_result]),
        ),
        (
            "input",
            registry_patched(vec![java_debug_pkg()], &[vsix_row(&body)], vec![bad_input]),
        ),
    ] {
        let (installer, layout) = openvsx_installer(&fx, reg);
        fx.consent(&installer, "java-debug-adapter");
        let err = installer
            .install(&settings_grant(), "java-debug-adapter", None, None, false)
            .await
            .unwrap_err();
        assert_eq!(
            code_and_stage(&err),
            (code::INSTALL_FAILED, Some("patch".to_owned())),
            "{what}: {err:?}"
        );
        assert!(
            !layout.version_dir("java-debug-adapter", "0.59.0").exists(),
            "{what}"
        );
        assert_eq!(
            layout.read_record("java-debug-adapter").unwrap(),
            None,
            "{what}"
        );
    }
}

/// 받은 것 속의 링크가 설치 폴더 밖을 가리키는 `share` 는 찾지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_share_that_leaves_the_install_folder_is_not_located() {
    let fx = Fx::new().await;
    let body = vsix();
    fx.server.route(VSIX_PATH, Reply::Body(body.clone()));
    let (installer, layout) = openvsx_installer(&fx, java_debug_registry(&body));
    fx.consent(&installer, "java-debug-adapter");
    installer
        .install(&settings_grant(), "java-debug-adapter", None, None, false)
        .await
        .unwrap();
    let dir = layout.version_dir("java-debug-adapter", "0.59.0");
    let jar = dir.join("extension/server/com.microsoft.java.debug.plugin-0.53.2.jar");
    let outside = fx.tmp.path().join("outside.jar");
    std::fs::write(&outside, b"evil").unwrap();
    std::fs::remove_file(&jar).unwrap();
    std::os::unix::fs::symlink(&outside, &jar).unwrap();
    assert!(matches!(
        InstalledTools::new(layout).locate_share("java-debug-adapter", JAR),
        Located::Broken { .. }
    ));
}

/// 가짜 node · python: 받은 argv 를 한 줄씩 표준 출력에 쓴다. python 은 `-I -m venv <폴더>` 면 venv 를 흉내 낸다(venv 의 python 은
/// 이 스크립트로 가는 링크), pip 은 아무것도 하지 않는다.
const ECHO_NODE: &str = "#!/bin/sh\necho node\nfor a in \"$@\"; do printf '%s\\n' \"$a\"; done\n";
const FAKE_PYTHON: &str = r#"#!/bin/sh
if [ "$1" = "-I" ] && [ "$2" = "-m" ] && [ "$3" = "venv" ]; then
  mkdir -p "$4/bin"
  ln -s "$0" "$4/bin/python"
  printf 'home = /fake\nversion_info = 3.12.1\n' > "$4/pyvenv.cfg"
  exit 0
fi
if [ "$3" = "pip" ]; then exit 0; fi
echo python
for a in "$@"; do printf '%s\n' "$a"; done
"#;

fn fake_runtimes(fx: &Fx) -> PathBuf {
    let dir = fx.tmp.path().join("runtimes");
    std::fs::create_dir_all(&dir).unwrap();
    for (name, body) in [("node", ECHO_NODE), ("python3", FAKE_PYTHON)] {
        let p = dir.join(name);
        std::fs::write(&p, body).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let env = env_with_path(&[&dir, Path::new("/usr/bin"), Path::new("/bin")]);
    *fx.chains.project.lock().unwrap() = env.clone();
    *fx.chains.login.lock().unwrap() = env;
    dir
}

fn adapter_archive() -> Vec<u8> {
    targz(&[
        Entry::Dir("extension/"),
        Entry::Dir("extension/adapter/"),
        // 보관 파일은 실행 비트를 적지 않았다 — `exec:` 이 준다.
        Entry::File(
            "extension/adapter/codelldb",
            b"#!/bin/sh\necho lldb\n".to_vec(),
            0o644,
        ),
        Entry::Dir("js-debug/"),
        Entry::Dir("js-debug/src/"),
        Entry::File("js-debug/src/dapDebugServer.js", b"//".to_vec(), 0o644),
    ])
}

fn run(path: &Path, args: &[&str], path_env: &str) -> (Option<i32>, String, String) {
    let out = Command::new(path)
        .args(args)
        .env_clear()
        .env("PATH", path_env)
        .output()
        .unwrap();
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// 완료 기준 ③: `exec:` 은 설치 폴더 안 파일에 실행 비트를 주고 그 경로가 실행 파일이다. `node:` 는 PATH 의 node 로 그 스크립트를,
/// `pyvenv:` 는 그 패키지 venv 의 python 으로 `-I -m <모듈>` 을 돌리는 런처가 실행 파일이다(설치 폴더의 공백 · 홑따옴표에도). 런처는
/// 받은 인자를 그대로 넘긴다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exec_node_and_pyvenv_bins_run_from_the_install_folder() {
    let fx = Fx::new().await;
    let runtimes = fake_runtimes(&fx);
    let body = adapter_archive();
    fx.server
        .route("/adapters.tar.gz", Reply::Body(body.clone()));
    let (installer, layout) = installer_at(&fx, registry_of_adapters(), FAKE);
    for name in ["adapters", "debugpy"] {
        fx.consent(&installer, name);
        installer
            .install(&project_grant("p1"), name, None, None, false)
            .await
            .unwrap_or_else(|e| panic!("{name}: {e:?}"));
    }
    let locator = InstalledTools::new(layout.clone());
    let path_of = |package: &str, bin: &str| match locator.locate(package, bin) {
        Located::Installed { path, .. } => path,
        other => panic!("{package} {bin}: {other:?}"),
    };
    let dir = layout.version_dir("adapters", "1.0.0");
    let with_node = format!("{}:/usr/bin:/bin", runtimes.display());

    // exec: 그 파일 그대로, 실행 비트가 섰다.
    let lldb = path_of("adapters", "codelldb");
    assert_eq!(lldb, dir.join("extension/adapter/codelldb"));
    assert_eq!(mode_of(&lldb), 0o755);
    assert_eq!(run(&lldb, &[], &with_node).1, "lldb\n");

    // node: 런처 → PATH 의 node 가 설치 폴더의 스크립트(절대 경로)와 인자를 받는다.
    let js = path_of("adapters", "js-debug-adapter");
    assert_eq!(js, dir.join("pitwall-bin/js-debug-adapter"));
    assert_eq!(mode_of(&js), 0o755);
    let script = std::fs::canonicalize(dir.join("js-debug/src/dapDebugServer.js")).unwrap();
    let (code, out, _) = run(&js, &["4711", "127.0.0.1", "it's x"], &with_node);
    assert_eq!(code, Some(0));
    assert_eq!(
        out,
        format!("node\n{}\n4711\n127.0.0.1\nit's x\n", script.display())
    );
    // `package.json` 없는 배포물의 스크립트가 바깥 폴더의 `"type": "module"` 을 따르지 않게 설치 폴더 꼭대기에 commonjs 표지가
    // 있다(M6-19). `node:` 가 없는 설치(debugpy)에는 두지 않는다.
    assert_eq!(
        std::fs::read_to_string(dir.join("package.json")).unwrap(),
        "{ \"type\": \"commonjs\" }\n"
    );
    assert!(
        !layout
            .version_dir("debugpy", "1.8.22")
            .join("package.json")
            .exists()
    );
    // PATH 에 node 가 없으면 그렇다고 적고 127 로 끝난다.
    let (code, _, err) = run(&js, &["4711"], "/nonexistent-pitwall-bin");
    assert_eq!(code, Some(127));
    assert!(
        err.contains("node is needed to run js-debug-adapter"),
        "{err}"
    );

    // pyvenv: 런처 → 그 venv 의 python 이 `-I -m debugpy.adapter` 와 인자를 받는다(PATH 에 python 이 없어도).
    let py = path_of("debugpy", "debugpy-adapter");
    let py_dir = layout.version_dir("debugpy", "1.8.22");
    assert_eq!(py, py_dir.join("pitwall-bin/debugpy-adapter"));
    let (code, out, err) = run(&py, &["--port", "5678"], "/nonexistent-pitwall-bin");
    assert_eq!(code, Some(0), "{err}");
    assert_eq!(out, "python\n-I\n-m\ndebugpy.adapter\n--port\n5678\n");
}

fn registry_of_adapters() -> Arc<ToolsRegistry> {
    registry(
        vec![
            files_pkg(
                "adapters",
                "1.0.0",
                &[("adapters.tar.gz", "/adapters.tar.gz")],
                &[
                    ("codelldb", "exec:extension/adapter/codelldb"),
                    ("js-debug-adapter", "node:js-debug/src/dapDebugServer.js"),
                ],
                &["MIT"],
            ),
            json!({
                "name": "debugpy", "description": "d", "homepage": "https://example.test",
                "licenses": ["MIT"], "languages": [], "categories": ["DAP"],
                "source": {"id": "pkg:pypi/debugpy@1.8.22"},
                "bin": {"debugpy-adapter": "pyvenv:debugpy.adapter"},
            }),
        ],
        &[],
    )
}

/// `node:` 실행 파일이 있는 패키지는 런타임 환경에 node 가 없으면 받기 전에 「node 필요」(4002)로 거절한다 — 받지도 폴더를 만들지도
/// 않는다. 설치에 쓰지 않으므로 「돌리려면」 이라고 적는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_node_bin_without_node_is_refused_before_downloading() {
    let fx = Fx::new().await;
    *fx.chains.project.lock().unwrap() = env_with_path(&[Path::new("/nonexistent-pitwall-bin")]);
    *fx.chains.login.lock().unwrap() = env_with_path(&[Path::new("/nonexistent-pitwall-bin")]);
    fx.server
        .route("/adapters.tar.gz", Reply::Body(adapter_archive()));
    let installer = fx.installer(registry_of_adapters(), Limits::default());
    fx.consent(&installer, "adapters");
    for grant in [project_grant("p1"), settings_grant()] {
        let err = installer
            .install(&grant, "adapters", None, None, false)
            .await
            .unwrap_err();
        assert_eq!(err.code, code::RUNTIME_MISSING);
        assert_eq!(err.data.as_ref().unwrap()["runtime"], "node");
        assert_eq!(
            err.message,
            "node is needed to run this tool — install it with mise"
        );
    }
    assert_eq!(fx.server.hits("/adapters.tar.gz"), 0);
    assert!(!fx.layout().package_dir("adapters").exists());
}
