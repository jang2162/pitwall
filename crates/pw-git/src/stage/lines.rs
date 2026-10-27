//! 줄 단위 스테이징 · 언스테이징 · 버리기의 패치(13-m2-plan.md M2-07).
//!
//! 창은 `git.diff` 의 hunk(M2-04 꼴)를 그대로 돌려보내며 그 안의 줄을 고른다. Host 는 저장소 잠금 안에서 그 파일의 두 쪽을
//! `git.diff` 와 같은 길(gix 의 diff 파이프라인 — 저장소에 담긴 꼴 `ToGit`, 같은 알고리즘, 미신뢰면 필터 드라이버를 뗀다)로 다시
//! 읽어 hunk 를 만들고, 보낸 hunk 가 그중 하나와 머리 · 줄까지 같지 않으면 아무것도 쓰지 않는다(conflict — 그 사이 파일이 바뀌었다).
//!
//! 패치는 창이 보낸 글이 아니라 Host 가 읽은 바이트 줄로 만든다 — CRLF · 파일 끝 개행 없음 · UTF-8 이 아닌 바이트가 그대로 간다:
//! 1. 고른 줄로 목표 내용을 바이트로 만든다([`target`]). 스테이징은 옛 쪽(index)에 고른 변경만 더한 것, 언스테이징 · 버리기는 새
//!    쪽(index · 작업 트리)에서 고른 변경만 되돌린 것이다.
//! 2. 바뀌는 쪽의 지금 내용과 목표의 diff 를 unified 패치로 쓴다([`patch`]). 스테이징은 지금 → 목표를 `git apply --cached` 로,
//!    언스테이징 · 버리기는 목표 → 지금(= 고른 변경)을 `git apply --cached -R` · `git apply -R` 로 되돌린다. 작업 트리에 쓰는
//!    `git apply` 는 파일을 저장소의 꼴로 바꿔 읽고 작업 트리의 꼴로 바꿔 쓰므로(clean · smudge · 줄바꿈 변환) 패치는 저장소의 꼴이다.
//!
//! 바이너리 · 심링크 · 서브모듈 · 충돌 중(단계 1~3) · index 에 없는 파일, hunk 가 `git.diff` 의 파일 상한을 넘는 파일은 줄 단위로
//! 하지 않는다(invalid params — 파일 단위 RPC 로).

use std::collections::HashSet;
use std::path::Path;

use gix::ObjectId;
use gix::bstr::ByteSlice;
use gix::diff::blob::pipeline::{Mode, WorktreeRoots};
use gix::diff::blob::platform::prepare_diff::Operation;
use gix::diff::blob::{Algorithm, ResourceKind};
use gix::index::entry::Mode as IndexMode;
use gix::objs::tree::EntryKind;
use pw_fs::limits::READ_ONLY_MAX;
use pw_rpc::{RpcError, code as rpc_code};

use super::methods::HunkSelection;
use crate::Opened;
use crate::diff::changes::{FILE_HUNKS_MAX, WorkTree, without_filter_drivers, without_safecrlf};
use crate::diff::hunks::{self, CONTEXT, Change};
use crate::diff::methods::{DiffHunk, LineKind};

/// 줄 단위 쓰기의 종류.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    /// index ↔ 작업 트리의 줄을 index 에 담는다.
    Stage,
    /// HEAD ↔ index 의 줄을 index 에서 되돌린다.
    Unstage,
    /// index ↔ 작업 트리의 줄을 작업 트리에서 되돌린다.
    Discard,
}

impl Op {
    /// 패치를 적용하는 git 인자(패치는 stdin).
    pub fn apply_args(self) -> &'static [&'static str] {
        match self {
            Self::Stage => &["apply", "--cached", "--whitespace=nowarn", "-"],
            Self::Unstage => &["apply", "--cached", "-R", "--whitespace=nowarn", "-"],
            Self::Discard => &["apply", "-R", "--whitespace=nowarn", "-"],
        }
    }
}

/// 고른 변경 줄(0 기준): 뺀 줄은 옛 쪽 번호, 더한 줄은 새 쪽 번호.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Picked {
    pub removed: HashSet<u32>,
    pub added: HashSet<u32>,
}

/// 고른 것을 어느 쪽으로 쓰는가.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// 옛 쪽에 고른 변경만 더한다.
    Apply,
    /// 새 쪽에서 고른 변경만 되돌린다.
    Revert,
}

/// 잠금 안에서 그 파일의 두 쪽을 다시 읽고 고른 줄을 패치로 만든다. 바뀔 것이 없으면 `None`.
pub fn prepare(
    opened: &Opened,
    op: Op,
    path: &str,
    old_path: Option<&str>,
    selection: &[HunkSelection],
) -> Result<Option<Vec<u8>>, RpcError> {
    let mut repo = opened.repo.clone();
    if !opened.trusted {
        without_filter_drivers(&mut repo)?;
    }
    // `git add -p` 처럼 diff 를 읽을 때 `core.safecrlf` 를 보지 않는다(패치는 `git apply` 가 쓴다).
    without_safecrlf(&mut repo)?;
    let (old, new, algorithm) = sides(&repo, &opened.target.work_tree, op, path, old_path)?;
    let (old_lines, new_lines) = (hunks::lines(&old), hunks::lines(&new));
    let changes = hunks::changes(&old_lines, &new_lines, algorithm, false);
    if hunks::size(&old_lines, &new_lines, &changes) > FILE_HUNKS_MAX {
        return Err(RpcError::invalid_params(format!(
            "the diff of `{path}` is too large for line selection; use the whole file"
        )));
    }
    let shown = hunks::emit(&old_lines, &new_lines, &changes);
    let picked = pick(&shown, selection).map_err(|e| match e {
        PickError::Stale => RpcError::new(
            rpc_code::CONFLICT,
            format!("the diff of `{path}` changed since it was read; read git.diff again"),
        ),
        PickError::Invalid(message) => RpcError::invalid_params(message),
    })?;
    Ok(match op {
        Op::Stage => {
            let target = target(&old_lines, &new_lines, &changes, &picked, Direction::Apply);
            patch(path, &old, &target)
        }
        Op::Unstage | Op::Discard => {
            let target = target(&old_lines, &new_lines, &changes, &picked, Direction::Revert);
            patch(path, &target, &new)
        }
    })
}

/// 고른 줄을 풀지 못한 이유.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickError {
    /// 보낸 hunk 가 지금 diff 에 없다.
    Stale,
    Invalid(String),
}

/// 보낸 hunk 마다 지금 diff(`shown`)에서 같은 hunk 를 찾아 고른 줄을 모은다.
pub fn pick(shown: &[DiffHunk], selection: &[HunkSelection]) -> Result<Picked, PickError> {
    if selection.is_empty() {
        return Err(PickError::Invalid("no hunk is selected".into()));
    }
    let mut picked = Picked::default();
    for chosen in selection {
        if !shown.contains(&chosen.hunk) {
            return Err(PickError::Stale);
        }
        let lines = &chosen.hunk.lines;
        let indexes: Vec<usize> = match &chosen.lines {
            None => (0..lines.len())
                .filter(|&i| lines[i].kind != LineKind::Context)
                .collect(),
            Some(indexes) if indexes.is_empty() => {
                return Err(PickError::Invalid(
                    "a hunk is sent with no line selected".into(),
                ));
            }
            Some(indexes) => indexes.iter().map(|&i| i as usize).collect(),
        };
        for i in indexes {
            let line = lines.get(i).ok_or_else(|| {
                PickError::Invalid(format!(
                    "line {i} is outside the hunk ({} lines)",
                    lines.len()
                ))
            })?;
            // 같은 hunk 를 찾았으니 줄 번호는 지금 내용의 것이다.
            match (line.kind, line.old_line, line.new_line) {
                (LineKind::Removed, Some(n), _) => picked.removed.insert(n - 1),
                (LineKind::Added, _, Some(n)) => picked.added.insert(n - 1),
                _ => {
                    return Err(PickError::Invalid(format!(
                        "line {i} of the hunk is not an added or removed line"
                    )));
                }
            };
        }
    }
    Ok(picked)
}

/// 고른 변경으로 만든 목표 내용. `changes` 는 `old` → `new` 의 변경 구간이다. 한 구간 안에서 남는 옛 줄이 먼저, 더하는 새 줄이
/// 뒤에 온다(`git add -p` 의 hunk 편집과 같은 자리). [`Direction::Revert`] 는 고르지 않은 변경만 옛 쪽에 더한 것과 같다.
pub fn target(
    old: &[&[u8]],
    new: &[&[u8]],
    changes: &[Change],
    picked: &Picked,
    direction: Direction,
) -> Vec<u8> {
    let apply = direction == Direction::Apply;
    let mut out = Vec::new();
    let mut at = 0usize;
    for change in changes {
        while at < change.before.start as usize {
            push(&mut out, old[at]);
            at += 1;
        }
        for k in change.before.clone() {
            if picked.removed.contains(&k) != apply {
                push(&mut out, old[k as usize]);
            }
        }
        for k in change.after.clone() {
            if picked.added.contains(&k) == apply {
                push(&mut out, new[k as usize]);
            }
        }
        at = change.before.end as usize;
    }
    while at < old.len() {
        push(&mut out, old[at]);
        at += 1;
    }
    out
}

/// 줄을 잇는다. 앞 줄이 개행 없는 파일 끝 줄이었는데(옛 파일의 끝 줄을 남기고 그 뒤에 새 줄을 더할 때) 뒤에 줄이 오면 앞 줄에
/// 줄바꿈을 붙인다 — 이을 줄이 CRLF 면 CRLF 로.
fn push(out: &mut Vec<u8>, line: &[u8]) {
    if !out.is_empty() && !out.ends_with(b"\n") {
        out.extend_from_slice(if line.ends_with(b"\r\n") {
            b"\r\n"
        } else {
            b"\n"
        });
    }
    out.extend_from_slice(line);
}

/// `before` → `after` 의 unified 패치(`diff --git`, 문맥 3줄, 이름은 늘 따옴표로 감싼다). 같으면 `None`. `diff --git` 머리가
/// 있으므로 결과가 빈 내용이어도 git 은 파일을 지우지 않고 빈 파일로 둔다.
pub fn patch(path: &str, before: &[u8], after: &[u8]) -> Option<Vec<u8>> {
    if before == after {
        return None;
    }
    let (old, new) = (hunks::lines(before), hunks::lines(after));
    let changes = hunks::changes(&old, &new, Algorithm::Myers, false);
    let (a, b) = (quote(&format!("a/{path}")), quote(&format!("b/{path}")));
    let mut out = format!("diff --git {a} {b}\n--- {a}\n+++ {b}\n").into_bytes();
    let (n1, n2) = (old.len() as u32, new.len() as u32);
    let mut i = 0;
    while i < changes.len() {
        // 다음 변경과의 사이가 문맥 두 배 이하면 같은 hunk(`hunks` 와 같은 묶음).
        let mut j = i;
        while j + 1 < changes.len()
            && changes[j + 1].before.start - changes[j].before.end <= 2 * CONTEXT
        {
            j += 1;
        }
        let s1 = changes[i].before.start.saturating_sub(CONTEXT);
        let s2 = changes[i].after.start.saturating_sub(CONTEXT);
        let e1 = (changes[j].before.end + CONTEXT).min(n1);
        let e2 = (changes[j].after.end + CONTEXT).min(n2);
        let (c1, c2) = (e1 - s1, e2 - s2);
        // 줄이 없는 쪽의 시작은 그 앞 줄 번호다(`@@ -0,0 +1 @@`).
        let start = |s: u32, c: u32| if c == 0 { s } else { s + 1 };
        out.extend_from_slice(
            format!("@@ -{},{c1} +{},{c2} @@\n", start(s1, c1), start(s2, c2)).as_bytes(),
        );
        let mut at = s1;
        for change in &changes[i..=j] {
            while at < change.before.start {
                line(&mut out, b' ', old[at as usize]);
                at += 1;
            }
            for k in change.before.clone() {
                line(&mut out, b'-', old[k as usize]);
            }
            for k in change.after.clone() {
                line(&mut out, b'+', new[k as usize]);
            }
            at = change.before.end;
        }
        while at < e1 {
            line(&mut out, b' ', old[at as usize]);
            at += 1;
        }
        i = j + 1;
    }
    Some(out)
}

fn line(out: &mut Vec<u8>, prefix: u8, raw: &[u8]) {
    out.push(prefix);
    out.extend_from_slice(raw);
    if !raw.ends_with(b"\n") {
        out.extend_from_slice(b"\n\\ No newline at end of file\n");
    }
}

/// git 의 C 꼴 따옴표(`"…"`): `"` · `\` · 탭 · 줄바꿈은 역슬래시로, 그 밖의 제어 문자와 ASCII 밖의 바이트는 8진 `\ooo` 로.
/// `git apply` 가 이름을 그대로 푼다.
pub fn quote(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 2);
    out.push('"');
    for b in name.bytes() {
        match b {
            b'"' => out.push_str("\\\""),
            b'\\' => out.push_str("\\\\"),
            b'\t' => out.push_str("\\t"),
            b'\n' => out.push_str("\\n"),
            0x20..=0x7e => out.push(b as char),
            _ => out.push_str(&format!("\\{b:03o}")),
        }
    }
    out.push('"');
    out
}

/// 그 파일의 옛 쪽 · 새 쪽 내용(저장소에 담긴 꼴)과 diff 알고리즘 — `git.diff` 가 hunk 를 만든 것과 같은 길이다.
fn sides(
    repo: &gix::Repository,
    work_tree: &Path,
    op: Op,
    path: &str,
    old_path: Option<&str>,
) -> Result<(Vec<u8>, Vec<u8>, Algorithm), RpcError> {
    let index = repo.index_or_empty().map_err(internal)?;
    let rel = path.as_bytes().as_bstr();
    if index
        .entry_range(rel)
        .is_some_and(|range| index.entries()[range].iter().any(|e| e.stage_raw() != 0))
    {
        return Err(RpcError::invalid_params(format!(
            "`{path}` has a conflict; resolve it or stage the whole file"
        )));
    }
    let staged = index
        .entry_by_path(rel)
        .ok_or_else(|| RpcError::invalid_params(format!("`{path}` is not in the index")))?;
    let staged_kind = regular(staged.mode)
        .ok_or_else(|| RpcError::invalid_params(format!("`{path}` is not a regular file")))?;
    let staged_id = staged.id;
    let roots = match op {
        Op::Stage | Op::Discard => WorktreeRoots {
            old_root: None,
            new_root: Some(work_tree.to_path_buf()),
        },
        Op::Unstage => WorktreeRoots::default(),
    };
    let mut cache = repo
        .diff_resource_cache(Mode::ToGit, roots)
        .map_err(internal)?;
    cache.filter.options.large_file_threshold_bytes = READ_ONLY_MAX;
    let hash = repo.object_hash();
    let read_error = |e: &dyn std::fmt::Display| internal(format!("cannot read `{path}`: {e}"));
    match op {
        Op::Stage | Op::Discard => {
            cache
                .set_resource(
                    staged_id,
                    staged_kind,
                    rel,
                    ResourceKind::OldOrSource,
                    &repo.objects,
                )
                .map_err(|e| read_error(&e))?;
            let file = WorkTree::new(work_tree)
                .check(rel, staged.mode)
                .map_err(RpcError::invalid_params)?;
            match std::fs::symlink_metadata(&file) {
                // 작업 트리 쪽에도 index 항목 id 를 준다 — `git.diff` 와 같이 「index 에 CRLF 가 있으면 줄바꿈을 바꾸지 않는다」를
                // 판정하게(`crate::diff::changes` 의 `contents`). 내용은 디스크에서 읽는다.
                Ok(meta) if meta.is_file() => cache
                    .set_resource(
                        staged_id,
                        staged_kind,
                        rel,
                        ResourceKind::NewOrDestination,
                        &repo.objects,
                    )
                    .map_err(|e| read_error(&e))?,
                Ok(_) => {
                    return Err(RpcError::invalid_params(format!(
                        "`{path}` is no longer a regular file in the work tree"
                    )));
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound && op == Op::Stage => {
                    // 작업 트리에서 지운 파일: 새 쪽은 빈 쪽이다. null id 는 루트가 있으면 디스크를 읽으라는 뜻이라 루트를 떼고 넣는다.
                    let roots = std::mem::take(&mut cache.filter.roots);
                    let set = cache.set_resource(
                        ObjectId::null(hash),
                        staged_kind,
                        rel,
                        ResourceKind::NewOrDestination,
                        &repo.objects,
                    );
                    cache.filter.roots = roots;
                    set.map_err(|e| read_error(&e))?;
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    return Err(RpcError::invalid_params(format!(
                        "`{path}` is deleted in the work tree; discard the whole file to restore it"
                    )));
                }
                Err(e) => return Err(read_error(&e)),
            }
        }
        Op::Unstage => {
            let head_path = old_path.unwrap_or(path);
            let head = match repo.head_commit() {
                Ok(commit) => commit
                    .tree()
                    .map_err(internal)?
                    .lookup_entry_by_path(head_path)
                    .map_err(internal)?,
                Err(_) => None,
            };
            match head {
                Some(entry) => {
                    let kind = entry.mode().kind();
                    if !matches!(kind, EntryKind::Blob | EntryKind::BlobExecutable) {
                        return Err(RpcError::invalid_params(format!(
                            "`{head_path}` is not a regular file in HEAD"
                        )));
                    }
                    cache
                        .set_resource(
                            entry.object_id(),
                            kind,
                            head_path.as_bytes().as_bstr(),
                            ResourceKind::OldOrSource,
                            &repo.objects,
                        )
                        .map_err(|e| read_error(&e))?;
                }
                // HEAD 에 없다(새로 더한 파일 · 커밋이 없는 저장소): 옛 쪽은 빈 쪽이다(루트가 없으니 null id 는 빈 내용).
                None => cache
                    .set_resource(
                        ObjectId::null(hash),
                        staged_kind,
                        rel,
                        ResourceKind::OldOrSource,
                        &repo.objects,
                    )
                    .map_err(|e| read_error(&e))?,
            }
            cache
                .set_resource(
                    staged_id,
                    staged_kind,
                    rel,
                    ResourceKind::NewOrDestination,
                    &repo.objects,
                )
                .map_err(|e| read_error(&e))?;
        }
    }
    let prepared = cache
        .prepare_diff()
        .map_err(|e| internal(format!("cannot diff `{path}`: {e}")))?;
    let algorithm = match prepared.operation {
        Operation::SourceOrDestinationIsBinary => {
            return Err(RpcError::invalid_params(format!(
                "`{path}` is binary (or too large); stage, unstage or discard the whole file"
            )));
        }
        Operation::InternalDiff { algorithm } => algorithm,
        // `git.diff` 와 같다 — 외부 diff 는 고르지 않는다.
        Operation::ExternalCommand { .. } => Algorithm::Myers,
    };
    let old = prepared.old.data.as_slice().unwrap_or_default().to_vec();
    let new = prepared.new.data.as_slice().unwrap_or_default().to_vec();
    Ok((old, new, algorithm))
}

/// 줄 단위로 다룰 수 있는 index 모드(보통 파일 · 실행 파일)의 종류.
fn regular(mode: IndexMode) -> Option<EntryKind> {
    match mode {
        IndexMode::FILE => Some(EntryKind::Blob),
        IndexMode::FILE_EXECUTABLE => Some(EntryKind::BlobExecutable),
        _ => None,
    }
}

fn internal(e: impl std::fmt::Display) -> RpcError {
    RpcError::internal(e.to_string())
}
