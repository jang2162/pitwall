//! 상주(18-m7-plan M7-24 ⑤ · 결정 M7-D2): **사람이 「이 머신에 상주시키기」 를 고를 때만** 이 판의 데몬을 launchd(macOS) ·
//! systemd user(Linux) 단위로 둔다 — 기본은 SSH 로 띄운 데몬이 유휴 규칙(M7-04 ④)까지 사는 것이고, 그래서 재부팅 뒤 첫 연결은 SSH
//! 다. 단위 파일은 `pitwall-relay resident on` 만 쓴다(데몬 · `proxy` · 설치는 쓰지 않는다). install.sh · Homebrew · apt 는 없다.
//!
//! - macOS: `~/Library/LaunchAgents/dev.pitwall.relay.<채널>.plist`, `launchctl bootstrap gui/<uid>` · `bootout`.
//! - Linux: `$XDG_CONFIG_HOME/systemd/user/pitwall-relay-<채널>.service`(없으면 `~/.config/…`), `systemctl --user enable --now` ·
//!   `disable --now`. 로그아웃 뒤에도 살리려면 사람이 `loginctl enable-linger` 를 따로 한다(관리자 권한이 들 수 있어 Pitwall 이 하지
//!   않는다 — `on` 이 안내를 찍는다).
//! - 단위는 이 바이너리(`pitwall-relay-<채널>-<판>`)를 `daemon --resident` 로 띄운다 — 유휴로 끝나지 않는다. 새 판을 깔아도 단위는
//!   옛 판을 가리킨다(사람이 새 판에서 다시 `on`). 채널마다 하나다.
//! - `off` 는 단위를 내리고(떠 있는 상주 데몬이 끝난다) 파일을 지운다. 그 뒤 데몬은 SSH 로 다시 뜰 때만 산다.

use std::io;
use std::path::{Path, PathBuf};

/// 상주 단위를 둘 서비스 관리자.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Manager {
    Launchd,
    SystemdUser,
}

impl Manager {
    /// 이 OS 의 것.
    pub fn current() -> Option<Self> {
        if cfg!(target_os = "macos") {
            Some(Self::Launchd)
        } else if cfg!(target_os = "linux") {
            Some(Self::SystemdUser)
        } else {
            None
        }
    }
}

/// 상주를 다루는 데 드는 것.
pub struct Resident {
    pub manager: Manager,
    pub home: PathBuf,
    /// `$XDG_CONFIG_HOME`(Linux).
    pub config_home: Option<PathBuf>,
    /// 단위가 띄울 바이너리(이 판).
    pub binary: PathBuf,
    pub channel: String,
    pub uid: u32,
    /// 서비스 관리자 명령을 부른다(프로그램, 인자). 시험은 가짜를 넣는다.
    pub run: Runner,
}

/// 서비스 관리자 명령을 부르는 것(프로그램, 인자).
pub type Runner = Box<dyn Fn(&str, &[String]) -> io::Result<()> + Send + Sync>;

/// 단위 파일 하나.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unit {
    pub path: PathBuf,
    pub label: String,
    pub text: String,
}

fn xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// systemd 의 `ExecStart` 인자 하나(공백 · 따옴표 · `%` · `$` 를 감싼다).
fn systemd_arg(text: &str) -> String {
    let escaped = text
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('%', "%%")
        .replace('$', "$$");
    format!("\"{escaped}\"")
}

/// 채널 이름에 쓸 수 있는 글자만.
fn safe_channel(channel: &str) -> String {
    channel
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        .collect()
}

impl Resident {
    pub fn unit(&self) -> Unit {
        let channel = safe_channel(&self.channel);
        let binary = self.binary.to_string_lossy();
        match self.manager {
            Manager::Launchd => {
                let label = format!("dev.pitwall.relay.{channel}");
                let text = format!(
                    "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
                     <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
                     <plist version=\"1.0\">\n<dict>\n\
                     \t<key>Label</key>\n\t<string>{label}</string>\n\
                     \t<key>ProgramArguments</key>\n\t<array>\n\t\t<string>{}</string>\n\t\t<string>daemon</string>\n\t\t<string>--resident</string>\n\t</array>\n\
                     \t<key>RunAtLoad</key>\n\t<true/>\n\
                     \t<key>KeepAlive</key>\n\t<dict>\n\t\t<key>SuccessfulExit</key>\n\t\t<false/>\n\t</dict>\n\
                     \t<key>ProcessType</key>\n\t<string>Background</string>\n\
                     </dict>\n</plist>\n",
                    xml(&binary)
                );
                Unit {
                    path: self
                        .home
                        .join("Library/LaunchAgents")
                        .join(format!("{label}.plist")),
                    label,
                    text,
                }
            }
            Manager::SystemdUser => {
                let label = format!("pitwall-relay-{channel}.service");
                let text = format!(
                    "# Written by `pitwall-relay resident on`; `pitwall-relay resident off` removes it.\n\
                     [Unit]\nDescription=Pitwall Relay ({channel})\n\n\
                     [Service]\nExecStart={} daemon --resident\nRestart=on-failure\n\n\
                     [Install]\nWantedBy=default.target\n",
                    systemd_arg(&binary)
                );
                let config = self
                    .config_home
                    .clone()
                    .unwrap_or_else(|| self.home.join(".config"));
                Unit {
                    path: config.join("systemd/user").join(&label),
                    label,
                    text,
                }
            }
        }
    }

    /// 단위가 있는가.
    pub fn is_on(&self) -> bool {
        self.unit().path.is_file()
    }

    /// 단위를 쓰고 올린다(이미 있으면 다시 쓴다 — 바이너리 경로가 이 판으로 바뀐다).
    pub fn on(&self) -> io::Result<Unit> {
        let unit = self.unit();
        if let Some(dir) = unit.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let was_on = unit.path.is_file();
        if was_on {
            // 옛 단위를 먼저 내린다(옛 판 바이너리를 띄우던 것).
            self.deactivate(&unit);
        }
        write_file(&unit.path, &unit.text)?;
        match self.manager {
            Manager::Launchd => (self.run)(
                "launchctl",
                &[
                    "bootstrap".into(),
                    format!("gui/{}", self.uid),
                    unit.path.to_string_lossy().into_owned(),
                ],
            )?,
            Manager::SystemdUser => {
                (self.run)("systemctl", &["--user".into(), "daemon-reload".into()])?;
                (self.run)(
                    "systemctl",
                    &[
                        "--user".into(),
                        "enable".into(),
                        "--now".into(),
                        unit.label.clone(),
                    ],
                )?;
            }
        }
        Ok(unit)
    }

    fn deactivate(&self, unit: &Unit) {
        let result = match self.manager {
            Manager::Launchd => (self.run)(
                "launchctl",
                &["bootout".into(), format!("gui/{}/{}", self.uid, unit.label)],
            ),
            Manager::SystemdUser => (self.run)(
                "systemctl",
                &[
                    "--user".into(),
                    "disable".into(),
                    "--now".into(),
                    unit.label.clone(),
                ],
            ),
        };
        if let Err(e) = result {
            tracing::warn!(%e, "the service manager did not take the unit down");
        }
    }

    /// 단위를 내리고(떠 있는 상주 데몬이 끝난다) 파일을 지운다. 없었으면 `false`.
    pub fn off(&self) -> io::Result<bool> {
        let unit = self.unit();
        if !unit.path.is_file() {
            return Ok(false);
        }
        self.deactivate(&unit);
        std::fs::remove_file(&unit.path)?;
        if self.manager == Manager::SystemdUser {
            let _ = (self.run)("systemctl", &["--user".into(), "daemon-reload".into()]);
        }
        Ok(true)
    }
}

fn write_file(path: &Path, text: &str) -> io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let tmp = path.with_extension("part");
    {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o644)
            .open(&tmp)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
    }
    std::fs::rename(&tmp, path)
}

/// 서비스 관리자 명령을 실제로 부른다.
pub fn run_command(program: &str, args: &[String]) -> io::Result<()> {
    let status = std::process::Command::new(program).args(args).status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "`{program} {}` failed ({status})",
            args.join(" ")
        )))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    fn resident(manager: Manager, home: &Path) -> (Resident, Arc<Mutex<Vec<String>>>) {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&calls);
        (
            Resident {
                manager,
                home: home.to_owned(),
                config_home: None,
                binary: PathBuf::from(
                    "/home/me/.local/share/pitwall/relay/pitwall-relay-dev-1.0.0",
                ),
                channel: "dev".into(),
                uid: 501,
                run: Box::new(move |program, args| {
                    seen.lock()
                        .unwrap()
                        .push(format!("{program} {}", args.join(" ")));
                    Ok(())
                }),
            },
            calls,
        )
    }

    /// 시험 ③: 단위 파일은 사람이 `on` 을 고를 때만 생긴다 — 만들기 전에는 없고, 데몬 · 설치 길에서는 이 모듈을 부르지 않는다
    /// (`apps/relay` 의 `resident` 하위 명령만 부른다). `off` 는 단위를 내리고 파일을 지운다.
    #[test]
    fn a_launchd_unit_exists_only_between_on_and_off() {
        let home = tempfile::tempdir().unwrap();
        let (r, calls) = resident(Manager::Launchd, home.path());
        assert!(!r.is_on());
        assert!(!home.path().join("Library").exists());
        let unit = r.on().unwrap();
        assert!(r.is_on());
        assert_eq!(
            unit.path,
            home.path()
                .join("Library/LaunchAgents/dev.pitwall.relay.dev.plist")
        );
        let text = std::fs::read_to_string(&unit.path).unwrap();
        assert!(text.contains("<string>daemon</string>"));
        assert!(text.contains("<string>--resident</string>"));
        assert!(text.contains("pitwall-relay-dev-1.0.0"));
        assert_eq!(
            calls.lock().unwrap().as_slice(),
            [format!(
                "launchctl bootstrap gui/501 {}",
                unit.path.display()
            )]
        );
        assert!(r.off().unwrap());
        assert!(!r.is_on());
        assert_eq!(
            calls.lock().unwrap().last().unwrap(),
            "launchctl bootout gui/501/dev.pitwall.relay.dev"
        );
        // 두 번째 끄기는 할 것이 없다.
        assert!(!r.off().unwrap());
    }

    #[test]
    fn a_systemd_user_unit_is_enabled_and_really_disabled() {
        let home = tempfile::tempdir().unwrap();
        let (r, calls) = resident(Manager::SystemdUser, home.path());
        let unit = r.on().unwrap();
        assert_eq!(
            unit.path,
            home.path()
                .join(".config/systemd/user/pitwall-relay-dev.service")
        );
        let text = std::fs::read_to_string(&unit.path).unwrap();
        assert!(text.contains(
            "ExecStart=\"/home/me/.local/share/pitwall/relay/pitwall-relay-dev-1.0.0\" daemon --resident"
        ));
        assert_eq!(
            calls.lock().unwrap().as_slice(),
            [
                "systemctl --user daemon-reload",
                "systemctl --user enable --now pitwall-relay-dev.service"
            ]
        );
        assert!(r.off().unwrap());
        assert!(!unit.path.exists());
        assert_eq!(
            calls.lock().unwrap()[2..],
            [
                "systemctl --user disable --now pitwall-relay-dev.service",
                "systemctl --user daemon-reload"
            ]
        );
    }

    /// 다시 `on` 하면 옛 단위를 먼저 내리고 이 판으로 바꿔 쓴다.
    #[test]
    fn on_again_replaces_the_unit() {
        let home = tempfile::tempdir().unwrap();
        let (r, calls) = resident(Manager::Launchd, home.path());
        r.on().unwrap();
        r.on().unwrap();
        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 3);
        assert!(calls[1].starts_with("launchctl bootout"));
    }

    #[test]
    fn paths_and_names_are_escaped() {
        let home = tempfile::tempdir().unwrap();
        let (mut r, _) = resident(Manager::Launchd, home.path());
        r.binary = PathBuf::from("/a b/<x>&\"y\"");
        r.channel = "dev/../../x".into();
        let unit = r.unit();
        assert!(unit.text.contains("/a b/&lt;x&gt;&amp;&quot;y&quot;"));
        assert_eq!(unit.label, "dev.pitwall.relay.dev....x");
        assert_eq!(
            unit.path.parent().unwrap(),
            home.path().join("Library/LaunchAgents")
        );
        r.manager = Manager::SystemdUser;
        r.binary = PathBuf::from("/a b/%h$x");
        assert!(r.unit().text.contains("ExecStart=\"/a b/%%h$$x\" daemon"));
    }
}
