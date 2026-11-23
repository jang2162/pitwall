//! HTTP 의 Bearer 토큰 파일(18-m7-plan 결정 M7-D11): 켤 때 만들어 `$STATE/mcp/http-token`(Relay 는 `$STATE/relay/mcp/` — Relay 의
//! `AppPaths::state` 가 이미 `$STATE/relay` 다)에 두고 앱이 다시 떠도 같은 값을 쓴다. 「토큰 새로 만들기」 가 바꾼다.
//!
//! - 파일은 0600, 새로 만드는 폴더는 0700 이다(감사 로그와 같다). 쓰기는 같은 폴더의 임시 파일(0600 으로 만든다)에 쓴 뒤 이름을
//!   바꾼다 — 읽는 쪽이 반쯤 쓴 토큰을 보지 않고, 권한이 넓은 순간이 없다.
//! - 읽을 때 **일반 파일 · 이 사용자 소유 · 그룹 · 다른 사용자 권한 없음 · 꼴이 맞음** 이 아니면 쓰지 않고 새로 만든다 — 다른
//!   사람이 읽었을 수 있는 토큰을 계속 쓰지 않는다.
//! - [`Token`] 은 `Debug` 에 값을 싣지 않고 `Display` 가 없다 — 로그 · 오류 글에 실수로 들어가지 않게. 값은 [`Token::reveal`] 로만
//!   꺼낸다(설정 창의 `mcp.http.token` 하나).

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// `$STATE/mcp` 안의 토큰 파일 이름.
pub const FILE_NAME: &str = "http-token";
/// 토큰의 바이트 수(16진 글로 두 배).
const BYTES: usize = 32;

/// Bearer 토큰. 값은 [`Self::reveal`] 로만 꺼낸다.
#[derive(Clone, PartialEq, Eq)]
pub struct Token(String);

impl std::fmt::Debug for Token {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Token(<hidden>)")
    }
}

impl Token {
    /// 새 토큰(OS 난수 32바이트의 16진 글).
    pub fn generate() -> io::Result<Self> {
        let mut bytes = [0u8; BYTES];
        getrandom::fill(&mut bytes).map_err(|e| io::Error::other(e.to_string()))?;
        Ok(Self(hex(&bytes)))
    }

    /// 사람에게 보이는 값(설정 창). 로그 · 오류 · 감사에 넣지 않는다.
    pub fn reveal(&self) -> &str {
        &self.0
    }

    /// 상수 시간 비교 — 앞에서부터 맞은 글자 수가 걸린 시간으로 새지 않는다. 길이는 비밀이 아니다(늘 64).
    pub fn matches(&self, presented: &[u8]) -> bool {
        let ours = self.0.as_bytes();
        if ours.len() != presented.len() {
            return false;
        }
        let diff = ours
            .iter()
            .zip(presented)
            .fold(0u8, |acc, (a, b)| acc | (a ^ b));
        std::hint::black_box(diff) == 0
    }

    fn parse(text: &str) -> Option<Self> {
        let text = text.trim_end_matches(['\n', '\r']);
        (text.len() == BYTES * 2 && text.bytes().all(|b| b.is_ascii_hexdigit()))
            .then(|| Self(text.to_ascii_lowercase()))
    }
}

/// 토큰 파일 하나.
#[derive(Debug, Clone)]
pub struct TokenFile {
    path: PathBuf,
}

impl TokenFile {
    /// `<state>/mcp/http-token`.
    pub fn under_state(state: &Path) -> Self {
        Self {
            path: state.join(crate::audit::DIR).join(FILE_NAME),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 쓸 수 있는 토큰이 있으면 그것. 없거나 쓸 수 없으면(권한이 넓음 · 다른 사용자 소유 · 일반 파일 아님 · 꼴이 틀림) `None`.
    pub fn read(&self) -> io::Result<Option<Token>> {
        let meta = match fs::symlink_metadata(&self.path) {
            Ok(m) => m,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
        if !meta.file_type().is_file() || !private(&meta) {
            tracing::warn!(
                file = %self.path.display(),
                "the mcp http token file is not a private regular file; a new token replaces it"
            );
            return Ok(None);
        }
        let text = fs::read_to_string(&self.path)?;
        Ok(Token::parse(&text))
    }

    /// 켤 때: 있는 토큰을 쓰고, 없으면(쓸 수 없으면) 만든다.
    pub fn load_or_create(&self) -> io::Result<Token> {
        match self.read()? {
            Some(token) => Ok(token),
            None => self.regenerate(),
        }
    }

    /// 새 토큰을 만들어 파일을 바꾼다(「토큰 새로 만들기」).
    pub fn regenerate(&self) -> io::Result<Token> {
        let token = Token::generate()?;
        let dir = self
            .path
            .parent()
            .ok_or_else(|| io::Error::other("the token file has no folder"))?;
        private_dir(dir)?;
        let mut suffix = [0u8; 8];
        getrandom::fill(&mut suffix).map_err(|e| io::Error::other(e.to_string()))?;
        let tmp = dir.join(format!(".{FILE_NAME}.{}.tmp", hex(&suffix)));
        let written = (|| {
            let mut file = create_private(&tmp)?;
            file.write_all(token.0.as_bytes())?;
            file.write_all(b"\n")?;
            file.sync_all()?;
            fs::rename(&tmp, &self.path)
        })();
        if written.is_err() {
            let _ = fs::remove_file(&tmp);
        }
        written.map(|()| token)
    }
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

#[cfg(unix)]
fn private(meta: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    meta.mode() & 0o077 == 0 && meta.uid() == unsafe { libc::geteuid() }
}

#[cfg(not(unix))]
fn private(_meta: &fs::Metadata) -> bool {
    // Windows: `$STATE` 는 사용자 프로필 아래(그 사용자만)다. ACL 은 따로 보지 않는다.
    true
}

#[cfg(unix)]
fn private_dir(dir: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    if dir.is_dir() {
        return Ok(());
    }
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
}

#[cfg(not(unix))]
fn private_dir(dir: &Path) -> io::Result<()> {
    fs::create_dir_all(dir)
}

#[cfg(unix)]
fn create_private(path: &Path) -> io::Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
}

#[cfg(not(unix))]
fn create_private(path: &Path) -> io::Result<fs::File> {
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_token_is_kept_across_loads_and_replaced_on_regenerate() {
        let tmp = tempfile::tempdir().unwrap();
        let file = TokenFile::under_state(tmp.path());
        assert!(file.read().unwrap().is_none());
        let first = file.load_or_create().unwrap();
        assert_eq!(first.reveal().len(), 64);
        assert_eq!(file.load_or_create().unwrap(), first, "다시 떠도 같은 토큰");
        assert_eq!(
            TokenFile::under_state(tmp.path()).read().unwrap(),
            Some(first.clone())
        );
        let second = file.regenerate().unwrap();
        assert_ne!(second, first);
        assert_eq!(file.load_or_create().unwrap(), second);
        // 임시 파일이 남지 않는다.
        let names: Vec<_> = fs::read_dir(file.path().parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, [FILE_NAME]);
    }

    #[test]
    fn the_token_never_shows_in_debug() {
        let token = Token::generate().unwrap();
        assert!(!format!("{token:?}").contains(token.reveal()));
    }

    #[test]
    fn matching_needs_every_byte() {
        let token = Token::generate().unwrap();
        assert!(token.matches(token.reveal().as_bytes()));
        let mut near = token.reveal().as_bytes().to_vec();
        near[63] = if near[63] == b'0' { b'1' } else { b'0' };
        assert!(!token.matches(&near));
        assert!(!token.matches(&token.reveal().as_bytes()[..63]));
        assert!(!token.matches(b""));
    }

    /// 파일은 0600, 새 폴더는 0700. 권한이 넓어진 토큰(다른 사람이 읽었을 수 있다)은 쓰지 않고 새로 만든다. 꼴이 틀린 파일 ·
    /// 심링크도 같다.
    #[cfg(unix)]
    #[test]
    fn the_token_file_is_private_and_a_widened_one_is_replaced() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let file = TokenFile::under_state(tmp.path());
        let first = file.load_or_create().unwrap();
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(file.path()), 0o600);
        assert_eq!(mode(file.path().parent().unwrap()), 0o700);

        fs::set_permissions(file.path(), fs::Permissions::from_mode(0o644)).unwrap();
        assert!(file.read().unwrap().is_none());
        let second = file.load_or_create().unwrap();
        assert_ne!(second, first, "넓어진 토큰은 버린다");
        assert_eq!(mode(file.path()), 0o600);

        fs::write(file.path(), "not a token").unwrap();
        assert!(file.read().unwrap().is_none());

        fs::remove_file(file.path()).unwrap();
        let elsewhere = tmp.path().join("elsewhere");
        fs::write(&elsewhere, format!("{}\n", second.reveal())).unwrap();
        fs::set_permissions(&elsewhere, fs::Permissions::from_mode(0o600)).unwrap();
        std::os::unix::fs::symlink(&elsewhere, file.path()).unwrap();
        assert!(file.read().unwrap().is_none(), "심링크는 따라가지 않는다");
        let third = file.load_or_create().unwrap();
        assert!(fs::symlink_metadata(file.path()).unwrap().is_file());
        assert_ne!(third, second);
    }
}
