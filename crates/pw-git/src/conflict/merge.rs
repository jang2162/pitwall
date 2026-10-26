//! 3-way 병합의 구간 계산(diff3). git 의 `xdl_merge`(`xdiff/xmerge.c`)를 옮긴 것이라 `git merge-file -p --diff3` 와 같은 구간이
//! 나온다.
//!
//! git 과 같게 하는 것:
//! - 줄은 줄바꿈(`\n`)까지다(`crate::diff::hunks::lines` — CR 은 줄 내용이다). 두 diff(base → ours, base → theirs)는 Myers 이고, 그
//!   뒤에 xdiff 의 `xdl_change_compact` 처럼 변경 구간을 옮긴다 — 들여쓰기 휴리스틱은 쓰지 않는다(`git merge-file` 은 diff 옵션을
//!   주지 않는다).
//! - diff3 꼴은 xdiff 가 병합 수준을 `XDL_MERGE_EAGER` 로 낮춘다: 양쪽이 같은 자리를 같게 바꾼 것만 충돌이 아니고, 충돌 구간을 더
//!   좁히지 않는다(ours · theirs 가 같은 줄을 공유해도 `XDL_MERGE_ZEALOUS` 의 다듬기를 하지 않는다 — base 를 함께 보이기 때문).
//! - 한쪽 변경이 다른 쪽 변경에 닿기만 해도(사이에 같은 줄이 없으면) 충돌이다. 겹치거나 닿는 변경은 한 구간으로 묶인다.
//! - 충돌이 아닌 곳은 ours 의 줄이다(xdiff 의 `xdl_fill_merge_buffer` 와 같다).
//!
//! git 과 갈리는 것: 사람이 `merge.conflictStyle` 을 `merge`(기본) · `zdiff3` 로 두면 git 이 작업 트리에 쓴 충돌 표시는 다듬은 좁은
//! 구간이다. 여기 구간은 base 를 보이는 diff3 의 것이다. merge-ort 의 diff 알고리즘 설정도 보지 않는다(늘 Myers).

use std::ops::Range;

use gix::diff::blob::{Algorithm, Diff, InternedInput, Interner, Token};

use crate::diff::hunks::lines;

/// 병합 결과의 구간 하나. 줄 번호는 0 기준이고 범위는 끝을 뺀다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Region {
    pub kind: Kind,
    pub base: Range<u32>,
    pub ours: Range<u32>,
    pub theirs: Range<u32>,
    /// 병합 결과에서 이 구간이 차지하는 줄. 충돌은 비어 있다(그 자리에 고른 줄이 들어간다).
    pub merged: Range<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// ours 만 바꿨다 — 결과에 ours 의 줄이 들어 있다.
    Ours,
    /// theirs 만 바꿨다 — 결과에 theirs 의 줄이 들어 있다.
    Theirs,
    Conflict,
}

/// 병합 결과: 충돌 구간을 비운 결과의 줄과 구간 목록(앞에서부터).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Merged<'a> {
    pub lines: Vec<&'a [u8]>,
    pub regions: Vec<Region>,
}

impl Merged<'_> {
    pub fn text(&self) -> Vec<u8> {
        self.lines.concat()
    }

    pub fn conflicts(&self) -> impl Iterator<Item = &Region> {
        self.regions.iter().filter(|r| r.kind == Kind::Conflict)
    }
}

/// diff 의 변경 하나(xdiff 의 `xdchange_t`): base 의 `i1` 부터 `chg1` 줄이 다른 쪽의 `i2` 부터 `chg2` 줄로.
#[derive(Debug, Clone, Copy)]
struct Change {
    i1: i64,
    chg1: i64,
    i2: i64,
    chg2: i64,
}

/// xdiff 의 `xdmerge_t`. `mode` 는 0 충돌 · 1 ours · 2 theirs. `i0` · `i1` · `i2` 는 base · ours · theirs 의 줄.
#[derive(Debug, Clone, Copy)]
struct Chunk {
    mode: u8,
    i0: i64,
    chg0: i64,
    i1: i64,
    chg1: i64,
    i2: i64,
    chg2: i64,
}

/// `base` 를 공통 조상으로 `ours` · `theirs` 를 병합한다.
pub fn merge<'a>(base: &'a [u8], ours: &'a [u8], theirs: &'a [u8]) -> Merged<'a> {
    let (base, ours, theirs) = (lines(base), lines(ours), lines(theirs));
    let chunks = chunks(
        &ours,
        &theirs,
        base.len() as i64,
        &changes(&base, &ours),
        &changes(&base, &theirs),
    );
    let mut out = Merged {
        lines: Vec::with_capacity(ours.len()),
        regions: Vec::with_capacity(chunks.len()),
    };
    // xdiff `xdl_fill_merge_buffer`: 구간 사이는 ours 의 줄이다.
    let mut i = 0;
    for c in &chunks {
        out.lines.extend_from_slice(&ours[i..c.i1 as usize]);
        let at = out.lines.len() as u32;
        let kind = match c.mode {
            1 => {
                out.lines
                    .extend_from_slice(&ours[c.i1 as usize..(c.i1 + c.chg1) as usize]);
                Kind::Ours
            }
            2 => {
                out.lines
                    .extend_from_slice(&theirs[c.i2 as usize..(c.i2 + c.chg2) as usize]);
                Kind::Theirs
            }
            _ => Kind::Conflict,
        };
        out.regions.push(Region {
            kind,
            base: range(c.i0, c.chg0),
            ours: range(c.i1, c.chg1),
            theirs: range(c.i2, c.chg2),
            merged: at..out.lines.len() as u32,
        });
        i = (c.i1 + c.chg1) as usize;
    }
    out.lines.extend_from_slice(&ours[i..]);
    out
}

fn range(start: i64, count: i64) -> Range<u32> {
    start as u32..(start + count) as u32
}

/// `before` → `after` 의 줄 diff: Myers 에 xdiff `xdl_change_compact` 와 같은 구간 옮기기(휴리스틱 없음).
fn changes(before: &[&[u8]], after: &[&[u8]]) -> Vec<Change> {
    let mut interner = Interner::new(before.len() + after.len());
    let b: Vec<Token> = before.iter().map(|l| interner.intern(*l)).collect();
    let a: Vec<Token> = after.iter().map(|l| interner.intern(*l)).collect();
    let input = InternedInput {
        before: b,
        after: a,
        interner,
    };
    let mut diff = Diff::compute(Algorithm::Myers, &input);
    diff.postprocess_no_heuristic(&input);
    diff.hunks()
        .map(|h| Change {
            i1: i64::from(h.before.start),
            chg1: i64::from(h.before.end - h.before.start),
            i2: i64::from(h.after.start),
            chg2: i64::from(h.after.end - h.after.start),
        })
        .collect()
}

/// xdiff `xdl_append_merge`: 앞 구간과 ours 나 theirs 에서 겹치거나 닿으면 그 구간을 넓히고(종류가 다르면 충돌), 아니면 새 구간.
fn append(out: &mut Vec<Chunk>, c: Chunk) {
    if let Some(m) = out.last_mut()
        && (c.i1 <= m.i1 + m.chg1 || c.i2 <= m.i2 + m.chg2)
    {
        if c.mode != m.mode {
            m.mode = 0;
        }
        m.chg0 = c.i0 + c.chg0 - m.i0;
        m.chg1 = c.i1 + c.chg1 - m.i1;
        m.chg2 = c.i2 + c.chg2 - m.i2;
    } else {
        out.push(c);
    }
}

/// xdiff `xdl_do_merge` 의 구간 만들기(`XDL_MERGE_EAGER`, 다듬기 없음). `x1` 은 base → ours, `x2` 는 base → theirs 의 변경이다.
fn chunks(
    ours: &[&[u8]],
    theirs: &[&[u8]],
    base_len: i64,
    x1: &[Change],
    x2: &[Change],
) -> Vec<Chunk> {
    let (ours_len, theirs_len) = (ours.len() as i64, theirs.len() as i64);
    let mut out = Vec::new();
    let (mut a, mut b) = (0, 0);
    while a < x1.len() && b < x2.len() {
        let (s1, s2) = (x1[a], x2[b]);
        if s1.i1 + s1.chg1 < s2.i1 {
            append(
                &mut out,
                Chunk {
                    mode: 1,
                    i0: s1.i1,
                    chg0: s1.chg1,
                    i1: s1.i2,
                    chg1: s1.chg2,
                    i2: s2.i2 - s2.i1 + s1.i1,
                    chg2: s1.chg1,
                },
            );
            a += 1;
            continue;
        }
        if s2.i1 + s2.chg1 < s1.i1 {
            append(
                &mut out,
                Chunk {
                    mode: 2,
                    i0: s2.i1,
                    chg0: s2.chg1,
                    i1: s1.i2 - s1.i1 + s2.i1,
                    chg1: s2.chg1,
                    i2: s2.i2,
                    chg2: s2.chg2,
                },
            );
            b += 1;
            continue;
        }
        let same = s1.i1 == s2.i1
            && s1.chg1 == s2.chg1
            && s1.chg2 == s2.chg2
            && ours[s1.i2 as usize..(s1.i2 + s1.chg2) as usize]
                == theirs[s2.i2 as usize..(s2.i2 + s2.chg2) as usize];
        if !same {
            // 두 변경을 함께 덮는 base 범위로 넓히고, 그만큼 ours · theirs 쪽도 넓힌다.
            let off = s1.i1 - s2.i1;
            let ffo = off + s1.chg1 - s2.chg1;
            let (mut i0, mut i1, mut i2) = (s1.i1, s1.i2, s2.i2);
            if off > 0 {
                i0 -= off;
                i1 -= off;
            } else {
                i2 += off;
            }
            let mut chg0 = s1.i1 + s1.chg1 - i0;
            let mut chg1 = s1.i2 + s1.chg2 - i1;
            let mut chg2 = s2.i2 + s2.chg2 - i2;
            if ffo < 0 {
                chg0 -= ffo;
                chg1 -= ffo;
            } else {
                chg2 += ffo;
            }
            append(
                &mut out,
                Chunk {
                    mode: 0,
                    i0,
                    chg0,
                    i1,
                    chg1,
                    i2,
                    chg2,
                },
            );
        }
        let (e1, e2) = (s1.i1 + s1.chg1, s2.i1 + s2.chg1);
        if e1 >= e2 {
            b += 1;
        }
        if e2 >= e1 {
            a += 1;
        }
    }
    for s1 in &x1[a..] {
        append(
            &mut out,
            Chunk {
                mode: 1,
                i0: s1.i1,
                chg0: s1.chg1,
                i1: s1.i2,
                chg1: s1.chg2,
                i2: s1.i1 + theirs_len - base_len,
                chg2: s1.chg1,
            },
        );
    }
    for s2 in &x2[b..] {
        append(
            &mut out,
            Chunk {
                mode: 2,
                i0: s2.i1,
                chg0: s2.chg1,
                i1: s2.i1 + ours_len - base_len,
                chg1: s2.chg1,
                i2: s2.i2,
                chg2: s2.chg2,
            },
        );
    }
    out
}
