//! `git.blame`(스트림). UI 전용이라 MCP 툴이 아니다(결정 M2-D7 — 스트림 메서드는 툴이 될 수 없다). 툴 `git_blame`
//! (`crate::tools::blame`)은 같은 엔진([`prepare`] · [`run`])을 부르고 청크를 모아 줄 범위로 자른다 — 엔진 고르기(신뢰 → git,
//! 미신뢰 → gix)는 여기 한 곳이다.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use pw_rpc::{Router, RpcError, StreamSink, rpc_method};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use super::{BlameChunk, BlameEngine, BlameFile, Subject, incremental, native, subject};
use crate::cli::{Access, GitCommand};
use crate::{Git, Opened};

/// 받는 쪽이 읽기를 기다리는 청크 수(그 뒤로는 만드는 쪽이 멈춰 기다린다).
const PENDING_CHUNKS: usize = 2;

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitBlameParams {
    pub project_id: String,
    /// 활성 워크트리의 꼭대기(`git.repo` 와 같다).
    #[serde(default)]
    pub worktree: Option<String>,
    /// 저장소 꼭대기 기준 파일 경로(`/` 구분 — Windows 는 `\` 도 받는다).
    pub path: String,
    /// 리비전(git 의 리비전 문법 — 「이 커밋 이전으로」 는 범위의 `previous` 커밋). 없으면 HEAD.
    #[serde(default)]
    pub rev: Option<String>,
}

rpc_method! {
    /// 파일의 줄마다 마지막으로 바꾼 커밋(F4-7 · F5-17). 범위마다 커밋 · 그 커밋에서의 경로(이름 바뀜 추적) · 「이 커밋
    /// 이전으로」 의 부모와 경로, 커밋마다 작성자 · 시각 · 제목(처음 나올 때 한 번). 첫 청크에 그 리비전의 해시 · 줄 수 · 엔진.
    /// 작업 트리 · 저장하지 않은 편집은 blame 하지 않는다. 저장소가 없거나 HEAD 가 태어나지 않았거나 그 리비전에 파일이 없으면
    /// 청크 없이 끝난다. 없는 프로젝트 · 워크트리, 잘못된 경로 · 리비전, 폴더 · 서브모듈 · 50 MiB 넘는 파일은 invalid params(엔진 `gix` 는 이력의 버전이 50 MiB 를 넘어도). 범위의 경로가
    /// UTF-8 이 아니면 그때까지의 청크를 보낸 뒤 invalid params 로 끝난다(경로를 U+FFFD 로 바꿔 싣지 않는다).
    pub GitBlameMethod: "git.blame" {
        params: GitBlameParams,
        stream: BlameChunk,
        tier: Read,
        roles: [Owner],
    }
}

/// 핸들러를 붙인다(`crate::methods::register` 가 부른다).
pub fn register(router: &mut Router, git: Arc<Git>) {
    router.stream(GitBlameMethod, move |_ctx, params: GitBlameParams, sink| {
        let git = Arc::clone(&git);
        async move { stream(git, params, &sink).await }
    });
}

async fn stream(
    git: Arc<Git>,
    params: GitBlameParams,
    sink: &StreamSink<BlameChunk>,
) -> Result<(), RpcError> {
    let Some((opened, subject)) = prepare(&git, params).await? else {
        return Ok(());
    };
    run(&git, opened, subject, sink).await
}

/// blame 청크를 받는 쪽: 스트림(`git.blame`)과 툴(`git_blame` 이 모은다).
pub(crate) trait BlameOut: Sync {
    /// 보내지 못하면(받는 쪽이 사라짐) 그 오류.
    fn send(&self, chunk: BlameChunk) -> impl Future<Output = Result<(), RpcError>> + Send;
}

impl BlameOut for StreamSink<BlameChunk> {
    fn send(&self, chunk: BlameChunk) -> impl Future<Output = Result<(), RpcError>> + Send {
        StreamSink::send(self, chunk)
    }
}

/// 저장소를 열고 blame 할 대상을 푼다(블로킹 스레드). 저장소가 없거나 HEAD 가 태어나지 않았거나 그 리비전에 파일이 없으면 `None`.
pub(crate) async fn prepare(
    git: &Arc<Git>,
    params: GitBlameParams,
) -> Result<Option<(Opened, Subject)>, RpcError> {
    let git = Arc::clone(git);
    tokio::task::spawn_blocking(move || -> Result<Option<(Opened, Subject)>, RpcError> {
        let Some(opened) = git.open(&params.project_id, params.worktree.as_deref())? else {
            return Ok(None);
        };
        let found = subject(&opened.repo, params.rev.as_deref(), &params.path)?;
        Ok(found.map(|s| (opened, s)))
    })
    .await
    .map_err(|e| RpcError::internal(e.to_string()))?
}

/// `subject` 를 blame 해 청크를 `out` 으로 보낸다. 엔진은 신뢰로 고른다(모듈 머리 주석): 신뢰한 프로젝트는 git CLI, 미신뢰 ·
/// git 을 찾지 못하면 gix.
pub(crate) async fn run(
    git: &Git,
    opened: Opened,
    subject: Subject,
    out: &impl BlameOut,
) -> Result<(), RpcError> {
    // 연 뒤에 신뢰를 거뒀으면 미신뢰로 본다(`Git::command` 도 같은 판정으로 거절한다).
    let trusted = opened.trusted && git.is_current(&opened);
    if trusted {
        match git.command(&opened.target, Access::Read).await {
            Ok(command) => return cli(command, &subject, out).await,
            Err(reason) => {
                tracing::info!(target: "pw_git", ?reason, "cannot run `git blame`; blaming with gix");
            }
        }
    }
    gix(opened, subject, trusted, out).await
}

/// `git blame --incremental -M`. 받는 쪽이 사라지면(스트림이 끝나 이 future 가 버려지면) git 의 프로세스 그룹을 끝낸다.
async fn cli(command: GitCommand, subject: &Subject, out: &impl BlameOut) -> Result<(), RpcError> {
    let mut running = incremental::command(command, subject)
        .spawn()
        .map_err(|e| RpcError::internal(e.to_string()))?;
    let stdout = running
        .stdout()
        .ok_or_else(|| RpcError::internal("git blame has no stdout"))?;
    let file = BlameFile {
        commit: subject.commit.to_string(),
        lines: subject.lines,
        engine: BlameEngine::Git,
        moves: incremental::moves(subject),
    };
    let mut to_sink = ToSink { out, gone: None };
    let Some(drained) = incremental::pump(stdout, file, &mut to_sink).await? else {
        return Err(to_sink.gone.unwrap_or_else(RpcError::cancelled));
    };
    let output = running
        .wait(&CancellationToken::new())
        .await
        .map_err(|e| RpcError::internal(e.to_string()))?;
    if !output.status.success() {
        return Err(RpcError::internal(format!(
            "git blame failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    if let Some(chunk) = drained.finish()? {
        out.send(chunk).await?;
    }
    Ok(())
}

/// 받는 쪽으로 보낸다. 보내지 못하면(연결이 닫힘) 그 오류를 쥔다.
struct ToSink<'a, O> {
    out: &'a O,
    gone: Option<RpcError>,
}

impl<O: BlameOut> incremental::Emit for ToSink<'_, O> {
    async fn emit(&mut self, chunk: BlameChunk) -> bool {
        match self.out.send(chunk).await {
            Ok(()) => true,
            Err(e) => {
                self.gone = Some(e);
                false
            }
        }
    }
}

/// gix blame(블로킹 스레드). 받는 쪽이 사라져 이 future 가 버려지면 멈춤 신호를 켠다.
async fn gix(
    opened: Opened,
    subject: Subject,
    trusted: bool,
    out: &impl BlameOut,
) -> Result<(), RpcError> {
    let stop = Arc::new(AtomicBool::new(false));
    let _stop_on_drop = StopOnDrop(Arc::clone(&stop));
    let (tx, mut rx) = tokio::sync::mpsc::channel::<BlameChunk>(PENDING_CHUNKS);
    let worker = tokio::task::spawn_blocking(move || {
        native::run(&opened.repo, trusted, &subject, &stop, &mut |chunk| {
            tx.blocking_send(chunk).is_ok()
        })
        .map(drop)
    });
    while let Some(chunk) = rx.recv().await {
        out.send(chunk).await?;
    }
    worker
        .await
        .map_err(|e| RpcError::internal(e.to_string()))?
}

/// 버려질 때 멈춤 신호를 켠다.
pub(crate) struct StopOnDrop(pub(crate) Arc<AtomicBool>);

impl Drop for StopOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}
