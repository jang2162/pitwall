import { describe, expect, it } from "vitest";

import { diffLines, splitLines } from "./line-diff";

/** 묶음을 `a` 에 적용해 `b` 를 다시 만든다. */
function apply(a: string[], b: string[], groups: ReturnType<typeof diffLines>): string[] {
  const out: string[] = [];
  let i = 1;
  for (const g of groups) {
    while (i < g.oldFrom) out.push(a[i++ - 1]!);
    out.push(...b.slice(g.newFrom - 1, g.newTo - 1));
    i = g.oldTo;
  }
  while (i <= a.length) out.push(a[i++ - 1]!);
  return out;
}

/** 바뀐 줄 수(뺀 줄 + 더한 줄). */
const cost = (groups: ReturnType<typeof diffLines>) =>
  groups.reduce((n, g) => n + (g.oldTo - g.oldFrom) + (g.newTo - g.newFrom), 0);

/** LCS 로 잰 최소 비용. */
function lcsCost(a: string[], b: string[]): number {
  const dp = Array.from({ length: a.length + 1 }, () =>
    Array.from({ length: b.length + 1 }, () => 0),
  );
  for (let i = a.length - 1; i >= 0; i--)
    for (let j = b.length - 1; j >= 0; j--)
      dp[i]![j] = a[i] === b[j] ? dp[i + 1]![j + 1]! + 1 : Math.max(dp[i + 1]![j]!, dp[i]![j + 1]!);
  return a.length + b.length - 2 * dp[0]![0]!;
}

function random(seed: number) {
  let s = seed;
  return () => {
    s = (s * 1103515245 + 12345) & 0x7fffffff;
    return s / 0x7fffffff;
  };
}

describe("diffLines", () => {
  it("finds nothing for equal texts and one group per edit", () => {
    expect(diffLines(["a", "b"], ["a", "b"])).toEqual([]);
    // 고친 줄 하나.
    expect(diffLines(["a", "b", "c"], ["a", "B", "c"])).toEqual([
      { oldFrom: 2, oldTo: 3, newFrom: 2, newTo: 3 },
    ]);
    // 더한 줄(옛 쪽은 비고 끼어드는 자리다).
    expect(diffLines(["a", "c"], ["a", "b", "c"])).toEqual([
      { oldFrom: 2, oldTo: 2, newFrom: 2, newTo: 3 },
    ]);
    // 지운 줄 · 파일 끝.
    expect(diffLines(["a", "b", "c"], ["a", "b"])).toEqual([
      { oldFrom: 3, oldTo: 4, newFrom: 3, newTo: 3 },
    ]);
    // 떨어진 두 곳.
    expect(diffLines(["1", "2", "3", "4", "5"], ["1", "x", "3", "4", "y"])).toEqual([
      { oldFrom: 2, oldTo: 3, newFrom: 2, newTo: 3 },
      { oldFrom: 5, oldTo: 6, newFrom: 5, newTo: 6 },
    ]);
  });

  it("rebuilds the new side with a minimal number of changed lines on random edits", () => {
    const next = random(7);
    for (let round = 0; round < 300; round++) {
      const a = Array.from({ length: Math.floor(next() * 30) }, () =>
        String.fromCharCode(97 + Math.floor(next() * 5)),
      );
      const b = a.slice();
      for (let e = Math.floor(next() * 6); e > 0; e--) {
        const at = Math.floor(next() * (b.length + 1));
        const roll = next();
        if (roll < 0.4) b.splice(at, 1);
        else if (roll < 0.8) b.splice(at, 0, String.fromCharCode(97 + Math.floor(next() * 5)));
        else b[at] = "z";
      }
      const groups = diffLines(a, b);
      expect(apply(a, b, groups)).toEqual(b);
      expect(cost(groups)).toBe(lcsCost(a, b));
    }
  });

  it("gives up on a huge middle and reports it as one group", () => {
    const a = Array.from({ length: 50 }, (_, i) => `a${i}`);
    const b = Array.from({ length: 50 }, (_, i) => `b${i}`);
    expect(diffLines(["same", ...a, "end"], ["same", ...b, "end"], 10)).toEqual([
      { oldFrom: 2, oldTo: 52, newFrom: 2, newTo: 52 },
    ]);
  });

  it("splits a base the way the document splits its lines", () => {
    expect(splitLines("a\r\nb\rc\nd", undefined)).toEqual(["a", "b", "c", "d"]);
    expect(splitLines("a\r\nb\rc\nd", "\n")).toEqual(["a\r", "b\rc", "d"]);
  });
});
