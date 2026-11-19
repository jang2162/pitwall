//! `justfile` 의 레시피 → 종류 `shell` 의 가상 구성 `just '<레시피>'`. `just` 를 부르지 않고(`just --dump` · `--summary` 도)
//! 정적으로 읽는다:
//!
//! - 파일은 루트의 `justfile` · `.justfile`(대소문자 무시 — just 와 같다). 둘 이상이면 just 처럼 까닭이다. `import` · `mod` 는
//!   따라가지 않는다.
//! - 레시피 머리: 0 칸에서 `@`? + 이름(`[A-Za-z_][A-Za-z0-9_-]*`) + 매개변수 + 따옴표 · 괄호 밖의 `:`(뒤에 `=` 가 아님). `set` ·
//!   `alias` · `export` · `import` · `mod` 줄과 변수 대입(`x := …`)은 레시피가 아니다.
//! - 빼는 레시피: 비공개(`_` 로 시작하거나 `[private]` 속성). 매개변수가 필요한 레시피도 넣는다 — 돌리면 just 가 무엇이 빠졌는지
//!   알리고, 「구성으로 저장」 한 뒤 명령에 인자를 더할 수 있다.
//! - 정의 본문: 파일 전체의 sha256 과 레시피 이름([`super::file_definition`]) — 의존 레시피 · `set shell` · 변수가 그 레시피의
//!   실행을 바꾸므로 레시피의 몸만으로는 모자라다.

use std::collections::BTreeSet;

use super::{
    DetectContext, DetectedTask, Found, TaskDetector, file, file_definition, lines_list,
    pick_names, sha256_hex, shell_command, shell_safe_name,
};
use crate::config::{RunConfig, RunKindConfig};

pub const ID: &str = "just";
/// 파일 감시가 보는 이름(just 는 대소문자를 가리지 않고 찾는다 — 흔한 꼴만 둔다).
pub const SOURCES: &[&str] = &["justfile", "Justfile", "JUSTFILE", ".justfile"];

pub struct JustDetector;

impl TaskDetector for JustDetector {
    fn id(&self) -> &'static str {
        ID
    }

    fn sources(&self) -> &'static [&'static str] {
        SOURCES
    }

    fn detect(&self, cx: &DetectContext<'_>) -> Result<Found, String> {
        let candidates: Vec<&String> = cx
            .entries
            .iter()
            .filter(|n| n.eq_ignore_ascii_case("justfile") || n.eq_ignore_ascii_case(".justfile"))
            .collect();
        let name = match candidates.as_slice() {
            [] => return Ok(Found::default()),
            [one] => one.as_str(),
            many => {
                let names: Vec<&str> = many.iter().map(|n| n.as_str()).collect();
                return Err(format!(
                    "more than one justfile in the project root ({}); just refuses to pick one",
                    names.join(", ")
                ));
            }
        };
        let Some((path, text)) = file::read(cx.root, name)? else {
            return Ok(Found::default());
        };
        let parsed = parse(&text);
        let mut found = Found::default();
        let sha = sha256_hex(&text);
        for recipe in pick_names(name, parsed.recipes, &mut found.skipped) {
            found.tasks.push(DetectedTask {
                detector: ID,
                config: RunConfig {
                    name: recipe.clone(),
                    kind: RunKindConfig::Shell {
                        command: shell_command("just", &recipe),
                    },
                    cwd: None,
                    env: Default::default(),
                    env_file: Vec::new(),
                    profiles: Default::default(),
                    debug: None,
                    before: Vec::new(),
                },
                definition: file_definition(&sha, &recipe),
                name: recipe,
                source: path.clone(),
            });
        }
        if !parsed.unsafe_lines.is_empty() {
            found.skipped.push(format!(
                "{name}: skipped {} recipe(s) whose names are not safe to pass to a shell (line {})",
                parsed.unsafe_lines.len(),
                lines_list(&parsed.unsafe_lines)
            ));
        }
        Ok(found)
    }
}

/// 읽은 결과.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Parsed {
    /// 레시피(중복 없이).
    pub recipes: BTreeSet<String>,
    /// 셸에 안전하지 않은 이름의 레시피가 있던 줄(1부터).
    pub unsafe_lines: Vec<usize>,
}

/// 레시피가 아닌 0 칸 줄의 첫 낱말.
const NOT_RECIPES: &[&str] = &[
    "set", "alias", "export", "unexport", "import", "import?", "mod", "mod?",
];

/// 따옴표(`'…'` · `"…"` · `` `…` ``) · 괄호 밖의 첫 `:` 자리.
fn header_colon(line: &str) -> Option<usize> {
    let mut quote: Option<char> = None;
    let mut depth = 0usize;
    let mut escaped = false;
    for (i, c) in line.char_indices() {
        if let Some(q) = quote {
            if escaped {
                escaped = false;
            } else if c == '\\' && q == '"' {
                escaped = true;
            } else if c == q {
                quote = None;
            }
            continue;
        }
        match c {
            '\'' | '"' | '`' => quote = Some(c),
            '(' | '{' => depth += 1,
            ')' | '}' => depth = depth.saturating_sub(1),
            ':' if depth == 0 => return Some(i),
            '#' if depth == 0 => return None,
            _ => {}
        }
    }
    None
}

/// 줄 머리의 이름(`@` 을 걷은) — 레시피 이름의 꼴이 아니어도 낱말 하나를 준다(안전 판정은 부르는 쪽).
fn header_name(head: &str) -> &str {
    let head = head.strip_prefix('@').unwrap_or(head);
    let end = head.find(|c: char| c.is_whitespace()).unwrap_or(head.len());
    &head[..end]
}

/// 속성 줄(`[private]` · `[group('x'), private]`)에 `private` 가 있는가.
fn attribute_private(line: &str) -> bool {
    let inner = line.trim().trim_start_matches('[').trim_end_matches(']');
    inner.split(',').any(|a| a.trim() == "private")
}

/// `justfile` 글을 읽는다.
pub fn parse(text: &str) -> Parsed {
    let mut parsed = Parsed::default();
    let lines: Vec<&str> = text
        .lines()
        .map(|l| l.strip_suffix('\r').unwrap_or(l))
        .collect();
    let mut private = false;
    let mut i = 0;
    while i < lines.len() {
        let (number, line) = (i + 1, lines[i]);
        i += 1;
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with([' ', '\t']) {
            continue; // 레시피의 몸
        }
        if line.starts_with('[') {
            private |= attribute_private(line);
            continue;
        }
        let attributes_private = std::mem::take(&mut private);
        let first_word = line.split_whitespace().next().unwrap_or("");
        if NOT_RECIPES.contains(&first_word) {
            continue;
        }
        let Some(colon) = header_colon(line) else {
            continue;
        };
        if line[colon + 1..].starts_with('=') {
            continue; // `x := …`
        }
        let name = header_name(&line[..colon]);
        if attributes_private || name.starts_with('_') {
            continue;
        }
        if !shell_safe_name(name) {
            parsed.unsafe_lines.push(number);
            continue;
        }
        parsed.recipes.insert(name.to_owned());
    }
    parsed
}
