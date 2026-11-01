//! npm · pip 을 부르는 자리(15-m4-plan.md M4-04 ① · 계획 리뷰 M3 · 결정 M4-D6).
//!
//! npm · python 은 **런타임 환경에서 찾는다** — 설치를 부른 창이 프로젝트 창이면 그 프로젝트의 런타임 환경(mise 등), 설정 창이면
//! 로그인 셸. 없으면 「node 가 필요하다 — mise 로 설치」 오류. 그러나 설치본은 `$DATA/tools` 로 모든 프로젝트가 함께 쓰므로 **그
//! 프로젝트의 환경이 출처를 바꾸지 못하게** 부른다:
//! - cwd 는 설치 폴더(프로젝트의 `.npmrc` · `pip.conf` 를 읽지 않게).
//! - 환경은 로그인 셸의 것을 바탕으로(프로젝트 런타임 환경 — mise `[env]` 등 — 은 쓰지 않는다) PATH 맨 앞에 고른 node · python 의
//!   폴더만 얹고, `NPM_CONFIG_*` · `npm_config_*` · `PIP_*` · `PYTHON*` 를 지운다(대소문자 무시). 앱 프로세스의 환경은 물려주지 않는다.
//! - 출처는 인자로 고정한다: npm `--registry https://registry.npmjs.org/ --userconfig <빈 파일> --globalconfig <빈 파일>`, pip
//!   `--isolated --index-url https://pypi.org/simple`(venv 를 만드는 python 도 `-I`).
//! - 패키지 명세 앞에는 `--` 를 둔다(옵션으로 읽히지 않게 — M4-03 의 이름 검사와 두 겹). 고정 해시가 있는 pypi 패키지는 명세를
//!   해시 파일(`-r`, `--require-hashes`)로 넘기므로 `--` 뒤가 빈다 — 해시 파일의 줄도 검사한 이름으로만 만든다.
//!
//! 이 모듈은 환경 값을 로그 · 오류에 싣지 않는다(`docs/design/03-architecture.md` §5-1) — 프로그램 경로 · 인자 · 끝난 코드만.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Stdio;

use pw_runtime::RuntimeEnv;
use pw_runtime::program::find_program;

/// npm 의 출처(인자로 고정).
pub const NPM_REGISTRY: &str = "https://registry.npmjs.org/";
/// PyPI 의 출처(인자로 고정).
pub const PYPI_INDEX: &str = "https://pypi.org/simple";
/// npm · pip 출력 중 로그에 남기는 바이트.
const MAX_CAPTURE: usize = 256 * 1024;

/// 설치기가 npm · python 을 찾을 환경을 묻는 곳. 앱은 `pw_runtime::Runtime` 으로 답한다([`RuntimeToolchains`]).
pub trait Toolchains: Send + Sync {
    /// 로그인 셸의 환경(설치 프로세스 환경의 바탕). 셸을 읽지 못했으면 앱 프로세스의 환경.
    fn login_shell(&self) -> RuntimeEnv;
    /// npm · python 을 찾을 환경: `project` 의 런타임 환경(주 루트), `None` 이면 로그인 셸.
    fn lookup(&self, project: Option<&str>) -> Result<RuntimeEnv, String>;
}

/// 앱의 런타임 환경으로 답한다.
pub struct RuntimeToolchains {
    pub runtime: std::sync::Arc<pw_runtime::Runtime>,
    pub projects: pw_core::registry::Registry,
}

impl Toolchains for RuntimeToolchains {
    fn login_shell(&self) -> RuntimeEnv {
        self.runtime.login_shell_env().0.env
    }

    fn lookup(&self, project: Option<&str>) -> Result<RuntimeEnv, String> {
        match project {
            None => Ok(self.login_shell()),
            Some(id) => {
                let project = self.projects.get(id).map_err(|e| e.to_string())?;
                let root = std::fs::canonicalize(project.root())
                    .unwrap_or_else(|_| project.root().to_path_buf());
                Ok(self.runtime.env(&project.id, &root).env.clone())
            }
        }
    }
}

/// 이 프로세스의 환경으로 답한다 — 앱이 아닌 곳(예제 `tools_install` · live 시험 · CI)용. 앱은 [`RuntimeToolchains`].
pub struct ProcessToolchains;

impl Toolchains for ProcessToolchains {
    fn login_shell(&self) -> RuntimeEnv {
        RuntimeEnv::process()
    }

    fn lookup(&self, _project: Option<&str>) -> Result<RuntimeEnv, String> {
        Ok(RuntimeEnv::process())
    }
}

/// 찾은 런타임.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    /// 부를 실행 파일(npm 이면 npm, python 이면 python).
    pub program: PathBuf,
    /// PATH 에 얹을 폴더(npm 이면 node 의 폴더와 npm 의 폴더).
    pub dirs: Vec<PathBuf>,
    /// node 또는 python 의 경로.
    pub runtime: PathBuf,
}

/// 런타임이 없다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Missing(pub &'static str);

/// 런타임 환경에서 npm 과 node 를 찾는다.
pub fn find_npm(env: &RuntimeEnv) -> Result<Found, Missing> {
    let node = find_program("node", &env.path).ok_or(Missing("node"))?;
    let npm = find_program("npm", &env.path).ok_or(Missing("node"))?;
    let mut dirs = Vec::new();
    for p in [&node, &npm] {
        if let Some(d) = p.parent()
            && !dirs.iter().any(|x: &PathBuf| x == d)
        {
            dirs.push(d.to_path_buf());
        }
    }
    Ok(Found {
        program: npm,
        dirs,
        runtime: node,
    })
}

/// 런타임 환경에서 python 을 찾는다(`python3` 먼저).
pub fn find_python(env: &RuntimeEnv) -> Result<Found, Missing> {
    let python = find_program("python3", &env.path)
        .or_else(|| find_program("python", &env.path))
        .ok_or(Missing("python"))?;
    let dirs = python.parent().map(Path::to_path_buf).into_iter().collect();
    Ok(Found {
        program: python.clone(),
        dirs,
        runtime: python,
    })
}

/// 지우는 변수인가(대소문자 무시).
pub fn is_stripped(name: &OsStr) -> bool {
    let upper = name.to_string_lossy().to_ascii_uppercase();
    upper.starts_with("NPM_CONFIG_") || upper.starts_with("PIP_") || upper.starts_with("PYTHON")
}

/// 설치 프로세스의 환경: 로그인 셸 바탕 + PATH 맨 앞에 `dirs`, 지우는 변수를 뺀 것.
pub fn install_env(base: &RuntimeEnv, dirs: &[PathBuf]) -> BTreeMap<OsString, OsString> {
    let mut out: BTreeMap<OsString, OsString> = base
        .vars
        .iter()
        .filter(|(k, _)| !is_stripped(k))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let mut path: Vec<PathBuf> = dirs.to_vec();
    for d in std::env::split_paths(&base.path) {
        if !path.contains(&d) {
            path.push(d);
        }
    }
    if let Ok(joined) = std::env::join_paths(path) {
        out.insert(OsString::from("PATH"), joined);
    }
    out
}

/// 프로세스 하나의 결과.
#[derive(Debug, Clone)]
pub struct Ran {
    pub success: bool,
    pub code: Option<i32>,
    /// stdout · stderr(앞에서부터 상한까지).
    pub output: String,
}

/// `program args` 를 `cwd` 에서 `env` 만으로 돌린다. 이 future 가 버려지면(작업 취소) 프로세스를 죽인다.
pub async fn run(
    program: &Path,
    args: &[OsString],
    cwd: &Path,
    env: &BTreeMap<OsString, OsString>,
) -> Result<Ran, String> {
    let mut cmd = tokio::process::Command::new(program);
    cmd.args(args)
        .current_dir(cwd)
        .env_clear()
        .envs(env)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("cannot start {}: {e}", program.display()))?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let (out, err, status) = tokio::join!(read_capped(stdout), read_capped(stderr), child.wait());
    let status = status.map_err(|e| e.to_string())?;
    let mut output = String::from_utf8_lossy(&out).into_owned();
    if !err.is_empty() {
        output.push_str(&String::from_utf8_lossy(&err));
    }
    Ok(Ran {
        success: status.success(),
        code: status.code(),
        output,
    })
}

/// 파이프를 끝까지 읽되 앞에서부터 [`MAX_CAPTURE`] 까지만 남긴다(나머지는 읽어 버린다 — 프로세스가 막히지 않게).
async fn read_capped(pipe: Option<impl tokio::io::AsyncRead + Unpin>) -> Vec<u8> {
    use tokio::io::AsyncReadExt as _;
    let mut buf = Vec::new();
    let Some(mut p) = pipe else {
        return buf;
    };
    let mut chunk = [0u8; 8192];
    loop {
        let n = p.read(&mut chunk).await.unwrap_or(0);
        if n == 0 {
            break;
        }
        if buf.len() < MAX_CAPTURE {
            let take = n.min(MAX_CAPTURE - buf.len());
            buf.extend_from_slice(&chunk[..take]);
        }
    }
    buf
}
