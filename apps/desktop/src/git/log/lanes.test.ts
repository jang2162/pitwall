// 레인 잇기가 pw-git 의 규칙과 같은지: 픽스처(testdata/lanes.json)는 pw-git 시험 `write_ui_lane_fixture` 가 합성 이력 · 접힌 레인
// 이력의 행 그래프와, Rust 쪽에서 이어 계산한 지나가는 레인을 적은 것이다.

import type { GraphRow } from "@pitwall/rpc-client";
import { describe, expect, it } from "vitest";

import { FOLDED_LANE, LaneIndex, stepLanes, widestLane } from "./lanes";
import fixture from "./testdata/lanes.json";

const cases = fixture as unknown as Record<string, { graphs: GraphRow[]; passing: number[][][] }>;

describe("LaneIndex", () => {
  for (const [name, { graphs, passing }] of Object.entries(cases)) {
    it(`gives pw-git's passing lanes for every row of the ${name} history, from any starting row`, () => {
      const index = new LaneIndex();
      // 청크처럼 나눠 받는다.
      for (let at = 0; at < graphs.length; at += 37) index.append(graphs.slice(at, at + 37));
      expect(index.length).toBe(graphs.length);
      const all = index.range(0, graphs.length).map((r) => r!.pass);
      expect(all).toEqual(passing);
      for (const from of [1, 63, 64, 65, 130, graphs.length - 5]) {
        const some = index.range(from, from + 20).map((r) => r!.pass);
        expect(some).toEqual(passing.slice(from, from + 20));
      }
    });
  }

  it("folds lanes past the cap into lane 64 in the fanout history", () => {
    const { graphs } = cases.fanout!;
    const index = new LaneIndex();
    index.append(graphs);
    expect(widestLane(index.range(0, graphs.length))).toBe(FOLDED_LANE);
  });

  it("keeps the folded lane open when it delivers to a node without closing", () => {
    const top = [
      [0, 3],
      [FOLDED_LANE, 4294967295],
    ] as const;
    const g: GraphRow = {
      lane: FOLDED_LANE,
      color: 4294967295,
      up: [[FOLDED_LANE, 4294967295]],
      down: [[FOLDED_LANE, 4294967295]],
    };
    expect(stepLanes(top, g).pass).toEqual([
      [0, 3],
      [FOLDED_LANE, 4294967295],
    ]);
    expect(stepLanes(top, { ...g, down: [], foldCloses: true }).bottom).toEqual([[0, 3]]);
  });

  it("has no lanes for a filtered list", () => {
    const index = new LaneIndex();
    index.append([null, null]);
    expect(index.range(0, 2)).toEqual([null, null]);
    expect(widestLane(index.range(0, 2))).toBe(-1);
  });
});
