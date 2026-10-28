//! `git.fileHistory`(스트림). UI 전용이라 MCP 툴이 아니다(결정 M2-D7 — 스트림 메서드는 툴이 될 수 없다).

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use pw_rpc::{Router, RpcError, StreamSink, rpc_method};
use serde::{Deserialize, Serialize};

use super::{FileHistoryChunk, check_start, run};
use crate::Git;
use crate::blame::methods::StopOnDrop;

/// 받는 쪽이 읽기를 기다리는 청크 수(그 뒤로는 걷기 스레드가 멈춰 기다린다).
const PENDING_CHUNKS: usize = 2;
/// 걷는 동안 객체 캐시 크기(트리를 경로마다 다시 읽는다 — 이웃 커밋은 하위 트리를 함께 쓴다).
const OBJECT_CACHE: usize = 16 * 1024 * 1024;

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitFileHistoryParams {
    pub project_id: String,
    /// 활성 워크트리의 꼭대기(`git.repo` 와 같다).
    #[serde(default)]
    pub worktree: Option<String>,
    /// 저장소 꼭대기 기준 파일 경로(`/` 구분 — Windows 는 `\` 도 받는다). 그 리비전에 없는 경로(지운 파일)도 따라간다.
    pub path: String,
    /// 시작 리비전(git 의 리비전 문법). 없으면 HEAD.
    #[serde(default)]
    pub rev: Option<String>,
}

rpc_method! {
    /// 이름 바뀜을 따라가는 파일 히스토리(F5-16 — `git log --follow --name-status` 와 같은 커밋 · 순서 · 상태). 행마다 해시 ·
    /// 부모 · 제목 · 작성자 · 시각 · 그 커밋에서의 경로 · 바뀜(A · M · D · R · C · T)과 이름 바뀜의 원래 경로. 첫 청크는 200행
    /// 이하, 그 뒤는 500행 이하이고 모인 행은 100 ms 안에 보낸다. 저장소가 없거나 HEAD 가 태어나지 않았으면 청크 없이 끝난다.
    /// 없는 프로젝트 · 워크트리, 잘못된 경로 · 리비전, 시작 리비전에서 폴더인 경로는 invalid params. 시작 리비전에서 서브모듈 안의
    /// 경로면 invalid params 이고 `data` 가 `{ detail, submodule }`(서브모듈 자리, 저장소 꼭대기 기준)이다. 걷다가 UTF-8 이 아닌
    /// 경로가 나오면 그때까지의 행을 보낸 뒤 invalid params 로 끝난다(경로를 U+FFFD 로 바꿔 싣지 않는다).
    pub GitFileHistoryMethod: "git.fileHistory" {
        params: GitFileHistoryParams,
        stream: FileHistoryChunk,
        tier: Read,
        roles: [Owner],
    }
}

/// 핸들러를 붙인다(`crate::methods::register` 가 부른다).
pub fn register(router: &mut Router, git: Arc<Git>) {
    router.stream(
        GitFileHistoryMethod,
        move |_ctx, params: GitFileHistoryParams, sink| {
            let git = Arc::clone(&git);
            async move { stream(git, params, &sink).await }
        },
    );
}

async fn stream(
    git: Arc<Git>,
    params: GitFileHistoryParams,
    sink: &StreamSink<FileHistoryChunk>,
) -> Result<(), RpcError> {
    let stop = Arc::new(AtomicBool::new(false));
    // 스트림이 끝나 이 future 가 버려지면 걷기도 멈춘다.
    let _stop_on_drop = StopOnDrop(Arc::clone(&stop));
    let (tx, mut rx) = tokio::sync::mpsc::channel::<FileHistoryChunk>(PENDING_CHUNKS);
    let worker = tokio::task::spawn_blocking(move || {
        let path = crate::log::filter::relative_path(&params.path)?;
        let Some(mut opened) = git.open(&params.project_id, params.worktree.as_deref())? else {
            return Ok(());
        };
        let Some(start) = crate::blame::start_commit(&opened.repo, params.rev.as_deref())? else {
            return Ok(());
        };
        check_start(&opened.repo, start, path.as_ref())?;
        opened.repo.object_cache_size_if_unset(OBJECT_CACHE);
        run(
            &opened.repo,
            opened.trusted,
            start,
            path.as_ref(),
            &stop,
            &mut |chunk| tx.blocking_send(chunk).is_ok(),
        )
        .map(drop)
    });
    while let Some(chunk) = rx.recv().await {
        sink.send(chunk).await?;
    }
    worker
        .await
        .map_err(|e| RpcError::internal(e.to_string()))?
}
