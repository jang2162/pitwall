//! 진단 문장 — `SHOW [COUNT(*)] WARNINGS|ERRORS [LIMIT …]` · `GET [CURRENT|STACKED] DIAGNOSTICS …` 는 준비 시도 없이 텍스트로 보낸다.
//!
//! 이 문장들은 앞 문장이 남긴 진단 영역을 읽는다. MySQL 8.4 은 이것들을 준비 문장으로 받지 않아(1295) 준비 시도가 실패하면서 진단
//! 영역을 드라이버 자신의 1295 로 덮는다 — 사람의 경고 · 오류 대신 「This command is not supported in the prepared statement
//! protocol」 이 보인다. 그래서 [`is_diagnostic`] 이 참이면 준비하지 않는다.
//!
//! 텍스트로 보내도 문장 하나여야 한다(`conn` 머리 주석의 「문장 하나」). 그래서 글이 **정확히** 그 꼴일 때만 참이다: 낱말 · 정수 ·
//! `@사용자 변수` · `( ) * , =` 와 끝의 `;` 하나 말고는 아무 글자도 받지 않는다(따옴표 · 역따옴표 · 그 밖의 기호가 하나라도 있으면
//! 거짓 — 지금 경로로 간다). 주석은 공백으로 보되 실행 주석(`/*!` · `/*M!`)과 옵티마이저 힌트(`/*+`)는 받지 않는다. 그 밖은
//! 서버가 문장 하나로 읽는 꼴이 아니면 지금 경로가 1064 로 거절한다.

/// 낱말 하나.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    /// 낱말(대문자로) 또는 정수.
    Word(String),
    /// `@이름`.
    Variable,
    Symbol(char),
}

/// 받을 수 없는 글자가 있으면 `None`.
fn tokens(sql: &str) -> Option<Vec<Token>> {
    let chars: Vec<char> = sql.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        if c.is_whitespace() {
            i += 1;
        } else if c == '#'
            || (c == '-' && next == Some('-') && chars.get(i + 2).is_none_or(|c| c.is_whitespace()))
        {
            // 줄 주석: 줄 끝까지.
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
        } else if c == '/' && next == Some('*') {
            // 실행 주석 · 힌트는 코드다.
            if matches!(chars.get(i + 2), Some('!' | '+' | 'M')) {
                return None;
            }
            let end = (i + 2..chars.len().saturating_sub(1))
                .find(|&j| chars[j] == '*' && chars[j + 1] == '/')?;
            i = end + 2;
        } else if c.is_ascii_alphanumeric() || c == '_' {
            let start = i;
            while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            out.push(Token::Word(
                chars[start..i]
                    .iter()
                    .collect::<String>()
                    .to_ascii_uppercase(),
            ));
        } else if c == '@' && next.is_some_and(|n| n.is_ascii_alphanumeric() || n == '_') {
            i += 1;
            while i < chars.len()
                && (chars[i].is_ascii_alphanumeric() || matches!(chars[i], '_' | '$'))
            {
                i += 1;
            }
            out.push(Token::Variable);
        } else if matches!(c, '(' | ')' | '*' | ',' | '=' | ';') {
            out.push(Token::Symbol(c));
            i += 1;
        } else {
            return None;
        }
    }
    Some(out)
}

/// 앞에서부터 꼴을 맞춰 나간다.
struct Cursor<'a> {
    tokens: &'a [Token],
    at: usize,
}

impl Cursor<'_> {
    fn word(&mut self, word: &str) -> bool {
        let hit = matches!(self.tokens.get(self.at), Some(Token::Word(w)) if w == word);
        self.at += usize::from(hit);
        hit
    }

    fn symbol(&mut self, symbol: char) -> bool {
        let hit = self.tokens.get(self.at) == Some(&Token::Symbol(symbol));
        self.at += usize::from(hit);
        hit
    }

    fn integer(&mut self) -> bool {
        let hit = matches!(self.tokens.get(self.at), Some(Token::Word(w)) if w.bytes().all(|b| b.is_ascii_digit()));
        self.at += usize::from(hit);
        hit
    }

    fn name(&mut self) -> bool {
        let hit = matches!(self.tokens.get(self.at), Some(Token::Word(w)) if w.starts_with(|c: char| c.is_ascii_alphabetic()));
        self.at += usize::from(hit);
        hit
    }

    fn variable(&mut self) -> bool {
        let hit = self.tokens.get(self.at) == Some(&Token::Variable);
        self.at += usize::from(hit);
        hit
    }

    /// 끝의 `;` 하나까지 받고 더 남은 것이 없는가.
    fn end(&mut self) -> bool {
        self.symbol(';');
        self.at == self.tokens.len()
    }

    /// `SHOW [COUNT(*)] WARNINGS|ERRORS [LIMIT n[, m]]`
    fn show(&mut self) -> bool {
        if !self.word("SHOW") {
            return false;
        }
        let count = self.word("COUNT");
        if count && !(self.symbol('(') && self.symbol('*') && self.symbol(')')) {
            return false;
        }
        if !(self.word("WARNINGS") || self.word("ERRORS")) {
            return false;
        }
        if !count
            && self.word("LIMIT")
            && !(self.integer() && (!self.symbol(',') || self.integer()))
        {
            return false;
        }
        self.end()
    }

    /// `GET [CURRENT|STACKED] DIAGNOSTICS @v = 항목[, …]` 또는 `… CONDITION n|@v @v = 항목[, …]`
    fn get_diagnostics(&mut self) -> bool {
        if !self.word("GET") {
            return false;
        }
        let _ = self.word("CURRENT") || self.word("STACKED");
        if !self.word("DIAGNOSTICS") {
            return false;
        }
        if self.word("CONDITION") && !(self.integer() || self.variable()) {
            return false;
        }
        loop {
            if !(self.variable() && self.symbol('=') && self.name()) {
                return false;
            }
            if !self.symbol(',') {
                break;
            }
        }
        self.end()
    }
}

/// 글이 정확히 진단 문장 하나인가(머리 주석).
pub(crate) fn is_diagnostic(sql: &str) -> bool {
    let Some(tokens) = tokens(sql) else {
        return false;
    };
    let show = Cursor {
        tokens: &tokens,
        at: 0,
    }
    .show();
    show || Cursor {
        tokens: &tokens,
        at: 0,
    }
    .get_diagnostics()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_diagnostic_statements_go_as_text() {
        for sql in [
            "SHOW WARNINGS",
            "show errors;",
            "  SHOW WARNINGS LIMIT 5 ;  ",
            "SHOW ERRORS LIMIT 2, 10",
            "SHOW COUNT(*) WARNINGS",
            "SHOW COUNT( * ) ERRORS;",
            "/* look */ SHOW WARNINGS -- why\n",
            "# note\nSHOW WARNINGS",
            "GET DIAGNOSTICS @n = NUMBER, @r = ROW_COUNT",
            "GET CURRENT DIAGNOSTICS CONDITION 1 @m = MESSAGE_TEXT, @e = MYSQL_ERRNO;",
            "get stacked diagnostics condition @i @s = RETURNED_SQLSTATE",
        ] {
            assert!(is_diagnostic(sql), "{sql}");
        }
    }

    #[test]
    fn anything_else_takes_the_usual_path() {
        for sql in [
            "SHOW WARNINGS; DROP TABLE t",
            "SHOW WARNINGS;; ",
            "SHOW WARNINGS /*! ; DROP TABLE t */",
            "SHOW WARNINGS /*M! ; DROP TABLE t */",
            "SHOW /*+ x */ WARNINGS",
            "SHOW WARNINGS LIMIT 'a'",
            "SHOW WARNINGS LIMIT 1,",
            "SHOW COUNT(*) WARNINGS LIMIT 1",
            "SHOW TABLES",
            "SHOW WARNINGS --x",
            "SHOW WARNINGS /* unterminated",
            "GET DIAGNOSTICS @m = MESSAGE_TEXT; DROP TABLE t",
            "GET DIAGNOSTICS CONDITION 1 @m = MESSAGE_TEXT, @`x` = MYSQL_ERRNO",
            "GET DIAGNOSTICS @'m' = NUMBER",
            "GET DIAGNOSTICS",
            "SELECT 1",
            "",
        ] {
            assert!(!is_diagnostic(sql), "{sql}");
        }
    }
}
