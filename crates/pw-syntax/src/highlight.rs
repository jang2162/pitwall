//! 구문 트리와 강조 쿼리에서 겹치지 않는 토큰 구간을 만든다.
//!
//! 규칙은 tree-sitter 의 강조기(`tree-sitter-highlight` 0.27)와 같다 — 내장 문법의 쿼리가 그 규칙을 전제로 쓰였다:
//! - 같은 노드를 여러 패턴이 잡으면 쿼리 파일에서 **뒤의 패턴**이 이긴다(그래서 쿼리는 `(identifier) @variable` 같은 넓은 규칙을
//!   먼저, 좁은 규칙을 뒤에 둔다). 강조가 아닌 캡처([`CaptureKind::Ignore`] — `@spell` 등)는 노드를 차지하지 않는다 — 강조기는
//!   그런 캡처가 뒤에 오면 그 노드의 강조를 지우지만, `(comment) @comment @spell` 처럼 강조와 함께 다는 이름이라 따르지 않는다.
//! - 잡힌 노드끼리 겹치면 안쪽(자식)이 이긴다 — 문자열 안의 이스케이프는 `string.escape` 다. `@none` 은 토큰 없이 안쪽을 차지해
//!   바깥 강조를 덮는다.

use std::cell::Cell;
use std::collections::HashMap;
use std::ops::{ControlFlow, Range};

use tree_sitter::{
    Query, QueryCursor, QueryCursorOptions, QueryCursorState, StreamingIterator as _, Tree,
};

use crate::token::{CaptureKind, SyntaxToken};

/// `range`(바이트) 안의 토큰 구간. 겹치지 않고 앞에서부터 차례이며, 같은 토큰이 이어지면 하나로 합친다. `range` 밖으로 나가는
/// 노드는 `range` 에 맞춰 자른다. `progress` 가 `Break` 를 돌려주면 그만두고 `None`(쿼리 실행에 걸리는 시간의 상한).
pub fn spans(
    query: &Query,
    kinds: &[CaptureKind],
    tree: &Tree,
    text: &str,
    range: Range<usize>,
    progress: &mut dyn FnMut() -> ControlFlow<()>,
) -> Option<Vec<(Range<usize>, SyntaxToken)>> {
    let mut cursor = QueryCursor::new();
    cursor.set_byte_range(range.clone());
    let stopped = Cell::new(false);
    let mut callback = |_: &QueryCursorState| {
        let flow = progress();
        stopped.set(flow.is_break());
        flow
    };
    let mut captures = cursor.captures_with_options(
        query,
        tree.root_node(),
        text.as_bytes(),
        QueryCursorOptions::new().progress_callback(&mut callback),
    );
    // 노드 id → `nodes` 의 자리. 같은 노드의 캡처는 패턴 순서로 이어서 오므로 덮어쓰면 뒤의 패턴이 이긴다.
    let mut taken: HashMap<usize, usize> = HashMap::new();
    let mut nodes: Vec<(usize, usize, Option<SyntaxToken>)> = Vec::new();
    while let Some((m, i)) = captures.next() {
        let capture = m.captures()[*i];
        let token = match kinds[capture.index as usize] {
            CaptureKind::Ignore => continue,
            CaptureKind::Plain => None,
            CaptureKind::Token(t) => Some(t),
        };
        let node = capture.node;
        if node.start_byte() >= node.end_byte() {
            continue;
        }
        match taken.get(&node.id()) {
            Some(&at) => nodes[at].2 = token,
            None => {
                taken.insert(node.id(), nodes.len());
                nodes.push((node.start_byte(), node.end_byte(), token));
            }
        }
    }
    drop(captures);
    if stopped.get() {
        return None;
    }
    // 바깥(긴 것)이 먼저. 같은 범위는 잡힌 차례 그대로(안정 정렬).
    nodes.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)));

    let mut out = Spans {
        spans: Vec::new(),
        range,
    };
    // 열린 노드(끝, 토큰). 안쪽이 위다.
    let mut open: Vec<(usize, Option<SyntaxToken>)> = Vec::new();
    let mut pos = out.range.start;
    for (start, end, token) in nodes {
        while let Some(&(top_end, top)) = open.last() {
            if top_end > start {
                break;
            }
            out.push(pos, top_end, top);
            pos = pos.max(top_end);
            open.pop();
        }
        if let Some(&(_, top)) = open.last() {
            out.push(pos, start, top);
        }
        pos = pos.max(start);
        open.push((end, token));
    }
    while let Some((end, token)) = open.pop() {
        out.push(pos, end, token);
        pos = pos.max(end);
    }
    Some(out.spans)
}

struct Spans {
    spans: Vec<(Range<usize>, SyntaxToken)>,
    range: Range<usize>,
}

impl Spans {
    fn push(&mut self, from: usize, to: usize, token: Option<SyntaxToken>) {
        let from = from.max(self.range.start);
        let to = to.min(self.range.end);
        let Some(token) = token else { return };
        if from >= to {
            return;
        }
        if let Some((last, last_token)) = self.spans.last_mut()
            && *last_token == token
            && last.end == from
        {
            last.end = to;
            return;
        }
        self.spans.push((from..to, token));
    }
}
