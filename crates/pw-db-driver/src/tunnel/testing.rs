//! 터널 시험 도우미(`test-support`): 로컬 sshd 에 붙는 설정 · ssh 감싸개와, 실 sshd 없이 쓰는 가짜 ssh.
//!
//! **실 sshd 시험**(`live_ssh_tunnel_*`, 표지 `cfg(pw_live_ssh)`): `scripts/db-ssh.mjs` 가 띄운 sshd 컨테이너(`PW_TEST_SSH_URL` =
//! `ssh://pitwall@127.0.0.1:<포트>`, 키 · known_hosts 는 `PW_TEST_SSH_DIR`)에 붙는다. 컨테이너 안에는 에코 서버([`ECHO_PORT`])가
//! 있어 「터널 너머 가짜 TCP 서버」 가 된다. ssh 는 사람의 설정을 읽지 않게 감싼다 — [`SshFixture`] 가 만든 `bin/ssh` 는 진짜
//! ssh 를 `-F <시험 설정>` 으로 부른다(`-F` 를 주면 ssh 는 `~/.ssh/config` 도 `/etc/ssh/ssh_config` 도 읽지 않는다). 고정 argv 는
//! 그 뒤에 그대로 간다. `HOME` 도 임시 폴더다.

use std::ffi::OsString;
use std::fs;
use std::io::Read;
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use super::{AskpassGuard, Launch};

/// sshd 의 URL(`ssh://pitwall@127.0.0.1:<포트>`).
pub const URL_ENV: &str = "PW_TEST_SSH_URL";
/// 클라이언트 키(`client`)와 known_hosts(`known_hosts`)가 있는 폴더.
pub const DIR_ENV: &str = "PW_TEST_SSH_DIR";
/// 컨테이너 안 에코 서버의 포트(받은 바이트를 그대로 돌려준다).
pub const ECHO_PORT: u16 = 7000;
/// 시험 설정의 호스트 이름: known_hosts 에 sshd 의 키가 있다.
pub const HOST: &str = "pw-sshd";
/// known_hosts 가 빈 호스트 이름(모르는 호스트 키 — `StrictHostKeyChecking ask`).
pub const HOST_UNKNOWN: &str = "pw-sshd-unknown";
/// `ControlMaster auto` 인 호스트 이름(사람의 마스터를 흉내 낸다).
pub const HOST_MASTER: &str = "pw-sshd-master";

static NEXT: AtomicU64 = AtomicU64::new(1);

/// 버리면 지우는 임시 폴더.
pub struct TempDir(PathBuf);

impl TempDir {
    /// `base` 아래 새 폴더. `base` 가 없으면 OS 임시 폴더.
    pub fn new(prefix: &str, base: Option<&Path>) -> Self {
        let base = base.map_or_else(std::env::temp_dir, Path::to_path_buf);
        let dir = base.join(format!(
            "{prefix}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// `PATH` 에서 실행 파일을 찾는다.
pub fn which(program: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join(program))
        .find(|p| p.is_file())
}

#[cfg(unix)]
pub fn write_script(path: &Path, body: &str) {
    use std::os::unix::fs::PermissionsExt;
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

/// 작은따옴표 안에 넣을 셸 문자열.
pub fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', r"'\''"))
}

/// 로컬 sshd 에 붙는 시험 설정과 ssh 감싸개.
pub struct SshFixture {
    pub dir: TempDir,
    /// ControlPath 는 104 바이트 아래여야 해서 짧은 `/tmp` 아래에 둔다.
    pub control: TempDir,
    /// `ssh` 감싸개가 있는 폴더(이것을 PATH 로 주면 런타임 환경의 PATH 에서 찾는 길을 그대로 지난다).
    pub bin: PathBuf,
    pub host: String,
    pub port: u16,
}

impl SshFixture {
    /// 표지(`cfg(pw_live_ssh)`)가 켜진 시험에서만 부른다.
    #[cfg(unix)]
    pub fn from_env() -> Self {
        let url = std::env::var(URL_ENV).unwrap_or_else(|_| panic!("{URL_ENV} is not set"));
        let keys = PathBuf::from(
            std::env::var_os(DIR_ENV).unwrap_or_else(|| panic!("{DIR_ENV} is not set")),
        );
        let rest = url
            .strip_prefix("ssh://")
            .unwrap_or_else(|| panic!("{URL_ENV} is not ssh://"));
        let (user, host_port) = rest.split_once('@').unwrap_or(("pitwall", rest));
        let (host, port) = host_port.rsplit_once(':').expect("ssh URL with a port");
        let port: u16 = port.trim_end_matches('/').parse().expect("port");
        let dir = TempDir::new("pw-ssh", None);
        let control = TempDir::new("pwcm", Some(Path::new("/tmp")));
        let bin = dir.path().join("bin");
        fs::create_dir_all(&bin).unwrap();
        fs::create_dir_all(dir.path().join("home")).unwrap();
        let empty = dir.path().join("empty_known_hosts");
        fs::write(&empty, "").unwrap();
        let key = keys.join("client");
        let known = keys.join("known_hosts");
        let common = |known: &Path, strict: &str| {
            format!(
                "  HostName {host}\n  Port {port}\n  User {user}\n  IdentityFile {}\n  IdentitiesOnly yes\n  UserKnownHostsFile {}\n  GlobalKnownHostsFile /dev/null\n  StrictHostKeyChecking {strict}\n  PasswordAuthentication no\n  KbdInteractiveAuthentication no\n  ConnectTimeout 10\n",
                key.display(),
                known.display()
            )
        };
        let config = format!(
            "Host {HOST}\n{}\nHost {HOST_UNKNOWN}\n{}\nHost {HOST_MASTER}\n{}  ControlMaster auto\n  ControlPath {}/%C\n  ControlPersist 30\n",
            common(&known, "yes"),
            common(&empty, "ask"),
            common(&known, "yes"),
            control.path().display()
        );
        let config_path = dir.path().join("config");
        fs::write(&config_path, config).unwrap();
        let real = which("ssh").expect("ssh on PATH");
        write_script(
            &bin.join("ssh"),
            &format!(
                "#!/bin/sh\nexec {} -F {} \"$@\"\n",
                shell_quote(&real),
                shell_quote(&config_path)
            ),
        );
        Self {
            dir,
            control,
            bin,
            host: host.to_owned(),
            port,
        }
    }

    /// 시험 설정 파일.
    pub fn config(&self) -> PathBuf {
        self.dir.path().join("config")
    }

    /// 자식 환경: PATH 는 감싸개 폴더 + 시스템, HOME 은 임시 폴더.
    pub fn env(&self) -> Vec<(OsString, OsString)> {
        let mut path = OsString::from(self.bin.as_os_str());
        path.push(":/usr/bin:/bin");
        vec![
            ("PATH".into(), path),
            ("HOME".into(), self.dir.path().join("home").into_os_string()),
        ]
    }

    pub fn launch(&self, askpass: Option<AskpassGuard>) -> Launch {
        Launch {
            program: self.bin.join("ssh"),
            env: self.env(),
            askpass,
        }
    }
}

/// sshd 가 떴는가: TCP 로 붙어 `SSH-2.0-` 인사를 받을 때까지(최대 `limit`). CI · 로컬 컨테이너가 패키지를 설치하는 동안 기다리는
/// 자리다.
pub fn wait_sshd(host: &str, port: u16, limit: Duration) -> Result<(), String> {
    let deadline = Instant::now() + limit;
    let mut last = String::new();
    while Instant::now() < deadline {
        match TcpStream::connect((host, port)) {
            Ok(mut stream) => {
                let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                let mut buf = [0u8; 8];
                match stream.read_exact(&mut buf) {
                    Ok(()) if &buf == b"SSH-2.0-" => return Ok(()),
                    Ok(()) => last = format!("unexpected greeting {buf:?}"),
                    Err(e) => last = e.to_string(),
                }
            }
            Err(e) => last = e.to_string(),
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    Err(format!("sshd at {host}:{port} is not ready: {last}"))
}
