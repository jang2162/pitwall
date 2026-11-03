//! 서버를 **돌리는** 런타임 찾기(정의의 `requires` — 15-m4-plan.md 결정 M4-D6). Pitwall 은 런타임을 설치하지 않는다(D26).
//!
//! 순서:
//!
//! 1. **그 디렉터리의 런타임 환경**(mise · 로그인 셸)의 PATH 에 있는 실행 파일(`node` · `python3`/`python` · `java`)의 판이 범위에
//!    맞으면 그것. 판은 실행해서 읽는다(`--version`, java 는 `-version`) — 이 길은 신뢰한 프로젝트에서 서버를 띄울 때만 돈다(허가 뒤).
//! 2. 없으면 **런타임 제공자가 이미 설치해 둔 판**: mise 의 설치 폴더(`$MISE_DATA_DIR` · `$XDG_DATA_HOME/mise` ·
//!    `~/.local/share/mise` 의 `installs/<도구>/<판>`), macOS 는 `/usr/libexec/java_home -V` 의 JDK 목록. 맞는 것 중 가장 높은 판.
//! 3. 그래도 없으면 `None` — 서버를 띄우지 않고 상태 `runtimeMissing`(안내 문구는 M4-17).
//!
//! 프로젝트가 **쓰는** 런타임과 따로다 — `java@17` 을 고정한 프로젝트에서도 서버는 기기에 설치된 JDK 21 로 돈다.
//!
//! macOS 의 `/usr/bin/java` · `/usr/bin/python3` 는 JDK · 명령줄 도구가 없으면 설치 창을 띄우는 껍데기라 실행하지 않는다(1 단계에서
//! 건너뛰고 2 단계의 `java_home` 이 맡는다).

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use pw_ext::{LspServerRuntime, RuntimeRequirement};
use pw_lsp_servers::{FoundRuntime, ProjectJdk};
use pw_runtime::RuntimeEnv;
use pw_runtime::program::find_program;
use semver::Version;

/// 판을 읽으려고 실행 파일을 돌리는 시한.
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// `requires` 하나를 결정 M4-D6 의 순서로 찾는다. 블로킹이다(실행 파일을 돌린다) — 블로킹 스레드에서 부른다.
pub fn find_runtime(req: &RuntimeRequirement, env: &RuntimeEnv) -> Option<FoundRuntime> {
    from_path(req, env)
        .or_else(|| from_mise(req, env))
        .or_else(|| from_java_home(req))
}

/// 프로젝트가 **쓰는** JDK(결정 M4-D6 · 15-m4-plan.md M4-18 — 정의의 `project_jdk_setting`). 그 디렉터리의 런타임 환경
/// (`ServerSetup::project_env` — 정의 · 설정의 `env` 를 덮기 전)에서만 찾는다: `JAVA_HOME` 의 `bin/java`, 없으면 PATH 의 `java`
/// (macOS 의 `/usr/bin/java` 껍데기는 건너뛴다). 판은 실행해서 읽는다 — 신뢰한 프로젝트에서 서버를 띄울 때만 돈다(허가 뒤).
/// 설치해 둔 JDK 목록(mise · `java_home`)은 보지 않는다 — 프로젝트가 고른 JDK 가 아니므로. 블로킹이다.
pub fn find_project_jdk(env: &RuntimeEnv) -> Option<ProjectJdk> {
    let from_home = env
        .get("JAVA_HOME")
        .filter(|v| !v.is_empty())
        .map(|home| {
            PathBuf::from(home)
                .join("bin")
                .join(if cfg!(windows) { "java.exe" } else { "java" })
        })
        .filter(|exe| exe.is_file());
    let candidates = from_home
        .into_iter()
        .chain(find_program("java", &env.path))
        .filter(|exe| !(cfg!(target_os = "macos") && exe.parent() == Some(Path::new("/usr/bin"))));
    for exe in candidates {
        let Some(version) = probe_version(LspServerRuntime::Java, &exe, env) else {
            continue;
        };
        // `1.8.0_292` 는 8 이다.
        let major = if version.major == 1 {
            version.minor
        } else {
            version.major
        };
        let real = std::fs::canonicalize(&exe).unwrap_or(exe);
        let bin = real.parent()?;
        let home = bin.parent().unwrap_or(bin).to_path_buf();
        return Some(ProjectJdk { home, major });
    }
    None
}

/// 런타임의 실행 파일 이름(찾는 순서).
fn executables(runtime: LspServerRuntime) -> &'static [&'static str] {
    match runtime {
        LspServerRuntime::Node => &["node"],
        LspServerRuntime::Python => &["python3", "python"],
        LspServerRuntime::Java => &["java"],
    }
}

/// 1 단계 — 런타임 환경의 PATH.
fn from_path(req: &RuntimeRequirement, env: &RuntimeEnv) -> Option<FoundRuntime> {
    for name in executables(req.runtime) {
        let Some(exe) = find_program(name, &env.path) else {
            continue;
        };
        if cfg!(target_os = "macos") && exe.parent() == Some(Path::new("/usr/bin")) {
            continue;
        }
        let Some(version) = probe_version(req.runtime, &exe, env) else {
            continue;
        };
        if !req.version.matches(&version) {
            tracing::debug!(target: "pw_lsp", exe = %exe.display(), %version, need = %req.version, "runtime on PATH does not fit");
            continue;
        }
        // 링크를 풀어 실제 설치 폴더를 고른다(mise 의 shim · 링크 등) — `JAVA_HOME` 은 실제 JDK 여야 한다.
        let real = std::fs::canonicalize(&exe).unwrap_or(exe);
        let bin = real.parent()?.to_path_buf();
        let home = bin.parent().map_or_else(|| bin.clone(), Path::to_path_buf);
        return Some(FoundRuntime {
            runtime: req.runtime,
            home,
            bin,
        });
    }
    None
}

/// 2 단계 — mise 의 설치 폴더.
fn from_mise(req: &RuntimeRequirement, env: &RuntimeEnv) -> Option<FoundRuntime> {
    let installs = mise_data_dir(env)?.join("installs").join(req.runtime.key());
    let mut found: Vec<(Version, PathBuf)> = std::fs::read_dir(&installs)
        .ok()?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name();
            let version = parse_version(name.to_str()?)?;
            let path = entry.path();
            // 「latest」 같은 링크는 같은 판을 두 번 셀 뿐이라 건너뛴다.
            if entry.file_type().ok()?.is_symlink() {
                return None;
            }
            Some((version, path))
        })
        .filter(|(version, _)| req.version.matches(version))
        .collect();
    found.sort_by(|a, b| b.0.cmp(&a.0));
    found.into_iter().find_map(|(_, dir)| {
        let exe_name = executables(req.runtime)[0];
        // macOS 의 JDK 는 `<판>/Contents/Home/bin` 에 있을 수 있다.
        [dir.join("bin"), dir.join("Contents/Home/bin")]
            .into_iter()
            .find(|bin| {
                bin.join(exe_name).is_file() || bin.join(format!("{exe_name}.exe")).is_file()
            })
            .map(|bin| FoundRuntime {
                runtime: req.runtime,
                home: bin.parent().map_or_else(|| bin.clone(), Path::to_path_buf),
                bin,
            })
    })
}

/// mise 의 데이터 폴더(런타임 환경의 변수로 — 앱 프로세스의 환경이 아니라).
fn mise_data_dir(env: &RuntimeEnv) -> Option<PathBuf> {
    let var = |name: &str| env.get(name).filter(|v| !v.is_empty()).map(PathBuf::from);
    var("MISE_DATA_DIR")
        .or_else(|| var("XDG_DATA_HOME").map(|d| d.join("mise")))
        .or_else(|| var("HOME").map(|h| h.join(".local/share/mise")))
}

/// 2 단계(macOS) — `/usr/libexec/java_home -V` 의 JDK 목록.
fn from_java_home(req: &RuntimeRequirement) -> Option<FoundRuntime> {
    if req.runtime != LspServerRuntime::Java || !cfg!(target_os = "macos") {
        return None;
    }
    let tool = Path::new("/usr/libexec/java_home");
    if !tool.is_file() {
        return None;
    }
    let output = run(tool, &[OsStr::new("-V")], None)?;
    let mut found: Vec<(Version, PathBuf)> = parse_java_home_list(&output)
        .into_iter()
        .filter(|(version, _)| req.version.matches(version))
        .collect();
    found.sort_by(|a, b| b.0.cmp(&a.0));
    let (_, home) = found.into_iter().next()?;
    let bin = home.join("bin");
    bin.join("java").is_file().then_some(FoundRuntime {
        runtime: req.runtime,
        home,
        bin,
    })
}

/// `java_home -V` 출력(stderr)의 한 줄: `    21.0.5 (arm64) "Eclipse Adoptium" - "OpenJDK 21.0.5" /Library/Java/…/Contents/Home`.
fn parse_java_home_list(output: &str) -> Vec<(Version, PathBuf)> {
    output
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let version = parse_version(line.split_whitespace().next()?)?;
            let home = line.rfind(" /").map(|at| PathBuf::from(&line[at + 1..]))?;
            Some((version, home))
        })
        .collect()
}

/// 실행 파일을 돌려 판을 읽는다.
fn probe_version(runtime: LspServerRuntime, exe: &Path, env: &RuntimeEnv) -> Option<Version> {
    let arg = match runtime {
        LspServerRuntime::Java => "-version",
        LspServerRuntime::Node | LspServerRuntime::Python => "--version",
    };
    let output = run(exe, &[OsStr::new(arg)], Some(env))?;
    let line = output.lines().find(|l| !l.trim().is_empty())?;
    // `v22.11.0` · `Python 3.12.1` · `openjdk version "21.0.5" 2024-10-15` · `java version "1.8.0_292"`.
    let token = match runtime {
        LspServerRuntime::Java => line.split('"').nth(1)?,
        LspServerRuntime::Node => line.trim(),
        LspServerRuntime::Python => line.split_whitespace().nth(1)?,
    };
    parse_version(token)
}

/// 판 글에서 첫 숫자 덩이(`21.0.13` · `v22` · `temurin-21.0.13+9.0.LTS` · `1.8.0_292`)를 semver 로. 빠진 마디는 0.
pub(crate) fn parse_version(text: &str) -> Option<Version> {
    let start = text.find(|c: char| c.is_ascii_digit())?;
    let digits: String = text[start..]
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    let mut parts = digits
        .split('.')
        .filter(|p| !p.is_empty())
        .map(|p| p.parse::<u64>().ok());
    let major = parts.next()??;
    let minor = parts.next().flatten().unwrap_or(0);
    let patch = parts.next().flatten().unwrap_or(0);
    Some(Version::new(major, minor, patch))
}

/// 실행해 stdout · stderr 를 합친 글. 시한을 넘기거나 띄우지 못하면 `None`. `env` 가 있으면 그 환경만 준다.
fn run(exe: &Path, args: &[&OsStr], env: Option<&RuntimeEnv>) -> Option<String> {
    let mut command = Command::new(exe);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(env) = env {
        command
            .env_clear()
            .env("PATH", &env.path)
            .envs(env.vars.iter().map(|(k, v)| (k.as_os_str(), v.as_os_str())));
    }
    let mut child = command.spawn().ok()?;
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() < PROBE_TIMEOUT => {
                std::thread::sleep(Duration::from_millis(10));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                tracing::debug!(target: "pw_lsp", exe = %exe.display(), "runtime version probe timed out");
                return None;
            }
        }
    }
    let output = child.wait_with_output().ok()?;
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push('\n');
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    Some(text)
}

/// 안내에 쓰는 이름(`runtimeMissing { tool }`).
pub fn runtime_name(runtime: LspServerRuntime) -> &'static str {
    runtime.key()
}

#[cfg(all(test, unix))]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    use semver::VersionReq;

    use super::*;

    fn script(path: &Path, body: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn req(runtime: LspServerRuntime, range: &str) -> RuntimeRequirement {
        RuntimeRequirement {
            runtime,
            version: VersionReq::parse(range).unwrap(),
        }
    }

    fn env(path: &[&Path], vars: &[(&str, &Path)]) -> RuntimeEnv {
        let mut env = RuntimeEnv::default();
        env.path = std::env::join_paths(path.iter().map(|p| p.to_path_buf())).unwrap();
        for (name, value) in vars {
            env.vars
                .insert((*name).into(), value.as_os_str().to_owned());
        }
        env
    }

    #[test]
    fn versions_are_read_from_the_usual_shapes() {
        for (text, want) in [
            ("v22.11.0", (22, 11, 0)),
            ("21", (21, 0, 0)),
            ("temurin-21.0.13+9.0.LTS", (21, 0, 13)),
            ("1.8.0_292", (1, 8, 0)),
            ("corretto-17.0.2.8.1", (17, 0, 2)),
        ] {
            assert_eq!(
                parse_version(text),
                Some(Version::new(want.0, want.1, want.2)),
                "{text}"
            );
        }
        assert_eq!(parse_version("latest"), None);
        let list = "Matching Java Virtual Machines (2):\n    21.0.5 (arm64) \"Eclipse Adoptium\" - \"OpenJDK 21.0.5\" /Library/Java/JavaVirtualMachines/temurin-21.jdk/Contents/Home\n    17.0.2 (arm64) \"Oracle\" - \"OpenJDK 17\" /Library/Java/JavaVirtualMachines/jdk-17.jdk/Contents/Home\n";
        assert_eq!(
            parse_java_home_list(list),
            vec![
                (
                    Version::new(21, 0, 5),
                    PathBuf::from("/Library/Java/JavaVirtualMachines/temurin-21.jdk/Contents/Home")
                ),
                (
                    Version::new(17, 0, 2),
                    PathBuf::from("/Library/Java/JavaVirtualMachines/jdk-17.jdk/Contents/Home")
                ),
            ]
        );
    }

    /// 결정 M4-D6: 런타임 환경의 PATH 에 맞는 판이 있으면 그것, 없으면(프로젝트가 17 을 고정) 제공자가 설치해 둔 21, 그것도 없으면 없음.
    #[test]
    fn the_environment_first_then_installed_versions_then_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let base = fs::canonicalize(tmp.path()).unwrap();
        let project_jdk = base.join("jdk17/bin");
        script(
            &project_jdk.join("java"),
            "echo 'openjdk version \"17.0.2\" 2022-01-18' >&2",
        );
        let server_jdk = base.join("mise/installs/java/temurin-21.0.5+11/bin");
        script(
            &server_jdk.join("java"),
            "echo 'openjdk version \"21.0.5\"' >&2",
        );
        // 범위 밖 판 · 이름이 판이 아닌 폴더는 고르지 않는다.
        script(
            &base.join("mise/installs/java/temurin-11.0.1/bin/java"),
            "exit 0",
        );
        fs::create_dir_all(base.join("mise/installs/java/latest-broken")).unwrap();
        let java21 = req(LspServerRuntime::Java, ">=21");

        // 프로젝트 JDK 17 이 PATH 에 있고 mise 설치 폴더에 21 이 있다 → 21.
        let e = env(
            &[&project_jdk, Path::new("/bin")],
            &[("MISE_DATA_DIR", &base.join("mise"))],
        );
        let found = find_runtime(&java21, &e).unwrap();
        assert_eq!(found.bin, server_jdk);
        assert_eq!(found.home, server_jdk.parent().unwrap());

        // PATH 의 것이 맞으면 그것.
        let e = env(&[&server_jdk, &project_jdk], &[]);
        assert_eq!(find_runtime(&java21, &e).unwrap().bin, server_jdk);

        // 어디에도 없으면 없음(runtimeMissing).
        let e = env(
            &[&project_jdk, Path::new("/bin")],
            &[("HOME", &base.join("nohome"))],
        );
        if !cfg!(target_os = "macos") {
            assert_eq!(find_runtime(&java21, &e), None);
        } else {
            // macOS 는 java_home 이 기기의 JDK 를 고를 수 있다 — 고른다면 범위 안이어야 한다.
            if let Some(found) = find_runtime(&java21, &e) {
                assert_ne!(found.bin, project_jdk);
            }
        }

        // node: `v20.x` 는 `>=20` 에 맞고 `>=22` 에는 아니다.
        let node = base.join("node/bin");
        script(&node.join("node"), "echo v20.11.1");
        let e = env(&[&node], &[("HOME", &base.join("nohome"))]);
        assert_eq!(
            find_runtime(&req(LspServerRuntime::Node, ">=20"), &e)
                .unwrap()
                .bin,
            node
        );
        assert_eq!(find_runtime(&req(LspServerRuntime::Node, ">=22"), &e), None);
    }

    /// 프로젝트 JDK 는 그 디렉터리 런타임 환경의 `JAVA_HOME`, 없으면 PATH 의 `java` 에서만 찾는다(링크를 푼 설치 폴더, 1.8 은 8) —
    /// 설치해 둔 JDK(mise)는 프로젝트가 고른 것이 아니라 보지 않는다(15-m4-plan.md M4-18).
    #[test]
    fn the_project_jdk_comes_from_java_home_then_path_only() {
        let tmp = tempfile::tempdir().unwrap();
        let base = fs::canonicalize(tmp.path()).unwrap();
        let jdk17 = base.join("jdk17");
        script(
            &jdk17.join("bin/java"),
            "echo 'openjdk version \"17.0.2\" 2022-01-18' >&2",
        );
        let jdk8 = base.join("jdk8");
        script(
            &jdk8.join("bin/java"),
            "echo 'java version \"1.8.0_292\"' >&2",
        );
        let shims = base.join("shims");
        fs::create_dir_all(&shims).unwrap();
        std::os::unix::fs::symlink(jdk17.join("bin/java"), shims.join("java")).unwrap();
        script(
            &base.join("mise/installs/java/25.0.1/bin/java"),
            "echo 'openjdk version \"25.0.1\"' >&2",
        );

        // PATH 의 링크 → 실제 설치 폴더.
        let e = env(
            &[&shims, Path::new("/bin")],
            &[("MISE_DATA_DIR", &base.join("mise"))],
        );
        assert_eq!(
            find_project_jdk(&e),
            Some(ProjectJdk {
                home: jdk17.clone(),
                major: 17
            })
        );
        // JAVA_HOME 이 PATH 보다 먼저.
        let e = env(&[&shims], &[("JAVA_HOME", &jdk8)]);
        assert_eq!(
            find_project_jdk(&e),
            Some(ProjectJdk {
                home: jdk8.clone(),
                major: 8
            })
        );
        // 런타임 환경에 java 가 없으면 설치해 둔 판(mise)이 있어도 없음.
        let e = env(
            &[Path::new("/nonexistent")],
            &[("MISE_DATA_DIR", &base.join("mise"))],
        );
        assert_eq!(find_project_jdk(&e), None);
    }
}
