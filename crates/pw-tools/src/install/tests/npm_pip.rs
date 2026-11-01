//! ④ npm · pip 을 격리해 부른다(가짜 npm · node · python 이 argv · env · cwd 를 적는다) · ⑨ node · python 이 없으면 「필요」 오류.

use std::os::unix::fs::PermissionsExt as _;

use super::*;

const FAKE_NPM: &str = r#"#!/bin/sh
{ for a in "$@"; do printf '%s\n' "$a"; done; } > "$FAKE_LOG.npm.argv"
env > "$FAKE_LOG.npm.env"
pwd -P > "$FAKE_LOG.npm.cwd"
prefix=""
prev=""
for a in "$@"; do
  if [ "$prev" = "--prefix" ]; then prefix="$a"; fi
  prev="$a"
done
mkdir -p "$prefix/node_modules/.bin"
printf '#!/bin/sh\n' > "$prefix/node_modules/.bin/$FAKE_BIN"
chmod 755 "$prefix/node_modules/.bin/$FAKE_BIN"
cp "$FAKE_LOCK" "$prefix/package-lock.json"
"#;

const FAKE_PYTHON: &str = r#"#!/bin/sh
if [ "$1" = "-I" ] && [ "$2" = "-m" ] && [ "$3" = "venv" ]; then
  { for a in "$@"; do printf '%s\n' "$a"; done; } > "$FAKE_LOG.venv.argv"
  mkdir -p "$4/bin"
  ln -s "$0" "$4/bin/python"
  printf 'home = /fake\nversion_info = 3.12.1\n' > "$4/pyvenv.cfg"
  exit 0
fi
{ for a in "$@"; do printf '%s\n' "$a"; done; } > "$FAKE_LOG.pip.argv"
env > "$FAKE_LOG.pip.env"
pwd -P > "$FAKE_LOG.pip.cwd"
prev=""
for a in "$@"; do
  if [ "$prev" = "-r" ]; then cp "$a" "$FAKE_LOG.pip.req"; fi
  prev="$a"
done
mkdir -p venv/bin
printf '#!/bin/sh\n' > "venv/bin/$FAKE_BIN"
chmod 755 "venv/bin/$FAKE_BIN"
"#;

const INTEGRITY: &str = "sha512-AAAA";

struct Runtimes {
    dir: PathBuf,
    log: PathBuf,
    project_root: PathBuf,
}

/// 가짜 npm · node · python3 을 한 폴더에 두고, 로그인 셸 · 프로젝트 환경을 채운다. 둘 다 출처를 바꾸려는 변수를 담는다.
fn runtimes(fx: &Fx, bin: &str, lock: &str) -> Runtimes {
    let dir = fx.tmp.path().join("runtimes");
    std::fs::create_dir_all(&dir).unwrap();
    for (name, body) in [
        ("npm", FAKE_NPM),
        ("node", "#!/bin/sh\n"),
        ("python3", FAKE_PYTHON),
    ] {
        let p = dir.join(name);
        std::fs::write(&p, body).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let log = fx.tmp.path().join("log");
    let lock_file = fx.tmp.path().join("lock.json");
    std::fs::write(&lock_file, lock).unwrap();
    // 프로젝트 루트의 .npmrc · pip.conf 는 출처를 바꾸려 한다.
    let project_root = fx.tmp.path().join("project");
    std::fs::create_dir_all(&project_root).unwrap();
    std::fs::write(project_root.join(".npmrc"), "registry=https://evil.test/\n").unwrap();
    std::fs::write(
        project_root.join("pip.conf"),
        "[global]\nindex-url = https://evil.test/simple\n",
    )
    .unwrap();
    let shell_bin = fx.tmp.path().join("shell-bin");
    std::fs::create_dir_all(&shell_bin).unwrap();

    let mut login = env_with_path(&[&shell_bin, Path::new("/usr/bin"), Path::new("/bin")]);
    for (k, v) in [
        ("FAKE_LOG", log.to_str().unwrap()),
        ("FAKE_BIN", bin),
        ("FAKE_LOCK", lock_file.to_str().unwrap()),
        ("HOME_LIKE", "kept"),
        ("npm_config_registry", "https://evil-login.test/"),
        ("NPM_CONFIG_USERCONFIG", "/evil/npmrc"),
        ("PIP_INDEX_URL", "https://evil-login.test/simple"),
        ("PYTHONPATH", "/evil"),
        ("PythonStartup", "/evil.py"),
    ] {
        login.vars.insert(k.into(), v.into());
    }
    *fx.chains.login.lock().unwrap() = login;
    // 프로젝트의 런타임 환경(mise `[env]`): node · python 을 여기서 찾지만, 그 밖의 변수는 설치 프로세스에 닿지 않는다.
    let mut project = env_with_path(&[&dir, Path::new("/usr/bin"), Path::new("/bin")]);
    for (k, v) in [
        ("NPM_CONFIG_REGISTRY", "https://evil-project.test/"),
        ("PIP_INDEX_URL", "https://evil-project.test/simple"),
        ("FROM_PROJECT_ENV", "1"),
        ("PWD", project_root.to_str().unwrap()),
    ] {
        project.vars.insert(k.into(), v.into());
    }
    *fx.chains.project.lock().unwrap() = project;
    Runtimes {
        dir,
        log,
        project_root,
    }
}

fn read_lines(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
        .lines()
        .map(str::to_owned)
        .collect()
}

fn env_names(lines: &[String]) -> Vec<String> {
    lines
        .iter()
        .filter_map(|l| l.split_once('=').map(|(k, _)| k.to_owned()))
        .collect()
}

/// 지운 변수 · 프로젝트 환경 · 프로젝트 경로가 설치 프로세스에 닿지 않았다.
fn assert_isolated(rt: &Runtimes, argv: &[String], env: &[String], cwd: &str, dir: &Path) {
    let names = env_names(env);
    for bad in [
        "npm_config_registry",
        "NPM_CONFIG_USERCONFIG",
        "NPM_CONFIG_REGISTRY",
        "PIP_INDEX_URL",
        "PYTHONPATH",
        "PythonStartup",
        "FROM_PROJECT_ENV",
    ] {
        assert!(
            !names.iter().any(|n| n == bad),
            "{bad} reached the installer: {names:?}"
        );
    }
    assert!(
        names.iter().any(|n| n == "HOME_LIKE"),
        "the login shell environment is the base"
    );
    let all = format!("{argv:?}{env:?}");
    assert!(!all.contains("evil"), "{all}");
    assert!(!all.contains(rt.project_root.to_str().unwrap()), "{all}");
    let path = env.iter().find_map(|l| l.strip_prefix("PATH=")).unwrap();
    assert!(path.starts_with(rt.dir.to_str().unwrap()), "{path}");
    assert_eq!(Path::new(cwd), std::fs::canonicalize(dir).unwrap());
}

fn lock_json(entries: &[(&str, &str, &str)]) -> String {
    let mut packages = serde_json::Map::new();
    packages.insert("".into(), json!({"name": "x"}));
    for (name, version, integrity) in entries {
        packages.insert(
            format!("node_modules/{name}"),
            json!({"version": version, "integrity": integrity}),
        );
    }
    json!({"lockfileVersion": 3, "packages": packages}).to_string()
}

fn npm_rows(lock_integrity: &str) -> Vec<Row> {
    ["demo-ls", "typescript"]
        .iter()
        .zip(["1.0.0", "5.0.0"])
        .map(|(dist, ver)| Row {
            package: "demo-ls".into(),
            version: "1.0.0".into(),
            target: "any".into(),
            name: format!("{dist}-{ver}.tgz"),
            url: format!("https://registry.npmjs.org/{dist}/-/{dist}-{ver}.tgz"),
            sha256: "1".repeat(64),
            size: 10,
            dist: Some(((*dist).into(), ver.into())),
            integrity: Some(lock_integrity.into()),
        })
        .collect()
}

/// npm: `--ignore-scripts` · 고정 `--registry` · 빈 `--userconfig` · `--globalconfig`, 명세는 `--` 뒤, cwd 는 설치 폴더, 환경은 로그인 셸
/// 바탕에 고른 node 의 폴더만 얹는다. 프로젝트 창에서 불렀으면 node 는 그 프로젝트의 런타임 환경에서 찾는다. `package-lock.json` 의
/// `integrity` 가 고정 값과 다르면 설치 실패.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn npm_runs_isolated_with_fixed_registry_and_checks_integrity() {
    let fx = Fx::new().await;
    let rt = runtimes(
        &fx,
        "demo-ls",
        &lock_json(&[
            ("demo-ls", "1.0.0", INTEGRITY),
            ("typescript", "5.0.0", INTEGRITY),
        ]),
    );
    let reg = registry(
        vec![npm_pkg("demo-ls", "1.0.0", &["typescript@5.0.0"])],
        &npm_rows(INTEGRITY),
    );
    let installer = fx.installer(reg, Limits::default());
    fx.consent(&installer, "demo-ls");
    let result = installer
        .install(&project_grant("p1"), "demo-ls", None, None, false)
        .await
        .unwrap();
    assert_eq!(result.checksum, ToolChecksum::Pinned);
    assert_eq!(
        *fx.chains.asked.lock().unwrap(),
        vec![Some("p1".to_owned())]
    );
    let dir = fx.layout().version_dir("demo-ls", "1.0.0");
    let argv = read_lines(&rt.log.with_extension("npm.argv"));
    let env = read_lines(&rt.log.with_extension("npm.env"));
    let cwd = read_lines(&rt.log.with_extension("npm.cwd")).remove(0);
    assert_eq!(argv[0], "install");
    let pos = |s: &str| {
        argv.iter()
            .position(|a| a == s)
            .unwrap_or_else(|| panic!("{s} in {argv:?}"))
    };
    assert_eq!(
        Path::new(&argv[pos("--prefix") + 1]),
        std::fs::canonicalize(&dir).unwrap()
    );
    assert_eq!(argv[pos("--registry") + 1], "https://registry.npmjs.org/");
    // 빈 설정 파일 둘(끝나면 지운다 — 남아 있지 않다).
    for flag in ["--userconfig", "--globalconfig"] {
        assert!(
            argv[pos(flag) + 1].starts_with(
                std::fs::canonicalize(&dir)
                    .unwrap()
                    .join(".pitwall-npmrc")
                    .to_str()
                    .unwrap()
            ),
            "{flag}"
        );
        assert!(!Path::new(&argv[pos(flag) + 1]).exists(), "{flag}");
    }
    assert_ne!(
        argv[pos("--globalconfig") + 1],
        argv[pos("--userconfig") + 1]
    );
    for flag in ["--ignore-scripts", "--no-audit", "--no-fund"] {
        assert!(pos(flag) < pos("--"), "{flag}");
    }
    assert_eq!(
        &argv[pos("--") + 1..],
        ["demo-ls@1.0.0", "typescript@5.0.0"]
    );
    assert_isolated(&rt, &argv, &env, &cwd, &dir);
    let record = fx.layout().read_record("demo-ls").unwrap().unwrap();
    let entry = record.current_entry().unwrap();
    assert_eq!(entry.bins["demo-ls"], "node_modules/.bin/demo-ls");
    assert_eq!(entry.node.as_ref().unwrap().path, rt.dir.join("node"));

    // integrity 가 다르면 실패(폴더 · 기록이 남지 않는다).
    let fx = Fx::new().await;
    runtimes(
        &fx,
        "demo-ls",
        &lock_json(&[
            ("demo-ls", "1.0.0", "sha512-EVIL"),
            ("typescript", "5.0.0", INTEGRITY),
        ]),
    );
    let reg = registry(
        vec![npm_pkg("demo-ls", "1.0.0", &["typescript@5.0.0"])],
        &npm_rows(INTEGRITY),
    );
    let installer = fx.installer(reg, Limits::default());
    fx.consent(&installer, "demo-ls");
    let err = installer
        .install(&project_grant("p1"), "demo-ls", None, None, false)
        .await
        .unwrap_err();
    assert_eq!(
        code_and_stage(&err),
        (code::INSTALL_FAILED, Some("checksum".into()))
    );
    assert!(!fx.layout().version_dir("demo-ls", "1.0.0").exists());
    assert!(fx.layout().read_record("demo-ls").unwrap().is_none());
}

fn pypi_rows() -> Vec<Row> {
    vec![
        Row {
            package: "demo-py".into(),
            version: "1.0.0".into(),
            target: "any".into(),
            name: "demo_py-1.0.0-py3-none-any.whl".into(),
            url: "https://files.pythonhosted.org/demo_py-1.0.0-py3-none-any.whl".into(),
            sha256: "a".repeat(64),
            size: 10,
            dist: Some(("demo-py".into(), "1.0.0".into())),
            integrity: None,
        },
        Row {
            package: "demo-py".into(),
            version: "1.0.0".into(),
            target: target().as_str().into(),
            name: "nodejs_wheel_binaries-24.19.0-x.whl".into(),
            url: "https://files.pythonhosted.org/nodejs_wheel_binaries-24.19.0-x.whl".into(),
            sha256: "b".repeat(64),
            size: 20,
            dist: Some(("nodejs-wheel-binaries".into(), "24.19.0".into())),
            integrity: None,
        },
    ]
}

/// pip: python 은 `-I -m venv`, pip 은 `--isolated` · `--only-binary=:all:` · 고정 `--index-url` · `--require-hashes -r <해시 파일>`
/// (의존 휠 포함 — 다른 대상의 휠은 넣지 않는다) · `--`, cwd 는 설치 폴더, 환경은 격리. venv 를 만든 python 의 실제 경로 · 판이
/// 기록되고, 그 python 이 사라지면 설치본 찾기가 「다시 설치」 로 본다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pip_runs_isolated_with_hashes_and_records_the_python() {
    let fx = Fx::new().await;
    let rt = runtimes(&fx, "demo-py", "{}");
    let mut rows = pypi_rows();
    // 다른 대상의 휠 — 해시 파일에 들어가면 안 된다.
    let other = ToolTarget::ALL.iter().find(|t| **t != target()).unwrap();
    rows.push(Row {
        target: other.as_str().into(),
        name: "nodejs_wheel_binaries-24.19.0-other.whl".into(),
        sha256: "c".repeat(64),
        ..rows[1].clone()
    });
    let installer = fx.installer(
        registry(vec![pypi_pkg("demo-py", "1.0.0")], &rows),
        Limits::default(),
    );
    fx.consent(&installer, "demo-py");
    let result = installer
        .install(&project_grant("p1"), "demo-py", None, None, false)
        .await
        .unwrap();
    assert_eq!(result.checksum, ToolChecksum::Pinned);
    let dir = fx.layout().version_dir("demo-py", "1.0.0");
    let venv_argv = read_lines(&rt.log.with_extension("venv.argv"));
    assert_eq!(
        venv_argv,
        [
            "-I",
            "-m",
            "venv",
            std::fs::canonicalize(&dir)
                .unwrap()
                .join("venv")
                .to_str()
                .unwrap()
        ]
    );
    let argv = read_lines(&rt.log.with_extension("pip.argv"));
    let env = read_lines(&rt.log.with_extension("pip.env"));
    let cwd = read_lines(&rt.log.with_extension("pip.cwd")).remove(0);
    let pos = |s: &str| {
        argv.iter()
            .position(|a| a == s)
            .unwrap_or_else(|| panic!("{s} in {argv:?}"))
    };
    assert_eq!(&argv[..4], ["-I", "-m", "pip", "install"]);
    for flag in [
        "--isolated",
        "--only-binary=:all:",
        "--require-hashes",
        "--no-input",
    ] {
        assert!(pos(flag) < pos("--"), "{flag}");
    }
    assert_eq!(argv[pos("--index-url") + 1], "https://pypi.org/simple");
    assert_eq!(
        argv.last().unwrap(),
        "--",
        "specs come from the hash file, nothing after --"
    );
    let req = std::fs::read_to_string(rt.log.with_extension("pip.req")).unwrap();
    assert_eq!(
        req,
        format!(
            "demo-py==1.0.0 --hash=sha256:{}\nnodejs-wheel-binaries==24.19.0 --hash=sha256:{}\n",
            "a".repeat(64),
            "b".repeat(64)
        )
    );
    assert_isolated(&rt, &argv, &env, &cwd, &dir);
    let entry = fx
        .layout()
        .read_record("demo-py")
        .unwrap()
        .unwrap()
        .current_entry()
        .unwrap()
        .clone();
    assert_eq!(entry.bins["demo-py"], "venv/bin/demo-py");
    let python = entry.python.unwrap();
    assert_eq!(
        python.path,
        std::fs::canonicalize(rt.dir.join("python3")).unwrap()
    );
    assert_eq!(python.version.as_deref(), Some("3.12.1"));
    assert!(!dir.join(".pitwall-requirements.txt").exists());

    // venv 를 만든 python 이 사라지면 「다시 설치 필요」.
    let locator = installer.locator().unwrap();
    assert!(matches!(
        pw_lsp_servers::ToolLocator::locate(&locator, "demo-py", "demo-py"),
        pw_lsp_servers::Located::Installed { .. }
    ));
    std::fs::remove_file(rt.dir.join("python3")).unwrap();
    assert!(matches!(
        pw_lsp_servers::ToolLocator::locate(&locator, "demo-py", "demo-py"),
        pw_lsp_servers::Located::Broken { .. }
    ));
}

/// 고정 해시가 없는 pypi 패키지는 명세를 `--` 뒤에 두고 `--require-hashes` 없이 부르며 「체크섬 없음」 으로 기록한다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pip_without_pinned_hashes_puts_the_spec_after_the_separator() {
    let fx = Fx::new().await;
    let rt = runtimes(&fx, "demo-py", "{}");
    let installer = fx.installer(
        registry(vec![pypi_pkg("demo-py", "1.0.0")], &[]),
        Limits::default(),
    );
    fx.consent(&installer, "demo-py");
    let result = installer
        .install(&project_grant("p1"), "demo-py", None, None, false)
        .await
        .unwrap();
    assert_eq!(result.checksum, ToolChecksum::None);
    let argv = read_lines(&rt.log.with_extension("pip.argv"));
    assert!(!argv.iter().any(|a| a == "--require-hashes" || a == "-r"));
    let sep = argv.iter().position(|a| a == "--").unwrap();
    assert_eq!(&argv[sep + 1..], ["demo-py==1.0.0"]);
}

/// ⑨ node 가 없는 런타임 환경에서 npm 패키지 설치는 「node 필요」(4002), python 이 없으면 「python 필요」 — 아무것도 띄우거나 받지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn without_node_or_python_the_install_says_what_is_needed() {
    let fx = Fx::new().await;
    *fx.chains.project.lock().unwrap() = env_with_path(&[Path::new("/nonexistent-pitwall-bin")]);
    *fx.chains.login.lock().unwrap() = env_with_path(&[Path::new("/nonexistent-pitwall-bin")]);
    let reg = registry(
        vec![
            npm_pkg("demo-ls", "1.0.0", &[]),
            pypi_pkg("demo-py", "1.0.0"),
        ],
        &[],
    );
    let installer = fx.installer(reg, Limits::default());
    for (name, runtime) in [("demo-ls", "node"), ("demo-py", "python")] {
        fx.consent(&installer, name);
        for grant in [project_grant("p1"), settings_grant()] {
            let err = installer
                .install(&grant, name, None, None, false)
                .await
                .unwrap_err();
            assert_eq!(err.code, code::RUNTIME_MISSING, "{name}");
            assert_eq!(err.data.as_ref().unwrap()["runtime"], runtime);
            assert!(
                err.message.contains(runtime) && err.message.contains("mise"),
                "{}",
                err.message
            );
        }
        assert!(!fx.layout().package_dir(name).exists());
    }
    assert!(installer.running().is_empty());
}
