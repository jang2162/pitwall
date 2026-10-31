// 원래 글을 지키는 json 파서(M3-16 리뷰): RFC 8259 문법 그대로 받고, 수는 받은 글 그대로, 너무 깊으면 던진다(쓰는 쪽은 글로 보인다).

import { describe, expect, it } from "vitest";

import { MAX_DEPTH, parseJsonText, writeJsonText } from "./json-text";

describe("parseJsonText", () => {
  it("keeps numbers as written and reads strings like JSON.parse", () => {
    expect(parseJsonText(" -0 ")).toEqual({ kind: "number", raw: "-0" });
    expect(parseJsonText("9007199254740993")).toEqual({ kind: "number", raw: "9007199254740993" });
    expect(parseJsonText('"a\\u00e9\\n\\""')).toEqual({ kind: "string", value: 'aé\n"' });
  });

  it("refuses what JSON refuses", () => {
    for (const bad of [
      "",
      "01",
      "1.",
      "+1",
      "[1,]",
      '{"a"}',
      "tru",
      "[1] x",
      '"\u0001"',
      "{'a':1}",
    ]) {
      expect(() => parseJsonText(bad), bad).toThrow(SyntaxError);
    }
  });

  it("throws past MAX_DEPTH instead of overflowing the stack", () => {
    const deep = `${"[".repeat(MAX_DEPTH + 2)}${"]".repeat(MAX_DEPTH + 2)}`;
    expect(() => parseJsonText(deep)).toThrow(/too deep/);
    const ok = `${"[".repeat(MAX_DEPTH)}${"]".repeat(MAX_DEPTH)}`;
    expect(() => parseJsonText(ok)).not.toThrow();
  });

  it("round-trips the reviewer's value through the writer unchanged", () => {
    const text = '{"id": 9007199254740993, "d": 0.10000000000000000555}';
    expect(writeJsonText(parseJsonText(text))).toBe(
      '{\n  "id": 9007199254740993,\n  "d": 0.10000000000000000555\n}',
    );
  });
});
