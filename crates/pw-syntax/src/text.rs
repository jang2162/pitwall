//! 문서 텍스트와 위치 변환. RPC 의 위치는 UTF-16 코드 단위(웹뷰 CodeMirror 의 위치 — JS 문자열 색인)이고 tree-sitter 는
//! UTF-8 바이트로 일한다. 줄마다 시작 바이트 · 시작 UTF-16 위치를 적어 둔 표([`LineIndex`])로 바꾼다.

use tree_sitter::{InputEdit, Point};

#[derive(Debug, Clone, Copy)]
struct Line {
    byte: usize,
    utf16: usize,
    /// 줄이 ASCII 뿐이면 줄 안에서 바이트 = UTF-16 위치다.
    ascii: bool,
}

/// 줄(`\n` 으로 나눈다)마다의 시작 위치. 텍스트가 바뀌면 새로 만든다.
#[derive(Debug, Clone)]
pub struct LineIndex {
    lines: Vec<Line>,
    len_bytes: usize,
    len_utf16: usize,
    /// 가장 긴 줄의 글자 수(줄바꿈 뺀 유니코드 스칼라 수 — `pw_fs::judge` 의 셈과 같다).
    longest_line: usize,
}

impl LineIndex {
    pub fn new(text: &str) -> Self {
        let mut lines = Vec::new();
        let mut utf16 = 0;
        let mut start = 0;
        let mut longest_line = 0;
        for segment in text.split_inclusive('\n') {
            let ascii = segment.is_ascii();
            lines.push(Line {
                byte: start,
                utf16,
                ascii,
            });
            let body = segment.strip_suffix('\n').unwrap_or(segment);
            longest_line = longest_line.max(if ascii {
                body.len()
            } else {
                body.chars().count()
            });
            utf16 += utf16_len(segment);
            start += segment.len();
        }
        // 끝이 `\n` 이거나 빈 텍스트면 마지막 빈 줄이 하나 더 있다.
        if text.is_empty() || text.ends_with('\n') {
            lines.push(Line {
                byte: start,
                utf16,
                ascii: true,
            });
        }
        Self {
            lines,
            len_bytes: text.len(),
            len_utf16: utf16,
            longest_line,
        }
    }

    pub fn len_utf16(&self) -> usize {
        self.len_utf16
    }

    pub fn longest_line(&self) -> usize {
        self.longest_line
    }

    /// UTF-16 위치 → 바이트. 텍스트 밖이거나 대리 쌍 가운데면 `None`.
    pub fn byte_of(&self, text: &str, utf16: usize) -> Option<usize> {
        if utf16 > self.len_utf16 {
            return None;
        }
        let line = self.lines[self.lines.partition_point(|l| l.utf16 <= utf16) - 1];
        if line.ascii {
            return Some(line.byte + (utf16 - line.utf16));
        }
        let mut at = line.utf16;
        for (offset, c) in text[line.byte..].char_indices() {
            if at == utf16 {
                return Some(line.byte + offset);
            }
            at += c.len_utf16();
            if at > utf16 {
                return None;
            }
        }
        (at == utf16).then_some(self.len_bytes)
    }

    /// 바이트(글자 경계) → UTF-16 위치.
    pub fn utf16_of(&self, text: &str, byte: usize) -> usize {
        let line = self.lines[self.lines.partition_point(|l| l.byte <= byte) - 1];
        if line.ascii {
            line.utf16 + (byte - line.byte)
        } else {
            line.utf16 + text[line.byte..byte].encode_utf16().count()
        }
    }

    /// 바이트 → tree-sitter 의 점(줄, 줄 안 바이트).
    pub fn point_of(&self, byte: usize) -> Point {
        let row = self.lines.partition_point(|l| l.byte <= byte) - 1;
        Point {
            row,
            column: byte - self.lines[row].byte,
        }
    }
}

/// 바꾸기 하나: UTF-16 위치 `from..to` 를 `insert` 로. 위치는 앞 바꾸기를 적용한 뒤의 텍스트 기준이다.
#[derive(Debug, Clone, Copy)]
pub struct Change<'a> {
    pub from: usize,
    pub to: usize,
    pub insert: &'a str,
}

/// [`apply`] 의 결과.
pub struct Applied {
    pub text: String,
    pub index: LineIndex,
    /// tree-sitter 에 차례로 알릴 편집.
    pub edits: Vec<InputEdit>,
}

/// 바꾸기들을 차례로 적용한다. 바꾸기가 없으면 `None`. 하나라도 범위가 텍스트 밖 · 대리 쌍 가운데 · 거꾸로면 오류다.
///
/// 위치가 앞으로만 나아가는 바꾸기 줄(CodeMirror 의 `iterChanges` 가 내는 모양 — 여러 커서 편집 · 바꾸기 모두)은 한 번에
/// 적용한다: 새 문자열과 줄 표를 한 번만 만들고 tree-sitter 편집도 하나로 합친다. 바꾸기마다 텍스트 전체를 다시 만들면 5 MB
/// 문서의 바꾸기 1,000개가 수 초 걸린다. 앞으로 되돌아가는 바꾸기에서 줄을 끊어 다음 줄로 넘긴다.
pub fn apply(
    text: &str,
    index: &LineIndex,
    changes: &[Change<'_>],
) -> Result<Option<Applied>, String> {
    let mut current: Option<Applied> = None;
    let mut rest = changes;
    while !rest.is_empty() {
        let n = forward_run(rest);
        let (base_text, base_index) = match &current {
            Some(a) => (a.text.as_str(), &a.index),
            None => (text, index),
        };
        let (next_text, next_index, edit) = apply_run(base_text, base_index, &rest[..n])?;
        let mut edits = current.map(|a| a.edits).unwrap_or_default();
        edits.push(edit);
        current = Some(Applied {
            text: next_text,
            index: next_index,
            edits,
        });
        rest = &rest[n..];
    }
    Ok(current)
}

/// 앞에서부터 위치가 앞으로만 나아가는 바꾸기 수: 다음 바꾸기가 앞 바꾸기가 넣은 글 뒤에서 시작한다.
fn forward_run(changes: &[Change<'_>]) -> usize {
    let mut end = changes[0].from.saturating_add(utf16_len(changes[0].insert));
    for (n, c) in changes.iter().enumerate().skip(1) {
        if c.from < end {
            return n;
        }
        end = c.from.saturating_add(utf16_len(c.insert));
    }
    changes.len()
}

/// 앞으로만 나아가는 바꾸기 줄 하나(비어 있지 않다)를 한 번에 적용한다.
fn apply_run(
    text: &str,
    index: &LineIndex,
    run: &[Change<'_>],
) -> Result<(String, LineIndex, InputEdit), String> {
    // 위치를 바꾸기 전 텍스트 기준으로 옮긴다 — 앞 바꾸기들이 늘리고 줄인 길이(UTF-16)를 뺀다. 앞으로만 나아가므로 옮긴
    // 범위들도 겹치지 않고 차례다.
    let mut shift: i64 = 0;
    let mut pieces = Vec::with_capacity(run.len());
    for c in run {
        if c.from > c.to {
            return Err(format!("change range {}..{} is reversed", c.from, c.to));
        }
        let from =
            usize::try_from(c.from as i64 - shift).expect("forward runs never move before 0");
        let to = from + (c.to - c.from);
        let (Some(start), Some(old_end)) = (index.byte_of(text, from), index.byte_of(text, to))
        else {
            return Err(format!(
                "change range {}..{} is outside the document (length {}) or splits a surrogate pair",
                c.from,
                c.to,
                index.len_utf16() as i64 + shift
            ));
        };
        pieces.push((start, old_end, c.insert));
        shift += utf16_len(c.insert) as i64 - (c.to - c.from) as i64;
    }
    let added: usize = pieces.iter().map(|p| p.2.len()).sum();
    let removed: usize = pieces.iter().map(|p| p.1 - p.0).sum();
    let mut out = String::with_capacity(text.len() - removed + added);
    let mut at = 0;
    for (start, old_end, insert) in &pieces {
        out.push_str(&text[at..*start]);
        out.push_str(insert);
        at = *old_end;
    }
    out.push_str(&text[at..]);
    let next_index = LineIndex::new(&out);

    let start = pieces[0].0;
    let old_end = pieces[pieces.len() - 1].1;
    let new_end = old_end - removed + added;
    let edit = InputEdit {
        start_byte: start,
        old_end_byte: old_end,
        new_end_byte: new_end,
        start_position: index.point_of(start),
        old_end_position: index.point_of(old_end),
        new_end_position: next_index.point_of(new_end),
    };
    Ok((out, next_index, edit))
}

fn utf16_len(s: &str) -> usize {
    if s.is_ascii() {
        s.len()
    } else {
        s.encode_utf16().count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_between_utf16_and_bytes_across_lines() {
        // `é` 는 UTF-8 2바이트 · UTF-16 1단위, `😀` 는 4바이트 · 2단위.
        let text = "ab\nxé😀y\n";
        let index = LineIndex::new(text);
        assert_eq!(index.len_utf16(), 9);
        let pairs = [
            (0, 0),
            (2, 2),
            (3, 3),
            (4, 4),
            (5, 6),
            (7, 10),
            (8, 11),
            (9, 12),
        ];
        for (utf16, byte) in pairs {
            assert_eq!(index.byte_of(text, utf16), Some(byte), "utf16 {utf16}");
            assert_eq!(index.utf16_of(text, byte), utf16, "byte {byte}");
        }
        // 대리 쌍 가운데 · 끝 너머.
        assert_eq!(index.byte_of(text, 6), None);
        assert_eq!(index.byte_of(text, 10), None);
        assert_eq!(index.point_of(10), Point { row: 1, column: 7 });
        assert_eq!(index.point_of(12), Point { row: 2, column: 0 });
    }

    fn change(from: usize, to: usize, insert: &str) -> Change<'_> {
        Change { from, to, insert }
    }

    /// 바꾸기를 하나씩 UTF-16 단위로 적용한 결과(견줄 기준).
    fn one_by_one(text: &str, changes: &[Change<'_>]) -> String {
        let mut units: Vec<u16> = text.encode_utf16().collect();
        for c in changes {
            units.splice(c.from..c.to, c.insert.encode_utf16());
        }
        String::from_utf16(&units).unwrap()
    }

    #[test]
    fn applies_a_change_and_reports_the_edit() {
        let text = "fn é() {}\nlet x;\n";
        let index = LineIndex::new(text);
        // `é()` 의 `é` (UTF-16 3) 를 두 줄로 바꾼다.
        let applied = apply(text, &index, &[change(3, 4, "a\nb")])
            .unwrap()
            .unwrap();
        assert_eq!(applied.text, "fn a\nb() {}\nlet x;\n");
        let [edit] = applied.edits[..] else {
            panic!("{:?}", applied.edits)
        };
        assert_eq!(
            (edit.start_byte, edit.old_end_byte, edit.new_end_byte),
            (3, 5, 6)
        );
        assert_eq!(edit.start_position, Point { row: 0, column: 3 });
        assert_eq!(edit.old_end_position, Point { row: 0, column: 5 });
        assert_eq!(edit.new_end_position, Point { row: 1, column: 1 });
        assert_eq!(
            applied.index.len_utf16(),
            applied.text.encode_utf16().count()
        );
    }

    #[test]
    fn rejects_ranges_outside_the_text() {
        let text = "a😀";
        let index = LineIndex::new(text);
        for (from, to) in [(0, 4), (2, 2), (2, 1)] {
            assert!(
                apply(text, &index, &[change(from, to, "x")]).is_err(),
                "{from}..{to}"
            );
        }
        // 뒤의 바꾸기 하나가 틀리면 전체가 오류다.
        assert!(apply(text, &index, &[change(0, 0, "x"), change(9, 9, "")]).is_err());
        let applied = apply(text, &index, &[change(3, 3, "!")]).unwrap().unwrap();
        assert_eq!(applied.text, "a😀!");
        assert!(apply(text, &index, &[]).unwrap().is_none());
    }

    /// 앞으로만 나아가는 바꾸기는 편집 하나로 합치고, 되돌아가는 곳에서 끊는다. 어느 쪽이든 하나씩 적용한 것과 같은 텍스트다.
    #[test]
    fn forward_changes_are_applied_in_one_pass_like_one_at_a_time() {
        let text = "ab\ncé😀d\nefg\n";
        let index = LineIndex::new(text);
        let cases: [(&[Change<'_>], usize); 5] = [
            // 여러 커서로 한 글자씩: 위치는 앞 바꾸기를 적용한 뒤 기준이라 넣은 만큼 밀린다.
            (
                &[change(0, 0, "X"), change(4, 4, "Y"), change(12, 12, "Z")],
                1,
            ),
            // 바로 이어 붙이기 · 지우기 · 비ASCII 넣기 · 대리 쌍 통째로 지우기.
            (&[change(1, 2, ""), change(1, 1, "→"), change(5, 7, "")], 1),
            // 뒤에서 앞으로: 바꾸기마다 끊긴다.
            (
                &[change(10, 11, "1"), change(4, 5, "2"), change(0, 1, "3")],
                3,
            ),
            // 앞으로 둘, 되돌아가 하나, 다시 앞으로.
            (
                &[
                    change(2, 3, "+\n"),
                    change(6, 6, "é"),
                    change(0, 0, "<"),
                    change(14, 14, ">"),
                ],
                2,
            ),
            // 텍스트 끝에 넣기.
            (&[change(13, 13, "end"), change(16, 16, "\n")], 1),
        ];
        for (changes, runs) in cases {
            let applied = apply(text, &index, changes).unwrap().unwrap();
            assert_eq!(applied.text, one_by_one(text, changes), "{changes:?}");
            assert_eq!(applied.edits.len(), runs, "{changes:?}");
            let fresh = LineIndex::new(&applied.text);
            assert_eq!(applied.index.len_utf16(), fresh.len_utf16());
            assert_eq!(applied.index.longest_line(), fresh.longest_line());
        }
    }

    #[test]
    fn counts_the_longest_line_in_characters_without_the_line_break() {
        assert_eq!(LineIndex::new("").longest_line(), 0);
        assert_eq!(LineIndex::new("abc\nde\n").longest_line(), 3);
        // 한글 · 이모지는 한 글자씩이다(UTF-16 · 바이트 길이가 아니다).
        assert_eq!(LineIndex::new("a\n가나😀다\nb").longest_line(), 4);
    }
}
