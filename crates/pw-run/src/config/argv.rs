//! 종류마다의 argv. `shell` 만 사람이 쓴 글을 셸에 넘기고(로그인 셸의 `-c`), 그 밖은 필드 하나가 argv 한 칸이다 — 필드를 셸 글로
//! 이어 붙이지 않으므로 인자에 공백 · 따옴표 · `$` 가 있어도 그대로 한 인자다.
//!
//! 프로그램 이름은 맨 이름(`cargo` · `npm`)이고 찾기는 띄우는 쪽(M6-02 — 런타임 환경의 PATH)이 한다. 래퍼(`gradlew` · `mvnw`)만
//! 찾은 파일의 절대 경로다. 디스크를 읽는 판정(lockfile · 래퍼)은 작업 폴더에서 프로젝트 루트까지 올라가며 처음 만나는 것을
//! 쓴다 — 모노레포의 하위 패키지는 lockfile · 래퍼가 위에 있다.

use std::path::{Path, PathBuf};

use super::{RunConfig, RunKindConfig};

/// argv 를 만드는 데 필요한 자리.
#[derive(Debug, Clone, Copy)]
pub struct ArgvContext<'a> {
    /// 프로젝트 루트(활성 워크트리 안). lockfile · 래퍼를 여기까지 찾는다.
    pub root: &'a Path,
    /// 작업 폴더(구성의 `cwd` 를 푼 것 — [`super::root::inside`]).
    pub cwd: &'a Path,
    /// 로그인 셸의 argv(`$SHELL -l` — `crate::shell::default_argv`). `shell` 종류가 뒤에 `-c <command>` 를 붙인다.
    pub login_shell: &'a [String],
}

/// 고른 프로필(`profile`)을 얹은 argv. 없는 프로필이면 오류다. 프로필의 `args` 는 종류의 `args` 를 통째로 바꾼다(env 는 M6-03).
pub fn argv(
    config: &RunConfig,
    profile: Option<&str>,
    cx: &ArgvContext<'_>,
) -> Result<Vec<String>, String> {
    let args_over = match profile {
        None => None,
        Some(name) => config
            .profiles
            .get(name)
            .ok_or_else(|| format!("profile `{name}` does not exist"))?
            .args
            .as_deref(),
    };
    let args = |own: &[String]| args_over.unwrap_or(own).to_vec();
    let mut out: Vec<String> = Vec::new();
    match &config.kind {
        RunKindConfig::Shell { command } => {
            if cx.login_shell.is_empty() {
                return Err("no login shell to run the command with".to_owned());
            }
            out.extend(cx.login_shell.iter().cloned());
            out.push(shell_flag().to_owned());
            out.push(command.clone());
        }
        RunKindConfig::Npm { script } => {
            out.push(package_manager(cx).to_owned());
            out.push("run".to_owned());
            out.push(script.clone());
        }
        RunKindConfig::Gradle { tasks } => {
            out.push(wrapper(cx, GRADLE_WRAPPER).unwrap_or_else(|| "gradle".to_owned()));
            out.extend(tasks.iter().cloned());
        }
        RunKindConfig::Maven { tasks } => {
            out.push(wrapper(cx, MAVEN_WRAPPER).unwrap_or_else(|| "mvn".to_owned()));
            out.extend(tasks.iter().cloned());
        }
        RunKindConfig::Cargo { command, args: own } => {
            out.extend(["cargo".to_owned(), command.clone()]);
            out.extend(args(own));
        }
        RunKindConfig::Go {
            command,
            package,
            args: own,
        } => {
            out.extend(["go".to_owned(), command.clone()]);
            out.extend(package.iter().cloned());
            out.extend(args(own));
        }
        RunKindConfig::Python {
            module,
            script,
            args: own,
        } => {
            out.push(PYTHON.to_owned());
            match (module, script) {
                (Some(module), None) => out.extend(["-m".to_owned(), module.clone()]),
                (None, Some(script)) => out.push(script.clone()),
                _ => {
                    return Err(
                        "a python config needs exactly one of `module` and `script`".to_owned()
                    );
                }
            }
            out.extend(args(own));
        }
        RunKindConfig::DockerCompose { services, file } => {
            out.extend(["docker".to_owned(), "compose".to_owned()]);
            // `file` 은 프로젝트 루트 기준이고 compose 는 `-f` 를 작업 폴더 기준으로 푼다 — 루트 안의 실제 경로로 넘긴다.
            if let Some(file) = file {
                let path =
                    super::root::inside(cx.root, file).map_err(|why| format!("file {why}"))?;
                out.extend(["-f".to_owned(), path.display().to_string()]);
            }
            out.push("up".to_owned());
            out.extend(services.iter().cloned());
        }
        RunKindConfig::MiseTask { task } => {
            out.extend(["mise".to_owned(), "run".to_owned(), task.clone()]);
        }
        // 프로세스를 띄우지 않는다 — DB 실행 경로(`pw_db::run_sql`)로 돈다(M6-09, D55).
        RunKindConfig::Sql { .. } => return Err(NOT_A_PROCESS.to_owned()),
    }
    Ok(out)
}

/// `sql` 구성의 argv 를 청했을 때의 까닭(창의 `run.start` · 에이전트의 프로세스 계획이 받는다).
pub const NOT_A_PROCESS: &str =
    "an `sql` configuration runs no process; it runs on its database connection (`run.sql.start`)";

/// 로그인 셸에 명령 글을 넘기는 옵션.
fn shell_flag() -> &'static str {
    if cfg!(windows) { "-Command" } else { "-c" }
}

/// Python 실행 파일. macOS · 리눅스의 시스템 Python 은 `python3` 만 두는 일이 흔하다(가상 환경 · mise · pyenv 도 `python3` 을 둔다).
const PYTHON: &str = if cfg!(windows) { "python" } else { "python3" };

/// lockfile → 패키지 관리자. 앞의 것이 먼저다(한 폴더에 둘이면 앞의 것).
pub const LOCKFILES: &[(&str, &str)] = &[
    ("pnpm-lock.yaml", "pnpm"),
    ("yarn.lock", "yarn"),
    ("bun.lock", "bun"),
    ("bun.lockb", "bun"),
    ("package-lock.json", "npm"),
    ("npm-shrinkwrap.json", "npm"),
];

const GRADLE_WRAPPER: &[&str] = if cfg!(windows) {
    &["gradlew.bat"]
} else {
    &["gradlew"]
};
const MAVEN_WRAPPER: &[&str] = if cfg!(windows) {
    &["mvnw.cmd"]
} else {
    &["mvnw"]
};

/// 작업 폴더부터 프로젝트 루트까지(둘 다 포함). 작업 폴더가 루트 밖이면 작업 폴더 하나. 둘을 실제 경로로 바꿔 비교한다 — 레지스트리의
/// 루트가 링크를 지난 경로(`/var/…` 대 `/private/var/…`)거나 Windows verbatim 경로가 섞이면 글로는 루트 안이 아니게 보여 lockfile ·
/// 래퍼를 놓친다(리뷰 권고 1).
fn upward(cx: &ArgvContext<'_>) -> Vec<PathBuf> {
    let real = |p: &Path| {
        std::fs::canonicalize(p)
            .map(pw_core::registry::path::simplify)
            .unwrap_or_else(|_| p.to_path_buf())
    };
    let (root, cwd) = (real(cx.root), real(cx.cwd));
    if !cwd.starts_with(&root) {
        return vec![cwd];
    }
    cwd.ancestors()
        .take_while(|dir| dir.starts_with(&root))
        .map(Path::to_path_buf)
        .collect()
}

/// 작업 폴더에서 루트까지 처음 만나는 lockfile 의 패키지 관리자. 없으면 `npm`.
fn package_manager(cx: &ArgvContext<'_>) -> &'static str {
    upward(cx)
        .into_iter()
        .find_map(|dir| {
            LOCKFILES
                .iter()
                .find(|(file, _)| dir.join(file).is_file())
                .map(|(_, pm)| *pm)
        })
        .unwrap_or("npm")
}

/// 작업 폴더에서 루트까지 처음 만나는 래퍼 파일의 절대 경로.
fn wrapper(cx: &ArgvContext<'_>, names: &[&str]) -> Option<String> {
    upward(cx).into_iter().find_map(|dir| {
        names
            .iter()
            .map(|name| dir.join(name))
            .find(|p: &PathBuf| p.is_file())
            .map(|p| p.display().to_string())
    })
}
