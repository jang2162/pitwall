//! 실행의 종류 「프로세스」 — PTY 로 띄운 프로그램(메인 판정 M6-P2). 출력은 실행의 링 버퍼로 곧바로 옮긴다(소비자를 기다리지
//! 않는다 — 버퍼가 차면 앞을 버리므로 프로그램이 쓰기에서 멈추지 않는다).
//!
//! 끝: 프로그램(세션 리더)이 끝나면 남은 출력을 받고, 세션에 남은 프로세스(리더가 띄우고 두고 간 것)를 같은 중지 방법으로 끝낸 뒤
//! 「끝났다」 를 알린다 — 실행이 끝났는데 그 자식이 포트를 쥐고 남는 일이 없다. 중지는 세션 전체에 [`Ending`] 의 신호를 보낸다
//! (`crate::tree::end_sessions` — 그룹이 아니라 세션 id 로 찾으므로 자식이 따로 만든 프로세스 그룹도 닿는다).

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use crate::pty::{CHUNK, Ending, Pty, Spec};

use super::{RunExit, RunKind, RunOutput};

/// [`RunKind::kind`].
pub const KIND: &str = "process";

/// 리더가 끝난 뒤 남은 출력을 기다리는 시간(터미널의 `DRAIN_AFTER_EXIT` 와 같다). 세션을 떠난 데몬이 PTY 를 쥐고 있으면 EOF 가
/// 오지 않는다.
const DRAIN_AFTER_EXIT: Duration = Duration::from_millis(200);

/// 띄울 프로세스.
#[derive(Clone)]
pub struct ProcessSpec {
    /// 첫째가 프로그램(맨 이름이면 `env` 의 `PATH` 로 찾는다).
    pub argv: Vec<String>,
    pub cwd: PathBuf,
    /// 자식의 환경 전체.
    pub env: Vec<(OsString, OsString)>,
    pub cols: u16,
    pub rows: u16,
    /// 중지의 방법.
    pub ending: Ending,
}

/// argv 원문 · env 값을 보이지 않는다(감사 · 로그에 명령줄 원문과 env 값을 남기지 않는다 — M5 의 규약).
impl std::fmt::Debug for ProcessSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProcessSpec")
            .field("argc", &self.argv.len())
            .field("cwd", &self.cwd)
            .field(
                "envKeys",
                &self.env.iter().map(|(k, _)| k).collect::<Vec<_>>(),
            )
            .field("size", &(self.cols, self.rows))
            .field("ending", &self.ending)
            .finish()
    }
}

/// 프로세스를 띄우는 것. 시험은 가짜를 넣어 불린 수를 센다(미신뢰 프로젝트는 0).
pub trait ProcessLauncher: Send + Sync + 'static {
    /// 띄우고 출력 · 끝을 `output` 으로 보낸다. tokio 런타임 안에서 불린다.
    fn launch(&self, spec: &ProcessSpec, output: RunOutput) -> Result<Arc<dyn RunKind>, String>;
}

/// PTY 로 띄운다.
pub struct PtyLauncher;

impl ProcessLauncher for PtyLauncher {
    fn launch(&self, spec: &ProcessSpec, output: RunOutput) -> Result<Arc<dyn RunKind>, String> {
        let runtime = tokio::runtime::Handle::try_current()
            .map_err(|_| "runs must be started inside the async runtime".to_owned())?;
        let pty = Arc::new(Pty::spawn(&Spec {
            argv: spec.argv.iter().map(OsString::from).collect(),
            cwd: spec.cwd.clone(),
            env: spec.env.clone(),
            cols: spec.cols,
            rows: spec.rows,
        })?);
        let process = Arc::new(ProcessRun {
            pid: pty.pid(),
            pty: Mutex::new(Some(Arc::clone(&pty))),
            ending: spec.ending,
        });
        runtime.spawn(pump(pty, Arc::clone(&process), output));
        Ok(process)
    }
}

struct ProcessRun {
    pid: Option<u32>,
    /// 끝나면 비운다 — 끝난 실행이 목록에 남는 동안 PTY 의 fd 를 쥐지 않게.
    pty: Mutex<Option<Arc<Pty>>>,
    ending: Ending,
}

impl ProcessRun {
    fn pty(&self) -> Option<Arc<Pty>> {
        self.pty
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl RunKind for ProcessRun {
    fn kind(&self) -> &'static str {
        KIND
    }

    fn pid(&self) -> Option<u32> {
        self.pid
    }

    fn input(&self, data: Vec<u8>) -> Result<(), String> {
        if self.pty().is_some_and(|pty| pty.write(data)) {
            Ok(())
        } else {
            Err("the run is not running".to_owned())
        }
    }

    fn resize(&self, cols: u16, rows: u16) -> Result<(), String> {
        match self.pty() {
            Some(pty) => pty.resize(cols, rows),
            None => Ok(()),
        }
    }

    fn stop(&self) {
        if let Some(pty) = self.pty() {
            pty.end(self.ending);
        }
    }
}

/// 출력을 끝까지 옮기고 끝을 알린다(머리 주석 「끝」).
async fn pump(pty: Arc<Pty>, process: Arc<ProcessRun>, output: RunOutput) {
    let mut exited = false;
    loop {
        let next = if exited {
            tokio::time::timeout(DRAIN_AFTER_EXIT, pty.peek(CHUNK))
                .await
                .unwrap_or(None)
        } else {
            tokio::select! {
                next = pty.peek(CHUNK) => next,
                _ = pty.exited() => {
                    exited = true;
                    continue;
                }
            }
        };
        let Some(bytes) = next else {
            break;
        };
        output.write(&bytes);
        pty.consume(bytes.len());
    }
    let exit = pty.exit().await;
    // 리더가 끝났으면 세션에 남은 것도 끝낸다. 중지 중이면 이미 시작한 끝내기가 이어진다(두 번째 `end` 는 아무것도 하지 않는다).
    pty.end(process.ending);
    pty.ended().await;
    // 세션을 떠난 데몬이 PTY 를 쥐고 있으면 읽기 스레드가 EOF 를 못 본다 — 닫아 그 스레드도 끝낸다.
    pty.close_output();
    process
        .pty
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take();
    drop(pty);
    output.exited(RunExit {
        code: exit.code,
        signal: exit.signal,
    });
}
