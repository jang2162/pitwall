//! `Makefile` 의 대상 → 종류 `shell` 의 가상 구성 `make '<대상>'`. `make` 를 부르지 않고 규칙 줄을 정적으로 읽는다(GNU make 의
//! 문법 중 흔한 것):
//!
//! - 파일은 GNU make 가 찾는 순서 `GNUmakefile` · `makefile` · `Makefile` 의 첫 것. `include` 는 따라가지 않는다.
//! - 규칙 줄: 탭으로 시작하지 않고, 괄호(`$(…)` · `${…}`) 밖의 첫 `:` 앞이 대상들이다. 변수 대입(`=` 이 `:` 앞, `:=` · `::=`,
//!   대상별 변수 `t: X = 1`)과 지시어 줄(`ifeq` · `include` · `define … endef` 의 몸 등)은 규칙이 아니다.
//! - 빼는 대상: `.` 으로 시작하는 것(`.PHONY` 같은 특수 대상 · 접미사 규칙 `.c.o`), 패턴 규칙(`%`), 변수 대상(`$(VAR)`), 와일드카드.
//!   셸에 안전하지 않은 이름([`super::shell_safe_name`])은 빼고 줄 번호를 남긴다.
//! - 정의 본문: 파일 전체의 sha256 과 대상 이름([`super::file_definition`]) — 선행 대상의 레시피 · 변수 · `SHELL` 이 그 대상의
//!   실행을 바꾸므로 대상의 레시피만으로는 모자라다. 대상마다 본문을 복사하지 않아 대상 수천 × 긴 레시피로 불어나지 않는다.

use std::collections::BTreeSet;

use super::{
    DetectContext, DetectedTask, Found, TaskDetector, file, file_definition, lines_list,
    pick_names, sha256_hex, shell_command, shell_safe_name,
};
use crate::config::{RunConfig, RunKindConfig};

pub const ID: &str = "make";
/// GNU make 가 찾는 순서.
pub const SOURCES: &[&str] = &["GNUmakefile", "makefile", "Makefile"];

pub struct MakeDetector;

impl TaskDetector for MakeDetector {
    fn id(&self) -> &'static str {
        ID
    }

    fn sources(&self) -> &'static [&'static str] {
        SOURCES
    }

    fn detect(&self, cx: &DetectContext<'_>) -> Result<Found, String> {
        let Some(name) = SOURCES.iter().find(|n| cx.entries.contains(**n)) else {
            return Ok(Found::default());
        };
        let Some((path, text)) = file::read(cx.root, name)? else {
            return Ok(Found::default());
        };
        let parsed = parse(&text);
        let mut found = Found::default();
        let sha = sha256_hex(&text);
        for target in pick_names(name, parsed.targets, &mut found.skipped) {
            found.tasks.push(DetectedTask {
                detector: ID,
                config: RunConfig {
                    name: target.clone(),
                    kind: RunKindConfig::Shell {
                        command: shell_command("make", &target),
                    },
                    cwd: None,
                    env: Default::default(),
                    env_file: Vec::new(),
                    profiles: Default::default(),
                    debug: None,
                    before: Vec::new(),
                },
                source: path.clone(),
                definition: file_definition(&sha, &target),
                name: target,
            });
        }
        if !parsed.unsafe_lines.is_empty() {
            found.skipped.push(format!(
                "{name}: skipped {} target(s) whose names are not safe to pass to a shell (line {})",
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
    /// 대상(중복 없이).
    pub targets: BTreeSet<String>,
    /// 셸에 안전하지 않은 이름의 대상이 있던 줄(1부터).
    pub unsafe_lines: Vec<usize>,
}

/// 지시어(규칙이 아닌 줄의 첫 낱말).
const DIRECTIVES: &[&str] = &[
    "include", "-include", "sinclude", "ifeq", "ifneq", "ifdef", "ifndef", "else", "endif",
    "export", "unexport", "override", "private", "vpath", "undefine", "define", "endef",
];

/// 논리 줄 하나(`\` 로 이은 물리 줄을 합친 것).
struct Line {
    /// 첫 물리 줄의 번호(1부터).
    number: usize,
    /// `\` 이음을 공백으로 바꾼 것 — 규칙을 읽는다.
    joined: String,
}

fn logical_lines(text: &str) -> Vec<Line> {
    let mut out = Vec::new();
    let mut current: Option<Line> = None;
    for (i, physical) in text.lines().enumerate() {
        let physical = physical.strip_suffix('\r').unwrap_or(physical);
        let continued = physical.ends_with('\\') && !physical.ends_with("\\\\");
        let piece = if continued {
            &physical[..physical.len() - 1]
        } else {
            physical
        };
        match &mut current {
            Some(line) => {
                line.joined.push(' ');
                line.joined.push_str(piece.trim_start());
            }
            None => {
                current = Some(Line {
                    number: i + 1,
                    joined: piece.to_owned(),
                });
            }
        }
        if !continued {
            out.extend(current.take());
        }
    }
    out.extend(current);
    out
}

/// `#` 주석을 걷는다(`\#` 는 글자다).
fn strip_comment(line: &str) -> &str {
    let bytes = line.as_bytes();
    for (i, b) in bytes.iter().enumerate() {
        if *b == b'#' && (i == 0 || bytes[i - 1] != b'\\') {
            return &line[..i];
        }
    }
    line
}

/// 괄호(`$(…)` · `${…}`) 밖의 첫 `:` · `=` 자리.
fn first_outside(line: &str) -> (Option<usize>, Option<usize>) {
    let (mut depth, mut colon, mut equals) = (0usize, None, None);
    for (i, c) in line.char_indices() {
        match c {
            '(' | '{' => depth += 1,
            ')' | '}' => depth = depth.saturating_sub(1),
            ':' if depth == 0 && colon.is_none() => colon = Some(i),
            '=' if depth == 0 && equals.is_none() => equals = Some(i),
            _ => {}
        }
    }
    (colon, equals)
}

/// 규칙 줄이면 그 대상들.
fn rule_targets(line: &str) -> Option<Vec<&str>> {
    let line = strip_comment(line);
    let (colon, equals) = first_outside(line);
    let colon = colon?;
    if equals.is_some_and(|e| e < colon) {
        return None; // `X = a:b`, `X ?= …`
    }
    let mut after = &line[colon + 1..];
    if let Some(rest) = after.strip_prefix(':') {
        after = rest; // 이중 콜론 규칙 `t::`
    }
    if after.starts_with('=') {
        return None; // `X := …`, `X ::= …`
    }
    // 레시피를 같은 줄에 적은 `t: deps ; cmd` 의 레시피는 `=` 을 담아도 된다.
    let prerequisites = after.split(';').next().unwrap_or("");
    if prerequisites.contains('=') {
        return None; // 대상별 변수 `t: X = 1`
    }
    let before = line[..colon].trim_end().trim_end_matches('&'); // 묶음 대상 `a b &: c`
    let targets: Vec<&str> = before.split_whitespace().collect();
    if targets.is_empty() {
        return None;
    }
    Some(targets)
}

/// 감지에 넣지 않는 대상(조용히 — 이름이 아니라 규칙의 꼴이다).
fn not_a_task(target: &str) -> bool {
    target.starts_with('.') || target.contains(['%', '$', '*', '?', '[', ']'])
}

/// `Makefile` 글을 읽는다.
pub fn parse(text: &str) -> Parsed {
    let mut parsed = Parsed::default();
    let lines = logical_lines(text);
    let mut define_depth = 0usize;
    let mut i = 0;
    while i < lines.len() {
        let line = &lines[i];
        i += 1;
        let first_word = line.joined.split_whitespace().next().unwrap_or("");
        if define_depth > 0 {
            if first_word == "endef" {
                define_depth -= 1;
            } else if is_define(&line.joined) {
                define_depth += 1;
            }
            continue;
        }
        if line.joined.starts_with('\t') {
            continue; // 규칙 밖의 레시피 줄
        }
        if is_define(&line.joined) {
            define_depth += 1;
            continue;
        }
        if DIRECTIVES.contains(&first_word) {
            continue;
        }
        let Some(targets) = rule_targets(&line.joined) else {
            continue;
        };
        // 그 레시피(탭 줄 — 사이의 빈 줄 · 주석 줄도)를 건너뛴다.
        while i < lines.len() {
            let next = &lines[i];
            if !(next.joined.starts_with('\t')
                || next.joined.trim().is_empty()
                || next.joined.starts_with('#'))
            {
                break;
            }
            i += 1;
        }
        for target in targets {
            if not_a_task(target) {
                continue;
            }
            if !shell_safe_name(target) {
                if parsed.unsafe_lines.last() != Some(&line.number) {
                    parsed.unsafe_lines.push(line.number);
                }
                continue;
            }
            if !parsed.targets.contains(target) {
                parsed.targets.insert(target.to_owned());
            }
        }
    }
    parsed
}

/// `define NAME` · `override define NAME` · `export define …`.
fn is_define(line: &str) -> bool {
    let mut words = line.split_whitespace();
    match words.next() {
        Some("define") => true,
        Some("override" | "export" | "private") => words.next() == Some("define"),
        _ => false,
    }
}
