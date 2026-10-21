// @vitest-environment happy-dom
// 문서 모델의 저장과 디스크 맞추기(documents.ts, 12-m1-plan M1-13 완료 기준 ①~④): 가짜 디스크(바이트 · 수정 시각 · `version`)에
// `fs.write` 의 기대 version 규칙을 그대로 두고 본다.

import { EditorView } from "@codemirror/view";
import {
  ERROR_CODE,
  type FsRead,
  type FsStat,
  type FsWriteParams,
  type FsWritten,
  type LineEnding,
} from "@pitwall/rpc-client";
import { afterEach, describe, expect, it, vi } from "vitest";

import {
  createDocumentStore,
  type DocumentSource,
  type FsChangedEvent,
  type TextDocumentState,
} from "./documents";
import type { LiveText } from "./live-text";

const BOM = "\uFEFF";

function rpcError(code: number, message: string) {
  return Object.assign(new Error(message), { code });
}

function lineEndingOf(text: string): LineEnding {
  const crlf = (text.match(/\r\n/g) ?? []).length;
  const cr = (text.match(/\r(?!\n)/g) ?? []).length;
  const lf = (text.match(/(?<!\r)\n/g) ?? []).length;
  const kinds = [crlf, cr, lf].filter((n) => n > 0).length;
  if (kinds === 0) return "none";
  if (kinds > 1) return "mixed";
  return crlf ? "crlf" : cr ? "cr" : "lf";
}

/** 가짜 디스크. 내용은 바이트 대신 문자열(BOM 은 U+FEFF)이고, `version` 은 pw-fs 처럼 크기 · 수정 시각 · 내용으로 정한다. */
class Disk {
  files = new Map<string, { bytes: string; mtime: number }>();
  private clock = 100;
  /** `fs.write` 가 디스크에 쓴 뒤, 응답하기 전에 부른다(감시의 이벤트가 응답보다 먼저 오는 경우). */
  afterWrite: ((path: string) => Promise<void>) | undefined;
  failWrite: Error | undefined;

  put(path: string, bytes: string) {
    this.files.set(path, { bytes, mtime: ++this.clock });
  }

  /** `mv from to`: 수정 시각이 남는다. */
  move(from: string, to: string) {
    this.files.set(to, this.files.get(from)!);
    this.files.delete(from);
  }

  version(path: string): string | null {
    const f = this.files.get(path);
    return f ? `${f.bytes.length}-${f.mtime}-${f.bytes}` : null;
  }

  private file(path: string) {
    const f = this.files.get(path);
    if (!f) throw rpcError(ERROR_CODE.NOT_FOUND, `not found: ${path}`);
    return f;
  }

  source = {
    stat: vi.fn(async (path: string): Promise<FsStat> => {
      const f = this.file(path);
      return {
        path,
        kind: "file",
        target: null,
        size: f.bytes.length,
        mtimeMs: f.mtime,
        readonly: false,
        sizeClass: "editable",
      };
    }),
    read: vi.fn(async ({ path }: { path: string }): Promise<FsRead> => {
      const f = this.file(path);
      const bom = f.bytes.startsWith(BOM);
      const text = bom ? f.bytes.slice(1) : f.bytes;
      return {
        path,
        size: f.bytes.length,
        mtimeMs: f.mtime,
        offset: 0,
        eof: true,
        version: this.version(path),
        content: { kind: "text", text },
        judgment: {
          binary: false,
          encoding: "UTF-8",
          bom: bom ? "utf8" : null,
          lineEnding: lineEndingOf(text),
          longLines: false,
          sizeClass: "editable",
          editable: true,
          highlight: true,
        },
      };
    }),
    write: vi.fn(
      async (
        params: Pick<FsWriteParams, "path" | "content" | "bom" | "expectedVersion">,
      ): Promise<FsWritten> => {
        if (this.failWrite) throw this.failWrite;
        const { path, content, bom, expectedVersion } = params;
        if (this.version(path) !== (expectedVersion ?? null))
          throw rpcError(ERROR_CODE.CONFLICT, "conflict");
        if (content.kind !== "text") throw new Error("text only");
        this.put(path, (bom ? BOM : "") + content.text);
        await this.afterWrite?.(path);
        const f = this.files.get(path)!;
        return { path, size: f.bytes.length, mtimeMs: f.mtime, version: this.version(path)! };
      },
    ),
  } satisfies DocumentSource;
}

const views: EditorView[] = [];

afterEach(() => {
  for (const view of views.splice(0)) view.destroy();
});

const flush = () => new Promise((r) => setTimeout(r, 0));

/** `path` 를 열어 텍스트 문서가 될 때까지 기다리고, 탭 하나의 뷰를 붙인다. */
async function openDoc(disk: Disk, path: string) {
  const docs = createDocumentStore(disk.source);
  docs.acquire(path);
  await flush();
  const doc = () => docs.get(path) as TextDocumentState;
  const live: LiveText = doc().live;
  const made = live.createState("tab", []);
  const view = new EditorView({
    parent: document.body,
    state: made.state,
    dispatchTransactions: (trs, v) => live.dispatch(trs, v),
  });
  views.push(view);
  live.attach("tab", view, () => undefined);
  return { docs, doc, view };
}

function type(view: EditorView, at: number, insert: string) {
  view.dispatch({ changes: { from: at, insert }, userEvent: "input.type" });
}

function event(paths: string[], kinds: FsChangedEvent["kinds"]): FsChangedEvent {
  return { root: "/p", worktree: null, paths, kinds, rescan: false };
}

const A = "/p/a.txt";

describe("saving", () => {
  it("writes with the version it read, and keeps CRLF and the BOM byte for byte (④)", async () => {
    const disk = new Disk();
    disk.put(A, `${BOM}first\r\nsecond\r\n`);
    const { docs, doc, view } = await openDoc(disk, A);
    const before = disk.version(A);
    type(view, view.state.doc.line(2).to, " line");
    expect(doc().dirty).toBe(true);

    expect(await docs.save(A)).toEqual({ kind: "saved" });
    expect(disk.source.write.mock.calls[0]![0]).toMatchObject({
      expectedVersion: before,
      bom: true,
    });
    expect(disk.files.get(A)!.bytes).toBe(`${BOM}first\r\nsecond line\r\n`);
    expect(doc()).toMatchObject({ dirty: false, version: disk.version(A), disk: { kind: "same" } });

    // 쓸 것이 없으면 쓰지 않는다.
    expect(await docs.save(A)).toEqual({ kind: "unchanged" });
    expect(disk.source.write).toHaveBeenCalledTimes(1);
  });

  it("keeps mixed line endings of untouched lines", async () => {
    const disk = new Disk();
    disk.put(A, "a\r\nb\nc\r");
    const { docs, view } = await openDoc(disk, A);
    type(view, view.state.doc.line(2).to, "!");
    await docs.save(A);
    expect(disk.files.get(A)!.bytes).toBe("a\r\nb!\nc\r");
  });

  it("does not overwrite a change the watch missed: the save stops with a conflict (③)", async () => {
    const disk = new Disk();
    disk.put(A, "mine\n");
    const { docs, doc, view } = await openDoc(disk, A);
    type(view, 0, "my ");
    disk.put(A, "theirs\n"); // fs.changed 없이

    expect(await docs.save(A)).toEqual({ kind: "conflict" });
    expect(disk.files.get(A)!.bytes).toBe("theirs\n");
    expect(doc().disk).toMatchObject({ kind: "changed", read: { content: { text: "theirs\n" } } });
    expect(view.state.doc.toString()).toBe("my mine\n");

    // [내 것 유지] 뒤의 저장은 지금 디스크를 덮는다.
    docs.keepMine(A);
    expect(doc().disk).toEqual({ kind: "same" });
    expect(await docs.save(A)).toEqual({ kind: "saved" });
    expect(disk.files.get(A)!.bytes).toBe("my mine\n");
  });

  it("reports a failed write and stays unsaved", async () => {
    const disk = new Disk();
    disk.put(A, "x");
    const { docs, doc, view } = await openDoc(disk, A);
    type(view, 1, "y");
    disk.failWrite = new Error("disk full");
    expect(await docs.save(A)).toEqual({ kind: "failed", message: "disk full" });
    expect(doc()).toMatchObject({ dirty: true, saving: false, disk: { kind: "same" } });
  });
});

describe("changes on disk", () => {
  it("does not take its own write for an external change, even when the event comes before the reply", async () => {
    const disk = new Disk();
    disk.put(A, "a\n");
    const { docs, doc, view } = await openDoc(disk, A);
    // 배너 상태가 잠깐이라도 생기는지 모두 적는다(끝 상태만 보면 저장이 마지막에 덮어 가려진다).
    const seen: string[] = [];
    docs.store.subscribe(({ docs: all }) => {
      const now = all[A];
      if (now?.status === "text" && now.disk.kind !== "same") seen.push(now.disk.kind);
    });
    let echo: Promise<void> | undefined;
    disk.afterWrite = async (path) => {
      echo = docs.changed(event([path], ["modified"]));
      await flush();
    };
    type(view, 1, "b");
    await docs.save(A);
    await echo;
    expect(seen).toEqual([]);
    expect(doc()).toMatchObject({ dirty: false, disk: { kind: "same" } });
    expect(view.state.doc.toString()).toBe("ab\n");
  });

  it("takes the new text when there are no unsaved edits, keeping the cursor line (①)", async () => {
    const disk = new Disk();
    const lines = Array.from({ length: 40 }, (_, i) => `line ${i + 1}`);
    disk.put(A, lines.join("\n"));
    const { docs, doc, view } = await openDoc(disk, A);
    view.dispatch({ selection: { anchor: view.state.doc.line(20).from + 3 } });

    lines[2] = "line 3 — edited by an agent";
    lines[35] = "line 36 — edited by an agent";
    disk.put(A, lines.join("\n"));
    await docs.changed(event([A], ["modified"]));
    expect(view.state.doc.toString()).toBe(lines.join("\n"));
    expect(view.state.doc.lineAt(view.state.selection.main.head).number).toBe(20);
    expect(doc()).toMatchObject({ dirty: false, version: disk.version(A), disk: { kind: "same" } });
  });

  it("keeps unsaved edits and shows the disk version when there are some (②), until one is chosen", async () => {
    const disk = new Disk();
    disk.put(A, "base\n");
    const { docs, doc, view } = await openDoc(disk, A);
    type(view, 0, "mine ");
    disk.put(A, "theirs\n");
    await docs.changed(event([A], ["modified"]));
    expect(view.state.doc.toString()).toBe("mine base\n");
    expect(doc().disk).toMatchObject({ kind: "changed", read: { content: { text: "theirs\n" } } });

    await docs.takeDisk(A);
    expect(view.state.doc.toString()).toBe("theirs\n");
    expect(doc()).toMatchObject({ dirty: false, disk: { kind: "same" }, version: disk.version(A) });
  });

  it("marks a deleted file, and saving it creates it again", async () => {
    const disk = new Disk();
    disk.put(A, "keep me");
    const { docs, doc } = await openDoc(disk, A);
    disk.files.delete(A);
    await docs.changed(event([A], ["removed"]));
    expect(doc().disk).toEqual({ kind: "deleted" });

    expect(await docs.save(A)).toEqual({ kind: "saved" });
    expect(disk.source.write.mock.calls[0]![0]).toMatchObject({ expectedVersion: null });
    expect(disk.files.get(A)!.bytes).toBe("keep me");
    expect(doc().disk).toEqual({ kind: "same" });
  });

  it("recognizes a rename: a file with the same content appears as the old one goes", async () => {
    const disk = new Disk();
    disk.put(A, "content");
    disk.put("/p/other.txt", "content!");
    const { docs, doc } = await openDoc(disk, A);
    disk.move(A, "/p/b.txt");
    await docs.changed(event([A, "/p/b.txt", "/p/other.txt"], ["removed", "created", "created"]));
    expect(doc().disk).toEqual({ kind: "renamed", to: "/p/b.txt" });
  });

  it("looks again at every open document under the root after a rescan", async () => {
    const disk = new Disk();
    disk.put(A, "old");
    const { docs, view } = await openDoc(disk, A);
    disk.put(A, "new");
    await docs.changed({ root: "/p", worktree: null, paths: [], kinds: [], rescan: true });
    expect(view.state.doc.toString()).toBe("new");
  });
});
