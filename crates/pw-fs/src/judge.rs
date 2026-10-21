//! 읽은 바이트의 판정과 크기 등급. 기준값은 [`crate::limits`](결정 M1-D9)이고, UTF-8 이 아닌 텍스트는 인코딩을 짐작한다(결정
//! M1-D12, `chardetng`).

use chardetng::{EncodingDetector, Iso2022JpDetection, Utf8Detection};
use serde::{Deserialize, Serialize};

use crate::limits::{EDITABLE_MAX, LONG_LINE_CHARS, READ_ONLY_MAX, SNIFF_BYTES};

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
    /// 읽은 범위의 WHATWG 인코딩 이름(`UTF-8` · `UTF-16LE` · `UTF-16BE` · 짐작한 것). UTF-16 은 BOM 으로, UTF-8 로 읽히지 않는
    /// 텍스트는 `chardetng` 로 짐작한다(예: `EUC-KR` · `Shift_JIS` · `windows-1252`, 결정 M1-D12). 바이너리면 `null`. 거의 UTF-8
    /// 이면(`lossy`) `UTF-8` 이다.
    pub encoding: Option<String>,
    /// 인코딩이 `UTF-8` 이지만 유효하지 않은 바이트가 있어(쓰는 중인 로그 · 끝이 잘린 파일 등) 그 바이트를 U+FFFD 로 바꿔 풀었다.
    /// 편집하지 않는다 — 저장하면 그 바이트가 U+FFFD 로 바뀐다.
    pub lossy: bool,
    /// 파일 앞의 BOM.
    pub bom: Option<Bom>,
    pub line_ending: LineEnding,
    /// 읽은 범위에 1만 자를 넘는 줄이 있다.
    pub long_lines: bool,
    /// 파일 전체 크기의 등급.
    pub size_class: SizeClass,
    /// 에디터로 편집 · 저장할 수 있다: 텍스트, 유효한 UTF-8(BOM 유무 무관, 결정 M1-D12, `lossy` 아님), 5 MiB 이하.
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
    // 편집은 라벨이 아니라 이 검사로 정한다 — `lossy` 도 라벨은 `UTF-8` 이다.
    let utf8 = !binary && !utf16 && is_utf8(body, truncated);
    let lossy = !binary && !utf16 && !utf8 && mostly_utf8(body);
    let encoding = if binary {
        None
    } else if let Some(bom) = bom.filter(|_| utf16) {
        Some(bom.encoding().to_owned())
    } else if utf8 || lossy {
        Some("UTF-8".to_owned())
    } else {
        Some(guess_encoding(body, truncated).name().to_owned())
    };
    let scan = Scan::of(body);
    let size_class = size_class(size);
    let text = !binary;
    Judgment {
        editable: utf8 && size_class == SizeClass::Editable,
        highlight: text && size_class == SizeClass::Editable && !scan.long_lines,
        binary,
        encoding,
        lossy,
        bom,
        line_ending: scan.line_ending(),
        long_lines: scan.long_lines,
        size_class,
    }
}

/// [`mostly_utf8`] 의 기준: 유효한 멀티바이트 글자 수가 유효하지 않은 시퀀스 수의 이 배수 이상이다. UTF-8 이 아닌 인코딩의 텍스트를
/// UTF-8 로 읽으면 우연히 맞는 시퀀스가 생기지만 그 비율은 1 에 못 미친다(한국어 EUC-KR · CP949 약 0.2~0.26, Shift_JIS · GBK 약
/// 0.2, Big5 약 0.32, windows-1252 · windows-1251 · KOI8-R 0~0.08). 16 은 그 최댓값의 약 50배다.
const MOSTLY_UTF8_RATIO: usize = 16;

/// UTF-8 로 쓰인 텍스트에 깨진 바이트가 조금 섞였다. 이런 파일을 짐작에 맡기면 깨진 바이트 하나 때문에 파일 전체가 다른 인코딩
/// (대개 windows-1252)으로 깨져 보인다.
fn mostly_utf8(bytes: &[u8]) -> bool {
    let (mut valid, mut invalid) = (0usize, 0usize);
    for chunk in bytes.utf8_chunks() {
        // 멀티바이트 글자의 첫 바이트(11xxxxxx)를 센다.
        valid += chunk.valid().bytes().filter(|&b| b >= 0xC0).count();
        if !chunk.invalid().is_empty() {
            invalid += 1;
        }
    }
    invalid > 0 && valid >= MOSTLY_UTF8_RATIO * invalid
}

/// 짐작에 보는 앞부분. 짐작은 앞부분의 바이트 분포로 충분하고, 50 MiB 를 다 먹이면 열기가 그만큼 늦다.
const GUESS_BYTES: usize = 64 * 1024;

/// UTF-8 로 읽히지 않는 텍스트의 인코딩. UTF-8 은 답으로 받지 않는다(이미 아니다). ISO-2022-JP 는 7비트라 UTF-8 로 읽히므로 여기
/// 오지 않는다.
fn guess_encoding(body: &[u8], truncated: bool) -> &'static encoding_rs::Encoding {
    let fed = &body[..body.len().min(GUESS_BYTES)];
    let mut detector = EncodingDetector::new(Iso2022JpDetection::Deny);
    detector.feed(fed, !truncated && fed.len() == body.len());
    detector.guess(None, Utf8Detection::Deny)
}

/// 판정한 인코딩으로 텍스트를 푼다. `bytes` 는 BOM 을 뗀 것이다. 모르는 이름 · 바이너리(`None`)면 `None`. 풀 수 없는 바이트는
/// U+FFFD 가 된다(끊긴 범위의 끝 글자 포함).
pub fn decode(encoding: Option<&str>, bytes: &[u8]) -> Option<String> {
    let encoding = encoding_rs::Encoding::for_label(encoding?.as_bytes())?;
    Some(encoding.decode_without_bom_handling(bytes).0.into_owned())
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
    fn invalid_utf8_is_not_utf8_and_is_not_editable() {
        // EUC-KR 「한글」.
        let file = b"\xC7\xD1\xB1\xDB\n";
        let j = judge(file, file, 5, false);
        assert!(!j.binary);
        assert_ne!(j.encoding.as_deref(), Some("UTF-8"));
        assert!(!j.editable);
        assert!(j.highlight);
        // 범위 읽기가 글자 중간에서 끊겼으면 끝의 미완성 글자는 받는다.
        let cut = "가나".as_bytes();
        assert_eq!(
            judge(cut, &cut[..4], 100, true).encoding.as_deref(),
            Some("UTF-8")
        );
        assert_ne!(
            judge(cut, &cut[..4], 4, false).encoding.as_deref(),
            Some("UTF-8")
        );
    }

    #[test]
    fn euc_kr_text_is_guessed_and_decodes_to_hangul() {
        let hangul =
            "// 한글 주석입니다. 파일을 읽기 전용으로 엽니다.\nlet 이름 = \"가나다라마바사\";\n";
        let (bytes, _, lossy) = encoding_rs::EUC_KR.encode(hangul);
        assert!(!lossy);
        let j = judge(&bytes, &bytes, bytes.len() as u64, false);
        assert!(!j.binary);
        assert_eq!(j.encoding.as_deref(), Some("EUC-KR"));
        assert!(!j.editable, "UTF-8 만 편집한다(결정 M1-D12)");
        assert_eq!(
            decode(j.encoding.as_deref(), &bytes).as_deref(),
            Some(hangul)
        );

        // Shift_JIS 도 같은 길이다.
        let japanese = "日本語のテキストファイルです。読み取り専用で開きます。\n";
        let (bytes, _, _) = encoding_rs::SHIFT_JIS.encode(japanese);
        let j = judge(&bytes, &bytes, bytes.len() as u64, false);
        assert_eq!(j.encoding.as_deref(), Some("Shift_JIS"));
        assert_eq!(
            decode(j.encoding.as_deref(), &bytes).as_deref(),
            Some(japanese)
        );
    }

    #[test]
    fn utf16_with_a_bom_decodes_and_binary_does_not() {
        let file = b"\xFF\xFEa\0\x5C\xD5";
        let j = judge(file, &file[2..], 6, false);
        assert_eq!(
            decode(j.encoding.as_deref(), &file[2..]).as_deref(),
            Some("a한")
        );
        assert_eq!(decode(None, b"\0\x01"), None);
    }

    #[test]
    fn utf8_with_a_few_broken_bytes_reads_as_utf8_but_is_not_editable() {
        // 한글 로그 50줄 + 끝에 깨진 바이트 하나(쓰는 중 · 끝이 잘린 파일).
        let log = "2026-10-21 로그 한 줄입니다\n".repeat(50);
        let mut bytes = log.clone().into_bytes();
        bytes.push(0xFF);
        let j = judge(&bytes, &bytes, bytes.len() as u64, false);
        assert_eq!(j.encoding.as_deref(), Some("UTF-8"));
        assert!(j.lossy);
        assert!(!j.editable, "저장하면 깨진 바이트가 U+FFFD 로 바뀐다");
        assert!(j.highlight);
        assert_eq!(
            decode(j.encoding.as_deref(), &bytes).as_deref(),
            Some(format!("{log}\u{FFFD}").as_str())
        );

        // 앞 64 KiB 가 ASCII 이고 한글 · 깨진 바이트가 뒤에 있어도 같다(짐작은 앞부분만 본다).
        let mut late = vec![b'a'; GUESS_BYTES + 10];
        late.push(b'\n');
        late.extend_from_slice(log.as_bytes());
        late.push(0xC3);
        let j = judge(&late[..SNIFF_BYTES], &late, late.len() as u64, false);
        assert_eq!(j.encoding.as_deref(), Some("UTF-8"));
        assert!(j.lossy && !j.editable);

        // 깨진 것이 많으면 거의 UTF-8 이 아니다 — 짐작으로 간다.
        let mut noisy = "한글".repeat(8).into_bytes();
        noisy.extend_from_slice(&[0xFF, 0x20, 0xFF]);
        assert!(!judge(&noisy, &noisy, noisy.len() as u64, false).lossy);
        // 유효한 멀티바이트 글자가 하나도 없으면(latin-1 의 「café」) 짐작한다.
        let latin = b"a README with one caf\xE9\n";
        let j = judge(latin, latin, latin.len() as u64, false);
        assert!(!j.lossy);
        assert_eq!(j.encoding.as_deref(), Some("windows-1252"));
        // 유효한 UTF-8 은 lossy 가 아니다.
        let j = judge(log.as_bytes(), log.as_bytes(), log.len() as u64, false);
        assert!(!j.lossy && j.editable);
    }

    #[test]
    fn legacy_encodings_are_not_mistaken_for_mostly_utf8() {
        let samples: [(&encoding_rs::Encoding, &str); 5] = [
            (
                encoding_rs::EUC_KR,
                "한글 주석입니다. 파일을 읽기 전용으로 엽니다. 가나다라마바사 ",
            ),
            (
                encoding_rs::SHIFT_JIS,
                "日本語のテキストファイルです。読み取り専用で開きます。",
            ),
            (encoding_rs::GBK, "这是一个中文文本文件，以只读方式打开。"),
            (
                encoding_rs::BIG5,
                "這是一個繁體中文文字檔案，以唯讀方式開啟。",
            ),
            (
                encoding_rs::WINDOWS_1252,
                "Café crème brûlée — naïve façade «quoted» © ® ",
            ),
        ];
        for (encoding, text) in samples {
            let text = text.repeat(30);
            let (bytes, _, _) = encoding.encode(&text);
            let j = judge(&bytes, &bytes, bytes.len() as u64, false);
            assert!(!j.lossy, "{}", encoding.name());
            assert_eq!(j.encoding.as_deref(), Some(encoding.name()));
        }
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
