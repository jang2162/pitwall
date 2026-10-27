//! diff · 커밋 상세 RPC(`docs/design/13-m2-plan.md` M2-04). UI 전용이라 MCP 툴이 아니다(결정 M2-D7). 입력은 struct 이고
//! `projectId` 를 담는다 — M5 가 조회 툴로 켤 수 있는 꼴이다.

use std::sync::Arc;

use pw_fs::judge::Judgment;
use pw_fs::methods::FsContent;
use pw_rpc::{Router, RpcError, rpc_method};
use serde::{Deserialize, Serialize};

use crate::Git;

/// 커밋 하나를 고르는 요청(`git.show` · `git.containedIn`).
#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitRevParams {
    pub project_id: String,
    /// 활성 워크트리의 꼭대기(`ActiveWorktree.top`, 절대 경로).
    #[serde(default)]
    pub worktree: Option<String>,
    /// 리비전(해시 · 브랜치 · 태그 · `HEAD~1` 처럼 git 의 리비전 문법). 커밋이 아니면(트리 · blob) invalid params.
    pub rev: String,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitShow {
    pub commit: String,
    pub tree: String,
    /// 부모 해시. 머지 커밋은 둘 이상이고 첫째가 첫 부모다. 루트 커밋과 얕은 클론의 경계 커밋([`Self::shallow`])은 비어 있다.
    pub parents: Vec<String>,
    /// 얕은 클론(`git clone --depth`)의 경계 커밋이다 — 부모가 저장소에 없다. git 처럼 부모가 없는 것으로 다룬다(`parents` 가
    /// 비고, `git.diff` 는 빈 트리와 견준다).
    pub shallow: bool,
    pub author: Signature,
    pub committer: Signature,
    /// 메시지 전체(제목 줄 · 본문 · 트레일러). UTF-8 이 아닌 바이트는 U+FFFD 다.
    pub message: String,
    /// HEAD 가 이 커밋이다(브랜치를 거쳐서든 분리된 HEAD 든).
    pub head: bool,
    /// 이 커밋을 가리키는 브랜치 · 원격 브랜치 · 태그(주석 태그는 벗긴 대상). 종류 순(로컬 · 원격 · 태그), 같은 종류는 이름순.
    pub refs: Vec<RefLabel>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct Signature {
    pub name: String,
    pub email: String,
    /// 유닉스 시각(초).
    #[specta(type = pw_rpc::JsNumber)]
    pub time: i64,
    /// 그 시각의 UTC 차이(초, 동쪽이 양수). `+0900` 이면 32400.
    pub offset: i32,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct RefLabel {
    /// 짧은 이름(`main` · `origin/main` · `v1.0`).
    pub name: String,
    /// 전체 이름(`refs/heads/main`).
    pub full_name: String,
    pub kind: RefKind,
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Serialize,
    Deserialize,
    specta::Type,
    schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum RefKind {
    LocalBranch,
    RemoteBranch,
    Tag,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitContainedIn {
    /// 그 커밋을 조상(자신 포함)으로 가진 브랜치 · 원격 브랜치 · 태그. 순서는 [`GitShow::refs`] 와 같다.
    pub refs: Vec<RefLabel>,
}

/// diff 의 한쪽.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum DiffSide {
    /// 커밋의 트리. `rev` 는 [`GitRevParams::rev`] 와 같은 리비전 문법이다.
    Commit { rev: String },
    /// index(스테이징 영역). 충돌 중인 경로(단계 1~3)는 빠진다.
    Index,
    /// 작업 트리. 추적하는 파일만 본다(`git diff` 와 같이 untracked 는 빠진다).
    WorkTree,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitDiffParams {
    pub project_id: String,
    /// 활성 워크트리의 꼭대기(`ActiveWorktree.top`, 절대 경로).
    #[serde(default)]
    pub worktree: Option<String>,
    /// 옛 쪽. 고를 수 있는 짝은 넷이다(결정 M2-D5):
    /// - `to` 가 커밋: `from` 이 없으면 그 커밋의 첫 부모(머지 커밋도 첫 부모, 루트 커밋은 빈 트리), 있으면 그 커밋(두 커밋 비교).
    /// - `to` 가 index: `from` 은 커밋(없으면 `HEAD`, 커밋이 없는 저장소면 빈 트리) — `git diff --cached`.
    /// - `to` 가 작업 트리: `from` 은 index(없어도 index) — `git diff`.
    ///
    /// 그 밖의 짝은 invalid params.
    #[serde(default)]
    pub from: Option<DiffSide>,
    pub to: DiffSide,
    /// 이 경로(저장소 꼭대기 기준, `/` 구분)와 그 아래만 본다. 글롭 · pathspec 문법은 쓰지 않는다(글자 그대로). 이름 바뀜은 거른
    /// 경로들 사이에서만 찾는다(`git diff -- <경로>` 와 같다).
    #[serde(default)]
    pub paths: Option<Vec<String>>,
    /// 공백을 모두 무시한다(`git diff -w`). 공백만 바뀐 파일(이름 · 모드는 그대로)은 목록에서 빠진다.
    #[serde(default)]
    pub ignore_whitespace: bool,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitDiff {
    /// 저장소의 작업 트리 꼭대기(절대 경로, `git.repo` 의 `workTree` 와 같은 꼴). 파일 경로는 이 아래의 상대 경로다.
    pub work_tree: String,
    /// 옛 쪽이 커밋이면 그 해시(머지 커밋의 첫 부모처럼 고른 것 포함). 빈 트리 · index 면 `null`.
    pub old_commit: Option<String>,
    /// 새 쪽이 커밋이면 그 해시.
    pub new_commit: Option<String>,
    /// 경로순(이름이 바뀐 것은 새 경로 자리).
    pub files: Vec<DiffFile>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DiffFile {
    pub status: FileStatus,
    /// 새 경로(지워졌으면 옛 경로). 작업 트리 꼭대기([`GitDiff::work_tree`]) 기준 상대 경로, `/` 구분(git porcelain 과 같다 —
    /// Windows 에서도).
    pub path: String,
    /// 이름이 바뀌었으면 옛 경로.
    pub old_path: Option<String>,
    /// git 의 8진 모드(`100644` · `100755` · `120000` 심링크 · `160000` 서브모듈). 그쪽에 없으면 `null`.
    pub old_mode: Option<String>,
    pub new_mode: Option<String>,
    /// 그쪽 blob(서브모듈이면 커밋) 해시. 작업 트리 쪽은 `null`.
    pub old_id: Option<String>,
    pub new_id: Option<String>,
    /// 한쪽이라도 바이너리다(앞 8000바이트에 NUL, 또는 속성 `-diff` · `binary`). hunk 가 없다.
    pub binary: bool,
    /// 너무 커서 hunk 를 싣지 않는다 — `paths` 로 그 파일만 다시 불러도 같다. 둘 중 하나다:
    /// - 한쪽이라도 대용량 기준(결정 M1-D9 의 50 MiB)을 넘는다. 내용을 읽지 않으므로 `binary` 는 거짓이고 `added` · `removed` 는 0 이다.
    /// - hunk 의 응답 크기가 파일 하나의 상한([`super::changes::FILE_HUNKS_MAX`], 5 MiB)을 넘는다. `added` · `removed` 는 맞다.
    pub too_large: bool,
    /// 응답 전체의 hunk 크기 상한([`super::changes::DIFF_HUNKS_MAX`], 50 MiB)에 닿아 이 파일의 hunk 를 싣지 않았다. 파일 자체는
    /// 크지 않다 — `paths` 로 그 파일만 다시 부르면 받는다. `added` · `removed` 는 맞다.
    pub hunks_omitted: bool,
    /// 생성 파일이다 — `.gitattributes` 의 `linguist-generated`, 없으면 Linguist 규칙(M1-02, 내용 규칙은 새 쪽(지워졌으면 옛 쪽)
    /// 앞부분으로 본다 — 결정 M1-D20 대안 ①). 속성은 지금 작업 트리의 것(`pw_lang::attributes`)이다 — 커밋 diff 도 그 커밋의
    /// `.gitattributes` 가 아니다. UI 는 기본으로 접는다(F5-3).
    pub generated: bool,
    /// 서브모듈 포인터가 바뀌었다(한쪽이라도 `160000`). hunk 가 없다.
    pub submodule: Option<SubmodulePointer>,
    /// 더한 줄 · 뺀 줄 수(`git diff --numstat`).
    pub added: u32,
    pub removed: u32,
    pub hunks: Vec<DiffHunk>,
    /// 이 파일의 내용을 읽지 못했다(객체가 없음 · 필터 실패 · 작업 트리 파일이 루트 밖을 가리킴 · 큰 파일 차례를 못 얻음). 목록의
    /// 다른 파일은 그대로 나온다. 있으면 hunk 가 없다.
    pub error: Option<String>,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum FileStatus {
    Added,
    Deleted,
    /// 내용이나 모드(실행 비트)가 바뀌었다.
    Modified,
    /// 이름이 바뀌었다(내용 50% 이상이 같다 — `git diff --find-renames`). 내용도 바뀌었을 수 있다.
    Renamed,
    /// 종류가 바뀌었다(파일 ↔ 심링크 ↔ 서브모듈). git 의 patch 는 지우기 + 더하기 둘로 보인다.
    TypeChanged,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct SubmodulePointer {
    /// 옛 커밋. 그쪽에 서브모듈이 없으면 `null`.
    pub old: Option<String>,
    /// 새 커밋. 작업 트리 쪽이면 체크아웃된 서브모듈의 HEAD. 그쪽에 서브모듈이 없으면(지워짐 · 파일이나 링크로 바뀜) `null`.
    pub new: Option<String>,
    /// 새 쪽이 작업 트리이고 체크아웃된 서브모듈 안에 커밋하지 않은 변경이 있다(HEAD ↔ index · index ↔ 작업 트리 — `git diff` 의
    /// `Subproject commit <해시>-dirty`. untracked 파일만 있으면 git 처럼 달지 않는다). 포인터가 그대로여도 이것만으로 목록에 든다(`old == new`). 서브모듈은 바깥
    /// 프로젝트의 신뢰로 열고 안의 서브모듈은 보지 않는다. 커밋 쪽은 늘 거짓이다.
    pub dirty: bool,
}

/// `git diff -U3` 의 hunk 하나.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DiffHunk {
    /// `@@ -oldStart,oldLines +newStart,newLines @@` 의 수 그대로. 줄이 없는 쪽(`oldLines == 0`)의 시작은 그 앞 줄 번호다.
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
    /// `@@` 뒤의 함수 줄(git 의 기본 규칙). 없으면 빈 문자열.
    pub header: String,
    pub lines: Vec<DiffLine>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DiffLine {
    pub kind: LineKind,
    /// 줄바꿈(`\n`)을 뺀 줄. CRLF 파일이면 `\r` 로 끝난다. UTF-8 이 아닌 바이트는 U+FFFD 다.
    pub text: String,
    /// 1 기준 줄 번호. 문맥 줄은 둘 다, 뺀 줄은 옛 쪽만, 더한 줄은 새 쪽만 있다.
    pub old_line: Option<u32>,
    pub new_line: Option<u32>,
    /// 파일 끝 개행이 없는 마지막 줄(git 의 `\ No newline at end of file`).
    pub no_newline: bool,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum LineKind {
    Context,
    Added,
    Removed,
}

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitBlobParams {
    pub project_id: String,
    /// 활성 워크트리의 꼭대기(`ActiveWorktree.top`, 절대 경로).
    #[serde(default)]
    pub worktree: Option<String>,
    /// 리비전(커밋). `null` 이면 index(단계 0 — 스테이징된 내용).
    #[serde(default)]
    pub rev: Option<String>,
    /// 저장소 꼭대기 기준 경로(`/` 구분).
    pub path: String,
    /// 범위 읽기의 시작 바이트(`fs.read` 와 같다). 둘 다 없으면 전체.
    #[serde(default)]
    #[specta(type = Option<pw_rpc::JsNumber>)]
    pub offset: Option<u64>,
    /// 범위 읽기의 최대 바이트(50 MiB 까지). 없으면 끝까지.
    #[serde(default)]
    #[specta(type = Option<pw_rpc::JsNumber>)]
    pub length: Option<u64>,
    /// 텍스트여도 바이트(base64)로 받는다.
    #[serde(default)]
    pub bytes: bool,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitBlob {
    /// blob 해시.
    pub id: String,
    /// 8진 모드(`100644` · `100755` · `120000` — 심링크의 내용은 대상 경로).
    pub mode: String,
    /// blob 전체 크기.
    #[specta(type = pw_rpc::JsNumber)]
    pub size: u64,
    /// 대용량 기준(50 MiB)을 넘는다. 내용을 풀지 않으므로 `content` · `judgment` 가 `null` 이다(범위 읽기도 하지 않는다).
    pub too_large: bool,
    /// 읽은 범위의 시작.
    #[specta(type = pw_rpc::JsNumber)]
    pub offset: u64,
    /// 읽은 범위가 끝까지 닿았다.
    pub eof: bool,
    pub content: Option<FsContent>,
    /// `fs.read` 와 같은 판정(텍스트/바이너리 · 인코딩 · 줄바꿈 · BOM · 크기 등급).
    pub judgment: Option<Judgment>,
}

rpc_method! {
    /// 커밋 하나의 상세: 메시지 · 작성자 · 커미터 · 부모 · 그 커밋을 가리키는 ref. 담은 브랜치는 `git.containedIn`(비싸다).
    pub GitShowMethod: "git.show" {
        params: GitRevParams,
        result: GitShow,
        tier: Read,
        roles: [Owner],
    }
}

rpc_method! {
    /// 그 커밋을 담은(조상으로 가진) 브랜치 · 원격 브랜치 · 태그. 모든 ref 에서 이력을 걷는다 — 커밋 상세를 연 뒤 따로 부른다.
    pub GitContainedInMethod: "git.containedIn" {
        params: GitRevParams,
        result: GitContainedIn,
        tier: Read,
        roles: [Owner],
    }
}

rpc_method! {
    /// 두 쪽(커밋 · index · 작업 트리)의 diff: 바뀐 파일 목록(상태 · 이름 바뀜 · 모드 · 바이너리 · 서브모듈 포인터 · 생성 파일 ·
    /// 대용량)과 파일별 hunk(`git diff --find-renames -U3` 과 같다). 저장소가 없으면 invalid params.
    pub GitDiffMethod: "git.diff" {
        params: GitDiffParams,
        result: GitDiff,
        tier: Read,
        roles: [Owner],
    }
}

rpc_method! {
    /// 리비전(또는 index)의 파일 내용 — 전체 또는 범위. 50 MiB 를 넘는 blob 은 풀지 않고 크기만 준다(`tooLarge`).
    pub GitBlobMethod: "git.blob" {
        params: GitBlobParams,
        result: GitBlob,
        tier: Read,
        roles: [Owner],
    }
}

/// 핸들러를 붙인다. 저장소 읽기(디스크 IO · 압축 풀기 · diff)는 블로킹 스레드에서 한다.
pub fn register(router: &mut Router, git: Arc<Git>) {
    let g = Arc::clone(&git);
    router.unary(GitShowMethod, move |_ctx, params: GitRevParams| {
        let git = Arc::clone(&g);
        blocking(move || git.show(&params))
    });
    let g = Arc::clone(&git);
    router.unary(GitContainedInMethod, move |_ctx, params: GitRevParams| {
        let git = Arc::clone(&g);
        blocking(move || git.contained_in(&params))
    });
    let g = Arc::clone(&git);
    router.unary(GitDiffMethod, move |_ctx, params: GitDiffParams| {
        let git = Arc::clone(&g);
        blocking(move || git.diff(&params))
    });
    router.unary(GitBlobMethod, move |_ctx, params: GitBlobParams| {
        let git = Arc::clone(&git);
        blocking(move || git.blob(&params))
    });
}

async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, RpcError> + Send + 'static,
) -> Result<T, RpcError> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| RpcError::internal(e.to_string()))?
}
