// @vitest-environment happy-dom
// 로그 레벨 강조(17-m6-plan M6-06 ④, 메인 판정 M6-P14)의 표: `ERROR` 줄은 칠하고, 프로그램이 ANSI 색을 쓴 줄과 120자 뒤의 `ERROR`
// 는 칠하지 않는다(완료 기준 ②). 실제 xterm 버퍼에 써서 decoration 이 걸린 줄을 본다.

import { type IDecorationOptions, Terminal } from "@xterm/xterm";
import { describe, expect, it, vi } from "vitest";

import { findLevel, LevelHighlighter } from "./run-levels";

const THEME = { red: "#aa0000", yellow: "#aaaa00", green: "#00aa00", brightBlack: "#555555" };

/** 줄들을 쓰고 강조한 뒤 decoration 이 걸린 (줄 번호, 색, x, 폭)을 돌려준다. */
async function paint(lines: string[], { flush = true, cols = 200 } = {}) {
  const term = new Terminal({ cols, rows: 10, allowProposedApi: true });
  const seen: { row: number; color?: string; x?: number; width?: number }[] = [];
  const register = term.registerDecoration.bind(term);
  vi.spyOn(term, "registerDecoration").mockImplementation((o: IDecorationOptions) => {
    seen.push({ row: o.marker.line, color: o.foregroundColor, x: o.x, width: o.width });
    return register(o);
  });
  const levels = new LevelHighlighter(term);
  levels.setTheme(THEME);
  await new Promise<void>((r) => term.write(lines.join("\r\n"), r));
  levels.scan();
  if (flush) levels.flush();
  term.dispose();
  return seen;
}

describe("findLevel", () => {
  it("finds the five words at word boundaries in any case", () => {
    expect(findLevel("2026-11-18 ERROR boom")).toEqual({ level: "error", start: 11, end: 16 });
    expect(findLevel("[warn] slow")).toMatchObject({ level: "warn" });
    expect(findLevel("Info: up")).toMatchObject({ level: "info" });
    expect(findLevel("debug x")).toMatchObject({ level: "debug" });
    expect(findLevel("TRACE x")).toMatchObject({ level: "trace" });
    expect(findLevel("ERRORS and WARNING and information")).toBeUndefined();
  });

  it("only looks at the first 120 characters", () => {
    expect(findLevel(`${"x".repeat(114)} ERROR`)).toMatchObject({ end: 120 });
    expect(findLevel(`${"x".repeat(115)} ERROR`)).toBeUndefined();
  });
});

describe("LevelHighlighter", () => {
  it("paints an ERROR line red across the line", async () => {
    const seen = await paint(["start", "12:00:01 ERROR failed to bind", "done"]);
    expect(seen).toEqual([{ row: 1, color: "#aa0000", x: 0, width: 200 }]);
  });

  it("leaves a line the program already coloured with ANSI", async () => {
    const seen = await paint(["\x1b[31mERROR\x1b[0m failed", "plain \x1b[1;33mWARN\x1b[0m slow"]);
    expect(seen).toEqual([]);
  });

  it("leaves an ERROR that ends past the first 120 characters", async () => {
    const seen = await paint([`${"x".repeat(130)} ERROR late`]);
    expect(seen).toEqual([]);
  });

  it("paints only the INFO word, and dims DEBUG lines", async () => {
    const seen = await paint(["ab INFO up", "DEBUG detail"]);
    expect(seen).toEqual([
      { row: 0, color: "#00aa00", x: 3, width: 4 },
      { row: 1, color: "#555555", x: 0, width: 200 },
    ]);
  });

  it("waits for the line the cursor is on until it ends", async () => {
    // 끝나지 않은 줄(커서 줄)은 scan 만으로는 칠하지 않는다.
    expect(await paint(["ERROR half"], { flush: false })).toEqual([]);
    expect(await paint(["ERROR half"], { flush: true })).toEqual([
      { row: 0, color: "#aa0000", x: 0, width: 200 },
    ]);
  });

  it("reads a wrapped line as one line", async () => {
    // 폭 20 에서 접힌 줄의 둘째 행에 있는 ERROR 도 그 줄의 것이다.
    const seen = await paint([`${"y".repeat(25)} ERROR wrapped`, "next"], { cols: 20 });
    expect(seen).toEqual([{ row: 0, color: "#aa0000", x: 0, width: 20 }]);
  });
});
