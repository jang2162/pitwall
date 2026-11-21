//! `relay.toml`(`$CONFIG/relay/relay.toml` — 18-m7-plan M7-04 ③). 그 머신의 사람이 쓰는 파일이다 — **RPC 로 쓰지 않는다**(메인
//! 판정 M7-P5: 원격 머신의 경계를 Desktop 이 넓히지 못하게). 데몬은 뜰 때 한 번 읽는다(바꾸면 데몬을 다시 띄운다).
//!
//! ```toml
//! schema = 1                    # 모든 설정 파일의 첫 줄(03 「설정 파일 형식」) — 없으면 Host 의 설정 읽기가 붙인다
//! name = "build-box"            # 선택 — 상태(`status`)에 싣는 이름
//! roots = ["~/work", "/srv/repos"]  # 프로젝트로 등록할 수 있는 폴더. 비면 아무것도 등록되지 않는다
//!
//! [idle]
//! exit_after = "30m"            # 링크 · 실행 · 터미널이 없으면 이만큼 뒤 끝난다(기본 30분)
//! stop_lsp_after = "30m"        # 링크가 없으면 이만큼 뒤 언어 서버를 끈다(기본 30분)
//! ```
//!
//! 시간은 `"90s"` · `"30m"` · `"2h"` 또는 초(정수)다. 파일이 없으면 기본값(`roots` 없음)이다. 읽을 수 없거나 틀린 값이면 데몬은 뜨지
//! 않는다 — 고친 줄 모르는 경계로 돌지 않게(fail-closed).

use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// 기본 유휴 시간(메인 판정 M7-P13 의 출발값).
pub const DEFAULT_EXIT_AFTER: Duration = Duration::from_secs(30 * 60);
pub const DEFAULT_STOP_LSP_AFTER: Duration = Duration::from_secs(30 * 60);

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RelayConfig {
    pub name: Option<String>,
    /// 적힌 그대로(`~/` 만 홈으로 푼다). 정규화는 [`crate::roots::Roots`] 가 한다.
    pub roots: Vec<PathBuf>,
    pub idle: IdleConfig,
    /// 이 판이 아직 쓰지 않는 표(`[listen]` — M7-24). 데몬은 경고만 남긴다 — 네트워크로 받지 않는 쪽이라 무시해도 넓어지지 않는다.
    pub not_yet: Vec<String>,
}

/// 알지만 이 판이 아직 쓰지 않는 표.
pub const NOT_YET: &[&str] = &["listen"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IdleConfig {
    pub exit_after: Duration,
    pub stop_lsp_after: Duration,
}

impl Default for IdleConfig {
    fn default() -> Self {
        Self {
            exit_after: DEFAULT_EXIT_AFTER,
            stop_lsp_after: DEFAULT_STOP_LSP_AFTER,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigError {
    pub path: PathBuf,
    pub reason: String,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.path.display(), self.reason)
    }
}

impl std::error::Error for ConfigError {}

/// 파일을 읽는다. 없으면 기본값.
pub fn load(path: &Path, home: Option<&Path>) -> Result<RelayConfig, ConfigError> {
    let error = |reason: String| ConfigError {
        path: path.to_path_buf(),
        reason,
    };
    match std::fs::read_to_string(path) {
        Ok(text) => parse(&text, home).map_err(error),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(RelayConfig::default()),
        Err(e) => Err(error(e.to_string())),
    }
}

/// 글을 푼다. 모르는 키는 오류다 — 틀린 이름(`root` 등)이 말없이 「roots 없음」 이 되지 않게.
pub fn parse(text: &str, home: Option<&Path>) -> Result<RelayConfig, String> {
    let doc: toml_edit::DocumentMut = text.parse().map_err(|e: toml_edit::TomlError| {
        // 원문 발췌는 걷는다(03 「로그 · 진단」 — 설정 오류의 원문은 로그에 싣지 않는다).
        e.message().to_owned()
    })?;
    let mut config = RelayConfig::default();
    for (key, item) in doc.iter() {
        match key {
            // 설정 파일 판(03). 이 판이 아는 것은 1 까지다 — 새 판의 파일은 읽지 않는다.
            "schema" => match item.as_integer() {
                Some(0 | 1) => {}
                _ => {
                    return Err(
                        "`schema` must be 1 (this relay reads relay.toml schema 1)".to_owned()
                    );
                }
            },
            "name" => {
                let name = item.as_str().ok_or("`name` must be a string")?.trim();
                config.name = (!name.is_empty()).then(|| name.to_owned());
            }
            "roots" => {
                let roots = item.as_array().ok_or("`roots` must be an array of paths")?;
                for root in roots {
                    let root = root.as_str().ok_or("`roots` must be an array of paths")?;
                    config.roots.push(expand(root, home)?);
                }
            }
            "idle" => {
                let table = item.as_table_like().ok_or("`[idle]` must be a table")?;
                for (key, item) in table.iter() {
                    let value = duration(item).ok_or_else(|| {
                        format!("`idle.{key}` must be a duration like \"30m\" or seconds")
                    })?;
                    match key {
                        "exit_after" => config.idle.exit_after = value,
                        "stop_lsp_after" => config.idle.stop_lsp_after = value,
                        other => return Err(format!("unknown key `idle.{other}`")),
                    }
                }
            }
            other if NOT_YET.contains(&other) => config.not_yet.push(other.to_owned()),
            other => return Err(format!("unknown key `{other}`")),
        }
    }
    Ok(config)
}

/// `~/…` 는 홈 아래로. 그 밖에는 절대 경로만 받는다 — 상대 경로는 데몬이 뜬 위치에 따라 뜻이 바뀐다.
fn expand(root: &str, home: Option<&Path>) -> Result<PathBuf, String> {
    let path = match root.strip_prefix("~/").or((root == "~").then_some("")) {
        Some(rest) => home
            .ok_or_else(|| format!("`{root}`: no home folder to expand `~`"))?
            .join(rest),
        None => PathBuf::from(root),
    };
    if path.is_absolute() {
        Ok(path)
    } else {
        Err(format!(
            "`{root}`: roots must be absolute paths (or start with `~/`)"
        ))
    }
}

fn duration(item: &toml_edit::Item) -> Option<Duration> {
    if let Some(seconds) = item.as_integer() {
        return u64::try_from(seconds).ok().map(Duration::from_secs);
    }
    let text = item.as_str()?.trim();
    let (number, unit) = text.split_at(text.find(|c: char| !c.is_ascii_digit())?);
    let number: u64 = number.parse().ok()?;
    let seconds = match unit.trim() {
        "s" => number,
        "m" => number.checked_mul(60)?,
        "h" => number.checked_mul(3600)?,
        _ => return None,
    };
    Some(Duration::from_secs(seconds))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_name_roots_and_idle() {
        let home = Path::new("/home/me");
        let config = parse(
            r#"
schema = 1
name = " box "
roots = ["~/work", "/srv/repos"]
[idle]
exit_after = "2h"
stop_lsp_after = 90
"#,
            Some(home),
        )
        .unwrap();
        assert_eq!(config.name.as_deref(), Some("box"));
        assert_eq!(
            config.roots,
            vec![PathBuf::from("/home/me/work"), PathBuf::from("/srv/repos")]
        );
        assert_eq!(config.idle.exit_after, Duration::from_secs(7200));
        assert_eq!(config.idle.stop_lsp_after, Duration::from_secs(90));
        assert!(config.not_yet.is_empty());
        let later = parse("[listen]\nport = 7420\n", None).unwrap();
        assert_eq!(later.not_yet, ["listen"]);
    }

    #[test]
    fn an_empty_file_has_no_roots_and_the_default_idle() {
        let config = parse("", None).unwrap();
        assert!(config.roots.is_empty());
        assert_eq!(config.idle, IdleConfig::default());
        assert_eq!(config.idle.exit_after, Duration::from_secs(30 * 60));
    }

    #[test]
    fn wrong_values_and_unknown_keys_are_errors() {
        for text in [
            "root = [\"/a\"]",
            "roots = \"/a\"",
            "roots = [\"relative/path\"]",
            "[idle]\nexit_after = \"soon\"",
            "[idle]\nexit = \"30m\"",
            "roots = [",
            "schema = 2",
        ] {
            assert!(parse(text, Some(Path::new("/h"))).is_err(), "{text}");
        }
        assert!(parse("roots = [\"~/x\"]", None).is_err());
    }

    #[test]
    fn a_missing_file_is_the_default() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            load(&dir.path().join("relay.toml"), None).unwrap(),
            RelayConfig::default()
        );
    }
}
