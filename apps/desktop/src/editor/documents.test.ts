import type { FsRead, FsStat, Judgment, SizeClass } from "@pitwall/rpc-client";
import { describe, expect, it, vi } from "vitest";

import { createDocumentStore, type DocumentSource, SNIFF_ABOVE, toText } from "./documents";

const MIB = 1024 * 1024;

const TEXT_JUDGMENT: Judgment = {
  binary: false,
  encoding: "UTF-8",
  bom: null,
  lineEnding: "lf",
  longLines: false,
  sizeClass: "editable",
  editable: true,
  highlight: true,
};

/** 가짜 디스크. `links` 는 요청 경로 → 실제 경로. */
function fakeSource(
  files: Record<string, { size?: number; text?: string; binary?: boolean; utf8?: boolean }>,
  links: Record<string, string> = {},
) {
  const real = (p: string) => links[p] ?? p;
  const sizeOf = (p: string) => files[p]!.size ?? files[p]!.text?.length ?? 0;
  const sizeClass = (size: number): SizeClass =>
    size > 50 * MIB ? "tooLarge" : size > 5 * MIB ? "readOnly" : "editable";
  const source = {
    stat: vi.fn(async (path: string): Promise<FsStat> => {
      const p = real(path);
      if (!files[p]) throw new Error(`not found: ${path}`);
      return {
        path: p,
        kind: "file",
        target: null,
        size: sizeOf(p),
        mtimeMs: 1,
        readonly: false,
        sizeClass: sizeClass(sizeOf(p)),
      };
    }),
    read: vi.fn(
      async ({ path, length }: { path: string; length?: number | null }): Promise<FsRead> => {
        const f = files[path]!;
        const size = sizeOf(path);
        const cls = sizeClass(size);
        const judgment = { ...TEXT_JUDGMENT, binary: f.binary === true, sizeClass: cls };
        return {
          path,
          size,
          mtimeMs: 1,
          offset: 0,
          eof: length === undefined,
          version: length === undefined ? "v1" : null,
          content:
            f.binary || f.utf8 === false
              ? { kind: "base64", data: "AAAA" }
              : { kind: "text", text: f.text ?? "" },
          judgment: { ...judgment, editable: cls === "editable", highlight: cls === "editable" },
        };
      },
    ),
  } satisfies DocumentSource;
  return source;
}

const flush = () => new Promise((r) => setTimeout(r, 0));

describe("document store", () => {
  it("keeps one document per real path, shared by every tab that holds it", async () => {
    const source = fakeSource({ "/p/a.rs": { text: "a\nb" } }, { "/p/link.rs": "/p/a.rs" });
    const docs = createDocumentStore(source);
    const release1 = docs.acquire("/p/a.rs");
    const release2 = docs.acquire("/p/a.rs");
    await flush();
    expect(source.read).toHaveBeenCalledTimes(1);
    const doc = docs.get("/p/a.rs");
    expect(doc).toMatchObject({ status: "text", kind: "text", version: "v1", readOnly: false });
    expect(doc?.status === "text" && doc.text.toString()).toBe("a\nb");
    expect(docs.holders("/p/a.rs")).toBe(2);

    release1();
    release1(); // 두 번 놓아도 한 번이다.
    expect(docs.get("/p/a.rs")).toBeDefined();
    release2();
    expect(docs.get("/p/a.rs")).toBeUndefined();
    expect(docs.holders("/p/a.rs")).toBe(0);
  });

  it("drops a document released before its read finished", async () => {
    const source = fakeSource({ "/p/a.rs": { text: "a" } });
    const docs = createDocumentStore(source);
    docs.acquire("/p/a.rs")();
    await flush();
    expect(docs.get("/p/a.rs")).toBeUndefined();
  });

  it("does not read a file over 50 MiB, and opens a 5 MiB+ file read-only", async () => {
    const source = fakeSource({
      "/p/huge.log": { size: 60 * MIB, text: "" },
      "/p/big.log": { size: 6 * MIB, text: "x" },
    });
    const docs = createDocumentStore(source);
    docs.acquire("/p/huge.log");
    docs.acquire("/p/big.log");
    await flush();
    expect(docs.get("/p/huge.log")).toEqual({ status: "tooLarge", size: 60 * MIB });
    expect(source.read.mock.calls.map(([p]) => p.path)).not.toContain("/p/huge.log");
    expect(docs.get("/p/big.log")).toMatchObject({ status: "text", readOnly: true });
  });

  it("reads only the head of a large binary file, and nothing of an image", async () => {
    const source = fakeSource({
      "/p/blob.bin": { size: SNIFF_ABOVE + 1, binary: true },
      "/p/logo.svg": { text: "<svg/>" },
      "/p/small.bin": { size: 10, binary: true },
    });
    const docs = createDocumentStore(source);
    for (const p of ["/p/blob.bin", "/p/logo.svg", "/p/small.bin"]) docs.acquire(p);
    await flush();
    expect(docs.get("/p/blob.bin")).toMatchObject({ status: "external", kind: "hex" });
    expect(docs.get("/p/small.bin")).toMatchObject({ status: "external", kind: "hex" });
    expect(docs.get("/p/logo.svg")).toMatchObject({ status: "external", kind: "image" });
    expect(source.read.mock.calls.map(([p]) => [p.path, p.length])).toEqual([
      ["/p/blob.bin", 8 * 1024],
      ["/p/small.bin", undefined],
    ]);
  });

  it("marks text that is not UTF-8, and a failed read", async () => {
    const source = fakeSource({ "/p/latin1.txt": { text: "x", utf8: false } });
    const docs = createDocumentStore(source);
    docs.acquire("/p/latin1.txt");
    docs.acquire("/p/gone.txt");
    await flush();
    expect(docs.get("/p/latin1.txt")).toEqual({ status: "notUtf8", size: 1 });
    expect(docs.get("/p/gone.txt")).toEqual({
      status: "failed",
      message: "not found: /p/gone.txt",
    });
  });

  it("opens Markdown as text for its viewer", async () => {
    const source = fakeSource({ "/p/README.md": { text: "# hi" } });
    const docs = createDocumentStore(source);
    docs.acquire("/p/README.md");
    await flush();
    expect(docs.get("/p/README.md")).toMatchObject({ status: "text", kind: "markdown" });
  });
});

it("splits text on the same line breaks as CodeMirror", () => {
  expect(toText("a\r\nb\rc\nd").lines).toBe(4);
  expect(toText("").lines).toBe(1);
});
