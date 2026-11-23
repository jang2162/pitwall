//! 판 바꾸기 · 정리(18-m7-plan M7-07 — 결정 M7-D6 ①, `docs/design/08-remote-mobile.md` 「Relay 설치」 6). 모든 원격 일은 설치기와
//! 같이 [`crate::ssh::Ssh`] 로 고정 스크립트를 부른다(argv 는 `ssh_argv` 하나 — 따로 만들지 않는다).
//!
//! - **판마다 따로**: Desktop 은 자기와 정확히 같은 (채널, 판)의 Relay 만 쓴다. 바이너리 이름(`pitwall-relay-{채널}-{판}`)도
//!   데몬 소켓(`$STATE/relay/<채널>-<판>.sock` — `pw_relay::socket::endpoint`)도 판마다 따로라, 다른 판 데몬만 떠 있으면 새 판을
//!   깔고 새 판 데몬이 따로 뜬다(설치기 · `proxy` 그대로). 핸드셰이크도 판을 견준다(`crate::handshake`).
//! - **옛 판 데몬**([`Installer::survey`] · [`Survey::old_daemons`]): 옛 판 데몬은 링크 · 실행 · 터미널이 모두 없어지면 유휴 규칙으로
//!   스스로 끝난다. Desktop 은 원격 `$DATA/relay/` 의 바이너리마다 **그 바이너리의** `pitwall-relay status` 를 불러(판에 묶이지 않는
//!   최소 질의 — JSON 한 줄, `pw_relay::command::Status`) 남은 실행 · 터미널 수를 읽고 「옛 판 Relay 에 n개 남음」 을 보인다.
//! - **[옛 판 끝내기]**([`Installer::stop_old`]): 사람이 본 그 데몬(pid · 실행 · 터미널 · 링크 수)이 그대로일 때만 SIGTERM 을 보낸다 —
//!   그사이 수가 바뀌었으면 보내지 않고 새 수를 돌려준다(사람이 모르는 실행을 끝내지 않게). 실행 트리 끝내기는 그 데몬이 한다
//!   (SIGTERM → `pw_run::shutdown`). 원격 셸이 바로 앞에서 `status` 를 다시 불러 pid 가 같을 때만 보낸다(pid 재사용).
//! - **정리**([`Installer::clean`]): 최근 두 판만 남긴다 — 이 Desktop 의 판과, 나머지 가운데 판(semver)이 가장 높은 하나.
//!   **떠 있는 데몬의 바이너리는 지우지 않는다** — 지우기 직전 원격 셸이 그 바이너리의 `status` 를 부르고, 데몬이 없는 것이
//!   확실할 때(종료 3 — 소켓이 없거나 받는 쪽이 없다)만 지운다. 답이 없거나(4 — 증명 전 연결 상한 · 바쁨 · 시간 초과) 그 밖의
//!   답이면 남기고 알린다(fail-closed — 리뷰 F1). 남은 올리기 임시 파일(`<이름>.part.<pid>` — 그
//!   셸이 끝났으면)도 지운다. **`pitwall-relay-*` 만 본다** — 같은 폴더의 `state.db` · `device.key` · `pairings.json` 은 이름으로
//!   거르기 전에 닿지 않는다(M7-04 리뷰 권고 9). 판을 읽을 수 없는 이름은 지우지도 실행하지도 않는다.
//! - **시간 상한**: 원격 `status` 는 연결 · 답에 각 5초(`pitwall-relay status`), Desktop 은 조사 · 정리 · 끝내기의 ssh 한 번마다
//!   [`SURVEY_TIMEOUT`] · [`CLEAN_TIMEOUT`] · [`STOP_TIMEOUT`] 을 둔다(넘으면 ssh 를 끝내고 오류).

use std::cmp::Reverse;
use std::time::Duration;

use serde::Deserialize;

use crate::install::{InstallError, Installer, marked, prelude};

/// 디스크에 남기는 판 수(이 Desktop 의 판 포함 — 결정 M7-P13 · 08 「Relay 설치」 6).
pub const KEEP_VERSIONS: usize = 2;

/// 조사 · 정리 · 끝내기의 ssh 한 번의 시간 상한. 원격 `status` 하나는 10초 안에 끝나고(연결 5 · 답 5), 끝내기는 20초를 기다린다.
pub const SURVEY_TIMEOUT: Duration = Duration::from_secs(60);
pub const CLEAN_TIMEOUT: Duration = Duration::from_secs(60);
pub const STOP_TIMEOUT: Duration = Duration::from_secs(90);

/// 시험만: 시간 상한을 줄인다(nextest 는 시험마다 프로세스가 따로다).
#[cfg(test)]
pub(crate) static TEST_LIMIT_MS: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(u64::MAX);

/// `pitwall-relay status` 의 「이 판 데몬이 없다는 것이 확실하다」 종료 코드(판 무관 약속 — `apps/relay` `STATUS_NO_DAEMON`). 4 는
/// 「모른다」 다.
pub const STATUS_NO_DAEMON: i32 = 3;

/// 조사: `$DATA/relay/pitwall-relay-*` 마다 한 줄 — `current`(이 판), `running <이름> <status JSON>`, `stopped <이름> <종료 코드>`,
/// `upload`·`upload-live <이름>`(올리기 임시 파일 — 그 셸이 살아 있으면 live). 끝 표지 `pitwall-survey end`. 이름에 빈칸이 든
/// 파일은 건너뛰고, `pitwall-relay-<채널>-<숫자>…` 꼴이 아닌 파일은 실행하지 않는다(사람이 둔 `pitwall-relay-backup.sh` 등 —
/// 리뷰 R3).
pub const SURVEY: &str = concat!(
    prelude!(),
    "if [ -d \"$d\" ]; then for p in \"$d\"/pitwall-relay-*; do \
     if [ -f \"$p\" ]; then :; else continue; fi; n=\"${p##*/}\"; \
     case \"$n\" in *[[:space:]]*) continue ;; \
     *.part.*) q=\"${n##*.part.}\"; \
     if [ \"$q\" -gt 1 ] 2>/dev/null && kill -0 \"$q\" 2>/dev/null; then echo \"pitwall-survey upload-live $n\"; \
     else echo \"pitwall-survey upload $n\"; fi; continue ;; esac; \
     if [ \"$p\" = \"$f\" ]; then echo \"pitwall-survey current $n\"; continue; fi; \
     case \"$n\" in pitwall-relay-*-[0-9]*) ;; *) continue ;; esac; \
     if o=$(\"$p\" status 2>/dev/null); then echo \"pitwall-survey running $n $o\"; \
     else echo \"pitwall-survey stopped $n $?\"; fi; done; fi; echo \"pitwall-survey end\""
);

/// 정리: `$3`… 의 이름마다 — `pitwall-relay-` 로 시작하고 `/` 가 없는 것만, 이 판(`$f`)은 빼고, 지우기 직전 그 바이너리의
/// `status` 가 3(데몬이 없는 것이 확실함)일 때만 지운다 — 그 밖(4 답 없음 · 126 실행 못 함 등)은 남기고 `unknown`. 올리기 임시 파일은 그 셸이 끝났을 때만. 한 줄씩
/// `pitwall-clean <결과> <이름>`, 끝 표지 `pitwall-clean end`.
pub const CLEAN: &str = concat!(
    prelude!(),
    "shift 2; for n in \"$@\"; do \
     case \"$n\" in pitwall-relay-*) ;; *) echo \"pitwall-clean refused $n\"; continue ;; esac; \
     case \"$n\" in */*) echo \"pitwall-clean refused $n\"; continue ;; esac; \
     p=\"$d/$n\"; if [ \"$p\" = \"$f\" ]; then echo \"pitwall-clean current $n\"; continue; fi; \
     if [ -f \"$p\" ]; then :; else echo \"pitwall-clean gone $n\"; continue; fi; \
     case \"$n\" in *.part.*) q=\"${n##*.part.}\"; \
     if [ \"$q\" -gt 1 ] 2>/dev/null && kill -0 \"$q\" 2>/dev/null; then echo \"pitwall-clean uploading $n\"; \
     else rm -f \"$p\"; echo \"pitwall-clean removed $n\"; fi; continue ;; esac; \
     if \"$p\" status >/dev/null 2>&1; then echo \"pitwall-clean running $n\"; \
     else c=$?; if [ \"$c\" = 3 ]; then rm -f \"$p\"; echo \"pitwall-clean removed $n\"; \
     else echo \"pitwall-clean unknown $n $c\"; fi; fi; done; echo \"pitwall-clean end\""
);

/// 옛 판 끝내기: `$3` 바이너리 이름 · `$4` 사람이 본 pid. 그 바이너리의 `status` 가 지금도 그 pid 를 낼 때만 SIGTERM 을 보내고
/// 끝날 때까지 20초 기다린다. `pitwall-stop done`·`slow`(아직 끝나는 중)·`changed`(pid 가 다르다 — 보내지 않음)·`gone`(데몬
/// 없음이 확실함)·`unknown`(데몬이 답하지 않음 — 보내지 않음)·`current`(이 판 — 보내지 않음).
pub const STOP: &str = concat!(
    prelude!(),
    "n=\"$3\"; case \"$n\" in pitwall-relay-*) ;; *) echo \"pitwall: not a relay binary\" >&2; exit 64 ;; esac; \
     case \"$n\" in */*|*.part.*) echo \"pitwall: not a relay binary\" >&2; exit 64 ;; esac; \
     p=\"$d/$n\"; if [ \"$p\" = \"$f\" ]; then echo \"pitwall-stop current\"; exit 0; fi; \
     if o=$(\"$p\" status 2>/dev/null); then case \"$o\" in \
     *?pid?:$4[,}]*) kill -TERM \"$4\"; i=0; while kill -0 \"$4\" 2>/dev/null; do \
     if [ \"$i\" -ge 20 ]; then echo \"pitwall-stop slow\"; exit 0; fi; i=$((i+1)); sleep 1; done; \
     echo \"pitwall-stop done\" ;; \
     *) echo \"pitwall-stop changed\" ;; esac; \
     else c=$?; if [ \"$c\" = 3 ]; then echo \"pitwall-stop gone\"; else echo \"pitwall-stop unknown\"; fi; fi"
);

/// 옛 판 바이너리의 `pitwall-relay status` 한 줄(판에 묶이지 않는 꼴 — `pw_relay::command::Status`). 모르는 칸은 버리고, 수 칸이
/// 없으면 0 이다 — `pid` 만 꼭 있어야 한다.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DaemonStatus {
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub release_channel: String,
    #[serde(default)]
    pub name: Option<String>,
    pub pid: u32,
    #[serde(default)]
    pub links: usize,
    #[serde(default)]
    pub owner_links: usize,
    #[serde(default)]
    pub runs: usize,
    #[serde(default)]
    pub terminals: usize,
}

/// 원격 바이너리 하나의 상태.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BinaryState {
    /// 이 Desktop 의 판.
    Current,
    /// 그 판의 데몬이 떠 있다.
    Running(DaemonStatus),
    /// 데몬이 없다는 것이 확실하다(`status` 의 종료 3).
    Stopped,
    /// 데몬이 있는지 모른다(`status` 가 3 이 아닌 오류 — 4 답 없음 · 126 실행 못 함 · 읽지 못하는 답). 끝내기 · 정리 대상이 아니다.
    Unknown { code: Option<i32> },
    /// 올리기 임시 파일(`<이름>.part.<pid>`). `live` — 그 셸이 아직 산다.
    Upload { live: bool },
}

/// `$DATA/relay/` 의 Relay 바이너리 하나.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteBinary {
    /// 파일 이름(`pitwall-relay-{채널}-{판}`).
    pub file: String,
    /// 이름에서 읽은 (채널, 판). 읽지 못하면 `None` — 정리하지 않는다.
    pub build: Option<(String, semver::Version)>,
    pub state: BinaryState,
}

/// [`Installer::survey`] 의 결과.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Survey {
    pub binaries: Vec<RemoteBinary>,
}

/// 떠 있는 옛 판 데몬 하나 — 화면의 「옛 판 Relay 에 n개 남음」 · [옛 판 끝내기] 의 재료.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OldDaemon {
    /// 그 데몬의 바이너리 이름.
    pub file: String,
    pub status: DaemonStatus,
}

impl OldDaemon {
    /// 남은 실행 · 터미널 수(사람에게 보이는 n). 붙은 링크 수(`status.links` — 아직 옛 판인 다른 Desktop)도 함께 보인다.
    pub fn left(&self) -> usize {
        self.status.runs + self.status.terminals
    }

    /// 사람이 본 것과 같은 데몬 · 같은 수인가(실행 · 터미널 · 붙은 링크 — 리뷰 R2: 다른 Desktop 이 새로 붙었으면 그 링크도 끊긴다).
    fn same_as(&self, seen: &OldDaemon) -> bool {
        self.file == seen.file
            && self.status.pid == seen.status.pid
            && self.status.runs == seen.status.runs
            && self.status.terminals == seen.status.terminals
            && self.status.links == seen.status.links
    }
}

/// [`Installer::clean`] 의 결과(파일 이름).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Cleaned {
    pub removed: Vec<String>,
    /// 데몬이 떠 있어 남겼다.
    pub running: Vec<String>,
    /// 올리는 중이라 남겼다.
    pub uploading: Vec<String>,
    /// `status` 가 데몬 여부를 답하지 못해 남겼다(종료 코드).
    pub unknown: Vec<(String, Option<i32>)>,
}

/// [`Installer::stop_old`] 의 결과.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopOutcome {
    /// 끝났다.
    Stopped,
    /// SIGTERM 은 보냈고 20초 안에 끝나지 않았다(실행 트리를 끝내는 중일 수 있다).
    Stopping,
    /// 사람이 본 뒤 그 데몬이 바뀌었다(pid · 실행 · 터미널 · 링크 수) — 보내지 않았다. 지금 것을 다시 보이고 다시 묻는다.
    Changed(OldDaemon),
    /// 그 데몬은 이미 없다.
    Gone,
    /// 그 데몬이 답하지 않아 있는지 모른다 — 보내지 않았다.
    NoAnswer,
}

/// 원격 파일 이름에 쓸 수 있는 글자인가(`Ssh::run` 의 인자 규칙보다 좁다 — `/` 없음).
fn name_ok(name: &str) -> bool {
    name.starts_with("pitwall-relay-")
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'+' | b'-'))
}

/// `pitwall-relay-{채널}-{판}` → (채널, 판). 채널은 첫 `-` 까지다(`stable` · `preview` · `dev`).
pub fn parse_name(file: &str) -> Option<(String, semver::Version)> {
    let rest = file.strip_prefix("pitwall-relay-")?;
    let (channel, version) = rest.split_once('-')?;
    if channel.is_empty() {
        return None;
    }
    Some((channel.to_owned(), semver::Version::parse(version).ok()?))
}

impl Survey {
    /// [`SURVEY`] 의 stdout 을 읽는다(로그인 셸이 뿌린 다른 줄은 무시). 끝 표지가 없으면 오류.
    pub fn parse(stdout: &[u8]) -> Result<Self, String> {
        let text = String::from_utf8_lossy(stdout);
        let mut binaries = Vec::new();
        let mut ended = false;
        for line in text.lines() {
            let Some(rest) = line.strip_prefix("pitwall-survey ") else {
                continue;
            };
            if rest == "end" {
                ended = true;
                continue;
            }
            let mut words = rest.splitn(3, ' ');
            let (Some(kind), Some(file)) = (words.next(), words.next()) else {
                continue;
            };
            let more = words.next().unwrap_or("").trim();
            if !name_ok(file) {
                continue;
            }
            let state = match kind {
                "current" => BinaryState::Current,
                "running" => match serde_json::from_str::<DaemonStatus>(more) {
                    Ok(status) => BinaryState::Running(status),
                    // 답은 했으나 읽지 못하는 꼴 — 떠 있다고만 안다. 끝내기 대상으로 보이지 않고 정리 스크립트가 남긴다.
                    Err(_) => BinaryState::Unknown { code: Some(0) },
                },
                "stopped" => match more.parse::<i32>().ok() {
                    Some(STATUS_NO_DAEMON) => BinaryState::Stopped,
                    code => BinaryState::Unknown { code },
                },
                "upload" => BinaryState::Upload { live: false },
                "upload-live" => BinaryState::Upload { live: true },
                _ => continue,
            };
            binaries.push(RemoteBinary {
                build: parse_name(file),
                file: file.to_owned(),
                state,
            });
        }
        if !ended {
            return Err("the remote survey did not finish".into());
        }
        Ok(Self { binaries })
    }

    /// 떠 있는 옛 판 데몬들(이 판은 빼고).
    pub fn old_daemons(&self) -> Vec<OldDaemon> {
        self.binaries
            .iter()
            .filter_map(|b| match &b.state {
                BinaryState::Running(status) => Some(OldDaemon {
                    file: b.file.clone(),
                    status: status.clone(),
                }),
                _ => None,
            })
            .collect()
    }

    /// 데몬이 있는지 모르는 옛 판 바이너리(답하지 않는 데몬일 수 있다 — 화면은 「답하지 않음」 으로 보인다).
    pub fn unknown(&self) -> Vec<&RemoteBinary> {
        self.binaries
            .iter()
            .filter(|b| matches!(b.state, BinaryState::Unknown { .. }))
            .collect()
    }

    /// 정리할 이름: 이 판과 나머지 가운데 판이 가장 높은 하나(합해 [`KEEP_VERSIONS`])를 남기고 판을 읽을 수 있는 나머지 전부, 그리고
    /// 셸이 끝난 올리기 임시 파일. **떠 있는지는 여기서 보지 않는다** — 원격 셸이 지우기 직전에 본다([`CLEAN`]).
    pub fn to_remove(&self) -> Vec<String> {
        let mut others: Vec<(&semver::Version, &str)> = self
            .binaries
            .iter()
            .filter(|b| !matches!(b.state, BinaryState::Current | BinaryState::Upload { .. }))
            .filter_map(|b| b.build.as_ref().map(|(_, v)| (v, b.file.as_str())))
            .collect();
        others.sort_by_key(|&(version, file)| (Reverse(version), file));
        let mut remove: Vec<String> = others
            .into_iter()
            .skip(KEEP_VERSIONS - 1)
            .map(|(_, file)| file.to_owned())
            .collect();
        remove.extend(
            self.binaries
                .iter()
                .filter(|b| b.state == BinaryState::Upload { live: false })
                .map(|b| b.file.clone()),
        );
        remove
    }
}

impl Cleaned {
    fn parse(stdout: &[u8]) -> Result<Self, String> {
        let text = String::from_utf8_lossy(stdout);
        let mut cleaned = Cleaned::default();
        let mut ended = false;
        for line in text.lines() {
            let Some(rest) = line.strip_prefix("pitwall-clean ") else {
                continue;
            };
            if rest == "end" {
                ended = true;
                continue;
            }
            let mut words = rest.split(' ');
            let (Some(kind), Some(file)) = (words.next(), words.next()) else {
                continue;
            };
            let file = file.to_owned();
            match kind {
                "removed" => cleaned.removed.push(file),
                "running" => cleaned.running.push(file),
                "uploading" => cleaned.uploading.push(file),
                "unknown" => cleaned
                    .unknown
                    .push((file, words.next().and_then(|c| c.parse().ok()))),
                _ => {}
            }
        }
        if !ended {
            return Err("the remote clean-up did not finish".into());
        }
        Ok(cleaned)
    }
}

impl Installer {
    /// 시간 상한을 둔 원격 스크립트 한 번. 넘으면 ssh 를 끝낸다(`Ssh` 의 자식은 `kill_on_drop`).
    async fn timed(
        &self,
        step: &'static str,
        limit: Duration,
        script: &str,
        args: &[&str],
    ) -> Result<crate::ssh::RemoteOutput, InstallError> {
        #[cfg(test)]
        let limit = limit.min(Duration::from_millis(
            TEST_LIMIT_MS.load(std::sync::atomic::Ordering::Relaxed),
        ));
        tokio::time::timeout(limit, self.run(step, script, args, None))
            .await
            .unwrap_or_else(|_| {
                Err(InstallError::Remote {
                    step,
                    code: None,
                    stderr: format!("no answer in {} s", limit.as_secs()),
                })
            })
    }

    /// 원격 `$DATA/relay/` 의 Relay 바이너리와 그 데몬들(옛 판 바이너리마다 그 바이너리의 `status`).
    pub async fn survey(&self) -> Result<Survey, InstallError> {
        let output = self.timed("survey", SURVEY_TIMEOUT, SURVEY, &[]).await?;
        Survey::parse(&output.stdout).map_err(InstallError::Protocol)
    }

    /// 최근 두 판만 남긴다([`Survey::to_remove`] → 원격 셸이 이름마다 떠 있는지 다시 보고 지운다). 지울 것이 없으면 ssh 를 부르지
    /// 않는다.
    pub async fn clean(&self, survey: &Survey) -> Result<Cleaned, InstallError> {
        let names = survey.to_remove();
        if names.is_empty() {
            return Ok(Cleaned::default());
        }
        let args: Vec<&str> = names.iter().map(String::as_str).collect();
        let output = self.timed("clean-up", CLEAN_TIMEOUT, CLEAN, &args).await?;
        Cleaned::parse(&output.stdout).map_err(InstallError::Protocol)
    }

    /// [옛 판 끝내기]: 사람이 본 그 데몬(`seen` — [`Survey::old_daemons`] 의 것)이 지금도 같은 pid · 같은 실행 · 터미널 수일 때만
    /// SIGTERM 을 보낸다. 다르면 보내지 않고 [`StopOutcome::Changed`] 로 지금 것을 돌려준다.
    pub async fn stop_old(&self, seen: &OldDaemon) -> Result<StopOutcome, InstallError> {
        if !name_ok(&seen.file) {
            return Err(InstallError::Protocol("not a relay binary name".to_owned()));
        }
        let now = self.survey().await?;
        let Some(current) = now.old_daemons().into_iter().find(|d| d.file == seen.file) else {
            // 답하지 않는 데몬은 「없음」 이 아니다(리뷰 F1).
            return Ok(if now.unknown().iter().any(|b| b.file == seen.file) {
                StopOutcome::NoAnswer
            } else {
                StopOutcome::Gone
            });
        };
        if !current.same_as(seen) {
            return Ok(StopOutcome::Changed(current));
        }
        let pid = seen.status.pid.to_string();
        let output = self
            .timed(
                "stop",
                STOP_TIMEOUT,
                STOP,
                &[seen.file.as_str(), pid.as_str()],
            )
            .await?;
        match marked(&output.stdout, "pitwall-stop ").as_deref() {
            Some("done") => Ok(StopOutcome::Stopped),
            Some("slow") => Ok(StopOutcome::Stopping),
            Some("gone") => Ok(StopOutcome::Gone),
            Some("unknown") => Ok(StopOutcome::NoAnswer),
            Some("changed") => {
                let again = self.survey().await?;
                Ok(again
                    .old_daemons()
                    .into_iter()
                    .find(|d| d.file == seen.file)
                    .map_or(StopOutcome::Gone, StopOutcome::Changed))
            }
            Some("current") => Err(InstallError::Protocol(
                "refused to stop this desktop's own relay version".into(),
            )),
            _ => Err(InstallError::Protocol(
                "the remote stop did not answer".into(),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binary(file: &str, state: BinaryState) -> RemoteBinary {
        RemoteBinary {
            build: parse_name(file),
            file: file.to_owned(),
            state,
        }
    }

    fn stopped(file: &str) -> RemoteBinary {
        binary(file, BinaryState::Stopped)
    }

    #[test]
    fn names_read_as_channel_and_version() {
        assert_eq!(
            parse_name("pitwall-relay-dev-1.2.3"),
            Some(("dev".into(), semver::Version::new(1, 2, 3)))
        );
        assert_eq!(
            parse_name("pitwall-relay-stable-1.0.0-beta.1").map(|(c, v)| (c, v.to_string())),
            Some(("stable".into(), "1.0.0-beta.1".into()))
        );
        for bad in [
            "pitwall-relay-dev",
            "pitwall-relay--1.0.0",
            "pitwall-relay-dev-latest",
            "pitwall-relay-1.0.0",
            "state.db",
        ] {
            assert_eq!(parse_name(bad), None, "{bad}");
        }
    }

    /// ③ 이 판과 나머지 가운데 가장 높은 판 하나를 남긴다 — 판은 semver 로 견준다(글자 차례면 1.9 가 1.10 보다 높다).
    #[test]
    fn the_newest_other_version_stays_and_the_rest_go() {
        let survey = Survey {
            binaries: vec![
                binary("pitwall-relay-dev-1.10.0", BinaryState::Current),
                stopped("pitwall-relay-dev-1.9.0"),
                stopped("pitwall-relay-dev-1.2.0"),
                stopped("pitwall-relay-stable-1.1.0"),
            ],
        };
        assert_eq!(
            survey.to_remove(),
            ["pitwall-relay-dev-1.2.0", "pitwall-relay-stable-1.1.0"]
        );
        // 1.10.0 이 1.9.0 보다 높다.
        let survey = Survey {
            binaries: vec![
                binary("pitwall-relay-dev-2.0.0", BinaryState::Current),
                stopped("pitwall-relay-dev-1.9.0"),
                stopped("pitwall-relay-dev-1.10.0"),
            ],
        };
        assert_eq!(survey.to_remove(), ["pitwall-relay-dev-1.9.0"]);
        // 판을 낮춘 Desktop: 이 판(1.0.0)과 더 높은 것 가운데 가장 높은 하나.
        let survey = Survey {
            binaries: vec![
                binary("pitwall-relay-dev-1.0.0", BinaryState::Current),
                stopped("pitwall-relay-dev-2.0.0"),
                stopped("pitwall-relay-dev-3.0.0"),
            ],
        };
        assert_eq!(survey.to_remove(), ["pitwall-relay-dev-2.0.0"]);
    }

    /// 두 판 이하면 지울 것이 없다. 판을 읽지 못하는 이름 · 살아 있는 올리기는 남고, 셸이 끝난 올리기는 지운다.
    #[test]
    fn unreadable_names_and_live_uploads_stay() {
        let survey = Survey {
            binaries: vec![
                binary("pitwall-relay-dev-2.0.0", BinaryState::Current),
                stopped("pitwall-relay-dev-1.0.0"),
                stopped("pitwall-relay-dev-latest"),
                binary(
                    "pitwall-relay-dev-2.0.0.part.77",
                    BinaryState::Upload { live: true },
                ),
                binary(
                    "pitwall-relay-dev-2.0.0.part.78",
                    BinaryState::Upload { live: false },
                ),
            ],
        };
        assert_eq!(survey.to_remove(), ["pitwall-relay-dev-2.0.0.part.78"]);
    }

    #[test]
    fn a_survey_reads_its_lines_and_needs_the_end_mark() {
        let out = b"motd noise\n\
            pitwall-survey current pitwall-relay-dev-2.0.0\n\
            pitwall-survey running pitwall-relay-dev-1.0.0 {\"version\":\"1.0.0\",\"releaseChannel\":\"dev\",\"name\":null,\"pid\":42,\"uptimeMs\":1,\"links\":1,\"ownerLinks\":1,\"runs\":2,\"terminals\":1,\"later\":true}\n\
            pitwall-survey stopped pitwall-relay-dev-0.9.0 3\n\
            pitwall-survey stopped pitwall-relay-dev-0.7.0 4\n\
            pitwall-survey upload pitwall-relay-dev-2.0.0.part.9\n\
            pitwall-survey stopped pitwall-relay-dev-0.8.0;rm 3\n\
            pitwall-survey end\n";
        let survey = Survey::parse(out).unwrap();
        assert_eq!(survey.binaries.len(), 5, "{survey:?}");
        assert_eq!(
            survey
                .unknown()
                .iter()
                .map(|b| b.file.as_str())
                .collect::<Vec<_>>(),
            ["pitwall-relay-dev-0.7.0"],
            "exit 4 is not \"no daemon\""
        );
        let old = survey.old_daemons();
        assert_eq!(old.len(), 1);
        assert_eq!(old[0].file, "pitwall-relay-dev-1.0.0");
        assert_eq!(old[0].status.pid, 42);
        assert_eq!(old[0].left(), 3);
        assert_eq!(survey.binaries[2].state, BinaryState::Stopped);
        assert!(Survey::parse(b"pitwall-survey current pitwall-relay-dev-2.0.0\n").is_err());
    }

    /// 판에 묶이지 않는 꼴: `pid` 만 있으면 읽힌다(칸이 줄거나 는 옛 · 새 판).
    #[test]
    fn a_minimal_status_line_is_enough() {
        let status: DaemonStatus = serde_json::from_str(r#"{"pid":7}"#).unwrap();
        assert_eq!(status.pid, 7);
        assert_eq!((status.runs, status.terminals), (0, 0));
        assert!(serde_json::from_str::<DaemonStatus>(r#"{"runs":1}"#).is_err());
    }

    /// 스크립트는 로그인 셸이 무엇이든 같게 읽힌다(`ssh::check_script` — 홑따옴표 · 역빗금 · `!` · 줄바꿈 없음).
    #[test]
    fn the_scripts_pass_the_script_check() {
        for script in [SURVEY, CLEAN, STOP] {
            crate::ssh::remote_command(script, &["dev", "1.0.0"]).unwrap();
        }
    }
}

#[cfg(all(test, unix))]
#[path = "upgrade_tests.rs"]
mod fake_remote_tests;
