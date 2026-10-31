//! 파서(sqlparser 0.63)가 못 읽는 흔한 문장의 좁은 인식기. 파싱 실패는 파괴 · `unparsed` 가 원칙이고(결정 M3-D5), 여기 있는 꼴만
//! 예외다. 규칙 둘(14-m3-plan.md M3-03 ②):
//!
//! - (가) **조회를 내지 않는다** — 파서가 읽는 쌍둥이 꼴(`PRAGMA table_info('t')` 은 쓰기)과 같거나 높은 등급만.
//! - (나) **정해진 토큰 꼴과 정확히 맞을 때만**(글 전체가 그 문장 하나, 끝의 `;` 하나만 허용). 조금이라도 다르면 `None` 이고
//!   부르는 쪽이 파괴 · `unparsed` 로 닫는다.
//!
//! 일부러 인식하지 않는 것(파괴 그대로): MySQL `LOAD DATA LOCAL INFILE`(사람 기기의 파일을 서버로 보낸다 — 막는 자리는 드라이버,
//! M3-07), PG `DO $$…$$`(본문이 임의의 PL/pgSQL), MySQL `INTO OUTFILE/DUMPFILE` · `HANDLER` · `XA` · `PREPARE … FROM`.

use pw_db_types::{RiskGrade, SqlDialect};
use sqlparser::tokenizer::{Location, Token, TokenWithSpan};

use super::{Classification, Found, RiskReason, classify_text};

pub(super) fn recognize(
    dialect: SqlDialect,
    text: &str,
    tokens: &[TokenWithSpan],
) -> Option<Classification> {
    if let Some(classification) = rewrite_locking_clause(dialect, text, tokens) {
        return Some(classification);
    }
    let significant: Vec<&Token> = tokens
        .iter()
        .map(|token| &token.token)
        .filter(|token| !matches!(token, Token::Whitespace(_) | Token::EOF))
        .collect();
    let statement = match significant.split_last() {
        Some((Token::SemiColon, rest)) => rest,
        _ => &significant[..],
    };
    let (grade, reason) = match dialect {
        SqlDialect::Sqlite => sqlite_pragma(statement)
            .or_else(|| sqlite_detach(statement))
            .or_else(|| sqlite_reindex(statement))?,
        SqlDialect::Postgres => postgres_refresh(statement)?,
        SqlDialect::Mysql => mysql_load_data_infile(statement)?,
    };
    Some(Classification {
        grade,
        reasons: vec![reason],
    })
}

/// 잠금절 꼴 셋(MySQL `LOCK IN SHARE MODE`, PG `FOR NO KEY UPDATE` · `FOR KEY SHARE`)을 파서가 읽는 `FOR SHARE` · `FOR UPDATE` 로
/// 바꿔 다시 분류하고 잠금 읽기(쓰기)를 더한다. 바꾼 자리는 토큰의 위치로 정하므로 문자열 · 주석 안의 같은 낱말은 건드리지 않는다.
fn rewrite_locking_clause(
    dialect: SqlDialect,
    text: &str,
    tokens: &[TokenWithSpan],
) -> Option<Classification> {
    let patterns: &[(&[&str], &str)] = match dialect {
        SqlDialect::Mysql => &[(&["LOCK", "IN", "SHARE", "MODE"], "FOR SHARE")],
        SqlDialect::Postgres => &[
            (&["FOR", "NO", "KEY", "UPDATE"], "FOR UPDATE"),
            (&["FOR", "KEY", "SHARE"], "FOR SHARE"),
        ],
        SqlDialect::Sqlite => return None,
    };
    let words: Vec<&TokenWithSpan> = tokens
        .iter()
        .filter(|token| !matches!(token.token, Token::Whitespace(_) | Token::EOF))
        .collect();
    let line_starts = line_starts(text);
    let mut replacements = Vec::new();
    let mut index = 0;
    while index < words.len() {
        let matched = patterns.iter().find(|(pattern, _)| {
            pattern.len() <= words.len() - index
                && pattern
                    .iter()
                    .zip(&words[index..])
                    .all(|(word, token)| is_keyword(&token.token, word))
        });
        if let Some((pattern, replacement)) = matched {
            let start = byte_offset(text, &line_starts, words[index].span.start)?;
            let end = byte_offset(
                text,
                &line_starts,
                words[index + pattern.len() - 1].span.end,
            )?;
            replacements.push((start, end, *replacement));
            index += pattern.len();
        } else {
            index += 1;
        }
    }
    if replacements.is_empty() {
        return None;
    }
    let mut rewritten = text.to_owned();
    for (start, end, replacement) in replacements.into_iter().rev() {
        rewritten.replace_range(start..end, replacement);
    }
    let outcome = classify_text(dialect, &rewritten);
    // 바꾼 글도 못 읽으면 인식하지 않는다(부르는 쪽이 파괴 · `unparsed`).
    outcome.statements?;
    let mut found = Found::default();
    found.add_classification(&outcome.classification);
    found.add(RiskGrade::Write, RiskReason::LockingRead);
    Some(found.finish())
}

/// SQLite `PRAGMA [schema.]name`, `PRAGMA [schema.]name = 값`, `PRAGMA [schema.]name(값)` — 값은 이름 · 수 · 문자열 하나.
/// 파서는 값이 이름(`journal_mode = WAL`, `table_info(t)`)이면 읽지 못한다.
fn sqlite_pragma(statement: &[&Token]) -> Option<(RiskGrade, RiskReason)> {
    let rest = keyword_prefix(statement, &["PRAGMA"])?;
    let rest = qualified_name(rest)?;
    let ok = match rest {
        [] => true,
        [Token::Eq, value @ ..] => pragma_value(value),
        [Token::LParen, inner @ .., Token::RParen] => pragma_value(inner),
        _ => false,
    };
    ok.then_some((RiskGrade::Write, RiskReason::Pragma))
}

fn pragma_value(value: &[&Token]) -> bool {
    matches!(
        value,
        [Token::Word(_) | Token::Number(..) | Token::SingleQuotedString(_)]
            | [Token::Minus | Token::Plus, Token::Number(..)]
    )
}

/// SQLite `DETACH [DATABASE] 이름`.
fn sqlite_detach(statement: &[&Token]) -> Option<(RiskGrade, RiskReason)> {
    let rest = keyword_prefix(statement, &["DETACH"])?;
    let rest = keyword_prefix(rest, &["DATABASE"]).unwrap_or(rest);
    matches!(rest, [Token::Word(_)]).then_some((RiskGrade::Write, RiskReason::Attach))
}

/// SQLite `REINDEX [[schema.]이름]`.
fn sqlite_reindex(statement: &[&Token]) -> Option<(RiskGrade, RiskReason)> {
    let rest = keyword_prefix(statement, &["REINDEX"])?;
    (rest.is_empty() || qualified_name(rest).is_some_and(<[_]>::is_empty))
        .then_some((RiskGrade::Write, RiskReason::Maintenance))
}

/// PG `REFRESH MATERIALIZED VIEW [CONCURRENTLY] [schema.]이름 [WITH [NO] DATA]` — 뷰의 내용을 다시 쓴다.
fn postgres_refresh(statement: &[&Token]) -> Option<(RiskGrade, RiskReason)> {
    let rest = keyword_prefix(statement, &["REFRESH", "MATERIALIZED", "VIEW"])?;
    let rest = keyword_prefix(rest, &["CONCURRENTLY"]).unwrap_or(rest);
    let rest = qualified_name(rest)?;
    let ok = rest.is_empty()
        || keyword_prefix(rest, &["WITH", "DATA"]).is_some_and(<[_]>::is_empty)
        || keyword_prefix(rest, &["WITH", "NO", "DATA"]).is_some_and(<[_]>::is_empty);
    ok.then_some((RiskGrade::Write, RiskReason::Maintenance))
}

/// MySQL `LOAD DATA [LOW_PRIORITY | CONCURRENT] INFILE '파일' [REPLACE | IGNORE] INTO TABLE [db.]이름 …` — 서버 쪽 파일을 표에 넣는
/// 일괄 쓰기. `LOCAL` 은 이 꼴에 없다(파괴 그대로). 뒤의 선택 절(`FIELDS TERMINATED BY …` · 열 목록 · `IGNORE n LINES` ·
/// `CHARACTER SET`)은 낱말 · 문자열 · 수 · 쉼표 · 괄호만 받는다 — 식을 쓰는 `SET 열 = 식` 절은 받지 않는다.
fn mysql_load_data_infile(statement: &[&Token]) -> Option<(RiskGrade, RiskReason)> {
    let rest = keyword_prefix(statement, &["LOAD", "DATA"])?;
    let rest = keyword_prefix(rest, &["LOW_PRIORITY"])
        .or_else(|| keyword_prefix(rest, &["CONCURRENT"]))
        .unwrap_or(rest);
    let rest = keyword_prefix(rest, &["INFILE"])?;
    let [
        Token::SingleQuotedString(_) | Token::DoubleQuotedString(_),
        rest @ ..,
    ] = rest
    else {
        return None;
    };
    let rest = keyword_prefix(rest, &["REPLACE"])
        .or_else(|| keyword_prefix(rest, &["IGNORE"]))
        .unwrap_or(rest);
    let rest = keyword_prefix(rest, &["INTO", "TABLE"])?;
    let tail = qualified_name(rest)?;
    let tail_ok = tail.iter().enumerate().all(|(index, token)| match token {
        Token::Word(word)
            if word.quote_style.is_none() && word.value.eq_ignore_ascii_case("SET") =>
        {
            index > 0 && is_keyword(tail[index - 1], "CHARACTER")
        }
        Token::Word(_)
        | Token::Number(..)
        | Token::SingleQuotedString(_)
        | Token::DoubleQuotedString(_)
        | Token::Comma
        | Token::LParen
        | Token::RParen => true,
        _ => false,
    });
    tail_ok.then_some((RiskGrade::Write, RiskReason::DataTransfer))
}

fn is_keyword(token: &Token, keyword: &str) -> bool {
    matches!(token, Token::Word(word) if word.quote_style.is_none() && word.value.eq_ignore_ascii_case(keyword))
}

/// `keywords` 로 시작하면 그 뒤를 돌려준다.
fn keyword_prefix<'a, 'b>(tokens: &'a [&'b Token], keywords: &[&str]) -> Option<&'a [&'b Token]> {
    (tokens.len() >= keywords.len()
        && keywords
            .iter()
            .zip(tokens)
            .all(|(keyword, token)| is_keyword(token, keyword)))
    .then(|| &tokens[keywords.len()..])
}

/// `이름` 또는 `schema.이름` 을 지나고 그 뒤를 돌려준다.
fn qualified_name<'a, 'b>(tokens: &'a [&'b Token]) -> Option<&'a [&'b Token]> {
    match tokens {
        [Token::Word(_), Token::Period, Token::Word(_), rest @ ..] => Some(rest),
        [Token::Word(_), rest @ ..] => Some(rest),
        _ => None,
    }
}

fn line_starts(text: &str) -> Vec<usize> {
    std::iter::once(0)
        .chain(text.match_indices('\n').map(|(index, _)| index + 1))
        .collect()
}

/// 토큰 위치(1부터 센 줄 · 글자)를 바이트 자리로.
fn byte_offset(text: &str, line_starts: &[usize], location: Location) -> Option<usize> {
    let line_start = *line_starts.get(usize::try_from(location.line).ok()?.checked_sub(1)?)?;
    let column = usize::try_from(location.column).ok()?.checked_sub(1)?;
    let line = &text[line_start..];
    let offset = if column == line.chars().count() {
        line.len()
    } else {
        line.char_indices().nth(column)?.0
    };
    Some(line_start + offset)
}
