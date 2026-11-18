//! `.env` 파일의 문법(17-m6-plan 메인 판정 M6-P5). 결과를 예측할 수 있게 작게 둔다 — 셸이 아니다.
//!
//! - 한 줄에 `KEY=VALUE` 하나. `KEY` 는 환경 변수 이름(`[A-Za-z_][A-Za-z0-9_]*`). `=` 앞뒤의 공백 · 탭은 버린다.
//! - 줄 앞의 `export ` 는 버린다(셸에서 `source` 하는 파일과 같이 쓰게). `export=1` 은 이름이 `export` 인 변수다.
//! - 빈 줄과 `#` 로 시작하는 줄은 주석이다. 따옴표 없는 값에서는 **앞에 공백 · 탭이 있는** `#` 부터 줄 끝까지가 주석이다 —
//!   `COLOR=#fff` 는 `#fff` 이고, `A=1 # 설명` 은 `1` 이다. 따옴표 없는 값의 끝 공백은 버리고 가운데 공백은 둔다.
//! - 홑따옴표 `'…'` 는 글자 그대로다. 겹따옴표 `"…"` 는 `\n`(두 글자)만 줄바꿈으로 풀고 다른 `\` 는 글자 그대로다 — `\"` 도
//!   풀지 않으므로 겹따옴표 값 안에 `"` 를 넣으려면 홑따옴표로 감싼다. 따옴표 값은 여러 줄에 걸칠 수 있다(PEM 키). 닫는 따옴표
//!   뒤에는 공백과 주석만 올 수 있다.
//! - **변수 확장(`${OTHER}` · `$OTHER`)은 하지 않는다** — 글자 그대로다. 셸 · 다른 도구와 결과가 다를 때 원인을 찾기 어렵다.
//!   `PATH=./node_modules/.bin:$PATH` 도 글자 그대로의 PATH 가 되어 런타임 환경의 PATH 를 통째로 바꾼다(그러면 거의 모든 명령을
//!   찾지 못한다) — PATH 앞에 붙이려면 앞뒤를 모두 적는다.
//! - 같은 이름이 두 번 나오면 뒤의 것이다. 파일 앞의 BOM 은 받는다.
//! - 줄 끝은 `\n` 과 `\r\n` 이다. 따옴표 값 안의 `\r\n` 도 `\n` 하나다(Windows 에서 저장한 PEM 이 다른 도구와 같은 값). 뒤에
//!   `\n` 이 없는 `\r`(옛 Mac 줄 끝)은 어디에 있든 오류다 — 줄 끝으로도 글자로도 보지 않는다(글자로 보면 `A=1\rB=2` 가 조용히
//!   변수 하나가 된다).
//!
//! 이 밖의 꼴(이름이 아닌 키, `=` 없는 줄, 닫히지 않은 따옴표, NUL, 외톨이 `\r`)은 오류다 — 조용히 건너뛰면 변수 하나가 빠진 채 돈다. 오류에는
//! 줄 번호와 까닭만 싣고 **그 줄의 글은 싣지 않는다**(값이 비밀일 수 있다 — M6-03 ③).

use std::fmt;

/// 파일 하나를 읽은 결과: 나온 순서대로 (이름, 값). 같은 이름이 둘이면 둘 다 들어 있고 뒤의 것이 이긴다([`super`] 가 합친다).
pub fn parse(text: &str) -> Result<Vec<(String, String)>, SyntaxError> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    if let Some(at) = lone_cr(text) {
        let line = text[..at].matches('\n').count() + 1;
        return Err(SyntaxError {
            line,
            problem: Problem::LoneCr,
        });
    }
    let mut cursor = Cursor {
        chars: text.chars().collect(),
        at: 0,
        line: 1,
    };
    let mut out = Vec::new();
    loop {
        cursor.skip_blank();
        let Some(c) = cursor.peek() else {
            return Ok(out);
        };
        if is_line_end(&cursor) {
            cursor.eat_line_end();
            continue;
        }
        if c == '#' {
            cursor.skip_to_line_end();
            continue;
        }
        let line = cursor.line;
        let err = |problem| SyntaxError { line, problem };
        if cursor.starts_with("export") && matches!(cursor.peek_at(6), Some(' ' | '\t')) {
            cursor.at += 6;
            cursor.skip_blank();
        }
        let name = cursor.take_while(|c| c.is_ascii_alphanumeric() || c == '_');
        if !crate::config::valid_env_name(&name) {
            return Err(err(Problem::InvalidName));
        }
        cursor.skip_blank();
        if cursor.peek() != Some('=') {
            return Err(err(if cursor.peek().is_none() || is_line_end(&cursor) {
                Problem::MissingEquals
            } else {
                Problem::InvalidName
            }));
        }
        cursor.at += 1;
        cursor.skip_blank();
        let value = match cursor.peek() {
            Some(quote @ ('\'' | '"')) => {
                cursor.at += 1;
                let value = cursor
                    .quoted(quote == '"')
                    .ok_or(err(Problem::UnclosedQuote))?;
                cursor.skip_blank();
                match cursor.peek() {
                    None => {}
                    Some('#') => cursor.skip_to_line_end(),
                    Some(_) if is_line_end(&cursor) => {}
                    Some(_) => return Err(err(Problem::TextAfterQuote)),
                }
                value
            }
            _ => cursor.unquoted(),
        };
        if value.contains('\0') {
            return Err(err(Problem::Nul));
        }
        out.push((name, value));
    }
}

/// 문법 오류 — 몇째 줄(그 변수가 시작한 줄)과 까닭. 그 줄의 글은 싣지 않는다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxError {
    pub line: usize,
    pub problem: Problem,
}

/// 문법 오류의 까닭.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Problem {
    /// 키가 환경 변수 이름이 아니다.
    InvalidName,
    /// `=` 가 없다.
    MissingEquals,
    /// 따옴표가 파일 끝까지 닫히지 않았다.
    UnclosedQuote,
    /// 닫는 따옴표 뒤에 주석이 아닌 글이 있다.
    TextAfterQuote,
    /// 값에 NUL 이 있다(환경 변수에 넣을 수 없다).
    Nul,
    /// 뒤에 `\n` 이 없는 `\r` 이 있다.
    LoneCr,
}

impl fmt::Display for SyntaxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let why = match self.problem {
            Problem::InvalidName => {
                "the key is not an environment variable name ([A-Za-z_][A-Za-z0-9_]*)"
            }
            Problem::MissingEquals => "expected `KEY=VALUE`",
            Problem::UnclosedQuote => "a quoted value is not closed",
            Problem::TextAfterQuote => "only a `#` comment may follow a closing quote",
            Problem::Nul => "the value contains a NUL character",
            Problem::LoneCr => {
                "a carriage return without a line feed (save the file with LF or CRLF line endings)"
            }
        };
        write!(f, "line {}: {why}", self.line)
    }
}

struct Cursor {
    chars: Vec<char>,
    at: usize,
    line: usize,
}

impl Cursor {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.at).copied()
    }

    fn peek_at(&self, ahead: usize) -> Option<char> {
        self.chars.get(self.at + ahead).copied()
    }

    fn starts_with(&self, word: &str) -> bool {
        word.chars()
            .enumerate()
            .all(|(i, c)| self.peek_at(i) == Some(c))
    }

    /// 공백 · 탭(줄바꿈은 아니다).
    fn skip_blank(&mut self) {
        while matches!(self.peek(), Some(' ' | '\t')) {
            self.at += 1;
        }
    }

    fn take_while(&mut self, keep: impl Fn(char) -> bool) -> String {
        let mut out = String::new();
        while let Some(c) = self.peek().filter(|c| keep(*c)) {
            out.push(c);
            self.at += 1;
        }
        out
    }

    fn eat_line_end(&mut self) {
        if self.peek() == Some('\r') {
            self.at += 1;
        }
        if self.peek() == Some('\n') {
            self.at += 1;
            self.line += 1;
        }
    }

    /// 줄 끝(`\n` · `\r\n`)까지 건너뛰고 줄바꿈도 먹는다.
    fn skip_to_line_end(&mut self) {
        while self.peek().is_some() && !is_line_end(self) {
            self.at += 1;
        }
        self.eat_line_end();
    }

    /// 여는 따옴표 다음부터 닫는 따옴표까지(닫는 것도 먹는다). 닫히지 않으면 `None`.
    fn quoted(&mut self, double: bool) -> Option<String> {
        let close = if double { '"' } else { '\'' };
        let mut out = String::new();
        loop {
            let c = self.peek()?;
            self.at += 1;
            match c {
                c if c == close => return Some(out),
                '\\' if double && self.peek() == Some('n') => {
                    self.at += 1;
                    out.push('\n');
                }
                // 외톨이 `\r` 은 [`parse`] 가 미리 거절했다 — 여기의 `\r` 은 `\r\n` 의 앞이다.
                '\r' => {}
                '\n' => {
                    self.line += 1;
                    out.push('\n');
                }
                c => out.push(c),
            }
        }
    }

    /// 따옴표 없는 값: 줄 끝 또는 앞에 공백이 있는 `#` 까지, 끝 공백을 버린다. 줄바꿈도 먹는다.
    fn unquoted(&mut self) -> String {
        let mut out = String::new();
        // `=` 뒤의 공백을 이미 건넜으므로 첫 글자 앞은 공백이었을 수 있다 — 그 `#` 도 주석이다(`A= #c` 는 빈 값).
        let mut after_blank = self.at > 0 && matches!(self.chars[self.at - 1], ' ' | '\t');
        while let Some(c) = self.peek() {
            if is_line_end(self) || (c == '#' && after_blank) {
                break;
            }
            out.push(c);
            after_blank = matches!(c, ' ' | '\t');
            self.at += 1;
        }
        self.skip_to_line_end();
        out.truncate(out.trim_end_matches([' ', '\t']).len());
        out
    }
}

/// 지금 자리가 줄 끝(`\n` 또는 `\r\n`)인가. 외톨이 `\r` 은 [`parse`] 가 미리 거절했다.
fn is_line_end(cursor: &Cursor) -> bool {
    match cursor.peek() {
        Some('\n') => true,
        Some('\r') => cursor.peek_at(1) == Some('\n'),
        _ => false,
    }
}

/// 뒤에 `\n` 이 없는 첫 `\r` 의 바이트 위치.
fn lone_cr(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    bytes
        .iter()
        .enumerate()
        .find(|(i, b)| **b == b'\r' && bytes.get(i + 1) != Some(&b'\n'))
        .map(|(i, _)| i)
}
