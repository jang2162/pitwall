//! 외부 도구의 명령 템플릿(`docs/design/07-extensibility.md` 「레지스트리」 표의 「외부 도구 명령」 행, 04 F8-2).
//!
//! 명령은 **문자열 배열**이다. 셸 없이 실행하므로 치환한 요소 하나가 그대로 argv 한 칸이 된다 — 요소 안의 공백 · 따옴표 ·
//! `$` · `;` 는 글자일 뿐이다. 첫 요소(실행 파일)에는 변수를 쓸 수 없다. 변수는 요소 안 어디에나 쓸 수 있고(`{file}:{line}`),
//! 중괄호를 글자로 쓰는 문법은 없다.
//!
//! 프로젝트만 여는 `command` 와 파일까지 여는 `file_command` 를 나눈다. `command` 에는 파일 변수(`{file}` · `{line}` ·
//! `{column}`)를 쓸 수 없고, `file_command` 는 `{file}` 을 써야 한다. 무엇을 고르는지는 외부 도구 실행(`crates/pw-open-with`)이 정한다.

use std::fmt;

/// 명령 템플릿의 변수.
pub const TEMPLATE_VARS: [&str; 5] = ["project_root", "file", "line", "column", "branch"];

/// 파일을 열 때만 값이 있는 변수. `command` 에는 쓸 수 없다.
pub const FILE_VARS: [&str; 3] = ["file", "line", "column"];

/// 어느 명령인가.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandKind {
    /// `command` — 프로젝트를 연다.
    Project,
    /// `file_command` — 파일까지 연다.
    File,
}

impl CommandKind {
    pub fn key(self) -> &'static str {
        match self {
            CommandKind::Project => "command",
            CommandKind::File => "file_command",
        }
    }
}

/// 요소 하나를 나눈 조각.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Piece<'a> {
    Text(&'a str),
    Var(&'a str),
}

/// 요소를 글자와 `{변수}` 로 나눈다. 중괄호가 짝을 이루지 않거나 모르는 변수면 오류다.
fn pieces(element: &str) -> Result<Vec<Piece<'_>>, String> {
    let mut out = Vec::new();
    let mut rest = element;
    while let Some(i) = rest.find(['{', '}']) {
        if rest[i..].starts_with('}') {
            return Err(format!("`{element}` has an unmatched `}}`"));
        }
        if i > 0 {
            out.push(Piece::Text(&rest[..i]));
        }
        let after = &rest[i + 1..];
        let end = after
            .find(['{', '}'])
            .filter(|&j| after[j..].starts_with('}'))
            .ok_or_else(|| format!("`{element}` has an unmatched `{{`"))?;
        let name = &after[..end];
        if !TEMPLATE_VARS.contains(&name) {
            return Err(format!(
                "`{element}` uses unknown variable `{{{name}}}` (known: {})",
                TEMPLATE_VARS.map(|v| format!("{{{v}}}")).join(", ")
            ));
        }
        out.push(Piece::Var(name));
        rest = &after[end + 1..];
    }
    if !rest.is_empty() {
        out.push(Piece::Text(rest));
    }
    Ok(out)
}

/// 명령 하나를 검사한다. 오류 문장은 키 이름(`command` · `file_command`) 없이 내용만 말한다.
pub fn check_command(kind: CommandKind, argv: &[String]) -> Result<(), String> {
    let Some(program) = argv.first() else {
        return Err("is empty — the first element is the program to run".to_owned());
    };
    if program.trim().is_empty() {
        return Err("starts with an empty program name".to_owned());
    }
    if program.contains(['{', '}']) {
        return Err(format!(
            "program `{program}` cannot use variables — the first element is the program to run"
        ));
    }
    let mut uses_file = false;
    for element in &argv[1..] {
        for piece in pieces(element)? {
            if let Piece::Var(name) = piece {
                if FILE_VARS.contains(&name) && kind == CommandKind::Project {
                    return Err(format!(
                        "`{element}` uses `{{{name}}}`, which only `file_command` can use"
                    ));
                }
                uses_file |= name == "file";
            }
        }
    }
    if kind == CommandKind::File && !uses_file {
        return Err("does not use `{file}`".to_owned());
    }
    Ok(())
}

/// 치환에 쓰는 값. 없는 변수는 `None`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Vars {
    pub project_root: Option<String>,
    pub file: Option<String>,
    pub line: Option<String>,
    pub column: Option<String>,
    pub branch: Option<String>,
}

impl Vars {
    fn get(&self, name: &str) -> Option<&str> {
        match name {
            "project_root" => self.project_root.as_deref(),
            "file" => self.file.as_deref(),
            "line" => self.line.as_deref(),
            "column" => self.column.as_deref(),
            "branch" => self.branch.as_deref(),
            _ => None,
        }
    }
}

/// 치환하지 못한 이유.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubstituteError {
    /// 검사([`check_command`])를 지나지 않은 명령.
    Invalid(String),
    /// 이 문맥에 값이 없는 변수(이름, 중괄호 없이).
    Missing(String),
}

impl fmt::Display for SubstituteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SubstituteError::Invalid(e) => write!(f, "invalid command: {e}"),
            SubstituteError::Missing(name) => write!(f, "`{{{name}}}` has no value here"),
        }
    }
}

/// 요소마다 변수를 값으로 바꾼다. 요소 수는 그대로다 — 값에 공백이 있어도 한 요소로 남는다. 쓰인 변수 하나라도 값이 없으면
/// 오류다(그 요소를 빼지 않는다 — `--line {line}` 같은 옵션 · 값 짝이 깨진다).
pub fn substitute(argv: &[String], vars: &Vars) -> Result<Vec<String>, SubstituteError> {
    argv.iter()
        .map(|element| {
            let mut out = String::with_capacity(element.len());
            for piece in pieces(element).map_err(SubstituteError::Invalid)? {
                match piece {
                    Piece::Text(text) => out.push_str(text),
                    Piece::Var(name) => out.push_str(
                        vars.get(name)
                            .ok_or_else(|| SubstituteError::Missing(name.to_owned()))?,
                    ),
                }
            }
            Ok(out)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn accepts_every_variable_in_the_command_it_belongs_to() {
        assert_eq!(
            check_command(
                CommandKind::File,
                &argv(&["t", "{project_root}", "{file}:{line}:{column}", "{branch}"])
            ),
            Ok(())
        );
        assert_eq!(
            check_command(
                CommandKind::Project,
                &argv(&["open", "-a", "Terminal", "{project_root}"])
            ),
            Ok(())
        );
        assert_eq!(
            check_command(CommandKind::Project, &argv(&["finder"])),
            Ok(())
        );
    }

    #[test]
    fn rejects_a_variable_in_the_program() {
        let e = check_command(CommandKind::Project, &argv(&["{project_root}/bin/x"])).unwrap_err();
        assert!(e.contains("cannot use variables"), "{e}");
        let e = check_command(CommandKind::Project, &argv(&["x}"])).unwrap_err();
        assert!(e.contains("cannot use variables"), "{e}");
    }

    #[test]
    fn rejects_empty_commands_and_programs() {
        assert!(check_command(CommandKind::Project, &[]).is_err());
        assert!(check_command(CommandKind::Project, &argv(&[" ", "{project_root}"])).is_err());
    }

    #[test]
    fn keeps_file_variables_out_of_the_project_command() {
        for var in ["{file}", "{line}", "x:{column}"] {
            let e = check_command(CommandKind::Project, &argv(&["zed", var])).unwrap_err();
            assert!(e.contains("only `file_command`"), "{var}: {e}");
        }
    }

    #[test]
    fn a_file_command_must_use_the_file() {
        let e = check_command(CommandKind::File, &argv(&["zed", "{project_root}"])).unwrap_err();
        assert_eq!(e, "does not use `{file}`");
    }

    #[test]
    fn rejects_unknown_variables_and_unmatched_braces() {
        for (element, wanted) in [
            ("{root}", "unknown variable `{root}`"),
            ("{file", "unmatched `{`"),
            ("x }", "unmatched `}`"),
            ("{a{file}}", "unmatched `{`"),
        ] {
            let e = check_command(CommandKind::File, &argv(&["t", element, "{file}"])).unwrap_err();
            assert!(e.contains(wanted), "{element}: {e}");
        }
    }

    /// 값의 공백 · 따옴표 · 셸 문자 · 개행은 그대로 한 요소에 남는다 — 요소를 쪼개거나 해석하지 않는다.
    #[test]
    fn substitutes_each_element_without_splitting_values() {
        let vars = Vars {
            project_root: Some("/tmp/a b/$(touch x); 'q'".to_owned()),
            file: Some("/tmp/a b/-n\nx\".rs".to_owned()),
            line: Some("3".to_owned()),
            column: Some("4".to_owned()),
            branch: None,
        };
        let out = substitute(
            &argv(&["zed", "{project_root}", "{file}:{line}:{column}", "a b"]),
            &vars,
        )
        .unwrap();
        assert_eq!(
            out,
            [
                "zed",
                "/tmp/a b/$(touch x); 'q'",
                "/tmp/a b/-n\nx\".rs:3:4",
                "a b"
            ]
        );
    }

    #[test]
    fn an_unchecked_command_fails_to_substitute() {
        let out = substitute(&argv(&["t", "lit {}x"]), &Vars::default());
        assert!(matches!(out, Err(SubstituteError::Invalid(_))), "{out:?}");
    }

    #[test]
    fn a_missing_value_fails_instead_of_dropping_the_element() {
        let vars = Vars {
            project_root: Some("/p".to_owned()),
            ..Vars::default()
        };
        assert_eq!(
            substitute(
                &argv(&["git-tool", "{project_root}", "--branch", "{branch}"]),
                &vars
            ),
            Err(SubstituteError::Missing("branch".to_owned()))
        );
    }
}
