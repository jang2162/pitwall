//! 그래프 레인 배치(결정 M2-D2 — 레인은 Rust 가 계산하고 UI 는 그리기만 한다). 행 순서대로 한 행씩 배치한다 — 앞 행들의 상태만
//! 보므로 첫 청크를 전체 이력 배치 전에 보낼 수 있다.
//!
//! 레인은 「다음에 올 커밋 하나를 기다리는 세로줄」 이다. 행마다:
//!
//! - 이 행의 커밋을 기다리던 레인들이 노드로 모인다(`up`). 노드는 그중 가장 왼쪽 레인에 놓인다. 기다리던 레인이 없으면(브랜치 끝)
//!   비어 있는 가장 왼쪽 레인에서 시작한다.
//! - 나머지 레인은 그대로 지나간다.
//! - 노드에서 부모로 간선이 나간다(`down`). 첫 부모는 노드의 레인을 이어 간다. 다른 부모는 이미 그 부모를 기다리는 레인이 있으면
//!   그 레인으로, 없으면 노드 오른쪽의 빈 레인(없으면 맨 끝)에 새 레인을 연다. 첫 부모를 다른 레인이 이미 기다려도 노드의 레인을
//!   잇는다 — 두 줄은 그 부모의 행에서 모인다(갈라진 자리가 그 행에 보인다).
//!
//! 레인은 자리를 옮기지 않는다(지나가는 간선은 언제나 수직). 빈 자리는 다음 새 레인이 다시 쓴다.
//!
//! **레인 상한**: 보통 레인은 [`MAX_LANES`] 개(0 ~ 63)다. 새 레인을 열 자리가 없으면 그 갈래는 맨 오른쪽의 접힌 레인
//! [`FOLDED`] 하나로 간다 — 접힌 레인은 여러 커밋을 함께 기다리고, 접힌 갈래의 노드는 그 칸에 놓인다. 오래된 원격 브랜치가 수천
//! 개 쌓인 저장소에서도 행의 폭이 상한을 넘지 않는다.
//!
//! **행은 앞 행과의 차이만 싣는다**: 윗변의 열린 레인은 앞 행의 아랫변과 같으므로 행마다 싣지 않는다. 읽는 쪽은 열린 레인 표(레인 →
//! 색)를 들고 행마다 이렇게 잇는다 — 윗변 = 앞 행의 아랫변(청크의 첫 행은 [`GraphRow::top`]), 이 행에서 끝나는 레인 = `up` 의
//! 레인(접힌 레인은 [`GraphRow::fold_closes`] 일 때만), 지나가는 레인 = 윗변 − 끝나는 레인, 아랫변 = 지나가는 레인 + `down` 의 레인.
//!
//! 색은 레인이 열릴 때 받는 번호다(0부터 늘어난다). 첫 부모로 이어 가는 레인은 노드의 색을 그대로 갖는다. UI 는 번호를 팔레트
//! 크기로 나눈 나머지로 `graph.lane.*` 토큰을 고른다. 접힌 레인의 간선 · 노드는 언제나 [`FOLDED_COLOR`] 다.

use std::collections::HashSet;

use serde::{Deserialize, Serialize, Serializer};

/// 보통 레인 수 상한(`docs/design/13-m2-plan.md` 결정 M2-D2).
pub const MAX_LANES: u32 = 64;
/// 접힌 레인의 번호 — 보통 레인 바로 오른쪽.
pub const FOLDED: u32 = MAX_LANES;
/// 접힌 레인의 색 번호.
pub const FOLDED_COLOR: u32 = u32::MAX;

/// 간선 하나: `[레인, 색]`.
pub type Edge = (u32, u32);

/// 한 행의 그래프. 행의 높이를 위 · 가운데(노드) · 아래로 나누어, 간선은 각각 「윗변 → 노드」, 「노드 → 아랫변」 이다. 곧게
/// 지나가는 간선은 싣지 않는다 — 앞 행에서 이어 계산한다(머리 주석).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GraphRow {
    /// 노드가 놓인 레인(0 = 가장 왼쪽). 보통 레인은 0 ~ 63, 64 는 접힌 레인이다([`MAX_LANES`] · [`FOLDED`]).
    pub lane: u32,
    /// 노드의 색 번호. 접힌 레인의 간선 · 노드는 4294967295([`FOLDED_COLOR`]).
    pub color: u32,
    /// 윗변의 그 레인에서 노드로 모이는 간선(레인 순). 노드의 레인이 들어 있으면 위에서 이어 온 줄이고, 비어 있으면 이 커밋이 줄의
    /// 시작이다.
    pub up: Vec<Edge>,
    /// 노드에서 아랫변의 그 레인으로 가는 간선(부모 순서). 비어 있으면 뿌리 커밋이다.
    pub down: Vec<Edge>,
    /// 윗변에 열려 있는 레인 전체(레인 순). 청크 · 쪽의 첫 행에만 있다 — 그 행부터 앞 행 없이 이어 계산할 수 있다.
    // 직렬화를 손으로 쓰는 이유는 `pw_rpc::RpcError` 와 같다(`skip_serializing_if` 를 달면 specta 가 타입을 둘로 가른다). 행마다
    // 붙는 값이라 없을 때 키를 뺀다.
    #[serde(default)]
    #[specta(optional)]
    pub top: Option<Vec<Edge>>,
    /// 접힌 레인이 이 행에서 닫힌다(기다리던 커밋이 모두 나왔다). 접힌 레인은 `up` 에 있어도 다른 갈래를 더 기다리면 이어진다.
    #[serde(default)]
    #[specta(optional)]
    pub fold_closes: bool,
}

impl Serialize for GraphRow {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let fields = 4 + usize::from(self.top.is_some()) + usize::from(self.fold_closes);
        let mut s = serializer.serialize_struct("GraphRow", fields)?;
        s.serialize_field("lane", &self.lane)?;
        s.serialize_field("color", &self.color)?;
        s.serialize_field("up", &self.up)?;
        s.serialize_field("down", &self.down)?;
        if let Some(top) = &self.top {
            s.serialize_field("top", top)?;
        }
        if self.fold_closes {
            s.serialize_field("foldCloses", &true)?;
        }
        s.end()
    }
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
    /// 보통 레인. 길이는 [`MAX_LANES`] 를 넘지 않는다.
    lanes: Vec<Option<Lane>>,
    /// 접힌 레인이 기다리는 커밋. 비어 있으면 접힌 레인이 닫혀 있다.
    folded: HashSet<u32>,
    next_color: u32,
}

impl Layout {
    pub fn new() -> Self {
        Self::default()
    }

    /// 다음 행 윗변에 열려 있는 레인(레인 순) — [`GraphRow::top`].
    pub fn top(&self) -> Vec<Edge> {
        let mut top: Vec<Edge> = self
            .lanes
            .iter()
            .enumerate()
            .filter_map(|(i, l)| l.map(|l| (i as u32, l.color)))
            .collect();
        if !self.folded.is_empty() {
            top.push((FOLDED, FOLDED_COLOR));
        }
        top
    }

    /// 커밋 `node`(부모 `parents`)의 행. 행 순서대로 불러야 한다. `top` 은 비워서 돌려준다.
    pub fn row(&mut self, node: u32, parents: &[u32]) -> GraphRow {
        let was_folded = !self.folded.is_empty();
        let mut up = Vec::new();
        for (i, slot) in self.lanes.iter_mut().enumerate() {
            if slot.is_some_and(|l| l.target == node) {
                up.push((i as u32, slot.take().map_or(0, |l| l.color)));
            }
        }
        if self.folded.remove(&node) {
            up.push((FOLDED, FOLDED_COLOR));
        }
        let (lane, color) = match up.first() {
            Some(&(lane, color)) => (lane, color),
            None => match self.free_from(0) {
                Some(i) => (i, self.color()),
                None => (FOLDED, FOLDED_COLOR),
            },
        };
        let mut down = Vec::with_capacity(parents.len());
        if let Some((&first, rest)) = parents.split_first() {
            self.put(lane, first, color);
            down.push((lane, color));
            for &parent in rest {
                let existing = self
                    .lanes
                    .iter()
                    .position(|l| l.is_some_and(|l| l.target == parent));
                let edge = match existing {
                    Some(i) => (i as u32, self.lanes[i].map_or(color, |l| l.color)),
                    None if self.folded.contains(&parent) => (FOLDED, FOLDED_COLOR),
                    None => {
                        let i = self.free_from(lane + 1).unwrap_or(FOLDED);
                        let c = if i == FOLDED {
                            FOLDED_COLOR
                        } else {
                            self.color()
                        };
                        self.put(i, parent, c);
                        (i, c)
                    }
                };
                down.push(edge);
            }
        }
        while self.lanes.last().is_some_and(Option::is_none) {
            self.lanes.pop();
        }
        GraphRow {
            lane,
            color,
            up,
            down,
            top: None,
            fold_closes: was_folded && self.folded.is_empty(),
        }
    }

    /// `from` 이상에서 비어 있는 첫 보통 레인. 모두 차 있으면 `None`(접힌 레인으로 간다).
    fn free_from(&self, from: u32) -> Option<u32> {
        let from = from as usize;
        (from..self.lanes.len())
            .find(|&i| self.lanes[i].is_none())
            .or_else(|| {
                let end = self.lanes.len().max(from);
                (end < MAX_LANES as usize).then_some(end)
            })
            .map(|i| i as u32)
    }

    /// 레인 `i` 가 `target` 을 기다리게 한다(접힌 레인이면 접힌 레인의 목록에 더한다).
    fn put(&mut self, i: u32, target: u32, color: u32) {
        if i == FOLDED {
            self.folded.insert(target);
            return;
        }
        let i = i as usize;
        if self.lanes.len() <= i {
            self.lanes.resize(i + 1, None);
        }
        self.lanes[i] = Some(Lane { target, color });
    }

    fn color(&mut self) -> u32 {
        let c = self.next_color;
        self.next_color += 1;
        c
    }
}
