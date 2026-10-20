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
}

impl LineIndex {
    pub fn new(text: &str) -> Self {
        let mut lines = Vec::new();
        let mut utf16 = 0;
        let mut start = 0;
        for segment in text.split_inclusive('\n') {
            let ascii = segment.is_ascii();
            lines.push(Line {
                byte: start,
                utf16,
                ascii,
            });
            utf16 += if ascii {
                segment.len()
            } else {
                segment.encode_utf16().count()
            };
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
        }
    }

    pub fn len_utf16(&self) -> usize {
        self.len_utf16
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

/// 바꾸기 하나를 텍스트에 적용하고 tree-sitter 에 알릴 편집을 돌려준다. 위치는 바꾸기 전 텍스트의 UTF-16 위치다.
/// 범위가 텍스트 밖 · 대리 쌍 가운데 · 거꾸로면 아무것도 바꾸지 않고 오류다.
pub fn apply(
    text: &mut String,
    index: &mut LineIndex,
    from: usize,
    to: usize,
    insert: &str,
) -> Result<InputEdit, String> {
    if from > to {
        return Err(format!("change range {from}..{to} is reversed"));
    }
    let len = index.len_utf16();
    let (Some(start), Some(old_end)) = (index.byte_of(text, from), index.byte_of(text, to)) else {
        return Err(format!(
            "change range {from}..{to} is outside the document (length {len}) or splits a surrogate pair"
        ));
    };
    let start_position = index.point_of(start);
    let old_end_position = index.point_of(old_end);
    text.replace_range(start..old_end, insert);
    let new_end = start + insert.len();
    *index = LineIndex::new(text);
    Ok(InputEdit {
        start_byte: start,
        old_end_byte: old_end,
        new_end_byte: new_end,
        start_position,
        old_end_position,
        new_end_position: index.point_of(new_end),
    })
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

    #[test]
    fn applies_a_change_and_reports_the_edit() {
        let mut text = String::from("fn é() {}\nlet x;\n");
        let mut index = LineIndex::new(&text);
        // `é()` 의 `é` (UTF-16 3) 를 두 줄로 바꾼다.
        let edit = apply(&mut text, &mut index, 3, 4, "a\nb").unwrap();
        assert_eq!(text, "fn a\nb() {}\nlet x;\n");
        assert_eq!(
            (edit.start_byte, edit.old_end_byte, edit.new_end_byte),
            (3, 5, 6)
        );
        assert_eq!(edit.start_position, Point { row: 0, column: 3 });
        assert_eq!(edit.old_end_position, Point { row: 0, column: 5 });
        assert_eq!(edit.new_end_position, Point { row: 1, column: 1 });
        assert_eq!(index.len_utf16(), text.encode_utf16().count());
    }

    #[test]
    fn rejects_ranges_outside_the_text_without_changing_it() {
        let mut text = String::from("a😀");
        let mut index = LineIndex::new(&text);
        for (from, to) in [(0, 4), (2, 2), (2, 1)] {
            assert!(
                apply(&mut text, &mut index, from, to, "x").is_err(),
                "{from}..{to}"
            );
        }
        assert_eq!(text, "a😀");
        apply(&mut text, &mut index, 3, 3, "!").unwrap();
        assert_eq!(text, "a😀!");
    }
}
