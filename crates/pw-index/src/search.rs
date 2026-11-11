//! 심볼 이름의 퍼지 매칭(결정 D31 — `frizbee`, 파일 검색 `search.files` 와 같은 점수).
//!
//! - 질의를 공백으로 가른 낱말이 모두 이름에 맞아야 한다. 낱말마다 frizbee 의 Smith-Waterman 점수(오타 없음, 대문자가 있을 때만
//!   대소문자를 가림)를 더한다. 첫 낱말은 모든 이름에 병렬로 맞추고(`match_list_parallel`), 다음 낱말은 남은 것에만 맞춘다.
//! - 순서: 점수가 높은 것, 같으면 짧은 이름, 같으면 경로 · 위치 순.
//! - 컨테이너 이름은 맞추지 않는다(화면이 흐린 글자로 보일 뿐이다).

use std::sync::Arc;

use frizbee::{Config, Matcher, Pattern, PatternConfig, SortStrategy};

use crate::Parsed;
use crate::extract::NO_PARENT;
use crate::kind::IndexSymbolKind;
use crate::methods::{IndexRange, IndexSymbolMatch, IndexSymbolSearch};

/// 루트 하나의 색인을 질의 하나 동안 붙잡은 것(경로순).
pub(crate) struct RootView {
    pub root: std::path::PathBuf,
    pub files: Vec<(Arc<str>, Arc<Parsed>)>,
}

/// 심볼 하나의 자리.
#[derive(Clone, Copy)]
struct At {
    view: u32,
    file: u32,
    sym: u32,
}

#[derive(Clone, Copy)]
struct Hit {
    at: At,
    score: u32,
    /// 이름의 바이트 길이(순서의 두 번째 열쇠 — 비교마다 이름을 찾아가지 않게 미리 둔다).
    len: u32,
}

/// 결과 순서: 점수 높은 것, 짧은 이름, 루트 차례, 경로, 파일 안 차례. 경로는 파일 번호로 견준다 — 루트의 파일은 경로순
/// (`RootView` — `BTreeMap<Arc<str>, _>` 의 바이트 순서로 모은다)이라 같은 루트 안에서 번호 순서가 경로 문자열 순서와 같다. 첫 키처럼
/// 맞은 것이 많고 점수 · 길이가 같은 것이 많으면 문자열 비교가 질의 시간의 큰 몫이었다(15-m4-plan §3 ⌘⌥O 첫 키 행).
fn order(a: &Hit, b: &Hit) -> std::cmp::Ordering {
    b.score
        .cmp(&a.score)
        .then_with(|| a.len.cmp(&b.len))
        .then_with(|| a.at.view.cmp(&b.at.view))
        .then_with(|| a.at.file.cmp(&b.at.file))
        .then_with(|| a.at.sym.cmp(&b.at.sym))
}

fn config() -> Config {
    // 순서는 여기서 정한다 — frizbee 의 정렬은 건너뛴다.
    Config::default().sort(SortStrategy::IndexAsc)
}

pub(crate) fn search(
    threads: usize,
    views: &[RootView],
    query: &str,
    kinds: Option<&[IndexSymbolKind]>,
    offset: usize,
    limit: usize,
    unindexed: Vec<String>,
) -> IndexSymbolSearch {
    let wanted = |k: IndexSymbolKind| match kinds {
        Some(kinds) => kinds.contains(&k),
        None => k != IndexSymbolKind::Impl,
    };
    // 후보(종류로 거른 것)와 그 이름. 심볼 수만큼 미리 잡는다(50만 개를 늘려 가며 옮기지 않게).
    let total: usize = views
        .iter()
        .flat_map(|v| v.files.iter())
        .map(|(_, parsed)| parsed.symbols.symbols.len())
        .sum();
    let mut at: Vec<At> = Vec::with_capacity(total);
    let mut names: Vec<&str> = Vec::with_capacity(total);
    for (vi, view) in views.iter().enumerate() {
        for (fi, (_, parsed)) in view.files.iter().enumerate() {
            for (si, sym) in parsed.symbols.symbols.iter().enumerate() {
                if wanted(sym.kind) {
                    at.push(At {
                        view: vi as u32,
                        file: fi as u32,
                        sym: si as u32,
                    });
                    names.push(parsed.symbols.name(sym));
                }
            }
        }
    }
    let indexed = at.len() as u64;
    let terms: Vec<&str> = query.split_whitespace().collect();
    let mut hits: Vec<Hit> = if terms.is_empty() {
        at.iter()
            .zip(&names)
            .map(|(&at, name)| Hit {
                at,
                score: 0,
                len: name.len() as u32,
            })
            .collect()
    } else {
        score_all(&names, &at, &terms, threads)
    };
    drop(names);
    let matched = hits.len() as u64;
    let end = offset.saturating_add(limit);
    if hits.len() > end {
        hits.select_nth_unstable_by(end, order);
        hits.truncate(end);
    }
    hits.sort_unstable_by(order);
    let mut matchers: Vec<Matcher> = terms
        .iter()
        .map(|t| Matcher::new(Pattern::new(t, PatternConfig::default()), &config()))
        .collect();
    let symbols = hits
        .iter()
        .skip(offset)
        .map(|hit| {
            let view = &views[hit.at.view as usize];
            let (rel, parsed) = &view.files[hit.at.file as usize];
            let sym = &parsed.symbols.symbols[hit.at.sym as usize];
            let name = parsed.symbols.name(sym);
            let mut bytes = Vec::new();
            for m in &mut matchers {
                if let Some(found) = m.match_one_indices(name, 0) {
                    bytes.extend(found.indices);
                }
            }
            let mut path = view.root.clone();
            path.extend(rel.split('/'));
            IndexSymbolMatch {
                name: name.to_owned(),
                kind: sym.kind,
                container: (sym.parent != NO_PARENT).then(|| {
                    parsed
                        .symbols
                        .name(&parsed.symbols.symbols[sym.parent as usize])
                        .to_owned()
                }),
                path: path.display().to_string(),
                root: view.root.display().to_string(),
                relative: rel.to_string(),
                language: parsed.language.to_string(),
                range: IndexRange::from_span(sym.range),
                selection_range: IndexRange::from_span(sym.selection),
                positions: utf16_positions(name, bytes),
                score: hit.score,
            }
        })
        .collect();
    IndexSymbolSearch {
        symbols,
        matched,
        indexed,
        unindexed,
    }
}

/// 모든 낱말에 맞는 후보와 그 점수 합.
fn score_all(names: &[&str], at: &[At], terms: &[&str], threads: usize) -> Vec<Hit> {
    let config = config();
    let mut found: Option<Vec<(usize, u32)>> = None;
    for term in terms {
        let mut matcher = Matcher::new(Pattern::new(term, PatternConfig::default()), &config);
        let next: Vec<(usize, u32)> = match &found {
            None => matcher
                .match_list_parallel(names, threads)
                .into_iter()
                .map(|m| (m.index as usize, u32::from(m.score)))
                .collect(),
            Some(found) => {
                let subset: Vec<&str> = found.iter().map(|&(i, _)| names[i]).collect();
                matcher
                    .match_list(&subset)
                    .into_iter()
                    .map(|m| {
                        let (i, score) = found[m.index as usize];
                        (i, score + u32::from(m.score))
                    })
                    .collect()
            }
        };
        if next.is_empty() {
            return Vec::new();
        }
        found = Some(next);
    }
    found
        .unwrap_or_default()
        .into_iter()
        .map(|(i, score)| Hit {
            at: at[i],
            score,
            len: names[i].len() as u32,
        })
        .collect()
}

/// 바이트 위치들(frizbee 는 여러 바이트 글자의 바이트를 모두 준다)을 그 글자의 UTF-16 위치로. 오름차순, 중복 없음.
fn utf16_positions(s: &str, mut bytes: Vec<u32>) -> Vec<u32> {
    bytes.sort_unstable();
    bytes.dedup();
    let mut out = Vec::new();
    let mut want = bytes.into_iter().peekable();
    let mut unit = 0u32;
    for (at, c) in s.char_indices() {
        let end = (at + c.len_utf8()) as u32;
        let mut hit = false;
        while want.next_if(|&b| b < end).is_some() {
            hit = true;
        }
        if hit {
            out.push(unit);
        }
        if want.peek().is_none() {
            break;
        }
        unit += c.len_utf16() as u32;
    }
    out
}
