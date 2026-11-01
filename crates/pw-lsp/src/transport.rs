//! stdio 의 LSP 틀(base protocol): `Content-Length: N\r\n` 머리들, 빈 줄, 본문 N 바이트.
//!
//! - 머리는 [`MAX_HEADER_BYTES`] 까지(빈 줄 포함). 넘으면 [`FrameError::HeaderTooLarge`].
//! - 본문은 [`MAX_BODY_BYTES`] 까지. `Content-Length` 가 그보다 크면 본문을 읽지 않고 곧바로 [`FrameError::BodyTooLarge`] — 서버가
//!   보낸 수를 믿고 메모리를 잡지 않는다. 부른 쪽(서버 관리)은 그 서버를 죽인다(15-m4-plan.md M4-01 ①).
//! - `Content-Type` 은 받되 charset 이 `utf-8`(옛 이름 `utf8`)이 아니면 거절한다 — LSP 는 UTF-8 만 정한다. 모르는 머리는 무시한다.
//! - 한 번의 읽기에 틀 여럿이 들어와도, 머리가 여러 읽기로 쪼개져 와도 같은 결과다(남은 바이트는 다음 틀의 앞이다).

use std::fmt;
use std::io;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// 머리 부분(마지막 빈 줄의 `\r\n\r\n` 포함)의 상한.
pub const MAX_HEADER_BYTES: usize = 8 * 1024;
/// 본문 하나의 상한.
pub const MAX_BODY_BYTES: usize = 64 * 1024 * 1024;

const TERMINATOR: &[u8] = b"\r\n\r\n";
/// 한 번에 더 읽을 크기의 바닥.
const READ_CHUNK: usize = 64 * 1024;
/// 틀을 낸 뒤 버퍼가 이보다 크면 줄인다.
const SHRINK_ABOVE: usize = 1024 * 1024;

#[derive(Debug)]
pub enum FrameError {
    /// 머리가 [`MAX_HEADER_BYTES`] 를 넘었다.
    HeaderTooLarge,
    /// `Content-Length` 가 [`MAX_BODY_BYTES`] 를 넘었다(값).
    BodyTooLarge(u64),
    /// `Content-Length` 가 없다.
    MissingContentLength,
    /// 머리 형식이 틀렸다(줄 · 값 · 지원하지 않는 charset).
    BadHeader(String),
    /// 틀 중간에 스트림이 끝났다.
    Truncated,
    Io(io::Error),
}

impl fmt::Display for FrameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::HeaderTooLarge => write!(f, "header is larger than {MAX_HEADER_BYTES} bytes"),
            Self::BodyTooLarge(n) => {
                write!(f, "body of {n} bytes is larger than {MAX_BODY_BYTES} bytes")
            }
            Self::MissingContentLength => f.write_str("no Content-Length header"),
            Self::BadHeader(detail) => write!(f, "bad header: {detail}"),
            Self::Truncated => f.write_str("the stream ended inside a message"),
            Self::Io(e) => write!(f, "read failed: {e}"),
        }
    }
}

impl std::error::Error for FrameError {}

/// 스트림에서 틀을 하나씩 읽는다. 다 쓰지 않은 바이트는 다음 틀을 위해 남긴다.
pub struct FrameReader<R> {
    inner: R,
    buf: Vec<u8>,
}

impl<R: AsyncRead + Unpin> FrameReader<R> {
    pub fn new(inner: R) -> Self {
        Self {
            inner,
            buf: Vec::new(),
        }
    }

    /// 다음 틀의 본문. 틀 사이에서 스트림이 끝나면 `None`.
    pub async fn read_frame(&mut self) -> Result<Option<Vec<u8>>, FrameError> {
        let header_end = loop {
            let window = &self.buf[..self.buf.len().min(MAX_HEADER_BYTES)];
            if let Some(at) = find(window, TERMINATOR) {
                break at + TERMINATOR.len();
            }
            if self.buf.len() >= MAX_HEADER_BYTES {
                return Err(FrameError::HeaderTooLarge);
            }
            if self.fill(MAX_HEADER_BYTES - self.buf.len()).await? == 0 {
                return if self.buf.is_empty() {
                    Ok(None)
                } else {
                    Err(FrameError::Truncated)
                };
            }
        };
        let length = content_length(&self.buf[..header_end - TERMINATOR.len()])?;
        let total = header_end + length;
        while self.buf.len() < total {
            if self.fill(total - self.buf.len()).await? == 0 {
                return Err(FrameError::Truncated);
            }
        }
        let body = self.buf[header_end..total].to_vec();
        self.buf.drain(..total);
        // 큰 틀 하나 뒤에 그 크기의 버퍼를 서버가 끝날 때까지 쥐지 않는다(M4-01 리뷰 L3).
        if self.buf.capacity() > SHRINK_ABOVE && self.buf.len() <= READ_CHUNK {
            self.buf.shrink_to(READ_CHUNK);
        }
        Ok(Some(body))
    }

    /// 적어도 `want` 바이트를 담을 자리를 잡고 한 번 읽는다. 읽은 바이트 수(0 이면 끝).
    async fn fill(&mut self, want: usize) -> Result<usize, FrameError> {
        self.buf.reserve(want.clamp(READ_CHUNK, MAX_BODY_BYTES));
        self.inner
            .read_buf(&mut self.buf)
            .await
            .map_err(FrameError::Io)
    }
}

/// 머리 줄들(마지막 빈 줄 없이)에서 본문 길이.
fn content_length(header: &[u8]) -> Result<usize, FrameError> {
    let text = std::str::from_utf8(header)
        .map_err(|_| FrameError::BadHeader("header is not ASCII".to_owned()))?;
    let mut length: Option<u64> = None;
    for line in text.split("\r\n") {
        let (name, value) = line
            .split_once(':')
            .ok_or_else(|| FrameError::BadHeader(format!("line without a colon: {line:?}")))?;
        let value = value.trim();
        if name.eq_ignore_ascii_case("Content-Length") {
            if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
                return Err(FrameError::BadHeader(format!(
                    "Content-Length is not a number: {value:?}"
                )));
            }
            // 자릿수가 많아 u64 를 넘으면 상한을 넘은 것으로 본다.
            let n = value.parse::<u64>().unwrap_or(u64::MAX);
            if length.is_some_and(|l| l != n) {
                return Err(FrameError::BadHeader(
                    "two different Content-Length headers".to_owned(),
                ));
            }
            length = Some(n);
        } else if name.eq_ignore_ascii_case("Content-Type") {
            check_content_type(value)?;
        }
    }
    let n = length.ok_or(FrameError::MissingContentLength)?;
    if n > MAX_BODY_BYTES as u64 {
        return Err(FrameError::BodyTooLarge(n));
    }
    usize::try_from(n).map_err(|_| FrameError::BodyTooLarge(n))
}

fn check_content_type(value: &str) -> Result<(), FrameError> {
    for param in value.split(';').skip(1) {
        let Some((key, charset)) = param.split_once('=') else {
            continue;
        };
        if key.trim().eq_ignore_ascii_case("charset") {
            let charset = charset.trim().trim_matches('"');
            if !(charset.eq_ignore_ascii_case("utf-8") || charset.eq_ignore_ascii_case("utf8")) {
                return Err(FrameError::BadHeader(format!(
                    "unsupported charset {charset:?}"
                )));
            }
        }
    }
    Ok(())
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// 본문 하나를 틀로 써서 내보낸다.
pub async fn write_frame<W: AsyncWrite + Unpin>(writer: &mut W, body: &[u8]) -> io::Result<()> {
    let header = format!("Content-Length: {}\r\n\r\n", body.len());
    writer.write_all(header.as_bytes()).await?;
    writer.write_all(body).await?;
    writer.flush().await
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::pin::Pin;
    use std::task::{Context, Poll};

    use tokio::io::ReadBuf;

    use super::*;

    /// 읽을 때마다 정해 둔 조각 하나씩 주는 스트림.
    struct Chunks(VecDeque<Vec<u8>>);

    impl Chunks {
        fn new<I: IntoIterator<Item = Vec<u8>>>(chunks: I) -> Self {
            Self(chunks.into_iter().collect())
        }
    }

    impl AsyncRead for Chunks {
        fn poll_read(
            mut self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            if let Some(mut chunk) = self.0.pop_front() {
                let n = chunk.len().min(buf.remaining());
                buf.put_slice(&chunk[..n]);
                if n < chunk.len() {
                    self.0.push_front(chunk.split_off(n));
                }
            }
            Poll::Ready(Ok(()))
        }
    }

    fn frame(body: &str) -> Vec<u8> {
        format!("Content-Length: {}\r\n\r\n{body}", body.len()).into_bytes()
    }

    async fn read_all(chunks: Vec<Vec<u8>>) -> Result<Vec<String>, FrameError> {
        let mut reader = FrameReader::new(Chunks::new(chunks));
        let mut out = Vec::new();
        while let Some(body) = reader.read_frame().await? {
            out.push(String::from_utf8(body).unwrap());
        }
        Ok(out)
    }

    #[tokio::test]
    async fn a_header_split_over_reads_still_reads_one_frame() {
        let whole = frame(r#"{"jsonrpc":"2.0","method":"x"}"#);
        // 머리 · 빈 줄 · 본문을 한 바이트씩까지 쪼갠다.
        for cut in [1, 5, 16, 17, 18, 19, 20, 21] {
            let (a, b) = whole.split_at(cut);
            assert_eq!(
                read_all(vec![a.to_vec(), b.to_vec()]).await.unwrap(),
                vec![r#"{"jsonrpc":"2.0","method":"x"}"#],
                "cut at {cut}"
            );
        }
        let bytes = whole.iter().map(|b| vec![*b]).collect();
        assert_eq!(read_all(bytes).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn two_messages_in_one_read_are_two_frames() {
        let mut both = frame("{\"a\":1}");
        both.extend(frame("{\"b\":2}"));
        assert_eq!(
            read_all(vec![both]).await.unwrap(),
            vec!["{\"a\":1}", "{\"b\":2}"]
        );
    }

    #[tokio::test]
    async fn a_content_type_header_is_accepted_in_either_order() {
        let body = "{}";
        let after = format!(
            "Content-Length: 2\r\nContent-Type: application/vscode-jsonrpc; charset=utf-8\r\n\r\n{body}"
        );
        let before = format!(
            "content-type: application/vscode-jsonrpc; charset=utf8\r\ncontent-length: 2\r\n\r\n{body}"
        );
        assert_eq!(
            read_all(vec![after.into_bytes()]).await.unwrap(),
            vec!["{}"]
        );
        assert_eq!(
            read_all(vec![before.into_bytes()]).await.unwrap(),
            vec!["{}"]
        );
        let latin = "Content-Length: 2\r\nContent-Type: x; charset=latin1\r\n\r\n{}";
        assert!(matches!(
            read_all(vec![latin.as_bytes().to_vec()]).await,
            Err(FrameError::BadHeader(_))
        ));
    }

    #[tokio::test]
    async fn a_body_one_byte_over_the_limit_is_refused_without_reading_it() {
        let over = MAX_BODY_BYTES as u64 + 1;
        let header = format!("Content-Length: {over}\r\n\r\n");
        // 본문은 보내지 않는다 — 읽으려 들면 `Truncated` 가 나온다.
        match read_all(vec![header.into_bytes()]).await {
            Err(FrameError::BodyTooLarge(n)) => assert_eq!(n, over),
            other => panic!("expected BodyTooLarge, got {other:?}"),
        }
        let at = format!("Content-Length: {MAX_BODY_BYTES}\r\n\r\n");
        assert!(matches!(
            read_all(vec![at.into_bytes()]).await,
            Err(FrameError::Truncated)
        ));
    }

    #[tokio::test]
    async fn a_header_over_8_kib_is_refused() {
        let long = format!("X-Pad: {}\r\n", "a".repeat(MAX_HEADER_BYTES));
        assert!(matches!(
            read_all(vec![long.into_bytes()]).await,
            Err(FrameError::HeaderTooLarge)
        ));
        // 상한 바로 안쪽은 읽는다.
        let pad = MAX_HEADER_BYTES - "Content-Length: 2\r\nX: \r\n\r\n".len();
        let fits = format!("Content-Length: 2\r\nX: {}\r\n\r\n{{}}", "a".repeat(pad));
        assert_eq!(read_all(vec![fits.into_bytes()]).await.unwrap(), vec!["{}"]);
    }

    #[tokio::test]
    async fn malformed_headers_and_cut_streams_are_errors() {
        for (bytes, want) in [
            ("Content-Length: 2\r\n\r\n{", "Truncated"),
            ("Content-Len", "Truncated"),
            ("X-Only: 1\r\n\r\n", "MissingContentLength"),
            ("Content-Length: -1\r\n\r\n", "BadHeader"),
            ("Content-Length: 2\r\nnocolon\r\n\r\n{}", "BadHeader"),
            (
                "Content-Length: 2\r\nContent-Length: 3\r\n\r\n{}",
                "BadHeader",
            ),
        ] {
            let got = read_all(vec![bytes.as_bytes().to_vec()]).await;
            let name = match got {
                Err(FrameError::Truncated) => "Truncated",
                Err(FrameError::MissingContentLength) => "MissingContentLength",
                Err(FrameError::BadHeader(_)) => "BadHeader",
                other => panic!("{bytes:?}: {other:?}"),
            };
            assert_eq!(name, want, "{bytes:?}");
        }
        assert_eq!(read_all(vec![]).await.unwrap(), Vec::<String>::new());
    }

    /// 리뷰 L3: 큰 틀 하나를 읽은 뒤 버퍼를 줄인다.
    #[tokio::test]
    async fn the_buffer_shrinks_after_a_big_frame() {
        let big = format!("{{\"x\":\"{}\"}}", "a".repeat(8 * 1024 * 1024));
        let mut bytes = frame(&big);
        bytes.extend(frame("{}"));
        let mut reader = FrameReader::new(Chunks::new([bytes]));
        assert_eq!(reader.read_frame().await.unwrap().unwrap().len(), big.len());
        assert!(
            reader.buf.capacity() <= 2 * READ_CHUNK,
            "{}",
            reader.buf.capacity()
        );
        assert_eq!(reader.read_frame().await.unwrap().unwrap(), b"{}");
    }

    #[tokio::test]
    async fn written_frames_read_back() {
        let mut out = Vec::new();
        write_frame(&mut out, "{\"한\":1}".as_bytes())
            .await
            .unwrap();
        write_frame(&mut out, b"{}").await.unwrap();
        assert_eq!(read_all(vec![out]).await.unwrap(), vec!["{\"한\":1}", "{}"]);
    }
}
