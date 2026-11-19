//! 실행 구성의 `[debug]` 에서 띄울 어댑터까지: 어댑터 고르기, 정의 · 설정의 해석(M6-16 — [`Adapters`]), 해석을 `pw-dap` 의
//! [`AdapterSpec`] 으로 옮기기, 요청 틀 채우기.
//!
//! # 어댑터 인자의 자리(17-m6-plan M6-17 ⑥)
//!
//! 어댑터 프로세스의 인자는 `[dap.<id>] args`(사람의 설정 — 있으면 정의의 `args` 를 통째로 이긴다, `AdapterConfig::args`)다. 정의의
//! `args` 와 **같은 규칙**으로 본다 — 사람이 적은 인자라고 자리 검사를 건너뛰지 않는다:
//!
//! - `stdio` 어댑터: 자리(`{port}` · `{host}` · `{socket}`)가 있으면 띄우지 않는다.
//! - `tcp` 어댑터: `{port}` 와 `{host}` 가 둘 다 있어야 한다(포트는 Pitwall 이 고르고 주소는 늘 `127.0.0.1` — 결정 M6-P7). 채우기는
//!   `pw-dap`(`Transport::Tcp`)이 한다. `{socket}` 은 쓸 수 없다.
//! - `socket` 어댑터: `{socket}` 이 있어야 한다(Pitwall 이 0700 폴더 안 경로를 고른다). `{port}` · `{host}` 는 쓸 수 없다.
//!
//! # 요청 틀의 값
//!
//! 틀(`launch` · `attach`)의 자리는 `[debug]` 의 키(`adapter` · `mode` 밖)로 채운다(`pw_ext::fill_request`). Pitwall 이 정하는 값
//! 셋은 `[debug]` 에 적을 수 없다 — 실행 구성의 판정을 지난 값이어야 해서다:
//!
//! - `{cwd}`: 구성의 `cwd`(프로젝트 루트 안 — 실행과 같은 판정)의 절대 경로.
//! - `{env}`: 구성의 `env_file` · `env` · 프로필을 차례로 얹은 변수(표). 런타임 환경은 어댑터 프로세스가 이미 받았다.
//! - `{root}`: 그 워크트리의 프로젝트 루트.
//!
//! # 대상이 띄운 어댑터에 붙기(17-m6-plan M6-19b)
//!
//! `[debug] connect = { port }`(attach 구성만, loopback 만 — `pw_run::config::DebugConfig::connect`)이면 어댑터를 띄우지 않고 디버그
//! 대상이 이미 띄운 어댑터(debugpy `--listen`)에 붙는다(`pw_dap::Transport::Listening`). 정의가 `attach_connect = true` 여야 한다.
//! 어댑터를 띄우지 않으므로 설치 여부 · `requires` 런타임은 보지 않는다(`[dap.<id>] enabled = false` 는 그대로 거절). `connect` 는
//! Pitwall 이 읽는 키라 틀에는 가지 않는다.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use pw_dap::{AdapterSpec, Transport};
use pw_ext::{DapRequest, DapTransport, DebugAdapterDecl};
use pw_lsp_servers::{AdapterLaunch, AdapterSetup, LspCommands, LspResolution, LspServers};
use pw_rpc::{RpcError, code};
use pw_run::config::{DebugMode, RunConfig, RunKindConfig};
use serde_json::{Map, Value, json};

/// Pitwall 이 정하는 틀 값(머리 주석) — `[debug]` 에 적을 수 없다.
pub const RESERVED_KEYS: &[&str] = &["cwd", "env", "root"];

/// 어댑터의 정의와 한 프로젝트의 해석. 앱은 [`LspServers`] 를 쓰고 시험은 해석을 직접 준다.
pub trait Adapters: Send + Sync + 'static {
    /// 등록된 정의(등록 순서).
    fn definitions(&self) -> Vec<DebugAdapterDecl>;

    /// `adapter_id` 를 `dir`(프로젝트 루트)에서 띄울 해석. 블로킹이다.
    fn setup(
        &self,
        project_id: &str,
        worktree: Option<&str>,
        adapter_id: &str,
        dir: &Path,
    ) -> Result<AdapterSetup, RpcError>;

    /// 어댑터 프로세스를 띄울 때: 정의의 `requires` 로 찾은 런타임을 `setup` 에 넣는다(없으면 거절). 대상이 띄운 어댑터에 붙을
    /// 때(`[debug] connect`)는 부르지 않는다. 블로킹이다.
    fn find_runtimes(&self, _setup: &mut AdapterSetup) -> Result<(), RpcError> {
        Ok(())
    }
}

impl Adapters for LspServers {
    fn definitions(&self) -> Vec<DebugAdapterDecl> {
        self.adapter_definitions().map(|(_, d)| d.clone()).collect()
    }

    fn setup(
        &self,
        project_id: &str,
        worktree: Option<&str>,
        adapter_id: &str,
        dir: &Path,
    ) -> Result<AdapterSetup, RpcError> {
        self.adapter_setup(project_id, worktree, adapter_id, dir)
    }

    fn find_runtimes(&self, setup: &mut AdapterSetup) -> Result<(), RpcError> {
        // 런타임은 어댑터를 띄울 때만 찾는다(언어 서버 관리자와 같은 함수 · 같은 순서 — 결정 M4-D6).
        if !matches!(
            setup.launch,
            AdapterLaunch::Process(LspResolution::Found { .. })
        ) {
            return Ok(());
        }
        let adapter_id = setup.definition.id.clone();
        for req in setup.definition.requires.clone() {
            match pw_lsp::requires::find_runtime(&req, &setup.env) {
                Some(found) => setup.with_runtime(&found),
                None => {
                    let tool = pw_lsp::requires::runtime_name(req.runtime);
                    return Err(RpcError::new(
                        code::INVALID_PARAMS,
                        format!(
                            "the debug adapter `{adapter_id}` needs {tool} {} — install it with mise",
                            req.version
                        ),
                    )
                    .with_data(json!({ "reason": "runtimeMissing", "adapter": adapter_id, "runtime": tool })));
                }
            }
        }
        Ok(())
    }
}

/// 구성의 종류가 쓰는 언어(어댑터를 적지 않은 구성의 어댑터 고르기).
fn language_of(kind: &RunKindConfig) -> Option<&'static str> {
    Some(match kind {
        RunKindConfig::Cargo { .. } => "rust",
        RunKindConfig::Npm { .. } => "javascript",
        RunKindConfig::Python { .. } => "python",
        RunKindConfig::Gradle { .. } | RunKindConfig::Maven { .. } => "java",
        RunKindConfig::Go { .. } => "go",
        _ => return None,
    })
}

/// 쓸 어댑터 id: `[debug] adapter`, 없으면 구성의 종류가 쓰는 언어를 맡는 첫 정의.
pub fn choose(config: &RunConfig, definitions: &[DebugAdapterDecl]) -> Result<String, RpcError> {
    let debug = config.debug.as_ref().ok_or_else(|| no_debug(config))?;
    if let Some(id) = &debug.adapter {
        return Ok(id.clone());
    }
    let language = language_of(&config.kind).ok_or_else(|| {
        RpcError::invalid_params(format!(
            "set `adapter` in the [debug] section of `{}` — a `{}` configuration has no default debugger",
            config.name,
            config.kind.type_name()
        ))
    })?;
    definitions
        .iter()
        .find(|d| d.languages.iter().any(|l| l.name == language))
        .map(|d| d.id.clone())
        .ok_or_else(|| {
            RpcError::invalid_params(format!("no debug adapter handles {language}"))
                .with_data(json!({ "reason": "noAdapter", "language": language }))
        })
}

fn no_debug(config: &RunConfig) -> RpcError {
    RpcError::invalid_params(format!(
        "the run config `{}` has no [debug] section",
        config.name
    ))
    .with_data(json!({ "reason": "noDebug" }))
}

/// `[debug] connect` 의 주소(없으면 `None`). 그 정의가 허락하지 않으면(`attach_connect`) 거절한다.
pub fn listening(
    definition: &DebugAdapterDecl,
    config: &RunConfig,
) -> Result<Option<SocketAddr>, RpcError> {
    let debug = config.debug.as_ref().ok_or_else(|| no_debug(config))?;
    let Some(addr) = debug.connect().map_err(RpcError::invalid_params)? else {
        return Ok(None);
    };
    if !definition.attach_connect {
        return Err(RpcError::invalid_params(format!(
            "the debug adapter `{}` cannot attach to an adapter the program started — remove `connect` from [debug]",
            definition.id
        ))
        .with_data(json!({ "reason": "noAttachConnect", "adapter": definition.id })));
    }
    Ok(Some(addr))
}

/// 정의의 요청 이름.
pub fn request_of(mode: DebugMode) -> DapRequest {
    match mode {
        DebugMode::Launch => DapRequest::Launch,
        DebugMode::Attach => DapRequest::Attach,
    }
}

/// `launch` · `attach` 요청의 인자: 틀을 `[debug]` 의 값과 Pitwall 의 값(머리 주석)으로 채운다.
pub fn request_arguments(
    definition: &DebugAdapterDecl,
    config: &RunConfig,
    cwd: &Path,
    env: &BTreeMap<String, String>,
    root: &Path,
) -> Result<Map<String, Value>, RpcError> {
    let debug = config.debug.as_ref().ok_or_else(|| no_debug(config))?;
    let request = request_of(debug.mode);
    let template = definition.template(request).ok_or_else(|| {
        RpcError::invalid_params(format!(
            "the debug adapter `{}` cannot {}",
            definition.id,
            match request {
                DapRequest::Launch => "launch a program",
                DapRequest::Attach => "attach to a program",
            }
        ))
    })?;
    if let Some(key) = RESERVED_KEYS
        .iter()
        .find(|k| debug.settings.contains_key(**k))
    {
        return Err(RpcError::invalid_params(format!(
            "`{key}` in [debug] is set by Pitwall from the run config — remove it"
        )));
    }
    let mut values: BTreeMap<String, Value> = debug.settings.clone();
    values.remove("connect");
    values.insert("cwd".into(), Value::String(path_text(cwd)));
    values.insert("root".into(), Value::String(path_text(root)));
    values.insert(
        "env".into(),
        Value::Object(
            env.iter()
                .map(|(k, v)| (k.clone(), Value::String(v.clone())))
                .collect(),
        ),
    );
    match pw_ext::fill_request(template, &values).map_err(|e| {
        RpcError::invalid_params(format!("the [debug] section does not fit the adapter: {e}"))
    })? {
        Value::Object(map) => Ok(map),
        _ => Err(RpcError::invalid_params(format!(
            "the {} template of `{}` is not a table",
            match request {
                DapRequest::Launch => "launch",
                DapRequest::Attach => "attach",
            },
            definition.id
        ))),
    }
}

fn path_text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

const PORT: &str = "{port}";
const HOST: &str = "{host}";
const SOCKET: &str = "{socket}";

fn has(args: &[String], slot: &str) -> bool {
    args.iter().any(|a| a.contains(slot))
}

/// 어댑터 인자의 자리를 그 길(`transport`)의 규칙으로 본다(머리 주석 「어댑터 인자의 자리」).
pub fn check_args(adapter: &str, transport: DapTransport, args: &[String]) -> Result<(), RpcError> {
    let bad = |why: String| {
        Err(RpcError::invalid_params(format!(
            "the arguments of the debug adapter `{adapter}` {why}"
        ))
        .with_data(json!({ "reason": "adapterArgs", "adapter": adapter })))
    };
    match transport {
        DapTransport::Stdio => {
            if let Some(slot) = [PORT, HOST, SOCKET].into_iter().find(|s| has(args, s)) {
                return bad(format!("use {slot}, but it talks over stdio"));
            }
        }
        DapTransport::Tcp => {
            if !has(args, PORT) || !has(args, HOST) {
                return bad(format!(
                    "need both {PORT} and {HOST} — Pitwall picks the port and binds it to 127.0.0.1"
                ));
            }
            if has(args, SOCKET) {
                return bad(format!("use {SOCKET}, but it talks over TCP"));
            }
        }
        DapTransport::Socket => {
            if !has(args, SOCKET) {
                return bad(format!("need {SOCKET} — Pitwall picks the socket path"));
            }
            if has(args, PORT) || has(args, HOST) {
                return bad(format!("use {PORT} or {HOST}, but it talks over a socket"));
            }
        }
    }
    Ok(())
}

/// 해석을 `pw-dap` 의 띄울 것으로 옮긴다. 언어 서버가 여는 어댑터면 `connect`(그 서버가 답한 포트)로, `[debug] connect` 면
/// `listening`(대상이 띄운 어댑터의 주소 — [`listening`])으로 붙는다.
pub fn spec(
    setup: &AdapterSetup,
    root: &Path,
    connect: Option<u16>,
    listening: Option<SocketAddr>,
) -> Result<AdapterSpec, RpcError> {
    let id = setup.definition.id.clone();
    let disabled = || {
        RpcError::invalid_params(format!(
            "the debug adapter `{id}` is turned off ([dap.{id}] enabled = false)"
        ))
        .with_data(json!({ "reason": "adapterDisabled", "adapter": id }))
    };
    if matches!(setup.launch, AdapterLaunch::Disabled) {
        return Err(disabled());
    }
    if let Some(addr) = listening {
        return Ok(AdapterSpec {
            adapter_id: id,
            program: PathBuf::new(),
            args: Vec::new(),
            transport: Transport::Listening { addr },
            cwd: root.to_owned(),
            env: setup.env.clone(),
            extra_env: BTreeMap::new(),
        });
    }
    let program: PathBuf = match &setup.launch {
        AdapterLaunch::Disabled => return Err(disabled()),
        AdapterLaunch::Process(LspResolution::Found { path, .. }) => path.clone(),
        AdapterLaunch::Process(other) => {
            return Err(RpcError::new(
                code::INVALID_PARAMS,
                format!("the debug adapter `{id}` is not installed"),
            )
            .with_data(json!({
                "reason": "adapterMissing",
                "adapter": id,
                "resolution": serde_json::to_value(other).unwrap_or(Value::Null),
            })));
        }
        AdapterLaunch::LanguageServer { .. } => {
            let port = connect.ok_or_else(|| {
                RpcError::internal("a language server adapter needs the port it opened")
            })?;
            return Ok(AdapterSpec {
                adapter_id: id,
                program: PathBuf::new(),
                args: Vec::new(),
                transport: Transport::Connect { port },
                cwd: root.to_owned(),
                env: setup.env.clone(),
                extra_env: BTreeMap::new(),
            });
        }
    };
    let args = &setup.config.args;
    check_args(&id, setup.definition.transport, args)?;
    let (args, transport): (Vec<OsString>, Transport) = match setup.definition.transport {
        DapTransport::Stdio => (args.iter().map(OsString::from).collect(), Transport::Stdio),
        DapTransport::Tcp => (
            Vec::new(),
            Transport::Tcp {
                port_args: args.clone(),
            },
        ),
        DapTransport::Socket => (
            Vec::new(),
            Transport::Socket {
                socket_args: args.clone(),
            },
        ),
    };
    Ok(AdapterSpec {
        adapter_id: id,
        program,
        args,
        transport,
        cwd: root.to_owned(),
        env: setup.env.clone(),
        extra_env: BTreeMap::new(),
    })
}

/// 언어 서버가 여는 어댑터(`start`)의 포트: 떠 있는 그 서버에 `start.command` 를 보내 답한 포트(1 – 65535). 서버가 떠 있지 않거나
/// 명령 길(`LspCommands`)이 없으면 띄우지 않고 거절한다 — 언어 서버의 수명은 서버 관리자의 것이다. `project_id` · `root` 는 세션을
/// 연 허가(`DebugGrant`)의 것이다 — 그 프로젝트 · 체크아웃의 서버에만 묻는다(M6-19).
pub async fn language_server_port(
    project_id: &str,
    root: &Path,
    setup: &AdapterSetup,
    commands: Option<Arc<dyn LspCommands>>,
    timeout: Duration,
) -> Result<Option<u16>, RpcError> {
    let AdapterLaunch::LanguageServer { server, command } = &setup.launch else {
        return Ok(None);
    };
    let not_running = || {
        RpcError::new(
            code::INVALID_PARAMS,
            format!("the language server `{server}` must be running to start this debug adapter — open a file it handles first"),
        )
        .with_data(json!({ "reason": "languageServerNotRunning", "server": server }))
    };
    let commands = commands.ok_or_else(not_running)?;
    let answer = tokio::time::timeout(
        timeout,
        commands.execute_command(project_id, root, server, command, Vec::new()),
    )
    .await
    .map_err(|_| RpcError::internal(format!("`{server}` did not open the debug adapter in time")))?
    .ok_or_else(not_running)?
    .map_err(|message| {
        RpcError::internal(format!(
            "`{server}` refused to start the debug adapter: {message}"
        ))
    })?;
    let port = answer
        .as_u64()
        .and_then(|p| u16::try_from(p).ok())
        .filter(|p| *p != 0)
        .ok_or_else(|| {
            RpcError::internal(format!(
                "`{server}` answered `{}`, which is not a port number",
                answer.to_string().chars().take(64).collect::<String>()
            ))
        })?;
    Ok(Some(port))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| (*s).to_owned()).collect()
    }

    /// ⑥ 사람의 `[dap.<id>] args` 도 정의와 같은 자리 규칙을 지난다.
    #[test]
    fn adapter_arguments_follow_the_transport() {
        use DapTransport::*;
        assert!(check_args("a", Stdio, &strings(&["--stdio"])).is_ok());
        assert!(check_args("a", Stdio, &strings(&["--port", "{port}"])).is_err());
        assert!(
            check_args(
                "a",
                Tcp,
                &strings(&["--port", "{port}", "--host", "{host}"])
            )
            .is_ok()
        );
        assert!(check_args("a", Tcp, &strings(&["{port}"])).is_err());
        assert!(check_args("a", Tcp, &strings(&["--host", "{host}"])).is_err());
        assert!(check_args("a", Tcp, &strings(&["{port}", "{host}", "{socket}"])).is_err());
        assert!(check_args("a", Socket, &strings(&["{socket}"])).is_ok());
        assert!(check_args("a", Socket, &strings(&["--listen"])).is_err());
        assert!(check_args("a", Socket, &strings(&["{socket}", "{port}"])).is_err());
    }
}
