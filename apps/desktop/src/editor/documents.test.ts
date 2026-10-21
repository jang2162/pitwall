// @vitest-environment happy-dom
import type { EntryKind, FsRead, FsStat, Judgment, SizeClass } from "@pitwall/rpc-client";
import { EditorView } from "@codemirror/view";
import { describe, expect, it, vi } from "vitest";

import { createDocumentStore, type DocumentSource, SNIFF_ABOVE } from "./documents";
import { EDITABLE_MAX, READ_ONLY_MAX } from "./limits";

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

/**
 * 가짜 디스크. `links` 는 요청 경로 → 실제 경로. `encoding` 이 UTF-8 이 아닌 파일은 Host 처럼 그 인코딩으로 풀어 텍스트로 주고 편집을
 * 막는다(결정 M1-D12). `kind` 는 stat 의 종류(FIFO 는 `other`), `target` 은 링크가 가리키는 것의 종류.
 */
function fakeSource(
  files: Record<
    string,
    {
      size?: number;
      text?: string;
      binary?: boolean;
      encoding?: string;
      kind?: EntryKind;
      target?: EntryKind;
    }
  >,
  links: Record<string, string> = {},
) {
  const real = (p: string) => links[p] ?? p;
  const sizeOf = (p: string) => files[p]!.size ?? files[p]!.text?.length ?? 0;
  const sizeClass = (size: number): SizeClass =>
    size > READ_ONLY_MAX ? "tooLarge" : size > EDITABLE_MAX ? "readOnly" : "editable";
  const source = {
    stat: vi.fn(async (path: string): Promise<FsStat> => {
      const p = real(path);
      if (!files[p]) throw new Error(`not found: ${path}`);
      const kind = files[p]!.kind ?? "file";
      return {
        path: p,
        kind,
        target: files[p]!.target ?? null,
        size: sizeOf(p),
        mtimeMs: 1,
        readonly: false,
        sizeClass: kind === "file" ? sizeClass(sizeOf(p)) : null,
      };
    }),
    read: vi.fn(
      async ({ path, length }: { path: string; length?: number | null }): Promise<FsRead> => {
        const f = files[path]!;
        const size = sizeOf(path);
        const cls = sizeClass(size);
        const encoding = f.binary ? null : (f.encoding ?? "UTF-8");
        const judgment = { ...TEXT_JUDGMENT, binary: f.binary === true, encoding, sizeClass: cls };
        return {
          path,
          size,
          mtimeMs: 1,
          offset: 0,
          eof: length === undefined,
          version: length === undefined ? "v1" : null,
          content: f.binary
            ? { kind: "base64", data: "AAAA" }
            : { kind: "text", text: f.text ?? "" },
          judgment: {
            ...judgment,
            editable: cls === "editable" && encoding === "UTF-8",
            highlight: cls === "editable",
          },
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
    expect(doc?.status === "text" && doc.live.text.toString()).toBe("a\nb");
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
    expect(docs.get("/p/huge.log")).toEqual({ status: "tooLarge", reason: "size", size: 60 * MIB });
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

  // 옛 `notUtf8` 상태가 지키던 것: UTF-8 이 아닌 텍스트를 고칠 수 있는 문서로 열지 않는다. 이제는 Host 가 짐작한 인코딩으로 푼
  // 텍스트를 보이되 읽기 전용이고, 편집이 생겨도 저장이 디스크에 쓰지 않는다(UTF-8 로 다시 쓰면 원래 바이트가 바뀐다).
  it("opens text that is not UTF-8 read-only as the host decoded it, and never writes it", async () => {
    const source = {
      ...fakeSource({ "/p/euc-kr.txt": { text: "한글 주석", encoding: "EUC-KR" } }),
      write: vi.fn(async () => {
        throw new Error("must not write");
      }),
    };
    const docs = createDocumentStore(source);
    docs.acquire("/p/euc-kr.txt");
    await flush();
    const doc = docs.get("/p/euc-kr.txt");
    expect(doc).toMatchObject({ status: "text", readOnly: true, judgment: { encoding: "EUC-KR" } });
    if (doc?.status !== "text") throw new Error("not text");
    expect(doc.live.text.toString()).toBe("한글 주석");

    // 뷰가 막지 못한 편집(확장 없는 뷰)이 생겨도 저장은 쓰지 않는다.
    const made = doc.live.createState("tab", []);
    const view = new EditorView({
      parent: document.body,
      state: made.state,
      dispatchTransactions: (trs, v) => doc.live.dispatch(trs, v),
    });
    doc.live.attach("tab", view, () => undefined);
    view.dispatch({ changes: { from: 0, insert: "x" }, userEvent: "input.type" });
    expect(doc.live.dirty).toBe(true);
    expect(await docs.save("/p/euc-kr.txt")).toEqual({ kind: "unchanged" });
    expect(source.write).not.toHaveBeenCalled();
    view.destroy();
  });

  it("does not read a FIFO, a device or a link to one, and marks a failed read", async () => {
    const source = fakeSource(
      {
        "/p/pipe": { kind: "other" },
        "/p/link": { kind: "symlink", target: "other" },
        "/p/dangling": { kind: "symlink" },
      },
      {},
    );
    const docs = createDocumentStore(source);
    for (const p of ["/p/pipe", "/p/link", "/p/dangling", "/p/gone.txt"]) docs.acquire(p);
    await flush();
    for (const p of ["/p/pipe", "/p/link", "/p/dangling"])
      expect(docs.get(p)).toEqual({ status: "tooLarge", reason: "special", size: 0 });
    expect(source.read).not.toHaveBeenCalled();
    expect(docs.get("/p/gone.txt")).toEqual({
      status: "failed",
      message: "not found: /p/gone.txt",
    });
  });

  it("decides a 1 GB file without reading a byte, within a second", async () => {
    const source = fakeSource({ "/p/huge.bin": { size: 1024 * MIB, binary: true } });
    const docs = createDocumentStore(source);
    const started = performance.now();
    docs.acquire("/p/huge.bin");
    await vi.waitFor(() => expect(docs.get("/p/huge.bin")?.status).toBe("tooLarge"));
    expect(performance.now() - started).toBeLessThan(1000);
    expect(source.read).not.toHaveBeenCalled();
  });

  it("opens files one byte either side of the size thresholds as decided (M1-D9)", async () => {
    const files = {
      "/p/5m": { size: EDITABLE_MAX, text: "a" },
      "/p/5m+1": { size: EDITABLE_MAX + 1, text: "a" },
      "/p/50m": { size: READ_ONLY_MAX, text: "a" },
      "/p/50m+1": { size: READ_ONLY_MAX + 1, text: "a" },
    };
    const source = fakeSource(files);
    const docs = createDocumentStore(source);
    for (const p of Object.keys(files)) docs.acquire(p);
    await flush();
    expect(docs.get("/p/5m")).toMatchObject({ status: "text", readOnly: false });
    expect(docs.get("/p/5m+1")).toMatchObject({ status: "text", readOnly: true });
    expect(docs.get("/p/50m")).toMatchObject({ status: "text", readOnly: true });
    expect(docs.get("/p/50m+1")).toMatchObject({ status: "tooLarge", reason: "size" });
    expect(source.read.mock.calls.map(([p]) => p.path)).not.toContain("/p/50m+1");
  });

  it("opens Markdown as text for its viewer", async () => {
    const source = fakeSource({ "/p/README.md": { text: "# hi" } });
    const docs = createDocumentStore(source);
    docs.acquire("/p/README.md");
    await flush();
    expect(docs.get("/p/README.md")).toMatchObject({ status: "text", kind: "markdown" });
  });
});
