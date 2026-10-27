//! 커밋(13-m2-plan.md M2-08 — F5-7): `git.commit`. UI 전용이라 MCP 툴이 아니다(결정 M2-D7 — git 쓰기는 M5 에서도 툴이 아니다).
//!
//! - 쓰기 실행기([`GitWrite::run`])로 `git commit --quiet -F -` 를 돌린다. 메시지는 stdin 으로 넘긴다 — 인자 길이 · 따옴표와
//!   상관없고, 진행(`git commit`)에 메시지가 보이지 않는다.
//! - 훅(pre-commit · commit-msg 등)은 git 이 하는 대로 런타임 환경에서 돈다(결정 M2-D8 — 신뢰한 프로젝트는 mise 등 제공자의 환경,
//!   미신뢰 프로젝트는 로그인 셸만). git 은 훅의 stdout 을 stderr 로 돌리므로, `--quiet` 로 git 자신의 요약을 끄면 stderr 가 훅과
//!   git 경고의 원문이다. 성공이면 결과의 `output`, 실패면 Task 의 오류(창의 실패 알림 body)와 RPC 오류의 `data.stderr` 다
//!   (`crate::write` 머리 주석 4).
//! - `commit.gpgsign` · `commit.cleanup` 은 git 설정을 따른다 — 인자로 덮지 않는다. `commit.template` 은 git 이 편집기를 띄울 때만
//!   쓰므로 `-F` 로 메시지를 주는 이 길에서는 쓰이지 않는다 — 템플릿을 메시지 칸에 미리 채우는 일은 M2-16 이 한다.
//! - index 를 커밋할 뿐 작업 트리를 바꾸지 않으므로 미저장 확인을 묻지 않는다([`Changes::None`]). 원격에 닿지 않으므로 askpass 도
//!   넣지 않는다 — 저장소의 훅이 암호 다이얼로그를 띄우지 못한다.
//! - 결과의 `moved` 는 HEAD 의 ref 를 커밋 전 → 뒤로 옮긴 기록이다([`GitRefMove`], M2-10 과 같은 꼴). amend 는 ref 를 옮기므로
//!   창이 그것으로 [되돌리기] 를 단다(결정 M2-D6 ③, 되돌리는 법은 `crate::history` 머리 주석).
//!
//! 메시지 히스토리는 창의 프로젝트 상태(`apps/desktop/src/layout/project-state.ts` → `state.json`)에 있고, amend 가 미리 채우는 직전
//! 메시지는 `git.show { rev: "HEAD" }` 의 `message` 다 — 이 모듈은 둘 다 다루지 않는다.

#[cfg(all(test, unix))]
mod tests;

use std::sync::Arc;

use pw_rpc::{Router, RpcError, rpc_method};
use serde::{Deserialize, Serialize};

use crate::Opened;
use crate::history::{GitRefMove, head_ref_name};
use crate::write::{Changes, GitWrite, WriteError, WriteRequest};

/// Task 제목(`extensions/i18n-default/i18n/*/project.ftl`).
pub const TITLE: &str = "task-title-git-commit";

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitCommitParams {
    pub project_id: String,
    /// 활성 워크트리의 꼭대기(`git.repo` 와 같다).
    #[serde(default)]
    pub worktree: Option<String>,
    /// 커밋 메시지 그대로. 정리(주석 줄 · 빈 줄)는 git 의 `commit.cleanup` 을 따른다. 비었으면 git 이 「빈 메시지」 로 거절한다.
    pub message: String,
    /// HEAD 커밋을 이 메시지 · 지금 index 로 바꾼다(`--amend`). 부모는 그대로다.
    #[serde(default)]
    pub amend: bool,
    /// `Signed-off-by:` 트레일러를 붙인다(`--signoff`, 신원은 git 설정).
    #[serde(default)]
    pub signoff: bool,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitCommitResult {
    /// 만든 커밋(새 HEAD)의 해시.
    pub commit: String,
    /// HEAD 의 ref(브랜치, 분리돼 있으면 `HEAD`)를 커밋 전 → 뒤로 옮긴 기록. 아직 커밋이 없던 브랜치면 `previous` 가 `null`.
    /// amend 뒤 [되돌리기] 는 `previous` 로 돌아간다.
    pub moved: GitRefMove,
    /// 저장소의 작업 트리 꼭대기(절대 경로).
    pub work_tree: String,
    /// git 과 훅이 stderr 에 낸 원문(훅의 stdout 도 git 이 stderr 로 돌린다). 없으면 빈 문자열.
    pub output: String,
}

rpc_method! {
    /// index 를 커밋한다(`git commit -F -`, 메시지는 stdin). 훅 · 서명 · 정리는 사람의 git 설정대로 돌고, 훅이 실패하면 커밋하지
    /// 않고 git 오류(`2004`, `data.stderr` 에 훅의 출력)로 끝난다. 성공해도 훅의 출력은 `output` 에 있다.
    pub GitCommitMethod: "git.commit" {
        params: GitCommitParams,
        result: GitCommitResult,
        tier: Write,
        roles: [Owner],
    }
}

/// `git.commit`(머리 주석).
pub async fn commit(
    write: &GitWrite,
    params: GitCommitParams,
) -> Result<GitCommitResult, WriteError> {
    let GitCommitParams {
        project_id,
        worktree,
        message,
        amend,
        signoff,
    } = params;
    let request = WriteRequest {
        project_id,
        worktree,
        title: TITLE.to_owned(),
        changes: Changes::None,
        askpass: false,
    };
    write
        .run(request, move |ctx| async move {
            // 잠금 안에서 읽는다 — 앞 쓰기를 기다리는 사이 HEAD 가 움직였을 수 있다.
            let (ref_name, previous) = ctx
                .read(|opened| head_ref_name(&opened.repo).map(|name| (name, head(opened))))
                .await?
                .map_err(WriteError::Internal)?;
            let mut command = ctx.git().args(["commit", "--quiet"]);
            if amend {
                command = command.arg("--amend");
            }
            if signoff {
                command = command.arg("--signoff");
            }
            let output = ctx
                .run(command.args(["-F", "-"]).stdin(message.into_bytes()))
                .await?;
            let commit = ctx.read(head).await?.ok_or_else(|| {
                WriteError::Internal("HEAD has no commit after `git commit` succeeded".into())
            })?;
            Ok(GitCommitResult {
                commit: commit.clone(),
                moved: GitRefMove {
                    ref_name,
                    previous,
                    current: Some(commit),
                },
                work_tree: crate::status::display(ctx.work_tree()),
                output: String::from_utf8_lossy(&output.stderr).into_owned(),
            })
        })
        .await
}

/// HEAD 가 가리키는 커밋. 태어나지 않은 브랜치면 `None`.
fn head(opened: &Opened) -> Option<String> {
    opened.repo.head_id().ok().map(|id| id.to_string())
}

/// 핸들러를 붙인다.
pub fn register(router: &mut Router, write: Arc<GitWrite>) {
    router.unary(GitCommitMethod, move |_ctx, params: GitCommitParams| {
        let write = Arc::clone(&write);
        async move { commit(&write, params).await.map_err(RpcError::from) }
    });
}
