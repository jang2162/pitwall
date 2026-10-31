//! MySQL · MariaDB 글은 서버 설정 · 판본에 따라 다르게 읽힌다. 분류는 서버가 고를 수 있는 읽기를 모두 분류해 가장 높은 등급을 받는다.
//!
//! - **실행 주석**: `/*! … */` 은 MySQL · MariaDB 가 늘 실행한다. `/*!NNNNN … */` 은 서버 판본이 NNNNN 이상일 때만, `/*M! … */` ·
//!   `/*M!NNNNNN … */` 은 MariaDB(그 판본 이상)만 실행하고 MySQL 은 주석으로 버린다. sqlparser 는 판본과 상관없이 늘 펼치므로 경계를
//!   주석 안에 두면(`DELETE FROM t /*!99999 WHERE id = 1 */`) 분류기는 범위 있는 쓰기로, MySQL 8 은 표 전체로 읽는다. 그래서 판본이
//!   붙었거나 MariaDB 전용인 주석은 「서버마다 다른 열쇠」 로 보고, 열쇠마다 펼침 · 버림의 모든 조합을 분류한다. 열쇠가
//!   [`MAX_EXECUTABLE_COMMENT_KEYS`] 를 넘으면 조합이 너무 많아 파괴 · `unparsed` 다. 어느 읽기든 파싱에 실패하면 파괴 · `unparsed`.
//! - **백슬래시**: 기본 `sql_mode` 에서 `\` 는 문자열 안의 이스케이프지만 `NO_BACKSLASH_ESCAPES` 면 글자다. `'a\'; DROP TABLE t; -- '`
//!   는 앞의 읽기로 문자열 하나, 뒤의 읽기로 문장 둘이다. 글에 `\` 가 있으면 두 읽기로 다 나눠 보고(문장 경계 `;` 를 이 파일의
//!   스캐너로 센다), 문장 수가 다르면 파괴 · `unparsed`. 같으면 뒤의 읽기도 분류해 높은 등급을 받는다(`DELETE … WHERE c = 'x\' OR
//!   1=1 -- '` 는 뒤의 읽기로 범위 없는 쓰기다). 단 뒤의 읽기를 읽지 못하면(파싱 실패 · 상한) 그 읽기는 등급에 넣지 않는다 — 넣으면 `'a\'b'` 같은
//!   흔한 데이터가 따옴표의 홀짝에 따라 파괴가 된다. 남는 틈: `NO_BACKSLASH_ESCAPES` 서버에서, 뒤의 읽기가 sqlparser 는 못 읽고
//!   서버는 읽는 꼴일 때(`WHERE c = 'a\' OR 1=1 INTO OUTFILE '…' -- '` 가 조회로 돌아 파일을 쓴다 — M3-17 리뷰 X3, MySQL 8.4.6).
//!   그래서 넣지 않은 읽기가 있었다는 것을 [`StatementReadings::unread_reading`] 으로 알리고, 조회만 받는 호출은 그 글을 조회로
//!   보지 않는다 — 사람이 치는 일반 실행의 등급은 그대로라 틈이 남는다. 뒤의 읽기에서 따옴표가 닫히지 않으면 서버는 그 문장에서
//!   멈추므로 그 앞에서 끝난 문장들만 본다.
//!
//! 이 둘은 분류 쪽의 방어이고, 「나뉜 문장 하나 = 서버가 돌리는 문장 하나」(드라이버가 다중 문장을 끄는 것, M3-06 · M3-07)가 겹방어다.

use pw_db_types::{RiskGrade, SqlDialect};

use super::{
    Classification, Found, MAX_STATEMENT_BYTES, RiskReason, StatementReadings, classify_text,
};

/// 서버마다 다른 실행 주석 열쇠(판본 · MariaDB 전용)의 상한. 조합은 2^열쇠 수 × 백슬래시 읽기 2 번까지 분류한다(4 → 최대 32 번).
pub(super) const MAX_EXECUTABLE_COMMENT_KEYS: usize = 4;

pub(super) fn classify(text: &str) -> StatementReadings {
    let only = |classification| StatementReadings {
        classification,
        unread_reading: false,
    };
    let escaped = match scan(text, true) {
        Ok(scan) => scan,
        // 이스케이프 읽기에서 닫히지 않은 글은 파서도 읽지 못한다.
        Err(_) => return only(Classification::unparsed()),
    };
    let keys = variable_keys(&escaped.pieces);
    if keys.len() > MAX_EXECUTABLE_COMMENT_KEYS {
        return only(Classification::unparsed());
    }
    let backslash = text.contains('\\');
    // 읽기마다 글 전체를 분류하므로 바이트 상한을 읽기 수로 나눠 쓴다 — 시간 상한(문장당 200 ms 목표)을 지키려고.
    let readings = (1usize << keys.len()) << usize::from(backslash);
    if text.len().saturating_mul(readings) > MAX_STATEMENT_BYTES {
        return only(Classification::destructive(RiskReason::TooLarge));
    }

    let mut found = Found::default();
    for mask in 0..(1u32 << keys.len()) {
        let reading = render(&escaped.pieces, &keys, mask, true);
        found.add_classification(&classify_text(SqlDialect::Mysql, &reading).classification);
    }
    if backslash {
        classify_without_backslash_escapes(text, &escaped, &keys, &mut found);
    }
    let unread_reading = found.unread_reading;
    StatementReadings {
        classification: found.finish(),
        unread_reading,
    }
}

/// `NO_BACKSLASH_ESCAPES` 의 읽기(머리 주석 「백슬래시」).
fn classify_without_backslash_escapes(
    text: &str,
    escaped: &Scan<'_>,
    keys: &[CommentKey],
    found: &mut Found,
) {
    let split_differs = |found: &mut Found| found.add(RiskGrade::Destructive, RiskReason::Unparsed);
    let plain = match scan(text, false) {
        Ok(plain) => plain,
        // 닫히지 않은 따옴표가 있는 문장에서 서버가 멈춘다. 그 앞에서 끝난 문장들만 돈다.
        Err(ScanError::Unterminated {
            last_statement_end,
            statements_before,
        }) => {
            if statements_before + 1 != escaped.statements {
                return split_differs(found);
            }
            let Some(end) = last_statement_end else {
                return;
            };
            match scan(&text[..end], false) {
                Ok(prefix) => Scan {
                    complete: false,
                    ..prefix
                },
                Err(_) => return split_differs(found),
            }
        }
        Err(ScanError::Invalid) => return split_differs(found),
    };
    if variable_keys(&plain.pieces) != keys
        || (plain.statements != escaped.statements && plain.complete)
    {
        return split_differs(found);
    }
    for mask in 0..(1u32 << keys.len()) {
        let outcome = classify_text(SqlDialect::Mysql, &render(&plain.pieces, keys, mask, false));
        // 읽지 못한 읽기(파싱 실패 · 상한)는 넣지 않는다(머리 주석 「백슬래시」). 조회는 까닭이 비어 있다 — 읽은 것이다.
        let reasons = &outcome.classification.reasons;
        let not_read = !reasons.is_empty()
            && reasons.iter().all(|reason| {
                matches!(
                    reason,
                    RiskReason::Unparsed | RiskReason::TooLarge | RiskReason::TooDeep
                )
            });
        if not_read {
            found.unread_reading = true;
        } else {
            found.add_classification(&outcome.classification);
        }
    }
}

/// 서버마다 다르게 실행되는 주석의 열쇠: (MariaDB 전용인가, 판본 숫자). 판본 없는 `/*!` 는 늘 실행되므로 열쇠가 아니다.
#[derive(Debug, Clone, PartialEq, Eq)]
struct CommentKey {
    mariadb: bool,
    version: String,
}

#[derive(Debug)]
enum Piece<'a> {
    /// 코드 · 주석 · 백틱 이름(그대로 낸다).
    Text(&'a str),
    /// 따옴표 문자열(따옴표 포함).
    Str(&'a str),
    /// 실행 주석의 여는 꼴(`/*!50000` · `/*M!` …). `None` 은 늘 실행되는 `/*!`.
    ExecOpen(Option<CommentKey>),
    /// 실행 주석의 닫는 `*/`.
    ExecClose,
}

struct Scan<'a> {
    pieces: Vec<Piece<'a>>,
    /// 문장 수 — `;` 로 나뉜 조각 가운데 주석 · 빈칸 밖의 것이 있는 조각(실행 주석은 코드로 센다).
    statements: usize,
    /// 글 전체를 읽었는가(닫히지 않은 따옴표에서 앞부분만 다시 읽은 것이면 거짓).
    complete: bool,
}

#[derive(Debug)]
enum ScanError {
    /// 따옴표 · 주석이 닫히지 않았다. `last_statement_end` 는 그 앞의 마지막 `;` 바로 뒤, `statements_before` 는 그 앞의 문장 수.
    Unterminated {
        last_statement_end: Option<usize>,
        statements_before: usize,
    },
    /// 실행 주석이 겹치거나 닫히지 않았거나 그 안에 `;` 가 있다 — 서버마다 나뉨이 갈려 분류하지 않는다.
    Invalid,
}

/// MySQL 렉서의 경계만 흉내 낸다: 따옴표 문자열(`''` 겹침, `backslash_escapes` 면 `\` 이스케이프), 백틱 이름(`` `` `` 겹침),
/// `#` · `-- ` 줄 주석, `/* */` 주석(겹치지 않음), 실행 주석의 여닫이, 문장 경계 `;`. 실행 주석 안은 코드로 계속 읽는다.
fn scan(text: &str, backslash_escapes: bool) -> Result<Scan<'_>, ScanError> {
    let bytes = text.as_bytes();
    let mut pieces = Vec::new();
    let mut start = 0;
    let mut i = 0;
    let mut in_exec = false;
    let mut last_statement_end = None;
    let mut statements = 0;
    // 지금 문장에 주석 · 빈칸 밖의 것이 있었는가.
    let mut has_code = false;
    let unterminated = |last_statement_end, statements| ScanError::Unterminated {
        last_statement_end,
        statements_before: statements,
    };

    while i < bytes.len() {
        match bytes[i] {
            quote @ (b'\'' | b'"') => {
                push_text(&mut pieces, text, start, i);
                let open = i;
                i += 1;
                loop {
                    let Some(&byte) = bytes.get(i) else {
                        return Err(unterminated(last_statement_end, statements));
                    };
                    if backslash_escapes && byte == b'\\' {
                        i += 2;
                    } else if byte == quote {
                        if bytes.get(i + 1) == Some(&quote) {
                            i += 2;
                        } else {
                            i += 1;
                            break;
                        }
                    } else {
                        i += 1;
                    }
                }
                pieces.push(Piece::Str(&text[open..i]));
                start = i;
                has_code = true;
            }
            b'`' => {
                i += 1;
                loop {
                    match bytes.get(i) {
                        None => return Err(unterminated(last_statement_end, statements)),
                        Some(b'`') if bytes.get(i + 1) == Some(&b'`') => i += 2,
                        Some(b'`') => {
                            i += 1;
                            break;
                        }
                        Some(_) => i += 1,
                    }
                }
                has_code = true;
            }
            b'#' => i = line_end(bytes, i),
            b'-' if bytes.get(i + 1) == Some(&b'-')
                && bytes.get(i + 2).is_none_or(|byte| *byte <= b' ') =>
            {
                i = line_end(bytes, i);
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                let rest = &text[i..];
                let open_len = if rest.starts_with("/*!") {
                    Some((3, false))
                } else if rest.starts_with("/*M!") {
                    Some((4, true))
                } else {
                    None
                };
                if let Some((len, mariadb)) = open_len {
                    if in_exec {
                        return Err(ScanError::Invalid);
                    }
                    push_text(&mut pieces, text, start, i);
                    let digits_start = i + len;
                    let digits_end = digits_start
                        + bytes[digits_start..]
                            .iter()
                            .take_while(|byte| byte.is_ascii_digit())
                            .count();
                    let version = &text[digits_start..digits_end];
                    let key = (mariadb || !version.is_empty()).then(|| CommentKey {
                        mariadb,
                        version: version.to_owned(),
                    });
                    pieces.push(Piece::ExecOpen(key));
                    in_exec = true;
                    has_code = true;
                    i = digits_end;
                    start = i;
                } else {
                    let Some(close) = text[i + 2..].find("*/") else {
                        return Err(unterminated(last_statement_end, statements));
                    };
                    i += 2 + close + 2;
                }
            }
            b'*' if in_exec && bytes.get(i + 1) == Some(&b'/') => {
                push_text(&mut pieces, text, start, i);
                pieces.push(Piece::ExecClose);
                in_exec = false;
                i += 2;
                start = i;
            }
            // 실행 주석 안의 `;` 는 서버마다 나뉨을 바꾼다.
            b';' if in_exec => return Err(ScanError::Invalid),
            b';' => {
                i += 1;
                last_statement_end = Some(i);
                statements += usize::from(has_code);
                has_code = false;
            }
            byte => {
                has_code |= !byte.is_ascii_whitespace();
                i += 1;
            }
        }
    }
    if in_exec {
        return Err(ScanError::Invalid);
    }
    push_text(&mut pieces, text, start, bytes.len());
    Ok(Scan {
        pieces,
        statements: statements + usize::from(has_code),
        complete: true,
    })
}

fn push_text<'a>(pieces: &mut Vec<Piece<'a>>, text: &'a str, start: usize, end: usize) {
    if start < end {
        pieces.push(Piece::Text(&text[start..end]));
    }
}

fn line_end(bytes: &[u8], from: usize) -> usize {
    bytes[from..]
        .iter()
        .position(|byte| *byte == b'\n')
        .map_or(bytes.len(), |offset| from + offset)
}

fn variable_keys(pieces: &[Piece<'_>]) -> Vec<CommentKey> {
    let mut keys: Vec<CommentKey> = Vec::new();
    for piece in pieces {
        if let Piece::ExecOpen(Some(key)) = piece
            && !keys.contains(key)
        {
            keys.push(key.clone());
        }
    }
    keys
}

/// 한 읽기의 글을 만든다. 펼치는 실행 주석은 여닫이를 빈칸으로 바꿔 안을 코드로, 버리는 것은 통째로 빈칸으로. 백슬래시 없는
/// 읽기는 문자열 안의 `\` 를 `\\` 로 바꿔, 늘 이스케이프로 읽는 파서가 서버와 같은 자리에서 문자열을 닫게 한다.
fn render(pieces: &[Piece<'_>], keys: &[CommentKey], mask: u32, backslash_escapes: bool) -> String {
    let mut out = String::new();
    let mut skipping = false;
    for piece in pieces {
        match piece {
            Piece::ExecOpen(key) => {
                out.push(' ');
                skipping = key.as_ref().is_some_and(|key| {
                    let index = keys
                        .iter()
                        .position(|candidate| candidate == key)
                        .unwrap_or(0);
                    mask & (1 << index) == 0
                });
            }
            Piece::ExecClose => {
                out.push(' ');
                skipping = false;
            }
            _ if skipping => {}
            Piece::Text(text) => out.push_str(text),
            Piece::Str(text) if backslash_escapes => out.push_str(text),
            Piece::Str(text) => out.push_str(&text.replace('\\', "\\\\")),
        }
    }
    out
}
