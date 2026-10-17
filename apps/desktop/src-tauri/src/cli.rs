//! `pitwall` 명령줄(docs/design/04-features.md F2 · F2-2).
//!
//! - `pitwall mcp`: MCP stdio 서버(M0-17). 앱을 띄우지 않고 단일 인스턴스로 전달하지도 않는다.
//! - `pitwall [<path>[:line[:col]]]...`: 경로를 연다. 상대 경로는 **명령을 실행한 프로세스의 cwd** 로 절대화한다 —
//!   두 번째 인스턴스면 그 프로세스의 cwd 가 단일 인스턴스 플러그인을 거쳐 함께 온다.
//!
//! 첫 인자가 `mcp` 이면 언제나 서브커맨드다. 이름이 `mcp` 인 폴더는 `pitwall ./mcp` 로 연다.
//! `-` 로 시작하는 인자는 경로가 아니다(macOS 가 붙이는 `-psn_…` 등). 그런 이름의 파일은 `--` 뒤에 둔다.

use std::path::{Path, PathBuf};

/// 명령줄이 고른 일.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Mcp,
    /// 열 경로들. 비었으면 앱(웰컴 창)만 앞으로 가져온다.
    Open(Vec<OpenTarget>),
}

/// 열 경로 하나. 줄 · 열은 1부터 센다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenTarget {
    /// 절대 경로. 정규화(심링크 · `..`)는 레지스트리가 한다.
    pub path: PathBuf,
    pub line: Option<u32>,
    pub col: Option<u32>,
}

/// 경로로 쓰지 않은 인자. 로그에 남긴다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Skipped {
    /// `-` 로 시작하는 인자.
    Flag(String),
    /// 상대 경로인데 cwd 를 모른다.
    NoCwd(String),
}

/// `args` 는 실행 파일 경로를 뺀 인자다. `exists` 는 `path:line` 판정에 쓴다(시험은 가짜를 넣는다).
pub fn parse(
    args: &[String],
    cwd: Option<&Path>,
    exists: impl Fn(&Path) -> bool,
) -> (Command, Vec<Skipped>) {
    if args.first().map(String::as_str) == Some("mcp") {
        return (Command::Mcp, Vec::new());
    }
    let mut targets = Vec::new();
    let mut skipped = Vec::new();
    let mut positional_only = false;
    for arg in args {
        if !positional_only && arg == "--" {
            positional_only = true;
            continue;
        }
        if arg.is_empty() {
            continue;
        }
        if !positional_only && arg.starts_with('-') {
            skipped.push(Skipped::Flag(arg.clone()));
            continue;
        }
        let absolute = |p: &str| -> Option<PathBuf> {
            let p = Path::new(p);
            if p.is_absolute() {
                Some(p.to_path_buf())
            } else {
                cwd.filter(|c| c.is_absolute()).map(|c| c.join(p))
            }
        };
        match location(arg, |p| absolute(p).is_some_and(|abs| exists(&abs))) {
            Some((path, line, col)) => match absolute(path) {
                Some(path) => targets.push(OpenTarget { path, line, col }),
                None => skipped.push(Skipped::NoCwd(arg.clone())),
            },
            None => skipped.push(Skipped::NoCwd(arg.clone())),
        }
    }
    (Command::Open(targets), skipped)
}

/// `path[:line[:col]]` 을 가른다. 경로 이름에 `:숫자` 가 실제로 들어 있을 수 있으므로(`foo:12` 라는 파일) 디스크를 본다:
///
/// 1. 가를 수 있는 방법은 셋이다 — 통째로 경로, 끝의 `:숫자` 하나를 줄로, 끝의 `:숫자:숫자` 를 줄 · 열로.
/// 2. **덜 가른 것부터** 보아 경로가 있는 첫 해석을 고른다. 그래서 있는 파일 `foo:12` 는 `foo` 의 12줄이 아니다.
/// 3. 어느 해석도 있는 경로를 가리키지 않으면 **가장 많이 가른 것**을 고른다(아직 없는 파일이면 `path:line:col` 이 흔하다).
///
/// 줄 · 열은 1 이상 `u32` 다. `foo:0` 의 `:0` 은 이름의 일부로 본다.
fn location(arg: &str, exists: impl Fn(&str) -> bool) -> Option<(&str, Option<u32>, Option<u32>)> {
    let mut readings = vec![(arg, None, None)];
    if let Some((rest, last)) = split_number(arg) {
        readings.push((rest, Some(last), None));
        if let Some((path, line)) = split_number(rest) {
            readings.push((path, Some(line), Some(last)));
        }
    }
    readings
        .iter()
        .find(|(path, ..)| exists(path))
        .or(readings.last())
        .copied()
}

/// 끝의 `:숫자` 를 떼어 낸다. 앞부분이 비면 떼지 않는다.
fn split_number(s: &str) -> Option<(&str, u32)> {
    let (rest, digits) = s.rsplit_once(':')?;
    if rest.is_empty() || digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let n: u32 = digits.parse().ok()?;
    (n >= 1).then_some((rest, n))
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    fn open(list: &[&str], cwd: &str, existing: &[&str]) -> Vec<OpenTarget> {
        let existing: HashSet<PathBuf> = existing.iter().map(PathBuf::from).collect();
        match parse(&args(list), Some(Path::new(cwd)), |p| existing.contains(p)).0 {
            Command::Open(targets) => targets,
            Command::Mcp => panic!("mcp 가 아니다"),
        }
    }

    fn target(path: &str, line: Option<u32>, col: Option<u32>) -> OpenTarget {
        OpenTarget {
            path: path.into(),
            line,
            col,
        }
    }

    #[test]
    fn mcp_is_a_subcommand_only_in_first_position() {
        assert_eq!(parse(&args(&["mcp"]), None, |_| false).0, Command::Mcp);
        assert_eq!(
            parse(&args(&["mcp", "--whatever"]), None, |_| false).0,
            Command::Mcp
        );
        assert_eq!(
            open(&["./mcp"], "/w", &[]),
            [target("/w/./mcp", None, None)]
        );
        assert_eq!(
            open(&["/a", "mcp"], "/w", &[]),
            [target("/a", None, None), target("/w/mcp", None, None)]
        );
    }

    #[test]
    fn no_arguments_open_nothing() {
        assert_eq!(open(&[], "/w", &[]), []);
    }

    #[test]
    fn relative_paths_are_joined_to_the_callers_cwd() {
        assert_eq!(
            open(&["src/main.rs", "/abs/dir"], "/home/me/repo", &[]),
            [
                target("/home/me/repo/src/main.rs", None, None),
                target("/abs/dir", None, None),
            ]
        );
    }

    #[test]
    fn relative_path_without_cwd_is_skipped() {
        let (command, skipped) = parse(&args(&["a.txt", "/b"]), None, |_| false);
        assert_eq!(command, Command::Open(vec![target("/b", None, None)]));
        assert_eq!(skipped, [Skipped::NoCwd("a.txt".into())]);
        let (_, skipped) = parse(&args(&["a.txt"]), Some(Path::new("rel")), |_| false);
        assert_eq!(
            skipped,
            [Skipped::NoCwd("a.txt".into())],
            "상대 cwd 도 모르는 것과 같다"
        );
    }

    #[test]
    fn line_and_column_are_split_off() {
        assert_eq!(
            open(&["/r/a.rs:12", "/r/b.rs:3:7"], "/", &["/r/a.rs", "/r/b.rs"]),
            [
                target("/r/a.rs", Some(12), None),
                target("/r/b.rs", Some(3), Some(7)),
            ]
        );
    }

    #[test]
    fn an_existing_file_whose_name_has_a_colon_number_wins() {
        // `foo:12` 가 있는 파일이면 그 파일이다. `foo` 가 함께 있어도 마찬가지(덜 가른 것부터).
        assert_eq!(
            open(&["/r/foo:12"], "/", &["/r/foo:12", "/r/foo"]),
            [target("/r/foo:12", None, None)]
        );
        // `foo:12` 의 5줄.
        assert_eq!(
            open(&["/r/foo:12:5"], "/", &["/r/foo:12", "/r/foo"]),
            [target("/r/foo:12", Some(5), None)]
        );
        // `foo:12` 가 없고 `foo` 가 있으면 12줄 5열.
        assert_eq!(
            open(&["/r/foo:12:5"], "/", &["/r/foo"]),
            [target("/r/foo", Some(12), Some(5))]
        );
    }

    #[test]
    fn nothing_exists_means_the_most_split_reading() {
        assert_eq!(
            open(&["new.rs:4:2", "x:9"], "/w", &[]),
            [
                target("/w/new.rs", Some(4), Some(2)),
                target("/w/x", Some(9), None)
            ]
        );
    }

    #[test]
    fn non_numbers_zero_and_empty_parts_stay_in_the_name() {
        assert_eq!(
            open(&["/r/a:b", "/r/a:0", "/r/a:", "/r/a:99999999999"], "/", &[]),
            [
                target("/r/a:b", None, None),
                target("/r/a:0", None, None),
                target("/r/a:", None, None),
                target("/r/a:99999999999", None, None),
            ]
        );
        // `:5` 만 있으면 앞이 비어 가르지 않는다(상대 경로 `:5`).
        assert_eq!(open(&[":5"], "/w", &[]), [target("/w/:5", None, None)]);
    }

    #[test]
    fn flags_are_skipped_and_double_dash_ends_them() {
        let (command, skipped) = parse(
            &args(&["-psn_0_123", "/a", "--", "-weird"]),
            Some(Path::new("/w")),
            |_| false,
        );
        assert_eq!(
            command,
            Command::Open(vec![
                target("/a", None, None),
                target("/w/-weird", None, None)
            ])
        );
        assert_eq!(skipped, [Skipped::Flag("-psn_0_123".into())]);
    }
}
