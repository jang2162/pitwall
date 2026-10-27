//! `git blame --incremental -M --no-textconv <커밋> -- <경로>`([`ARGS`])의 출력을 받는 대로 범위로 푼다(신뢰한 프로젝트 — [`super`] 머리 주석).
//!
//! 출력은 범위마다 `<해시> <원래 줄> <지금 줄> <줄 수>` 한 줄로 시작하고 `filename <경로>` 로 끝난다. 그 사이에 그 커밋이 처음
//! 나올 때만 작성자 · 시각 · 제목(`author` … `summary`) · `boundary` 가, 그 범위의 원래 파일에 부모가 있으면 `previous <해시>
//! <경로>` 가 온다. 경로는 git 의 따옴표 규칙(`core.quotePath`)으로 감쌀 수 있어 푼다. 모르는 키는 건너뛴다(git 이 더해도 깨지지
//! 않는다).

use gix::bstr::{BString, ByteSlice};
use pw_rpc::RpcError;
use tokio::io::{AsyncRead, AsyncReadExt};

use super::{
    BlameChunk, BlameCommit, BlameFile, BlamePrevious, BlameRange, Chunker, Subject, path_text,
};
use crate::cli::GitCommand;
use crate::log::commit::{MAX_IDENT, MAX_SUBJECT, clip};

/// 한 줄 상한(바이트). 넘는 부분은 버린다 — 제목이 아주 긴 커밋이 메모리를 채우지 않게(잘린 제목은 [`MAX_SUBJECT`] 로 어차피
/// 자른다).
const MAX_LINE: usize = 64 * 1024;

/// `git blame` 의 인자(커밋 · `--` · 경로 앞). `--no-textconv`: git blame 은 기본으로 저장소 설정의 textconv 를 돌려 그 출력의 줄을
/// blame 한다 — 줄이 커밋의 내용(에디터 · [`super::BlameFile::lines`])과 어긋나고, textconv 는 신뢰해도 쓰지 않는다(M2-04 의
/// `crate::diff` 와 같다).
pub const ARGS: [&str; 4] = ["blame", "--incremental", "-M", "--no-textconv"];

/// `subject` 를 blame 하는 git 명령(신뢰한 프로젝트의 읽기 명령에 인자를 붙인다).
pub fn command(git: GitCommand, subject: &Subject) -> GitCommand {
    git.args(ARGS)
        .arg(subject.commit.to_string())
        .arg("--")
        // 경로는 RPC 의 글자(UTF-8)에서 왔다.
        .arg(subject.path.to_str_lossy().into_owned())
}

/// 해석한 것 하나.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// 커밋이 처음 나왔다.
    Commit(BlameCommit),
    Range(BlameRange),
}

/// 범위 하나를 읽는 중.
#[derive(Debug, Default)]
struct Pending {
    hash: String,
    source_line: u32,
    line: u32,
    count: u32,
    meta: Option<BlameCommit>,
    previous: Option<BlamePrevious>,
}

/// 줄 단위 해석기. 바이트를 받는 대로 [`Parser::feed`] 하면 다 읽은 범위를 내준다.
#[derive(Debug, Default)]
pub struct Parser {
    line: Vec<u8>,
    /// 지금 줄이 [`MAX_LINE`] 을 넘었다 — 줄 끝까지 버린다.
    overflow: bool,
    pending: Option<Pending>,
}

impl Parser {
    pub fn feed(&mut self, bytes: &[u8], out: &mut Vec<Event>) -> Result<(), RpcError> {
        for piece in bytes.split_inclusive(|&b| b == b'\n') {
            let (body, ended) = match piece.strip_suffix(b"\n") {
                Some(body) => (body, true),
                None => (piece, false),
            };
            if !self.overflow {
                let room = MAX_LINE.saturating_sub(self.line.len());
                self.line.extend_from_slice(&body[..body.len().min(room)]);
                self.overflow = body.len() > room;
            }
            if ended {
                let line = std::mem::take(&mut self.line);
                self.overflow = false;
                self.line_done(&line, out)?;
            }
        }
        Ok(())
    }

    /// 쥐고 있는 미완성 줄의 바이트 수(시험 — [`MAX_LINE`] 상한을 본다).
    #[cfg(test)]
    pub fn buffered(&self) -> usize {
        self.line.len()
    }

    /// 출력이 끝났다. 읽다 만 범위가 있으면 오류다.
    pub fn finish(&self) -> Result<(), RpcError> {
        if self.pending.is_some() || !self.line.is_empty() {
            return Err(bad("output ended in the middle of an entry"));
        }
        Ok(())
    }

    fn line_done(&mut self, line: &[u8], out: &mut Vec<Event>) -> Result<(), RpcError> {
        let Some(pending) = &mut self.pending else {
            self.pending = Some(header(line)?);
            return Ok(());
        };
        let (key, value) = match line.find_byte(b' ') {
            Some(i) => (&line[..i], &line[i + 1..]),
            None => (line, &b""[..]),
        };
        match key {
            b"author" => meta(pending).author_name = clip(&value.to_str_lossy(), MAX_IDENT),
            b"author-mail" => {
                let mail = value.strip_prefix(b"<").unwrap_or(value);
                let mail = mail.strip_suffix(b">").unwrap_or(mail);
                meta(pending).author_email = clip(&mail.to_str_lossy(), MAX_IDENT);
            }
            b"author-time" => meta(pending).author_time = number(value)?,
            b"summary" => meta(pending).subject = clip(&value.to_str_lossy(), MAX_SUBJECT),
            b"boundary" => meta(pending).boundary = true,
            b"previous" => {
                let (hash, path) = value
                    .split_once_str(" ")
                    .ok_or_else(|| bad("`previous` without a path"))?;
                pending.previous = Some(BlamePrevious {
                    commit: hash_text(hash)?,
                    path: path_text(unquote(path).as_bstr()),
                });
            }
            b"filename" => {
                let Some(pending) = self.pending.take() else {
                    unreachable!("matched above");
                };
                if let Some(meta) = pending.meta {
                    out.push(Event::Commit(meta));
                }
                out.push(Event::Range(BlameRange {
                    line: pending.line,
                    count: pending.count,
                    commit: pending.hash,
                    source_path: path_text(unquote(value).as_bstr()),
                    source_line: pending.source_line,
                    previous: pending.previous,
                }));
            }
            _ => {}
        }
        Ok(())
    }
}

/// 읽는 범위의 커밋 정보(처음 나온 커밋이라 실린다).
fn meta(pending: &mut Pending) -> &mut BlameCommit {
    let hash = pending.hash.clone();
    pending.meta.get_or_insert_with(|| BlameCommit {
        hash,
        author_name: String::new(),
        author_email: String::new(),
        author_time: 0,
        subject: String::new(),
        boundary: false,
    })
}

/// `<해시> <원래 줄> <지금 줄> <줄 수>`.
fn header(line: &[u8]) -> Result<Pending, RpcError> {
    let mut parts = line.split(|&b| b == b' ');
    let mut next = || parts.next().ok_or_else(|| bad("short entry header"));
    let hash = hash_text(next()?)?;
    let source_line = number(next()?)?;
    let line = number(next()?)?;
    let count = number(next()?)?;
    if source_line == 0 || line == 0 || count == 0 {
        return Err(bad("line numbers start at 1"));
    }
    Ok(Pending {
        hash,
        source_line,
        line,
        count,
        ..Pending::default()
    })
}

fn hash_text(bytes: &[u8]) -> Result<String, RpcError> {
    gix::ObjectId::from_hex(bytes)
        .map(|id| id.to_string())
        .map_err(|_| bad("bad commit hash"))
}

fn number<T: std::str::FromStr>(bytes: &[u8]) -> Result<T, RpcError> {
    bytes
        .to_str()
        .ok()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| bad("bad number"))
}

fn bad(why: &str) -> RpcError {
    RpcError::internal(format!(
        "unexpected `git blame --incremental` output: {why}"
    ))
}

/// git 의 따옴표 규칙(`quote_c_style`)으로 감싼 경로를 푼다. 따옴표가 없으면 그대로.
pub fn unquote(s: &[u8]) -> BString {
    let Some(inner) = s
        .strip_prefix(b"\"")
        .and_then(|rest| rest.strip_suffix(b"\""))
    else {
        return s.into();
    };
    let mut out = Vec::with_capacity(inner.len());
    let mut i = 0;
    while i < inner.len() {
        let b = inner[i];
        i += 1;
        if b != b'\\' {
            out.push(b);
            continue;
        }
        let Some(&c) = inner.get(i) else {
            out.push(b'\\');
            break;
        };
        i += 1;
        let decoded = match c {
            b'a' => 0x07,
            b'b' => 0x08,
            b't' => b'\t',
            b'n' => b'\n',
            b'v' => 0x0b,
            b'f' => 0x0c,
            b'r' => b'\r',
            b'0'..=b'3'
                if inner
                    .get(i..i + 2)
                    .is_some_and(|d| d.iter().all(|d| (b'0'..=b'7').contains(d))) =>
            {
                let value = (c - b'0') * 64 + (inner[i] - b'0') * 8 + (inner[i + 1] - b'0');
                i += 2;
                value
            }
            other => other,
        };
        out.push(decoded);
    }
    out.into()
}

/// 청크를 받는 쪽.
pub trait Emit: Send {
    /// 받는 쪽이 사라졌으면 `false`.
    fn emit(&mut self, chunk: BlameChunk) -> impl Future<Output = bool> + Send;
}

/// 다 읽은 출력의 남은 것. git 의 종료 코드를 본 뒤에 [`Drained::finish`] 한다 — git 이 실패했으면 그 오류가 먼저다.
pub struct Drained {
    parser: Parser,
    chunker: Chunker,
}

impl Drained {
    /// 읽다 만 범위가 있으면 오류. 아니면 남은 청크(아무것도 보내지 않았으면 범위 없는 첫 청크).
    pub fn finish(self) -> Result<Option<BlameChunk>, RpcError> {
        self.parser.finish()?;
        Ok(self.chunker.finish())
    }
}

/// `stdout`(git 의 출력)을 끝까지 읽어 청크로 `emit` 한다. `emit` 이 `false` 를 돌려주면(받는 쪽이 사라짐) 멈추고 `Ok(None)`.
/// 다 읽었으면 남은 것을 돌려준다.
pub async fn pump(
    mut stdout: impl AsyncRead + Unpin,
    file: BlameFile,
    emit: &mut impl Emit,
) -> Result<Option<Drained>, RpcError> {
    let mut parser = Parser::default();
    let mut chunker = Chunker::new(file);
    let mut buf = vec![0u8; 64 * 1024];
    let mut events = Vec::new();
    loop {
        let read = match chunker.wait() {
            Some(wait) => match tokio::time::timeout(wait, stdout.read(&mut buf)).await {
                Ok(read) => read,
                Err(_) => {
                    if !emit.emit(chunker.take()).await {
                        return Ok(None);
                    }
                    continue;
                }
            },
            None => stdout.read(&mut buf).await,
        };
        let n = read.map_err(|e| RpcError::internal(format!("cannot read git blame: {e}")))?;
        if n == 0 {
            return Ok(Some(Drained { parser, chunker }));
        }
        parser.feed(&buf[..n], &mut events)?;
        for event in events.drain(..) {
            match event {
                Event::Commit(commit) => chunker.commit(commit),
                Event::Range(range) => chunker.range(range),
            }
        }
        if chunker.due() && !emit.emit(chunker.take()).await {
            return Ok(None);
        }
    }
}
