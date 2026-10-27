// 커밋 그래프의 레인 잇기(docs/design/13-m2-plan.md 결정 M2-D2): Host 의 `git.log` 행은 앞 행과의 차이만 싣는다 — 노드로 모이는
// 간선(`up`) · 노드에서 나가는 간선(`down`)뿐이고, 곧게 지나가는 레인은 여기서 앞 행에서 이어 계산한다. 규칙의 정본은
// crates/pw-git/src/log/graph.rs 머리 주석이다:
//
//   윗변 = 앞 행의 아랫변(청크 · 쪽의 첫 행은 `top`), 끝나는 레인 = `up` 의 레인(접힌 레인은 `foldCloses` 일 때만),
//   지나가는 레인 = 윗변 − 끝나는 레인, 아랫변 = 지나가는 레인 + `down` 의 레인.
//
// 10만 행의 지나가는 레인을 모두 들고 있지 않는다 — `STEP` 행마다 윗변 하나(많아야 65 레인)를 적어 두고, 화면에 그릴 행은 가장
// 가까운 적어 둔 행부터 다시 잇는다(한 번에 많아야 `STEP` + 화면 행 수). React 상태가 아니라 이 객체 안의 버퍼다(10-ui.md
// 「성능 규칙」).

import type { GraphRow } from "@pitwall/rpc-client";

/** 접힌 레인의 번호(보통 레인 0 ~ 63 의 바로 오른쪽, pw-git `FOLDED`). */
export const FOLDED_LANE = 64;
/** 접힌 레인의 간선 · 노드의 색 번호(pw-git `FOLDED_COLOR`). 팔레트 나머지를 내기 전에 따로 본다. */
export const FOLDED_COLOR = 4294967295;

/** `[레인, 색]`. */
export type Edge = readonly [number, number];

/** 그릴 행 하나의 레인. */
export interface RowLanes {
  /** 곧게 지나가는 레인(레인 순). */
  pass: readonly Edge[];
  graph: GraphRow;
}

/** 윗변을 적어 두는 간격(행). */
const STEP = 64;

function byLane(a: Edge, b: Edge) {
  return a[0] - b[0];
}

/** 윗변 `top` 의 행 `g` 에서 지나가는 레인과 아랫변. */
export function stepLanes(top: readonly Edge[], g: GraphRow): { pass: Edge[]; bottom: Edge[] } {
  const ends = new Set<number>();
  for (const [lane] of g.up) if (lane !== FOLDED_LANE || g.foldCloses) ends.add(lane);
  const pass = top.filter(([lane]) => !ends.has(lane));
  const bottom = new Map<number, number>(pass.map(([lane, color]) => [lane, color]));
  for (const [lane, color] of g.down) if (!bottom.has(lane)) bottom.set(lane, color);
  return { pass, bottom: [...bottom].sort(byLane) };
}

/** 받은 행의 레인을 이어 계산한다. 행은 받은 차례대로 `append` 한다. 그래프 없는 행(걸러진 목록)이면 레인이 없다. */
export class LaneIndex {
  private readonly graphs: (GraphRow | null)[] = [];
  /** `k * STEP` 행의 윗변. */
  private readonly tops: (readonly Edge[])[] = [];
  private open: readonly Edge[] = [];

  get length(): number {
    return this.graphs.length;
  }

  append(graphs: readonly (GraphRow | null)[]): void {
    for (const g of graphs) {
      const i = this.graphs.length;
      this.graphs.push(g);
      if (!g) continue;
      const top = g.top ?? this.open;
      if (i % STEP === 0) this.tops[i / STEP] = top;
      this.open = stepLanes(top, g).bottom;
    }
  }

  /** `from` 이상 `to` 미만 행의 레인. 그래프가 없는 행은 빠진다(`null`). */
  range(from: number, to: number): (RowLanes | null)[] {
    const end = Math.min(to, this.graphs.length);
    if (from >= end) return [];
    let at = Math.floor(from / STEP) * STEP;
    let top: readonly Edge[] = this.tops[at / STEP] ?? [];
    const out: (RowLanes | null)[] = [];
    for (; at < end; at++) {
      const g = this.graphs[at];
      if (!g) {
        if (at >= from) out.push(null);
        continue;
      }
      if (g.top) top = g.top;
      const { pass, bottom } = stepLanes(top, g);
      if (at >= from) out.push({ pass, graph: g });
      top = bottom;
    }
    return out;
  }
}

/** 행들에서 가장 오른쪽 레인(그래프 칸의 폭을 정한다). 레인이 없으면 -1. */
export function widestLane(rows: readonly (RowLanes | null)[]): number {
  let widest = -1;
  for (const row of rows) {
    if (!row) continue;
    const g = row.graph;
    widest = Math.max(widest, g.lane);
    for (const [lane] of row.pass) widest = Math.max(widest, lane);
    for (const [lane] of g.up) widest = Math.max(widest, lane);
    for (const [lane] of g.down) widest = Math.max(widest, lane);
  }
  return widest;
}
