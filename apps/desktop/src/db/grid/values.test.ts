// DbValue 표시 · 정렬 규칙(M3-16, 14-m3-plan 결정 M3-D8): 문자열로 온 큰 정수 · decimal 은 숫자처럼 오른쪽 정렬 · 값으로 정렬,
// NULL 은 흐린 `NULL`(글자 「NULL」 과 갈린다) · 늘 맨 뒤, 바이트는 `0x…`, 잘린 값은 `…`.

import type { DbValue } from "@pitwall/rpc-client";
import { describe, expect, it } from "vitest";

import {
  CELL_CHARS,
  compareKeys,
  displayText,
  isNumeric,
  isTruncated,
  originalSize,
  sortKey,
  valueKind,
  valueText,
} from "./values";

const big: DbValue = { int: "9007199254740993" };

describe("display", () => {
  it("shows big integers and decimals as right-aligned numbers", () => {
    expect(isNumeric(big)).toBe(true);
    expect(isNumeric({ decimal: "0.10" })).toBe(true);
    expect(isNumeric({ float: "NaN" })).toBe(true);
    expect(isNumeric({ text: "123" })).toBe(false);
    expect(displayText(big)).toBe("9007199254740993");
    expect(displayText({ decimal: "-12.3400" })).toBe("-12.3400");
  });

  it("tells NULL apart from the text NULL by kind", () => {
    expect(displayText("null")).toBe("NULL");
    expect(valueKind("null")).toBe("null");
    expect(valueKind({ text: "NULL" })).toBe("text");
    expect(valueText("null")).toBeNull();
    expect(valueText({ text: "NULL" })).toBe("NULL");
  });

  it("shows bytes as 0x hex and marks truncated values with their original size", () => {
    const bytes: DbValue = { bytes: { hex: "0xdeadbeef", size: 10, truncated: true } };
    expect(displayText(bytes)).toBe("0xdeadbeef…");
    expect(isTruncated(bytes)).toBe(true);
    expect(originalSize(bytes)).toBe(10);
    const t: DbValue = {
      truncated: { of: "text", prefix: "abc", size: 2_000_000, typeName: null },
    };
    expect(displayText(t)).toBe("abc…");
    expect(originalSize(t)).toBe(2_000_000);
    expect(originalSize({ text: "x" })).toBeUndefined();
  });

  it("draws only the first line and the first CELL_CHARS characters", () => {
    expect(displayText({ text: "a\nb" })).toBe("a ⏎…");
    expect(displayText({ text: "x".repeat(CELL_CHARS + 5) })).toBe(`${"x".repeat(CELL_CHARS)}…`);
  });

  it("writes PG arrays in their text form", () => {
    expect(valueText({ array: [{ int: 1 }, "null", { text: 'a"b' }, { text: "c d" }] })).toBe(
      '{1,NULL,"a\\"b","c d"}',
    );
  });
});

describe("sort", () => {
  const sorted = (values: DbValue[], dir: 1 | -1 = 1) =>
    values
      .map((v) => ({ v, k: sortKey(v) }))
      .sort((a, b) => {
        if (a.k === null || b.k === null) return compareKeys(a.k, b.k);
        return dir * compareKeys(a.k, b.k);
      })
      .map((x) => x.v);

  it("orders numbers by value, keeping precision for big integers and decimals", () => {
    const values: DbValue[] = [
      { int: "9007199254740993" },
      { int: "9007199254740992" },
      { int: 5 },
      { int: "-9007199254740993" },
      { decimal: "10.5" },
      { decimal: "9.75" },
      { decimal: "-0.001" },
    ];
    expect(sorted(values).map(valueText)).toEqual([
      "-9007199254740993",
      "-0.001",
      "5",
      "9.75",
      "10.5",
      "9007199254740992",
      "9007199254740993",
    ]);
  });

  it("puts NULL last in both directions", () => {
    const values: DbValue[] = ["null", { int: 2 }, "null", { int: 1 }];
    expect(sorted(values).map(valueText)).toEqual(["1", "2", null, null]);
    expect(sorted(values, -1).map(valueText)).toEqual(["2", "1", null, null]);
  });

  it("orders text case-insensitively and NaN after every number", () => {
    expect(sorted([{ text: "b" }, { text: "A" }, { text: "a" }]).map(valueText)).toEqual([
      "A",
      "a",
      "b",
    ]);
    expect(
      sorted([{ float: "NaN" }, { float: "Infinity" }, { float: 1.5 }, { float: "-Infinity" }]).map(
        valueText,
      ),
    ).toEqual(["-Infinity", "1.5", "Infinity", "NaN"]);
  });
});
