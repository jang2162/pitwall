//! 두 텍스트의 줄 diff 를 `git diff -U3` 과 같은 hunk 로 만든다(결정 M2-D9 — diff 는 Rust 에서 계산한다).
//!
//! git(xdiff)과 같게 하는 것:
//! - 줄은 줄바꿈(`\n`)까지다. 비교에 줄바꿈이 들어가므로 파일 끝 개행이 없는 마지막 줄은 개행이 있는 같은 글의 줄과 다르다
//!   (`\ No newline at end of file`). CR 은 줄 내용이다(CRLF 파일의 줄은 `\r` 로 끝난다).
//! - 알고리즘은 저장소 설정 `diff.algorithm`(기본 Myers)이고, 그 뒤에 git 의 기본값(`diff.indentHeuristic = true`)과 같은 들여쓰기
//!   휴리스틱으로 변경 구간을 옮긴다(`imara-diff` 의 git 포팅).
//! - 문맥 3줄, 변경 사이의 같은 줄이 6줄(문맥의 두 배) 이하면 한 hunk 로 묶는다. 문맥 줄은 새 쪽의 줄이다(공백 무시에서 다를 수 있다).
//! - `@@` 뒤의 함수 줄은 git 의 기본 규칙(`def_ff`): hunk 시작 바로 위에서부터 옛 쪽을 거슬러 올라가며 첫 글자가 영문자 · `_` · `$`
//!   인 줄을 80바이트에서 자르고 끝 공백을 뗀 것. 앞 hunk 와의 사이에서 못 찾으면 앞 hunk 의 것을 그대로 쓴다. diff 드라이버의
//!   `xfuncname` 은 보지 않는다.
//! - 공백 무시(`-w`): 공백(` ` · `\t` · `\n` · `\r`)을 모두 빼고 비교한다.

use std::borrow::Cow;
use std::ops::Range;

use gix::diff::blob::{Algorithm, Diff, IndentHeuristic, IndentLevel, SliderHeuristic};
use gix::diff::blob::{InternedInput, Interner, Token};

use super::methods::{DiffHunk, DiffLine, LineKind};

/// 앞뒤 문맥 줄 수(`-U3`).
pub const CONTEXT: u32 = 3;

/// 함수 줄의 최대 바이트(xdiff 의 `func_line.buf`).
const FUNC_LINE_MAX: usize = 80;

/// git 의 `isspace`(`sane_ctype` — 로캘을 보지 않는다).
fn is_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r')
}

/// 줄바꿈을 포함해 줄로 나눈다. 마지막 줄은 줄바꿈이 없을 수 있다. 빈 입력은 줄이 없다.
pub fn lines(data: &[u8]) -> Vec<&[u8]> {
    data.split_inclusive(|&b| b == b'\n').collect()
}

/// `old` → `new` 의 hunk. 같으면 비어 있다.
pub fn hunks(
    old: &[u8],
    new: &[u8],
    algorithm: Algorithm,
    ignore_whitespace: bool,
) -> Vec<DiffHunk> {
    let (old, new) = (lines(old), lines(new));
    let changes = changes(&old, &new, algorithm, ignore_whitespace);
    emit(&old, &new, &changes)
}

/// 변경 구간(옛 줄 범위 → 새 줄 범위). 순서대로이고 겹치지 않는다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub before: Range<u32>,
    pub after: Range<u32>,
}

/// 줄 diff. 줄 하나가 토큰 하나다 — 공백 무시면 공백을 뺀 내용이 토큰의 열쇠다.
pub fn changes<'a>(
    old: &[&'a [u8]],
    new: &[&'a [u8]],
    algorithm: Algorithm,
    ignore_whitespace: bool,
) -> Vec<Change> {
    let mut input = InternedInput {
        before: Vec::with_capacity(old.len()),
        after: Vec::with_capacity(new.len()),
        interner: Interner::new(old.len() + new.len()),
    };
    let mut intern = |line: &'a [u8]| -> Token {
        let key: Cow<'a, [u8]> = if ignore_whitespace {
            Cow::Owned(line.iter().copied().filter(|&b| !is_space(b)).collect())
        } else {
            Cow::Borrowed(line)
        };
        input.interner.intern(key)
    };
    let before: Vec<Token> = old.iter().map(|l| intern(l)).collect();
    let after: Vec<Token> = new.iter().map(|l| intern(l)).collect();
    input.before = before;
    input.after = after;
    let mut diff = Diff::compute(algorithm, &input);
    // 들여쓰기 휴리스틱은 그 자리의 실제 줄을 본다 — 공백 무시면 `}` 와 `    }` 가 한 토큰이라 토큰으로는 들여쓰기를 알 수 없다.
    // 휴리스틱이 받는 토큰 열은 들여쓰기를 찾는 데만 쓰이므로(같음 비교는 하지 않는다) 줄 번호를 토큰 삼은 열로 바꿔 넘긴다.
    let lines: Vec<&[u8]> = old.iter().chain(new).copied().collect();
    let before_at: Vec<Token> = (0..old.len() as u32).map(Token).collect();
    let after_at: Vec<Token> = (old.len() as u32..lines.len() as u32).map(Token).collect();
    let mut indent = IndentHeuristic::new(|t: Token| {
        IndentLevel::for_ascii_line(lines[t.0 as usize].iter().copied(), 8)
    });
    let before_ptr = input.before.as_ptr();
    diff.postprocess_with(
        &input.before,
        &input.after,
        |tokens: &[Token], hunk: Range<u32>, earliest_end: u32| {
            let at = if tokens.as_ptr() == before_ptr {
                &before_at
            } else {
                &after_at
            };
            indent.best_slider_end(at, hunk, earliest_end)
        },
    );
    diff.hunks()
        .map(|h| Change {
            before: h.before,
            after: h.after,
        })
        .collect()
}

/// 변경 구간을 `-U3` hunk 로 묶어 줄을 채운다.
pub fn emit(old: &[&[u8]], new: &[&[u8]], changes: &[Change]) -> Vec<DiffHunk> {
    let (n1, n2) = (old.len() as u32, new.len() as u32);
    let mut out = Vec::new();
    let mut func: &[u8] = &[];
    // 함수 줄을 찾은 아래 끝(이 줄 위로는 앞 hunk 가 이미 봤다). xdiff 의 `funclineprev`.
    let mut searched_to: i64 = -1;
    let mut i = 0;
    while i < changes.len() {
        // xdiff `xdl_get_hunk`: 다음 변경과의 사이가 문맥 두 배 이하면 같은 hunk.
        let mut j = i;
        while j + 1 < changes.len()
            && changes[j + 1].before.start - changes[j].before.end <= 2 * CONTEXT
        {
            j += 1;
        }
        let (first, last) = (&changes[i], &changes[j]);
        let s1 = first.before.start.saturating_sub(CONTEXT);
        let s2 = first.after.start.saturating_sub(CONTEXT);
        let e1 = (last.before.end + CONTEXT).min(n1);
        let e2 = (last.after.end + CONTEXT).min(n2);

        let start = i64::from(s1) - 1;
        let mut l = start;
        while l > searched_to && l >= 0 {
            if let Some(found) = function_line(old[l as usize]) {
                func = found;
                break;
            }
            l -= 1;
        }
        searched_to = start;

        let mut lines = Vec::new();
        let (mut a, mut b) = (s1, s2);
        let context = |a: u32, b: u32, lines: &mut Vec<DiffLine>| {
            lines.push(line(LineKind::Context, new[b as usize], Some(a), Some(b)));
        };
        for change in &changes[i..=j] {
            while a < change.before.start {
                context(a, b, &mut lines);
                a += 1;
                b += 1;
            }
            for k in change.before.clone() {
                lines.push(line(LineKind::Removed, old[k as usize], Some(k), None));
            }
            for k in change.after.clone() {
                lines.push(line(LineKind::Added, new[k as usize], None, Some(k)));
            }
            a = change.before.end;
            b = change.after.end;
        }
        while a < e1 && b < e2 {
            context(a, b, &mut lines);
            a += 1;
            b += 1;
        }
        let (c1, c2) = (e1 - s1, e2 - s2);
        out.push(DiffHunk {
            // git 은 줄이 없는 쪽의 시작을 그 앞 줄 번호로 적는다(`@@ -0,0 +1 @@`).
            old_start: if c1 == 0 { s1 } else { s1 + 1 },
            old_lines: c1,
            new_start: if c2 == 0 { s2 } else { s2 + 1 },
            new_lines: c2,
            header: String::from_utf8_lossy(func).into_owned(),
            lines,
        });
        i = j + 1;
    }
    out
}

/// xdiff `def_ff`.
fn function_line(line: &[u8]) -> Option<&[u8]> {
    let first = *line.first()?;
    if !(first.is_ascii_alphabetic() || first == b'_' || first == b'$') {
        return None;
    }
    let mut len = line.len().min(FUNC_LINE_MAX);
    while len > 0 && is_space(line[len - 1]) {
        len -= 1;
    }
    Some(&line[..len])
}

/// 0 기준 줄 번호를 받아 1 기준으로 싣는다.
fn line(kind: LineKind, raw: &[u8], old: Option<u32>, new: Option<u32>) -> DiffLine {
    let no_newline = !raw.ends_with(b"\n");
    let text = if no_newline {
        raw
    } else {
        &raw[..raw.len() - 1]
    };
    DiffLine {
        kind,
        text: String::from_utf8_lossy(text).into_owned(),
        old_line: old.map(|n| n + 1),
        new_line: new.map(|n| n + 1),
        no_newline,
    }
}
