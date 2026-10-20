//! `linguist/*.yml`(GitHub Linguist, 고정 버전은 `linguist/VERSION`)을 `$OUT_DIR/linguist.rs` 의 정적 표로 바꾼다.
//!
//! - 언어 표와 확장자 · 파일명 · 인터프리터 · 별칭 색인(정렬해 두고 이분 탐색한다).
//! - heuristics: 언어 이름을 표 번호로 풀고, 정규식을 Rust `regex` 로 옮겨(`src/ruby_regex.rs`) 컴파일해 본다. 옮기지 못한
//!   대안은 빠지고, 빠지면 뜻이 달라지는 규칙(부정 패턴 · `and` 안)은 규칙째 빠진다. 빠진 것은 `SKIPPED_PATTERNS` 와
//!   빌드 로그(`cargo build -vv`, 또는 `target/*/build/pw-lang-*/output`)에 남는다.
//! - vendor · documentation: 옮긴 패턴만 묶는다. generic: 확장자 목록. popular: 언어마다 `popular` 표시.

#[path = "src/ruby_regex.rs"]
mod ruby_regex;

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

use yaml_rust2::{Yaml, YamlLoader};

struct Language {
    name: String,
    fs_name: Option<String>,
    kind: String,
    color: Option<String>,
    group: Option<String>,
    aliases: Vec<String>,
    extensions: Vec<String>,
    filenames: Vec<String>,
    interpreters: Vec<String>,
    popular: bool,
}

/// 옮긴 패턴. `partial` 이면 대안 일부가 빠졌다.
struct Pattern {
    index: usize,
    partial: bool,
}

enum Cond {
    Always,
    Pattern(usize),
    Negative(usize),
    And(Vec<Cond>),
}

/// heuristics 의 한 묶음: 확장자들과 (언어 번호들, 조건) 규칙들.
type Disambiguation = (Vec<String>, Vec<(Vec<usize>, Cond)>);

struct Skip {
    file: &'static str,
    context: String,
    pattern: String,
    reason: String,
}

fn main() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    println!("cargo:rerun-if-changed=linguist");
    println!("cargo:rerun-if-changed=src/ruby_regex.rs");

    let popular = strings(&load(&dir.join("linguist/popular.yml")));
    let mut languages = languages(&load(&dir.join("linguist/languages.yml")));
    for l in &mut languages {
        l.popular = popular.contains(&l.name);
    }
    let by_name = name_index(&languages);
    let mut skips = Vec::new();
    let mut patterns: Vec<String> = Vec::new();
    let disambiguations = heuristics(
        &load(&dir.join("linguist/heuristics.yml")),
        &by_name,
        &mut patterns,
        &mut skips,
    );
    let vendor = path_patterns(
        &load(&dir.join("linguist/vendor.yml")),
        "vendor.yml",
        &mut skips,
    );
    let documentation = path_patterns(
        &load(&dir.join("linguist/documentation.yml")),
        "documentation.yml",
        &mut skips,
    );
    let generic = strings(&load(&dir.join("linguist/generic.yml"))["extensions"]);

    let mut out = String::new();
    emit_languages(&mut out, &languages);
    emit_indexes(&mut out, &languages);
    emit_heuristics(&mut out, &patterns, &disambiguations);
    emit_strs(&mut out, "VENDOR_PATTERNS", &vendor);
    emit_strs(&mut out, "DOCUMENTATION_PATTERNS", &documentation);
    let generic: Vec<String> = generic.iter().map(|e| e.to_lowercase()).collect();
    emit_strs(&mut out, "GENERIC_EXTENSIONS", &generic);
    emit_skips(&mut out, &skips);
    for s in &skips {
        println!(
            "pw-lang: skipped {} {}: {} — `{}`",
            s.file, s.context, s.reason, s.pattern
        );
    }
    let path = Path::new(&std::env::var("OUT_DIR").unwrap()).join("linguist.rs");
    std::fs::write(path, out).unwrap();
}

fn load(path: &Path) -> Yaml {
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    YamlLoader::load_from_str(&text)
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("{} is empty", path.display()))
}

fn strings(y: &Yaml) -> Vec<String> {
    match y {
        Yaml::Array(items) => items.iter().map(scalar).collect(),
        Yaml::BadValue => Vec::new(),
        other => vec![scalar(other)],
    }
}

fn scalar(y: &Yaml) -> String {
    match y {
        Yaml::String(s) | Yaml::Real(s) => s.clone(),
        Yaml::Integer(i) => i.to_string(),
        Yaml::Boolean(b) => b.to_string(),
        other => panic!("expected a scalar, found {other:?}"),
    }
}

fn opt(y: &Yaml) -> Option<String> {
    (!y.is_badvalue() && !y.is_null()).then(|| scalar(y))
}

fn languages(doc: &Yaml) -> Vec<Language> {
    let Yaml::Hash(map) = doc else {
        panic!("languages.yml: expected a mapping");
    };
    map.iter()
        .map(|(name, attrs)| {
            let name = scalar(name);
            // Linguist 의 기본 별칭: 이름을 소문자로, 공백을 `-` 로(`Language#default_alias`).
            let default_alias: String = name
                .to_lowercase()
                .chars()
                .map(|c| if c.is_whitespace() { '-' } else { c })
                .collect();
            let mut aliases = vec![default_alias];
            aliases.extend(strings(&attrs["aliases"]));
            let mut interpreters = strings(&attrs["interpreters"]);
            interpreters.sort();
            Language {
                fs_name: opt(&attrs["fs_name"]),
                kind: opt(&attrs["type"]).unwrap_or_else(|| panic!("{name}: no type")),
                color: opt(&attrs["color"]),
                group: opt(&attrs["group"]),
                aliases,
                extensions: strings(&attrs["extensions"]),
                filenames: strings(&attrs["filenames"]),
                interpreters,
                popular: false,
                name,
            }
        })
        .collect()
}

/// `Language[]` 의 색인: 이름 · 별칭의 소문자.
fn name_index(languages: &[Language]) -> BTreeMap<String, usize> {
    let mut index = BTreeMap::new();
    for (i, l) in languages.iter().enumerate() {
        index.insert(l.name.to_lowercase(), i);
        if let Some(fs) = &l.fs_name {
            index.insert(fs.to_lowercase(), i);
        }
        for a in &l.aliases {
            index.insert(a.to_lowercase(), i);
        }
    }
    index
}

fn resolve(by_name: &BTreeMap<String, usize>, name: &str) -> usize {
    *by_name
        .get(&name.to_lowercase())
        .unwrap_or_else(|| panic!("heuristics.yml: unknown language `{name}`"))
}

/// 패턴(문자열 또는 문자열 배열)을 옮겨 `patterns` 에 넣는다. 하나도 못 옮기면 `None`.
fn add_pattern(
    y: &Yaml,
    context: &str,
    file: &'static str,
    patterns: &mut Vec<String>,
    skips: &mut Vec<Skip>,
) -> Option<Pattern> {
    let sources = strings(y);
    let mut ok = Vec::new();
    for source in &sources {
        match translate_checked(source) {
            Ok(t) => ok.push(t),
            Err(reason) => skips.push(Skip {
                file,
                context: context.to_owned(),
                pattern: source.clone(),
                reason,
            }),
        }
    }
    if ok.is_empty() {
        return None;
    }
    let partial = ok.len() < sources.len();
    let joined = if ok.len() == 1 {
        ok.pop().unwrap()
    } else {
        ruby_regex::union(ok.iter().map(String::as_str))
    };
    patterns.push(joined);
    Some(Pattern {
        index: patterns.len() - 1,
        partial,
    })
}

fn translate_checked(source: &str) -> Result<String, String> {
    let t = ruby_regex::translate(source)?;
    regex::bytes::Regex::new(&t).map_err(|e| {
        let msg = e.to_string();
        let last = msg.lines().last().unwrap_or(&msg).trim().to_owned();
        format!("rust regex: {last}")
    })?;
    Ok(t)
}

fn heuristics(
    doc: &Yaml,
    by_name: &BTreeMap<String, usize>,
    patterns: &mut Vec<String>,
    skips: &mut Vec<Skip>,
) -> Vec<Disambiguation> {
    let mut named: BTreeMap<String, Option<Pattern>> = BTreeMap::new();
    if let Yaml::Hash(map) = &doc["named_patterns"] {
        for (k, v) in map {
            let key = scalar(k);
            let p = add_pattern(
                v,
                &format!("named pattern `{key}`"),
                "heuristics.yml",
                patterns,
                skips,
            );
            named.insert(key, p);
        }
    }
    let Yaml::Array(list) = &doc["disambiguations"] else {
        panic!("heuristics.yml: no disambiguations");
    };
    list.iter()
        .map(|d| {
            let exts: Vec<String> = strings(&d["extensions"])
                .iter()
                .map(|e| e.to_lowercase())
                .collect();
            let Yaml::Array(rules) = &d["rules"] else {
                panic!("heuristics.yml: {exts:?} has no rules");
            };
            let mut kept = Vec::new();
            for (n, rule) in rules.iter().enumerate() {
                let languages: Vec<usize> = strings(&rule["language"])
                    .iter()
                    .map(|l| resolve(by_name, l))
                    .collect();
                let context = format!("{} rule {} ({})", exts.join(" "), n + 1, {
                    let names = strings(&rule["language"]);
                    names.join(", ")
                });
                match cond(rule, &context, false, &mut named, patterns, skips) {
                    Some(c) => kept.push((languages, c)),
                    None => skips.push(Skip {
                        file: "heuristics.yml",
                        context: context.clone(),
                        pattern: String::new(),
                        reason: "rule dropped (a pattern it needs could not be ported)".into(),
                    }),
                }
            }
            (exts, kept)
        })
        .collect()
}

/// 규칙의 조건. `strict` 면(부정 · `and` 안) 대안이 일부 빠진 패턴도 쓰지 않는다 — 빠지면 뜻이 넓어진다.
fn cond(
    rule: &Yaml,
    context: &str,
    strict: bool,
    named: &mut BTreeMap<String, Option<Pattern>>,
    patterns: &mut Vec<String>,
    skips: &mut Vec<Skip>,
) -> Option<Cond> {
    let usable = |p: Option<Pattern>, strict: bool| p.filter(|p| !(strict && p.partial));
    if let Yaml::Array(parts) = &rule["and"] {
        let mut conds = Vec::new();
        for part in parts {
            conds.push(cond(part, context, true, named, patterns, skips)?);
        }
        return Some(Cond::And(conds));
    }
    if !rule["pattern"].is_badvalue() {
        let p = add_pattern(&rule["pattern"], context, "heuristics.yml", patterns, skips);
        return usable(p, strict).map(|p| Cond::Pattern(p.index));
    }
    if !rule["negative_pattern"].is_badvalue() {
        let p = add_pattern(
            &rule["negative_pattern"],
            context,
            "heuristics.yml",
            patterns,
            skips,
        );
        return usable(p, true).map(|p| Cond::Negative(p.index));
    }
    if !rule["named_pattern"].is_badvalue() {
        let key = scalar(&rule["named_pattern"]);
        let p = named
            .get(&key)
            .unwrap_or_else(|| panic!("heuristics.yml: unknown named pattern `{key}`"));
        return match p {
            Some(p) if !(strict && p.partial) => Some(Cond::Pattern(p.index)),
            _ => None,
        };
    }
    Some(Cond::Always)
}

fn path_patterns(doc: &Yaml, file: &'static str, skips: &mut Vec<Skip>) -> Vec<String> {
    strings(doc)
        .into_iter()
        .filter_map(|source| match translate_checked(&source) {
            Ok(t) => Some(t),
            Err(reason) => {
                skips.push(Skip {
                    file,
                    context: "path pattern".into(),
                    pattern: source,
                    reason,
                });
                None
            }
        })
        .collect()
}

fn lit(s: &str) -> String {
    format!("{s:?}")
}

fn lits(items: &[String]) -> String {
    let parts: Vec<String> = items.iter().map(|s| lit(s)).collect();
    format!("&[{}]", parts.join(", "))
}

fn opt_lit(s: &Option<String>) -> String {
    match s {
        Some(s) => format!("Some({})", lit(s)),
        None => "None".into(),
    }
}

fn emit_languages(out: &mut String, languages: &[Language]) {
    writeln!(
        out,
        "pub(crate) static LANGUAGES: [LanguageData; {}] = [",
        languages.len()
    )
    .unwrap();
    for l in languages {
        let kind = match l.kind.as_str() {
            "programming" => "Programming",
            "markup" => "Markup",
            "data" => "Data",
            "prose" => "Prose",
            other => panic!("{}: unknown type `{other}`", l.name),
        };
        writeln!(
            out,
            "    LanguageData {{ name: {}, fs_name: {}, kind: LanguageKind::{kind}, color: {}, group: {}, popular: {}, aliases: {}, extensions: {}, filenames: {}, interpreters: {} }},",
            lit(&l.name),
            opt_lit(&l.fs_name),
            opt_lit(&l.color),
            opt_lit(&l.group),
            l.popular,
            lits(&l.aliases),
            lits(&l.extensions),
            lits(&l.filenames),
            lits(&l.interpreters),
        )
        .unwrap();
    }
    out.push_str("];\n");
}

/// 키 → 언어 번호 목록. 목록은 languages.yml 순서다(Linguist 의 색인과 같다).
fn emit_index(out: &mut String, name: &str, entries: BTreeMap<String, Vec<usize>>) {
    writeln!(
        out,
        "pub(crate) static {name}: [(&str, &[u16]); {}] = [",
        entries.len()
    )
    .unwrap();
    for (k, v) in entries {
        let ids: Vec<String> = v.iter().map(usize::to_string).collect();
        writeln!(out, "    ({}, &[{}]),", lit(&k), ids.join(", ")).unwrap();
    }
    out.push_str("];\n");
}

fn emit_indexes(out: &mut String, languages: &[Language]) {
    let mut extensions: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    let mut filenames: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    let mut interpreters: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    let mut aliases: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    let mut names: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (i, l) in languages.iter().enumerate() {
        for e in &l.extensions {
            extensions.entry(e.to_lowercase()).or_default().push(i);
        }
        for f in &l.filenames {
            filenames.entry(f.clone()).or_default().push(i);
        }
        for p in &l.interpreters {
            interpreters.entry(p.clone()).or_default().push(i);
        }
        for a in &l.aliases {
            let entry = aliases.entry(a.to_lowercase()).or_default();
            assert!(entry.is_empty(), "duplicate alias `{a}`");
            entry.push(i);
        }
        names.entry(l.name.to_lowercase()).or_default().push(i);
        if let Some(fs) = &l.fs_name {
            names.entry(fs.to_lowercase()).or_default().push(i);
        }
    }
    emit_index(out, "EXTENSIONS", extensions);
    emit_index(out, "FILENAMES", filenames);
    emit_index(out, "INTERPRETERS", interpreters);
    emit_index(out, "ALIASES", aliases);
    emit_index(out, "NAMES", names);
}

fn emit_cond(c: &Cond) -> String {
    match c {
        Cond::Always => "Cond::Always".into(),
        Cond::Pattern(i) => format!("Cond::Pattern({i})"),
        Cond::Negative(i) => format!("Cond::Negative({i})"),
        Cond::And(parts) => {
            let inner: Vec<String> = parts.iter().map(emit_cond).collect();
            format!("Cond::And(&[{}])", inner.join(", "))
        }
    }
}

fn emit_heuristics(out: &mut String, patterns: &[String], disambiguations: &[Disambiguation]) {
    emit_strs(out, "HEURISTIC_PATTERNS", patterns);
    writeln!(
        out,
        "pub(crate) const HEURISTIC_PATTERN_COUNT: usize = {};",
        patterns.len()
    )
    .unwrap();
    writeln!(
        out,
        "pub(crate) static DISAMBIGUATIONS: [Disambiguation; {}] = [",
        disambiguations.len()
    )
    .unwrap();
    for (exts, rules) in disambiguations {
        let rules: Vec<String> = rules
            .iter()
            .map(|(langs, c)| {
                let ids: Vec<String> = langs.iter().map(usize::to_string).collect();
                format!(
                    "Rule {{ languages: &[{}], cond: {} }}",
                    ids.join(", "),
                    emit_cond(c)
                )
            })
            .collect();
        writeln!(
            out,
            "    Disambiguation {{ extensions: {}, rules: &[{}] }},",
            lits(exts),
            rules.join(", ")
        )
        .unwrap();
    }
    out.push_str("];\n");
}

fn emit_strs(out: &mut String, name: &str, items: &[String]) {
    writeln!(out, "pub(crate) static {name}: [&str; {}] = [", items.len()).unwrap();
    for s in items {
        writeln!(out, "    {},", lit(s)).unwrap();
    }
    out.push_str("];\n");
}

fn emit_skips(out: &mut String, skips: &[Skip]) {
    writeln!(
        out,
        "pub static SKIPPED_PATTERNS: [SkippedPattern; {}] = [",
        skips.len()
    )
    .unwrap();
    for s in skips {
        writeln!(
            out,
            "    SkippedPattern {{ file: {}, context: {}, pattern: {}, reason: {} }},",
            lit(s.file),
            lit(&s.context),
            lit(&s.pattern),
            lit(&s.reason)
        )
        .unwrap();
    }
    out.push_str("];\n");
}
