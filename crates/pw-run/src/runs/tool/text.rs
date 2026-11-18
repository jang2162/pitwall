//! `run_logs` 가 내는 글: 출력 한 줄의 바이트에서 터미널 제어(ANSI 이스케이프 · 제어 문자)를 걷는다(17-m6-plan M6-12 ③).
//!
//! 걷는 것: CSI(`ESC [` … 끝 글자, 8비트 `U+009B` 포함 — 색 · 커서 이동 · 지우기), OSC(`ESC ]` … BEL 또는 `ESC \` — 창 제목 ·
//! 하이퍼링크), DCS · SOS · PM · APC(`ESC P` · `ESC X` · `ESC ^` · `ESC _` … `ESC \`), 그 밖의 두 · 세 바이트 이스케이프(`ESC ( B`
//! 등), 탭을 뺀 C0 · C1 제어 문자. `\r` 은 터미널처럼 줄 앞으로 돌아간다 — 마지막 `\r` 뒤의 글이 그 줄이다(진행 막대가 쌓이지
//! 않게). 줄 끝의 `\r`(`\r\n`)은 그냥 걷는다. UTF-8 이 아닌 바이트는 U+FFFD 로 바꾼다.
//!
//! 줄 하나 안에서만 본다 — 줄을 넘는 OSC(드물다)는 그 줄 끝에서 끝난 것으로 본다.

/// 줄 하나(끝의 `\n` 은 뺀 바이트)를 보이는 글로.
pub fn clean_line(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\u{1b}' => match chars.next() {
                Some('[') => skip_csi(&mut chars),
                Some(']' | 'P' | 'X' | '^' | '_') => skip_string(&mut chars),
                // `ESC` 뒤 중간 글자(0x20-0x2F)들과 끝 글자 하나(`ESC ( B` · `ESC 7` · `ESC =`).
                Some(c) if ('\u{20}'..='\u{2f}').contains(&c) => {
                    while chars
                        .next_if(|c| ('\u{20}'..='\u{2f}').contains(c))
                        .is_some()
                    {}
                    chars.next();
                }
                Some(_) | None => {}
            },
            '\u{9b}' => skip_csi(&mut chars),
            '\u{9d}' | '\u{90}' | '\u{98}' | '\u{9e}' | '\u{9f}' => skip_string(&mut chars),
            '\r' => {
                // 뒤에 글이 더 있으면 줄 앞으로 돌아가 덮는다. 줄 끝의 `\r` 이면 아무것도 지우지 않는다.
                if chars.peek().is_some() {
                    out.clear();
                }
            }
            '\t' => out.push(c),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

/// `ESC [` 뒤: 매개변수 · 중간 글자 뒤의 끝 글자(0x40-0x7E)까지.
fn skip_csi(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    for c in chars.by_ref() {
        if ('\u{40}'..='\u{7e}').contains(&c) {
            return;
        }
    }
}

/// 문자열 이스케이프(OSC · DCS 등): BEL · `ESC \` · 8비트 ST(`U+009C`)까지.
fn skip_string(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    while let Some(c) = chars.next() {
        match c {
            '\u{7}' | '\u{9c}' => return,
            '\u{1b}' => {
                chars.next_if_eq(&'\\');
                return;
            }
            _ => {}
        }
    }
}
