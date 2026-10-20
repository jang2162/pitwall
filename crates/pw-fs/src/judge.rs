//! 읽은 바이트의 판정과 크기 등급. 기준값은 `docs/design/12-m1-plan.md` 결정 M1-D9(대용량) · M1-D12(인코딩)이고, 에디터 ·
//! 검색 · 인덱스(M1-07 · M1-12 · M1-17 · M1-23)가 이 상수를 함께 쓴다.

use serde::{Deserialize, Serialize};

const MIB: u64 = 1024 * 1024;

/// 바이너리 판정에 보는 파일 앞부분.
pub const SNIFF_BYTES: usize = 8 * 1024;
/// 이 크기까지는 편집 + 강조.
pub const EDITABLE_MAX: u64 = 5 * MIB;
/// 이 크기까지는 읽기 전용(강조 · 줄바꿈 끔). 넘으면 열지 않는다. 범위 읽기 한 번의 상한이기도 하다.
pub const READ_ONLY_MAX: u64 = 50 * MIB;
/// 한 줄이 이 글자 수를 넘으면 강조 · 줄바꿈을 끈다.
pub const LONG_LINE_CHARS: usize = 10_000;

const UTF8_BOM: &[u8] = &[0xEF, 0xBB, 0xBF];

/// 크기 등급(결정 M1-D9).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum SizeClass {
    /// 5 MiB 이하: 편집 + 강조.
    Editable,
    /// 5 MiB 초과 50 MiB 이하: 읽기 전용, 강조 · 줄바꿈 끔.
    ReadOnly,
    /// 50 MiB 초과: 열지 않는다(앞부분 · hex 는 범위 읽기로).
    TooLarge,
}

pub fn size_class(size: u64) -> SizeClass {
    if size <= EDITABLE_MAX {
        SizeClass::Editable
    } else if size <= READ_ONLY_MAX {
        SizeClass::ReadOnly
    } else {
        SizeClass::TooLarge
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum Bom {
    Utf8,
    Utf16Le,
    Utf16Be,
}

impl Bom {
    pub fn detect(head: &[u8]) -> Option<Self> {
        if head.starts_with(UTF8_BOM) {
            Some(Bom::Utf8)
        } else if head.starts_with(&[0xFF, 0xFE]) {
            Some(Bom::Utf16Le)
        } else if head.starts_with(&[0xFE, 0xFF]) {
            Some(Bom::Utf16Be)
        } else {
            None
        }
    }

    pub fn byte_len(self) -> usize {
        match self {
            Bom::Utf8 => 3,
            Bom::Utf16Le | Bom::Utf16Be => 2,
        }
    }

    /// WHATWG 인코딩 이름.
    fn encoding(self) -> &'static str {
        match self {
            Bom::Utf8 => "UTF-8",
            Bom::Utf16Le => "UTF-16LE",
            Bom::Utf16Be => "UTF-16BE",
        }
    }
}

/// 읽은 범위의 줄바꿈.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum LineEnding {
    Lf,
    Crlf,
    Cr,
    /// 둘 이상이 섞였다.
    Mixed,
    /// 줄바꿈이 없다(한 줄).
    None,
}

/// 읽은 내용의 판정.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct Judgment {
    /// 파일 앞 8 KiB 에 NUL 이 있다(UTF-16 BOM 이 있으면 텍스트로 본다).
    pub binary: bool,
    /// 읽은 범위의 WHATWG 인코딩 이름(`UTF-8` · `UTF-16LE` · `UTF-16BE`). UTF-8 로 읽히지 않는 텍스트는 `null` 이다 —
    /// 짐작은 M1-23(`encoding_rs` + `chardetng`)이 한다. 바이너리면 `null`.
    pub encoding: Option<String>,
    /// 파일 앞의 BOM.
    pub bom: Option<Bom>,
    pub line_ending: LineEnding,
    /// 읽은 범위에 1만 자를 넘는 줄이 있다.
    pub long_lines: bool,
    /// 파일 전체 크기의 등급.
    pub size_class: SizeClass,
    /// 에디터로 편집 · 저장할 수 있다: 텍스트, UTF-8(BOM 유무 무관, 결정 M1-D12), 5 MiB 이하.
    pub editable: bool,
    /// 구문 강조 · 줄바꿈을 켠다: 텍스트, 5 MiB 이하, 1만 자 넘는 줄 없음(결정 M1-D9).
    pub highlight: bool,
}

/// `head` 는 파일 앞부분(최대 [`SNIFF_BYTES`]), `body` 는 읽은 범위(파일 앞에서 읽었으면 BOM 을 뺀 것), `size` 는 파일 크기.
/// `truncated` 면 `body` 끝이 파일 끝이 아니라 글자 중간에서 끊겼을 수 있다.
pub fn judge(head: &[u8], body: &[u8], size: u64, truncated: bool) -> Judgment {
    let bom = Bom::detect(head);
    let utf16 = matches!(bom, Some(Bom::Utf16Le | Bom::Utf16Be));
    let sniff = &head[..head.len().min(SNIFF_BYTES)];
    let binary = !utf16 && sniff.contains(&0);
    let encoding = if binary {
        None
    } else if let Some(bom) = bom.filter(|_| utf16) {
        Some(bom.encoding().to_owned())
    } else if is_utf8(body, truncated) {
        Some("UTF-8".to_owned())
    } else {
        None
    };
    let scan = Scan::of(body);
    let size_class = size_class(size);
    let text = !binary;
    Judgment {
        editable: text && encoding.as_deref() == Some("UTF-8") && size_class == SizeClass::Editable,
        highlight: text && size_class == SizeClass::Editable && !scan.long_lines,
        binary,
        encoding,
        bom,
        line_ending: scan.line_ending(),
        long_lines: scan.long_lines,
        size_class,
    }
}

/// 끊긴 범위면 끝의 미완성 글자는 받는다.
fn is_utf8(bytes: &[u8], truncated: bool) -> bool {
    match std::str::from_utf8(bytes) {
        Ok(_) => true,
        Err(e) => truncated && e.error_len().is_none(),
    }
}

/// 줄바꿈 종류와 긴 줄을 한 번에 센다.
struct Scan {
    lf: bool,
    crlf: bool,
    cr: bool,
    long_lines: bool,
}

impl Scan {
    fn of(bytes: &[u8]) -> Self {
        let mut scan = Scan {
            lf: false,
            crlf: false,
            cr: false,
            long_lines: false,
        };
        // UTF-8 글자 수 = 이어지는 바이트(10xxxxxx)가 아닌 바이트 수. UTF-8 이 아니어도 바이트 수로 근사된다.
        let mut chars = 0usize;
        let mut i = 0;
        while i < bytes.len() {
            match bytes[i] {
                b'\n' => {
                    scan.lf = true;
                    chars = 0;
                }
                b'\r' => {
                    if bytes.get(i + 1) == Some(&b'\n') {
                        scan.crlf = true;
                        i += 1;
                    } else {
                        scan.cr = true;
                    }
                    chars = 0;
                }
                b if b & 0xC0 != 0x80 => {
                    chars += 1;
                    if chars > LONG_LINE_CHARS {
                        scan.long_lines = true;
                    }
                }
                _ => {}
            }
            i += 1;
        }
        scan
    }

    fn line_ending(&self) -> LineEnding {
        match (self.lf, self.crlf, self.cr) {
            (false, false, false) => LineEnding::None,
            (true, false, false) => LineEnding::Lf,
            (false, true, false) => LineEnding::Crlf,
            (false, false, true) => LineEnding::Cr,
            _ => LineEnding::Mixed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nul_in_the_first_8_kib_means_binary() {
        let mut bytes = vec![b'a'; SNIFF_BYTES];
        bytes[SNIFF_BYTES - 1] = 0;
        let j = judge(&bytes, &bytes, bytes.len() as u64, false);
        assert!(j.binary);
        assert_eq!(j.encoding, None);
        assert!(!j.editable && !j.highlight);

        // NUL 이 8 KiB 뒤에만 있으면 텍스트다.
        let mut later = vec![b'a'; SNIFF_BYTES + 10];
        later[SNIFF_BYTES] = 0;
        let j = judge(&later[..SNIFF_BYTES], &later, later.len() as u64, false);
        assert!(!j.binary);
    }

    #[test]
    fn bom_and_line_endings_are_reported() {
        let file = b"\xEF\xBB\xBFfn main() {}\r\nlet x = 1;\r\n";
        let j = judge(file, &file[3..], file.len() as u64, false);
        assert_eq!(j.bom, Some(Bom::Utf8));
        assert_eq!(j.encoding.as_deref(), Some("UTF-8"));
        assert_eq!(j.line_ending, LineEnding::Crlf);
        assert!(j.editable && j.highlight);

        assert_eq!(
            judge(b"a\nb", b"a\nb", 3, false).line_ending,
            LineEnding::Lf
        );
        assert_eq!(
            judge(b"a\rb", b"a\rb", 3, false).line_ending,
            LineEnding::Cr
        );
        assert_eq!(
            judge(b"a\r\nb\n", b"a\r\nb\n", 5, false).line_ending,
            LineEnding::Mixed
        );
        assert_eq!(judge(b"ab", b"ab", 2, false).line_ending, LineEnding::None);
        assert_eq!(judge(b"ab", b"ab", 2, false).bom, None);
    }

    #[test]
    fn utf16_with_a_bom_is_text_even_with_nuls() {
        let file = b"\xFF\xFEa\0b\0";
        let j = judge(file, &file[2..], 6, false);
        assert!(!j.binary);
        assert_eq!(j.bom, Some(Bom::Utf16Le));
        assert_eq!(j.encoding.as_deref(), Some("UTF-16LE"));
        assert!(!j.editable, "UTF-8 만 편집한다(결정 M1-D12)");
    }

    #[test]
    fn invalid_utf8_has_no_encoding_and_is_not_editable() {
        // EUC-KR 「한글」.
        let file = b"\xC7\xD1\xB1\xDB\n";
        let j = judge(file, file, 5, false);
        assert!(!j.binary);
        assert_eq!(j.encoding, None);
        assert!(!j.editable);
        assert!(j.highlight);
        // 범위 읽기가 글자 중간에서 끊겼으면 끝의 미완성 글자는 받는다.
        let cut = "가나".as_bytes();
        assert_eq!(
            judge(cut, &cut[..4], 100, true).encoding.as_deref(),
            Some("UTF-8")
        );
        assert_eq!(judge(cut, &cut[..4], 4, false).encoding, None);
    }

    #[test]
    fn size_classes_follow_the_m1_d9_thresholds() {
        assert_eq!(size_class(0), SizeClass::Editable);
        assert_eq!(size_class(EDITABLE_MAX), SizeClass::Editable);
        assert_eq!(size_class(EDITABLE_MAX + 1), SizeClass::ReadOnly);
        assert_eq!(size_class(READ_ONLY_MAX), SizeClass::ReadOnly);
        assert_eq!(size_class(READ_ONLY_MAX + 1), SizeClass::TooLarge);
        assert_eq!(EDITABLE_MAX, 5 * 1024 * 1024);
        assert_eq!(READ_ONLY_MAX, 50 * 1024 * 1024);

        let j = judge(b"a", b"a", EDITABLE_MAX + 1, true);
        assert_eq!(j.size_class, SizeClass::ReadOnly);
        assert!(!j.editable && !j.highlight);
    }

    #[test]
    fn a_line_over_10000_chars_turns_highlighting_off() {
        // 글자 수로 센다: 1만 자 한글 줄은 3만 바이트지만 1만 자다.
        let exactly = "가".repeat(LONG_LINE_CHARS);
        let j = judge(exactly.as_bytes(), exactly.as_bytes(), 30_000, false);
        assert!(!j.long_lines && j.highlight);

        let over = format!("short\n{}\nshort", "a".repeat(LONG_LINE_CHARS + 1));
        let j = judge(over.as_bytes(), over.as_bytes(), over.len() as u64, false);
        assert!(j.long_lines);
        assert!(!j.highlight);
        assert!(j.editable, "긴 줄은 강조만 끈다");
    }
}
