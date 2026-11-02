import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { ERROR_CODE, type Judgment, RpcCallError } from "@pitwall/rpc-client";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { DocumentState } from "../editor/documents";
import { EDITABLE_MAX } from "../editor/limits";
import { createLspSessions, LSP_ERROR, lspEligible } from "./lsp-sessions";
import { fakeLspDocuments, fakeLspServer } from "./lsp-testing";

afterEach(() => {
  vi.useRealTimers();
});

const settle = () => new Promise((r) => setTimeout(r, 0));

function setup(texts: Record<string, string> = { "/p/a.rs": "fn a() {}" }, options = {}) {
  const server = fakeLspServer(options);
  const documents = fakeLspDocuments(texts);
  const sessions = createLspSessions({ source: server.source, documents, projectId: "proj" });
  return { server, documents, sessions };
}

describe("document sync", () => {
  it("opens one Host document for every tab of a document and closes it once with the last tab (②)", async () => {
    const { server, sessions } = setup();
    const one = sessions.acquire("/p/a.rs", "Rust", "/wt");
    const two = sessions.acquire("/p/a.rs", "Rust", "/wt");
    await settle();
    expect(server.called("open")).toEqual([
      {
        projectId: "proj",
        worktree: "/wt",
        path: "/p/a.rs",
        language: "Rust",
        version: 0,
        text: "fn a() {}",
      },
    ]);
    one.release();
    one.release();
    await settle();
    expect(server.called("close")).toHaveLength(0);
    two.release();
    await settle();
    expect(server.called("close")).toEqual([{ docId: "d1" }]);
  });

  it("sends each edit with the next version and reopens with the current text when the Host lost the document", async () => {
    const { server, documents, sessions } = setup();
    const handle = sessions.acquire("/p/a.rs", "Rust");
    await settle();
    documents.edit("/p/a.rs", 0, 0, "pub ");
    documents.edit("/p/a.rs", 0, 0, "// 한글\n");
    await settle();
    expect(server.called("change").map((c) => (c as { version: number }).version)).toEqual([1, 2]);
    expect(server.docs.get("d1")!.text).toBe(documents.text("/p/a.rs"));
    server.docs.delete("d1");
    documents.edit("/p/a.rs", 0, 0, "x");
    await vi.waitFor(() => expect(server.called("open")).toHaveLength(2));
    await settle();
    expect(server.docs.get("d2")!.text).toBe(documents.text("/p/a.rs"));
    expect(handle.document.version).toBe(0);
    handle.release();
  });

  it("does not open text over the edit tier, and stops on a refused open without asking again (④)", async () => {
    vi.useFakeTimers();
    const big = setup({ "/p/big.rs": "a".repeat(EDITABLE_MAX + 1) });
    big.sessions.acquire("/p/big.rs", "Rust");
    await vi.advanceTimersByTimeAsync(0);
    expect(big.server.called("open")).toHaveLength(0);

    const refused = setup({ "/p/a.rs": "x" }, { refuse: ERROR_CODE.INVALID_PARAMS });
    const handle = refused.sessions.acquire("/p/a.rs", "Rust");
    await vi.advanceTimersByTimeAsync(60_000);
    expect(refused.server.called("open")).toHaveLength(1);
    expect(await handle.document.request("hover", async () => ({ staleVersion: false }))).toBe(
      undefined,
    );
  });

  it("sends a save after the document model wrote the file", async () => {
    const { server, documents, sessions } = setup();
    sessions.acquire("/p/a.rs", "Rust");
    await settle();
    documents.save("/p/a.rs", "v1");
    await settle();
    expect(server.called("save")).toEqual([{ docId: "d1" }]);
  });
});

describe("eligibility (④)", () => {
  const judgment = (over: Partial<Judgment> = {}) =>
    ({ sizeClass: "editable", longLines: false, ...over }) as Judgment;
  const text = (over: Partial<Judgment> = {}) =>
    ({ status: "text", judgment: judgment(over) }) as DocumentState;

  it("opens only file tabs of edit-tier text documents with a language", () => {
    expect(lspEligible(true, text(), "Rust")).toBe(true);
    // git · DB 문서 탭(파일 탭이 아님).
    expect(lspEligible(false, text(), "Rust")).toBe(false);
    // 5 MiB 초과(읽기 전용 등급) · 1만 자 줄.
    expect(lspEligible(true, text({ sizeClass: "readOnly" }), "Rust")).toBe(false);
    expect(lspEligible(true, text({ longLines: true }), "Rust")).toBe(false);
    // 언어 없음 · 플레인 텍스트 · 텍스트가 아님.
    expect(lspEligible(true, text(), null)).toBe(false);
    expect(lspEligible(true, text(), "Text")).toBe(false);
    expect(lspEligible(true, { status: "loading" }, "Rust")).toBe(false);
  });
});

describe("request helper (③)", () => {
  it("drops an answer when the document changed during the request, and when the Host marks it stale", async () => {
    const { documents, sessions } = setup();
    const { document } = sessions.acquire("/p/a.rs", "Rust");
    await settle();
    // 바뀌지 않음 → 그대로.
    expect(
      await document.request("hover", async (at) => ({ staleVersion: false, at })),
    ).toMatchObject({ at: { docId: "d1", version: 0 } });
    // 요청 중 편집 → 버린다.
    let answer!: (v: { staleVersion: boolean }) => void;
    const pending = document.request(
      "hover",
      () => new Promise<{ staleVersion: boolean }>((r) => (answer = r)),
    );
    await settle();
    documents.edit("/p/a.rs", 0, 0, "x");
    answer({ staleVersion: false });
    expect(await pending).toBeUndefined();
    // Host 가 stale 이라 했다(다른 창이 서버의 텍스트를 바꿈 등) → 버린다.
    expect(await document.request("hover", async () => ({ staleVersion: true }))).toBeUndefined();
  });

  it("cancels the older request of the same kind and keeps other kinds", async () => {
    const { sessions } = setup();
    const { document } = sessions.acquire("/p/a.rs", "Rust");
    await settle();
    const signals: AbortSignal[] = [];
    const slow = (value: string) => (_: unknown, signal: AbortSignal) => {
      signals.push(signal);
      return new Promise<{ staleVersion: boolean; value: string }>((r) =>
        setTimeout(() => r({ staleVersion: false, value }), 10),
      );
    };
    const first = document.request("hover", slow("first"));
    const other = document.request("signature", slow("other"));
    await settle();
    const second = document.request("hover", slow("second"));
    expect(await first).toBeUndefined();
    expect(signals[0]!.aborted).toBe(true);
    expect(await second).toMatchObject({ value: "second" });
    expect(await other).toMatchObject({ value: "other" });
  });

  it("answers nothing when no server offers the feature, and rethrows other failures", async () => {
    const { sessions } = setup();
    const { document } = sessions.acquire("/p/a.rs", "Rust");
    const fail = (code: number) => async () => {
      throw new RpcCallError({ code, message: "x" });
    };
    expect(await document.request("hover", fail(LSP_ERROR.NO_SERVER))).toBeUndefined();
    expect(await document.request("hover", fail(ERROR_CODE.CANCELLED))).toBeUndefined();
    await expect(document.request("hover", fail(LSP_ERROR.SERVER_ERROR))).rejects.toMatchObject({
      code: LSP_ERROR.SERVER_ERROR,
    });
  });

  it("keeps the error codes of the server manager (crates/pw-lsp/src/manager.rs)", () => {
    const rust = readFileSync(
      resolve(__dirname, "../../../../crates/pw-lsp/src/manager.rs"),
      "utf8",
    );
    for (const [name, value] of Object.entries(LSP_ERROR))
      expect(rust).toContain(`pub const ${name}: i32 = ${value};`);
  });
});

describe("IME composition (⑥)", () => {
  it("holds the changes while composing and sends them once after, as one version", async () => {
    const { server, documents, sessions } = setup({ "/p/a.rs": "" });
    const { document } = sessions.acquire("/p/a.rs", "Rust");
    await settle();
    const release = document.hold();
    documents.edit("/p/a.rs", 0, 0, "ㅎ");
    documents.edit("/p/a.rs", 0, 1, "하");
    documents.edit("/p/a.rs", 0, 1, "한");
    await settle();
    expect(server.called("change")).toHaveLength(0);
    expect(document.version).toBe(0);
    // 조합 중에는 묻지 않는다.
    const call = vi.fn(async () => ({ staleVersion: false }));
    expect(await document.request("hover", call)).toBeUndefined();
    expect(call).not.toHaveBeenCalled();
    release();
    release();
    await settle();
    expect(server.called("change")).toEqual([
      { docId: "d1", version: 1, changes: [{ from: 0, to: 0, insert: "한" }] },
    ]);
    expect(server.docs.get("d1")!.text).toBe("한");
  });
});

describe("capabilities", () => {
  it("asks when the document opens and again after a status event, one request at a time", async () => {
    const { server, sessions } = setup();
    server.setFeatures({});
    const { document } = sessions.acquire("/p/a.rs", "Rust");
    const seen: unknown[] = [];
    document.onCapabilities(() => seen.push(document.capabilities));
    await vi.waitFor(() => expect(server.called("capabilities")).toHaveLength(1));
    server.setFeatures({
      completion: {
        server: "rust-analyzer",
        options: { triggerCharacters: ["."] },
        optionsTruncated: false,
      },
    });
    // 진행 알림처럼 잦은 상태 이벤트 — 묻는 중에 온 것은 끝난 뒤 한 번으로 묶인다.
    sessions.statusChanged();
    sessions.statusChanged();
    sessions.statusChanged();
    await vi.waitFor(() => expect(server.called("capabilities")).toHaveLength(3));
    await settle();
    expect(server.called("capabilities")).toHaveLength(3);
    expect(document.capabilities.completion?.options).toEqual({ triggerCharacters: ["."] });
    expect(seen.length).toBeGreaterThanOrEqual(2);
  });
});
