//! 로그 필터(F5-2) 판정. 브랜치 필터(시작 ref)는 걷기가 하고, 여기는 그 밖의 필터다 — 켜지면 그래프 없이 맞는 커밋만 낸다
//! (결정 M2-D2).
//!
//! 각 필터는 같은 옵션의 `git log` 와 같은 커밋을 고른다:
//!
//! | 필터 | git |
//! |---|---|
//! | `author` | `--author=<패턴>` — mailmap 을 적용한 「이름 <메일>」 에 맞춘다([`super::commit`]) |
//! | `message` | `--grep=<패턴>` — 메시지(제목 · 본문)의 줄마다 맞춘다 |
//! | `regex` | 켜면 `-E`(정규식), 끄면 `-F`(글자 그대로). `author` · `message` 둘 다 |
//! | `ignoreCase` | `-i` |
//! | `since` · `until` | `--since` · `--until`(커미터 시각, 둘 다 경계 포함). `since` 는 기준보다 오래된 커밋의 조상도 뺀다([`super::walk::before`]) |
//! | `paths` | `-- <경로>…` — 이력 단순화는 `walk` 가 한다 |
//! | `hash` | 해시 접두(git 에는 같은 옵션이 없다) |
//!
//! 여럿이면 모두 맞아야 한다(git 도 `--author` 와 `--grep` 을 AND 로 묶는다). 작성자 · 메시지는 커밋의 `encoding` 헤더에서 UTF-8 로
//! 바꾼 글자에 맞춘다(git 과 같다). 정규식 문법은 Rust `regex` 다 — POSIX ERE 와 흔한 문법은 같고, 역참조 · POSIX 문자 클래스 이름
//! 같은 것은 다르다. 경로는 글자 그대로의 파일 · 폴더다 — git pathspec 의 글롭(`dir/*`)은 받지 않는다.
//!
//! 시각 필터의 순서: 거르기 전 전체 이력의 순서에서 맞는 커밋만 남긴다. git 은 commit-graph 가 있을 때 이렇게 하고, 없을 때는
//! `--until` 로 빠진 커밋을 순서 계산에서도 빼서 머지 위를 자르면 순서가 달라진다(같은 git 이 commit-graph 유무로 순서가 다르다).

use gix::ObjectId;
use pw_rpc::RpcError;
use regex::{Regex, RegexBuilder};

use super::LogFilter;

/// 판정할 수 있게 다듬은 필터.
#[derive(Debug, Clone)]
pub struct Filter {
    author: Option<Regex>,
    message: Option<Regex>,
    since: Option<i64>,
    until: Option<i64>,
    /// 소문자 16진.
    hash: Option<String>,
    /// `/` 로 나눈 상대 경로.
    pub paths: Vec<gix::bstr::BString>,
}

/// 패턴 · 경로 · 해시의 길이 상한.
const MAX_PATTERN: usize = 4096;

impl Filter {
    pub fn new(f: &LogFilter) -> Result<Self, RpcError> {
        let pattern = |name: &str, p: &Option<String>| -> Result<Option<Regex>, RpcError> {
            let Some(p) = p.as_deref() else {
                return Ok(None);
            };
            if p.is_empty() {
                return Err(RpcError::invalid_params(format!("`{name}` is empty")));
            }
            if p.len() > MAX_PATTERN {
                return Err(RpcError::invalid_params(format!(
                    "`{name}` is longer than {MAX_PATTERN} bytes"
                )));
            }
            let source = if f.regex {
                p.to_owned()
            } else {
                regex::escape(p)
            };
            RegexBuilder::new(&source)
                .case_insensitive(f.ignore_case)
                .build()
                .map(Some)
                .map_err(|e| {
                    RpcError::invalid_params(format!("`{name}` is not a valid regex: {e}"))
                })
        };
        let hash = match f.hash.as_deref() {
            None => None,
            Some(h)
                if h.is_empty() || h.len() > 64 || !h.bytes().all(|b| b.is_ascii_hexdigit()) =>
            {
                return Err(RpcError::invalid_params(format!(
                    "`hash` must be 1 to 64 hex digits: `{h}`"
                )));
            }
            Some(h) => Some(h.to_ascii_lowercase()),
        };
        let mut paths = Vec::with_capacity(f.paths.len());
        for p in &f.paths {
            paths.push(relative_path(p)?);
        }
        Ok(Self {
            author: pattern("author", &f.author)?,
            message: pattern("message", &f.message)?,
            since: f.since,
            until: f.until,
            hash,
            paths,
        })
    }

    /// 그래프를 그릴 수 있는가 — 브랜치 필터 밖의 필터가 하나도 없다.
    pub fn keeps_graph(&self) -> bool {
        self.author.is_none()
            && self.message.is_none()
            && self.since.is_none()
            && self.until.is_none()
            && self.hash.is_none()
            && self.paths.is_empty()
    }

    /// 커밋을 읽지 않고 볼 수 있는 것(커미터 시각 · 해시).
    pub fn matches_cheaply(&self, id: &ObjectId, committer_time: i64) -> bool {
        if self.since.is_some_and(|s| committer_time < s)
            || self.until.is_some_and(|u| committer_time > u)
        {
            return false;
        }
        match &self.hash {
            Some(prefix) => id.to_hex().to_string().starts_with(prefix.as_str()),
            None => true,
        }
    }

    /// `since`(커미터 시각, 유닉스 초).
    pub fn since(&self) -> Option<i64> {
        self.since
    }

    /// 커밋 내용을 봐야 하는 것이 있는가.
    pub fn needs_commit(&self) -> bool {
        self.author.is_some() || self.message.is_some()
    }

    /// 작성자(「이름 <메일>」) · 메시지.
    pub fn matches_commit(&self, author: &str, message: &str) -> bool {
        self.author.as_ref().is_none_or(|re| re.is_match(author))
            && self
                .message
                .as_ref()
                .is_none_or(|re| message.lines().any(|line| re.is_match(line)))
    }
}

/// 저장소 작업 트리 기준 상대 경로(git RPC 의 경로 꼴 — 구분자 `/`)를 구성 요소로 다듬는다. Windows 에서는 `\` 도 구분자로
/// 받아 `/` 로 바꾼다(그곳 파일 이름에는 `\` 가 올 수 없다). 다른 OS 에서 `\` 는 이름의 일부다. 절대 경로 · 빈 경로 · `.` ·
/// `..` 는 거절한다.
pub(super) fn relative_path(p: &str) -> Result<gix::bstr::BString, RpcError> {
    let bad = |why: &str| RpcError::invalid_params(format!("path `{p}` {why}"));
    if p.len() > MAX_PATTERN {
        return Err(bad("is too long"));
    }
    let unified = if cfg!(windows) {
        p.replace('\\', "/")
    } else {
        p.to_owned()
    };
    if unified.starts_with('/') || std::path::Path::new(p).is_absolute() {
        return Err(bad("must be relative to the top of the work tree"));
    }
    let parts: Vec<&str> = unified.split('/').filter(|s| !s.is_empty()).collect();
    if parts.is_empty() {
        return Err(bad("is empty"));
    }
    if parts.iter().any(|s| *s == "." || *s == "..") {
        return Err(bad("must not contain `.` or `..`"));
    }
    Ok(parts.join("/").into())
}
