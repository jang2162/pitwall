// 결과 버퍼(M3-16): 청크를 React 상태 밖에 붙이고, 보유한 행 위에서 정렬 · 필터하며(서버로 다시 묻지 않는다), 알림은 한 번에
// 모은다.

import type { DbValue } from "@pitwall/rpc-client";
import { afterEach, describe, expect, it, vi } from "vitest";

import { cellBytes, RESORT_MS, ResultBuffer } from "./result-buffer";

const now = (run: () => void) => run();

function buffer() {
  const b = new ResultBuffer(now);
  b.setColumns([
    { name: "id", dbType: "int8", nullable: false },
    { name: "name", dbType: "text", nullable: true },
  ]);
  return b;
}

const row = (id: number, name: string | null): DbValue[] => [
  { int: id },
  name === null ? "null" : { text: name },
];

afterEach(() => vi.useRealTimers());

describe("ResultBuffer", () => {
  it("appends chunks and reads rows by index", () => {
    const b = buffer();
    b.append([row(1, "a"), row(2, "b")]);
    b.append([row(3, "c")]);
    expect(b.rowCount).toBe(3);
    expect(b.row(2)).toEqual(row(3, "c"));
    expect(b.row(3)).toBeUndefined();
  });

  it("batches change notifications into one per scheduled turn", () => {
    const queued: (() => void)[] = [];
    const b = new ResultBuffer((run) => queued.push(run));
    const seen = vi.fn();
    b.subscribe(seen);
    b.append([row(1, "a")]);
    b.append([row(2, "b")]);
    expect(queued).toHaveLength(1);
    queued[0]!();
    expect(seen).toHaveBeenCalledTimes(1);
  });

  it("sorts and filters the rows it holds", () => {
    const b = buffer();
    b.append([row(3, "Carol"), row(1, null), row(2, "alice")]);
    b.setSort({ column: 1, direction: "asc" });
    expect([0, 1, 2].map((i) => b.row(i)?.[0])).toEqual([{ int: 2 }, { int: 3 }, { int: 1 }]);
    b.setFilter({ text: "AL" });
    expect(b.rowCount).toBe(1);
    expect(b.row(0)).toEqual(row(2, "alice"));
    b.setFilter({ text: "1", column: 0 });
    expect(b.rowCount).toBe(1);
    b.setFilter(undefined);
    b.setSort(undefined);
    expect(b.row(0)).toEqual(row(3, "Carol"));
  });

  it("filters rows that arrive after the filter was set", () => {
    const b = buffer();
    b.setFilter({ text: "x" });
    b.append([row(1, "x1"), row(2, "y")]);
    b.append([row(3, "x3")]);
    expect(b.rowCount).toBe(2);
    expect(b.loadedRows).toBe(3);
  });

  it("re-sorts rows that arrive while sorted, and at the end of the stream", () => {
    vi.useFakeTimers();
    const b = buffer();
    b.append([row(2, "b")]);
    b.setSort({ column: 0, direction: "desc" });
    b.append([row(5, "e"), row(1, "a")]);
    // 다시 정렬하기 전에는 새 행이 뒤에 붙는다.
    expect(b.row(0)).toEqual(row(2, "b"));
    vi.advanceTimersByTime(RESORT_MS);
    expect([0, 1, 2].map((i) => b.row(i)?.[0])).toEqual([{ int: 5 }, { int: 2 }, { int: 1 }]);
    b.append([row(9, "z")]);
    b.finish({ kind: "done", truncated: false, elapsedMs: 3, rowsAffected: null });
    expect(b.row(0)).toEqual(row(9, "z"));
  });

  it("counts the held cell bytes the way the Host counts them for the byte limit", () => {
    expect(cellBytes("null")).toBe(8);
    expect(cellBytes({ int: "9007199254740993" })).toBe(8);
    expect(cellBytes({ text: "abc" })).toBe(3);
    expect(cellBytes({ text: "한" })).toBe(3);
    expect(cellBytes({ text: "😀" })).toBe(4);
    expect(cellBytes({ bytes: { hex: "0x00ff", size: 2, truncated: false } })).toBe(6);
    expect(cellBytes({ array: [{ text: "ab" }, "null"] })).toBe(12);
    expect(cellBytes({ truncated: { of: "text", prefix: "pq", size: 9e6, typeName: null } })).toBe(
      2,
    );
    const b = buffer();
    b.append([row(1, "한글"), row(2, null)]);
    expect(b.heldBytes).toBe(8 + 6 + 8 + 8);
  });

  it("each result has its own id, and filtering finds text after the first line", () => {
    expect(buffer().id).not.toBe(buffer().id);
    const b = buffer();
    b.append([row(1, "first\nneedle"), row(2, "other")]);
    b.setFilter({ text: "NEEDLE" });
    expect(b.rowCount).toBe(1);
  });
});
