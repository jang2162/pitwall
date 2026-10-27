//! 커밋 하나를 행 · 필터 판정에 쓸 글자로 읽는다. git log 와 같게:
//!
//! - 메시지 · 작성자는 커밋의 `encoding` 헤더(없으면 UTF-8)에서 UTF-8 로 바꾼다(git 의 `i18n.logOutputEncoding` 기본값). 바꿀 수 없는
//!   바이트는 `U+FFFD` 다.
//! - 작성자는 mailmap 으로 바꾼 뒤 보이고 `--author` 에 맞춘다(git 의 `log.mailmap` 기본값 `true`). mailmap 은 작업 트리의
//!   `.mailmap` 파일이다 — 파일 읽기뿐이라 미신뢰 저장소에서도 읽는다(링크는 따라가지 않는다, git 과 같다). 설정의 `mailmap.file` ·
//!   `mailmap.blob` 은 신뢰한 저장소에서만 본다.
//! - 행에 싣는 제목 · 이름 · 메일은 길이를 자른다(필터는 자르기 전 글자에 맞춘다). 전체는 커밋 상세가 따로 읽는다.

use std::borrow::Cow;

use gix::bstr::{BStr, ByteSlice};
use gix::objs::CommitRef;
use pw_rpc::RpcError;

/// 행의 제목 상한(글자).
pub const MAX_SUBJECT: usize = 1024;
/// 행의 작성자 이름 · 메일 상한(글자).
pub const MAX_IDENT: usize = 256;
/// 미신뢰 저장소에서 읽는 `.mailmap` 크기 상한. 넘으면 mailmap 없이 보인다.
const MAX_MAILMAP: u64 = 16 * 1024 * 1024;

/// 저장소의 mailmap. 없거나 꺼져 있으면 비어 있다.
pub struct Mailmap(Option<gix::mailmap::Snapshot>);

impl Mailmap {
    /// `repo` 의 mailmap(머리 주석). `trusted` 가 아니면 작업 트리의 `.mailmap` 만 읽는다.
    pub fn of(repo: &gix::Repository, trusted: bool) -> Self {
        if repo.config_snapshot().boolean("log.mailmap") == Some(false) {
            return Self(None);
        }
        let snapshot = if trusted {
            repo.open_mailmap()
        } else {
            let Some(path) = repo.workdir().map(|w| w.join(".mailmap")) else {
                return Self(None);
            };
            match std::fs::symlink_metadata(&path) {
                Ok(meta) if meta.is_file() && meta.len() <= MAX_MAILMAP => {
                    match std::fs::read(&path) {
                        Ok(bytes) => gix::mailmap::Snapshot::from_bytes(&bytes),
                        Err(e) => {
                            tracing::debug!(target: "pw_git", path = %path.display(), %e, "cannot read .mailmap");
                            return Self(None);
                        }
                    }
                }
                _ => return Self(None),
            }
        };
        Self((!snapshot.entries().is_empty()).then_some(snapshot))
    }
}

/// 읽은 커밋의 글자(UTF-8, mailmap 적용, 자르기 전).
pub struct Text {
    pub author_name: String,
    pub author_email: String,
    /// 작성 시각(유닉스 초).
    pub author_time: i64,
    /// 메시지 전체.
    pub message: String,
}

impl Text {
    pub fn read(commit: &CommitRef<'_>, mailmap: &Mailmap) -> Result<Self, RpcError> {
        let author = commit
            .author()
            .map_err(|e| RpcError::internal(e.to_string()))?;
        let encoding = commit.encoding;
        let name = decode(author.name.trim().as_bstr(), encoding);
        let email = decode(author.email.trim().as_bstr(), encoding);
        let (name, email) = match &mailmap.0 {
            Some(map) => {
                let resolved = map.resolve_cow(gix::actor::SignatureRef {
                    name: name.as_bytes().as_bstr(),
                    email: email.as_bytes().as_bstr(),
                    time: author.time,
                });
                (
                    resolved.name.to_str_lossy().into_owned(),
                    resolved.email.to_str_lossy().into_owned(),
                )
            }
            None => (name.into_owned(), email.into_owned()),
        };
        Ok(Self {
            author_name: name,
            author_email: email,
            author_time: author.seconds(),
            message: decode(commit.message, encoding).into_owned(),
        })
    }

    /// `--author` 가 맞추는 글자(「이름 <메일>」).
    pub fn ident(&self) -> String {
        format!("{} <{}>", self.author_name, self.author_email)
    }
}

/// 메시지 첫 문단을 한 줄로(git `%s`) — [`MAX_SUBJECT`] 글자에서 자른다.
pub fn subject(commit: &CommitRef<'_>) -> String {
    clip(
        &decode(commit.message().summary().as_ref(), commit.encoding),
        MAX_SUBJECT,
    )
}

/// `max` 글자를 넘으면 그 앞에서 자르고 `…` 를 붙인다.
pub fn clip(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((end, _)) => format!("{}…", &text[..end]),
        None => text.to_owned(),
    }
}

/// `encoding` 헤더의 인코딩에서 UTF-8 로. 헤더가 없거나 UTF-8 이면 그대로(잘못된 바이트는 `U+FFFD`), 모르는 인코딩도 그대로 둔다
/// (git 도 바꾸지 못하면 바이트를 그대로 낸다).
pub fn decode<'a>(bytes: &'a BStr, encoding: Option<&BStr>) -> Cow<'a, str> {
    let Some(label) = encoding.and_then(|e| e.to_str().ok()).map(str::trim) else {
        return bytes.to_str_lossy();
    };
    let key: String = label
        .chars()
        .filter(|c| *c != '-' && *c != '_')
        .map(|c| c.to_ascii_lowercase())
        .collect();
    match key.as_str() {
        "utf8" => bytes.to_str_lossy(),
        // ISO-8859-1 은 바이트 = 코드 포인트다. `encoding_rs` 는 이 이름을 windows-1252 로 읽어(WHATWG) 0x80~0x9F 가 git(iconv)과
        // 달라지므로 따로 푼다.
        "iso88591" | "iso885911987" | "latin1" | "l1" => {
            Cow::Owned(bytes.iter().map(|&b| char::from(b)).collect())
        }
        _ => match encoding_rs::Encoding::for_label(label.as_bytes()) {
            Some(enc) if enc != encoding_rs::UTF_8 => enc.decode_without_bom_handling(bytes).0,
            _ => bytes.to_str_lossy(),
        },
    }
}
