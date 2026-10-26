//! 충돌 RPC(`docs/design/13-m2-plan.md` M2-11). UI 전용이라 MCP 툴이 아니다(결정 M2-D7 — 해결은 쓰기라 M5 에서도 툴이 아니다).
//!
//! - `git.conflicts` · `git.conflict.get`: 읽기(gix 로 index 의 단계 1~3 과 그 blob — 미신뢰 프로젝트도 프로세스를 띄우지 않는다).
//! - `git.conflict.resolve`: 쓰기 실행기(M2-06)로 git CLI.

use std::sync::Arc;

use pw_fs::judge::Judgment;
use pw_fs::methods::FsContent;
use pw_rpc::{Router, RpcError, rpc_method};
use serde::{Deserialize, Serialize};

use crate::status::GitConflict;
use crate::write::GitWrite;

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitConflictsParams {
    pub project_id: String,
    /// 활성 워크트리의 꼭대기(`ActiveWorktree.top`, 절대 경로).
    #[serde(default)]
    pub worktree: Option<String>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitConflicts {
    /// 저장소의 작업 트리 꼭대기(절대 경로, `git.repo` 의 `workTree` 와 같은 꼴). 파일 경로는 이 아래의 상대 경로다.
    pub work_tree: String,
    /// 충돌 중인 경로(index 에 단계 1~3 이 있는 것). 경로순.
    pub files: Vec<ConflictFile>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct ConflictFile {
    /// 작업 트리 꼭대기 기준 상대 경로, `/` 구분. UTF-8 이 아닌 바이트는 U+FFFD 다 — 그런 경로는 `git.conflict.get` ·
    /// `git.conflict.resolve` 로 지목할 수 없다(invalid params).
    pub path: String,
    /// index 단계가 있는 쪽들로 가른 갈래(`git status --porcelain` 의 두 글자 — `git.status` 와 같은 값).
    pub kind: GitConflict,
    /// 공통 조상(index 단계 1). 그 쪽이 없으면 `null`.
    pub base: Option<ConflictStage>,
    /// 우리 쪽(단계 2) — HEAD 쪽이다. rebase 중이면 옮겨 가는 대상(upstream) 쪽이다(git 의 `--ours` 와 같다).
    pub ours: Option<ConflictStage>,
    /// 그쪽(단계 3) — 들여오는 커밋 쪽. rebase 중이면 다시 적용하는 커밋 쪽이다.
    pub theirs: Option<ConflictStage>,
    /// 어느 쪽이든 바이너리다(앞 8 KiB 에 NUL — `fs.read` 의 판정). 5 MiB 이하인 쪽만 본다(`large` 인 쪽은 목록이 풀지 않는다 —
    /// 그쪽의 판정은 `git.conflict.get` 의 각 쪽 `judgment`). `.gitattributes` 의 `merge` 속성은 보지 않는다.
    pub binary: bool,
    /// 어느 쪽이든 서브모듈(`160000`)이다.
    pub submodule: bool,
    /// 어느 쪽이든 심링크(`120000`)다.
    pub symlink: bool,
    /// 어느 쪽이든 편집 기준(결정 M1-D9 의 5 MiB)을 넘는다 — 자동 병합 결과가 없으므로 창은 한쪽 고르기만 한다.
    pub large: bool,
    /// 어느 쪽이든 대용량 기준(결정 M1-D9 의 50 MiB)을 넘어 내용을 읽지 않았다 — 그쪽의 바이너리 여부는 모른다.
    pub too_large: bool,
}

/// index 단계 하나.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct ConflictStage {
    /// blob 해시(서브모듈이면 커밋).
    pub id: String,
    /// git 의 8진 모드(`100644` · `100755` · `120000` · `160000`).
    pub mode: String,
}

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitConflictParams {
    pub project_id: String,
    #[serde(default)]
    pub worktree: Option<String>,
    /// 작업 트리 꼭대기 기준 상대 경로(`/` 구분, 글자 그대로).
    pub path: String,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitConflictDetail {
    pub work_tree: String,
    /// 목록(`git.conflicts`)의 그 항목.
    pub file: ConflictFile,
    /// 작업 트리 파일의 지금 `version`(`fs.read` 와 같은 값) — `content` · `side` 로 풀 때 `expectedVersion` 으로 돌려준다. 일반
    /// 파일이 없으면(지워짐 · 폴더 · 링크인 앞 폴더 · 루트 밖 링크) 또는 읽을 수 없으면 `null`. 50 MiB 를 넘으면 내용을 읽지 않은
    /// `large-` 로 시작하는 값(크기 · mtime · 유닉스면 inode — 그것들이 그대로인 제자리 편집은 못 잡는다)이고, 그 값으로는 `side` 만 풀린다.
    pub version: Option<String>,
    pub base: Option<ConflictSide>,
    pub ours: Option<ConflictSide>,
    pub theirs: Option<ConflictSide>,
    /// 자동 병합 결과 — 충돌 구간은 비어 있다(결정 M2-D4 의 결과 칸 처음 내용). ours · theirs 가 다 있고, 있는 쪽이 모두 편집할 수
    /// 있는 텍스트(일반 파일 · UTF-8 · 5 MiB 이하 — 판정의 `editable`)일 때만 있다. 공통 조상이 없으면(`bothAdded`) 빈 파일을 조상으로
    /// 본다. 세 쪽 모두 앞의 UTF-8 BOM 을 떼고 병합한다 — 판정의 `bom` 이 그쪽에 BOM 이 있었는지다.
    pub merged: Option<String>,
    /// `merged` 가 있고 충돌 구간이 하나도 없다. git 은 이 경로를 충돌로 남겼다 — git 의 merge 는 histogram diff 로 병합하고 이
    /// 계산은 Myers(`git merge-file` 의 기본)라 드물게 갈린다. 창은 고를 충돌 없이 세 쪽의 diff 로 보이고, 저장하면 그래도
    /// `git.conflict.resolve` 로 푼다(index 는 아직 충돌이다).
    pub clean: bool,
    /// 병합 결과의 구간(앞에서부터): 한쪽만 바꾼 곳과 충돌. `merged` 가 없으면 비어 있다. `git merge-file -p --diff3` 의 구간과 같다.
    pub regions: Vec<MergeRegion>,
}

/// 한 쪽의 내용. `git.blob` 과 같은 꼴이다.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct ConflictSide {
    pub id: String,
    pub mode: String,
    /// blob 크기(서브모듈이면 0).
    #[specta(type = pw_rpc::JsNumber)]
    pub size: u64,
    /// 대용량 기준(50 MiB)을 넘는다. 풀지 않으므로 `content` · `judgment` 가 `null` 이다.
    pub too_large: bool,
    /// 텍스트면 그 글(앞의 BOM 을 뺀 것 — `fs.read` 와 같은 규칙). 바이너리 · 서브모듈 · 대용량이면 `null`. 심링크면 대상 경로다.
    pub content: Option<FsContent>,
    pub judgment: Option<Judgment>,
}

/// 병합 결과의 구간 하나. 줄은 `\n` 으로 나눈다(CRLF 의 `\r` 은 줄 내용이다). 번호는 0 기준, `count` 줄.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct MergeRegion {
    pub kind: MergeRegionKind,
    pub base: MergeLines,
    pub ours: MergeLines,
    pub theirs: MergeLines,
    /// 병합 결과(`merged`)에서 차지하는 줄. 충돌이면 비어 있다 — `start` 가 고른 줄이 들어갈 자리다.
    pub merged: MergeLines,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum MergeRegionKind {
    /// 우리 쪽만 바꿨다 — 결과에 우리 쪽 줄이 들어 있다.
    Ours,
    /// 그쪽만 바꿨다 — 결과에 그쪽 줄이 들어 있다.
    Theirs,
    Conflict,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct MergeLines {
    pub start: u32,
    pub count: u32,
}

/// 내용으로 풀기.
#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct ConflictContent {
    /// 결과(UTF-8 글).
    pub text: String,
    /// 앞에 UTF-8 BOM 을 붙인다(`fs.write` 와 같다).
    #[serde(default)]
    pub bom: bool,
    /// `git.conflict.get` 이 준 `version`(`null` 이었으면 `null`). 지금 작업 트리 파일과 다르면 쓰지 않고 `CONFLICT` — `fs.write`
    /// 의 `expectedVersion` 과 같다. 빠뜨릴 수 없다(그사이 터미널 · 다른 창에서 고친 것을 말없이 덮지 않게).
    // serde 는 빠진 `Option` 을 `None` 으로 받는다 — `deserialize_with` 로 빠뜨림을 오류로 하고, 그러면 두 생성기가 꼴을 모르므로
    // 손으로 적는다.
    #[serde(deserialize_with = "Option::deserialize")]
    #[specta(type = Option<String>)]
    #[schemars(required, extend("type" = ["string", "null"]))]
    pub expected_version: Option<String>,
}

/// 한쪽으로 풀기.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct ConflictChoice {
    pub pick: ConflictPick,
    /// `git.conflict.get` 이 준 `version` — `content` 의 `expectedVersion` 과 같다. 한쪽 고르기도 작업 트리 파일을 덮거나
    /// 지운다(서브모듈은 index 만 바꾸지만 같은 필드로 견준다 — 폴더라 `null`).
    #[serde(deserialize_with = "Option::deserialize")]
    #[specta(type = Option<String>)]
    #[schemars(required, extend("type" = ["string", "null"]))]
    pub expected_version: Option<String>,
}

/// 해결에서 고르는 쪽.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum ConflictPick {
    Ours,
    Theirs,
}

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitConflictResolveParams {
    pub project_id: String,
    #[serde(default)]
    pub worktree: Option<String>,
    pub path: String,
    /// 결과 내용. 파일에 쓰고 `git add` 한다. `side` 와 둘 중 하나만.
    #[serde(default)]
    pub content: Option<ConflictContent>,
    /// 한쪽을 고른다. 그쪽이 있으면 그 내용으로(`git checkout --ours|--theirs` 뒤 `git add`), 지운 쪽이면 `git rm` 이다.
    #[serde(default)]
    pub side: Option<ConflictChoice>,
}

rpc_method! {
    /// 충돌 중인 경로 목록: index 단계로 가른 종류(양쪽 수정 · 우리/그쪽 삭제 · 양쪽 추가 …)와 바이너리 · 서브모듈 · 심링크 표시.
    /// 저장소가 없으면 invalid params.
    pub GitConflictsMethod: "git.conflicts" {
        params: GitConflictsParams,
        result: GitConflicts,
        tier: Read,
        roles: [Owner],
    }
}

rpc_method! {
    /// 충돌 경로 하나: base · ours · theirs 내용과 자동 병합 결과 · 구간(diff3). 충돌 중이 아닌 경로면 not found.
    pub GitConflictGetMethod: "git.conflict.get" {
        params: GitConflictParams,
        result: GitConflictDetail,
        tier: Read,
        roles: [Owner],
    }
}

rpc_method! {
    /// 충돌 경로 하나를 해결한다. 작업 트리 파일이 `expectedVersion` 그대로일 때만(아니면 `CONFLICT`): `content` 면 파일에 쓰고
    /// (원자 쓰기, 프로젝트 파일 쓰기와 같은 경로 규칙 — 앞 폴더가 심링크면 거절) `git add`, `side` 면 그쪽을
    /// `git checkout --ours|--theirs` 로 꺼내 `git add`(지운 쪽이면 `git rm`, 서브모듈이면 index 만). 미저장 문서 확인을 거친다. 해결
    /// 뒤의 충돌 목록을 돌려준다.
    pub GitConflictResolveMethod: "git.conflict.resolve" {
        params: GitConflictResolveParams,
        result: GitConflicts,
        tier: Write,
        roles: [Owner],
    }
}

/// 핸들러를 붙인다. 읽기(디스크 IO · 압축 풀기 · 병합 계산)는 블로킹 스레드에서 한다.
pub fn register(router: &mut Router, write: Arc<GitWrite>) {
    let git = Arc::clone(write.git());
    router.unary(
        GitConflictsMethod,
        move |_ctx, params: GitConflictsParams| {
            let git = Arc::clone(&git);
            blocking(move || git.conflicts(&params))
        },
    );
    let git = Arc::clone(write.git());
    router.unary(
        GitConflictGetMethod,
        move |_ctx, params: GitConflictParams| {
            let git = Arc::clone(&git);
            blocking(move || git.conflict(&params))
        },
    );
    router.unary(
        GitConflictResolveMethod,
        move |_ctx, params: GitConflictResolveParams| {
            let write = Arc::clone(&write);
            async move { Ok(write.resolve(params).await?) }
        },
    );
}

async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, RpcError> + Send + 'static,
) -> Result<T, RpcError> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| RpcError::internal(e.to_string()))?
}
