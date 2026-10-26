//! 그래프 레인 배치(결정 M2-D2 — 레인은 Rust 가 계산하고 UI 는 그리기만 한다). 행 순서대로 한 행씩 배치한다 — 앞 행들의 상태만
//! 보므로 첫 청크를 전체 이력 배치 전에 보낼 수 있다.
//!
//! 레인은 「다음에 올 커밋 하나를 기다리는 세로줄」 이다. 행마다:
//!
//! - 이 행의 커밋을 기다리던 레인들이 노드로 모인다(`up`). 노드는 그중 가장 왼쪽 레인에 놓인다. 기다리던 레인이 없으면(브랜치 끝)
//!   비어 있는 가장 왼쪽 레인에서 시작한다.
//! - 나머지 레인은 그대로 지나간다(`pass`).
//! - 노드에서 부모로 간선이 나간다(`down`). 첫 부모는 노드의 레인을 이어 간다. 다른 부모는 이미 그 부모를 기다리는 레인이 있으면
//!   그 레인으로, 없으면 노드 오른쪽의 빈 레인(없으면 맨 끝)에 새 레인을 연다. 첫 부모를 다른 레인이 이미 기다려도 노드의 레인을
//!   잇는다 — 두 줄은 그 부모의 행에서 모인다(갈라진 자리가 그 행에 보인다).
//!
//! 레인은 자리를 옮기지 않는다(지나가는 간선은 언제나 수직). 빈 자리는 다음 새 레인이 다시 쓴다.
//!
//! 색은 레인이 열릴 때 받는 번호다(0부터 늘어난다). 첫 부모로 이어 가는 레인은 노드의 색을 그대로 갖는다. UI 는 번호를 팔레트
//! 크기로 나눈 나머지로 `graph.lane.*` 토큰을 고른다.

use serde::{Deserialize, Serialize};

/// 한 행의 그래프. 행의 높이를 위 · 가운데(노드) · 아래로 나누어, 간선은 각각 「윗변 → 노드」, 「노드 → 아랫변」, 「윗변 → 아랫변」 이다.
/// 간선 하나는 `[레인, 색]` 이다.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GraphRow {
    /// 노드가 놓인 레인(0 = 가장 왼쪽).
    pub lane: u32,
    /// 노드의 색 번호.
    pub color: u32,
    /// 윗변의 그 레인에서 노드로 모이는 간선. 노드의 레인이 들어 있으면 위에서 이어 온 줄이고, 비어 있으면 이 커밋이 줄의 시작이다.
    pub up: Vec<(u32, u32)>,
    /// 노드에서 아랫변의 그 레인으로 가는 간선(부모 순서). 비어 있으면 뿌리 커밋이다.
    pub down: Vec<(u32, u32)>,
    /// 윗변에서 아랫변으로 같은 레인을 곧게 지나가는 간선.
    pub pass: Vec<(u32, u32)>,
}

#[derive(Debug, Clone, Copy)]
struct Lane {
    /// 기다리는 커밋(`walk::Dag` 의 번호).
    target: u32,
    color: u32,
}

/// 행을 하나씩 배치하는 상태.
#[derive(Debug, Default)]
pub struct Layout {
    lanes: Vec<Option<Lane>>,
    next_color: u32,
}

impl Layout {
    pub fn new() -> Self {
        Self::default()
    }

    /// 커밋 `node`(부모 `parents`)의 행. 행 순서대로 불러야 한다.
    pub fn row(&mut self, node: u32, parents: &[u32]) -> GraphRow {
        let mut up = Vec::new();
        let mut pass = Vec::new();
        let mut lane = None;
        for (i, slot) in self.lanes.iter().enumerate() {
            let Some(l) = slot else { continue };
            if l.target == node {
                up.push((i as u32, l.color));
                lane.get_or_insert((i, l.color));
            } else {
                pass.push((i as u32, l.color));
            }
        }
        for &(i, _) in &up {
            self.lanes[i as usize] = None;
        }
        let (lane, color) = match lane {
            Some(found) => found,
            None => (self.free_from(0), self.color()),
        };
        let mut down = Vec::with_capacity(parents.len());
        if let Some((&first, rest)) = parents.split_first() {
            self.put(
                lane,
                Lane {
                    target: first,
                    color,
                },
            );
            down.push((lane as u32, color));
            for &parent in rest {
                let existing = self
                    .lanes
                    .iter()
                    .position(|l| l.is_some_and(|l| l.target == parent));
                let (i, c) = match existing {
                    Some(i) => (i, self.lanes[i].map_or(color, |l| l.color)),
                    None => {
                        let i = self.free_from(lane + 1);
                        let c = self.color();
                        self.put(
                            i,
                            Lane {
                                target: parent,
                                color: c,
                            },
                        );
                        (i, c)
                    }
                };
                down.push((i as u32, c));
            }
        }
        while self.lanes.last().is_some_and(Option::is_none) {
            self.lanes.pop();
        }
        GraphRow {
            lane: lane as u32,
            color,
            up,
            down,
            pass,
        }
    }

    /// `from` 이상에서 비어 있는 첫 레인(없으면 맨 끝).
    fn free_from(&self, from: usize) -> usize {
        (from..self.lanes.len())
            .find(|&i| self.lanes[i].is_none())
            .unwrap_or(self.lanes.len().max(from))
    }

    fn put(&mut self, i: usize, lane: Lane) {
        if self.lanes.len() <= i {
            self.lanes.resize(i + 1, None);
        }
        self.lanes[i] = Some(lane);
    }

    fn color(&mut self) -> u32 {
        let c = self.next_color;
        self.next_color += 1;
        c
    }
}
