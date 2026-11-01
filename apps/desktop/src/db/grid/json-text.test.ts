// 원래 글을 지키는 json 파서(M3-16 리뷰): RFC 8259 문법 그대로 받고, 수는 받은 글 그대로, 너무 깊으면 던진다(쓰는 쪽은 글로 보인다).

import { describe, expect, it } from "vitest";

import { type JsonText, MAX_DEPTH, parseJsonText, writeJsonText } from "./json-text";

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

  it("takes MAX_DEPTH nested arrays and objects and refuses one more", () => {
    // 배열 · 객체를 번갈아 n 겹(가장 안은 수 하나).
    const nested = (n: number) => {
      let text = "1";
      for (let d = 0; d < n; d++) text = d % 2 === 0 ? `[${text}]` : `{"k":${text}}`;
      return text;
    };
    const ok = nested(MAX_DEPTH);
    expect(() => JSON.parse(ok)).not.toThrow();
    expect(writeJsonText(parseJsonText(ok)).replaceAll(/\s/g, "")).toBe(ok);
    expect(() => parseJsonText(`${"[".repeat(MAX_DEPTH)}${"]".repeat(MAX_DEPTH)}`)).not.toThrow();
    // 한 겹 더: JSON.parse 는 받지만 여기는 거절한다(쓰는 쪽이 원래 글로 보인다).
    const deep = nested(MAX_DEPTH + 1);
    expect(() => JSON.parse(deep)).not.toThrow();
    expect(() => parseJsonText(deep)).toThrow(/too deep/);
    expect(() => parseJsonText(`${"[".repeat(MAX_DEPTH + 1)}${"]".repeat(MAX_DEPTH + 1)}`)).toThrow(
      /too deep/,
    );
    expect(() => parseJsonText(`{"a":${"[".repeat(MAX_DEPTH)}${"]".repeat(MAX_DEPTH)}}`)).toThrow(
      /too deep/,
    );
    // 스택을 넘길 만큼 깊어도 던질 뿐이다.
    expect(() => parseJsonText("[".repeat(100_000))).toThrow(/too deep/);
  });

  it("accepts and refuses exactly what JSON.parse does on random near-JSON text", () => {
    // 무작위 JSON(깊이 ≤ 4 — MAX_DEPTH 밖은 위 시험)을 무작위 공백 · 수 꼴 · 탈출로 쓰고, 그 글을 무작위로 망가뜨린 것까지
    // JSON.parse 와 견준다. 받은 것은 다시 써도 JSON.parse 로 같은 값이다. 긴 판은 `PW_JSON_FUZZ_CASES=200000`,
    // 실패하면 시드가 보인다 — `PW_JSON_FUZZ_SEED=<시드> PW_JSON_FUZZ_CASES=1` 로 그 경우만.
    const cases = Number(process.env.PW_JSON_FUZZ_CASES ?? 20_000);
    const firstSeed = Number(process.env.PW_JSON_FUZZ_SEED ?? 1);
    let accepted = 0;
    for (let seed = firstSeed; seed < firstSeed + cases; seed++) {
      const text = nearJson(seed);
      let expected: unknown;
      let refused = false;
      try {
        expected = JSON.parse(text);
      } catch {
        refused = true;
      }
      let got: JsonText | undefined;
      try {
        got = parseJsonText(text);
      } catch (e) {
        expect(e, `seed ${seed}: ${JSON.stringify(text)}`).toBeInstanceOf(SyntaxError);
      }
      expect(got === undefined, `seed ${seed}: ${JSON.stringify(text)}`).toBe(refused);
      if (got !== undefined) {
        accepted++;
        expect(JSON.parse(writeJsonText(got)), `seed ${seed}`).toEqual(expected);
      }
    }
    // 받는 쪽 · 거절하는 쪽을 둘 다 고르게 본다.
    if (cases >= 1000) {
      expect(accepted / cases).toBeGreaterThan(0.2);
      expect(accepted / cases).toBeLessThan(0.8);
    }
  });

  it("round-trips the reviewer's value through the writer unchanged", () => {
    const text = '{"id": 9007199254740993, "d": 0.10000000000000000555}';
    expect(writeJsonText(parseJsonText(text))).toBe(
      '{\n  "id": 9007199254740993,\n  "d": 0.10000000000000000555\n}',
    );
  });
});

/** 시드로 정해지는 난수(mulberry32). */
function random(seed: number): () => number {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

/** JSON 이거나 JSON 에서 몇 글자 어긋난 글. */
function nearJson(seed: number): string {
  const r = random(seed);
  const pick = <V>(xs: readonly V[]): V => xs[Math.floor(r() * xs.length)]!;
  const ws = () => (r() < 0.7 ? "" : pick([" ", "\t", "\n", "\r", "  ", "\u00a0", "\v"]));
  const NUMBERS = [
    "0",
    "-0",
    "1",
    "-1",
    "10",
    "9007199254740993",
    "1.5",
    "0.10000000000000000555",
    "1e5",
    "1E+5",
    "-2.5e-3",
    "01",
    "1.",
    ".5",
    "+1",
    "-",
    "1e",
    "0x10",
    "Infinity",
    "NaN",
    "1_000",
  ] as const;
  const STRINGS = [
    '""',
    '"a"',
    '"\\n"',
    '"\\u00e9"',
    '"\\ud800"',
    '"\\x41"',
    '"\\u12"',
    '"\\/"',
    '"a\tb"',
    '"\u0001"',
    '"\u2028"',
    '"\\"',
    "'a'",
    '"\\""',
    '"🍎"',
  ] as const;
  const KEYS = ['"a"', '"b"', '""', '"a"', '"\\u0061"', "a", "1"] as const;
  const value = (depth: number): string => {
    const k = r();
    if (depth < 4 && k < 0.2) {
      const n = Math.floor(r() * 4);
      return `[${ws()}${Array.from({ length: n }, () => ws() + value(depth + 1) + ws()).join(",")}]`;
    }
    if (depth < 4 && k < 0.4) {
      const n = Math.floor(r() * 4);
      const entries = Array.from(
        { length: n },
        () => `${ws()}${pick(KEYS)}${ws()}:${ws()}${value(depth + 1)}${ws()}`,
      );
      return `{${entries.join(",")}${ws()}}`;
    }
    if (k < 0.6) return pick(NUMBERS);
    if (k < 0.8) return pick(STRINGS);
    return pick(["true", "false", "null", "tru", "nul", "True"]);
  };
  let text = ws() + value(0) + ws();
  // 절반쯤은 몇 글자를 망가뜨린다.
  const NOISE = ["{", "}", "[", "]", ",", ":", '"', "\\", "-", "0", "e", ".", " ", "a", "\u0000"];
  if (r() < 0.5) {
    for (let m = 1 + Math.floor(r() * 3); m > 0; m--) {
      const at = Math.floor(r() * (text.length + 1));
      const op = r();
      if (op < 0.4) text = text.slice(0, at) + text.slice(at + 1);
      else if (op < 0.8) text = text.slice(0, at) + pick(NOISE) + text.slice(at);
      else text = text.slice(0, at) + text.slice(at, at + 3) + text.slice(at);
    }
  }
  return text;
}
