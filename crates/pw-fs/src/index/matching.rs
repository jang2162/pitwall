//! 퍼지 매칭과 점수(결정 M1-D4, `frizbee`).
//!
//! - 질의를 공백으로 가른 낱말이 모두 상대 경로에 맞아야 한다. 낱말마다 frizbee 의 Smith-Waterman 점수(오타 없음, 대문자가 있을
//!   때만 대소문자를 가림)를 더한다. frizbee 는 영숫자가 아닌 글자(`/` · `.` · `-` · `_`) 뒤의 일치에 보너스를 줘서 경로 구성
//!   요소의 머리가 앞선다.
//! - **파일 이름 가중**: 낱말이 파일 이름에도 맞으면 그 점수를 [`NAME_WEIGHT`] 배 더한다 — `lib` 는 `pw-lib/src/a.rs` 보다
//!   `src/lib.rs` 가 위다. 일치 위치도 파일 이름 쪽을 준다.
//! - **벤더 · 생성 낮춤**(M1-02): 그 파일의 점수는 [`LOWERED_DIVISOR`] 로 나눈다. 빼지는 않는다.
//! - 순서: 점수가 높은 것, 같으면 짧은 경로, 같으면 경로순.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use frizbee::{Config, Matcher, Pattern, PatternConfig, SortStrategy};

use super::walk::Flags;
use super::{FileMatch, SearchFiles};

/// 파일 이름 점수에 곱하는 수.
pub(crate) const NAME_WEIGHT: u32 = 1;
/// 벤더 · 생성 파일의 점수를 나누는 수.
pub(crate) const LOWERED_DIVISOR: u32 = 2;

/// 루트 하나의 목록을 매칭용으로 펼친 것(경로순).
pub(crate) struct Snapshot {
    root: PathBuf,
    paths: Vec<Arc<str>>,
    flags: Vec<Flags>,
    /// 파일 이름이 시작하는 바이트.
    names: Vec<u32>,
}

impl Snapshot {
    pub(crate) fn new(root: &Path, files: &BTreeMap<Arc<str>, Flags>) -> Self {
        let mut paths = Vec::with_capacity(files.len());
        let mut flags = Vec::with_capacity(files.len());
        let mut names = Vec::with_capacity(files.len());
        for (path, &flag) in files {
            names.push(
                path.rfind('/')
                    .map_or(0, |i| u32::try_from(i + 1).unwrap_or(0)),
            );
            paths.push(Arc::clone(path));
            flags.push(flag);
        }
        Self {
            root: root.to_path_buf(),
            paths,
            flags,
            names,
        }
    }

    fn len(&self) -> usize {
        self.paths.len()
    }

    fn name(&self, item: usize) -> &str {
        &self.paths[item][self.names[item] as usize..]
    }

    fn file(&self, item: usize, score: u32, positions: Vec<u32>) -> FileMatch {
        let relative = &self.paths[item];
        let mut path = self.root.clone();
        path.extend(relative.split('/'));
        let flags = self.flags[item];
        FileMatch {
            path: path.display().to_string(),
            root: self.root.display().to_string(),
            relative: relative.to_string(),
            positions,
            score,
            vendored: flags.vendored,
            generated: flags.generated,
        }
    }
}

/// 맞은 파일 하나.
#[derive(Clone, Copy)]
struct Hit {
    snap: usize,
    item: usize,
    score: u32,
}

fn config() -> Config {
    // 순서는 여기서 다시 정한다 — frizbee 의 정렬은 건너뛴다.
    Config::default().sort(SortStrategy::IndexAsc)
}

/// 질의 하나. `limit` 개까지.
pub(crate) fn search(snaps: &[Arc<Snapshot>], query: &str, limit: usize) -> SearchFiles {
    let indexed = snaps.iter().map(|s| s.len() as u64).sum();
    let terms: Vec<&str> = query.split_whitespace().collect();
    if terms.is_empty() {
        let files = snaps
            .iter()
            .flat_map(|s| (0..s.len()).map(move |i| s.file(i, 0, Vec::new())))
            .take(limit)
            .collect();
        return SearchFiles {
            files,
            matched: indexed,
            indexed,
        };
    }
    let config = config();
    let mut matchers: Vec<Matcher> = terms
        .iter()
        .map(|t| Matcher::new(Pattern::new(t, PatternConfig::default()), &config))
        .collect();
    let mut hits = Vec::new();
    for (si, snap) in snaps.iter().enumerate() {
        for (item, score) in score_all(snap, &mut matchers) {
            let flags = snap.flags[item];
            let score = if flags.vendored || flags.generated {
                score / LOWERED_DIVISOR
            } else {
                score
            };
            hits.push(Hit {
                snap: si,
                item,
                score,
            });
        }
    }
    let matched = hits.len() as u64;
    let order = |a: &Hit, b: &Hit| {
        let (pa, pb) = (&snaps[a.snap].paths[a.item], &snaps[b.snap].paths[b.item]);
        b.score
            .cmp(&a.score)
            .then_with(|| pa.len().cmp(&pb.len()))
            .then_with(|| pa.cmp(pb))
    };
    if hits.len() > limit {
        hits.select_nth_unstable_by(limit, order);
        hits.truncate(limit);
    }
    hits.sort_unstable_by(order);
    let files = hits
        .iter()
        .map(|hit| {
            let snap = &snaps[hit.snap];
            let positions = positions(snap, hit.item, &mut matchers);
            snap.file(hit.item, hit.score, positions)
        })
        .collect();
    SearchFiles {
        files,
        matched,
        indexed,
    }
}

/// 모든 낱말에 맞는 항목과 그 점수(경로 점수 + 파일 이름 점수 × [`NAME_WEIGHT`] 의 낱말 합).
fn score_all(snap: &Snapshot, matchers: &mut [Matcher]) -> Vec<(usize, u32)> {
    let mut found: Option<Vec<(usize, u32)>> = None;
    for matcher in matchers {
        let mut next: Vec<(usize, u32)> = match &found {
            None => matcher
                .match_list(&snap.paths)
                .into_iter()
                .map(|m| (m.index as usize, u32::from(m.score)))
                .collect(),
            Some(found) => {
                let paths: Vec<&str> = found.iter().map(|&(i, _)| &*snap.paths[i]).collect();
                matcher
                    .match_list(&paths)
                    .into_iter()
                    .map(|m| {
                        let (item, score) = found[m.index as usize];
                        (item, score + u32::from(m.score))
                    })
                    .collect()
            }
        };
        let names: Vec<&str> = next.iter().map(|&(i, _)| snap.name(i)).collect();
        for m in matcher.match_list(&names) {
            next[m.index as usize].1 += NAME_WEIGHT * u32::from(m.score);
        }
        if next.is_empty() {
            return next;
        }
        found = Some(next);
    }
    found.unwrap_or_default()
}

/// 상대 경로 안의 일치 위치(UTF-16). 낱말이 파일 이름에 맞으면 파일 이름 쪽, 아니면 경로 쪽의 정렬을 쓴다.
fn positions(snap: &Snapshot, item: usize, matchers: &mut [Matcher]) -> Vec<u32> {
    let path = &*snap.paths[item];
    let name_at = snap.names[item];
    let mut bytes = Vec::new();
    for matcher in matchers {
        if let Some(m) = matcher.match_one_indices(snap.name(item), 0) {
            bytes.extend(m.indices.iter().map(|i| i + name_at));
        } else if let Some(m) = matcher.match_one_indices(path, 0) {
            bytes.extend(m.indices);
        }
    }
    utf16_positions(path, bytes)
}

/// 바이트 위치들(frizbee 는 여러 바이트 글자의 바이트를 모두 준다)을 그 글자의 UTF-16 위치로. 오름차순, 중복 없음.
pub(crate) fn utf16_positions(s: &str, mut bytes: Vec<u32>) -> Vec<u32> {
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
