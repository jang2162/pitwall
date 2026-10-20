//! Linguist 의 판별 단계(`lib/linguist.rb` 의 `STRATEGIES`)를 옮긴 것. 단계와 순서는 Linguist 와 같다:
//! 모드라인 → 파일명 → shebang → 확장자(`generic.yml` 의 흔한 확장자는 건너뜀) → XML 머리 → manpage → heuristics.
//! 각 단계는 앞 단계가 남긴 후보와 교집합을 내고, 후보가 하나가 되면 멈춘다.
//!
//! Linguist 의 마지막 단계(베이즈 분류기, `classifier.rb`)는 옮기지 않는다 — 학습 데이터가 samples 전체다. 후보가 여럿 남으면
//! 대표 확장자 · popular.yml 로 고른다([`pick`]).
//!
//! 내용은 앞부분만 본다(heuristics 는 Linguist 도 앞 50 KiB 만 본다). 모드라인의 「끝 5줄」은 받은 앞부분의 끝 5줄이다 —
//! 앞부분이 파일 전체가 아니면 Linguist 와 다를 수 있다.

use std::sync::{LazyLock, OnceLock};

use regex::Regex;
use regex::bytes::Regex as BytesRegex;

include!(concat!(env!("OUT_DIR"), "/linguist.rs"));

/// Linguist heuristics 가 보는 앞부분 크기(`Heuristics::HEURISTICS_CONSIDER_BYTES`).
pub const HEAD_BYTES: usize = 50 * 1024;

/// languages.yml 의 `type`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LanguageKind {
    Programming,
    Markup,
    Data,
    Prose,
}

/// languages.yml 의 한 언어.
#[derive(Debug)]
pub struct LanguageData {
    pub name: &'static str,
    /// 파일 시스템에 쓸 수 없는 글자가 이름에 있을 때의 이름(`F*` → `Fstar`). samples 폴더 이름이 이것이다.
    pub fs_name: Option<&'static str>,
    pub kind: LanguageKind,
    pub color: Option<&'static str>,
    pub group: Option<&'static str>,
    /// popular.yml 에 있다.
    pub popular: bool,
    /// 첫 요소는 기본 별칭(이름을 소문자로, 공백을 `-` 로).
    pub aliases: &'static [&'static str],
    pub extensions: &'static [&'static str],
    pub filenames: &'static [&'static str],
    pub interpreters: &'static [&'static str],
}

/// 빌드 스크립트가 옮기지 못한 정규식(또는 그 때문에 빠진 heuristics 규칙 — `pattern` 이 비어 있다).
#[derive(Debug)]
pub struct SkippedPattern {
    pub file: &'static str,
    pub context: &'static str,
    pub pattern: &'static str,
    pub reason: &'static str,
}

pub(crate) struct Disambiguation {
    extensions: &'static [&'static str],
    rules: &'static [Rule],
}

pub(crate) struct Rule {
    languages: &'static [u16],
    cond: Cond,
}

pub(crate) enum Cond {
    Always,
    Pattern(u16),
    Negative(u16),
    And(&'static [Cond]),
}

/// 표의 언어 번호.
pub type LangId = u16;

pub fn language(id: LangId) -> &'static LanguageData {
    &LANGUAGES[usize::from(id)]
}

pub fn languages() -> &'static [LanguageData] {
    &LANGUAGES
}

fn lookup(index: &'static [(&'static str, &'static [u16])], key: &str) -> &'static [u16] {
    index
        .binary_search_by(|(k, _)| (*k).cmp(key))
        .map_or(&[], |i| index[i].1)
}

/// `Language.find_by_alias`: 별칭(소문자)으로, 없으면 첫 `,` 앞으로 한 번 더.
pub fn find_by_alias(name: &str) -> Option<LangId> {
    if name.is_empty() {
        return None;
    }
    let by = |s: &str| lookup(&ALIASES, &s.to_lowercase()).first().copied();
    by(name).or_else(|| by(name.split(',').next().unwrap_or(name)))
}

/// `Language[]`: 이름 · `fs_name` · 별칭(소문자)으로.
pub fn find_by_name(name: &str) -> Option<LangId> {
    let lower = name.to_lowercase();
    lookup(&NAMES, &lower)
        .first()
        .or_else(|| lookup(&ALIASES, &lower).first())
        .copied()
}

/// 판별 단계의 이름.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strategy {
    Modeline,
    Filename,
    Shebang,
    Extension,
    Xml,
    Manpage,
    Heuristics,
    /// 후보가 여럿 남아 [`pick`] 으로 골랐다(Linguist 는 여기서 분류기를 쓴다).
    Ambiguous,
}

/// `path` 는 `/` 로 가른 경로(저장소 기준이거나 파일 이름). `head` 는 파일 앞부분 — 없거나 비었으면 내용을 보는 단계
/// (모드라인 · shebang · XML)를 건너뛰고 heuristics 는 빈 내용으로 돌린다(Linguist 는 빈 파일을 판별하지 않는다).
pub fn detect(path: &str, head: Option<&[u8]>) -> Option<(LangId, Strategy)> {
    let name = path.rsplit('/').next().unwrap_or(path);
    let data = head
        .map(|h| &h[..h.len().min(HEAD_BYTES)])
        .filter(|h| !h.is_empty());
    let text = data.map(String::from_utf8_lossy);
    let text = text.as_deref();
    let mut run = Run::default();

    if let Some(t) = text
        && run.step(modeline(t).into_iter().collect(), Strategy::Modeline)
    {
        return run.done();
    }
    let found = narrow(&run.candidates, lookup(&FILENAMES, name));
    if run.step(found, Strategy::Filename) {
        return run.done();
    }
    if let Some(t) = text {
        let found = shebang(t).map_or_else(Vec::new, |i| {
            narrow(&run.candidates, lookup(&INTERPRETERS, &i))
        });
        if run.step(found, Strategy::Shebang) {
            return run.done();
        }
    }
    let found = if is_generic(path) {
        run.candidates.clone()
    } else {
        narrow(&run.candidates, extension_languages(name).1)
    };
    if run.step(found, Strategy::Extension) {
        return run.done();
    }
    // XML · manpage 는 후보가 있으면 그대로 넘긴다(`return candidates if candidates.any?`).
    if run.candidates.is_empty() {
        let xml = text.is_some_and(|t| first_lines(t, 2).contains("xml version="));
        let found = if xml {
            find_by_name("XML").into_iter().collect()
        } else {
            Vec::new()
        };
        if run.step(found, Strategy::Xml) {
            return run.done();
        }
        let found = if is_manpage(name) {
            ["Roff Manpage", "Roff"]
                .iter()
                .filter_map(|n| find_by_name(n))
                .collect()
        } else {
            Vec::new()
        };
        if run.step(found, Strategy::Manpage) {
            return run.done();
        }
    }
    // 앞부분이 없거나 비어도 빈 내용으로 돌린다 — 묶음의 기본 규칙(`.md` 의 `\A\z` → Markdown, `.txt` 의 마지막 Text 등)이
    // 대표 언어를 고른다. Linguist 는 빈 파일을 판별하지 않으므로 이 경로는 Pitwall 이 정한 것이다.
    if run.step(heuristics(path, data.unwrap_or(&[])), Strategy::Heuristics) {
        return run.done();
    }
    let first = *run.candidates.first()?;
    if run.candidates.len() == 1 {
        return Some((first, run.last?));
    }
    Some((pick(&run.candidates, name), Strategy::Ambiguous))
}

/// `Linguist.detect` 의 반복: 후보가 하나면 멈추고, 여럿이면 다음 단계로 넘기고, 없으면 앞 후보를 그대로 둔다.
#[derive(Default)]
struct Run {
    candidates: Vec<LangId>,
    /// 지금 후보를 낸 단계.
    last: Option<Strategy>,
}

impl Run {
    fn step(&mut self, found: Vec<LangId>, strategy: Strategy) -> bool {
        if found.is_empty() {
            return false;
        }
        self.last = Some(strategy);
        self.candidates = found;
        self.candidates.len() == 1
    }

    fn done(&self) -> Option<(LangId, Strategy)> {
        Some((*self.candidates.first()?, self.last?))
    }
}

/// `candidates.any? ? candidates & found : found` — 순서는 앞 후보의 순서다.
fn narrow(candidates: &[LangId], found: &[u16]) -> Vec<LangId> {
    if candidates.is_empty() {
        found.to_vec()
    } else {
        candidates
            .iter()
            .copied()
            .filter(|c| found.contains(c))
            .collect()
    }
}

/// 분류기 대신 고른다. 앞의 것부터: 그 확장자를 대표 확장자(첫 확장자)로 적었고 popular.yml 에 있는 언어, 대표 확장자로
/// 적은 언어, popular 언어, 첫 후보. 내용이 없을 때 `.rs` 가 RenderScript(둘 다 `.rs` 가 대표 확장자, 이름순으로 앞)가 아니라
/// Rust 가 되게 popular 를 본다.
fn pick(candidates: &[LangId], name: &str) -> LangId {
    let (ext, _) = extension_languages(name);
    let primary = |id: LangId| {
        ext.is_some_and(|ext| {
            language(id)
                .extensions
                .first()
                .is_some_and(|e| e.eq_ignore_ascii_case(ext))
        })
    };
    let popular = |id: LangId| language(id).popular;
    let tiers: [&dyn Fn(LangId) -> bool; 3] =
        [&|id| primary(id) && popular(id), &primary, &popular];
    tiers
        .iter()
        .find_map(|tier| candidates.iter().copied().find(|&id| tier(id)))
        .unwrap_or(candidates[0])
}

/// `Blob#extensions`(`a.html.erb` → `.html.erb`, `.erb`) 중 색인에 있는 첫 것과 그 언어들.
pub fn extension_languages(name: &str) -> (Option<&'static str>, &'static [u16]) {
    let lower = name.to_lowercase();
    let mut rest = lower.as_str();
    // 첫 `.` 앞은 확장자가 아니다(`.bashrc` 는 `.bashrc` 하나다 — 앞이 빈 문자열이다).
    let Some(dot) = rest.find('.') else {
        return (None, &[]);
    };
    rest = &rest[dot..];
    loop {
        let hit = EXTENSIONS.binary_search_by(|(k, _)| (*k).cmp(rest));
        if let Ok(i) = hit {
            return (Some(EXTENSIONS[i].0), EXTENSIONS[i].1);
        }
        match rest[1..].find('.') {
            Some(next) => rest = &rest[next + 1..],
            None => return (None, &[]),
        }
    }
}

fn is_generic(path: &str) -> bool {
    let lower = path.to_lowercase();
    GENERIC_EXTENSIONS.iter().any(|e| lower.ends_with(e))
}

/// `Strategy::Manpage::MANPAGE_EXTS` = `\.(?:[1-9](?![0-9])[a-z_0-9]*|0p|n|man|mdoc)(?:\.in)?$`(대소문자 무시).
fn is_manpage(name: &str) -> bool {
    let lower = name.to_lowercase();
    let ok = |ext: &str| {
        let b = ext.as_bytes();
        let numbered = matches!(b.first(), Some(b'1'..=b'9'))
            && !b.get(1).is_some_and(u8::is_ascii_digit)
            && b.iter()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'_');
        numbered || matches!(ext, "0p" | "n" | "man" | "mdoc")
    };
    let last_ext = |s: &str| s.rfind('.').map(|i| s[i + 1..].to_owned());
    if last_ext(&lower).is_some_and(|e| ok(&e)) {
        return true;
    }
    lower
        .strip_suffix(".in")
        .and_then(last_ext)
        .is_some_and(|e| ok(&e))
}

fn first_lines(text: &str, n: usize) -> String {
    text.lines().take(n).collect::<Vec<_>>().join("\n")
}

fn last_lines(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

/// `Shebang.interpreter`: `#!` 줄에서 인터프리터 이름.
pub fn shebang(text: &str) -> Option<String> {
    let first = text.split('\n').next()?;
    let rest = first.strip_prefix("#!")?;
    let rest = rest.trim_start();
    let path_len = rest.find(char::is_whitespace).unwrap_or(rest.len());
    if path_len == 0 {
        return None;
    }
    let path = &rest[..path_len];
    let mut after = &rest[path_len..];
    let mut script = path.rsplit('/').next().unwrap_or(path).to_owned();
    if script == "env" {
        static OPTION: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(r"^(?:(?:-[i0uCSv]*|--\S+)\s+)+").unwrap());
        static ASSIGN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(?:\S+=\S+\s+)+").unwrap());
        after = after.trim_start();
        loop {
            if let Some(m) = OPTION.find(after) {
                after = &after[m.end()..];
            } else if let Some(m) = ASSIGN.find(after) {
                after = &after[m.end()..];
            } else {
                break;
            }
        }
        let end = after.find(char::is_whitespace).unwrap_or(after.len());
        if end == 0 {
            return None;
        }
        script = after[..end].to_owned();
        after = &after[end..];
    }
    // "python2.6" → "python2"
    static VERSION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\.\d+$").unwrap());
    script = VERSION.replace(&script, "").into_owned();
    if script == "sh" {
        static EXEC: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(r#"exec (\w+)[\s"']+\$0[\s"']+\$@"#).unwrap());
        if let Some(c) = text.lines().take(5).find_map(|l| EXEC.captures(l)) {
            script = c[1].to_owned();
        }
    }
    // osascript 의 `-l <언어>` 는 인터프리터가 있는 언어가 아닐 수 있다 — 다음 단계에 맡긴다.
    if script == "osascript" && after.contains("-l") {
        return None;
    }
    Some(script.rsplit('/').next().unwrap_or(&script).to_owned())
}

/// `Strategy::Modeline`: 앞 5줄 + 끝 5줄에서 Emacs, 없으면 Vim 모드라인의 이름을 별칭으로 찾는다.
pub fn modeline(text: &str) -> Option<LangId> {
    let header = first_lines(text, 5);
    // Vimball 파일의 모드라인은 파일 유형이 아니다.
    if header.contains("UseVimball") {
        return None;
    }
    let scope = format!("{header}\n{}", last_lines(text, 5));
    let name = scope
        .lines()
        .find_map(emacs_mode)
        .or_else(|| scope.lines().find_map(vim_filetype))?;
    find_by_alias(&name)
}

/// `Strategy::Modeline::EMACS_MODELINE` 을 손으로 옮긴 것(앞뒤 보기가 있어 `regex` 로 옮길 수 없다).
/// `-*- ruby -*-`(짧은 꼴), `-*- foo: bar; mode: ruby -*-`(긴 꼴). 같은 줄에 닫는 `-*-` 가 있어야 한다.
fn emacs_mode(line: &str) -> Option<String> {
    const MARK: &str = "-*-";
    let mut from = 0;
    while let Some(open) = line[from..].find(MARK).map(|i| from + i) {
        from = open + 1;
        let body_start = open + MARK.len();
        let Some(close) = line[body_start..].find(MARK).map(|i| body_start + i) else {
            continue;
        };
        let inner = &line[body_start..close];
        let is_name = |c: char| !(c == ':' || c == ';' || c.is_whitespace());
        // 이름 뒤에 바로 `-*-` 가 붙으면(`rest` 가 비었다) 이름이 `-` · `*` 로 끝나면 안 된다(`ruby--*-`).
        let touching_ok = |name: &str, rest: &str| rest.is_empty() && !name.ends_with(['-', '*']);
        // 짧은 꼴: 공백 · 이름 · 공백뿐.
        let short = inner.trim_start_matches([' ', '\t']);
        let name_len = short.find(|c| !is_name(c)).unwrap_or(short.len());
        if name_len > 0 && short[name_len..].trim_matches([' ', '\t']).is_empty() {
            let (name, rest) = short.split_at(name_len);
            if !rest.is_empty() || touching_ok(name, rest) {
                return Some(name.to_owned());
            }
        }
        // 긴 꼴: `mode` 앞은 줄 머리(여는 `-*-` 바로 뒤) · 공백 · `;` 이다.
        let lower = inner.to_ascii_lowercase();
        let mut at = 0;
        while let Some(i) = lower[at..].find("mode").map(|i| at + i) {
            at = i + 1;
            if i > 0 && !matches!(inner.as_bytes()[i - 1], b' ' | b'\t' | b';') {
                continue;
            }
            let rest = inner[i + 4..].trim_start_matches([' ', '\t']);
            let Some(rest) = rest.strip_prefix(':') else {
                continue;
            };
            let rest = rest.trim_start_matches([' ', '\t']);
            let len = rest.find(|c| !is_name(c)).unwrap_or(rest.len());
            if len == 0 {
                continue;
            }
            let (name, after) = rest.split_at(len);
            if after.starts_with([' ', '\t', ';']) || touching_ok(name, after) {
                return Some(name.to_owned());
            }
        }
    }
    None
}

/// `Strategy::Modeline::VIM_MODELINE` 을 손으로 옮긴 것. `vim: set ft=ruby:` · `vi: ft=ruby` · `ex: syntax=ruby` 등.
fn vim_filetype(line: &str) -> Option<String> {
    // 여는 말: 줄 머리나 공백 뒤의 `vi` · `vim` · `Vim` · `vim<703` 등, 또는 공백 뒤의 `ex`. 그리고 `:`.
    static OPENER: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?:^|[ \t])(?:vi(?:m[<=>]?[0-9]+|m)?|Vim(?:[<=>]?[0-9]+)?)(:)|[ \t]ex(:)")
            .unwrap()
    });
    static SET_FORM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[ \t]*set?[ \t]").unwrap());
    static SET_FORM_OK: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^[ \t]*set?[ \t][^\r\n:]+:").unwrap());
    // 앞에 오는 다른 옵션들(`noai`, `ts=4`, `titlestring=\ x`)이 이 꼴이어야 한다.
    static OPTIONS: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"^(?:(?:[ \t]*:[ \t]*|[ \t])(?-u:\w)*(?:[ \t]*=(?:[^\\\s]|\\.)*)?)*$").unwrap()
    });
    static FILETYPE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"[ \t:](?:filetype|ft|syntax)[ \t]*=((?-u:\w)+)").unwrap());
    // 한 줄에서 보는 여는 말 · `ft=` 후보 수의 상한. 원본 정규식은 백트래킹으로 모든 조합을 보는데, 그대로 옮기면 여는 말마다
    // 줄 끝까지 다시 훑어 줄 길이에 대해 2제곱이다(50 KiB 의 `" ex: ft=a,"` 반복 한 줄이 debug 에서 수십 초). 실제 모드라인은
    // 한 줄에 여는 말 하나 · `ft=` 하나다.
    const MAX_OPENERS: usize = 4;
    const MAX_CANDIDATES: usize = 4;
    for opener in OPENER.captures_iter(line).take(MAX_OPENERS) {
        let colon = opener.get(1).or_else(|| opener.get(2))?.start();
        let after_colon = &line[colon + 1..];
        if SET_FORM.is_match(after_colon) && !SET_FORM_OK.is_match(after_colon) {
            continue;
        }
        let body = &line[colon..];
        let mut search = 0;
        let mut checked = 0;
        while checked < MAX_CANDIDATES
            && let Some(c) = FILETYPE.captures_at(body, search)
        {
            let whole = c.get(0)?;
            search = whole.start() + 1;
            checked += 1;
            let value = c.get(1)?;
            let next = body[value.end()..].chars().next();
            if !next.is_none_or(|n| n.is_whitespace() || n == ':') {
                continue;
            }
            if OPTIONS.is_match(&body[..whole.start()]) {
                return Some(value.as_str().to_owned());
            }
        }
    }
    None
}

fn pattern(index: u16) -> &'static BytesRegex {
    static COMPILED: [OnceLock<BytesRegex>; HEURISTIC_PATTERN_COUNT] =
        [const { OnceLock::new() }; HEURISTIC_PATTERN_COUNT];
    let i = usize::from(index);
    COMPILED[i].get_or_init(|| {
        BytesRegex::new(HEURISTIC_PATTERNS[i]).expect("the build script compiled every pattern")
    })
}

fn matches(cond: &Cond, data: &[u8]) -> bool {
    match cond {
        Cond::Always => true,
        Cond::Pattern(i) => pattern(*i).is_match(data),
        Cond::Negative(i) => !pattern(*i).is_match(data),
        Cond::And(parts) => parts.iter().all(|c| matches(c, data)),
    }
}

/// `Heuristics.call`: 확장자가 맞는 첫 묶음의 규칙 중 처음 맞는 것의 언어. 묶음이 맞으면 규칙이 하나도 안 맞아도 거기서 끝난다.
fn heuristics(path: &str, data: &[u8]) -> Vec<LangId> {
    let lower = path.to_lowercase();
    let Some(d) = DISAMBIGUATIONS
        .iter()
        .find(|d| d.extensions.iter().any(|e| lower.ends_with(e)))
    else {
        return Vec::new();
    };
    d.rules
        .iter()
        .find(|r| matches(&r.cond, data))
        .map_or_else(Vec::new, |r| r.languages.to_vec())
}

/// vendor.yml 의 패턴을 하나로 묶은 것(Linguist 의 `VendoredRegexp`).
pub fn vendored_path(path: &str) -> bool {
    static RE: LazyLock<Regex> = LazyLock::new(|| combined(&VENDOR_PATTERNS));
    RE.is_match(path)
}

/// documentation.yml 의 패턴을 하나로 묶은 것(Linguist 의 `DocumentationRegexp`).
pub fn documentation_path(path: &str) -> bool {
    static RE: LazyLock<Regex> = LazyLock::new(|| combined(&DOCUMENTATION_PATTERNS));
    RE.is_match(path)
}

fn combined(patterns: &[&str]) -> Regex {
    let parts: Vec<String> = patterns.iter().map(|p| format!("(?:{p})")).collect();
    Regex::new(&parts.join("|")).expect("the build script compiled every pattern")
}
