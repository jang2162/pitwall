// 콘솔 응답의 글 줄(reply-text.ts): redis-cli 와 같은 꼴 — 번호 · 들여쓰기 · 맵 · 따옴표 · 바이트 · 끊긴 응답 · 줄 상한.

import type { KvReply } from "@pitwall/rpc-client";
import { describe, expect, it } from "vitest";

import { bundledTranslator } from "../../i18n/testing";
import { formatSize } from "../grid/values";
import { MAX_REPLY_LINES, replyLines } from "./reply-text";
import { dbRedisStrings } from "./strings";

const T = dbRedisStrings(bundledTranslator("ko")).console.reply;

describe("replyLines", () => {
  it("words scalars like redis-cli", () => {
    expect(replyLines("nil", T)).toEqual(["(nil)"]);
    expect(replyLines({ status: "OK" }, T)).toEqual(["OK"]);
    expect(replyLines({ error: "ERR x" }, T)).toEqual([T.error("ERR x")]);
    expect(replyLines({ int: { int: 42 } }, T)).toEqual([T.integer("42")]);
    expect(replyLines({ int: { int: "9007199254740993" } }, T)).toEqual([
      T.integer("9007199254740993"),
    ]);
    expect(replyLines({ double: "1.5" }, T)).toEqual([T.double("1.5")]);
    expect(replyLines({ bool: true }, T)).toEqual(["(true)"]);
    expect(replyLines({ bulk: { text: 'a"b\n' } }, T)).toEqual(['"a\\"b\\n"']);
    expect(
      replyLines({ bulk: { bytes: { hex: "0xff00", size: 2, truncated: false } } }, T),
    ).toEqual(["0xff00"]);
  });

  it("numbers arrays and indents nested ones", () => {
    const reply: KvReply = {
      array: [{ bulk: { text: "a" } }, { array: [{ int: { int: 1 } }, { array: [] }] }, "nil"],
    };
    expect(replyLines(reply, T)).toEqual([
      '1) "a"',
      `2) 1) ${T.integer("1")}`,
      `   2) ${T.empty}`,
      "3) (nil)",
    ]);
    expect(replyLines({ array: [] }, T)).toEqual([T.empty]);
  });

  it("pads the numbers of a long array to one width", () => {
    const lines = replyLines({ array: Array.from({ length: 10 }, () => ({ int: { int: 0 } })) }, T);
    expect(lines[0]).toBe(` 1) ${T.integer("0")}`);
    expect(lines[9]).toBe(`10) ${T.integer("0")}`);
  });

  it("writes maps as key => value", () => {
    const reply: KvReply = {
      map: [
        { key: { bulk: { text: "f" } }, value: { bulk: { text: "v" } } },
        { key: { status: "n" }, value: { array: [{ int: { int: 1 } }, { int: { int: 2 } }] } },
      ],
    };
    expect(replyLines(reply, T)).toEqual([
      '1# "f" => "v"',
      `2# n => 1) ${T.integer("1")}`,
      `        2) ${T.integer("2")}`,
    ]);
  });

  it("says where the console limit cut the reply", () => {
    const reply: KvReply = {
      truncated: {
        reply: { array: [{ int: { int: 1 } }] },
        elements: 20_000,
        bytes: 5 * 1024 * 1024,
      },
    };
    expect(replyLines(reply, T)).toEqual([
      `1) ${T.integer("1")}`,
      T.truncated(20_000, formatSize(5 * 1024 * 1024)),
    ]);
  });

  it("stops at the line limit", () => {
    const lines = replyLines(
      { array: Array.from({ length: MAX_REPLY_LINES + 5 }, () => "nil" as const) },
      T,
    );
    expect(lines).toHaveLength(MAX_REPLY_LINES + 1);
    expect(lines.at(-1)).toBe("…");
  });
});
