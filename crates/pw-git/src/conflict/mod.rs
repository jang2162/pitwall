//! 충돌 데이터 · 해결(`docs/design/13-m2-plan.md` M2-11 — F5-13 의 코어, 결정 M2-D4).
//!
//! - 읽기(`git.conflicts` · `git.conflict.get`): index 의 단계 1(base) · 2(ours) · 3(theirs)과 그 blob 을 gix 로 읽는다. 미신뢰
//!   프로젝트도 같은 길이다 — git CLI 를 띄우지 않는다. 작업 트리는 `git.conflict.get` 이 그 파일의 `version` 을 만들려고 바이트만
//!   읽으므로(git 을 거치지 않는다) 필터 드라이버가 돌 일도 없다. 자동 병합 결과와 구간은 [`merge`](diff3)가 계산한다. 각 쪽의
//!   내용은 대용량 기준(결정 M1-D9)을 따른다: 50 MiB 를 넘으면 풀지 않고, 5 MiB 를 넘으면 `git.conflict.get` 만 `git.blob` 과 같은
//!   차례(`crate::diff::blob::BIG_BLOBS`)를 받아 푼다 — 목록은 그 쪽을 풀지 않는다(내용이 필요 없다).
//! - 경로는 index 의 바이트로 맞춘다. UTF-8 이 아닌 경로는 목록에 U+FFFD 로 보이지만 get · resolve 로 지목할 수 없다(invalid
//!   params) — 같은 글자로 보이는 다른 이름을 만들지 않게.
//! - 해결(`git.conflict.resolve`): 쓰기 실행기([`crate::write`])로 돈다 — 미저장 문서 확인은 그 경로 하나다. 두 갈래 모두 작업
//!   트리 파일이 `git.conflict.get` 이 준 `version` 그대로일 때만 한다(아니면 `CONFLICT` — `fs.write` 의 `expectedVersion` 과
//!   같다). 묻기 전에 한 번, 저장소 잠금 안에서 그 경로가 아직 충돌 중인지 다시 읽으며 한 번 더 견준 뒤:
//!   - `content`: `fs.write` 와 같은 경로별 쓰기 잠금([`pw_fs::Fs::write_lock`]) 안에서 다시 견주고 원자
//!     쓰기(`pw_fs::write::write_atomic`)를 한 뒤 `git add`. 50 MiB 를 넘는 작업 트리 파일은 덮지 않는다. 경로 판정은 파일 조작(M1-21)의 규칙 A
//!     에 git 의 판정 하나를 더한 것이다 — 앞 폴더 중 하나라도 심링크면 거절하고(git 의 「beyond a symbolic link」: 작업 트리 안을
//!     가리키는 링크여도 따라가면 다른 git 경로에 쓴다), 들어 있는 폴더를 실제 경로로 풀어 작업 트리 안이어야 하고, 그 항목이
//!     심링크면 대상도 작업 트리 안이어야 한다. 쓰기는 항목 자체를 바꾼다(심링크면 링크가 일반 파일이 된다 — `git add` 가 링크를
//!     담지 않게).
//!   - `side`: 그쪽이 있으면 `git checkout --ours|--theirs` 뒤 `git add` — git 이 smudge 필터 · 줄바꿈 변환을 하고(LFS · autocrlf
//!     에서도 맞는 파일), 앞 폴더의 심링크를 따라 쓰지 않는다. 서브모듈이면 `git update-index --cacheinfo` 로 index 만 바꾼다(서브모듈
//!     체크아웃은 그대로 — merge 가 서브모듈을 옮기지 않는 것과 같다). 그쪽이 지운 쪽이면 `git rm`. git 이 파일을 쓰므로 쓰기
//!     잠금은 쥐지 않는다 — 잠금 안 확인과 git 의 쓰기 사이에 같은 프로세스의 `fs.write` 가 끼면 그 저장은 덮인다(다른 프로세스의
//!     쓰기와 같은 수준).
//!
//!   경로는 글자 그대로 넘긴다(`--literal-pathspecs`). 해결 뒤의 충돌 목록을 돌려준다.

pub mod merge;
pub mod methods;

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use gix::ObjectId;
use gix::bstr::ByteSlice;
use gix::index::entry::Mode;
use pw_core::registry::path::normalize;
use pw_fs::judge::{Bom, Judgment, judge};
use pw_fs::limits::{EDITABLE_MAX, READ_ONLY_MAX, SNIFF_BYTES};
use pw_fs::read::read_whole;
use pw_fs::scope::{Denied, Roots};
use pw_rpc::{ConnectionScope, RpcError, code};

use crate::diff::blob::{BIG_BLOB_WAIT, BIG_BLOBS, BIG_BLOBS_AT_ONCE};
use crate::status::GitConflict;
use crate::write::{Changes, GitWrite, WriteError, WriteRequest};
use crate::{Git, Opened};
use methods::{
    ConflictChoice, ConflictContent, ConflictFile, ConflictPick, ConflictSide, ConflictStage,
    GitConflictDetail, GitConflictParams, GitConflictResolveParams, GitConflicts,
    GitConflictsParams, MergeLines, MergeRegion, MergeRegionKind,
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
        let file = entry.file(sides.iter().flatten().map(Side::seen));
        let (merged, regions) = match merged(&sides) {
            Some((text, regions)) => (Some(text), regions),
            None => (None, Vec::new()),
        };
        let clean =
            merged.is_some() && !regions.iter().any(|r| r.kind == MergeRegionKind::Conflict);
        // 못 읽는 파일(권한 등)이어도 세 쪽은 보인다 — 그 파일에 쓰는 해결이 그 오류를 낸다.
        let version = work_version(&opened.target.work_tree, &params.path)
            .ok()
            .flatten();
        let [base, ours, theirs] = sides.map(|s| s.map(Side::into_rpc));
        Ok(GitConflictDetail {
            work_tree: crate::status::display(&opened.target.work_tree),
            file,
            version,
            base,
            ours,
            theirs,
            merged,
            clean,
            regions,
        })
    }
}

impl GitWrite {
    /// `git.conflict.resolve`(모듈 머리 주석).
    pub async fn resolve(
        &self,
        caller: ConnectionScope,
        params: GitConflictResolveParams,
    ) -> Result<GitConflicts, WriteError> {
        check_path(&params.path).map_err(WriteError::Rpc)?;
        let resolution = match (params.content, params.side) {
            (
                Some(ConflictContent {
                    text,
                    bom,
                    expected_version,
                }),
                None,
            ) => {
                let mut bytes = Vec::with_capacity(text.len() + UTF8_BOM.len());
                if bom {
                    bytes.extend_from_slice(UTF8_BOM);
                }
                bytes.extend_from_slice(text.as_bytes());
                Resolution::Content {
                    bytes: bytes.into(),
                    expected: expected_version,
                }
            }
            (
                None,
                Some(ConflictChoice {
                    pick,
                    expected_version,
                }),
            ) => Resolution::Side {
                pick,
                expected: expected_version,
            },
            _ => {
                return Err(WriteError::Rpc(RpcError::invalid_params(
                    "give exactly one of `content` or `side`",
                )));
            }
        };
        let path = params.path;
        // 미저장 확인을 묻기 전에 거절할 것은 거절한다(충돌 중이 아님 · 내용으로 풀 수 없는 갈래 · 쓸 수 없는 자리 · 그사이 바뀐
        // 파일). 잠금 안에서 다시 본다.
        let precheck = {
            let (git, project, worktree, path, resolution) = (
                Arc::clone(self.git()),
                params.project_id.clone(),
                params.worktree.clone(),
                path.clone(),
                resolution.clone(),
            );
            tokio::task::spawn_blocking(move || {
                let Some(opened) = git
                    .open(&project, worktree.as_deref())
                    .map_err(WriteError::Rpc)?
                else {
                    return Err(WriteError::NoRepository);
                };
                let entry = find(&opened.repo, &path).map_err(WriteError::Rpc)?;
                fits(&entry, &resolution).map_err(WriteError::Rpc)?;
                let work_tree = &opened.target.work_tree;
                if matches!(resolution, Resolution::Content { .. }) {
                    place(work_tree, &path).map_err(WriteError::Rpc)?;
                }
                unchanged(work_tree, &path, &resolution)
            })
            .await
            .map_err(|e| WriteError::Internal(e.to_string()))?
        };
        precheck?;
        let request = WriteRequest {
            caller,
            project_id: params.project_id,
            worktree: params.worktree,
            title: RESOLVE_TITLE.to_owned(),
            changes: Changes::Paths(vec![path.clone()]),
            askpass: false,
        };
        let fs = Arc::clone(self.fs());
        #[cfg(test)]
        let before_write = self
            .before_conflict_write
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
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
            let work_tree = ctx.work_tree().to_path_buf();
            match resolution {
                Resolution::Content { .. } => {
                    let rel = path.clone();
                    tokio::task::spawn_blocking(move || {
                        // `fs.write` 와 같은 쓰기 잠금 안에서 확인하고 쓴다 — 같은 파일의 저장과 둘 다 확인을 지나 하나가 사라지지
                        // 않게.
                        let Place { item, .. } =
                            place(&work_tree, &rel).map_err(WriteError::Rpc)?;
                        let real = normalize(&item).unwrap_or(item);
                        let _guard = fs.write_lock(&real);
                        unchanged(&work_tree, &rel, &resolution)?;
                        #[cfg(test)]
                        if let Some(hook) = before_write {
                            hook();
                        }
                        let Resolution::Content { bytes, .. } = &resolution else {
                            unreachable!()
                        };
                        write_content(&work_tree, &rel, bytes)
                    })
                    .await
                    .map_err(|e| WriteError::Internal(e.to_string()))??;
                    ctx.run(literal(&["add", "--", &path])).await?;
                }
                Resolution::Side { pick, .. } => {
                    // git 이 작업 트리 파일을 덮거나 지우기 전에 잠금 안에서 다시 본다.
                    let rel = path.clone();
                    tokio::task::spawn_blocking(move || unchanged(&work_tree, &rel, &resolution))
                        .await
                        .map_err(|e| WriteError::Internal(e.to_string()))??;
                    match entry.side(pick) {
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
                    }
                }
            }
            ctx.read(list).await?.map_err(WriteError::Rpc)
        })
        .await
    }
}

#[derive(Clone)]
enum Resolution {
    Content {
        bytes: Arc<[u8]>,
        /// `git.conflict.get` 이 준 작업 트리 파일의 `version`(없던 파일이면 `None`).
        expected: Option<String>,
    },
    Side {
        pick: ConflictPick,
        /// `Content` 의 `expected` 와 같다.
        expected: Option<String>,
    },
}

impl Resolution {
    fn expected(&self) -> Option<&String> {
        match self {
            Self::Content { expected, .. } | Self::Side { expected, .. } => expected.as_ref(),
        }
    }
}

/// 내용으로는 서브모듈 · 심링크 충돌을 풀 수 없다 — 한쪽을 고른다.
fn fits(entry: &Entry, resolution: &Resolution) -> Result<(), RpcError> {
    if matches!(resolution, Resolution::Content { .. })
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
    /// index 의 경로 바이트.
    raw: Vec<u8>,
    /// 보이는 이름(UTF-8 이 아닌 바이트는 U+FFFD).
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

    /// `seen`: 있는 쪽마다 하나.
    fn file(&self, seen: impl Iterator<Item = Seen>) -> ConflictFile {
        let [base, ours, theirs] = self.stages.map(|s| s.as_ref().map(Stage::rpc));
        let mut file = ConflictFile {
            path: self.path.clone(),
            kind: self.kind(),
            base,
            ours,
            theirs,
            binary: false,
            submodule: self.has(Mode::COMMIT),
            symlink: self.has(Mode::SYMLINK),
            large: false,
            too_large: false,
        };
        for side in seen {
            file.binary |= side.binary;
            file.large |= side.large;
            file.too_large |= side.too_large;
        }
        file
    }
}

/// 목록에 싣는 한 쪽의 표시. 바이너리 판정은 5 MiB 이하인 쪽만 한다 — 목록은 그보다 큰 쪽을 풀지 않고, get 도 같은 값을 낸다.
#[derive(Debug, Clone, Copy, Default)]
struct Seen {
    binary: bool,
    large: bool,
    too_large: bool,
}

impl Seen {
    /// 목록의 한 쪽: 크기는 header 로 보고, 5 MiB 이하만 풀어 앞 8 KiB 로 판정한다(풀어 둔 내용은 바로 버린다).
    fn of(repo: &gix::Repository, stage: Stage) -> Result<Self, RpcError> {
        if stage.mode == Mode::COMMIT {
            return Ok(Self::default());
        }
        let size = repo.find_header(stage.id).map_err(internal)?.size();
        if size > EDITABLE_MAX {
            return Ok(Self {
                binary: false,
                large: true,
                too_large: size > READ_ONLY_MAX,
            });
        }
        let data = repo.find_object(stage.id).map_err(internal)?.detach().data;
        let head = &data[..data.len().min(SNIFF_BYTES)];
        let bom = bom_len(head);
        Ok(Self {
            binary: judge(head, &data[bom..], size, false).binary,
            ..Self::default()
        })
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
                        raw: path.to_vec(),
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

/// 충돌 중인 `path`(index 의 바이트가 같은 것). 아니면 not found. UTF-8 이 아닌 경로가 U+FFFD 로 그 글자가 되면 invalid params —
/// 그 이름으로 쓰면 다른 파일을 만든다.
fn find(repo: &gix::Repository, path: &str) -> Result<Entry, RpcError> {
    let entries = entries(repo)?;
    if let Some(at) = entries.iter().position(|e| e.raw == path.as_bytes()) {
        return Ok(entries.into_iter().nth(at).expect("found above"));
    }
    if entries.iter().any(|e| e.path == path) {
        return Err(RpcError::invalid_params(format!(
            "`{path}` is not UTF-8 in the index and cannot be named here; resolve it with git"
        )));
    }
    Err(RpcError::new(
        code::NOT_FOUND,
        format!("`{path}` is not in conflict"),
    ))
}

/// 충돌 목록. 바이너리 판정에 5 MiB 이하인 쪽의 blob 을 읽는다([`Seen::of`]).
fn list(opened: &Opened) -> Result<GitConflicts, RpcError> {
    let repo = &opened.repo;
    let mut files = Vec::new();
    for entry in entries(repo)? {
        let seen = entry
            .stages
            .iter()
            .flatten()
            .map(|&stage| Seen::of(repo, stage))
            .collect::<Result<Vec<_>, _>>()?;
        files.push(entry.file(seen.into_iter()));
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

    /// 목록과 같은 표시([`Seen`]).
    fn seen(&self) -> Seen {
        let large = self.size > EDITABLE_MAX;
        Seen {
            binary: !large && self.judgment.as_ref().is_some_and(|j| j.binary),
            large,
            too_large: self.too_large,
        }
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
    Some((text, regions_of(&merged)))
}

/// 병합 결과의 구간을 RPC 꼴로.
pub(crate) fn regions_of(merged: &merge::Merged<'_>) -> Vec<MergeRegion> {
    let line_range = |r: &std::ops::Range<u32>| MergeLines {
        start: r.start,
        count: r.end - r.start,
    };
    merged
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
        .collect()
}

/// `content` 해결의 쓸 자리(모듈 머리 주석의 규칙 A): 들어 있는 폴더(실제 경로)와 그 아래의 항목. `version` 도 이 판정을
/// 거친 자리만 읽는다.
struct Place {
    parent: PathBuf,
    item: PathBuf,
}

fn place(work_tree: &Path, rel: &str) -> Result<Place, RpcError> {
    let refuse = |d: Denied| RpcError::invalid_params(d.to_string());
    // git 과 같이 앞 폴더의 링크는 대상이 어디든 따라가지 않는다(`git add` 의 「beyond a symbolic link」). 없는 폴더부터는 만든다.
    let mut at = work_tree.to_path_buf();
    let leading = rel.rsplit_once('/').map_or("", |(dir, _)| dir);
    for part in leading.split('/').filter(|p| !p.is_empty()) {
        at.push(part);
        match std::fs::symlink_metadata(&at) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(RpcError::invalid_params(format!(
                    "`{rel}` is beyond a symbolic link"
                )));
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => break,
            Err(e) => return Err(internal(format!("`{rel}`: {e}"))),
        }
    }
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
    Ok(Place { parent, item })
}

/// 작업 트리의 그 파일의 `version`(`fs.read` 와 같은 값 — 50 MiB 를 넘으면 [`large_version`]). 일반 파일이 없으면(없음 · 폴더 ·
/// 루트 밖 링크 · 링크 앞 폴더) `None`.
fn work_version(work_tree: &Path, rel: &str) -> std::io::Result<Option<String>> {
    Ok(work_file(work_tree, rel)?.map(|(_, version)| version))
}

/// (크기, `version`). 일반 파일이 없으면 `None`.
fn work_file(work_tree: &Path, rel: &str) -> std::io::Result<Option<(u64, String)>> {
    let Ok(Place { item, .. }) = place(work_tree, rel) else {
        return Ok(None);
    };
    let roots = Roots::new([work_tree.to_path_buf()]);
    let Ok(readable) = roots.readable(&item) else {
        return Ok(None);
    };
    let meta = match std::fs::metadata(&readable.real) {
        Ok(meta) if meta.is_file() => meta,
        Ok(_) => return Ok(None),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    if meta.len() > READ_ONLY_MAX {
        return Ok(Some((meta.len(), large_version(&meta))));
    }
    let snapshot = read_whole(&readable.real, &|p| roots.open_inside(p))?;
    Ok(Some((snapshot.bytes.len() as u64, snapshot.version())))
}

/// 50 MiB 를 넘는 파일의 `version`: 내용을 읽지 않고 크기 · mtime · inode(유닉스)로 만든다. 원자 쓰기(rename)는 inode 가, 보통의
/// 덮어쓰기는 mtime 이 바꾼다 — 크기 · mtime 이 같은 제자리 편집은 못 잡는다. 내용 해시의 `version` 과 섞이지 않게 `large-` 로
/// 시작한다.
fn large_version(meta: &std::fs::Metadata) -> String {
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos());
    #[cfg(unix)]
    let inode = {
        use std::os::unix::fs::MetadataExt as _;
        format!("-{}-{}", meta.dev(), meta.ino())
    };
    #[cfg(not(unix))]
    let inode = String::new();
    format!("large-{}-{mtime}{inode}", meta.len())
}

/// 해결 전: 작업 트리 파일이 `git.conflict.get` 이 준 `version` 그대로인가. 다르면 `CONFLICT`(`fs.write` 와 같은 꼴 —
/// `data.current` 가 지금의 `version`). 내용 쓰기는 50 MiB 를 넘는 파일을 덮지 않는다(내용으로 견줄 수 없다).
fn unchanged(work_tree: &Path, rel: &str, resolution: &Resolution) -> Result<(), WriteError> {
    let current = work_file(work_tree, rel)
        .map_err(|e| WriteError::Internal(format!("read `{rel}`: {e}")))?;
    if let (Some((size, _)), Resolution::Content { .. }) = (&current, resolution)
        && *size > READ_ONLY_MAX
    {
        return Err(WriteError::Rpc(RpcError::invalid_params(format!(
            "`{rel}` is {size} bytes in the work tree — too large to compare before writing"
        ))));
    }
    let current = current.map(|(_, version)| version);
    if current.as_ref() != resolution.expected() {
        return Err(WriteError::Rpc(
            RpcError::new(code::CONFLICT, "conflict")
                .with_data(serde_json::json!({ "path": rel, "current": current })),
        ));
    }
    Ok(())
}

/// `content` 해결의 파일 쓰기([`place`] 의 판정).
fn write_content(work_tree: &Path, rel: &str, bytes: &[u8]) -> Result<(), WriteError> {
    let refuse = |d: Denied| WriteError::Rpc(RpcError::invalid_params(d.to_string()));
    let roots = Roots::new([work_tree.to_path_buf()]);
    let Place { parent, item } = place(work_tree, rel).map_err(WriteError::Rpc)?;
    let requested = work_tree.join(rel);
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
