// 심볼 검색의 질의 · 합치기(symbol-search.ts, 15-m4-plan M4-16): 인덱스만 · 서버만 · 둘 다(겹침 제거 · 순서), 서버가 늦으면 인덱스
// 결과가 먼저(200 ms) · 뒤에 더해짐, 클래스로 이동이 함수를 뺌, 요청 하나씩 · 마지막 것만, 「바쁨」 재시도, 취소된 파일 목록의 다시
// 만들기, 상한.

import type {
  IndexStatus,
  IndexSymbolKind,
  IndexSymbolMatch,
  IndexSymbolSearch,
  IndexSymbolSearchParams,
  LspWorkspaceSymbol,
  LspWorkspaceSymbolParams,
  LspWorkspaceSymbols,
} from "@pitwall/rpc-client";
import { RpcCallError } from "@pitwall/rpc-client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { isTypeKind, kindOfLsp, lspKindsOf, TYPE_KINDS } from "./symbol-kinds";
import {
  BUSY_RETRY_MS,
  LSP_GRACE_MS,
  mergeSymbols,
  NAME_LIMIT,
  SYMBOL_LIMIT,
  SymbolSearch,
  type SymbolSearchState,
} from "./symbol-search";

const SCOPE = { projectId: "p1", worktree: null };

function indexMatch(
  name: string,
  line: number,
  kind: IndexSymbolKind = "function",
  file = "src/lib.rs",
): IndexSymbolMatch {
  const at = {
    start: { line: line - 1, character: 3 },
    end: { line: line - 1, character: 3 + name.length },
  };
  return {
    name,
    kind,
    container: null,
    path: `/work/demo/${file}`,
    root: "/work/demo",
    relative: file,
    language: "Rust",
    range: at,
    selectionRange: at,
    positions: [0],
    score: 1,
  };
}

function indexAnswer(symbols: IndexSymbolMatch[], unindexed: string[] = []): IndexSymbolSearch {
  return { symbols, matched: symbols.length, indexed: 500, unindexed };
}

function lspSymbol(name: string, line: number, kind = 12, file = "src/lib.rs"): LspWorkspaceSymbol {
  return {
    name,
    kind,
    container: "crate",
    server: "rust-analyzer",
    language: "Rust",
    relative: file,
    location: {
      path: `/private/work/demo/${file}`,
      token: null,
      line,
      column: 4,
      endLine: line,
      endColumn: 4 + name.length,
      preview: null,
    },
  };
}

function lspAnswer(
  symbols: LspWorkspaceSymbol[],
  servers = ["rust-analyzer"],
): LspWorkspaceSymbols {
  return { symbols, servers, failed: [], truncated: false };
}

/** 부를 때마다 매달린 약속을 하나 만든다 — 시험이 풀 때까지 답하지 않는다. */
function pendingCalls<P, R>() {
  const calls: { params: P; signal: AbortSignal; resolve(r: R): void; reject(e: unknown): void }[] =
    [];
  const fn = (params: P, signal: AbortSignal) =>
    new Promise<R>((resolve, reject) => calls.push({ params, signal, resolve, reject }));
  return { calls, fn };
}

const flush = () => vi.advanceTimersByTimeAsync(0);

function setup(options: { lsp?: boolean } = {}) {
  const index = pendingCalls<IndexSymbolSearchParams, IndexSymbolSearch>();
  const lsp = pendingCalls<LspWorkspaceSymbolParams, LspWorkspaceSymbols>();
  const status = vi.fn<(s: typeof SCOPE) => Promise<IndexStatus>>(async () => ({ roots: [] }));
  const rebuild = vi.fn(async () => ({}));
  const search = new SymbolSearch({
    index: index.fn,
    ...(options.lsp !== false && { lsp: lsp.fn }),
    status,
    rebuild,
  });
  const seen: SymbolSearchState[] = [];
  search.subscribe(() => seen.push(search.get()));
  return { search, index, lsp, status, rebuild, seen };
}

const names = (s: SymbolSearchState) => s.rows.map((r) => `${r.source}:${r.name}`);

beforeEach(() => vi.useFakeTimers());
afterEach(() => vi.useRealTimers());

describe("SymbolSearch", () => {
  it("shows index results alone when the window has no language servers", async () => {
    const { search, index } = setup({ lsp: false });
    search.request("wid", false, SCOPE);
    expect(index.calls[0]!.params).toEqual({
      projectId: "p1",
      worktree: null,
      query: "wid",
      kinds: null,
      limit: SYMBOL_LIMIT,
      offset: null,
    });
    index.calls[0]!.resolve(indexAnswer([indexMatch("Widget", 3, "struct")]));
    await flush();
    // 기다릴 서버가 없다 — 200 ms 를 기다리지 않는다.
    expect(names(search.get())).toEqual(["index:Widget"]);
    expect(search.get().pending).toBe(false);
    expect(search.get().rows[0]).toMatchObject({
      kind: "struct",
      line: 3,
      column: 4,
      relative: "src/lib.rs",
    });
  });

  it("shows server results alone, with LSP kinds mapped to index kinds", async () => {
    const { search, index, lsp } = setup();
    search.request("wid", false, SCOPE);
    expect(lsp.calls[0]!.params).toEqual({
      projectId: "p1",
      worktree: null,
      query: "wid",
      kinds: null,
      limit: SYMBOL_LIMIT,
    });
    lsp.calls[0]!.resolve(lspAnswer([lspSymbol("Widget", 3, 23), lspSymbol("render", 9, 6)]));
    index.calls[0]!.resolve(indexAnswer([]));
    await flush();
    expect(names(search.get())).toEqual(["lsp:Widget", "lsp:render"]);
    expect(search.get().rows.map((r) => r.kind)).toEqual(["struct", "method"]);
    expect(search.get().rows[0]).toMatchObject({
      server: "rust-analyzer",
      container: "crate",
      path: "/private/work/demo/src/lib.rs",
    });
  });

  it("merges both: one row for the same file, line and name, server results first", async () => {
    const { search, index, lsp, seen } = setup();
    search.request("wid", false, SCOPE);
    index.calls[0]!.resolve(
      indexAnswer([
        indexMatch("Widget", 3, "struct"),
        indexMatch("widget_id", 12, "function", "src/ui.rs"),
        indexMatch("Widget", 20, "struct", "src/other.rs"),
      ]),
    );
    await flush();
    // 서버를 기다린다 — 인덱스만 그리지 않는다.
    expect(seen.filter((s) => s.rows.length > 0)).toEqual([]);
    // 서버의 실제 경로는 심링크를 지나 인덱스 경로와 다르지만 상대 경로 · 줄 · 이름이 같으면 하나다.
    lsp.calls[0]!.resolve(lspAnswer([lspSymbol("Widget", 3, 23), lspSymbol("WidgetKind", 7, 10)]));
    await flush();
    expect(names(search.get())).toEqual([
      "lsp:Widget",
      "lsp:WidgetKind",
      "index:widget_id",
      "index:Widget",
    ]);
    expect(search.get().rows[3]!.relative).toBe("src/other.rs");
    // 서버는 4열(선언 처음 등)을 줬어도 인덱스가 아는 이름 자리(4열 — 같다)로. 다른 열이면 인덱스 것.
    expect(search.get().rows[0]).toMatchObject({ source: "lsp", column: 4 });
    // 한 번에 그렸다(인덱스만 보인 순간이 없다).
    expect(seen.filter((s) => s.rows.length > 0)).toHaveLength(1);
    expect(search.get().lspPending).toBe(false);
  });

  it("draws the index results after 200 ms when the server is slow and adds the server's when they come", async () => {
    const { search, index, lsp, seen } = setup();
    search.request("wid", false, SCOPE);
    index.calls[0]!.resolve(
      indexAnswer([indexMatch("Widget", 3, "struct"), indexMatch("wide", 5)]),
    );
    await vi.advanceTimersByTimeAsync(LSP_GRACE_MS - 1);
    expect(search.get().rows).toEqual([]);
    await vi.advanceTimersByTimeAsync(1);
    expect(names(search.get())).toEqual(["index:Widget", "index:wide"]);
    expect(search.get().lspPending).toBe(true);
    await vi.advanceTimersByTimeAsync(2000);
    lsp.calls[0]!.resolve(lspAnswer([lspSymbol("WidgetKind", 7, 10), lspSymbol("Widget", 3, 23)]));
    await flush();
    expect(names(search.get())).toEqual(["lsp:WidgetKind", "lsp:Widget", "index:wide"]);
    expect(search.get().lspPending).toBe(false);
    expect(seen.filter((s) => s.rows.length > 0).map((s) => s.rows.length)).toEqual([2, 3]);
  });

  it("does not wait for servers once the last answer had none (untrusted or none running)", async () => {
    const { search, index, lsp } = setup();
    search.request("a", false, SCOPE);
    lsp.calls[0]!.reject(
      new RpcCallError({ code: -32001, message: "untrusted", data: { reason: "untrusted" } }),
    );
    index.calls[0]!.resolve(indexAnswer([indexMatch("a", 1)]));
    await flush();
    expect(names(search.get())).toEqual(["index:a"]);
    search.request("ab", false, SCOPE);
    await flush();
    index.calls[1]!.resolve(indexAnswer([indexMatch("ab", 2)]));
    await flush();
    // 서버 요청은 여전히 가지만 그 답을 기다리지 않는다.
    expect(lsp.calls).toHaveLength(2);
    expect(names(search.get())).toEqual(["index:ab"]);
    expect(search.get().error).toBeUndefined();
  });

  it("asks only for type kinds and drops functions for Go to Class", async () => {
    const { search, index, lsp } = setup();
    search.request("w", true, SCOPE);
    expect(index.calls[0]!.params.kinds).toEqual([...TYPE_KINDS]);
    expect(lsp.calls[0]!.params.kinds).toEqual([5, 10, 11, 23, 26]);
    // 출처가 거르지 않고 함수를 줘도 남지 않는다.
    lsp.calls[0]!.resolve(lspAnswer([lspSymbol("make_widget", 1, 12), lspSymbol("Widget", 3, 23)]));
    index.calls[0]!.resolve(
      indexAnswer([indexMatch("widget", 5, "function"), indexMatch("WidgetTrait", 8, "trait")]),
    );
    await flush();
    expect(names(search.get())).toEqual(["lsp:Widget", "index:WidgetTrait"]);
  });

  it("keeps one request in flight per source and sends only the last query typed meanwhile", async () => {
    const { search, index, lsp } = setup();
    search.request("w", false, SCOPE);
    search.request("wi", false, SCOPE);
    search.request("wid", false, SCOPE);
    expect(index.calls.map((c) => c.params.query)).toEqual(["w"]);
    expect(lsp.calls.map((c) => c.params.query)).toEqual(["w"]);
    // 느린 서버가 인덱스의 차례를 막지 않는다.
    index.calls[0]!.resolve(indexAnswer([indexMatch("w", 1)]));
    await flush();
    expect(index.calls.map((c) => c.params.query)).toEqual(["w", "wid"]);
    expect(lsp.calls.map((c) => c.params.query)).toEqual(["w"]);
    // 옛 질의의 답은 그리지 않는다.
    expect(search.get().rows).toEqual([]);
    lsp.calls[0]!.resolve(lspAnswer([lspSymbol("w", 1)]));
    await flush();
    expect(lsp.calls.map((c) => c.params.query)).toEqual(["w", "wid"]);
    index.calls[1]!.resolve(indexAnswer([indexMatch("wid", 2)]));
    lsp.calls[1]!.resolve(lspAnswer([]));
    await flush();
    expect(names(search.get())).toEqual(["index:wid"]);
    expect(search.get().query).toBe("wid");
  });

  it("keeps the results when the index is busy and sends the last query again", async () => {
    const { search, index, lsp } = setup({ lsp: false });
    void lsp;
    search.request("a", false, SCOPE);
    index.calls[0]!.resolve(indexAnswer([indexMatch("a", 1)]));
    await flush();
    search.request("ab", false, SCOPE);
    index.calls[1]!.reject(
      new RpcCallError({
        code: -32603,
        message: "internal error",
        data: "the symbol index is busy: 4 requests are already running",
      }),
    );
    await flush();
    expect(names(search.get())).toEqual(["index:a"]);
    expect(search.get()).toMatchObject({ busy: true, pending: true });
    await vi.advanceTimersByTimeAsync(BUSY_RETRY_MS - 1);
    expect(index.calls).toHaveLength(2);
    await vi.advanceTimersByTimeAsync(1);
    expect(index.calls.map((c) => c.params.query)).toEqual(["a", "ab", "ab"]);
    index.calls[2]!.resolve(indexAnswer([indexMatch("ab", 2)]));
    await flush();
    expect(names(search.get())).toEqual(["index:ab"]);
    expect(search.get()).toMatchObject({ busy: false, error: undefined });
  });

  it("tells a cancelled file list apart and rebuilds it through the file search before asking again", async () => {
    const { search, index, status, rebuild } = setup({ lsp: false });
    status.mockResolvedValue({
      roots: [{ root: "/work/demo", state: "suspended", files: 0, symbols: 0, pending: 0 }],
    });
    search.request("a", false, SCOPE);
    index.calls[0]!.resolve(indexAnswer([], ["/work/demo"]));
    await flush();
    expect(status).toHaveBeenCalledWith(SCOPE);
    expect(search.get().suspended).toBe(true);
    const done = search.rebuild();
    await flush();
    expect(rebuild).toHaveBeenCalledWith(SCOPE);
    await done;
    expect(index.calls.map((c) => c.params.query)).toEqual(["a", "a"]);
    index.calls[1]!.resolve(indexAnswer([indexMatch("a", 1)]));
    await flush();
    expect(search.get()).toMatchObject({ suspended: false });
    expect(names(search.get())).toEqual(["index:a"]);
  });

  it("does not report a root that is only unwatched as cancelled", async () => {
    const { search, index, status } = setup({ lsp: false });
    status.mockResolvedValue({
      roots: [{ root: "/work/demo", state: "none", files: 0, symbols: 0, pending: 0 }],
    });
    search.request("a", false, SCOPE);
    index.calls[0]!.resolve(indexAnswer([], ["/work/demo"]));
    await flush();
    expect(status).toHaveBeenCalled();
    expect(search.get().suspended).toBe(false);
  });

  it("cancels what is in flight when stopped", async () => {
    const { search, index, lsp } = setup();
    search.request("a", false, SCOPE);
    search.stop();
    expect(index.calls[0]!.signal.aborted).toBe(true);
    expect(lsp.calls[0]!.signal.aborted).toBe(true);
  });
});

describe("mergeSymbols", () => {
  it("takes the index's name column for a server symbol that points at the start of its declaration line", () => {
    const declaration = {
      ...lspSymbol("Widget", 3, 5),
      location: { ...lspSymbol("Widget", 3).location, column: 1 },
    };
    const { rows } = mergeSymbols(
      lspAnswer([declaration]),
      indexAnswer([indexMatch("Widget", 3, "class")]),
      "w",
      false,
    );
    expect(rows).toHaveLength(1);
    expect(rows[0]).toMatchObject({ source: "lsp", line: 3, column: 4, server: "rust-analyzer" });
  });

  it("caps the rows and the name length whatever the source", () => {
    const long = "x".repeat(NAME_LIMIT + 40);
    const index = indexAnswer(
      Array.from({ length: 150 }, (_, i) => indexMatch(i === 0 ? long : `s${i}`, i + 1)),
    );
    const lsp = lspAnswer([{ ...lspSymbol(long, 900), container: long }]);
    const { rows, truncated } = mergeSymbols(lsp, index, "s", false);
    expect(rows).toHaveLength(SYMBOL_LIMIT);
    expect(truncated).toBe(true);
    expect(rows[0]!.name).toHaveLength(NAME_LIMIT);
    expect(rows[0]!.container).toHaveLength(NAME_LIMIT);
    expect(rows[1]!.name).toHaveLength(NAME_LIMIT);
  });

  it("keeps an outside token and the server's language for the read-only tab", () => {
    const lsp = lspAnswer([
      {
        ...lspSymbol("Vec", 400, 23),
        relative: null,
        location: {
          ...lspSymbol("Vec", 400).location,
          path: "/rust/library/alloc/src/vec/mod.rs",
          token: "tok-1",
        },
      },
    ]);
    const [row] = mergeSymbols(lsp, undefined, "vec", false).rows;
    expect(row).toMatchObject({
      token: "tok-1",
      language: "Rust",
      path: "/rust/library/alloc/src/vec/mod.rs",
    });
    expect(row!.relative).toBeUndefined();
    expect(row!.positions).toEqual([0, 1, 2]);
  });
});

describe("symbol kinds", () => {
  it("keeps one table: Go to Class takes types and leaves functions and methods out", () => {
    expect(TYPE_KINDS.every(isTypeKind)).toBe(true);
    for (const k of [
      "function",
      "method",
      "constructor",
      "field",
      "variable",
      "constant",
      "module",
    ] as const)
      expect(isTypeKind(k), k).toBe(false);
    expect(lspKindsOf(TYPE_KINDS).map(kindOfLsp)).toEqual([
      "class",
      "enum",
      "interface",
      "struct",
      "typeAlias",
    ]);
    expect(kindOfLsp(0)).toBe("variable");
    expect(kindOfLsp(99)).toBe("variable");
  });
});
