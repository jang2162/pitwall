//! 파일 하나를 파싱해 정의 쿼리로 심볼을 뽑는다.
//!
//! - 매치마다 `@name` 노드와 `@definition.<종류>` 노드를 짝짓는다. 둘 중 하나가 없는 매치(`@reference.*` 등)는 버린다.
//! - **겹침**: 같은 이름 노드를 여러 패턴이 정의로 잡으면 쿼리 파일에서 앞의 패턴이 이긴다 — tree-sitter 의 tags 와 같다
//!   (tree-sitter-rust 는 impl 안의 함수를 `method` 패턴이 먼저, `function` 패턴이 뒤에 잡는다).
//! - **컨테이너**: 정의 범위가 이 심볼의 정의 범위를 엄격히 품는 가장 안쪽 심볼이다(같은 범위는 품지 않는다 — Java 의
//!   `int a, b;` 의 두 필드는 같은 선언을 정의 범위로 갖는다).
//! - **지역 값은 버린다**: 함수 · 메서드 · 생성자 안의 상수 · 속성 · 변수 · 필드(tree-sitter-kotlin 은 함수 안 `val` 도
//!   `property_declaration` 이라 정의로 잡는다). 함수 안의 함수 · 타입은 둔다.
//! - **위치**: 줄은 0 부터, 글자는 줄 안의 UTF-16 코드 단위(LSP 의 기본 위치 인코딩 · 에디터 문서의 단위)다. 겹침을 지운 뒤 남은
//!   심볼의 위치만, 텍스트를 한 번 훑어 구한다([`utf16_columns`]) — 한 줄짜리 1 MiB 파일에서도 선형이다.
//! - **시한**: 파싱과 쿼리 매칭을 합쳐 [`PARSE_BUDGET`] 이다. 넘거나 `stop` 이 켜지면 그 파일의 심볼은 없다(일부만 내지 않는다).
//! - 이름은 `@name` 노드의 텍스트다. 공백 · 줄바꿈 연속은 공백 하나로 접고, [`MAX_NAME_BYTES`] 를 넘으면 그 심볼을 버린다.

use std::ops::ControlFlow;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use tree_sitter::{
    Node, ParseOptions, ParseState, Parser, QueryCursor, QueryCursorOptions, QueryCursorState,
    StreamingIterator,
};

use crate::kind::{IndexSymbolKind, in_type, refine};
use crate::lang::TagsLanguage;

/// 이름의 바이트 상한. 넘는 것(긴 제네릭 impl 대상 등)은 심볼로 두지 않는다.
pub const MAX_NAME_BYTES: usize = 256;
/// 파일 하나의 파싱 · 쿼리 매칭 시한. 넘으면 그 파일의 심볼은 없다(병적인 입력이 색인 전체를 붙잡지 않게).
pub const PARSE_BUDGET: Duration = Duration::from_secs(2);
/// 한 파일에서 뽑는 심볼 수 상한(1 MiB 파일에 수만 개면 생성 코드다).
pub const MAX_SYMBOLS_PER_FILE: usize = 20_000;

/// 줄 · 글자(UTF-16) 범위. `[start_line, start_char, end_line, end_char]`.
pub(crate) type Span = [u32; 4];

/// 심볼 하나(이름은 [`Extracted::names`] 안의 바이트 범위).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Sym {
    pub name_at: u32,
    pub name_len: u16,
    pub kind: IndexSymbolKind,
    /// 컨테이너 심볼의 색인. 없으면 [`NO_PARENT`].
    pub parent: u32,
    /// 정의 전체.
    pub range: Span,
    /// 이름.
    pub selection: Span,
}

pub(crate) const NO_PARENT: u32 = u32::MAX;

/// 파일 하나의 심볼. 문서 순서(정의 시작 위치, 같으면 바깥 것 먼저).
#[derive(Debug, Default)]
pub(crate) struct Extracted {
    pub names: Box<str>,
    pub symbols: Box<[Sym]>,
}

impl Extracted {
    pub fn name(&self, sym: &Sym) -> &str {
        let at = sym.name_at as usize;
        &self.names[at..at + usize::from(sym.name_len)]
    }

    /// 메모리(힙) 크기의 근사.
    pub fn heap_bytes(&self) -> usize {
        self.names.len() + self.symbols.len() * std::mem::size_of::<Sym>()
    }
}

/// 스레드 하나의 파서 · 커서. 언어가 바뀔 때만 문법을 바꾼다.
#[derive(Default)]
pub(crate) struct Extractor {
    parser: Parser,
    cursor: QueryCursor,
    current: Option<std::sync::Arc<str>>,
}

/// 뽑는 중의 심볼(위치는 아직 바이트 · 줄).
struct Raw {
    pattern: usize,
    name: (usize, usize),
    def: (usize, usize),
    kind: IndexSymbolKind,
    /// 정의 · 이름의 시작 · 끝 줄.
    rows: [u32; 4],
}

impl Extractor {
    /// `text` 의 심볼. `stop` 이 켜지거나 [`PARSE_BUDGET`] 을 넘으면 `None`.
    pub fn extract(
        &mut self,
        language: &TagsLanguage,
        text: &str,
        stop: &AtomicBool,
    ) -> Option<Extracted> {
        self.extract_within(language, text, stop, PARSE_BUDGET)
    }

    /// [`Self::extract`] 를 시한 `budget` 으로(시험이 쿼리 매칭의 시한 갈래를 가르려고 쓴다).
    pub(crate) fn extract_within(
        &mut self,
        language: &TagsLanguage,
        text: &str,
        stop: &AtomicBool,
        budget: Duration,
    ) -> Option<Extracted> {
        if self.current.as_deref() != Some(&*language.id) {
            self.parser.set_language(&language.ts).ok()?;
            self.current = Some(std::sync::Arc::clone(&language.id));
        }
        let deadline = Instant::now() + budget;
        let bytes = text.as_bytes();
        let mut progress = |_: &ParseState| {
            if stop.load(Ordering::Relaxed) || Instant::now() > deadline {
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            }
        };
        let tree = self.parser.parse_with_options(
            &mut |offset, _| bytes.get(offset..).unwrap_or_default(),
            None,
            Some(ParseOptions::new().progress_callback(&mut progress)),
        );
        let Some(tree) = tree else {
            self.parser.reset();
            return None;
        };
        let mut over = false;
        let mut query_progress = |_: &QueryCursorState| {
            if stop.load(Ordering::Relaxed) || Instant::now() > deadline {
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            }
        };
        let mut raw: Vec<Raw> = Vec::new();
        let mut matches = self.cursor.matches_with_options(
            &language.query,
            tree.root_node(),
            bytes,
            QueryCursorOptions::new().progress_callback(&mut query_progress),
        );
        let mut seen = 0usize;
        while let Some(m) = matches.next() {
            // 진행 콜백은 커서가 노드를 지날 때만 불린다 — 매치 256개마다(첫 매치 포함)도 본다.
            if seen.is_multiple_of(256)
                && (stop.load(Ordering::Relaxed) || Instant::now() > deadline)
            {
                over = true;
                break;
            }
            seen += 1;
            let mut name: Option<Node<'_>> = None;
            let mut def = None;
            for c in m.captures() {
                if c.index == language.name {
                    name = Some(c.node);
                } else if let Some(capture) = language.defs.get(c.index as usize).copied().flatten()
                {
                    def = Some((c.node, capture));
                }
            }
            let (Some(name), Some((node, capture))) = (name, def) else {
                continue;
            };
            let row = |r: usize| u32::try_from(r).unwrap_or(u32::MAX);
            raw.push(Raw {
                pattern: m.pattern_index,
                name: (name.start_byte(), name.end_byte()),
                def: (node.start_byte(), node.end_byte()),
                kind: refine(capture, node),
                rows: [
                    row(node.start_position().row),
                    row(node.end_position().row),
                    row(name.start_position().row),
                    row(name.end_position().row),
                ],
            });
            if raw.len() > MAX_SYMBOLS_PER_FILE * 4 {
                break;
            }
        }
        drop(matches);
        // 쿼리가 시한 · 취소로 멈췄으면(진행 콜백이 끊으면 매치가 그냥 끝난다) 일부만 내지 않는다.
        if over || stop.load(Ordering::Relaxed) || Instant::now() > deadline {
            return None;
        }
        Some(finish(text, raw))
    }
}

/// 겹침을 지우고, 문서 순서로 늘어놓고, 컨테이너를 잇는다.
fn finish(text: &str, mut raw: Vec<Raw>) -> Extracted {
    // 같은 이름 노드는 앞 패턴 하나만.
    raw.sort_by(|a, b| a.name.cmp(&b.name).then(a.pattern.cmp(&b.pattern)));
    raw.dedup_by(|b, a| a.name == b.name);
    // 정의 시작 순, 같으면 바깥(긴) 것 먼저.
    raw.sort_by(|a, b| {
        a.def
            .0
            .cmp(&b.def.0)
            .then(b.def.1.cmp(&a.def.1))
            .then(a.name.cmp(&b.name))
    });
    let mut names = String::new();
    let mut symbols: Vec<Sym> = Vec::with_capacity(raw.len().min(MAX_SYMBOLS_PER_FILE));
    // 열린 정의들(바깥 → 안): (정의 바이트 범위, 심볼 색인).
    let mut open: Vec<((usize, usize), u32)> = Vec::new();
    // 심볼마다 (정의 시작 · 끝, 이름 시작 · 끝) 바이트.
    let mut offsets: Vec<[usize; 4]> = Vec::with_capacity(symbols.capacity());
    // 버린 심볼(이름이 깨짐)은 컨테이너도 되지 않는다.
    for r in raw {
        if symbols.len() >= MAX_SYMBOLS_PER_FILE {
            break;
        }
        let Some(name) = clean_name(&text[r.name.0..r.name.1]) else {
            continue;
        };
        while open
            .last()
            .is_some_and(|(d, _)| !(d.0 <= r.def.0 && r.def.1 <= d.1 && *d != r.def))
        {
            open.pop();
        }
        let parent = open.last().map_or(NO_PARENT, |(_, i)| *i);
        let container = (parent != NO_PARENT).then(|| symbols[parent as usize].kind);
        if local_value(r.kind, container) {
            continue;
        }
        let index = u32::try_from(symbols.len()).unwrap_or(NO_PARENT);
        symbols.push(Sym {
            name_at: u32::try_from(names.len()).unwrap_or(u32::MAX),
            name_len: u16::try_from(name.len()).unwrap_or(u16::MAX),
            kind: in_type(r.kind, container),
            parent,
            range: [r.rows[0], 0, r.rows[1], 0],
            selection: [r.rows[2], 0, r.rows[3], 0],
        });
        offsets.push([r.def.0, r.def.1, r.name.0, r.name.1]);
        names.push_str(&name);
        open.push((r.def, index));
    }
    // 남은 심볼의 글자 위치를 한 번에.
    let mut wanted: Vec<usize> = offsets.iter().flatten().copied().collect();
    wanted.sort_unstable();
    wanted.dedup();
    let columns = utf16_columns(text, &wanted);
    let column = |byte: usize| columns[wanted.binary_search(&byte).unwrap_or(0)];
    for (sym, at) in symbols.iter_mut().zip(&offsets) {
        sym.range[1] = column(at[0]);
        sym.range[3] = column(at[1]);
        sym.selection[1] = column(at[2]);
        sym.selection[3] = column(at[3]);
    }
    Extracted {
        names: names.into_boxed_str(),
        symbols: symbols.into_boxed_slice(),
    }
}

/// 함수 안의 값(지역 변수)인가.
fn local_value(kind: IndexSymbolKind, container: Option<IndexSymbolKind>) -> bool {
    use IndexSymbolKind as K;
    matches!(kind, K::Constant | K::Property | K::Variable | K::Field)
        && matches!(container, Some(K::Function | K::Method | K::Constructor))
}

/// 공백 연속을 하나로 접은 이름. 비었거나 [`MAX_NAME_BYTES`] 를 넘으면 `None`.
fn clean_name(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.len() > MAX_NAME_BYTES * 4 {
        return None;
    }
    let name = if trimmed.contains(char::is_whitespace) {
        trimmed.split_whitespace().collect::<Vec<_>>().join(" ")
    } else {
        trimmed.to_owned()
    };
    (name.len() <= MAX_NAME_BYTES).then_some(name)
}

/// 오름차순 바이트 위치들의 줄 안 UTF-16 글자 위치. 텍스트를 앞에서 한 번만 훑는다 — O(텍스트 + 위치 수). 글자 경계가 아닌
/// 위치(있을 수 없지만)는 그 글자의 시작으로 본다.
pub(crate) fn utf16_columns(text: &str, sorted: &[usize]) -> Vec<u32> {
    let mut out = Vec::with_capacity(sorted.len());
    let mut want = sorted.iter().copied().peekable();
    let mut column = 0u32;
    for (at, c) in text.char_indices() {
        while want.next_if(|&b| b <= at).is_some() {
            out.push(column);
        }
        if want.peek().is_none() {
            return out;
        }
        if c == '\n' {
            column = 0;
        } else {
            column = column.saturating_add(c.len_utf16() as u32);
        }
    }
    // 텍스트 끝(과 그 뒤).
    out.extend(want.map(|_| column));
    out
}
