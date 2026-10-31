// 실행 범위(run-scope.ts): 선택과 겹치는 문장들, 선택이 없으면 Host 의 「현재 문장」.

import { describe, expect, it, vi } from "vitest";

import { overlapping, statementsToRun } from "./run-scope";

const RANGES = [
  { from: 0, to: 8 },
  { from: 10, to: 18 },
  { from: 20, to: 28 },
];

describe("run scope", () => {
  it("picks the statements a selection touches, and none for a selection in the gaps", () => {
    expect(overlapping(RANGES, { from: 0, to: 28 })).toEqual([0, 1, 2]);
    expect(overlapping(RANGES, { from: 7, to: 11 })).toEqual([0, 1]);
    expect(overlapping(RANGES, { from: 8, to: 10 })).toEqual([]);
    expect(overlapping(RANGES, { from: 18, to: 21 })).toEqual([2]);
  });

  it("asks the Host for the statement at the caret when nothing is selected", async () => {
    const call = vi.fn(async (method: string) =>
      method === "db.sql.statementAt"
        ? { statement: { index: 1, range: RANGES[1] } }
        : { statements: RANGES },
    );
    expect(await statementsToRun(call as never, "postgres", "x", { from: 12, to: 12 })).toEqual([
      1,
    ]);
    expect(call).toHaveBeenLastCalledWith("db.sql.statementAt", {
      dialect: "postgres",
      sql: "x",
      offset: 12,
    });
    expect(await statementsToRun(call as never, "mysql", "x", { from: 1, to: 22 })).toEqual([
      0, 1, 2,
    ]);
    expect(call).toHaveBeenLastCalledWith("db.sql.split", { dialect: "mysql", sql: "x" });
    const none = vi.fn(async () => ({ statement: null }));
    expect(await statementsToRun(none as never, "sqlite", "", { from: 0, to: 0 })).toEqual([]);
  });
});
