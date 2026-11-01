//! Host 가 쥐는 문서 텍스트(15-m4-plan.md M4-05, 결정 M4-D10).
//!
//! 웹뷰 문서 모델이 정본이고 Host 는 `lsp.document.change` 로 증분을 받는다. 위치는 **UTF-16 하나**다 — 웹뷰(CodeMirror)의 오프셋도,
//! LSP `Position.character` 도 UTF-16 단위다(`crate::capabilities` 는 `utf-16` 만 알린다). 그래서 줄 시작 표에 줄마다 (바이트,
//! UTF-16) 시작을 두고, 오프셋 ↔ `Position` 은 줄을 이분 탐색한 뒤 그 줄 안만 걷는다.
//!
//! 바꾸기([`LspTextChange`])는 `syntax.edit` 와 같은 꼴이다: 차례로 적용하고, 각 바꾸기의 위치는 앞 바꾸기를 적용한 뒤의 텍스트
//! 기준이다. LSP 의 증분 `didChange` 도 `contentChanges` 를 차례로 적용하므로 바꾸기 하나가 그대로 변경 하나가 된다 — 범위는 그
//! 바꾸기를 적용하기 **전** 텍스트의 위치다.
//!
//! 줄 끝은 `\n` 하나로 가른다. `\r\n` 의 `\r` 은 그 줄의 끝 글자로 남는다 — LSP 는 `\r` · `\r\n` 도 줄 끝으로 보지만, 웹뷰는 문서를
//! `\n` 으로 고쳐 보내므로(CodeMirror 의 `Text`) 이 차이가 드러나지 않는다.

use lsp_types::Position;
use pw_fs::limits::{EDITABLE_MAX, LONG_LINE_CHARS};
use serde::{Deserialize, Serialize};

/// 바꾸기 하나(UTF-16 범위 — `syntax.edit` 의 `TextChange` 와 같은 꼴).
#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct LspTextChange {
    pub from: u32,
    pub to: u32,
    pub insert: String,
}

/// 줄 하나의 시작.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LineStart {
    byte: usize,
    utf16: usize,
}

/// 서버에 보낼 수 없는 문서(편집 등급 밖 — 결정 M1-D9).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverLimit {
    /// 5 MiB 를 넘는다.
    TooLarge,
    /// 1만 자를 넘는 줄이 있다.
    LongLine,
}

/// 문서 텍스트와 줄 시작 표.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocText {
    text: String,
    lines: Vec<LineStart>,
    len_utf16: usize,
}

fn utf16_len(s: &str) -> usize {
    s.chars().map(char::len_utf16).sum()
}

impl DocText {
    pub fn new(text: impl Into<String>) -> Self {
        let text = text.into();
        let mut doc = Self {
            text,
            lines: Vec::new(),
            len_utf16: 0,
        };
        doc.reindex();
        doc
    }

    pub fn as_str(&self) -> &str {
        &self.text
    }

    pub fn len_utf16(&self) -> usize {
        self.len_utf16
    }

    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    /// 편집 등급(≤ 5 MiB · 1만 자 넘는 줄 없음 — 결정 M1-D9)이 아니면 그 까닭.
    pub fn over_limit(&self) -> Option<OverLimit> {
        if self.text.len() as u64 > EDITABLE_MAX {
            return Some(OverLimit::TooLarge);
        }
        // 줄 시작 표로 줄마다의 바이트 길이를 보고, 바이트로 넘는 줄만 글자를 센다(한 번의 편집마다 문서 전체를 걷지 않는다).
        let long = self.lines.iter().enumerate().any(|(i, start)| {
            let end = self
                .lines
                .get(i + 1)
                .map_or(self.text.len(), |next| next.byte - 1);
            end - start.byte > LONG_LINE_CHARS
                && self.text[start.byte..end].chars().count() > LONG_LINE_CHARS
        });
        long.then_some(OverLimit::LongLine)
    }

    /// 줄 시작 표를 처음부터 만든다.
    fn reindex(&mut self) {
        self.lines.clear();
        self.lines.push(LineStart { byte: 0, utf16: 0 });
        let mut utf16 = 0;
        for (i, ch) in self.text.char_indices() {
            utf16 += ch.len_utf16();
            if ch == '\n' {
                self.lines.push(LineStart { byte: i + 1, utf16 });
            }
        }
        self.len_utf16 = utf16;
    }

    /// 바이트 `from..to`(UTF-16 `from16..to16`)를 `insert` 로 바꾸고 표를 고친다: 지운 범위 안의 줄 시작은 빼고, 넣은 글의 줄
    /// 시작을 넣고, 뒤의 줄은 차이만큼 민다 — 문서 끝까지 다시 걷지 않는다.
    fn splice(&mut self, from: usize, to: usize, from16: usize, to16: usize, insert: &str) {
        self.text.replace_range(from..to, insert);
        // 줄 시작 s 앞의 `\n` 은 s - 1 이다 — 그것이 지운 범위 [from, to) 안이면 그 줄 시작이 사라진다.
        let first = self.lines.partition_point(|l| l.byte <= from);
        let last = self.lines.partition_point(|l| l.byte <= to);
        let mut added = Vec::new();
        let mut utf16 = from16;
        for (i, ch) in insert.char_indices() {
            utf16 += ch.len_utf16();
            if ch == '\n' {
                added.push(LineStart {
                    byte: from + i + 1,
                    utf16,
                });
            }
        }
        let inserted16 = utf16 - from16;
        for line in &mut self.lines[last..] {
            line.byte = line.byte - to + from + insert.len();
            line.utf16 = line.utf16 - to16 + from16 + inserted16;
        }
        self.lines.splice(first..last, added);
        self.len_utf16 = self.len_utf16 - to16 + from16 + inserted16;
    }

    /// UTF-16 오프셋이 든 줄(0부터).
    fn line_of(&self, utf16: usize) -> usize {
        match self.lines.binary_search_by(|l| l.utf16.cmp(&utf16)) {
            Ok(i) => i,
            Err(i) => i - 1,
        }
    }

    /// UTF-16 오프셋 → 바이트 오프셋. 텍스트 밖이거나 대리쌍 가운데면 `None`.
    pub fn byte_of(&self, utf16: usize) -> Option<usize> {
        if utf16 > self.len_utf16 {
            return None;
        }
        let line = self.lines[self.line_of(utf16)];
        let mut at = line.utf16;
        let mut byte = line.byte;
        for ch in self.text[line.byte..].chars() {
            if at == utf16 {
                return Some(byte);
            }
            at += ch.len_utf16();
            byte += ch.len_utf8();
            if at > utf16 {
                return None;
            }
        }
        (at == utf16).then_some(byte)
    }

    /// UTF-16 오프셋 → LSP 위치. 텍스트 밖이거나 대리쌍 가운데면 `None`.
    pub fn position(&self, utf16: usize) -> Option<Position> {
        self.byte_of(utf16)?;
        let line = self.line_of(utf16);
        let character = utf16 - self.lines[line].utf16;
        Some(Position::new(
            u32::try_from(line).ok()?,
            u32::try_from(character).ok()?,
        ))
    }

    /// LSP 위치 → UTF-16 오프셋. 줄 끝을 넘는 `character` 는 줄 끝으로, 마지막 줄을 넘는 줄은 문서 끝으로 맞춘다(LSP 명세의 「줄
    /// 길이보다 크면 줄 길이로」). 대리쌍 가운데는 그 글자 앞으로.
    pub fn offset(&self, position: Position) -> usize {
        let Some(line) = self.lines.get(position.line as usize) else {
            return self.len_utf16;
        };
        let end = self
            .lines
            .get(position.line as usize + 1)
            .map_or(self.len_utf16, |next| next.utf16 - 1);
        let want = line.utf16 + position.character as usize;
        if want >= end {
            return end;
        }
        // 대리쌍 가운데면 앞으로.
        let mut at = line.utf16;
        for ch in self.text[line.byte..].chars() {
            let next = at + ch.len_utf16();
            if next > want {
                return at;
            }
            at = next;
        }
        at
    }

    /// 바꾸기를 차례로 적용하고, 각 바꾸기의 LSP 증분 변경(적용 전 텍스트의 범위 + 넣은 글)을 돌려준다. 하나라도 범위가 틀리면
    /// 아무것도 바꾸지 않고 오류다.
    pub fn apply(
        &mut self,
        changes: &[LspTextChange],
    ) -> Result<Vec<lsp_types::TextDocumentContentChangeEvent>, String> {
        let mut next = self.clone();
        let mut events = Vec::with_capacity(changes.len());
        for change in changes {
            let (from, to) = (change.from as usize, change.to as usize);
            if from > to {
                return Err(format!("change {from}..{to} runs backwards"));
            }
            let (Some(start), Some(end)) = (next.position(from), next.position(to)) else {
                return Err(format!(
                    "change {from}..{to} is outside the text (length {}) or splits a character",
                    next.len_utf16
                ));
            };
            let (Some(from_byte), Some(to_byte)) = (next.byte_of(from), next.byte_of(to)) else {
                return Err(format!("change {from}..{to} splits a character"));
            };
            next.splice(from_byte, to_byte, from, to, &change.insert);
            events.push(lsp_types::TextDocumentContentChangeEvent {
                range: Some(lsp_types::Range::new(start, end)),
                range_length: None,
                text: change.insert.clone(),
            });
        }
        *self = next;
        Ok(events)
    }

    /// 전체 텍스트를 바꾼다(`Full` 동기화 서버에 보낼 꼴은 [`Self::as_str`]).
    pub fn replace(&mut self, text: impl Into<String>) {
        *self = Self::new(text);
    }

    /// 글 전체의 UTF-16 길이(오프셋이 아니라 디버그 · 시험용).
    pub fn utf16_len_of(s: &str) -> usize {
        utf16_len(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn change(from: u32, to: u32, insert: &str) -> LspTextChange {
        LspTextChange {
            from,
            to,
            insert: insert.to_owned(),
        }
    }

    #[test]
    fn positions_count_utf16_units_per_line() {
        // 한글은 1 단위, 이모지(🦀)는 대리쌍 2 단위.
        let doc = DocText::new("ab\n한글🦀x\n\nend");
        assert_eq!(doc.line_count(), 4);
        assert_eq!(doc.position(0), Some(Position::new(0, 0)));
        assert_eq!(doc.position(2), Some(Position::new(0, 2)));
        assert_eq!(doc.position(3), Some(Position::new(1, 0)));
        assert_eq!(doc.position(5), Some(Position::new(1, 2)));
        // 🦀 의 가운데는 위치가 아니다.
        assert_eq!(doc.position(6), None);
        assert_eq!(doc.position(7), Some(Position::new(1, 4)));
        assert_eq!(doc.position(9), Some(Position::new(2, 0)));
        assert_eq!(doc.position(13), Some(Position::new(3, 3)));
        assert_eq!(doc.position(14), None);
        for offset in [0, 2, 3, 5, 7, 8, 9, 10, 13] {
            assert_eq!(doc.offset(doc.position(offset).unwrap()), offset);
        }
        // 줄 끝을 넘는 글자는 줄 끝, 없는 줄은 문서 끝.
        assert_eq!(doc.offset(Position::new(0, 99)), 2);
        assert_eq!(doc.offset(Position::new(99, 0)), 13);
        // 대리쌍 가운데는 그 글자 앞.
        assert_eq!(doc.offset(Position::new(1, 3)), 5);
    }

    #[test]
    fn changes_apply_in_order_and_report_ranges_before_each_change() {
        let mut doc = DocText::new("fn main() {\n}\n");
        let events = doc
            .apply(&[
                change(12, 12, "    let 한 = \"🦀\";\n"),
                change(0, 2, "pub fn"),
            ])
            .unwrap();
        assert_eq!(doc.as_str(), "pub fn main() {\n    let 한 = \"🦀\";\n}\n");
        assert_eq!(
            events[0].range,
            Some(lsp_types::Range::new(
                Position::new(1, 0),
                Position::new(1, 0)
            ))
        );
        assert_eq!(
            events[1].range,
            Some(lsp_types::Range::new(
                Position::new(0, 0),
                Position::new(0, 2)
            ))
        );
        assert_eq!(doc.len_utf16(), DocText::utf16_len_of(doc.as_str()));
        // 줄 시작 표가 새 텍스트와 맞는다.
        assert_eq!(doc, DocText::new(doc.as_str().to_owned()));
    }

    #[test]
    fn a_bad_change_changes_nothing() {
        let mut doc = DocText::new("a🦀b");
        let before = doc.clone();
        assert!(doc.apply(&[change(0, 1, "x"), change(2, 2, "y")]).is_err());
        assert!(doc.apply(&[change(3, 9, "")]).is_err());
        assert!(doc.apply(&[change(3, 1, "")]).is_err());
        assert_eq!(doc, before);
    }

    #[test]
    fn deleting_and_joining_lines_keeps_the_table_right() {
        let mut doc = DocText::new("one\ntwo\nthree\n");
        doc.apply(&[change(3, 8, " ")]).unwrap();
        assert_eq!(doc.as_str(), "one three\n");
        assert_eq!(doc, DocText::new("one three\n"));
        doc.apply(&[change(9, 10, "")]).unwrap();
        assert_eq!(doc, DocText::new("one three"));
        doc.apply(&[change(0, 9, "\n\n")]).unwrap();
        assert_eq!(doc, DocText::new("\n\n"));
        assert_eq!(doc.position(2), Some(Position::new(2, 0)));
    }

    #[test]
    fn the_edit_tier_is_5_mib_and_10000_char_lines() {
        assert_eq!(DocText::new("x".repeat(LONG_LINE_CHARS)).over_limit(), None);
        assert_eq!(
            DocText::new("x".repeat(LONG_LINE_CHARS + 1)).over_limit(),
            Some(OverLimit::LongLine)
        );
        // 글자로 센다 — 1만 자 한글 줄은 3만 바이트지만 넘지 않는다.
        assert_eq!(
            DocText::new("한".repeat(LONG_LINE_CHARS)).over_limit(),
            None
        );
        let line = format!("{}\n", "x".repeat(99));
        let big = line.repeat((EDITABLE_MAX as usize) / line.len() + 1);
        assert_eq!(DocText::new(big).over_limit(), Some(OverLimit::TooLarge));
    }
}
