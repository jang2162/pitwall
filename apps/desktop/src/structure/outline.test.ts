// 아웃라인 모델(outline.ts): 두 출처(LSP · 인덱스)가 같은 트리가 되는지, 커서 따라가기 · 정렬 · 거르기.

import { Text } from "@codemirror/state";
import type { IndexFileSymbols, LspDocumentSymbols } from "@pitwall/rpc-client";
import { describe, expect, it } from "vitest";

import {
  filterRows,
  fromIndex,
  fromLsp,
  fuzzyMatches,
  innermostAt,
  type Outline,
  visibleRows,
} from "./outline";

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
    const rows = filterRows(o, "x", "position");
    expect(rows.map((r) => [o.symbols[r.id]!.name, r.match])).toEqual([
      ["점", false],
      ["x", true],
    ]);
    expect(filterRows(o, "", "position")).toHaveLength(3);
    expect(filterRows(o, "zz", "position")).toEqual([]);
  });
});
