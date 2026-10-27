//! `--follow` 의 원본 찾기(git 의 `try_to_follow_renames` — `diffcore-rename.c` · `diffcore-delta.c` 를 옮겼다).
//!
//! 경로가 새로 생긴 커밋에서, 부모 트리의 **모든** 파일(`--find-copies-harder`)을 원본 후보로 그 경로의 새 내용과 견준다:
//!
//! 1. 같은 객체(정확히 같은 내용 — 링크 · 서브모듈이면 모드까지 같은 것)가 있으면 그것. 여럿이면 git 의 점수(새 트리에서 지워진
//!    원본 1 + 기본 이름이 같으면 1)가 높은 것, 같으면 경로 순서로 앞의 것.
//! 2. 없으면 일반 파일끼리 비슷함 점수(git 의 spanhash — 줄 또는 64바이트 조각의 해시가 겹치는 바이트 수 ÷ 큰 쪽 크기)를 매겨
//!    (점수, 기본 이름이 같은가) 순으로 상위 4개만 남긴다(`NUM_CANDIDATE_PER_DST`). 그중 50% 이상에서 먼저 지워진 원본(이름 바뀜)을,
//!    없으면 남은 원본(복사)까지 본다 — git 은 복사 원본을 처음부터 「쓰인 것」 으로 세어 첫 패스에서 뺀다.
//!
//! 원본이 새 트리에 남아 있으면 복사(`C`), 없으면 이름 바뀜(`R`)이다. 빈 파일은 견주지 않는다.
//!
//! gix 의 이름 바뀜 추적(`gix::diff::Rewrites`)을 쓰지 않는 까닭: 커밋의 바뀜이 그 경로의 추가 하나뿐이면 gix 는 복사 원본 찾기를
//! 건너뛴다(gix-diff 0.70 의 `Tracker::emit` 조기 종료 — 지운 파일과 추가가 짝을 이루지 않으면 할 일이 없다고 본다). 지운 파일을
//! 다른 파일의 복사로 다시 만든 커밋에서 git 은 `C` 를 보이는데 gix 는 `A` 다. 비슷함 계산 · 후보 고르기도 git 과 같게
//! 하려고 둘 다 옮겼다.

use gix::ObjectId;
use gix::bstr::{BStr, BString, ByteSlice};
use gix::objs::FindExt;
use gix::objs::tree::EntryKind;
use pw_rpc::RpcError;

use super::internal;

/// git 의 `MAX_SCORE`.
const MAX_SCORE: u64 = 60_000;
/// git 의 기본 비슷함 하한(`DEFAULT_RENAME_SCORE` — 50%).
const MIN_SCORE: u64 = 30_000;
/// git 의 `HASHBASE`.
const HASHBASE: u32 = 107_927;
/// 대상마다 남기는 비슷함 후보 수(git 의 `NUM_CANDIDATE_PER_DST`).
const CANDIDATES: usize = 4;
/// 바이너리 판정에 보는 앞부분(git 의 `FIRST_FEW_BYTES`).
const FIRST_FEW_BYTES: usize = 8000;

/// 원본 후보 하나.
struct Source {
    path: BString,
    id: ObjectId,
    kind: EntryKind,
}

/// `path` 가 `old` → `new` 에서 새로 생겼을 때 그 원본: (원본 경로, 복사인가). 없으면 `None`.
pub fn follow(
    repo: &gix::Repository,
    old: ObjectId,
    new: ObjectId,
    path: &BStr,
) -> Result<Option<(BString, bool)>, RpcError> {
    let new_tree = repo.find_tree(new).map_err(internal)?;
    let Some(dst) = new_tree
        .lookup_entry_by_path(path.to_path_lossy())
        .map_err(internal)?
    else {
        return Ok(None);
    };
    let (dst_id, dst_kind) = (dst.object_id(), dst.mode().kind());
    if !matches!(
        dst_kind,
        EntryKind::Blob | EntryKind::BlobExecutable | EntryKind::Link
    ) {
        return Ok(None);
    }
    let sources = sources(repo, old)?;
    let base = basename(path);
    let deleted = |s: &Source| -> Result<bool, RpcError> {
        Ok(new_tree
            .lookup_entry_by_path(s.path.to_path_lossy())
            .map_err(internal)?
            .is_none())
    };
    let mut buf = Vec::new();
    let dst_size = repo.find_header(dst_id).map_err(internal)?.size();
    if dst_size == 0 {
        return Ok(None);
    }
    // 1. 정확히 같은 내용.
    let mut exact: Option<(u8, &Source)> = None;
    for s in &sources {
        let modes_fit = (is_file(s.kind) && is_file(dst_kind)) || s.kind == dst_kind;
        if s.id != dst_id || !modes_fit {
            continue;
        }
        let score = u8::from(deleted(s)?) + u8::from(basename(s.path.as_bstr()) == base);
        if exact.is_none_or(|(best, _)| score > best) {
            exact = Some((score, s));
        }
    }
    let found = match exact {
        Some((_, s)) => Some(s.path.clone()),
        None if is_file(dst_kind) => {
            let dst_data = repo
                .objects
                .find_blob(&dst_id, &mut buf)
                .map_err(internal)?
                .data
                .to_vec();
            let dst_hash = Spans::of(&dst_data);
            // git 의 후보 칸(`record_if_better`): 원본마다(점수 0 인 것 — 링크 · 서브모듈 · 크기가 너무 다른 것 — 까지) 가장 나쁜
            // 칸보다 나으면 그 칸에 넣는다. 칸의 자리가 동률일 때의 차례를 정한다(아래 안정 정렬).
            let mut slots: [Option<Candidate<'_>>; CANDIDATES] = [None; CANDIDATES];
            for s in &sources {
                let mut score = 0;
                if is_file(s.kind) {
                    let size = repo.find_header(s.id).map_err(internal)?.size();
                    if size > 0 && sizes_close(size, dst_size) {
                        let data = repo
                            .objects
                            .find_blob(&s.id, &mut buf)
                            .map_err(internal)?
                            .data;
                        score =
                            Spans::of(data).copied_into(&dst_hash) * MAX_SCORE / size.max(dst_size);
                    }
                }
                let o = Candidate {
                    score,
                    same_name: basename(s.path.as_bstr()) == base,
                    source: s,
                };
                let mut worst = 0;
                for i in 1..CANDIDATES {
                    if worse(slots[i].as_ref(), slots[worst].as_ref()) {
                        worst = i;
                    }
                }
                if worse(slots[worst].as_ref(), Some(&o)) {
                    slots[worst] = Some(o);
                }
            }
            let mut kept: Vec<Candidate<'_>> = slots.into_iter().flatten().collect();
            // 안정 정렬: 점수가 높은 것, 같으면 기본 이름이 같은 것.
            kept.sort_by_key(|c| std::cmp::Reverse((c.score, c.same_name)));
            kept.retain(|c| c.score >= MIN_SCORE);
            let mut pick = None;
            for c in &kept {
                if deleted(c.source)? {
                    pick = Some(c.source);
                    break;
                }
            }
            pick.or_else(|| kept.first().map(|c| c.source))
                .map(|s| s.path.clone())
        }
        None => None,
    };
    let Some(source) = found else {
        return Ok(None);
    };
    let copy = new_tree
        .lookup_entry_by_path(source.to_path_lossy())
        .map_err(internal)?
        .is_some();
    Ok(Some((source, copy)))
}

/// 비슷함 후보 칸 하나.
#[derive(Clone, Copy)]
struct Candidate<'a> {
    score: u64,
    same_name: bool,
    source: &'a Source,
}

/// git 의 `score_compare(a, b) > 0` — `a` 가 `b` 보다 뒤에 설 것인가(빈 칸은 맨 뒤).
fn worse(a: Option<&Candidate<'_>>, b: Option<&Candidate<'_>>) -> bool {
    match (a, b) {
        (None, b) => b.is_some(),
        (Some(_), None) => false,
        (Some(a), Some(b)) => (b.score, b.same_name) > (a.score, a.same_name),
    }
}

/// 일반 파일(실행 비트와 상관없이)인가 — 링크 · 서브모듈이 아니다.
fn is_file(kind: EntryKind) -> bool {
    matches!(kind, EntryKind::Blob | EntryKind::BlobExecutable)
}

fn basename(path: &BStr) -> &BStr {
    path.rfind_byte(b'/')
        .map_or(path, |i| &path[i + 1..])
        .as_bstr()
}

/// git 의 크기 거르기: 크기 차이가 작은 쪽 기준으로 너무 크면 견주지 않는다.
fn sizes_close(a: u64, b: u64) -> bool {
    let (max, min) = (a.max(b), a.min(b));
    max * (MAX_SCORE - MIN_SCORE) >= (max - min) * MAX_SCORE
}

/// 부모 트리의 파일 · 링크 · 서브모듈 전부(경로 순서 — git 의 diff 차례와 같다).
fn sources(repo: &gix::Repository, tree: ObjectId) -> Result<Vec<Source>, RpcError> {
    let mut out = Vec::new();
    let mut stack: Vec<(BString, ObjectId)> = vec![(BString::default(), tree)];
    let mut buf = Vec::new();
    while let Some((prefix, id)) = stack.pop() {
        let iter = repo
            .objects
            .find_tree_iter(&id, &mut buf)
            .map_err(internal)?;
        for entry in iter {
            let entry = entry.map_err(internal)?;
            let mut path = prefix.clone();
            if !path.is_empty() {
                path.push(b'/');
            }
            path.extend_from_slice(entry.filename);
            match entry.mode.kind() {
                EntryKind::Tree => stack.push((path, entry.oid.to_owned())),
                // 서브모듈도 원본 후보다(점수는 늘 0 — 후보 칸의 자리만 차지한다).
                kind => out.push(Source {
                    path,
                    id: entry.oid.to_owned(),
                    kind,
                }),
            }
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

/// 내용을 조각(줄, 줄이 길면 64바이트)으로 나눈 해시와 그 해시의 바이트 수(git 의 `hash_chars`). 해시 순서로 정렬돼 있다.
struct Spans(Vec<(u32, u64)>);

impl Spans {
    fn of(data: &[u8]) -> Self {
        let text = !data[..data.len().min(FIRST_FEW_BYTES)].contains(&0);
        let mut counts: std::collections::HashMap<u32, u64> = std::collections::HashMap::new();
        let (mut accum1, mut accum2, mut n) = (0u32, 0u32, 0u64);
        let mut flush = |accum1: u32, accum2: u32, n: u64| {
            let hash = accum1.wrapping_add(accum2.wrapping_mul(0x61)) % HASHBASE;
            *counts.entry(hash).or_default() += n;
        };
        for (i, &c) in data.iter().enumerate() {
            // 텍스트면 CRLF 의 CR 을 건너뛴다.
            if text && c == b'\r' && data.get(i + 1) == Some(&b'\n') {
                continue;
            }
            let old1 = accum1;
            accum1 = (accum1 << 7) ^ (accum2 >> 25);
            accum2 = (accum2 << 7) ^ (old1 >> 25);
            accum1 = accum1.wrapping_add(u32::from(c));
            n += 1;
            if n < 64 && c != b'\n' {
                continue;
            }
            flush(accum1, accum2, n);
            (accum1, accum2, n) = (0, 0, 0);
        }
        if n > 0 {
            flush(accum1, accum2, n);
        }
        let mut spans: Vec<(u32, u64)> = counts.into_iter().collect();
        spans.sort_unstable();
        Self(spans)
    }

    /// 이 쪽(원본)에서 `dst` 로 옮겨 간 바이트 수 — 해시마다 두 쪽 바이트 수의 작은 쪽의 합(git 의 `diffcore_count_changes` 의
    /// `src_copied`).
    fn copied_into(&self, dst: &Spans) -> u64 {
        let (mut i, mut j, mut copied) = (0, 0, 0);
        while i < self.0.len() && j < dst.0.len() {
            let (sh, sc) = self.0[i];
            let (dh, dc) = dst.0[j];
            match sh.cmp(&dh) {
                std::cmp::Ordering::Less => i += 1,
                std::cmp::Ordering::Greater => j += 1,
                std::cmp::Ordering::Equal => {
                    copied += sc.min(dc);
                    i += 1;
                    j += 1;
                }
            }
        }
        copied
    }
}
