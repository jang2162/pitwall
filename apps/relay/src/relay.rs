//! `pitwall-relay` 의 몸통(unix). Relay 는 Windows 를 지원하지 않는다(18-m7-plan 결정 M7-D4).

use std::process::ExitCode;
use std::sync::Arc;

use pw_core::config::AppPaths;
use pw_link::{RelayIdentity, RelayKeyFile, RelayParts};
use pw_relay::command::Command;
use pw_relay::daemon::{Daemon, DaemonParts, Ended, MAX_UNPROVEN};
use tracing_subscriber::Layer as _;
use tracing_subscriber::fmt;
use tracing_subscriber::layer::SubscriberExt as _;

const USAGE: &str = "usage: pitwall-relay <proxy|daemon|mcp [--project <path>]|tool <name> [flags]|status [--json]|open <file>...|askpass <prompt>|link-command [--remove]|resident <on|off|status>|machine|pair [--role <owner|mobile|readonly>] [--name <name>]|pair --from-ssh <key> [<name>]|unpair <device id>|--verify <file> <sha256>|--version>";

pub fn main() -> ExitCode {
    let mut argv = std::env::args_os();
    let argv0 = argv.next();
    let mut args: Vec<String> = argv.map(|a| a.to_string_lossy().into_owned()).collect();
    // 사람이 고른 `~/.local/bin/pitwall` 링크로 불렸다(메인 판정 M7-P12) — `open` · `mcp` · `tool` 만 읽는다.
    if crate::shell::called_as_pitwall(argv0.as_ref()) {
        match crate::shell::pitwall_args(&args) {
            Some(read) => args = read,
            None => {
                eprintln!("{}", crate::shell::PITWALL_USAGE);
                return ExitCode::from(2);
            }
        }
    }
    match args.first().map(String::as_str) {
        Some("proxy") => proxy(),
        Some("daemon") if args.len() == 1 => daemon(false),
        // 상주 단위(launchd · systemd user)가 띄운다 — 유휴로 끝나지 않는다(18-m7-plan M7-24 ⑤).
        Some("daemon") if args.len() == 2 && args[1] == "--resident" => daemon(true),
        Some("resident") => resident(&args[1..]),
        // `status` 는 판에 묶이지 않는 최소 질의다(18-m7-plan M7-07 ②): 새 판 Desktop 이 옛 판 바이너리로 부른다. `--json` 을 받아도
        // 같다(출력은 늘 JSON 한 줄 — 꼴은 `pw_relay::command::Status`, 시험이 고정한다).
        Some("status") if args.len() == 1 || (args.len() == 2 && args[1] == "--json") => status(),
        Some("pair") => pair(&args[1..]),
        Some("unpair") => unpair(&args[1..]),
        Some("--verify") => verify(&args[1..]),
        Some("askpass") => crate::shell::askpass(&args[1..]),
        Some("open") => match paths().and_then(|p| socket_path(&p)) {
            Ok(socket) => crate::shell::open(&socket, &args[1..]),
            Err(code) => code,
        },
        Some("link-command") => crate::shell::link_command(&args[1..]),
        Some("machine") if args.len() == 1 => machine(),
        Some("mcp") => mcp(&args[1..]),
        Some("tool") => tool(&args[1..]),
        Some("--version" | "version") => {
            println!(
                "pitwall-relay {} ({})",
                pw_relay::version(),
                pw_relay::RELEASE_CHANNEL
            );
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
}

fn paths() -> Result<AppPaths, ExitCode> {
    pw_relay::paths::from_env().map_err(|e| {
        eprintln!("pitwall-relay: cannot resolve the app folders: {e}");
        ExitCode::from(1)
    })
}

fn socket_path(paths: &AppPaths) -> Result<std::path::PathBuf, ExitCode> {
    pw_relay::socket::endpoint(
        paths,
        pw_relay::RELEASE_CHANNEL,
        pw_relay::version(),
        &std::env::temp_dir(),
        pw_relay::socket::current_uid(),
    )
    .map_err(|e| {
        eprintln!("pitwall-relay: {e}");
        ExitCode::from(1)
    })
}

fn runtime(threads: usize) -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(threads)
        .enable_all()
        .build()
        .expect("tokio runtime")
}

fn proxy() -> ExitCode {
    let (paths, socket) = match paths().and_then(|p| socket_path(&p).map(|s| (p, s))) {
        Ok(found) => found,
        Err(code) => return code,
    };
    let uid = pw_relay::socket::current_uid();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    rt.block_on(async {
        let spawn = || {
            let exe = std::env::current_exe()?;
            pw_relay::proxy::spawn_daemon(&exe, &["daemon"])
        };
        let stream = match pw_relay::proxy::connect_or_spawn(&socket, uid, spawn).await {
            Ok(stream) => stream,
            Err(e) => {
                eprintln!(
                    "pitwall-relay proxy: {e}. The daemon log is under {}",
                    pw_relay::proxy::log_hint(&paths).display()
                );
                return ExitCode::from(1);
            }
        };
        match pw_relay::proxy::pipe(stream).await {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("pitwall-relay proxy: {e}");
                ExitCode::from(1)
            }
        }
    })
}

fn mcp_socket_path(paths: &AppPaths) -> Result<std::path::PathBuf, ExitCode> {
    pw_relay::mcp::endpoint(
        paths,
        pw_relay::RELEASE_CHANNEL,
        pw_relay::version(),
        &std::env::temp_dir(),
        pw_relay::socket::current_uid(),
    )
    .map_err(|e| {
        eprintln!("pitwall-relay mcp: {e}");
        ExitCode::from(1)
    })
}

/// `mcp [--project <path>]`(18-m7-plan M7-20 ①): 원격 머신의 에이전트가 이 머신의 Relay 에 붙는다. stdio ↔ 데몬의 MCP 소켓을
/// 잇는다(`pw_mcp::proxy` — Desktop 의 `pitwall mcp` 와 같은 프록시). 데몬이 없으면 띄운다 — 헤드리스는 없다. stdout 은 MCP 바이트만.
fn mcp(args: &[String]) -> ExitCode {
    let request = match crate::shell::mcp_request(args) {
        Ok(request) => request,
        Err(e) => {
            eprintln!("pitwall-relay mcp: {e}\nusage: pitwall-relay mcp [--project <path>]");
            return ExitCode::from(2);
        }
    };
    let (paths, daemon_socket, mcp_socket) = match paths().and_then(|p| {
        let daemon = socket_path(&p)?;
        let mcp = mcp_socket_path(&p)?;
        Ok((p, daemon, mcp))
    }) {
        Ok(found) => found,
        Err(code) => return code,
    };
    let uid = pw_relay::socket::current_uid();
    let rt = runtime(2);
    let code = rt.block_on(async {
        let spawn = || {
            let exe = std::env::current_exe()?;
            pw_relay::proxy::spawn_daemon(&exe, &["daemon"])
        };
        let stream =
            match pw_relay::mcp::connect_or_spawn(&daemon_socket, &mcp_socket, uid, request, spawn)
                .await
            {
                Ok(stream) => stream,
                Err(e) => {
                    eprintln!(
                        "pitwall-relay mcp: {e}. The daemon log is under {}",
                        pw_relay::proxy::log_hint(&paths).display()
                    );
                    return ExitCode::from(1);
                }
            };
        let (from_daemon, to_daemon) = stream.into_split();
        let mut stdout = tokio::io::stdout();
        match pw_mcp::proxy::run(tokio::io::stdin(), &mut stdout, from_daemon, to_daemon).await {
            Ok(pw_mcp::proxy::ProxyEnd::ClientClosed) => ExitCode::SUCCESS,
            Ok(pw_mcp::proxy::ProxyEnd::AppClosed { .. }) => {
                eprintln!("pitwall-relay mcp: the relay daemon closed the connection");
                ExitCode::from(1)
            }
            Err(e) => {
                eprintln!("pitwall-relay mcp: {e}");
                ExitCode::from(1)
            }
        }
    });
    // stdin 이 막힌 읽기는 기다리지 않는다.
    rt.shutdown_timeout(std::time::Duration::from_secs(2));
    code
}

/// `tool <name> [flags]`(18-m7-plan M7-20 · 메인 판정 M7-P12): 원격 머신의 셸 · 스크립트가 그 머신 Relay 의 툴을 MCP 와 같은 길로
/// 부른다 — 몸통은 Desktop 의 `pitwall tool` 과 같은 `pw_mcp::cli` 이고, 붙는 곳만 이 판 데몬의 MCP 소켓이다(데몬이 없으면 띄운다,
/// 헤드리스는 없다). 승인 · 「창이 열려 있는 동안 CLI 허용」 · 붙은 owner Desktop 이 없을 때의 거절은 MCP 세션과 같다.
fn tool(args: &[String]) -> ExitCode {
    let args: Vec<std::ffi::OsString> = args.iter().map(Into::into).collect();
    let (invocation, input) = match pw_mcp::cli::begin(&args) {
        Ok(begun) => begun,
        Err(code) => return exit_code(code),
    };
    let (paths, daemon_socket, mcp_socket) = match paths().and_then(|p| {
        let daemon = socket_path(&p)?;
        let mcp = mcp_socket_path(&p)?;
        Ok((p, daemon, mcp))
    }) {
        Ok(found) => found,
        Err(code) => return code,
    };
    let session = pw_mcp::SessionRequest {
        project: invocation.project.clone(),
        cwd: std::env::current_dir().ok(),
    };
    let uid = pw_relay::socket::current_uid();
    let rt = runtime(2);
    let linked = pw_mcp::Catalog::linked();
    let code = rt.block_on(async {
        let spawn = || {
            let exe = std::env::current_exe()?;
            pw_relay::proxy::spawn_daemon(&exe, &["daemon"])
        };
        let stream =
            match pw_relay::mcp::connect_or_spawn(&daemon_socket, &mcp_socket, uid, session, spawn)
                .await
            {
                Ok(stream) => stream,
                Err(e) => {
                    eprintln!(
                        "pitwall tool: {e}. The relay daemon log is under {}",
                        pw_relay::proxy::log_hint(&paths).display()
                    );
                    return pw_mcp::cli::exit::FAILED;
                }
            };
        let request = pw_mcp::cli::Request {
            action: &invocation.action,
            input,
            linked: &linked,
            headless: None,
        };
        pw_mcp::cli::execute(
            request,
            stream,
            pw_mcp::cli::interrupted(),
            &mut std::io::stdout(),
            &mut std::io::stderr(),
        )
        .await
    });
    rt.shutdown_timeout(std::time::Duration::from_secs(2));
    exit_code(code)
}

fn exit_code(code: i32) -> ExitCode {
    ExitCode::from(u8::try_from(code).unwrap_or(1))
}

/// `pair --from-ssh <공개키> [<이름>]`(18-m7-plan M7-06 ⑧ · 결정 M7-D13): Desktop 이 SSH 로 부른다. 데몬 소켓 명령이 아니다 —
/// 이 프로세스가 `$DATA/relay/pairings.json` 에 잠금을 쥐고 직접 쓰고, 데몬은 파일을 다시 읽는다. 표에 없는 키면 `owner` 로 넣는다.
/// stdout 에 JSON 한 줄(`machine` — 이 머신의 id, `added`).
fn pair(args: &[String]) -> ExitCode {
    if args.first().is_none_or(|a| a != "--from-ssh") {
        return pair_code(args);
    }
    let (key, name) = match args {
        [flag, key] if flag == "--from-ssh" => (key, "desktop"),
        [flag, key, name] if flag == "--from-ssh" => (key, name.as_str()),
        _ => {
            eprintln!("usage: pitwall-relay pair --from-ssh <device public key> [<name>]");
            return ExitCode::from(2);
        }
    };
    let Some(key) = pw_link::PublicKey::parse(key) else {
        eprintln!("pitwall-relay pair: the device public key is not a base64 Ed25519 key");
        return ExitCode::from(2);
    };
    let paths = match paths() {
        Ok(paths) => paths,
        Err(code) => return code,
    };
    match pw_relay::pairings::pair(&paths.data, &key, name) {
        Ok(paired) => {
            if paired.added {
                std::thread::sleep(pw_relay::pairings::SETTLE);
            }
            println!(
                "{}",
                serde_json::json!({"machine": paired.machine, "added": paired.added})
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("pitwall-relay pair: {e}");
            ExitCode::from(1)
        }
    }
}

/// `pair [--role <r>] [--name <n>]`(18-m7-plan M7-23 ② — 수동 배치한 Relay): 다른 기기를 SSH 없이 붙일 6자리 코드를 만들어
/// 터미널에 보인다(5분 · 틀린 시도 세 번이면 끝 · 한 번 쓰면 끝 — `pw_relay::codes`). 앞의 대기 코드는 끝난다. role 은 기본 `owner`.
/// 코드는 이 터미널에만 보인다 — 로그에 쓰지 않는다. 기기는 직접 연결(Noise — M7-24)로 코드를 낸다.
fn pair_code(args: &[String]) -> ExitCode {
    const USAGE: &str =
        "usage: pitwall-relay pair [--role <owner|mobile|readonly>] [--name <name>]";
    let (mut role, mut name) = (pw_rpc::Role::Owner, None);
    let mut rest = args.iter();
    while let Some(flag) = rest.next() {
        match (flag.as_str(), rest.next()) {
            ("--role", Some(value)) => match serde_json::from_value(serde_json::json!(value)) {
                Ok(r) => role = r,
                Err(_) => {
                    eprintln!("{USAGE}");
                    return ExitCode::from(2);
                }
            },
            ("--name", Some(value)) => name = Some(value.clone()),
            _ => {
                eprintln!("{USAGE}");
                return ExitCode::from(2);
            }
        }
    }
    let paths = match paths() {
        Ok(paths) => paths,
        Err(code) => return code,
    };
    let now = (pw_relay::codes::system_clock())();
    match pw_relay::codes::issue(&paths.data, role, name.as_deref(), now) {
        Ok((code, _)) => {
            let digits = code.reveal();
            println!(
                "pairing code: {} {}  (role {}, valid for 5 minutes, three tries)",
                &digits[..3],
                &digits[3..],
                serde_json::json!(role).as_str().unwrap_or("owner")
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("pitwall-relay pair: {e}");
            ExitCode::from(1)
        }
    }
}

/// `machine`(18-m7-plan M7-07b — 리뷰 R-1): 이 머신의 id(Relay 기기 키의 지문)를 한 줄 낸다. Desktop 이 판 바꾸기 · 정리 스크립트에서
/// 「ssh 로 닿은 곳이 링크가 증명한 그 머신인가」 를 보는 데만 쓴다(같은지 확인 — 기억 · 판정의 근거가 아니다). 키를 만들지 않는다:
/// 키가 없으면(Relay 를 쓴 적 없는 머신) 3.
fn machine() -> ExitCode {
    let paths = match paths() {
        Ok(paths) => paths,
        Err(code) => return code,
    };
    let file = RelayKeyFile::in_dir(&paths.data);
    if !file.path().exists() {
        return ExitCode::from(3);
    }
    match file.load_or_create() {
        Ok(key) => {
            println!("{}", key.id());
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("pitwall-relay machine: {e}");
            ExitCode::from(1)
        }
    }
}

/// `unpair <기기 id>`(18-m7-plan M7-06 ⑨): 그 지문의 기기를 페어링 표에서 뺀다(파일에 직접). 데몬이 다시 읽고 그 기기의 링크를
/// 끊는다. stdout 에 JSON 한 줄(`removed`).
fn unpair(args: &[String]) -> ExitCode {
    let [device] = args else {
        eprintln!("usage: pitwall-relay unpair <device id>");
        return ExitCode::from(2);
    };
    let paths = match paths() {
        Ok(paths) => paths,
        Err(code) => return code,
    };
    match pw_relay::pairings::unpair(
        &paths.data,
        &pw_link::DeviceId::from_stored(device.as_str()),
    ) {
        Ok(removed) => {
            println!("{}", serde_json::json!({ "removed": removed }));
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("pitwall-relay unpair: {e}");
            ExitCode::from(1)
        }
    }
}

/// `--verify <파일> <sha256>`: 원격에 `sha256sum` · `shasum` 이 없을 때 설치기(M7-06 ④)가 올린 파일의 해시를 이 바이너리로 본다.
/// 같으면 0, 다르면 1.
fn verify(args: &[String]) -> ExitCode {
    let [path, expected] = args else {
        eprintln!("usage: pitwall-relay --verify <file> <sha256>");
        return ExitCode::from(2);
    };
    use sha2::Digest as _;
    use std::io::Read as _;
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(e) => {
            eprintln!("pitwall-relay --verify: {e}");
            return ExitCode::from(1);
        }
    };
    let mut hasher = sha2::Sha256::new();
    let mut buffer = vec![0u8; 1 << 16];
    loop {
        match file.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => hasher.update(&buffer[..n]),
            Err(e) => {
                eprintln!("pitwall-relay --verify: {e}");
                return ExitCode::from(1);
            }
        }
    }
    let actual: String = hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    if actual == expected.to_ascii_lowercase() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

/// `status` 가 데몬의 답을 기다리는 시간. 받기는 하되 답하지 않는 데몬이 조사(`pw_link::upgrade`)를 붙잡지 않게.
const STATUS_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// `status` 의 종료 코드 — 판에 묶이지 않는 약속이다(옛 판 바이너리에도 박혀 나간다, 18-m7-plan M7-07 리뷰 F1):
/// 0 답(JSON 한 줄), 3 **이 판의 데몬이 없다는 것이 확실하다**(소켓 파일이 없거나 받는 쪽이 없다), 4 **모른다**(소켓은 있는데
/// 답이 없다 — 증명 전 연결 상한이 찼거나 바쁘거나 시간 초과 · 폴더 권한), 1 그 밖의 오류. 정리는 3 일 때만 바이너리를 지운다.
pub(crate) const STATUS_NO_DAEMON: u8 = 3;
pub(crate) const STATUS_UNKNOWN: u8 = 4;

/// 연결 오류가 「데몬 없음」 을 확실히 말하는가.
fn no_daemon(e: &std::io::Error) -> bool {
    matches!(
        e.kind(),
        std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
    )
}

fn status() -> ExitCode {
    let socket = match paths().and_then(|p| socket_path(&p)) {
        Ok(socket) => socket,
        Err(code) => return code,
    };
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    let uid = pw_relay::socket::current_uid();
    let answer = rt.block_on(async {
        let stream =
            match tokio::time::timeout(STATUS_TIMEOUT, pw_relay::socket::connect(&socket, uid))
                .await
            {
                Ok(Ok(stream)) => stream,
                Ok(Err(e)) => return Err((no_daemon(&e), e.to_string())),
                Err(_) => {
                    return Err((
                        false,
                        format!("no connection in {} s", STATUS_TIMEOUT.as_secs()),
                    ));
                }
            };
        match tokio::time::timeout(
            STATUS_TIMEOUT,
            pw_relay::command::request_on(
                pw_link::LinkStream::split(stream),
                Command::Status,
                serde_json::Value::Null,
            ),
        )
        .await
        {
            Ok(Ok(reply)) => Ok(reply),
            // 붙은 뒤의 오류(답 없이 닫힘 — 증명 전 상한 등)는 데몬이 있다는 뜻이다.
            Ok(Err(e)) => Err((false, e.to_string())),
            Err(_) => Err((
                false,
                format!("no answer in {} s", STATUS_TIMEOUT.as_secs()),
            )),
        }
    });
    match answer {
        Ok(Ok(status)) => {
            println!("{status}");
            ExitCode::SUCCESS
        }
        Ok(Err(e)) => {
            eprintln!("pitwall-relay status: {e}");
            ExitCode::from(1)
        }
        Err((true, e)) => {
            eprintln!("pitwall-relay status: no daemon of this version is running ({e})");
            ExitCode::from(STATUS_NO_DAEMON)
        }
        Err((false, e)) => {
            eprintln!("pitwall-relay status: the daemon of this version did not answer ({e})");
            ExitCode::from(STATUS_UNKNOWN)
        }
    }
}

/// `pitwall-relay resident on|off|status`: 이 판의 데몬을 상주시키거나(사람이 고를 때만 — 결정 M7-D2) 내린다. 마지막 줄은 JSON
/// `{"resident": bool, "path": …}` 이다(Desktop 의 「상주시키기」 가 ssh 로 부르고 읽는다).
fn resident(args: &[String]) -> ExitCode {
    let action = match args {
        [a] if a == "on" || a == "off" || a == "status" => a.as_str(),
        _ => {
            eprintln!("usage: pitwall-relay resident <on|off|status>");
            return ExitCode::from(2);
        }
    };
    let Some(manager) = pw_relay::resident::Manager::current() else {
        eprintln!("pitwall-relay resident: no service manager on this system");
        return ExitCode::from(1);
    };
    let env = pw_core::config::PathEnv::current();
    let Some(home) = env.home.clone() else {
        eprintln!("pitwall-relay resident: no home folder");
        return ExitCode::from(1);
    };
    let binary = match std::env::current_exe().and_then(std::fs::canonicalize) {
        Ok(exe) => exe,
        Err(e) => {
            eprintln!("pitwall-relay resident: {e}");
            return ExitCode::from(1);
        }
    };
    let r = pw_relay::resident::Resident {
        manager,
        home,
        config_home: std::env::var_os("XDG_CONFIG_HOME")
            .map(std::path::PathBuf::from)
            .filter(|p| p.is_absolute()),
        binary,
        channel: pw_relay::RELEASE_CHANNEL.to_owned(),
        uid: pw_relay::socket::current_uid(),
        run: Box::new(pw_relay::resident::run_command),
    };
    let result = match action {
        "on" => r.on().map(|unit| {
            eprintln!(
                "pitwall-relay resident: this relay now starts with your login ({}). `pitwall-relay resident off` undoes it.",
                unit.path.display()
            );
            if manager == pw_relay::resident::Manager::SystemdUser {
                eprintln!(
                    "pitwall-relay resident: to keep it running after you log out, run `loginctl enable-linger` yourself."
                );
            }
            true
        }),
        "off" => r.off().map(|_| false),
        _ => Ok(r.is_on()),
    };
    match result {
        Ok(on) => {
            println!(
                "{}",
                serde_json::json!({ "resident": on, "path": r.unit().path })
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("pitwall-relay resident: {e}");
            ExitCode::from(1)
        }
    }
}

/// 상주 데몬의 유휴 시한(사실상 끝나지 않는다).
const RESIDENT_IDLE: std::time::Duration = std::time::Duration::from_secs(100 * 365 * 24 * 3600);

fn daemon(resident: bool) -> ExitCode {
    let paths = match paths() {
        Ok(paths) => paths,
        Err(code) => return code,
    };
    // 소켓 폴더(`$STATE/relay`)는 로그 폴더의 부모이기도 하다 — 로그가 umask 권한으로 먼저 만들지 않게 0700 으로 먼저 만든다.
    if let Err(e) = pw_relay::socket::private_dir(&paths.state, pw_relay::socket::current_uid()) {
        eprintln!("pitwall-relay daemon: {e}");
        return ExitCode::from(1);
    }
    init_logging(&paths);
    let home = pw_core::config::PathEnv::current().home;
    let config = match pw_relay::config::load(&pw_relay::paths::relay_toml(&paths), home.as_deref())
    {
        Ok(config) => config,
        Err(e) => {
            tracing::error!(%e, "cannot read relay.toml; not starting");
            eprintln!("pitwall-relay daemon: {e}");
            return ExitCode::from(78);
        }
    };
    let mut config = config;
    if resident {
        tracing::info!(
            "resident daemon: the idle exit is off (`pitwall-relay resident off` takes it down)"
        );
        config.idle.exit_after = RESIDENT_IDLE;
    }
    let socket = match socket_path(&paths) {
        Ok(socket) => socket,
        Err(code) => return code,
    };
    let uid = pw_relay::socket::current_uid();
    let lock = match pw_relay::socket::lock(&socket, uid) {
        Ok(Some(lock)) => lock,
        Ok(None) => {
            tracing::info!("another daemon of this version holds the socket; leaving");
            return ExitCode::SUCCESS;
        }
        Err(e) => {
            tracing::error!(%e, "cannot use the relay socket folder");
            eprintln!("pitwall-relay daemon: {e}");
            return ExitCode::from(1);
        }
    };
    let rt = runtime(2);
    let ended = rt.block_on(async {
        let listener = match pw_relay::socket::bind(&socket, uid, &lock) {
            Ok(listener) => listener,
            Err(e) => {
                tracing::error!(%e, "cannot open the relay socket");
                return Err(ExitCode::from(1));
            }
        };
        let key = match RelayKeyFile::in_dir(&paths.data).load_or_create() {
            Ok(key) => key,
            Err(e) => {
                tracing::error!(%e, "cannot read the relay device key");
                return Err(ExitCode::from(1));
            }
        };
        let pairings_file = pw_relay::pairings::file(&paths.data);
        let (pairings, seen) = pw_relay::pairings::initial(&pairings_file);
        tokio::spawn(pw_relay::pairings::follow(
            pairings_file,
            pairings.clone(),
            seen,
        ));
        // 원격 git 의 암호 질문(M7-15 ①): 도우미 스크립트를 데몬 소켓 옆에 두고, 질문은 데몬 소켓의 좁은 명령 `askpass` 로 받는다.
        let askpass = pw_git::write::Askpass::default();
        let open_requests = pw_core::open::OpenRequests::default();
        // MCP 세션 등록부 — Relay Host 의 `mcp.*` 와 MCP 소켓이 같은 것을 쓴다(링크의 창이 그 승인 허브를 듣는다, M7-20).
        let mcp_sessions = pw_mcp::sessions::Sessions::new();
        let assembled = pw_relay::host::assemble_serving(
            paths.clone(),
            &config,
            tokio::runtime::Handle::current(),
            askpass.clone(),
            open_requests.clone(),
            mcp_sessions.clone(),
        );
        let _askpass = match std::env::current_exe()
            .and_then(|exe| askpass.serve_through(socket.clone(), &exe))
        {
            Ok(through) => Some(through),
            Err(e) => {
                tracing::warn!(%e, "cannot write the askpass helper; git on this relay will not ask for passwords");
                None
            }
        };
        let narrow = Arc::new(pw_relay::narrow::RelayNarrow::new(
            askpass,
            open_requests,
            assembled.host.registry().cloned(),
        ));
        // 기기 관리(`relay.*` — M7-23): 페어링 표 · 대기 코드 · 기기 시각. owner 링크의 RPC 만 닿는다.
        let listen_state = pw_relay::listen::ListenState::default();
        if let Some(devices) = &assembled.devices {
            devices.attach(Arc::new(
                pw_relay::devices::RelayDevices::new(
                    paths.data.clone(),
                    pairings.clone(),
                    pw_relay::codes::system_clock(),
                )
                .with_listen(listen_state.clone()),
            ));
        }
        let codes: Arc<dyn pw_link::CodeBook> = Arc::new(pw_relay::codes::FileCodeBook::new(
            paths.data.clone(),
            Some(pairings.clone()),
            pw_relay::codes::system_clock(),
        ));
        let host = Arc::new(assembled.host);
        let daemon = Daemon::new(DaemonParts {
            link: RelayParts {
                identity: Arc::new(RelayIdentity {
                    key: Arc::new(key),
                    versions: pw_relay::versions(),
                }),
                pairings,
                host: host.clone() as Arc<dyn pw_core::Host>,
                limits: pw_link::LinkLimits::default(),
                // 이어 붙기를 기다리는 링크(18-m7-plan M7-09 ①) — 데몬 수명 하나.
                resumes: pw_link::Resumes::default(),
            },
            activity: Arc::new(assembled.activity),
            narrow,
            idle: config.idle,
            name: config.name.clone(),
            max_unproven: MAX_UNPROVEN,
        });
        // 기기 화면의 마지막 접속(M7-23).
        {
            let data = paths.data.clone();
            daemon.on_proven(Arc::new(move |proven: &pw_link::Proven| {
                let (data, device) = (data.clone(), proven.device.clone());
                let now = (pw_relay::codes::system_clock())();
                tokio::task::spawn_blocking(move || {
                    pw_relay::devices::record_seen(&data, &device, now);
                });
            }));
        }
        tracing::info!(socket = %socket.display(), roots = config.roots.len(), "relay daemon listening");
        let shutdown = tokio_util::sync::CancellationToken::new();
        // MCP 소켓 · HTTP 의 세션 수 — 원격 에이전트가 붙어 있는 동안 데몬이 끝나지 않는다(유휴 판정).
        let agents = pw_mcp::Connections::default();
        daemon.count_agents(agents.clone());
        // Streamable HTTP(M7-25) — `[mcp.http] enabled` 일 때만 루프백에 듣는다. 판정은 MCP 소켓과 같다.
        {
            let presence = daemon.presence();
            pw_relay::mcp::serve_http(
                &host,
                &paths,
                mcp_sessions.clone(),
                Arc::new(move || presence.has_owner()),
                agents.clone(),
                shutdown.child_token(),
            );
        }
        // MCP 소켓(M7-20 ①). 열지 못해도 데몬은 돈다 — `pitwall-relay mcp` 가 그 까닭을 찍고 끝난다(헤드리스는 없다).
        if let Ok(listener) = mcp_socket_path(&paths).and_then(|path| {
            pw_mcp::socket::bind(&path, uid).map_err(|e| {
                tracing::warn!(%e, "cannot open the relay mcp socket; agents on this machine cannot connect");
                ExitCode::from(1)
            })
        }) {
                let presence = daemon.presence();
                let services = pw_relay::mcp::services(
                    &host,
                    &paths,
                    mcp_sessions.clone(),
                    Arc::new(move || presence.has_owner()),
                );
                let connections = agents.clone();
                tracing::info!(socket = %listener.path().display(), "relay mcp socket listening");
                tokio::spawn(pw_mcp::socket::accept_loop(
                    listener,
                    services,
                    connections,
                    shutdown.child_token(),
                ));
        }
        // 직접 연결(QUIC + Noise — M7-24): `[listen]` 이 고른 인터페이스 주소마다. 묶지 못한 주소는 건너뛴다(SSH 는 그대로).
        let direct = tokio::spawn(pw_relay::direct::run(
            daemon.clone(),
            config.listen.clone(),
            codes,
            listen_state,
            pw_relay::listen::scan,
        ));
        tokio::spawn(stop_on_signal(shutdown.clone()));
        let ended = daemon.run(listener, shutdown.clone()).await;
        direct.abort();
        // 열린 MCP 세션을 닫고 대기 중인 승인을 거둔다(앱의 끝내기와 같다 — 감사 큐를 비울 때까지 잠깐 기다린다).
        shutdown.cancel();
        let _ = tokio::task::spawn_blocking(move || {
            mcp_sessions.shutdown(std::time::Duration::from_secs(2))
        })
        .await;
        drop(host);
        Ok(ended)
    });
    // 남은 터미널 · 실행의 프로세스 트리를 끝낸다(유휴로 끝났으면 남은 것이 없다).
    pw_run::shutdown();
    drop(lock);
    match ended {
        Ok(Ended::Idle) => {
            tracing::info!("relay daemon exits: idle");
            ExitCode::SUCCESS
        }
        Ok(Ended::Shutdown) => {
            tracing::info!("relay daemon exits: asked to stop");
            ExitCode::SUCCESS
        }
        Err(code) => code,
    }
}

async fn stop_on_signal(shutdown: tokio_util::sync::CancellationToken) {
    use tokio::signal::unix::{SignalKind, signal};
    let (Ok(mut term), Ok(mut int)) = (
        signal(SignalKind::terminate()),
        signal(SignalKind::interrupt()),
    ) else {
        return;
    };
    tokio::select! {
        _ = term.recv() => {}
        _ = int.recv() => {}
    }
    shutdown.cancel();
}

/// `$STATE/relay/logs/relay.<날짜>.log`(03 「로그 · 진단 (D22)」). 경고 이상은 stderr 에도(앞에서 띄운 데몬일 때 보인다).
fn init_logging(paths: &AppPaths) {
    let dir = pw_relay::paths::logs_dir(paths);
    let level = pw_diag::LogConfig::new(dir.clone()).level_from_env().level;
    let stderr = fmt::layer()
        .with_ansi(false)
        .with_writer(std::io::stderr)
        .with_filter(tracing::level_filters::LevelFilter::WARN.min(level));
    let file = pw_diag::RollingFile::open(
        dir,
        pw_relay::paths::LOG_COMPONENT,
        pw_diag::logging::DEFAULT_RETENTION_DAYS,
        Arc::new(pw_diag::SystemClock),
    );
    let registry = tracing_subscriber::registry().with(stderr);
    let installed = match file {
        Ok(file) => tracing::subscriber::set_global_default(
            registry.with(
                fmt::layer()
                    .with_ansi(false)
                    .with_writer(file)
                    .with_filter(level),
            ),
        ),
        Err(e) => {
            eprintln!("pitwall-relay: no log file ({e}); warnings go to stderr only");
            tracing::subscriber::set_global_default(registry)
        }
    };
    if installed.is_ok() {
        tracing::info!(
            version = pw_relay::version(),
            "pitwall-relay daemon starting"
        );
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    /// Relay 바이너리가 링크한 메서드 선언이 생성물(schema.json)을 덮는다 — Relay 에서 뺀 `bundle.*` 말고는 모두 있고, 생성물에
    /// 없는 것도 없다(M7-02 리뷰 권고 3 — 앱의 `generated_rpc_covers_the_router` 와 같은 자리).
    #[test]
    fn generated_rpc_covers_the_relay_router_but_bundles() {
        let schema: serde_json::Value = serde_json::from_str(include_str!(
            "../../../packages/rpc-client/src/generated/schema.json"
        ))
        .unwrap();
        let generated: BTreeSet<String> = schema["methods"]
            .as_object()
            .expect("schema.json 의 methods 객체")
            .keys()
            .filter(|name| {
                !pw_host::RELAY_EXCLUDED_PREFIXES
                    .iter()
                    .any(|p| name.starts_with(p))
            })
            .cloned()
            .collect();
        let dir = tempfile::tempdir().unwrap();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let assembled = pw_relay::host::assemble(
            pw_relay::paths::relay_paths(&pw_core::config::AppPaths::under(dir.path())),
            &pw_relay::config::RelayConfig::default(),
            rt.handle().clone(),
        );
        let routed: BTreeSet<String> = assembled
            .host
            .router()
            .methods()
            .into_iter()
            .map(|m| m.name.to_owned())
            .collect();
        assert!(routed.len() > 100);
        assert_eq!(generated, routed);
    }

    /// 생성물의 토픽(`TOPICS`)이 모두 Relay 바이너리에 선언돼 있다 — 선언 크레이트를 링크하지 않으면 그 토픽 구독이 오류 없이
    /// 거절된다(M7-02 리뷰 권고 3).
    #[test]
    fn generated_topics_are_declared_in_the_relay() {
        let generated = include_str!("../../../packages/rpc-client/src/generated/rpc.ts");
        let block = generated
            .split_once("export const TOPICS")
            .expect("rpc.ts 의 TOPICS")
            .1
            .split_once("\n};")
            .expect("TOPICS 의 끝")
            .0;
        let topics: BTreeSet<&str> = block
            .lines()
            .filter_map(|line| line.trim().strip_prefix('"')?.split_once('"'))
            .map(|(name, _)| name)
            .collect();
        assert!(topics.len() > 10, "{topics:?}");
        let declared: BTreeSet<&str> = pw_rpc::events().into_iter().map(|e| e.name).collect();
        let missing: Vec<_> = topics.difference(&declared).collect();
        assert!(
            missing.is_empty(),
            "Relay 에 선언이 링크되지 않은 토픽 {missing:?}"
        );
    }
}
