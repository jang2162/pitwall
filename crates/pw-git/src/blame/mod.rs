//! blame(`docs/design/13-m2-plan.md` M2-05 — 결정 M2-D1 의 되돌림 조건).
//!
//! 엔진이 둘이다. 첫 청크의 [`BlameFile::engine`] 이 어느 쪽인지 알린다.
//!
//! - [`incremental`]: 신뢰한 프로젝트는 `git blame --incremental -M` 을 띄워 출력을 받는 대로 청크로 낸다. 결정 M2-D1 의 되돌림
//!   조건을 넘었기 때문이다 — gix blame(gix-blame 0.20)은 파일 안에서 옮긴 줄을 찾지 않아(`-M`) 줄 이동 픽스처에서 `git blame -M`
//!   과 다르다(13-m2-plan.md 결정 M2-D1 의 판정 — 시험 `gix_blame_misses_moved_lines_so_trusted_projects_use_the_cli`).
//! - [`native`]: 미신뢰 프로젝트(git CLI 로 읽지 않는다 — 03-architecture.md 「보안」)와 git 을 찾지 못했을 때는 gix blame 이다.
//!   `git blame`(`-M` 없이)과 같은 결과다 — 옮긴 줄은 옮긴 커밋의 것이 된다. 바이너리 · `-diff` 파일도 git 처럼 바이트를 줄로
//!   견준다(`native::text_cache`). 이력의 버전이 [`READ_ONLY_MAX`] 를 넘으면 invalid params 다. 저장소의 프로그램(필터 드라이버 ·
//!   textconv)은 돌지 않는다: 커밋의 blob 만 견준다.
//!
//! 공통: 리비전(`rev`, 없으면 HEAD)을 gix 로 커밋까지 풀어 그 해시를 git 에 넘긴다(git 이 리비전 문법을 다시 풀지 않는다 — `-` 로
//! 시작하는 글자가 옵션이 되지 않는다). 작업 트리 · index 는 읽지 않는다 — 커밋하지 않은 변경 · 저장하지 않은 편집은 blame 하지
//! 않는다. 창이 그 리비전의 줄을 지금 글에 맞춰 옮기고(거터의 HEAD 비교 — 결정 M2-D9, M2-17), 맞는 줄이 없는 줄이 「아직 커밋 안
//! 됨」 이다. 「이 커밋 이전으로」 는 범위의 [`BlameRange::previous`](그 커밋의 부모와 그 부모에서의 경로 — git 의 `previous`)로
//! 다시 부른다. 보통 `<커밋>^` 이지만 머지 커밋이 넣은 줄이고 첫 부모에 그 파일이 없으면 그 파일이 있는 부모(`<커밋>^2` 등)다.

pub mod incremental;
pub mod methods;
pub mod native;

#[cfg(test)]
pub(crate) mod tests;

use std::collections::HashSet;
use std::time::{Duration, Instant};

use gix::ObjectId;
use gix::bstr::{BStr, BString, ByteSlice};
use pw_fs::limits::READ_ONLY_MAX;
use pw_rpc::RpcError;
use serde::{Deserialize, Serialize};

/// 범위를 이만큼 모으면 청크를 보낸다(첫 청크는 범위가 생기는 대로 보낸다).
pub const CHUNK: usize = 500;
/// 모인 범위가 있으면 이만큼 기다린 뒤 보낸다.
const FLUSH_AFTER: Duration = Duration::from_millis(100);

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum BlameEngine {
    /// `git blame --incremental -M` — 파일 안에서 옮긴 줄을 원래 커밋으로 돌린다.
    Git,
    /// gix blame — `git blame`(`-M` 없이)과 같다. 옮긴 줄은 옮긴 커밋의 것이다.
    Gix,
}

/// blame 한 파일. 첫 청크에만 실린다.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct BlameFile {
    /// blame 한 커밋(받은 `rev` 를 푼 해시).
    pub commit: String,
    /// 그 커밋에서의 파일 줄 수. 모든 청크의 범위를 합치면 1 ~ `lines` 를 겹치지 않고 한 번씩 덮는다.
    #[specta(type = pw_rpc::JsNumber)]
    pub lines: u32,
    pub engine: BlameEngine,
}

/// 범위가 가리키는 커밋. 스트림에서 처음 나올 때 한 번만 실린다.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct BlameCommit {
    /// 전체 해시(16진).
    pub hash: String,
    /// 작성자(mailmap 적용).
    pub author_name: String,
    pub author_email: String,
    /// 작성 시각(유닉스 초).
    #[specta(type = pw_rpc::JsNumber)]
    pub author_time: i64,
    /// 제목(메시지 첫 문단을 한 줄로).
    pub subject: String,
    /// 경계 커밋(git 의 `boundary`): 첫 커밋(`blame.showRoot` 를 켜지 않았으면 — git 의 기본)이거나 얕은 클론의 끝이라 그 앞을 볼
    /// 수 없는 커밋. 얕은 클론의 끝이면 이 커밋의 줄이 정말 이 커밋에서 생겼는지는 모른다.
    pub boundary: bool,
}

/// 「이 커밋 이전으로」 의 대상: 부모 커밋과 그 부모에서의 파일 경로.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct BlamePrevious {
    pub commit: String,
    /// 저장소 꼭대기 기준 경로(`/` 구분). 이름이 바뀐 커밋이면 바뀌기 전 이름이다.
    pub path: String,
}

/// 연속한 줄 범위 하나와 그 줄을 마지막으로 바꾼 커밋.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct BlameRange {
    /// 첫 줄(1부터, blame 한 리비전의 파일 기준).
    #[specta(type = pw_rpc::JsNumber)]
    pub line: u32,
    /// 줄 수(1 이상).
    #[specta(type = pw_rpc::JsNumber)]
    pub count: u32,
    /// 커밋 해시([`BlameChunk::commits`] 에 앞서 실렸다).
    pub commit: String,
    /// 그 커밋에서의 경로(이름 바뀜을 따라간 것).
    pub source_path: String,
    /// 그 커밋의 파일에서의 첫 줄(1부터).
    #[specta(type = pw_rpc::JsNumber)]
    pub source_line: u32,
    /// 「이 커밋 이전으로」. 그 커밋이 첫 커밋이거나 부모에 그 파일이 없으면 `null`.
    pub previous: Option<BlamePrevious>,
}

/// `git.blame` 스트림의 항목 하나.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct BlameChunk {
    /// 첫 청크에만 있다.
    pub file: Option<BlameFile>,
    /// 이 청크의 범위가 처음 가리키는 커밋(앞 청크에 실린 커밋은 다시 싣지 않는다).
    pub commits: Vec<BlameCommit>,
    /// 범위(순서는 정해져 있지 않다 — `git` 엔진은 찾는 대로 보낸다).
    pub ranges: Vec<BlameRange>,
}

/// blame 할 대상: 커밋 · 그 커밋에서의 경로 · 줄 수.
#[derive(Debug, Clone)]
pub struct Subject {
    pub commit: ObjectId,
    pub path: BString,
    pub lines: u32,
}

/// 리비전을 커밋으로 푼다(태그는 벗긴다). `rev` 가 없으면 HEAD — 태어나지 않았으면 `None`. 못 찾거나 커밋이 아니면 invalid params.
pub(crate) fn start_commit(
    repo: &gix::Repository,
    rev: Option<&str>,
) -> Result<Option<ObjectId>, RpcError> {
    let Some(rev) = rev else {
        return Ok(repo.head().map_err(internal)?.id().map(gix::Id::detach));
    };
    let id = repo
        .rev_parse_single(rev.as_bytes().as_bstr())
        .map_err(|e| RpcError::invalid_params(format!("cannot resolve `{rev}`: {e}")))?;
    let commit = id
        .object()
        .map_err(internal)?
        .peel_to_commit()
        .map_err(|e| RpcError::invalid_params(format!("`{rev}` is not a commit: {e}")))?;
    Ok(Some(commit.id))
}

/// `rev` 의 `path` 를 blame 할 대상으로 푼다. 리비전이 없거나(태어나지 않은 HEAD) 그 커밋에 그 파일이 없으면 `None` — 스트림은
/// 청크 없이 끝난다. 폴더 · 서브모듈이거나 [`READ_ONLY_MAX`] 를 넘으면 invalid params.
pub fn subject(
    repo: &gix::Repository,
    rev: Option<&str>,
    path: &str,
) -> Result<Option<Subject>, RpcError> {
    let path = crate::log::filter::relative_path(path)?;
    let Some(commit) = start_commit(repo, rev)? else {
        return Ok(None);
    };
    let tree = repo
        .find_commit(commit)
        .map_err(internal)?
        .tree()
        .map_err(internal)?;
    let Some(entry) = tree
        .lookup_entry_by_path(path.to_path_lossy())
        .map_err(internal)?
    else {
        return Ok(None);
    };
    if !entry.mode().is_blob_or_symlink() {
        return Err(RpcError::invalid_params(format!(
            "`{path}` is not a file in {commit}"
        )));
    }
    let header = repo.find_header(entry.object_id()).map_err(internal)?;
    if header.size() > READ_ONLY_MAX {
        return Err(RpcError::invalid_params(format!(
            "`{path}` is {} bytes; blame reads at most {READ_ONLY_MAX}",
            header.size()
        )));
    }
    let blob = repo.find_blob(entry.object_id()).map_err(internal)?;
    let lines = u32::try_from(count_lines(&blob.data))
        .map_err(|_| RpcError::invalid_params(format!("`{path}` has too many lines")))?;
    Ok(Some(Subject {
        commit,
        path,
        lines,
    }))
}

/// git 이 세는 줄 수: 줄바꿈마다 하나, 끝에 줄바꿈 없는 글자가 남으면 하나 더.
pub fn count_lines(data: &[u8]) -> usize {
    let breaks = data.iter().filter(|&&b| b == b'\n').count();
    breaks + usize::from(data.last().is_some_and(|&b| b != b'\n'))
}

fn internal(e: impl std::fmt::Display) -> RpcError {
    RpcError::internal(e.to_string())
}

/// 경로를 글로. UTF-8 이 아니면 invalid params — 손실 변환(U+FFFD)한 경로로는 그 파일을 다시 지목할 수 없어(「이 커밋 이전으로」 ·
/// 리비전의 파일 열기) 조용히 빈 결과가 된다. RPC 의 경로는 모두 글이라 바이트로 싣지 않는다(conflict 모듈과 같은 판정).
pub(crate) fn path_text(path: &BStr) -> Result<String, RpcError> {
    path.to_str().map(str::to_owned).map_err(|_| {
        RpcError::invalid_params(format!(
            "the path `{path}` is not UTF-8; it cannot be shown or named"
        ))
    })
}

/// 범위를 청크로 묶는다. 첫 청크는 [`BlameFile`] 을 싣고, 커밋 정보는 처음 나올 때 한 번만 싣는다.
pub struct Chunker {
    file: Option<BlameFile>,
    commits: Vec<BlameCommit>,
    ranges: Vec<BlameRange>,
    /// 보낸(또는 이 청크에 실은) 커밋.
    known: HashSet<String>,
    sent: usize,
    since_flush: Instant,
}

impl Chunker {
    pub fn new(file: BlameFile) -> Self {
        Self {
            file: Some(file),
            commits: Vec::new(),
            ranges: Vec::new(),
            known: HashSet::new(),
            sent: 0,
            since_flush: Instant::now(),
        }
    }

    /// 이 커밋을 이미 실었는가.
    pub fn knows(&self, hash: &str) -> bool {
        self.known.contains(hash)
    }

    /// 커밋 정보를 싣는다(이미 실었으면 버린다).
    pub fn commit(&mut self, commit: BlameCommit) {
        if self.known.insert(commit.hash.clone()) {
            self.commits.push(commit);
        }
    }

    pub fn range(&mut self, range: BlameRange) {
        self.ranges.push(range);
    }

    /// 지금 보낼 때인가: 첫 청크는 범위가 생기자마자, 그 뒤는 [`CHUNK`] 개가 모였거나 [`FLUSH_AFTER`] 가 지났을 때.
    pub fn due(&self) -> bool {
        !self.ranges.is_empty()
            && (self.sent == 0
                || self.ranges.len() >= CHUNK
                || self.since_flush.elapsed() >= FLUSH_AFTER)
    }

    /// 보내지 않은 범위가 있으면, 다음 [`Self::due`] 까지 남은 시간.
    pub fn wait(&self) -> Option<Duration> {
        (!self.ranges.is_empty()).then(|| FLUSH_AFTER.saturating_sub(self.since_flush.elapsed()))
    }

    pub fn take(&mut self) -> BlameChunk {
        self.sent += 1;
        self.since_flush = Instant::now();
        BlameChunk {
            file: self.file.take(),
            commits: std::mem::take(&mut self.commits),
            ranges: std::mem::take(&mut self.ranges),
        }
    }

    /// 남은 것. 아무것도 보내지 않았으면(빈 파일) 범위 없는 첫 청크 하나.
    pub fn finish(mut self) -> Option<BlameChunk> {
        (self.sent == 0 || !self.ranges.is_empty()).then(|| self.take())
    }
}
