//! 시험용 가짜 `ssh`(18-m7-plan M7-06 — **실제 SSH 는 쓰지 않는다**). 받은 argv 를 기록하고, `--` 뒤의 호스트 · 원격 명령을 꺼내
//! 원격 머신 대신 이 기기의 `sh -c` 로 돌린다 — 실제 sshd 처럼 원격 명령 글을 셸이 한 번 더 읽는다. 원격 환경은 비워 두고
//! `HOME`(공백 · 따옴표 · `$` 가 든 폴더를 줄 수 있다) · `PATH`(고른 도구만 든 폴더)만 준다.
//!
//! 호스트 키(`FAKE_SSH_HOSTKEY`): `known`(기본 — 묻지 않음), `unknown`(known 파일에 없으면 `SSH_ASKPASS` 를
//! `SSH_ASKPASS_PROMPT=confirm` 으로 불러 OpenSSH 의 문장으로 묻고, 예면 기억 · 아니면 255), `changed`(OpenSSH 의 경고 원문과 255).
//! `FAKE_SSH_CORRUPT=1` 이면 원격 명령의 stdin 끝에 한 바이트를 더한다(전송 중 바뀐 바이너리).

use std::ffi::OsString;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

const SCRIPT: &str = r#"#!/bin/sh
for a in "$@"; do printf '%s\n' "$a" >> "$FAKE_SSH_LOG"; done
printf '%s\n' '--end--' >> "$FAKE_SSH_LOG"
while [ $# -gt 0 ]; do
  if [ "$1" = "--" ]; then shift; break; fi
  shift
done
dest="$1"; shift
cmd="$*"
case "${FAKE_SSH_HOSTKEY:-known}" in
  changed)
    cat >&2 <<EOF
@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@
@    WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED!     @
@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@
IT IS POSSIBLE THAT SOMEONE IS DOING SOMETHING NASTY!
The fingerprint for the ED25519 key sent by the remote host is
SHA256:changedfingerprint.
Host key for $dest has changed and you have requested strict checking.
Host key verification failed.
EOF
    exit 255 ;;
  unknown)
    if ! grep -qx "$dest" "$FAKE_SSH_KNOWN" 2>/dev/null; then
      if [ "$SSH_ASKPASS_REQUIRE" != force ] || [ -z "$SSH_ASKPASS" ]; then
        echo "Host key verification failed." >&2; exit 255
      fi
      prompt="The authenticity of host '$dest (192.0.2.7)' can't be established.
ED25519 key fingerprint is SHA256:fakefingerprint.
Are you sure you want to continue connecting (yes/no/[fingerprint])?"
      if answer=$(SSH_ASKPASS_PROMPT=confirm "$SSH_ASKPASS" "$prompt"); then :; else
        echo "Host key verification failed." >&2; exit 255
      fi
      case "$answer" in
        ""|yes) echo "$dest" >> "$FAKE_SSH_KNOWN" ;;
        *) echo "Host key verification failed." >&2; exit 255 ;;
      esac
    fi ;;
esac
if [ "${FAKE_SSH_CORRUPT:-}" = 1 ]; then
  { cat; printf x; } | env -i HOME="$FAKE_REMOTE_HOME" PATH="$FAKE_REMOTE_PATH" sh -c "$cmd"
  exit $?
fi
exec env -i HOME="$FAKE_REMOTE_HOME" PATH="$FAKE_REMOTE_PATH" sh -c "$cmd"
"#;

/// 원격 머신 하나(가짜 ssh · 원격 홈 · 원격 PATH · argv 기록).
pub struct FakeRemote {
    pub dir: PathBuf,
    pub ssh: PathBuf,
    pub home: PathBuf,
    pub bin: PathBuf,
    pub log: PathBuf,
    pub known: PathBuf,
}

/// 원격 PATH 에 넣는 도구.
const TOOLS: [&str; 13] = [
    "sh", "cat", "mkdir", "chmod", "mv", "rm", "cut", "uname", "env", "grep", "ls", "sleep",
    "printf",
];

/// 실행 파일을 쓴다(0755).
pub fn write_executable(path: &Path, text: &str) {
    std::fs::write(path, text).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn which(tool: &str) -> Option<PathBuf> {
    let out = std::process::Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {tool}"))
        .output()
        .ok()?;
    let path = String::from_utf8(out.stdout).ok()?;
    let path = path.trim();
    path.starts_with('/').then(|| PathBuf::from(path))
}

impl FakeRemote {
    /// `home_name` 은 원격 `$HOME` 의 마지막 이름(공백 · 따옴표 · `$` 를 넣어 본다). `hashers` 가 거짓이면 원격에 `sha256sum` ·
    /// `shasum` 이 없다.
    pub fn new(dir: &Path, home_name: &str, hashers: bool) -> Self {
        let bin = dir.join("remote-bin");
        let home = dir.join("remote").join(home_name);
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        let mut tools: Vec<&str> = TOOLS.to_vec();
        if hashers {
            tools.extend(["sha256sum", "shasum", "perl"]);
        }
        for tool in tools {
            if let Some(path) = which(tool) {
                let _ = std::os::unix::fs::symlink(path, bin.join(tool));
            }
        }
        let ssh = dir.join("ssh");
        write_executable(&ssh, SCRIPT);
        Self {
            dir: dir.to_path_buf(),
            ssh,
            home,
            bin,
            log: dir.join("ssh.log"),
            known: dir.join("known_hosts"),
        }
    }

    /// 원격 `uname -sm` 을 바꾼다.
    pub fn uname(&self, answer: &str) {
        let path = self.bin.join("uname");
        let _ = std::fs::remove_file(&path);
        write_executable(&path, &format!("#!/bin/sh\necho '{answer}'\n"));
    }

    /// ssh 의 환경(`Ssh::env`). `extra` 는 `FAKE_SSH_HOSTKEY` 등.
    pub fn env(&self, extra: &[(&str, &str)]) -> Vec<(OsString, OsString)> {
        let mut env: Vec<(OsString, OsString)> = vec![
            ("PATH".into(), std::env::var_os("PATH").unwrap_or_default()),
            ("FAKE_SSH_LOG".into(), self.log.clone().into()),
            ("FAKE_SSH_KNOWN".into(), self.known.clone().into()),
            ("FAKE_REMOTE_HOME".into(), self.home.clone().into()),
            ("FAKE_REMOTE_PATH".into(), self.bin.clone().into()),
        ];
        env.extend(extra.iter().map(|(k, v)| ((*k).into(), (*v).into())));
        env
    }

    /// 기록한 ssh 호출마다의 argv.
    pub fn calls(&self) -> Vec<Vec<String>> {
        let text = std::fs::read_to_string(&self.log).unwrap_or_default();
        let mut calls = Vec::new();
        let mut current = Vec::new();
        for line in text.lines() {
            if line == "--end--" {
                calls.push(std::mem::take(&mut current));
            } else {
                current.push(line.to_owned());
            }
        }
        calls
    }

    /// 원격 `$DATA/relay`(`$HOME/.local/share/pitwall/relay`).
    pub fn relay_dir(&self) -> PathBuf {
        self.home.join(".local/share/pitwall/relay")
    }
}
