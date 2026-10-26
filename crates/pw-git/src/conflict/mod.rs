//! 충돌 데이터 · 해결(`docs/design/13-m2-plan.md` M2-11 — F5-13 의 코어, 결정 M2-D4).
//!
//! - 읽기(`git.conflicts` · `git.conflict.get`): index 의 단계 1(base) · 2(ours) · 3(theirs)과 그 blob 을 gix 로 읽는다. 미신뢰
//!   프로젝트도 같은 길이다 — git CLI 를 띄우지 않고, 작업 트리를 읽지 않으므로 필터 드라이버가 돌 일도 없다. 자동 병합 결과와
//!   구간은 [`merge`](diff3)가 계산한다. 각 쪽의 내용은 대용량 기준(결정 M1-D9)을 따른다: 50 MiB 를 넘으면 풀지 않고, 5 MiB 를
//!   넘으면 `git.blob` 과 같은 차례(`crate::diff::blob::BIG_BLOBS`)를 받아 푼다.
//! - 해결(`git.conflict.resolve`): 쓰기 실행기([`crate::write`])로 돈다 — 미저장 문서 확인은 그 경로 하나다. 저장소 잠금 안에서 그
//!   경로가 아직 충돌 중인지 다시 읽은 뒤:
//!   - `content`: 작업 트리에 원자 쓰기(`pw_fs::write::write_atomic`)를 하고 `git add`. 경로 판정은 파일 조작(M1-21)의 규칙 A 다 —
//!     들어 있는 폴더를 실제 경로로 풀어 작업 트리 안이어야 하고, 그 항목이 심링크면 대상도 작업 트리 안이어야 한다. 쓰기는 항목
//!     자체를 바꾼다(심링크면 링크가 일반 파일이 된다 — `git add` 가 링크를 담지 않게).
//!   - `side`: 그쪽이 있으면 `git checkout --ours|--theirs` 뒤 `git add` — git 이 smudge 필터 · 줄바꿈 변환을 하고(LFS · autocrlf
//!     에서도 맞는 파일), 앞 폴더의 심링크를 따라 쓰지 않는다. 서브모듈이면 `git update-index --cacheinfo` 로 index 만 바꾼다(서브모듈
//!     체크아웃은 그대로 — merge 가 서브모듈을 옮기지 않는 것과 같다). 그쪽이 지운 쪽이면 `git rm`.
//!
//!   경로는 글자 그대로 넘긴다(`--literal-pathspecs`). 해결 뒤의 충돌 목록을 돌려준다.

pub mod merge;
pub mod methods;

#[cfg(test)]
mod tests;

use std::path::Path;
use std::sync::Arc;

use gix::ObjectId;
use gix::bstr::ByteSlice;
use gix::index::entry::Mode;
use pw_fs::judge::{Bom, Judgment, judge};
use pw_fs::limits::{EDITABLE_MAX, READ_ONLY_MAX, SNIFF_BYTES};
use pw_fs::scope::{Denied, Roots};
use pw_rpc::{RpcError, code};

use crate::diff::blob::{BIG_BLOB_WAIT, BIG_BLOBS, BIG_BLOBS_AT_ONCE};
use crate::status::GitConflict;
use crate::write::{Changes, GitWrite, WriteError, WriteRequest};
use crate::{Git, Opened};
use methods::{
    ConflictFile, ConflictPick, ConflictSide, ConflictStage, GitConflictDetail, GitConflictParams,
    GitConflictResolveParams, GitConflicts, GitConflictsParams, MergeLines, MergeRegion,
    MergeRegionKind,
};

/// 해결 Task 의 제목(`extensions/i18n-default/i18n/*/project.ftl`).
pub const RESOLVE_TITLE: &str = "task-title-git-resolve";

const UTF8_BOM: &[u8] = &[0xEF, 0xBB, 0xBF];

impl Git {
    /// `git.conflicts`.
    pub fn conflicts(&self, params: &GitConflictsParams) -> Result<GitConflicts, RpcError> {
        list(&opened(
            self,
            &params.project_id,
            params.worktree.as_deref(),
        )?)
    }

    /// `git.conflict.get`.
    pub fn conflict(&self, params: &GitConflictParams) -> Result<GitConflictDetail, RpcError> {
        check_path(&params.path)?;
        let opened = opened(self, &params.project_id, params.worktree.as_deref())?;
        let entry = find(&opened.repo, &params.path)?;
        let sides = entry
            .stages
            .map(|stage| stage.map(|s| Side::load(&opened.repo, s)).transpose());
        let [base, ours, theirs] = sides;
        let sides = [base?, ours?, theirs?];
        let file = entry.file(&sides);
        let (merged, regions) = match merged(&sides) {
            Some((text, regions)) => (Some(text), regions),
            None => (None, Vec::new()),
        };
        let [base, ours, theirs] = sides.map(|s| s.map(Side::into_rpc));
        Ok(GitConflictDetail {
            work_tree: crate::status::display(&opened.target.work_tree),
            file,
            base,
            ours,
            theirs,
            merged,
            regions,
        })
    }
}

impl GitWrite {
    /// `git.conflict.resolve`(모듈 머리 주석).
    pub async fn resolve(
        &self,
        params: GitConflictResolveParams,
    ) -> Result<GitConflicts, WriteError> {
        check_path(&params.path).map_err(WriteError::Rpc)?;
        let resolution = match (params.content, params.side) {
            (Some(text), None) => {
                let mut bytes = Vec::with_capacity(text.len() + UTF8_BOM.len());
                if params.bom {
                    bytes.extend_from_slice(UTF8_BOM);
                }
                bytes.extend_from_slice(text.as_bytes());
                Resolution::Content(bytes)
            }
            (None, Some(side)) if !params.bom => Resolution::Side(side),
            (None, Some(_)) => {
                return Err(WriteError::Rpc(RpcError::invalid_params(
                    "`bom` applies only to `content`",
                )));
            }
            _ => {
                return Err(WriteError::Rpc(RpcError::invalid_params(
                    "give exactly one of `content` or `side`",
                )));
            }
        };
        let path = params.path;
        // 미저장 확인을 묻기 전에 거절할 것은 거절한다(충돌 중이 아님 · 내용으로 풀 수 없는 갈래). 잠금 안에서 다시 본다.
        let precheck = {
            let (git, project, worktree, path) = (
                Arc::clone(self.git()),
                params.project_id.clone(),
                params.worktree.clone(),
                path.clone(),
            );
            tokio::task::spawn_blocking(move || match git.open(&project, worktree.as_deref())? {
                Some(opened) => find(&opened.repo, &path).map(Some),
                None => Ok(None),
            })
            .await
            .map_err(|e| WriteError::Internal(e.to_string()))?
        };
        match precheck {
            Ok(Some(entry)) => fits(&entry, &resolution).map_err(WriteError::Rpc)?,
            Ok(None) => return Err(WriteError::NoRepository),
            Err(e) => return Err(WriteError::Rpc(e)),
        }
        let request = WriteRequest {
            project_id: params.project_id,
            worktree: params.worktree,
            title: RESOLVE_TITLE.to_owned(),
            changes: Changes::Paths(vec![path.clone()]),
            askpass: false,
        };
        self.run(request, move |ctx| async move {
            // 잠금을 받은 뒤 다시 읽는다 — 기다리는 사이 터미널에서 풀었을 수 있다.
            let entry = {
                let path = path.clone();
                ctx.read(move |o| find(&o.repo, &path))
                    .await?
                    .map_err(WriteError::Rpc)?
            };
            let literal = |args: &[&str]| {
                ctx.git()
                    .arg("--literal-pathspecs")
                    .args(args.iter().copied())
            };
            fits(&entry, &resolution).map_err(WriteError::Rpc)?;
            match resolution {
                Resolution::Content(bytes) => {
                    let (work_tree, rel) = (ctx.work_tree().to_path_buf(), path.clone());
                    tokio::task::spawn_blocking(move || write_content(&work_tree, &rel, &bytes))
                        .await
                        .map_err(|e| WriteError::Internal(e.to_string()))??;
                    ctx.run(literal(&["add", "--", &path])).await?;
                }
                Resolution::Side(pick) => match entry.side(pick) {
                    None => {
                        ctx.run(literal(&["rm", "--quiet", "--", &path])).await?;
                    }
                    Some(stage) if stage.mode == Mode::COMMIT => {
                        let info = format!("160000,{},{path}", stage.id);
                        ctx.run(ctx.git().args(["update-index", "--cacheinfo", &info]))
                            .await?;
                    }
                    Some(_) => {
                        let flag = match pick {
                            ConflictPick::Ours => "--ours",
                            ConflictPick::Theirs => "--theirs",
                        };
                        ctx.run(literal(&["checkout", flag, "--", &path])).await?;
                        ctx.run(literal(&["add", "--", &path])).await?;
                    }
                },
            }
            ctx.read(list).await?.map_err(WriteError::Rpc)
        })
        .await
    }
}

enum Resolution {
    Content(Vec<u8>),
    Side(ConflictPick),
}

/// 내용으로는 서브모듈 · 심링크 충돌을 풀 수 없다 — 한쪽을 고른다.
fn fits(entry: &Entry, resolution: &Resolution) -> Result<(), RpcError> {
    if matches!(resolution, Resolution::Content(_))
        && (entry.has(Mode::COMMIT) || entry.has(Mode::SYMLINK))
    {
        return Err(RpcError::invalid_params(format!(
            "`{}` is a submodule or symlink conflict; pick a side",
            entry.path
        )));
    }
    Ok(())
}

/// 저장소가 없으면 invalid params.
fn opened(git: &Git, project_id: &str, worktree: Option<&str>) -> Result<Opened, RpcError> {
    git.open(project_id, worktree)?.ok_or_else(|| {
        RpcError::invalid_params(format!("project `{project_id}` has no git repository"))
    })
}

/// RPC 의 경로 꼴: 작업 트리 꼭대기 기준 상대, `/` 구분, `.` · `..` · 빈 구성 요소 없음.
fn check_path(path: &str) -> Result<(), RpcError> {
    let ok = !path.is_empty()
        && !path.contains('\\')
        && !path.contains('\0')
        && path
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..");
    if ok {
        Ok(())
    } else {
        Err(RpcError::invalid_params(format!(
            "`{path}` must be a path relative to the repository top, separated by `/`"
        )))
    }
}

/// index 단계 하나.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Stage {
    id: ObjectId,
    mode: Mode,
}

impl Stage {
    fn mode_text(&self) -> &'static str {
        match self.mode {
            Mode::FILE_EXECUTABLE => "100755",
            Mode::SYMLINK => "120000",
            Mode::COMMIT => "160000",
            _ => "100644",
        }
    }

    fn rpc(&self) -> ConflictStage {
        ConflictStage {
            id: self.id.to_string(),
            mode: self.mode_text().to_owned(),
        }
    }
}

/// 충돌 경로 하나: base · ours · theirs 단계.
#[derive(Debug, Clone)]
struct Entry {
    path: String,
    stages: [Option<Stage>; 3],
}

impl Entry {
    fn kind(&self) -> GitConflict {
        match self.stages.map(|s| s.is_some()) {
            [true, true, true] => GitConflict::BothModified,
            [false, true, true] => GitConflict::BothAdded,
            [true, false, true] => GitConflict::DeletedByUs,
            [true, true, false] => GitConflict::DeletedByThem,
            [false, true, false] => GitConflict::AddedByUs,
            [false, false, true] => GitConflict::AddedByThem,
            _ => GitConflict::BothDeleted,
        }
    }

    fn has(&self, mode: Mode) -> bool {
        self.stages.iter().flatten().any(|s| s.mode == mode)
    }

    fn side(&self, pick: ConflictPick) -> Option<Stage> {
        match pick {
            ConflictPick::Ours => self.stages[1],
            ConflictPick::Theirs => self.stages[2],
        }
    }

    fn file(&self, sides: &[Option<Side>; 3]) -> ConflictFile {
        let sides = || sides.iter().flatten();
        let [base, ours, theirs] = self.stages.map(|s| s.as_ref().map(Stage::rpc));
        ConflictFile {
            path: self.path.clone(),
            kind: self.kind(),
            base,
            ours,
            theirs,
            binary: sides().any(|s| s.judgment.as_ref().is_some_and(|j| j.binary)),
            submodule: self.has(Mode::COMMIT),
            symlink: self.has(Mode::SYMLINK),
            too_large: sides().any(|s| s.too_large),
        }
    }
}

/// index 의 충돌 경로(경로순 — index 의 순서).
fn entries(repo: &gix::Repository) -> Result<Vec<Entry>, RpcError> {
    let index = repo.index_or_empty().map_err(internal)?;
    let mut out: Vec<(Vec<u8>, Entry)> = Vec::new();
    for entry in index.entries() {
        let stage = entry.stage_raw();
        if !(1..=3).contains(&stage) {
            continue;
        }
        let path = entry.path(&index);
        let at = match out.last() {
            Some((last, _)) if last.as_slice() == path.as_bytes() => out.len() - 1,
            _ => {
                out.push((
                    path.to_vec(),
                    Entry {
                        path: path.to_str_lossy().into_owned(),
                        stages: [None; 3],
                    },
                ));
                out.len() - 1
            }
        };
        out[at].1.stages[stage as usize - 1] = Some(Stage {
            id: entry.id,
            mode: entry.mode,
        });
    }
    Ok(out.into_iter().map(|(_, e)| e).collect())
}

/// 충돌 중인 `path`. 아니면 not found.
fn find(repo: &gix::Repository, path: &str) -> Result<Entry, RpcError> {
    entries(repo)?
        .into_iter()
        .find(|e| e.path == path)
        .ok_or_else(|| RpcError::new(code::NOT_FOUND, format!("`{path}` is not in conflict")))
}

/// 충돌 목록. 바이너리 판정에 각 쪽의 blob 을 읽는다.
fn list(opened: &Opened) -> Result<GitConflicts, RpcError> {
    let repo = &opened.repo;
    let mut files = Vec::new();
    for entry in entries(repo)? {
        let mut sides: [Option<Side>; 3] = [None, None, None];
        for (slot, stage) in sides.iter_mut().zip(entry.stages) {
            if let Some(stage) = stage {
                *slot = Some(Side::load(repo, stage)?);
            }
        }
        files.push(entry.file(&sides));
    }
    Ok(GitConflicts {
        work_tree: crate::status::display(&opened.target.work_tree),
        files,
    })
}

/// 읽은 한 쪽.
struct Side {
    stage: Stage,
    size: u64,
    too_large: bool,
    /// blob 전체(BOM 포함). 서브모듈 · 대용량이면 없다.
    data: Option<Vec<u8>>,
    judgment: Option<Judgment>,
}

impl Side {
    fn load(repo: &gix::Repository, stage: Stage) -> Result<Self, RpcError> {
        let mut side = Self {
            stage,
            size: 0,
            too_large: false,
            data: None,
            judgment: None,
        };
        if stage.mode == Mode::COMMIT {
            return Ok(side);
        }
        side.size = repo.find_header(stage.id).map_err(internal)?.size();
        if side.size > READ_ONLY_MAX {
            side.too_large = true;
            return Ok(side);
        }
        let _seat = if side.size > EDITABLE_MAX {
            Some(BIG_BLOBS.enter(BIG_BLOB_WAIT).ok_or_else(|| {
                RpcError::internal(format!(
                    "git.conflict is busy: {BIG_BLOBS_AT_ONCE} large blobs are already being read"
                ))
            })?)
        } else {
            None
        };
        let data = repo.find_object(stage.id).map_err(internal)?.detach().data;
        let head = &data[..data.len().min(SNIFF_BYTES)];
        let bom = bom_len(head);
        side.judgment = Some(judge(head, &data[bom..], side.size, false));
        side.data = Some(data);
        Ok(side)
    }

    /// 병합할 수 있는 쪽이면 BOM 을 뗀 내용.
    fn mergeable(&self) -> Option<&[u8]> {
        let data = self.data.as_deref()?;
        let regular = matches!(self.stage.mode, Mode::FILE | Mode::FILE_EXECUTABLE);
        (regular && self.judgment.as_ref().is_some_and(|j| j.editable))
            .then(|| data.strip_prefix(UTF8_BOM).unwrap_or(data))
    }

    fn into_rpc(self) -> ConflictSide {
        let content = match (&self.data, &self.judgment) {
            (Some(data), Some(judgment)) if !judgment.binary => Some(pw_fs::content(
                false,
                judgment,
                data,
                bom_len(&data[..data.len().min(SNIFF_BYTES)]),
            )),
            _ => None,
        };
        ConflictSide {
            id: self.stage.id.to_string(),
            mode: self.stage.mode_text().to_owned(),
            size: self.size,
            too_large: self.too_large,
            content,
            judgment: self.judgment,
        }
    }
}

fn bom_len(head: &[u8]) -> usize {
    Bom::detect(head).map_or(0, Bom::byte_len)
}

/// 자동 병합 결과와 구간. ours · theirs 가 있고 있는 쪽이 모두 병합할 수 있을 때만.
fn merged(sides: &[Option<Side>; 3]) -> Option<(String, Vec<MergeRegion>)> {
    let [base, ours, theirs] = sides;
    let base = match base {
        Some(side) => side.mergeable()?,
        None => &[],
    };
    let (ours, theirs) = (ours.as_ref()?.mergeable()?, theirs.as_ref()?.mergeable()?);
    let merged = merge::merge(base, ours, theirs);
    let text = String::from_utf8(merged.text()).ok()?;
    let line_range = |r: &std::ops::Range<u32>| MergeLines {
        start: r.start,
        count: r.end - r.start,
    };
    let regions = merged
        .regions
        .iter()
        .map(|r| MergeRegion {
            kind: match r.kind {
                merge::Kind::Ours => MergeRegionKind::Ours,
                merge::Kind::Theirs => MergeRegionKind::Theirs,
                merge::Kind::Conflict => MergeRegionKind::Conflict,
            },
            base: line_range(&r.base),
            ours: line_range(&r.ours),
            theirs: line_range(&r.theirs),
            merged: line_range(&r.merged),
        })
        .collect();
    Some((text, regions))
}

/// `content` 해결의 파일 쓰기(모듈 머리 주석의 규칙 A).
fn write_content(work_tree: &Path, rel: &str, bytes: &[u8]) -> Result<(), WriteError> {
    use pw_core::registry::path::normalize;

    let refuse = |d: Denied| WriteError::Rpc(RpcError::invalid_params(d.to_string()));
    let roots = Roots::new([work_tree.to_path_buf()]);
    let requested = work_tree.join(rel);
    let (Some(parent), Some(name)) = (requested.parent(), requested.file_name()) else {
        return Err(refuse(Denied::Outside(requested)));
    };
    let parent = normalize(parent).map_err(|_| refuse(Denied::Relative(requested.clone())))?;
    let item = parent.join(name);
    if !roots.contains(&item) || roots.as_slice().contains(&item) {
        return Err(refuse(Denied::Outside(requested)));
    }
    match std::fs::symlink_metadata(&item) {
        Ok(meta) if meta.is_dir() => {
            return Err(WriteError::Rpc(RpcError::invalid_params(format!(
                "`{rel}` is a folder in the work tree"
            ))));
        }
        Ok(meta) if meta.file_type().is_symlink() => {
            let real = normalize(&item).map_err(|_| refuse(Denied::Relative(requested.clone())))?;
            if !roots.contains(&real) {
                return Err(refuse(Denied::LinkOutside(requested)));
            }
        }
        _ => {}
    }
    let io = |e: std::io::Error| WriteError::Internal(format!("write `{rel}`: {e}"));
    std::fs::create_dir_all(&parent).map_err(io)?;
    pw_fs::write::write_atomic(&item, bytes).map_err(io)
}

fn internal(e: impl std::fmt::Display) -> RpcError {
    RpcError::internal(e.to_string())
}
