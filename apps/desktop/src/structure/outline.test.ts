// 아웃라인 모델(outline.ts): 두 출처(LSP · 인덱스)가 같은 트리가 되는지, 커서 따라가기 · 정렬 · 거르기.

import { Text } from "@codemirror/state";
import { searchKey } from "@pitwall/keymap";
import type { IndexFileSymbols, LspDocumentSymbols, LspOutlineSymbol } from "@pitwall/rpc-client";
import { describe, expect, it, vi } from "vitest";

import {
  filterRows,
  fromIndex,
  fromLsp,
  fuzzyMatches,
  innermostAt,
  MAX_DEPTH,
  MAX_SYMBOLS,
  type Outline,
  type OutlineSymbol,
  symbolKeys,
  visibleRows,
} from "./outline";

// 거르기가 키 입력마다 검색 키를 새로 만들지 않는지 센다(M4-10 리뷰 M1).
vi.mock("@pitwall/keymap", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@pitwall/keymap")>();
  return { ...actual, searchKey: vi.fn(actual.searchKey) };
});

const TEXT = Text.of(["struct 점 {", "    x: u8,", "}", "fn main() {}"]);

const lsp: LspDocumentSymbols = {
  server: "rust-analyzer",
  staleVersion: false,
  truncated: false,
  symbols: [
    {
      name: "점",
      detail: null,
      kind: 23,
      deprecated: false,
      parent: null,
      from: 0,
      to: 23,
      selectionFrom: 7,
      selectionTo: 8,
    },
    {
      name: "x",
      detail: "u8",
      kind: 8,
      deprecated: false,
      parent: 0,
      from: 15,
      to: 20,
      selectionFrom: 15,
      selectionTo: 16,
    },
    {
      name: "main",
      detail: "fn()",
      kind: 12,
      deprecated: false,
      parent: null,
      from: 24,
      to: 36,
      selectionFrom: 27,
      selectionTo: 31,
    },
  ],
};

const pos = (line: number, character: number) => ({ line, character });

const index: IndexFileSymbols = {
  language: "Rust",
  symbols: [
    {
      name: "점",
      kind: "struct",
      parent: null,
      range: { start: pos(0, 0), end: pos(2, 1) },
      selectionRange: { start: pos(0, 7), end: pos(0, 8) },
    },
    {
      name: "x",
      kind: "field",
      parent: 0,
      range: { start: pos(1, 4), end: pos(1, 9) },
      selectionRange: { start: pos(1, 4), end: pos(1, 5) },
    },
    {
      name: "main",
      kind: "function",
      parent: null,
      range: { start: pos(3, 0), end: pos(3, 12) },
      selectionRange: { start: pos(3, 3), end: pos(3, 7) },
    },
  ],
};

const shape = (o: Outline) =>
  o.symbols.map((s) => [s.name, s.kind, s.parent ?? null, s.depth, s.from, s.to, s.selectionFrom]);

describe("outline", () => {
  it("builds the same tree from the server's answer and from the symbol index", () => {
    const a = fromLsp(lsp);
    const b = fromIndex(index, TEXT);
    expect(a.source).toBe("lsp");
    expect(a.server).toBe("rust-analyzer");
    expect(b.source).toBe("index");
    expect(shape(a)).toEqual([
      ["점", "struct", null, 0, 0, 23, 7],
      ["x", "field", 0, 1, 15, 20, 15],
      ["main", "function", null, 0, 24, 36, 27],
    ]);
    expect(shape(b)).toEqual(shape(a));
    expect(a.roots).toEqual([0, 2]);
    expect(a.symbols[0]!.children).toEqual([1]);
    expect(a.symbols[1]!.detail).toBe("u8");
  });

  it("puts a symbol whose parent is not before it at the top, and pulls positions outside the text to its end", () => {
    const broken = fromLsp({
      ...lsp,
      symbols: [{ ...lsp.symbols[1]!, parent: 5 }],
    });
    expect(broken.roots).toEqual([0]);
    expect(broken.symbols[0]!.depth).toBe(0);
    const far = fromIndex(
      {
        language: "Rust",
        symbols: [
          {
            ...index.symbols[2]!,
            range: { start: pos(3, 0), end: pos(99, 0) },
            selectionRange: { start: pos(50, 0), end: pos(50, 1) },
          },
        ],
      },
      TEXT,
    );
    expect([far.symbols[0]!.to, far.symbols[0]!.selectionFrom]).toEqual([TEXT.length, TEXT.length]);
  });

  it("picks the innermost symbol holding a position, the end included", () => {
    const o = fromLsp(lsp);
    expect(innermostAt(o, 16)).toBe(1);
    expect(innermostAt(o, 20)).toBe(1);
    expect(innermostAt(o, 21)).toBe(0);
    expect(innermostAt(o, 30)).toBe(2);
    expect(innermostAt({ ...o, symbols: [] }, 30)).toBeUndefined();
  });

  it("orders rows by position or by name and hides collapsed children", () => {
    const o = fromLsp(lsp);
    const names = (rows: { id: number }[]) => rows.map((r) => o.symbols[r.id]!.name);
    expect(names(visibleRows(o, "position", new Set()))).toEqual(["점", "x", "main"]);
    expect(names(visibleRows(o, "name", new Set()))).toEqual(["main", "점", "x"]);
    expect(names(visibleRows(o, "position", new Set([0])))).toEqual(["점", "main"]);
  });

  it("filters by a fuzzy name match and keeps the ancestors dim", () => {
    const o = fromLsp(lsp);
    expect(fuzzyMatches("getName", "gtNm")).toBe(true);
    expect(fuzzyMatches("getName", "ngt")).toBe(false);
    // 자모 단위 — 조합 중인 「저」 도 「점」 에 맞는다.
    expect(fuzzyMatches("점", "저")).toBe(true);
    const { rows } = filterRows(o, "x", "position");
    expect(rows.map((r) => [o.symbols[r.id]!.name, r.match])).toEqual([
      ["점", false],
      ["x", true],
    ]);
    expect(filterRows(o, "", "position").rows).toHaveLength(3);
    expect(filterRows(o, "zz", "position")).toEqual({ rows: [], total: 0 });
    // 앞에서 `limit` 줄까지, 전체 수는 따로.
    expect(filterRows(o, "", "position", 2)).toMatchObject({ total: 3 });
    expect(filterRows(o, "", "position", 2).rows).toHaveLength(2);
    expect(filterRows(o, "x", "position", 1)).toEqual({
      rows: [{ id: 0, depth: 0, match: false }],
      total: 2,
    });
  });

  it("cuts a deep chain from the index at 32 levels and marks it truncated, whatever the source", () => {
    const chain = (depth: number): IndexFileSymbols => ({
      language: "Rust",
      symbols: Array.from({ length: depth }, (_, i) => ({
        ...index.symbols[2]!,
        name: `f${i}`,
        parent: i === 0 ? null : i - 1,
      })),
    });
    for (const depth of [MAX_DEPTH, 20_000]) {
      const o = fromIndex(chain(depth), TEXT);
      expect(o.symbols).toHaveLength(MAX_DEPTH);
      expect(o.symbols.at(-1)!.depth).toBe(MAX_DEPTH - 1);
      expect(o.truncated).toBe(depth > MAX_DEPTH);
      expect(visibleRows(o, "position", new Set())).toHaveLength(MAX_DEPTH);
      expect(filterRows(o, "f", "position").total).toBe(MAX_DEPTH);
    }
    // 서버 답도 같은 자리를 지난다. 버린 심볼 뒤의 형제는 남는다.
    const deep: LspOutlineSymbol[] = Array.from({ length: 40 }, (_, i) => ({
      ...lsp.symbols[2]!,
      name: `g${i}`,
      parent: i === 0 ? null : i - 1,
    }));
    const o = fromLsp({ ...lsp, symbols: [...deep, { ...lsp.symbols[2]!, parent: 0 }] });
    expect(o.symbols).toHaveLength(MAX_DEPTH + 1);
    expect(o.truncated).toBe(true);
    expect(o.symbols.at(-1)!.parent).toBe(0);
  });

  it("marks an index answer at the symbol limit as cut, and keeps at most that many from any source", () => {
    const flat = (length: number) => ({
      language: "Rust",
      symbols: Array.from({ length }, (_, i) => ({ ...index.symbols[2]!, name: `f${i}` })),
    });
    expect(fromIndex(flat(10), TEXT).truncated).toBe(false);
    expect(fromIndex(flat(MAX_SYMBOLS), TEXT).truncated).toBe(true);
    const many = fromLsp({
      ...lsp,
      symbols: Array.from({ length: MAX_SYMBOLS + 5 }, () => lsp.symbols[2]!),
    });
    expect(many.symbols).toHaveLength(MAX_SYMBOLS);
    expect(many.truncated).toBe(true);
  });

  it("clips long names and details at 256 UTF-8 bytes on a character boundary", () => {
    const o = fromIndex(
      { language: "Rust", symbols: [{ ...index.symbols[2]!, name: "가".repeat(100) }] },
      TEXT,
    );
    // 「가」 는 3 바이트 — 85 글자(255 바이트)에 `…`.
    expect(o.symbols[0]!.name).toBe(`${"가".repeat(85)}…`);
    const d = fromLsp({ ...lsp, symbols: [{ ...lsp.symbols[2]!, detail: "a".repeat(300) }] });
    expect(d.symbols[0]!.detail).toBe(`${"a".repeat(256)}…`);
    expect(fromLsp(lsp).symbols[2]!.name).toBe("main");
  });

  it("walks a tree deeper than the limit without recursion", () => {
    // 상한을 거치지 않은 트리(그리는 함수가 상한에 기대지 않는다).
    const depth = 50_000;
    const symbols: OutlineSymbol[] = Array.from({ length: depth }, (_, i) => ({
      id: i,
      name: `f${i}`,
      kind: "function",
      deprecated: false,
      ...(i > 0 && { parent: i - 1 }),
      depth: i,
      children: i + 1 < depth ? [i + 1] : [],
      from: 0,
      to: 1,
      selectionFrom: 0,
      selectionTo: 0,
    }));
    const o: Outline = { source: "index", symbols, roots: [0], truncated: false };
    expect(visibleRows(o, "position", new Set())).toHaveLength(depth);
    expect(visibleRows(o, "name", new Set())).toHaveLength(depth);
    expect(filterRows(o, `f${depth - 1}`, "position").total).toBe(depth);
  });

  it("filters 20,000 symbols without building their search keys again on each key (time per key is logged)", () => {
    // 2만 심볼(Host 상한) — 모듈 200 개 아래 함수 99 개씩. 이름은 리뷰가 잰 꼴.
    const big = fromLsp({
      ...lsp,
      symbols: Array.from({ length: MAX_SYMBOLS }, (_, i) => ({
        ...lsp.symbols[2]!,
        name: `someLongerFunctionName_${i}_한글이름`,
        parent: i % 100 === 0 ? null : i - (i % 100),
        kind: i % 100 === 0 ? 2 : 12,
      })),
    });
    const keyCalls = vi.mocked(searchKey);
    // 트리가 바뀔 때 한 번(팝업을 열 때) — 키 입력과 따로 잰다.
    let at = performance.now();
    filterRows(big, "", "position", 300);
    const prepared = performance.now() - at;
    // 키 입력 하나 — 영문 · 한글 조합(ㅎ → 하 → 한) · 지움 · 맞는 것 없음.
    const keys = [
      "s",
      "sl",
      "slf",
      "slfn",
      "slf",
      "z",
      "zzz",
      "ㅎ",
      "하",
      "한",
      "한ㄱ",
      "한글",
      "",
    ];
    const times: number[] = [];
    const results = keys.map((q) => {
      keyCalls.mockClear();
      at = performance.now();
      const rows = filterRows(big, q, "position", 300).rows;
      times.push(performance.now() - at);
      // 검색 키는 질의 하나만 — 심볼마다 다시 만들지 않는다.
      expect(keyCalls.mock.calls.length, q).toBeLessThanOrEqual(1);
      return rows.filter((r) => r.match).map((r) => r.id);
    });
    // 결과(앞 300 줄)는 모든 심볼을 하나씩 본 것과 같다(앞 질의의 맞은 것만 다시 보는 증분 거르기도).
    const full = keys.map((q) => filterRows(big, q, "position").rows);
    // 벽시계는 콘솔에만 남긴다(부하에 흔들리는 최댓값은 단언하지 않는다 — 계획 M4-10 ⓗ 의 규칙). 목표는 키 입력당 30 ms(결정 M4-D19 ⑤).
    console.log(
      `[perf] file structure filter, 20,000 symbols: prepare ${prepared.toFixed(1)} ms, ms per key ${times.map((t) => t.toFixed(1)).join(" / ")}`,
    );
    keys.forEach((q, i) => {
      const brute = big.symbols.filter((s) => fuzzyMatches(s.name, q)).map((s) => s.id);
      expect(
        full[i]!.filter((r) => r.match).map((r) => r.id),
        q,
      ).toEqual(brute);
      expect(results[i], q).toEqual(
        full[i]!.slice(0, 300)
          .filter((r) => r.match)
          .map((r) => r.id),
      );
    });
  });

  it("gives a symbol the same key when a re-read shifts the indexes", () => {
    const a = fromLsp(lsp);
    const b = fromLsp({
      ...lsp,
      symbols: [
        lsp.symbols[2]!,
        ...lsp.symbols.map((s) => ({ ...s, parent: s.parent === null ? null : s.parent + 1 })),
      ],
    });
    const [ka, kb] = [symbolKeys(a), symbolKeys(b)];
    // a: 점, x, main / b: main, 점, x, main
    expect(kb[1]).toBe(ka[0]);
    expect(kb[2]).toBe(ka[1]);
    expect(kb[0]).toBe(ka[2]);
    expect(new Set(ka).size).toBe(3);
  });
});
