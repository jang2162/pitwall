//! 문장 나누기 · 현재 문장(M3-04, `docs/design/14-m3-plan.md`). 직접 구현했다 — DBX `SqlStatementSplitter` 의 코드는 옮기지
//! 않았다(결정 M3-D5 의 차용 범위 밖).
//!
//! 바이트 단위 렉서 하나가 방언별 따옴표 · 식별자 인용 · 주석 · PG 달러 인용 · MySQL `DELIMITER` · 루틴의 `BEGIN … END` 를
//! 지나 구분자에서 나눈다. 파서가 아니므로 파서가 못 읽는 문장도 나뉜다(분류는 M3-03 이 문장마다 따로 한다).
//!
//! **범위**: 문장의 범위는 앞뒤의 공백 · 주석을 뺀 첫 토큰부터 마지막 토큰까지다 — 끝의 구분자(`;` 나 `DELIMITER` 로 바꾼
//! 것)는 들지 않는다. 앞 주석을 빼는 까닭: `SELECT 1; -- 하나` 다음 줄의 문장에 그 주석이 붙으면, 첫 줄 끝의 커서가 둘째
//! 문장을 고른다. 공백 · 주석뿐인 덩어리(`;;`, 마지막 `;` 뒤의 주석)는 문장이 아니다. MySQL 의 `/*! … */` 와 MariaDB 의
//! `/*M! … */` 실행 주석은 주석이 아니라 내용이다(서버가 안을 실행한다 — mysqldump 의 `/*!40101 SET … */;` 가 문장이다).
//!
//! **닫히지 않은 것은 끝까지 내용이다**: 문자열 · 달러 인용 · 블록 주석 · PG 괄호가 닫히지 않으면 그 뒤 전체가 한 문장이다.
//! 서버와 분류기(M3-03, 파싱 실패는 파괴 등급)가 모두 오류로 보는 쪽이다 — 주석이라고 버리면 「전체 실행」 이 앞 문장만
//! 돌리고 조용히 끝난다.
//!
//! **PG 의 괄호**: psql 처럼 괄호 안의 `;` 에서 나누지 않는다(`CREATE RULE … DO ALSO (…; …)` 가 한 문장). 나누면 규칙 본문의
//! `DELETE FROM t` 가 혼자 유효한 문장이 된다. MySQL · SQLite 클라이언트는 괄호를 세지 않으므로 그대로 나눈다.
//!
//! **오프셋**: RPC 의 오프셋은 UTF-16 단위다(CodeMirror 와 같다). Rust 쪽은 바이트 범위도 함께 준다([`Statement::bytes`]).
//!
//! **루틴 본문**: `CREATE`/`ALTER` 뒤 처음 나온 객체 종류 낱말이 `PROCEDURE` · `FUNCTION` · `TRIGGER` · `EVENT` 인 문장에서,
//! 본문 자리의 `BEGIN` 만 블록을 연다(MySQL 저장 프로시저 · SQLite 트리거 · PG `BEGIN ATOMIC`, MariaDB 는 루틴 밖의
//! `BEGIN NOT ATOMIC` 도). 본문 자리: PG 는 `BEGIN ATOMIC` 일 때만. MySQL · SQLite 는 괄호 밖이고, 앞 토큰이 `)` · `:` ·
//! 인용 · 식을 기대하지 않는 낱말(`NOT_BEFORE_BODY` 밖)이고, 다음 토큰이 낱말(`NOT_AFTER_BODY` 밖)일 때 — 그래서 매개변수
//! `p(IN begin INT)` · `NEW.begin` · `SET begin = 1` · `SELECT begin FROM t` 의 열 이름 `begin` 은 열지 않는다. 블록 안에서는
//! 문장 머리(앞 토큰이 `;` · `:` · `BEGIN` · `THEN` · `ELSE` · `DO` · `LOOP` · `REPEAT`)의 `BEGIN` 과 `CASE` 가 열고, `END`
//! (`END CASE` 포함, `END IF` · `LOOP` · `WHILE` · `REPEAT` · `FOR` 제외)가 닫으며, 블록 안의 `;` 는 나누지 않는다. 그래서
//! 트랜잭션 `BEGIN;` 과 `CREATE TABLE t (event int, begin int)` 는 블록을 열지 않는다. `DELIMITER` 로 바꾼 구분자는 블록과
//! 상관없이 늘 나눈다(mysql 클라이언트와 같다). 구분자는 mysql 처럼 15 바이트까지다(넘는 부분은 버린다 — 긴 구분자로
//! 나누기가 이차 시간이 되지 않게).
//!
//! **나누기가 서버와 갈라질 때**: 서버 설정(MySQL `NO_BACKSLASH_ESCAPES` · `ANSI_QUOTES`, PG `standard_conforming_strings`
//! off)은 모르므로 기본값으로 읽는다. 마지막 방어는 실행 경로의 계약이다 — 나뉜 문장 하나를 서버 문장 하나로만 보낸다(PG
//! 확장 프로토콜, MySQL 다중 문장 끔 — 14-m3-plan.md M3-05 · M3-06 · M3-07). 갈라지면 숨은 둘째 문장이 아니라 서버 문법
//! 오류가 된다.
//!
//! 하지 않는 것: psql · mysql 의 백슬래시 명령(`\g` 등), 서버 모드에 따른 렉싱(위), 루틴 밖의 MariaDB 복합문(`IF … END IF`
//! 단독), 블록 안에서 열 이름으로 쓴 `end`(MySQL 의 비예약어 — 블록을 일찍 닫아 더 나뉘는 쪽이다).

use std::ops::Range;

use pw_db_types::SqlDialect;
use pw_rpc::{Router, RpcError, rpc_method};
use serde::{Deserialize, Serialize};

/// 원문 안의 범위. UTF-16 단위, `to` 는 포함하지 않는다(CodeMirror 의 `from` · `to` 와 같다).
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    specta::Type,
    schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct SqlRange {
    pub from: u32,
    pub to: u32,
}

/// 나눈 문장 하나.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Statement {
    /// 원문의 바이트 범위([`Statement::text`]).
    pub bytes: Range<usize>,
    /// 같은 범위의 UTF-16 오프셋.
    pub range: SqlRange,
}

impl Statement {
    /// 이 문장의 원문(끝의 구분자 없이).
    pub fn text<'a>(&self, sql: &'a str) -> &'a str {
        &sql[self.bytes.clone()]
    }
}

/// `sql` 을 문장으로 나눈다. 순서대로이고 범위가 겹치지 않는다.
pub fn split(dialect: SqlDialect, sql: &str) -> Vec<Statement> {
    let bytes = Splitter::new(dialect, sql.as_bytes()).run();
    let mut utf16 = Utf16Walker::new(sql);
    bytes
        .into_iter()
        .map(|bytes| {
            let from = utf16.at(bytes.start);
            let to = utf16.at(bytes.end);
            Statement {
                bytes,
                range: SqlRange { from, to },
            }
        })
        .collect()
}

/// 커서(UTF-16 오프셋)가 가리키는 문장의 번호. 커서가 문장 안이나 바로 뒤면 그 문장, 두 문장 사이(공백 · 주석 · `;`)면 앞
/// 문장, 첫 문장 앞이면 첫 문장이다. 문장이 없으면 `None`.
pub fn statement_index_at(statements: &[Statement], offset: u32) -> Option<usize> {
    if statements.is_empty() {
        return None;
    }
    let started = statements.partition_point(|s| s.range.from <= offset);
    Some(started.saturating_sub(1))
}

/// [`split`] 뒤 [`statement_index_at`]. 번호와 그 문장.
pub fn statement_at(dialect: SqlDialect, sql: &str, offset: u32) -> Option<(usize, Statement)> {
    let mut statements = split(dialect, sql);
    let index = statement_index_at(&statements, offset)?;
    Some((index, statements.swap_remove(index)))
}

/// 바이트 오프셋을 UTF-16 오프셋으로 바꾼다. 오프셋은 늘어나는 순서로만 묻는다(한 번 훑기).
struct Utf16Walker<'a> {
    sql: &'a str,
    byte: usize,
    units: u32,
}

impl<'a> Utf16Walker<'a> {
    fn new(sql: &'a str) -> Self {
        Self {
            sql,
            byte: 0,
            units: 0,
        }
    }

    fn at(&mut self, byte: usize) -> u32 {
        debug_assert!(byte >= self.byte, "offsets must not go backwards");
        let gap = &self.sql[self.byte..byte];
        self.units += if gap.is_ascii() {
            gap.len() as u32
        } else {
            gap.chars().map(|c| c.len_utf16() as u32).sum()
        };
        self.byte = byte;
        self.units
    }
}

/// 문장 머리(`CREATE`/`ALTER` 뒤 처음 나온 객체 종류)로 정한 루틴 여부.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Head {
    /// 아직 첫 낱말이 없다.
    Empty,
    /// `CREATE`/`ALTER` 로 시작했고 객체 종류 낱말을 아직 못 봤다.
    CreateOrAlter,
    /// 루틴 정의 — 본문 자리의 `BEGIN` 이 블록을 연다.
    Routine,
    /// 그 밖 — `BEGIN` 이 블록을 열지 않는다.
    Other,
}

/// 루틴이 아닌 객체 종류. 이것이 먼저 나오면 뒤의 `EVENT` · `BEGIN` 이 열 이름이어도 블록을 열지 않는다.
const OTHER_OBJECTS: &[&str] = &[
    "TABLE",
    "VIEW",
    "INDEX",
    "SCHEMA",
    "DATABASE",
    "SEQUENCE",
    "TYPE",
    "DOMAIN",
    "ROLE",
    "USER",
    "EXTENSION",
    "MATERIALIZED",
    "RULE",
    "POLICY",
    "SERVER",
    "TABLESPACE",
    "COLLATION",
    "CAST",
];
const ROUTINE_OBJECTS: &[&str] = &["PROCEDURE", "FUNCTION", "TRIGGER", "EVENT"];
/// 블록 안에서 `END` 뒤에 오면 블록이 아니라 그 제어문을 닫는 낱말.
const END_OF_CONTROL: &[&str] = &["IF", "LOOP", "WHILE", "REPEAT", "FOR"];
/// 루틴 머리에서 이 낱말 바로 뒤의 `begin` 은 본문이 아니라 식 · 이름이다(`SELECT begin` · `SET begin =` · `PROCEDURE begin`).
const NOT_BEFORE_BODY: &[&str] = &[
    "SELECT",
    "SET",
    "WHERE",
    "BY",
    "AND",
    "OR",
    "NOT",
    "ON",
    "AS",
    "DISTINCT",
    "WHEN",
    "THEN",
    "ELSE",
    "RETURN",
    "INTO",
    "VALUES",
    "IN",
    "OUT",
    "INOUT",
    "IS",
    "LIKE",
    "HAVING",
    "FROM",
    "JOIN",
    "UPDATE",
    "DECLARE",
    "CALL",
    "PROCEDURE",
    "FUNCTION",
    "TRIGGER",
    "EVENT",
    "TABLE",
    "BETWEEN",
    "DIV",
    "MOD",
    "XOR",
    "REGEXP",
];
/// 루틴 머리에서 `begin` 바로 뒤에 이 낱말이 오면 `begin` 은 식이다(`SELECT max(x) begin FROM t`).
const NOT_AFTER_BODY: &[&str] = &[
    "FROM", "AS", "IS", "IN", "LIKE", "AND", "OR", "BETWEEN", "NOT", "ASC", "DESC", "REGEXP",
    "DIV", "MOD", "XOR",
];
/// 블록 안에서 이 낱말 바로 뒤는 문장 머리다 — 거기의 `BEGIN` 이 겹친 블록을 연다.
const STATEMENT_STARTERS: &[&str] = &["BEGIN", "THEN", "ELSE", "DO", "LOOP", "REPEAT"];
/// mysql 클라이언트의 구분자 길이 상한(`mysql.cc` 의 `char delimiter[16]`).
pub const MAX_DELIMITER_BYTES: usize = 15;

struct Splitter<'a> {
    b: &'a [u8],
    dialect: SqlDialect,
    /// 지금의 구분자. MySQL 은 `DELIMITER` 로 바꾼다.
    delimiter: Vec<u8>,
    out: Vec<Range<usize>>,
    /// 지금 문장의 첫 내용 토큰 시작과 마지막 내용 토큰 끝.
    content: Option<Range<usize>>,
    /// 마지막 내용 토큰(본문 자리 판정이 앞 토큰을 본다).
    last: Option<Range<usize>>,
    head: Head,
    /// `BEGIN` · `CASE` 블록 깊이.
    depth: u32,
    /// 괄호 깊이. PG 는 이것이 0 일 때만 `;` 로 나눈다.
    parens: u32,
}

impl<'a> Splitter<'a> {
    fn new(dialect: SqlDialect, b: &'a [u8]) -> Self {
        Self {
            b,
            dialect,
            delimiter: b";".to_vec(),
            out: Vec::new(),
            content: None,
            last: None,
            head: Head::Empty,
            depth: 0,
            parens: 0,
        }
    }

    fn run(mut self) -> Vec<Range<usize>> {
        let n = self.b.len();
        let mut i = 0;
        while i < n {
            if let Some(len) = self.delimiter_at(i) {
                self.finish();
                i += len;
                continue;
            }
            let c = self.b[i];
            let next = self.b.get(i + 1).copied();
            i = match c {
                _ if c.is_ascii_whitespace() => i + 1,
                b'-' if next == Some(b'-') && self.dash_comment_at(i) => self.line_end(i),
                b'#' if self.dialect == SqlDialect::Mysql => self.line_end(i),
                // MySQL `/*! … */` · MariaDB `/*M! … */` 은 내용이다 — 여는 글자만 내용으로 넘기고 안은 평소처럼 읽는다(머리 주석).
                b'/' if next == Some(b'*') && self.executable_comment_at(i) > 0 => {
                    let len = self.executable_comment_at(i);
                    self.mark(i, i + len)
                }
                b'/' if next == Some(b'*') => match self.block_comment_end(i) {
                    (end, true) => end,
                    // 닫히지 않은 주석은 끝까지 내용이다(머리 주석).
                    (end, false) => self.mark(i, end),
                },
                b'\'' => {
                    let end = self.quoted_end(i, b'\'', self.dialect == SqlDialect::Mysql);
                    self.mark(i, end)
                }
                b'"' => {
                    let end = self.quoted_end(i, b'"', self.dialect == SqlDialect::Mysql);
                    self.mark(i, end)
                }
                b'`' if self.dialect != SqlDialect::Postgres => {
                    let end = self.quoted_end(i, b'`', false);
                    self.mark(i, end)
                }
                b'[' if self.dialect == SqlDialect::Sqlite => {
                    let end = self.bracket_end(i);
                    self.mark(i, end)
                }
                b'$' if self.dialect == SqlDialect::Postgres => {
                    if let Some(tag_end) = self.dollar_tag(i) {
                        let end = self.dollar_quoted_end(i, tag_end);
                        self.mark(i, end)
                    } else if next.is_some_and(|c| c.is_ascii_digit()) {
                        // 매개변수 `$1` 은 숫자까지다 — `$1$a$ … $a$` 의 `$a$` 는 달러 인용이다(PG 렉서와 같다).
                        let end = i
                            + 1
                            + self.b[i + 1..]
                                .iter()
                                .take_while(|c| c.is_ascii_digit())
                                .count();
                        self.mark(i, end)
                    } else {
                        self.word(i)
                    }
                }
                b'(' => {
                    self.parens += 1;
                    self.mark(i, i + 1)
                }
                b')' => {
                    self.parens = self.parens.saturating_sub(1);
                    self.mark(i, i + 1)
                }
                _ if is_word_byte(c) => self.word(i),
                _ => self.mark(i, i + 1),
            };
        }
        self.finish();
        self.out
    }

    /// `i` 에서 구분자가 시작하면 그 길이. 기본 `;` 는 블록 밖(PG 는 괄호 밖도)에서만 나누고, `DELIMITER` 로 바꾼 구분자는 늘
    /// 나눈다.
    fn delimiter_at(&self, i: usize) -> Option<usize> {
        if self.delimiter == b";" {
            let in_parens = self.dialect == SqlDialect::Postgres && self.parens > 0;
            (self.b[i] == b';' && self.depth == 0 && !in_parens).then_some(1)
        } else {
            self.b[i..]
                .starts_with(&self.delimiter)
                .then_some(self.delimiter.len())
        }
    }

    /// 지금 문장을 닫는다. 내용이 없으면(공백 · 주석뿐) 문장이 아니다.
    fn finish(&mut self) {
        if let Some(content) = self.content.take() {
            self.out.push(content);
        }
        self.last = None;
        self.head = Head::Empty;
        self.depth = 0;
        self.parens = 0;
    }

    /// `start..end` 를 내용으로 적고 `end` 를 돌려준다.
    fn mark(&mut self, start: usize, end: usize) -> usize {
        match &mut self.content {
            Some(content) => content.end = end,
            None => self.content = Some(start..end),
        }
        self.last = Some(start..end);
        end
    }

    /// MySQL 의 실행 주석 `/*!` · `/*M!` 이면 여는 글자 수, 아니면 0.
    fn executable_comment_at(&self, i: usize) -> usize {
        if self.dialect != SqlDialect::Mysql {
            return 0;
        }
        if self.b.get(i + 2) == Some(&b'!') {
            3
        } else if self.b.get(i + 2..i + 4) == Some(b"M!") {
            4
        } else {
            0
        }
    }

    /// `--` 가 주석인가. MySQL 은 뒤에 공백 · 제어 문자가 있거나 끝이어야 한다(`1--1` 은 빼기 둘이다).
    fn dash_comment_at(&self, i: usize) -> bool {
        self.dialect != SqlDialect::Mysql
            || self
                .b
                .get(i + 2)
                .is_none_or(|&c| c.is_ascii_whitespace() || c.is_ascii_control())
    }

    /// 줄 주석의 끝(줄바꿈 앞).
    fn line_end(&self, i: usize) -> usize {
        self.b[i..]
            .iter()
            .position(|&c| c == b'\n')
            .map_or(self.b.len(), |p| i + p)
    }

    /// `/* … */` 의 끝과 닫혔는지. PG 는 겹쳐 쓸 수 있다. 닫히지 않으면 끝까지.
    fn block_comment_end(&self, i: usize) -> (usize, bool) {
        let nested = self.dialect == SqlDialect::Postgres;
        let mut depth = 1u32;
        let mut j = i + 2;
        while j < self.b.len() {
            match (self.b[j], self.b.get(j + 1)) {
                (b'*', Some(b'/')) => {
                    depth -= 1;
                    j += 2;
                    if depth == 0 {
                        return (j, true);
                    }
                }
                (b'/', Some(b'*')) if nested => {
                    depth += 1;
                    j += 2;
                }
                _ => j += 1,
            }
        }
        (self.b.len(), false)
    }

    /// 따옴표 `q` 로 연 것의 끝(닫는 따옴표 뒤). 같은 따옴표 둘은 그 글자 하나이고, `backslash` 면 `\` 가 다음 글자를 감춘다.
    /// 닫히지 않으면 끝까지.
    fn quoted_end(&self, i: usize, q: u8, backslash: bool) -> usize {
        let mut j = i + 1;
        while j < self.b.len() {
            let c = self.b[j];
            if backslash && c == b'\\' {
                j += 2;
            } else if c == q {
                if self.b.get(j + 1) == Some(&q) {
                    j += 2;
                } else {
                    return j + 1;
                }
            } else {
                j += 1;
            }
        }
        self.b.len()
    }

    /// SQLite 의 `[식별자]`.
    fn bracket_end(&self, i: usize) -> usize {
        self.b[i..]
            .iter()
            .position(|&c| c == b']')
            .map_or(self.b.len(), |p| i + p + 1)
    }

    /// PG 달러 인용의 여는 꼬리표(`$$` · `$tag$`)면 그 끝. 꼬리표는 숫자로 시작하지 않는다(`$1` 은 매개변수).
    fn dollar_tag(&self, i: usize) -> Option<usize> {
        let mut j = i + 1;
        if let Some(&c) = self.b.get(j)
            && c != b'$'
        {
            if !(c.is_ascii_alphabetic() || c == b'_' || c >= 0x80) {
                return None;
            }
            while j < self.b.len()
                && (self.b[j].is_ascii_alphanumeric() || self.b[j] == b'_' || self.b[j] >= 0x80)
            {
                j += 1;
            }
        }
        (self.b.get(j) == Some(&b'$')).then_some(j + 1)
    }

    /// `$tag$ … $tag$` 의 끝. 닫히지 않으면 끝까지.
    fn dollar_quoted_end(&self, i: usize, tag_end: usize) -> usize {
        let tag = &self.b[i..tag_end];
        let body = &self.b[tag_end..];
        body.windows(tag.len())
            .position(|w| w == tag)
            .map_or(self.b.len(), |p| tag_end + p + tag.len())
    }

    /// 낱말(키워드 · 식별자 · 수) 하나를 읽고 그 낱말이 바꾸는 상태(구분자 · 블록)를 적용한다.
    fn word(&mut self, i: usize) -> usize {
        let end = self.word_end(i);
        let word = &self.b[i..end];
        // PG `E'…'` 는 백슬래시 이스케이프 문자열이다.
        if self.dialect == SqlDialect::Postgres
            && word.eq_ignore_ascii_case(b"E")
            && self.b.get(end) == Some(&b'\'')
        {
            let end = self.quoted_end(end, b'\'', true);
            return self.mark(i, end);
        }
        let first = self.content.is_none();
        if first
            && self.dialect == SqlDialect::Mysql
            && word.eq_ignore_ascii_case(b"DELIMITER")
            && let Some(line_end) = self.delimiter_command(end)
        {
            return line_end;
        }
        let prev = self.last.clone();
        self.mark(i, end);
        if self.depth == 0 {
            self.word_at_top(word, end, first, prev)
        } else {
            self.word_in_block(word, end, prev)
        }
    }

    /// 낱말의 끝. 바꾼 구분자는 낱말 가운데서도 나눈다(`END$$` — mysql 클라이언트와 같다).
    fn word_end(&self, i: usize) -> usize {
        let custom = self.delimiter != b";";
        let mut j = i + 1;
        while j < self.b.len()
            && is_word_byte(self.b[j])
            && !(custom && self.b[j..].starts_with(&self.delimiter))
        {
            j += 1;
        }
        j
    }

    /// MySQL `DELIMITER <구분자>` 줄. 문장 머리에서 공백이 뒤따를 때만 명령이다. 줄 전체가 문장이 아니므로 줄 끝을 돌려준다.
    fn delimiter_command(&mut self, after: usize) -> Option<usize> {
        if !matches!(self.b.get(after), Some(b' ' | b'\t')) {
            return None;
        }
        let line_end = self.line_end(after);
        let arg = self.b[after..line_end]
            .split(|c| c.is_ascii_whitespace())
            .find(|t| !t.is_empty())
            .map(unquote);
        // 인자가 없으면 mysql 은 오류를 내고 구분자를 그대로 둔다 — 줄만 건너뛴다.
        if let Some(arg) = arg
            && !arg.is_empty()
        {
            self.delimiter = clip_delimiter(arg).to_vec();
        }
        Some(line_end)
    }

    /// 블록 밖의 낱말: 문장 머리를 읽고, 루틴이면 본문 자리의 `BEGIN` 이 블록을 연다.
    fn word_at_top(
        &mut self,
        word: &[u8],
        end: usize,
        first: bool,
        prev: Option<Range<usize>>,
    ) -> usize {
        if first {
            self.head = if eq_any(word, &["CREATE", "ALTER"]) {
                Head::CreateOrAlter
            } else {
                Head::Other
            };
            // MariaDB 의 `BEGIN NOT ATOMIC … END` 복합문.
            if self.dialect == SqlDialect::Mysql
                && word.eq_ignore_ascii_case(b"BEGIN")
                && let Some((not_end, not)) = self.next_word(end)
                && not.eq_ignore_ascii_case(b"NOT")
                && let Some((_, atomic)) = self.next_word(not_end)
                && atomic.eq_ignore_ascii_case(b"ATOMIC")
            {
                self.depth = 1;
            }
            return end;
        }
        match self.head {
            Head::CreateOrAlter if eq_any(word, ROUTINE_OBJECTS) => self.head = Head::Routine,
            Head::CreateOrAlter if eq_any(word, OTHER_OBJECTS) => self.head = Head::Other,
            Head::Routine
                if word.eq_ignore_ascii_case(b"BEGIN") && self.body_begin_at(end, prev) =>
            {
                self.depth = 1;
            }
            _ => {}
        }
        end
    }

    /// 루틴 머리의 `BEGIN`(끝이 `end`, 앞 토큰 `prev`)이 본문을 여는가(머리 주석).
    fn body_begin_at(&self, end: usize, prev: Option<Range<usize>>) -> bool {
        if self.parens > 0 {
            return false;
        }
        if self.dialect == SqlDialect::Postgres {
            return self
                .next_word(end)
                .is_some_and(|(_, w)| w.eq_ignore_ascii_case(b"ATOMIC"))
                && prev.is_none_or(|p| self.b[p.start] != b'.');
        }
        let prev_ok = prev.is_some_and(|p| {
            let tok = &self.b[p.clone()];
            match tok[0] {
                b')' | b':' | b'\'' | b'"' | b'`' => true,
                c if is_word_byte(c) => !eq_any(tok, NOT_BEFORE_BODY),
                _ => false,
            }
        });
        prev_ok
            && self
                .next_word(end)
                .is_some_and(|(_, w)| !eq_any(w, NOT_AFTER_BODY))
    }

    /// 블록 안의 낱말: 문장 머리의 `BEGIN` 과 `CASE` 가 열고 `END` 가 닫는다(머리 주석).
    fn word_in_block(&mut self, word: &[u8], end: usize, prev: Option<Range<usize>>) -> usize {
        if word.eq_ignore_ascii_case(b"CASE")
            || (word.eq_ignore_ascii_case(b"BEGIN") && self.statement_head(prev))
        {
            self.depth += 1;
            return end;
        }
        if !word.eq_ignore_ascii_case(b"END") {
            return end;
        }
        match self.next_word(end) {
            Some((_, next)) if eq_any(next, END_OF_CONTROL) => end,
            // `END CASE` 의 `CASE` 가 다시 열지 않게 함께 넘긴다.
            Some((next_end, next)) if next.eq_ignore_ascii_case(b"CASE") => {
                self.depth -= 1;
                self.mark(end, next_end)
            }
            _ => {
                self.depth -= 1;
                end
            }
        }
    }

    /// 블록 안에서 앞 토큰 `prev` 뒤가 문장 머리인가(괄호 밖이고 `;` · `:` · [`STATEMENT_STARTERS`] 뒤).
    fn statement_head(&self, prev: Option<Range<usize>>) -> bool {
        self.parens == 0
            && prev.is_some_and(|p| {
                let tok = &self.b[p];
                matches!(tok, b";" | b":") || eq_any(tok, STATEMENT_STARTERS)
            })
    }

    /// `i` 뒤 공백 · 주석을 건너 다음 낱말(끝, 낱말). 낱말이 아니면 `None`.
    fn next_word(&self, i: usize) -> Option<(usize, &'a [u8])> {
        let b: &'a [u8] = self.b;
        let start = self.skip_trivia(i);
        if start >= b.len() || !is_word_byte(b[start]) || self.delimiter_at(start).is_some() {
            return None;
        }
        let end = self.word_end(start);
        Some((end, &b[start..end]))
    }

    /// `i` 부터 공백과 (실행 주석이 아닌) 주석을 건넌 자리.
    fn skip_trivia(&self, mut i: usize) -> usize {
        let b = self.b;
        while i < b.len() {
            let next = b.get(i + 1).copied();
            i = match b[i] {
                c if c.is_ascii_whitespace() => i + 1,
                b'-' if next == Some(b'-') && self.dash_comment_at(i) => self.line_end(i),
                b'#' if self.dialect == SqlDialect::Mysql => self.line_end(i),
                b'/' if next == Some(b'*') && self.executable_comment_at(i) == 0 => {
                    match self.block_comment_end(i) {
                        (end, true) => end,
                        _ => return i,
                    }
                }
                _ => return i,
            };
        }
        i
    }
}

/// 낱말을 이루는 바이트. UTF-8 의 비ASCII 바이트는 모두 낱말이다(한글 식별자) — 그래서 토큰 경계는 늘 글자 경계다.
fn is_word_byte(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c == b'$' || c >= 0x80
}

fn eq_any(word: &[u8], keywords: &[&str]) -> bool {
    keywords
        .iter()
        .any(|k| word.eq_ignore_ascii_case(k.as_bytes()))
}

/// `DELIMITER '$$'` 처럼 따옴표로 감싼 인자.
fn unquote(arg: &[u8]) -> &[u8] {
    match arg {
        [q @ (b'\'' | b'"' | b'`'), inner @ .., last] if q == last => inner,
        _ => arg,
    }
}

/// 구분자를 [`MAX_DELIMITER_BYTES`] 로 자른다. 글자 가운데서 자르지 않는다 — 나눈 범위가 늘 글자 경계여야 한다.
fn clip_delimiter(arg: &[u8]) -> &[u8] {
    let mut len = arg.len().min(MAX_DELIMITER_BYTES);
    while len < arg.len() && len > 0 && arg[len] & 0xC0 == 0x80 {
        len -= 1;
    }
    &arg[..len]
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct SqlSplitParams {
    pub dialect: SqlDialect,
    pub sql: String,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct SqlSplitResult {
    /// 문장마다의 범위(UTF-16), 순서대로.
    pub statements: Vec<SqlRange>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct SqlStatementAtParams {
    pub dialect: SqlDialect,
    pub sql: String,
    /// 커서 위치(UTF-16). `sql` 의 길이까지 받는다.
    pub offset: u32,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct SqlStatementRef {
    /// `db.sql.split` 의 몇 번째 문장인가.
    pub index: u32,
    pub range: SqlRange,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct SqlStatementAtResult {
    /// 문장이 없으면(공백 · 주석뿐) `null`.
    pub statement: Option<SqlStatementRef>,
}

rpc_method! {
    /// SQL 스크립트를 문장으로 나눈다(방언별 따옴표 · 주석 · 달러 인용 · `DELIMITER` · 루틴 본문을 지나 구분자로). 범위는
    /// UTF-16 이고 끝의 구분자와 앞뒤 공백 · 주석을 뺀다. 파서가 못 읽는 문장도 나뉜다. `sql` 이 16 MiB 를 넘으면
    /// `INVALID_PARAMS`.
    pub DbSqlSplit: "db.sql.split" {
        params: SqlSplitParams,
        result: SqlSplitResult,
        tier: Read,
    }

    /// 커서(UTF-16 오프셋)의 「현재 문장」 — 빈 줄이 아니라 구분자 기준. 두 문장 사이면 앞 문장, 첫 문장 앞이면 첫 문장.
    /// 오프셋이 `sql` 의 길이를 넘으면 `INVALID_PARAMS`.
    pub DbSqlStatementAt: "db.sql.statementAt" {
        params: SqlStatementAtParams,
        result: SqlStatementAtResult,
        tier: Read,
    }
}

/// RPC 가 받는 SQL 의 상한(바이트). 넘으면 `INVALID_PARAMS` — 범위의 `u32` 가 감기지 않고, 큰 텍스트가 Host 를 붙잡지 않게.
pub const MAX_SQL_BYTES: usize = 16 << 20;

fn check_size(sql: &str) -> Result<(), RpcError> {
    if sql.len() > MAX_SQL_BYTES {
        return Err(RpcError::invalid_params(format!(
            "the SQL is {} bytes, over the {MAX_SQL_BYTES} byte limit",
            sql.len()
        )));
    }
    Ok(())
}

/// 나누기는 작업 스레드 대신 블로킹 스레드에서 돈다(1 MiB 에 수 ms — 그동안 다른 RPC 를 막지 않게).
async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, RpcError> + Send + 'static,
) -> Result<T, RpcError> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| RpcError::internal(e.to_string()))?
}

/// 두 메서드의 핸들러를 붙인다. 상태 없는 계산이다.
pub fn register(router: &mut Router) {
    router
        .unary(DbSqlSplit, |_ctx, p: SqlSplitParams| {
            blocking(move || {
                check_size(&p.sql)?;
                let statements = split(p.dialect, &p.sql)
                    .into_iter()
                    .map(|s| s.range)
                    .collect();
                Ok(SqlSplitResult { statements })
            })
        })
        .unary(DbSqlStatementAt, |_ctx, p: SqlStatementAtParams| {
            blocking(move || {
                check_size(&p.sql)?;
                let len = p.sql.encode_utf16().count();
                if p.offset as usize > len {
                    return Err(RpcError::invalid_params(format!(
                        "offset {} is past the end of the SQL ({len} UTF-16 units)",
                        p.offset
                    )));
                }
                let statement =
                    statement_at(p.dialect, &p.sql, p.offset).map(|(index, s)| SqlStatementRef {
                        index: index as u32,
                        range: s.range,
                    });
                Ok(SqlStatementAtResult { statement })
            })
        });
}

#[cfg(test)]
mod tests;
