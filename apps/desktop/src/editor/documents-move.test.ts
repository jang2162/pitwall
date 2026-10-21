// @vitest-environment happy-dom
// 문서 모델이 이 앱의 파일 조작을 따라간다(documents.ts `move` · `markDeleted`, 12-m1-plan M1-21): 이름을 바꾸거나 옮긴 파일의
// 문서는 같은 `LiveText`(미저장 편집 · undo 기록)로 새 경로에 가고, 잡은 쪽의 놓기 함수도 새 경로를 따라간다. 지운 파일은 곧바로
// 「지워짐」 이다.

import { EditorView } from "@codemirror/view";
import { ERROR_CODE, type FsRead, type FsStat } from "@pitwall/rpc-client";
import { afterEach, describe, expect, it, vi } from "vitest";

import {
  createDocumentStore,
  type DocumentChange,
  type DocumentSource,
  type TextDocumentState,
} from "./documents";
import type { LiveText } from "./live-text";

/** 경로 → 내용. 이름을 바꿔도 수정 시각이 남는다(`mtime` 은 늘 1). */
function memoryDisk(files: Record<string, string>) {
  const disk = new Map(Object.entries(files));
  let readGate: Promise<void> | undefined;
  const missing = (path: string) =>
    Object.assign(new Error(`not found: ${path}`), { code: ERROR_CODE.NOT_FOUND });
  const source = {
    stat: vi.fn(async (path: string): Promise<FsStat> => {
      const text = disk.get(path);
      if (text === undefined) throw missing(path);
      return {
        path,
        kind: "file",
        target: null,
        size: text.length,
        mtimeMs: 1,
        readonly: false,
        sizeClass: "editable",
      };
    }),
    read: vi.fn(async ({ path }: { path: string }): Promise<FsRead> => {
      await readGate;
      const text = disk.get(path);
      if (text === undefined) throw missing(path);
      return {
        path,
        size: text.length,
        mtimeMs: 1,
        offset: 0,
        eof: true,
        version: `v-${text}`,
        content: { kind: "text", text },
        judgment: {
          binary: false,
          encoding: "UTF-8",
          bom: null,
          lineEnding: "lf",
          longLines: false,
          sizeClass: "editable",
          editable: true,
          highlight: true,
        },
      };
    }),
    /** 확장자로만 정하는 가짜 `lang.detect`. */
    detect: vi.fn(async (path: string) => (path.endsWith(".rs") ? "Rust" : null)),
  } satisfies DocumentSource;
  return {
    disk,
    source,
    /** 다음 읽기들을 `release` 까지 잡아 둔다. */
    holdReads() {
      let release!: () => void;
      readGate = new Promise<void>((r) => (release = r));
      return () => {
        readGate = undefined;
        release();
      };
    },
    rename(from: string, to: string) {
      for (const [path, text] of Array.from(disk))
        if (path === from || path.startsWith(`${from}/`)) {
          disk.delete(path);
          disk.set(to + path.slice(from.length), text);
        }
    },
  };
}

const views: EditorView[] = [];
afterEach(() => {
  for (const view of views.splice(0)) view.destroy();
});

const flush = () => new Promise((r) => setTimeout(r, 0));

function attachView(live: LiveText, panelId: string) {
  const made = live.createState(panelId, []);
  const view = new EditorView({
    parent: document.body,
    state: made.state,
    dispatchTransactions: (trs, v) => live.dispatch(trs, v),
  });
  views.push(view);
  live.attach(panelId, view, () => undefined);
  return view;
}

describe("move", () => {
  it("carries the live text, unsaved edits and undo history to the new path, and the hold follows", async () => {
    const fake = memoryDisk({ "/p/a.txt": "hello" });
    const docs = createDocumentStore(fake.source);
    const hold = docs.acquire("/p/a.txt");
    await flush();
    const before = docs.get("/p/a.txt") as TextDocumentState;
    const view = attachView(before.live, "tab");
    view.dispatch({ changes: { from: 5, insert: " world" }, userEvent: "input.type" });
    expect(docs.unsaved()).toEqual(["/p/a.txt"]);

    fake.rename("/p/a.txt", "/p/b.txt");
    expect(docs.move("/p/a.txt", "/p/b.txt")).toEqual([["/p/a.txt", "/p/b.txt"]]);
    expect(docs.get("/p/a.txt")).toBeUndefined();
    const after = docs.get("/p/b.txt") as TextDocumentState;
    expect(after.live).toBe(before.live);
    expect(after.live.text.toString()).toBe("hello world");
    expect(after.dirty).toBe(true);
    expect(docs.unsaved()).toEqual(["/p/b.txt"]);
    expect(hold.path).toBe("/p/b.txt");
    expect(docs.holders("/p/b.txt")).toBe(1);
    // undo 기록이 남았다.
    expect(after.live.undo(view)).toBe(true);
    expect(after.live.text.toString()).toBe("hello");
    // 옮긴 뒤 편집 · 미저장 바뀜이 새 경로로 나간다.
    view.dispatch({ changes: { from: 0, insert: "> " }, userEvent: "input.type" });
    expect((docs.get("/p/b.txt") as TextDocumentState).dirty).toBe(true);
    // 디스크에서 다시 읽지 않았다.
    expect(fake.source.read).toHaveBeenCalledTimes(1);

    // 놓기 함수는 새 경로의 문서를 놓는다.
    hold();
    expect(docs.get("/p/b.txt")).toBeUndefined();
    expect(docs.holders("/p/b.txt")).toBe(0);
  });

  it("moves every document under a folder and tells change listeners at the new path", async () => {
    const fake = memoryDisk({ "/p/dir/a.txt": "a", "/p/dir/sub/b.txt": "b", "/p/dir2/c.txt": "c" });
    const docs = createDocumentStore(fake.source);
    for (const path of fake.disk.keys()) docs.acquire(path);
    await flush();
    const changes: DocumentChange[] = [];
    docs.onChange("/p/moved/sub/b.txt", (c) => changes.push(c));

    fake.rename("/p/dir", "/p/moved");
    const moved = docs.move("/p/dir", "/p/moved");
    expect(moved.sort()).toEqual([
      ["/p/dir/a.txt", "/p/moved/a.txt"],
      ["/p/dir/sub/b.txt", "/p/moved/sub/b.txt"],
    ]);
    // 이름이 `dir` 로 시작할 뿐인 형제 폴더는 그대로다.
    expect(docs.get("/p/dir2/c.txt")?.status).toBe("text");
    expect(changes.map((c) => c.kind)).toEqual(["reset"]);
  });

  it("clears a deleted banner the watcher put up before the move landed", async () => {
    const fake = memoryDisk({ "/p/a.txt": "a" });
    const docs = createDocumentStore(fake.source);
    docs.acquire("/p/a.txt");
    await flush();
    // 감시 이벤트가 응답보다 먼저 와 옛 경로를 「이름 바뀜」 으로 보았다.
    fake.rename("/p/a.txt", "/p/b.txt");
    await docs.changed({
      root: "/p",
      worktree: null,
      paths: ["/p/a.txt", "/p/b.txt"],
      kinds: ["removed", "created"],
      rescan: false,
    });
    expect((docs.get("/p/a.txt") as TextDocumentState).disk.kind).toBe("renamed");
    docs.move("/p/a.txt", "/p/b.txt");
    expect((docs.get("/p/b.txt") as TextDocumentState).disk).toEqual({ kind: "same" });
  });

  it("judges the language and viewer again by the new name and keeps the unsaved edits", async () => {
    const fake = memoryDisk({ "/p/a.txt": "fn main() {}", "/p/notes.txt": "# notes" });
    const docs = createDocumentStore(fake.source);
    docs.acquire("/p/a.txt");
    docs.acquire("/p/notes.txt");
    await flush();
    const before = docs.get("/p/a.txt") as TextDocumentState;
    expect([before.language, before.kind]).toEqual([null, "text"]);
    const view = attachView(before.live, "tab");
    view.dispatch({ changes: { from: 0, insert: "// x\n" }, userEvent: "input.type" });

    fake.rename("/p/a.txt", "/p/a.rs");
    docs.move("/p/a.txt", "/p/a.rs");
    await flush();
    const after = docs.get("/p/a.rs") as TextDocumentState;
    expect(after.language).toBe("Rust");
    expect(after.live).toBe(before.live);
    expect(after.dirty).toBe(true);
    expect(fake.source.detect).toHaveBeenLastCalledWith("/p/a.rs", "// x\nfn main() {}", undefined);

    fake.rename("/p/notes.txt", "/p/notes.md");
    docs.move("/p/notes.txt", "/p/notes.md");
    expect((docs.get("/p/notes.md") as TextDocumentState).kind).toBe("markdown");
    // 이름으로 이미지가 되어도 텍스트 문서로 남는다(미저장 편집을 잃지 않게).
    fake.rename("/p/notes.md", "/p/notes.png");
    docs.move("/p/notes.md", "/p/notes.png");
    expect(docs.get("/p/notes.png")).toMatchObject({ status: "text", kind: "markdown" });
    // 내용을 뷰어가 직접 읽는 문서(이미지)는 종류가 바뀌면 새 경로로 다시 읽는다.
    fake.disk.set("/p/pic.png", "x");
    docs.acquire("/p/pic.png");
    await flush();
    expect(docs.get("/p/pic.png")).toMatchObject({ status: "external", kind: "image" });
    fake.rename("/p/pic.png", "/p/pic.txt");
    docs.move("/p/pic.png", "/p/pic.txt");
    await flush();
    expect(docs.get("/p/pic.txt")).toMatchObject({ status: "text", kind: "text" });
  });

  it("reads again at the new path when the document was still loading", async () => {
    const fake = memoryDisk({ "/p/a.txt": "a" });
    const docs = createDocumentStore(fake.source);
    const release = fake.holdReads();
    docs.acquire("/p/a.txt");
    await flush();
    expect(docs.get("/p/a.txt")?.status).toBe("loading");
    fake.rename("/p/a.txt", "/p/b.txt");
    docs.move("/p/a.txt", "/p/b.txt");
    release();
    await flush();
    await flush();
    expect(docs.get("/p/a.txt")).toBeUndefined();
    const doc = docs.get("/p/b.txt") as TextDocumentState;
    expect(doc.status).toBe("text");
    expect(doc.live.text.toString()).toBe("a");
  });
});

describe("markDeleted", () => {
  it("puts text documents under the path in the deleted state at once", async () => {
    const fake = memoryDisk({ "/p/dir/a.txt": "a", "/p/keep.txt": "k" });
    const docs = createDocumentStore(fake.source);
    docs.acquire("/p/dir/a.txt");
    docs.acquire("/p/keep.txt");
    await flush();
    docs.markDeleted("/p/dir");
    expect((docs.get("/p/dir/a.txt") as TextDocumentState).disk).toEqual({ kind: "deleted" });
    expect((docs.get("/p/keep.txt") as TextDocumentState).disk).toEqual({ kind: "same" });
  });
});
