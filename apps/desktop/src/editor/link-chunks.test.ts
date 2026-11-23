// 조각 읽기 · 쓰기(18-m7-plan M7-13b)의 자르기와 다시 읽기. 창을 거친 시험은 reconnect-window.test.tsx.
import { ERROR_CODE, RpcCallError } from "@pitwall/rpc-client";
import type { FsContent, FsReadChunk } from "@pitwall/rpc-client";
import { describe, expect, it } from "vitest";

import { readInChunks, splitTextByWire, writeInChunks } from "./link-chunks";
import { LINK_FRAME_BYTES, REMOTE_MESSAGE_LIMIT, requestBytes } from "./link-limits";
import { CHUNK_WIRE_MAX } from "./limits";

const MIB = 1024 * 1024;
const wire = (text: string) => new TextEncoder().encode(JSON.stringify(text)).length;
/** 제어 문자 줄(JSON 에서 6배) — 8 KiB 뒤에 NUL 하나. */
function worstText(bytes: number): string {
  const text = `${"\u0001".repeat(63)}\n`.repeat(bytes / 64);
  return `${text.slice(0, 10_000)}\u0000${text.slice(10_001)}`;
}

describe("splitTextByWire", () => {
  it("keeps every piece within the budget and joins back to the text", () => {
    const text = `a"\\\n\u0001\u0000한😀\ud800x`.repeat(5_000);
    for (const budget of [16, 17, 100, 4096]) {
      const pieces = splitTextByWire(text, budget);
      expect(pieces.join("")).toBe(text);
      for (const p of pieces) expect(wire(p)).toBeLessThanOrEqual(budget);
      // 대리 쌍을 가르지 않는다.
      for (let i = 1; i < pieces.length; i++)
        expect(/[\ud800-\udbff]$/.test(pieces[i - 1]!) && /^[\udc00-\udfff]/.test(pieces[i]!)).toBe(
          false,
        );
    }
    expect(splitTextByWire("")).toEqual([""]);
  });
});

describe("writeInChunks", () => {
  it("sends 30 MiB of worst-case text without any request over the link frame", async () => {
    const text = worstText(30 * MIB);
    expect(requestBytes("fs.write", { content: { kind: "text", text } })).toBeGreaterThan(
      LINK_FRAME_BYTES,
    );
    const sizes: number[] = [];
    const received: string[] = [];
    const written = await writeInChunks(
      {
        chunk: async (params) => {
          const request = { projectId: "r1", path: "/work/demo/big.log", ...params };
          sizes.push(requestBytes("fs.writeChunk", request));
          received.push((params.content as { text: string }).text);
          return { upload: "u1", received: 0 };
        },
        commit: async (params) => {
          sizes.push(requestBytes("fs.writeCommit", params));
          return { path: "/work/demo/big.log", size: 0, mtimeMs: 1, version: "v2" };
        },
        abort: async () => {},
      },
      { kind: "text", text },
      false,
      "v1",
    );
    expect(written.version).toBe("v2");
    expect(received.join("")).toBe(text);
    expect(received.length).toBeGreaterThan(1);
    expect(Math.max(...sizes)).toBeLessThanOrEqual(CHUNK_WIRE_MAX + 64 * 1024);
    expect(Math.max(...sizes)).toBeLessThan(REMOTE_MESSAGE_LIMIT);
  });

  it("splits base64 into pieces that decode on their own", async () => {
    const data = btoa("x".repeat(100));
    const pieces: string[] = [];
    await writeInChunks(
      {
        chunk: async ({ content }) => {
          pieces.push((content as { data: string }).data);
          return { upload: "u1", received: 0 };
        },
        commit: async () => ({ path: "p", size: 0, mtimeMs: null, version: "v" }),
        abort: async () => {},
      },
      { kind: "base64", data },
      false,
      null,
      30,
    );
    expect(pieces.length).toBeGreaterThan(1);
    expect(pieces.map((p) => atob(p)).join("")).toBe("x".repeat(100));
  });

  it("discards the upload when a chunk fails, and never commits", async () => {
    const aborted: string[] = [];
    let commits = 0;
    let n = 0;
    await expect(
      writeInChunks(
        {
          chunk: async () => {
            if (++n === 2) throw new Error("link dropped");
            return { upload: "u7", received: 0 };
          },
          commit: async () => {
            commits++;
            return { path: "p", size: 0, mtimeMs: null, version: "v" };
          },
          abort: async (upload) => void aborted.push(upload),
        },
        { kind: "text", text: "a".repeat(100) },
        false,
        null,
        20,
      ),
    ).rejects.toThrow("link dropped");
    expect(aborted).toEqual(["u7"]);
    expect(commits).toBe(0);
  });
});

describe("readInChunks", () => {
  const judgment = {
    binary: false,
    encoding: "UTF-8",
    lossy: false,
    bom: null,
    lineEnding: "lf",
    longLines: false,
    sizeClass: "editable",
    editable: true,
    highlight: false,
  } as const;
  /** `file()` 을 조각마다 다시 보고, `expectedVersion` 이 다르면 `CONFLICT` 인 가짜 Host(crates/pw-fs/src/chunks.rs 의 규칙). */
  function host(file: () => { text: string; version: string }, size = 4) {
    const calls: { offset: number; expectedVersion: string | null }[] = [];
    const read = async (p: {
      offset: number;
      maxBytes: number;
      expectedVersion: string | null;
    }) => {
      calls.push({ offset: p.offset, expectedVersion: p.expectedVersion });
      const { text, version } = file();
      if (p.expectedVersion !== null && p.expectedVersion !== version)
        throw new RpcCallError({ code: ERROR_CODE.CONFLICT, message: "conflict", data: null });
      const next = Math.min(text.length, p.offset + size);
      const content: FsContent = { kind: "text", text: text.slice(p.offset, next) };
      return {
        path: "/f",
        size: text.length,
        mtimeMs: 1,
        version,
        judgment,
        offset: p.offset,
        next,
        total: text.length,
        eof: next === text.length,
        content,
      } satisfies FsReadChunk;
    };
    return { calls, read };
  }

  it("reads again from the start when the file changed between chunks, and gives the new version", async () => {
    let state = { text: "aaaaaaaaaa", version: "v1" };
    const h = host(() => state);
    const reading = readInChunks(async (p) => {
      const r = await h.read(p);
      // 첫 조각 뒤에 파일이 바뀐다.
      if (h.calls.length === 1) state = { text: "bbbbbbbbbbbb", version: "v2" };
      return r;
    });
    const read = await reading;
    expect(read.version).toBe("v2");
    expect(read.content).toEqual({ kind: "text", text: "bbbbbbbbbbbb" });
    expect(h.calls[1]).toEqual({ offset: 4, expectedVersion: "v1" });
    expect(h.calls[2]).toEqual({ offset: 0, expectedVersion: null });
  });

  it("gives up with the conflict when the file keeps changing", async () => {
    let n = 0;
    const h = host(() => ({ text: "aaaaaaaaaa", version: `v${n++}` }));
    await expect(readInChunks(h.read)).rejects.toMatchObject({ code: ERROR_CODE.CONFLICT });
  });
});
