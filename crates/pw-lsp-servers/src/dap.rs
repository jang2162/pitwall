//! 디버그 어댑터의 설정 · 해석 · 시작(17-m6-plan.md M6-16 ①②). 정의는 내장 확장의 `[contributes.debug_adapters.<id>]` 다
//! (`pw_ext::DebugAdapterDecl`).
//!
//! - 설정 `[dap.<id>]`: `enabled` · `path` · `args` · `env`(전역 `settings.toml` · 프로젝트 `.pitwall/project.toml`, 프로젝트가 키마다
//!   이긴다 — 언어 서버의 `[lsp.<id>]` 와 같은 규칙). 언어 서버에만 있는 키(`settings` · `features` · `install_prompt`)는 경고하고
//!   버린다.
//! - 해석: 실행 파일이 있는 어댑터는 언어 서버와 같은 함수([`crate::resolve::resolve_executable`] — 메인 판정 M6-P8)로 찾는다.
//!   언어 서버가 포트를 여는 어댑터(`start`)는 [`connect_via_language_server`] 로 붙는다.
//!
//! 어댑터 프로세스를 띄우고 DAP 로 이야기하는 것은 디버그 세션(M6-17 — `pw-dap` 의 클라이언트)이다. 이 모듈은 「무엇을 어떻게
//! 띄우나」 와, `start` 어댑터면 그 포트에 붙은 연결까지 준다.

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use pw_ext::{ADAPTER_HOST, AdapterStart, DebugAdapterDecl};
use pw_runtime::RuntimeEnv;
use serde_json::{Map, Value as Json};
use tokio::net::TcpStream;

use crate::resolve::{
    ExecutableDef, LspResolution, ResolveContext, ToolLocator, resolve_executable,
};
use crate::settings::{self, ConfiguredPath, LspSettingsOrigin, ServerSettings};

/// 설정 파일에서 이 기능의 표 이름(`[dap]`).
pub const DAP_SETTINGS_KEY: &str = "dap";

/// `[dap.<id>]` 가 받는 키. 나머지(언어 서버의 키 포함)는 경고하고 버린다.
const ADAPTER_KEYS: &[&str] = &["enabled", "path", "args", "env"];

/// 설정 파일 하나의 `[dap]` 값에서 어댑터 id 별 설정. `origin` 은 경고에 붙일 이름이다.
pub fn parse_layer(
    value: Option<&Json>,
    origin: &str,
    warnings: &mut Vec<String>,
) -> BTreeMap<String, ServerSettings> {
    let mut out = BTreeMap::new();
    let Some(value) = value else { return out };
    let Some(table) = value.as_object() else {
        warnings.push(format!(
            "{origin}: `[{DAP_SETTINGS_KEY}]` must be a table; ignored"
        ));
        return out;
    };
    for (id, entry) in table {
        let at = format!("{origin}: `[{DAP_SETTINGS_KEY}.{id}]`");
        let Some(entry) = entry.as_object() else {
            warnings.push(format!("{at} must be a table; ignored"));
            continue;
        };
        let known: Map<String, Json> = entry
            .iter()
            .filter(|(key, _)| {
                let known = ADAPTER_KEYS.contains(&key.as_str());
                if !known {
                    warnings.push(format!("{at}: unknown key `{key}` is ignored"));
                }
                known
            })
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        out.insert(id.clone(), settings::parse_server(&known, &at, warnings));
    }
    out
}

/// 정의 위에 설정을 합친 어댑터 하나의 값.
///
/// `env` 에는 사람이 적은 비밀이 있을 수 있어 `Debug` 는 이름만 찍는다(`ServerConfig` 와 같다).
#[derive(Clone, PartialEq)]
pub struct AdapterConfig {
    pub enabled: bool,
    pub path: Option<ConfiguredPath>,
    /// 어댑터 프로세스의 인자(`{port}` · `{host}` 자리는 띄울 때 채운다 — `DebugAdapterDecl::args_for` 와 같은 규칙).
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
}

impl std::fmt::Debug for AdapterConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AdapterConfig")
            .field("enabled", &self.enabled)
            .field("path", &self.path)
            .field("args", &self.args)
            .field("env_names", &self.env.keys().collect::<Vec<_>>())
            .finish()
    }
}

/// 정의 위에 전역 · 프로젝트 설정을 합친다: `enabled` · `path` · `args` 는 뒤의 겹(프로젝트)이 이기고, `env` 는 변수마다 덮는다.
pub fn adapter_config(
    def: &DebugAdapterDecl,
    global: Option<&ServerSettings>,
    project: Option<&ServerSettings>,
) -> AdapterConfig {
    let layers: Vec<(LspSettingsOrigin, &ServerSettings)> = [
        global.map(|s| (LspSettingsOrigin::Global, s)),
        project.map(|s| (LspSettingsOrigin::Project, s)),
    ]
    .into_iter()
    .flatten()
    .collect();
    let last = |pick: &dyn Fn(&ServerSettings) -> bool| {
        layers.iter().rev().find(|(_, s)| pick(s)).copied()
    };
    let mut env = def.env.clone();
    for (_, layer) in &layers {
        env.extend(layer.env.iter().map(|(k, v)| (k.clone(), v.clone())));
    }
    AdapterConfig {
        enabled: last(&|s| s.enabled.is_some())
            .and_then(|(_, s)| s.enabled)
            .unwrap_or(true),
        path: last(&|s| s.path.is_some()).map(|(origin, s)| ConfiguredPath {
            value: s.path.clone().unwrap_or_default(),
            origin,
        }),
        args: last(&|s| s.args.is_some())
            .and_then(|(_, s)| s.args.clone())
            .unwrap_or_else(|| def.args.clone()),
        env,
    }
}

/// 어댑터를 어떻게 띄우나.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdapterLaunch {
    /// 실행 파일 — 언어 서버와 같은 해석의 결과(`Disabled` · `InvalidPath` · `NotInstalled` 를 포함한다).
    Process(LspResolution),
    /// 언어 서버 `server` 에 `workspace/executeCommand` 의 `command` 를 보내 받은 포트로 붙는다([`connect_via_language_server`]).
    LanguageServer { server: String, command: String },
    /// `[dap.<id>] enabled = false`.
    Disabled,
}

/// 어댑터 하나를 어떻게 띄우나(실행 파일은 머리 주석의 해석 순서로 찾는다). 사람이 `[dap.<id>] path` 를 적었으면 `start`
/// 어댑터라도 그 실행 파일이다 — 사람이 고른 경로가 이긴다(언어 서버 없이 띄우는 어댑터를 따로 깐 경우).
pub fn resolve_adapter(
    def: &DebugAdapterDecl,
    config: &AdapterConfig,
    ctx: &ResolveContext<'_>,
) -> AdapterLaunch {
    if !config.enabled {
        return AdapterLaunch::Disabled;
    }
    if let Some(executable) = ExecutableDef::of_adapter(def) {
        return AdapterLaunch::Process(resolve_executable(
            &executable,
            config.enabled,
            config.path.as_ref(),
            ctx,
        ));
    }
    match (&def.start, &config.path) {
        (_, Some(_)) => AdapterLaunch::Process(resolve_executable(
            &ExecutableDef {
                command: &def.id,
                mason: None,
                project_local: None,
                resolve_hint: None,
            },
            true,
            config.path.as_ref(),
            ctx,
        )),
        (Some(start), None) => AdapterLaunch::LanguageServer {
            server: start.lsp.name.clone(),
            command: start.command.clone(),
        },
        // 정의는 `command` 나 `start` 하나를 반드시 가진다(pw-ext 가 읽을 때 본다).
        (None, None) => AdapterLaunch::Disabled,
    }
}

/// 한 프로젝트에서 어댑터 하나를 띄우는 데 필요한 것. 디버그 세션(M6-17)이 받는다.
#[derive(Clone, PartialEq)]
pub struct AdapterSetup {
    /// 기여한 확장의 id.
    pub extension: String,
    pub definition: DebugAdapterDecl,
    pub config: AdapterConfig,
    pub launch: AdapterLaunch,
    /// 어댑터 프로세스에 넘길 환경 전체: 그 디렉터리의 런타임 환경 위에 정의 · 설정의 `env` 를 덮은 것. 정의의 `requires` 로 찾은
    /// 런타임은 [`AdapterSetup::with_runtime`] 으로 PATH 맨 앞에 더한다(`node:` 위임의 런처가 그 node 를 쓴다).
    pub env: RuntimeEnv,
}

impl std::fmt::Debug for AdapterSetup {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AdapterSetup")
            .field("extension", &self.extension)
            .field("adapter", &self.definition.id)
            .field("config", &self.config)
            .field("launch", &self.launch)
            .field("env", &self.env)
            .finish()
    }
}

impl AdapterSetup {
    /// `requires` 로 찾은 런타임을 어댑터 환경에 넣는다(언어 서버의 [`crate::ServerSetup::with_runtime`] 과 같은 규칙).
    pub fn with_runtime(&mut self, found: &crate::FoundRuntime) {
        crate::put_runtime_first(&mut self.env, found, &self.config.env);
    }
}

/// 언어 서버가 여는 어댑터(`start.bundles`)를 그 서버의 `initializationOptions.bundles` 에 넣는다 — 어댑터 패키지에 설치된 `share`
/// 파일의 절대 경로를, 적힌 순서로, 이미 있는 목록 뒤에. 꺼 둔 어댑터(`[dap.<id>] enabled = false`) · 설치되지 않은 파일은 넣지
/// 않는다. java-debug-adapter 의 jar 를 jdtls 가 싣는 자리다(17-m6-plan.md M6-16 ③).
pub fn add_bundles<'a>(
    options: &mut Json,
    server_id: &str,
    adapters: impl IntoIterator<Item = (&'a DebugAdapterDecl, bool)>,
    tools: &dyn ToolLocator,
) {
    let mut found = Vec::new();
    for (adapter, enabled) in adapters {
        let (Some(start), Some(package), true) = (&adapter.start, &adapter.mason, enabled) else {
            continue;
        };
        if start.lsp.name != server_id {
            continue;
        }
        for bundle in &start.bundles {
            match tools.locate_share(package, bundle) {
                crate::Located::Installed { path, .. } => {
                    found.push(Json::String(path.display().to_string()));
                }
                other => {
                    tracing::debug!(target: "pw_lsp_servers", adapter = %adapter.id, %bundle, ?other, "a debug adapter bundle is not installed")
                }
            }
        }
    }
    if found.is_empty() {
        return;
    }
    if !options.is_object() {
        *options = Json::Object(Map::new());
    }
    let slot = options
        .as_object_mut()
        .expect("an object")
        .entry("bundles")
        .or_insert_with(|| Json::Array(Vec::new()));
    if let Json::Array(list) = slot {
        list.extend(found);
    } else {
        tracing::warn!(target: "pw_lsp_servers", server = %server_id, "`bundles` in the initialization options is not a list; the debug adapter bundles are not added");
    }
}

/// [`LspCommands::execute_command`] 의 답: 그 서버가 떠 있지 않으면 `None`, 서버의 거절은 `Err`.
pub type CommandAnswer<'a> =
    Pin<Box<dyn Future<Output = Option<Result<Json, String>>> + Send + 'a>>;

/// 떠 있는 언어 서버에 명령을 보내는 곳. 서버 관리자(`pw-lsp`)가 구현하고 디버그 세션(M6-17)이 그 프로젝트 · 워크트리에 맞춰
/// 붙인다.
pub trait LspCommands: Send + Sync {
    /// 떠 있는 서버 `server` 에 `workspace/executeCommand { command, arguments }` 를 보내고 답(`result`)을 받는다. 그 서버가 떠 있지
    /// 않으면 `None`.
    fn execute_command<'a>(
        &'a self,
        server: &'a str,
        command: &'a str,
        arguments: Vec<Json>,
    ) -> CommandAnswer<'a>;
}

/// [`connect_via_language_server`] 가 붙지 못한 까닭.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdapterStartError {
    /// 그 언어 서버가 떠 있지 않다 — 그 언어의 파일을 열어 서버를 띄운 뒤 다시.
    NotRunning { server: String },
    /// 서버가 명령을 거절했다.
    Command { server: String, message: String },
    /// 답이 포트 번호(1 – 65535)가 아니다.
    BadPort { server: String, answer: String },
    /// 그 포트에 붙지 못했다.
    Connect { port: u16, message: String },
    /// 시한 안에 끝나지 않았다(명령의 답 또는 연결).
    TimedOut { server: String },
}

impl std::fmt::Display for AdapterStartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotRunning { server } => write!(
                f,
                "the language server `{server}` must be running to start this debug adapter — open a file it handles first"
            ),
            Self::Command { server, message } => {
                write!(
                    f,
                    "`{server}` refused to start the debug adapter: {message}"
                )
            }
            Self::BadPort { server, answer } => write!(
                f,
                "`{server}` answered `{answer}`, which is not a port number"
            ),
            Self::Connect { port, message } => {
                write!(
                    f,
                    "cannot connect to the debug adapter on {ADAPTER_HOST}:{port}: {message}"
                )
            }
            Self::TimedOut { server } => {
                write!(f, "`{server}` did not open the debug adapter in time")
            }
        }
    }
}

impl std::error::Error for AdapterStartError {}

/// 언어 서버가 여는 어댑터(`start`)에 붙는다: 그 서버에 `workspace/executeCommand` 의 `start.command`(인자 없음)를 보내고, 답한
/// 포트(`127.0.0.1`)에 TCP 로 붙은 연결과 그 포트를 준다. 서버가 떠 있지 않으면 띄우지 않고 거절한다 — 언어 서버를 띄우는 것은
/// 서버 관리자의 일이고, 디버그 세션이 그 서버의 수명을 쥐지 않는다. `timeout` 은 명령의 답과 연결을 합친 시한이다.
pub async fn connect_via_language_server(
    start: &AdapterStart,
    servers: &dyn LspCommands,
    timeout: Duration,
) -> Result<(u16, TcpStream), AdapterStartError> {
    let server = start.lsp.name.as_str();
    let work = async {
        let answer = servers
            .execute_command(server, &start.command, Vec::new())
            .await
            .ok_or_else(|| AdapterStartError::NotRunning {
                server: server.to_owned(),
            })?
            .map_err(|message| AdapterStartError::Command {
                server: server.to_owned(),
                message,
            })?;
        let port = answer
            .as_u64()
            .and_then(|p| u16::try_from(p).ok())
            .filter(|p| *p != 0)
            .ok_or_else(|| AdapterStartError::BadPort {
                server: server.to_owned(),
                answer: answer.to_string().chars().take(64).collect(),
            })?;
        let stream = TcpStream::connect((ADAPTER_HOST, port))
            .await
            .map_err(|e| AdapterStartError::Connect {
                port,
                message: e.to_string(),
            })?;
        Ok((port, stream))
    };
    tokio::time::timeout(timeout, work)
        .await
        .map_err(|_| AdapterStartError::TimedOut {
            server: server.to_owned(),
        })?
}
