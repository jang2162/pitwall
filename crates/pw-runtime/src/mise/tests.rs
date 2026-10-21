//! mise 제공자. 가짜 `mise`(고정한 mise 두 버전의 픽스처 `fixtures/mise/` 를 그대로 내보내는 sh 스크립트)를 바탕 환경의 PATH 에
//! 두고 감지 · 실행 · 출력 해석 · 실패 갈래를 본다.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::*;

const VERSIONS: [&str; 2] = ["2025.1.0", "2026.4.3"];

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/mise")
}

/// `dir/mise` 에 sh 스크립트를 쓴다.
pub(crate) fn write_mise(dir: &Path, body: &str) -> PathBuf {
    fs::create_dir_all(dir).unwrap();
    let path = dir.join("mise");
    fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}

/// 픽스처 하나(`<판>/<경우>`)를 그대로 내보내는 가짜 mise. 부를 때마다 `log` 에 `cwd|인자|BASE_ONLY` 를 한 줄 적는다.
fn replaying(dir: &Path, version: &str, case: &str, log: &Path) -> PathBuf {
    let base = fixtures().join(version).join(case);
    let base = base.display();
    write_mise(
        dir,
        &format!(
            "printf '%s|%s|%s\\n' \"$PWD\" \"$*\" \"$BASE_ONLY\" >> '{}'\ncat '{base}.stdout'\ncat '{base}.stderr' >&2\nexit \"$(cat '{base}.exit')\"",
            log.display()
        ),
    )
}

struct Fixture {
    _tmp: tempfile::TempDir,
    /// 임시 폴더의 실제 경로(macOS 의 `/var` → `/private/var`) — mise 가 받는 `$PWD` 와 견준다.
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        Self { _tmp: tmp, root }
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.root.join(rel)
    }

    /// 로그인 셸 환경 흉내: PATH 에 `bin`(가짜 mise 자리)과 시스템 폴더, 홈은 `home`.
    fn base(&self) -> RuntimeEnv {
        let mut env = RuntimeEnv {
            path: std::env::join_paths([self.path("bin"), "/usr/bin".into(), "/bin".into()])
                .unwrap(),
            ..RuntimeEnv::default()
        };
        env.vars
            .insert("HOME".into(), self.path("home").into_os_string());
        env.vars.insert("BASE_ONLY".into(), "kept".into());
        env.vars.insert("GREETING".into(), "from the shell".into());
        env
    }

    fn project(&self, files: &[(&str, &str)]) -> PathBuf {
        let dir = self.path("home/project");
        fs::create_dir_all(&dir).unwrap();
        for (name, body) in files {
            let path = dir.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, body).unwrap();
        }
        dir
    }

    fn log(&self) -> Vec<String> {
        fs::read_to_string(self.path("calls.log"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }
}

/// 고정한 두 판의 신뢰된 출력: mise 의 변수가 로그인 셸 환경 위에 덮이고(같은 이름은 mise 의 것), PATH 는 mise 가 준 전체 값,
/// 여러 줄 · 비ASCII 값도 그대로다. mise 는 그 디렉터리에서 `env --json` 으로 불리고 로그인 셸 환경을 받는다.
#[test]
fn trusted_output_of_both_versions_overlays_the_login_shell_environment() {
    for version in VERSIONS {
        let f = Fixture::new();
        let dir = f.project(&[("mise.toml", "[tools]\nnode = \"22\"\n")]);
        replaying(&f.path("bin"), version, "trusted", &f.path("calls.log"));
        let base = f.base();
        assert!(Mise::default().applies_to(&dir, &base), "{version}");
        let layer = Mise::default().resolve(&dir, &base).unwrap();
        assert_eq!(
            layer.env.path,
            OsString::from(
                "/sandbox/proj/bin:/sandbox/data/installs/node/22.23.3/bin:/usr/bin:/bin:/usr/sbin:/sbin"
            ),
            "{version}"
        );
        let get = |name| layer.env.get(name).map(|v| v.to_str().unwrap().to_owned());
        assert_eq!(get("GREETING").as_deref(), Some("hello world"), "{version}");
        assert_eq!(
            get("MULTILINE").as_deref(),
            Some("first\nsecond"),
            "{version}"
        );
        assert_eq!(get("UNICODE").as_deref(), Some("한국어 ✓"), "{version}");
        assert_eq!(get("BASE_ONLY").as_deref(), Some("kept"), "{version}");
        assert_eq!(
            layer.source,
            RuntimeSource::ConfigFiles {
                files: vec![dir.join("mise.toml").display().to_string()]
            }
        );
        assert_eq!(
            f.log(),
            [format!("{}|env --json|kept", dir.display())],
            "{version}"
        );
    }
}

/// 미신뢰: 2025.1.0 은 끝 코드 0 에 설정을 뺀 JSON, 2026.4.3 은 끝 코드 1 — 둘 다 「mise 가 신뢰하지 않음」 이다.
#[test]
fn untrusted_config_is_not_trusted_in_both_versions() {
    for version in VERSIONS {
        let f = Fixture::new();
        let dir = f.project(&[("mise.toml", "[tools]\nnode = \"22\"\n")]);
        replaying(&f.path("bin"), version, "untrusted", &f.path("calls.log"));
        let err = Mise::default().resolve(&dir, &f.base()).unwrap_err();
        assert_eq!(
            err.problem,
            RuntimeProblem::NotTrusted,
            "{version}: {err:?}"
        );
        assert!(
            err.reason.contains("mise trust"),
            "{version}: {}",
            err.reason
        );
    }
}

#[test]
fn a_broken_config_fails_in_both_versions() {
    for version in VERSIONS {
        let f = Fixture::new();
        let dir = f.project(&[("mise.toml", "[tools\n")]);
        replaying(&f.path("bin"), version, "broken", &f.path("calls.log"));
        let err = Mise::default().resolve(&dir, &f.base()).unwrap_err();
        assert_eq!(err.problem, RuntimeProblem::Failed, "{version}: {err:?}");
        assert!(
            err.reason.contains("TOML parse error"),
            "{version}: {}",
            err.reason
        );
    }
}

/// 설정 파일이 있는데 PATH 에 mise 가 없으면 쓰되(경고를 내야 하므로) 「설치되지 않음」 으로 실패한다.
#[test]
fn a_config_without_mise_on_the_path_is_not_installed() {
    let f = Fixture::new();
    let dir = f.project(&[(".mise.toml", "")]);
    let base = f.base();
    assert!(Mise::default().applies_to(&dir, &base));
    let err = Mise::default().resolve(&dir, &base).unwrap_err();
    assert_eq!(err.problem, RuntimeProblem::NotInstalled);
}

/// `.tool-versions` 만이면 asdf 의 파일일 수 있다 — mise 가 PATH 에 있을 때만 쓴다.
#[test]
fn tool_versions_alone_needs_mise_on_the_path() {
    let f = Fixture::new();
    let dir = f.project(&[(".tool-versions", "nodejs 22.11.0\n")]);
    assert!(!Mise::default().applies_to(&dir, &f.base()));
    replaying(&f.path("bin"), "2026.4.3", "trusted", &f.path("calls.log"));
    assert!(Mise::default().applies_to(&dir, &f.base()));
    assert!(f.log().is_empty(), "detection must not run mise");
}

/// 설정은 위 폴더에서도 찾지만 홈 폴더(`~/.config/mise` 는 전역 설정)에서 멈춘다.
#[test]
fn config_files_are_found_upwards_until_home() {
    let f = Fixture::new();
    let home = f.path("home");
    let nested = f.project(&[("mise.toml", "")]).join("packages/app");
    fs::create_dir_all(&nested).unwrap();
    fs::create_dir_all(home.join(".config/mise")).unwrap();
    fs::write(home.join(".config/mise/config.toml"), "").unwrap();
    fs::write(nested.join(".tool-versions"), "").unwrap();
    fs::create_dir_all(nested.join(".config/mise/conf.d")).unwrap();
    fs::write(nested.join(".config/mise/conf.d/b.toml"), "").unwrap();
    fs::write(nested.join(".config/mise/conf.d/a.toml"), "").unwrap();
    fs::write(nested.join(".config/mise/conf.d/notes.txt"), "").unwrap();
    let (files, specific) = config_files(&nested, Some(&home));
    assert!(specific);
    assert_eq!(
        files,
        [
            nested.join(".config/mise/conf.d/a.toml"),
            nested.join(".config/mise/conf.d/b.toml"),
            nested.join(".tool-versions"),
            f.path("home/project/mise.toml"),
        ]
    );
    // 홈 밖의 폴더는 홈 설정과 무관하게 루트까지 본다.
    let (outside, _) = config_files(&f.path("elsewhere"), Some(&home));
    assert!(outside.is_empty());
    // 홈 아래 프로젝트에 설정이 없으면 전역 설정만으로는 쓰지 않는다.
    let bare = home.join("bare");
    fs::create_dir_all(&bare).unwrap();
    assert!(!Mise::default().applies_to(&bare, &f.base()));
}

#[test]
fn a_hanging_mise_fails_within_its_timeout() {
    let f = Fixture::new();
    let dir = f.project(&[("mise.toml", "")]);
    write_mise(&f.path("bin"), "sleep 30");
    let started = Instant::now();
    let err = Mise::new(Duration::from_millis(300))
        .resolve(&dir, &f.base())
        .unwrap_err();
    assert_eq!(err.problem, RuntimeProblem::Failed);
    assert!(err.reason.contains("did not finish"), "{}", err.reason);
    assert!(started.elapsed() < Duration::from_secs(3));
}

#[test]
fn output_that_is_not_an_env_object_fails() {
    for (body, wanted) in [
        ("echo 'not json'", "not JSON"),
        ("echo '[1]'", "not a JSON object"),
        ("echo '{\"A\": \"1\"}'", "no PATH"),
    ] {
        let f = Fixture::new();
        let dir = f.project(&[("mise.toml", "")]);
        write_mise(&f.path("bin"), body);
        let err = Mise::default().resolve(&dir, &f.base()).unwrap_err();
        assert_eq!(err.problem, RuntimeProblem::Failed);
        assert!(err.reason.contains(wanted), "{body}: {}", err.reason);
    }
}

#[test]
fn runtime_files_are_mise_configs_and_version_files() {
    for yes in [
        "/p/mise.toml",
        "/p/.mise.toml",
        "/p/mise.local.toml",
        "/p/mise.ci.toml",
        "/p/.mise.ci.local.toml",
        "/p/.config/mise/config.toml",
        "/p/.config/mise/conf.d/node.toml",
        "/p/.mise/config.toml",
        "/p/.tool-versions",
        "/p/.nvmrc",
        "/p/sub/.python-version",
    ] {
        assert!(is_runtime_file(Path::new(yes)), "{yes}");
    }
    for no in [
        "/p/Cargo.toml",
        "/p/config.toml",
        "/p/src/mise.rs",
        "/p/.config/other/config.toml",
    ] {
        assert!(!is_runtime_file(Path::new(no)), "{no}");
    }
}
