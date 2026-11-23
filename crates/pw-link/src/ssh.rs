//! 시스템 `ssh` 로 원격 명령을 부르는 길(18-m7-plan M7-06 ① ② ⑤ — 메인 판정 M7-P3 · M7-P17). **ssh argv 는 이 모듈의
//! [`ssh_argv`] 하나로만 만든다** — 설치(`crate::install`) · 판 바꾸기 · 정리(M7-07) · 연결 관리(M7-08)가 모두 [`Ssh`] 를 거친다
//! (18-m7-plan 「공유 부품의 주인」 — 「SSH 실행(argv · askpass 연결)」).
//!
//! - **사람의 ssh 그대로**: 실행 파일은 시스템 `ssh` 이고 사람의 `~/.ssh/config` · 에이전트 · ProxyJump · ControlMaster ·
//!   `known_hosts` 를 ssh 가 따른다. Pitwall 이 넣는 옵션은 `-T`(pty 없음 — 링크는 바이트 스트림이다)뿐이다.
//! - **호스트 키 확인은 ssh 의 것**(M7-P17): `StrictHostKeyChecking=no` · `accept-new` · `UserKnownHostsFile=/dev/null` 같은 확인을
//!   끄는 옵션은 넘기지 않고, 사람의 `ssh_args`(전역 `[[remote_hosts]]`)에 있는 흔한 꼴도 거절한다([`check_options`] — 완전한
//!   목록이 아니다: `-F` 로 준 설정 파일 · `KnownHostsCommand` 등은 ssh 가 따르는 사람의 설정이다). 처음 보는 호스트
//!   키는 ssh 가 askpass 로 묻고 그 질문이 창의 다이얼로그로 간다([`SshAskpass`] — 데스크톱이 git 과 같은 askpass(결정 D43)를
//!   붙인다). **그 질문은 `SSH_ASKPASS_PROMPT` 힌트 없이 온다**(OpenSSH `sshconnect.c` `confirm()` — `confirm` 힌트는 에이전트 ·
//!   ControlMaster 확인에만 붙는다). 답이 `yes`(또는 지문)여야 받고 빈 답 · `no` · askpass 실패는 거절이다 — 창은 질문 글로 호스트
//!   키 확인을 알아보고 [예] 에 `yes` 를 돌려준다(M7-08). 바뀐 호스트 키는 ssh 가 거절한 원문을 [`SshError::HostKeyChanged`] 로 그대로 돌려준다 — 덮는
//!   길이 없다.
//! - **주입이 들어갈 자리가 없다**: 목적지 · 사용자는 허용 글자(`[A-Za-z0-9._-]`, `-` 로 시작하지 않음 — IPv6 주소는 따로)만 받고
//!   `--` 뒤에 둔다(`-oProxyCommand=…` 같은 호스트 이름이 옵션이 되지 않게, 사람의 `ProxyCommand … %h` 로 셸 메타문자가 명령이 되지
//!   않게 — CVE-2023-51385). 원격 명령은 고정 글의 `sh -c '<스크립트>'` 와 허용 글자만 받는 인자([`check_arg`])다 — 원격 경로는
//!   원격 셸이 푼다(원격 `$HOME` 의 공백 · 따옴표 · `$` 는 스크립트 안에서 늘 `"…"` 로 감싼다). 스크립트 글은 사람의 로그인 셸
//!   (bash · zsh · fish · csh)이 한 번 읽으므로 홑따옴표 · 역빗금 · `!` · 줄바꿈을 담지 않는다([`check_script`]).
//! - **비밀**: askpass 의 답은 ssh 와 도우미 사이로만 간다. ssh 의 stderr 는 진단에 보이되(마지막 4 KiB, 제어 · 양방향 글자 제거)
//!   로그에 쓰지 않는다.

use std::any::Any;
use std::ffi::OsString;
use std::io;
use std::path::PathBuf;
use std::pin::Pin;
use std::process::Stdio;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll};

use pw_core::BoxFuture;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt, ReadBuf};
use tokio::process::{Child, ChildStdout, Command};

use crate::transport::{LinkStream, Transport};

/// 진단에 싣는 ssh stderr 의 끝부분(바이트).
pub const STDERR_LIMIT: usize = 4096;
/// 원격 명령의 stdout 상한(바이트). 설치 스크립트의 답은 몇 줄이다 — 로그인 셸이 뿌린 글이 커도 이만큼만 쥔다.
pub const STDOUT_LIMIT: usize = 64 * 1024;
/// 원격 명령 인자 하나의 최대 길이.
pub const MAX_ARG: usize = 256;

/// ssh 가 연결 · 인증 · 호스트 키에서 실패하면 내는 종료 코드. [`Ssh::run`] 은 이 코드를 늘 ssh 의 실패로 본다 — **원격 스크립트 ·
/// `pitwall-relay` 하위 명령은 255 로 끝나지 않는다**(M7-07 · M7-08 이 스크립트를 더할 때도).
const SSH_FAILED: i32 = 255;

/// 붙을 곳: `~/.ssh/config` 의 호스트 이름(별칭) 또는 호스트 이름 · 주소, 사용자, 포트. 사용자 · 포트가 없으면 ssh 가 사람의
/// 설정대로 정한다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshTarget {
    pub host: String,
    pub user: Option<String>,
    pub port: Option<u16>,
}

impl SshTarget {
    /// `~/.ssh/config` 의 호스트 하나(사용자 · 포트는 그 설정대로).
    pub fn alias(host: impl Into<String>) -> Self {
        Self {
            host: host.into(),
            user: None,
            port: None,
        }
    }

    /// 허용 글자 검사. 거절하면 까닭(값은 담지 않는다).
    pub fn check(&self) -> Result<(), SshError> {
        if !(name_chars(&self.host) || ipv6_chars(&self.host)) {
            return Err(SshError::Invalid(
                "the ssh host takes letters, digits, `.`, `_`, `-` (not starting with `-`) or an IPv6 address"
                    .into(),
            ));
        }
        if let Some(user) = &self.user
            && !name_chars(user)
        {
            return Err(SshError::Invalid(
                "the ssh user takes letters, digits, `.`, `_`, `-` (not starting with `-`)".into(),
            ));
        }
        if self.port == Some(0) {
            return Err(SshError::Invalid("the ssh port must not be 0".into()));
        }
        Ok(())
    }
}

/// 호스트 이름 · 사용자 이름에 받는 글자: `[A-Za-z0-9._-]` 이고 `-` 로 시작하지 않는다(허용 목록 — 금지 목록으로는 셸 메타문자를 다
/// 셀 수 없다. `pw_db_driver::tunnel` 의 목적지 검사와 같은 규칙).
pub fn name_chars(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && !value.starts_with('-')
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// IPv6 주소 글자: `[0-9A-Fa-f:.]` 이고 `:` 를 담는다. 영역(`%en0`)은 `[A-Za-z0-9._-]`. 대괄호는 받지 않는다(`-p` 로 포트를
/// 따로 준다).
fn ipv6_chars(value: &str) -> bool {
    let (address, zone) = match value.split_once('%') {
        Some((address, zone)) => (address, Some(zone)),
        None => (value, None),
    };
    address.contains(':')
        && value.len() <= 64
        && address
            .bytes()
            .all(|b| b.is_ascii_hexdigit() || matches!(b, b':' | b'.'))
        && zone.is_none_or(|z| {
            !z.is_empty()
                && z.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
        })
}

/// 사람의 `ssh_args` 에 있어도 넘기지 않는 설정(소문자 키, 값). 호스트 키 확인을 끄거나 사람에게 묻지 않고 받는 것들이다(M7-P17).
const HOST_KEY_BYPASS: [(&str, &[&str]); 3] = [
    (
        "stricthostkeychecking",
        &["no", "off", "accept-new", "false"],
    ),
    ("userknownhostsfile", &["/dev/null", "none"]),
    ("globalknownhostsfile", &["/dev/null", "none"]),
];

/// 값을 받는 ssh 옵션(OpenSSH `ssh(1)` 의 SYNOPSIS).
const VALUE_FLAGS: &str = "BbcDEeFIiJLlmOoPpQRSWw";
/// 링크를 깨뜨려 받지 않는 옵션: 명령 없음(`-N`) · 배경(`-f`) · stdin 끊기(`-n`) · pty(`-t` — 바이트 스트림이 바뀐다) · 하위 시스템
/// (`-s`) · stdio 포워딩(`-W`) · 설정 출력(`-G`) · 판 출력(`-V`) · 마스터 제어(`-O`).
const REFUSED_FLAGS: &str = "NfntsWGVO";

/// 사람의 `ssh_args`(전역 설정의 `[[remote_hosts]]` — 메인 판정 M7-P16)를 본다. 옵션 아닌 낱말(목적지 · 명령이 될 것), 링크를 깨는
/// 옵션, 호스트 키 확인을 끄는 설정은 거절한다. 거절하면 까닭(사람이 읽을 문장).
pub fn check_options(args: &[String]) -> Result<(), String> {
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        let Some(flags) = arg.strip_prefix('-').filter(|f| !f.is_empty() && *f != "-") else {
            return Err(format!(
                "ssh_args takes ssh options only; `{}` is not an option",
                clean_text(arg)
            ));
        };
        for (at, flag) in flags.char_indices() {
            if REFUSED_FLAGS.contains(flag) {
                return Err(format!(
                    "ssh_args must not carry `-{flag}`; it breaks the relay link"
                ));
            }
            if VALUE_FLAGS.contains(flag) {
                let inline = &flags[at + flag.len_utf8()..];
                let value = if inline.is_empty() {
                    i += 1;
                    args.get(i)
                        .map(String::as_str)
                        .ok_or_else(|| format!("ssh_args ends without the value of `-{flag}`"))?
                } else {
                    inline
                };
                if flag == 'o' {
                    check_config_option(value)?;
                }
                break;
            }
            if !flag.is_ascii_alphanumeric() {
                return Err(format!(
                    "ssh_args has an unknown option `{}`",
                    clean_text(arg)
                ));
            }
        }
        i += 1;
    }
    Ok(())
}

/// `-o` 의 값 하나(`Key=Value` 또는 `Key Value`)를 본다.
fn check_config_option(option: &str) -> Result<(), String> {
    let (key, value) = option
        .split_once(['=', ' ', '\t'])
        .map(|(k, v)| (k, v.trim_start_matches(['=', ' ', '\t'])))
        .unwrap_or((option, ""));
    let key = key.trim().to_ascii_lowercase();
    let value = value.trim().trim_matches('"').to_ascii_lowercase();
    for (bad_key, bad_values) in HOST_KEY_BYPASS {
        if key == bad_key && bad_values.contains(&value.as_str()) {
            return Err(format!(
                "ssh_args must not turn off the host key check (`{key}={value}`); Pitwall leaves host keys to \
                 ssh and your known_hosts"
            ));
        }
    }
    // 호스트 키를 known_hosts 가 아닌 명령에서 받는다 — 확인이 사람의 known_hosts 를 지나지 않는다(M7-06 리뷰 R-e, M7-08 이 넓힘).
    if key == "knownhostscommand" && value != "none" {
        return Err(
            "ssh_args must not set KnownHostsCommand; Pitwall leaves host keys to ssh and your known_hosts"
                .to_owned(),
        );
    }
    for (bad_key, bad_values) in LINK_BREAKING {
        let refused = match bad_values {
            Some(values) => values.contains(&value.as_str()),
            // 값이 무엇이든(`none` 만 빼고) 링크의 원격 명령 · 스트림을 바꾼다.
            None => value != "none",
        };
        if key == *bad_key && refused {
            return Err(format!(
                "ssh_args must not carry `{key}={value}`; it breaks the relay link"
            ));
        }
    }
    Ok(())
}

/// 링크를 깨뜨려 넘기지 않는 `-o` 설정(소문자 키, 거절하는 값 — `None` 이면 `none` 밖의 모든 값). 플래그 꼴의 거절
/// ([`REFUSED_FLAGS`])과 같은 것을 `-o` 로 준 것들이다(M7-06 리뷰 R-e — M7-08 이 넓힘).
const LINK_BREAKING: [(&str, Option<&[&str]>); 5] = [
    // pty 를 청하면 바이트 스트림이 바뀐다(`-t`).
    ("requesttty", Some(&["yes", "force"])),
    // 명령 없음(`-N`) · 하위 시스템(`-s`).
    ("sessiontype", Some(&["none", "subsystem"])),
    // stdin 끊기(`-n`).
    ("stdinnull", Some(&["yes", "true"])),
    // 배경(`-f`).
    ("forkafterauthentication", Some(&["yes", "true"])),
    // Pitwall 의 원격 명령 대신 돈다.
    ("remotecommand", None),
];

/// 원격 명령 인자 하나에 받는 글자: `[A-Za-z0-9._+=/:@-]`(판 · 채널 · 해시 · base64 공개키 · 기기 id), `-` 로 시작하지 않음.
/// 인자는 홑따옴표로 감싸 넘기지만 따옴표 · 역빗금 · `$` 같은 글자는 처음부터 받지 않는다 — 사람의 로그인 셸이 무엇이든(fish 는
/// 홑따옴표 안의 역빗금을 푼다) 글자 그대로 간다.
pub fn check_arg(arg: &str) -> bool {
    !arg.is_empty()
        && arg.len() <= MAX_ARG
        && !arg.starts_with('-')
        && arg.bytes().all(|b| {
            b.is_ascii_alphanumeric()
                || matches!(b, b'.' | b'_' | b'+' | b'=' | b'/' | b':' | b'@' | b'-')
        })
}

/// 원격 스크립트 글의 검사: 로그인 셸이 `sh -c '…'` 를 한 번 읽을 때 뜻이 바뀌는 글자(홑따옴표 · 역빗금 · csh 의 `!` · 줄바꿈 ·
/// 제어 문자)가 없다. 스크립트는 이 크레이트의 상수뿐이다 — 시험이 모두 지나게 한다.
pub fn check_script(script: &str) -> bool {
    !script.is_empty()
        && script
            .chars()
            .all(|c| !matches!(c, '\'' | '\\' | '!') && !c.is_control())
}

/// 원격 명령 글: `sh -c '<스크립트>' pitwall '<인자>'…`. 원격 sshd 가 사람의 로그인 셸로 이 글을 푼다.
pub fn remote_command(script: &str, args: &[&str]) -> Result<String, SshError> {
    if !check_script(script) {
        return Err(SshError::Invalid(
            "a remote script must not carry quotes, backslashes, `!` or control characters".into(),
        ));
    }
    let mut command = format!("sh -c '{script}' pitwall");
    for arg in args {
        if !check_arg(arg) {
            return Err(SshError::Invalid(
                "a remote command argument takes letters, digits and `._+=/:@-` only".into(),
            ));
        }
        command.push_str(" '");
        command.push_str(arg);
        command.push('\'');
    }
    Ok(command)
}

/// ssh 의 인자(실행 파일 뒤): `<사람의 ssh_args> -T [-l <사용자>] [-p <포트>] -- <호스트> <원격 명령>`. **argv 를 만드는 곳은
/// 여기와 [`ssh_check_argv`] 뿐이고 둘 다 [`build_argv`] 하나를 지난다.**
pub fn ssh_argv(
    target: &SshTarget,
    options: &[String],
    remote_command: &str,
) -> Result<Vec<OsString>, SshError> {
    build_argv(target, options, Remote::Command(remote_command))
}

/// 링크를 열지 않는 가벼운 상태 확인의 argv(18-m7-plan M7-08 ③ — 메인 판정 M7-P6): `<사람의 ssh_args> -O check [-l <사용자>]
/// [-p <포트>] -- <호스트>`. ssh 는 사람의 ControlMaster 소켓만 묻고 원격에 붙지 않는다(소켓이 없으면 255).
pub fn ssh_check_argv(target: &SshTarget, options: &[String]) -> Result<Vec<OsString>, SshError> {
    build_argv(target, options, Remote::CheckMaster)
}

/// ssh 가 원격에서 할 일.
enum Remote<'a> {
    Command(&'a str),
    CheckMaster,
}

fn build_argv(
    target: &SshTarget,
    options: &[String],
    remote: Remote<'_>,
) -> Result<Vec<OsString>, SshError> {
    target.check()?;
    check_options(options).map_err(SshError::Invalid)?;
    let mut argv: Vec<OsString> = options.iter().map(OsString::from).collect();
    match remote {
        Remote::Command(_) => argv.push("-T".into()),
        Remote::CheckMaster => {
            argv.push("-O".into());
            argv.push("check".into());
        }
    }
    if let Some(user) = &target.user {
        argv.push("-l".into());
        argv.push(user.into());
    }
    if let Some(port) = target.port {
        argv.push("-p".into());
        argv.push(port.to_string().into());
    }
    argv.push("--".into());
    argv.push((&target.host).into());
    if let Remote::Command(command) = remote {
        argv.push(command.into());
    }
    Ok(argv)
}

/// ssh 한 번의 askpass(데스크톱이 `pw_git::write::Askpass::session` 으로 만든다 — 질문은 원격 호스트를 붙이는 창으로). 환경
/// (`SSH_ASKPASS` · `SSH_ASKPASS_REQUIRE=force` · 소켓 · 토큰)과, ssh 가 끝날 때까지 쥐고 있을 것(그 토큰 표의 항목 — 버리면 묻기가
/// 끝난다).
pub struct AskpassSession {
    env: Vec<(OsString, OsString)>,
    _keep: Box<dyn Any + Send + Sync>,
}

impl AskpassSession {
    pub fn new(env: Vec<(OsString, OsString)>, keep: impl Any + Send + Sync) -> Self {
        Self {
            env,
            _keep: Box::new(keep),
        }
    }
}

/// ssh 를 띄울 때마다 askpass 하나를 준다. 쓸 수 없으면(소켓을 열지 못함 · Windows) `None` — 그러면 ssh 는 묻지 못하고 실패한다
/// (호스트 키를 묻지 않고 받는 길로 가지 않는다).
pub trait SshAskpass: Send + Sync {
    fn session(&self) -> Option<AskpassSession>;
}

/// ssh 실패.
#[derive(Debug)]
pub enum SshError {
    /// 띄우기 전에 거절(목적지 · 옵션 · 인자의 글자).
    Invalid(String),
    /// ssh 를 띄우지 못했다(설치되지 않음 등).
    Spawn(io::Error),
    /// 호스트 키가 known_hosts 의 것과 다르다. ssh 의 거절 원문(덮는 길은 없다 — 사람이 known_hosts 를 고친다).
    HostKeyChanged { stderr: String },
    /// 호스트 키를 받지 않았다(확인에서 아니오 · 답 없음 · 물을 수 없음).
    HostKeyNotAccepted { stderr: String },
    /// 연결 · 인증 실패(ssh 종료 코드 255).
    Connect { stderr: String },
}

impl std::fmt::Display for SshError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SshError::Invalid(why) => write!(f, "{why}"),
            SshError::Spawn(e) => write!(f, "cannot run ssh: {e}"),
            SshError::HostKeyChanged { stderr } => {
                write!(
                    f,
                    "the remote host key changed; ssh refused to connect:\n{stderr}"
                )
            }
            SshError::HostKeyNotAccepted { stderr } => {
                write!(f, "the remote host key was not accepted:\n{stderr}")
            }
            SshError::Connect { stderr } => write!(f, "ssh could not connect:\n{stderr}"),
        }
    }
}

impl std::error::Error for SshError {}

/// ssh 가 255 로 끝났을 때 stderr 로 까닭을 가른다.
fn classify(stderr: String) -> SshError {
    if stderr.contains("REMOTE HOST IDENTIFICATION HAS CHANGED") {
        SshError::HostKeyChanged { stderr }
    } else if stderr.contains("Host key verification failed") {
        SshError::HostKeyNotAccepted { stderr }
    } else {
        SshError::Connect { stderr }
    }
}

/// 진단에 보일 글: 줄바꿈만 남기고 제어 문자(탭은 빈칸) · 양방향 제어 문자를 뺀다(`pw_git::write::askpass_text` 와 같은 규칙).
pub fn clean_text(raw: &str) -> String {
    raw.chars()
        .filter_map(|c| match c {
            '\n' => Some('\n'),
            '\t' => Some(' '),
            c if c.is_control() => None,
            '\u{061C}'
            | '\u{200E}'
            | '\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2066}'..='\u{2069}' => None,
            c => Some(c),
        })
        .collect()
}

/// 끝부분 [`STDERR_LIMIT`] 바이트만 남긴 진단 글.
fn tail(bytes: &[u8]) -> String {
    let start = bytes.len().saturating_sub(STDERR_LIMIT);
    clean_text(String::from_utf8_lossy(&bytes[start..]).trim())
}

/// 원격 명령 한 번의 결과(ssh 가 붙고 원격 명령이 끝났다).
#[derive(Debug)]
pub struct RemoteOutput {
    /// 원격 명령의 종료 코드(신호로 끝났으면 `None`).
    pub code: Option<i32>,
    pub stdout: Vec<u8>,
    /// 끝부분 4 KiB, 제어 문자 제거.
    pub stderr: String,
}

/// 원격 호스트 하나로 ssh 를 부르는 것.
pub struct Ssh {
    program: PathBuf,
    env: Option<Vec<(OsString, OsString)>>,
    askpass: Option<Arc<dyn SshAskpass>>,
    target: SshTarget,
    options: Vec<String>,
}

impl Ssh {
    /// 시스템 `ssh`(앱 프로세스의 PATH · 환경 — `SSH_AUTH_SOCK` 등). 목적지 · 옵션은 여기서 거른다.
    pub fn new(target: SshTarget, options: Vec<String>) -> Result<Self, SshError> {
        Self::with_program(PathBuf::from("ssh"), target, options)
    }

    /// 실행 파일을 정한다(시험의 가짜 `ssh`).
    pub fn with_program(
        program: PathBuf,
        target: SshTarget,
        options: Vec<String>,
    ) -> Result<Self, SshError> {
        target.check()?;
        check_options(&options).map_err(SshError::Invalid)?;
        Ok(Self {
            program,
            env: None,
            askpass: None,
            target,
            options,
        })
    }

    /// 자식 환경 전체를 정한다(앱 프로세스의 환경을 물려주지 않는다). 없으면 앱 프로세스의 환경이다.
    pub fn env(mut self, env: Vec<(OsString, OsString)>) -> Self {
        self.env = Some(env);
        self
    }

    pub fn askpass(mut self, askpass: Arc<dyn SshAskpass>) -> Self {
        self.askpass = Some(askpass);
        self
    }

    pub fn target(&self) -> &SshTarget {
        &self.target
    }

    /// 진단에 보일 이름(비밀 없음).
    pub fn describe(&self) -> String {
        let host = &self.target.host;
        match (&self.target.user, self.target.port) {
            (Some(user), Some(port)) => format!("ssh {user}@{host}:{port}"),
            (Some(user), None) => format!("ssh {user}@{host}"),
            (None, Some(port)) => format!("ssh {host}:{port}"),
            (None, None) => format!("ssh {host}"),
        }
    }

    /// 원격 스크립트 하나의 argv([`ssh_argv`] 그대로).
    pub fn argv(&self, script: &str, args: &[&str]) -> Result<Vec<OsString>, SshError> {
        ssh_argv(&self.target, &self.options, &remote_command(script, args)?)
    }

    /// 사람의 ControlMaster 가 이 호스트에 떠 있는가(`ssh -O check` — 원격에 붙지 않고 askpass 도 쓰지 않는다). 마스터 설정이 없거나
    /// 떠 있지 않으면 거짓.
    pub async fn check_master(&self) -> Result<bool, SshError> {
        let argv = ssh_check_argv(&self.target, &self.options)?;
        let mut command = Command::new(&self.program);
        command.args(argv);
        if let Some(env) = &self.env {
            command.env_clear();
            command.envs(env.iter().map(|(k, v)| (k, v)));
        }
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let status = command.status().await.map_err(SshError::Spawn)?;
        Ok(status.success())
    }

    fn command(
        &self,
        script: &str,
        args: &[&str],
    ) -> Result<(Command, Option<AskpassSession>), SshError> {
        let argv = self.argv(script, args)?;
        let mut command = Command::new(&self.program);
        command.args(argv);
        if let Some(env) = &self.env {
            command.env_clear();
            command.envs(env.iter().map(|(k, v)| (k, v)));
        }
        let session = self.askpass.as_ref().and_then(|a| a.session());
        if let Some(session) = &session {
            command.envs(session.env.iter().map(|(k, v)| (k, v)));
            if session.env.iter().any(|(k, _)| k == "SSH_ASKPASS")
                && !session.env.iter().any(|(k, _)| k == "SSH_ASKPASS_REQUIRE")
            {
                command.env("SSH_ASKPASS_REQUIRE", "force");
            }
        }
        command.kill_on_drop(true);
        Ok((command, session))
    }

    /// 원격 스크립트 하나를 돌린다. `input` 은 원격 명령의 stdin 으로 흘린다(설치의 바이너리 — 다 쓰면 닫는다). ssh 자체가
    /// 실패하면(255) [`SshError`], 원격 명령이 끝났으면 그 종료 코드와 출력.
    pub async fn run(
        &self,
        script: &str,
        args: &[&str],
        input: Option<&[u8]>,
    ) -> Result<RemoteOutput, SshError> {
        let (mut command, _session) = self.command(script, args)?;
        command
            .stdin(if input.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().map_err(SshError::Spawn)?;
        let stdin = child.stdin.take();
        let mut stdout = child.stdout.take().expect("piped stdout");
        let mut stderr = child.stderr.take().expect("piped stderr");
        let write = async {
            if let (Some(mut stdin), Some(input)) = (stdin, input) {
                // 원격이 먼저 끝나면(거절 등) 쓰기가 실패한다 — 결과는 종료 코드 · stderr 가 말한다.
                let _ = stdin.write_all(input).await;
                let _ = stdin.shutdown().await;
            }
        };
        let read_out = async {
            let mut out = Vec::new();
            let mut buf = [0u8; 8192];
            loop {
                match stdout.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let room = STDOUT_LIMIT.saturating_sub(out.len());
                        out.extend_from_slice(&buf[..n.min(room)]);
                    }
                }
            }
            out
        };
        let read_err = async {
            let mut err = Vec::new();
            let mut buf = [0u8; 4096];
            loop {
                match stderr.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        err.extend_from_slice(&buf[..n]);
                        if err.len() > STDERR_LIMIT * 2 {
                            err.drain(..err.len() - STDERR_LIMIT);
                        }
                    }
                }
            }
            err
        };
        let ((), out, err) = tokio::join!(write, read_out, read_err);
        let status = child.wait().await.map_err(SshError::Spawn)?;
        let stderr = tail(&err);
        if status.code() == Some(SSH_FAILED) {
            return Err(classify(stderr));
        }
        Ok(RemoteOutput {
            code: status.code(),
            stdout: out,
            stderr,
        })
    }

    /// 원격 스크립트의 stdio 를 링크 바이트 스트림으로 쓰는 전송(18-m7-plan M7-06 ⑤ — `ssh -T <호스트> … proxy`). `open` 마다 ssh 를
    /// 새로 띄운다.
    pub fn transport(self: &Arc<Self>, script: &'static str, args: Vec<String>) -> SshTransport {
        SshTransport {
            ssh: Arc::clone(self),
            script,
            args,
            stderr: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

/// [`Ssh::transport`]. ssh 의 stderr 끝부분을 쥐고 있어 링크가 끊기면 진단에 보일 수 있다([`SshTransport::last_stderr`]).
pub struct SshTransport {
    ssh: Arc<Ssh>,
    script: &'static str,
    args: Vec<String>,
    stderr: Arc<Mutex<Vec<u8>>>,
}

impl SshTransport {
    /// 마지막으로 연 ssh 의 stderr 끝부분(제어 문자 제거). 호스트 키 거절 · 인증 실패의 원문이 여기 있다.
    pub fn last_stderr(&self) -> String {
        tail(&self.stderr.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

impl Transport for SshTransport {
    fn describe(&self) -> String {
        self.ssh.describe()
    }

    fn open(&self) -> BoxFuture<'_, io::Result<LinkStream>> {
        Box::pin(async move {
            let args: Vec<&str> = self.args.iter().map(String::as_str).collect();
            let (mut command, session) = self
                .ssh
                .command(self.script, &args)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e.to_string()))?;
            command
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            let mut child = command.spawn()?;
            let stdin = child.stdin.take().expect("piped stdin");
            let stdout = child.stdout.take().expect("piped stdout");
            let mut stderr = child.stderr.take().expect("piped stderr");
            let sink = Arc::clone(&self.stderr);
            sink.lock().unwrap_or_else(PoisonError::into_inner).clear();
            tokio::spawn(async move {
                let mut buf = [0u8; 4096];
                while let Ok(n) = stderr.read(&mut buf).await {
                    if n == 0 {
                        break;
                    }
                    let mut kept = sink.lock().unwrap_or_else(PoisonError::into_inner);
                    kept.extend_from_slice(&buf[..n]);
                    if kept.len() > STDERR_LIMIT * 2 {
                        let cut = kept.len() - STDERR_LIMIT;
                        kept.drain(..cut);
                    }
                }
            });
            Ok(LinkStream::new(
                ChildOut {
                    out: stdout,
                    _child: child,
                    _session: session,
                },
                stdin,
            ))
        })
    }
}

/// ssh 의 stdout. 버리면 ssh 를 끝낸다(`kill_on_drop`) · askpass 표를 놓는다.
struct ChildOut {
    out: ChildStdout,
    _child: Child,
    _session: Option<AskpassSession>,
}

impl AsyncRead for ChildOut {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.out).poll_read(cx, buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(argv: &[OsString]) -> Vec<String> {
        argv.iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    /// 목적지 · 사용자 이름은 허용 글자만 — 옵션 주입(`-oProxyCommand=…`) · 셸 메타문자 · 공백은 띄우기 전에 거절한다.
    #[test]
    fn hosts_and_users_that_could_become_options_or_shell_are_refused() {
        for host in [
            "-oProxyCommand=touch /tmp/x",
            "-oProxyCommand=x",
            "mini;id",
            "mini$(id)",
            "mini`id`",
            "mini host",
            "mini\nhost",
            "",
            "[::1]",
            "user@mini",
            "mini:22",
        ] {
            assert!(
                SshTarget::alias(host).check().is_err(),
                "host {host:?} was accepted"
            );
        }
        for user in ["-l", "a b", "a;b", "$USER", ""] {
            let target = SshTarget {
                user: Some(user.into()),
                ..SshTarget::alias("mini")
            };
            assert!(target.check().is_err(), "user {user:?} was accepted");
        }
        for ok in [
            "mini",
            "build.tail1234.ts.net",
            "10.0.0.2",
            "fe80::1%en0",
            "a_b-c",
        ] {
            SshTarget::alias(ok).check().unwrap();
        }
    }

    /// argv 의 꼴: 사람의 옵션, `-T`, 사용자 · 포트, `--`, 호스트, 원격 명령 하나. 호스트 키 확인을 끄는 옵션이 없다.
    #[test]
    fn the_argv_ends_with_the_host_after_a_double_dash_and_never_turns_off_host_keys() {
        let target = SshTarget {
            host: "mini".into(),
            user: Some("dev".into()),
            port: Some(2222),
        };
        let argv = strings(
            &ssh_argv(
                &target,
                &["-o".into(), "ServerAliveInterval=30".into()],
                "sh -c 'true' pitwall",
            )
            .unwrap(),
        );
        assert_eq!(
            argv,
            [
                "-o",
                "ServerAliveInterval=30",
                "-T",
                "-l",
                "dev",
                "-p",
                "2222",
                "--",
                "mini",
                "sh -c 'true' pitwall"
            ]
        );
        let joined = argv.join(" ").to_ascii_lowercase();
        for bypass in ["stricthostkeychecking", "userknownhostsfile", "/dev/null"] {
            assert!(!joined.contains(bypass), "{joined}");
        }
    }

    /// 사람의 `ssh_args` 라도 호스트 키 확인을 끄는 설정 · 링크를 깨는 옵션 · 옵션 아닌 낱말은 받지 않는다.
    #[test]
    fn ssh_argv_itself_refuses_a_host_that_could_become_an_option() {
        for host in ["-oProxyCommand=x", "a;b", "a b"] {
            assert!(
                ssh_argv(&SshTarget::alias(host), &[], "true").is_err(),
                "{host:?}"
            );
        }
    }

    #[test]
    fn ssh_args_that_skip_the_host_key_check_or_break_the_link_are_refused() {
        let refused: &[&[&str]] = &[
            &["-o", "StrictHostKeyChecking=no"],
            &["-oStrictHostKeyChecking=no"],
            &["-o", "stricthostkeychecking no"],
            &["-o", "StrictHostKeyChecking=accept-new"],
            &["-o", "UserKnownHostsFile=/dev/null"],
            &["-o", "UserKnownHostsFile /dev/null"],
            &["-o", "GlobalKnownHostsFile=/dev/null"],
            &["-4", "-oStrictHostKeyChecking=off"],
            &["-t"],
            &["-tt"],
            &["-N"],
            &["-n"],
            &["-vf"],
            &["-W", "host:22"],
            &["other-host"],
            &["-p"],
            &["--"],
            &["-"],
            // M7-08 이 넓힌 `-o` 꼴(M7-06 리뷰 R-e).
            &["-o", "KnownHostsCommand=/bin/echo mini ssh-ed25519 AAAA"],
            &["-oKnownHostsCommand /x"],
            &["-o", "RequestTTY=force"],
            &["-o", "requesttty yes"],
            &["-o", "SessionType=none"],
            &["-o", "SessionType subsystem"],
            &["-o", "StdinNull=yes"],
            &["-o", "ForkAfterAuthentication=yes"],
            &["-o", "RemoteCommand=sh"],
            &["-O", "exit"],
        ];
        for args in refused {
            let args: Vec<String> = args.iter().map(|a| (*a).to_owned()).collect();
            assert!(check_options(&args).is_err(), "{args:?} was accepted");
            assert!(
                Ssh::new(SshTarget::alias("mini"), args.clone()).is_err(),
                "{args:?}"
            );
        }
        let accepted: &[&[&str]] = &[
            &["-o", "ServerAliveInterval=30"],
            &["-F", "/home/me/ssh config"],
            &["-i", "/k", "-J", "jump", "-4", "-vC"],
            &["-oStrictHostKeyChecking=yes"],
            &["-o", "UserKnownHostsFile=/home/me/known"],
            &["-o", "ProxyCommand=nc %h %p"],
            &["-o", "KnownHostsCommand=none"],
            &["-o", "RequestTTY=no"],
            &["-o", "SessionType=default"],
            &["-o", "RemoteCommand=none"],
            &["-o", "ControlMaster=auto", "-o", "ControlPersist=60"],
        ];
        for args in accepted {
            let args: Vec<String> = args.iter().map(|a| (*a).to_owned()).collect();
            check_options(&args).unwrap();
        }
    }

    /// 원격 명령: 스크립트는 홑따옴표 하나로 감싸고, 인자는 허용 글자만 — 따옴표 · `$` · 공백 · 역빗금은 거절한다.
    #[test]
    fn remote_command_arguments_take_only_plain_characters() {
        assert_eq!(
            remote_command("exec \"$1\"", &["0.1.0", "SHA256:ab+/c="]).unwrap(),
            "sh -c 'exec \"$1\"' pitwall '0.1.0' 'SHA256:ab+/c='"
        );
        for arg in [
            "a b", "a'b", "$HOME", "a\\b", "a;b", "-x", "", "a\"b", "a`b`",
        ] {
            assert!(remote_command("true", &[arg]).is_err(), "{arg:?}");
        }
        for script in ["echo 'x'", "echo \\x", "echo hi!", "a\nb"] {
            assert!(remote_command(script, &[]).is_err(), "{script:?}");
        }
    }

    #[test]
    fn ssh_failures_are_told_apart_by_the_ssh_message() {
        assert!(matches!(
            classify("@@@ WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED! @@@".into()),
            SshError::HostKeyChanged { .. }
        ));
        assert!(matches!(
            classify("Host key verification failed.".into()),
            SshError::HostKeyNotAccepted { .. }
        ));
        assert!(matches!(
            classify("Permission denied (publickey).".into()),
            SshError::Connect { .. }
        ));
        assert_eq!(clean_text("a\u{1b}[31mb\u{202e}c\td\n"), "a[31mbc d\n");
    }

    /// 가벼운 상태 확인의 argv: 같은 거르기를 지나고(목적지 · 옵션), `-O check` 뒤 `--` 와 호스트로 끝난다 — 원격 명령이 없다.
    #[test]
    fn the_master_check_argv_goes_through_the_same_checks_and_carries_no_command() {
        let target = SshTarget {
            host: "mini".into(),
            user: Some("dev".into()),
            port: Some(2222),
        };
        let argv = strings(&ssh_check_argv(&target, &["-4".into()]).unwrap());
        assert_eq!(
            argv,
            ["-4", "-O", "check", "-l", "dev", "-p", "2222", "--", "mini"]
        );
        for host in ["-oProxyCommand=x", "a;b", "a b", "$(id)"] {
            assert!(
                ssh_check_argv(&SshTarget::alias(host), &[]).is_err(),
                "{host:?}"
            );
        }
        for options in [
            vec!["-oStrictHostKeyChecking=no".to_owned()],
            vec!["-o".to_owned(), "RemoteCommand=id".to_owned()],
            vec!["mini".to_owned()],
        ] {
            assert!(
                ssh_check_argv(&SshTarget::alias("mini"), &options).is_err(),
                "{options:?}"
            );
        }
    }
}
