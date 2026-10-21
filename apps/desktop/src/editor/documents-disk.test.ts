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
  /** `fs.read` · `fs.stat` 마다 읽기 전에 부른다(읽기를 잡아 두는 시험). */
  beforeRead: (() => Promise<void>) | undefined;
  beforeStat: (() => Promise<void>) | undefined;

  put(path: string, bytes: string) {
    this.files.set(path, { bytes, mtime: ++this.clock });
  }

  /** `touch`: 내용은 그대로, 수정 시각만 바뀐다. */
  touch(path: string) {
    this.put(path, this.files.get(path)!.bytes);
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
      await this.beforeStat?.();
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
      await this.beforeRead?.();
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
          lossy: false,
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

/** 탭 `panelId` 의 뷰를 `live` 에 붙인다. */
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

/** `path` 를 열어 텍스트 문서가 될 때까지 기다리고, 탭 하나의 뷰를 붙인다. */
async function openDoc(disk: Disk, path: string) {
  const docs = createDocumentStore(disk.source);
  docs.acquire(path);
  await flush();
  const doc = () => docs.get(path) as TextDocumentState;
  const view = attachView(doc().live, "tab");
  return { docs, doc, view };
}

/** 다음 `fs.read`(또는 `fs.stat`) 하나를 잡아 둔다. `held` 는 그 읽기가 시작되면 풀리고, `release` 로 읽기를 보낸다. */
function holdNextRead(disk: Disk, hook: "beforeRead" | "beforeStat" = "beforeRead") {
  let release!: () => void;
  let started!: () => void;
  const gate = new Promise<void>((r) => (release = r));
  const held = new Promise<void>((r) => (started = r));
  disk[hook] = async () => {
    disk[hook] = undefined;
    started();
    await gate;
  };
  return { held, release };
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

  it("types at the end of a CRLF line of a mixed file before the CR, keeping CRLF whole", async () => {
    const disk = new Disk();
    disk.put(A, "a\r\nb\n");
    const { docs, view } = await openDoc(disk, A);
    // End 키 자리(`line.to`)는 그 줄의 `\r` 뒤다.
    const end = view.state.doc.line(1).to;
    view.dispatch({
      changes: { from: end, insert: "x" },
      selection: { anchor: end + 1 },
      userEvent: "input.type",
    });
    // 커서는 친 글자 바로 뒤(`\r` 앞)다.
    expect(view.state.selection.main.head).toBe(2);
    view.dispatch(view.state.replaceSelection("y"));
    await docs.save(A);
    expect(disk.files.get(A)!.bytes).toBe("axy\r\nb\n");
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

  it("does not raise the banner when only the modification time changed (touch)", async () => {
    const disk = new Disk();
    disk.put(A, "same\n");
    const { docs, doc, view } = await openDoc(disk, A);
    type(view, 0, "mine ");
    disk.touch(A);
    await docs.changed(event([A], ["modified"]));
    expect(doc()).toMatchObject({ dirty: true, disk: { kind: "same" }, version: disk.version(A) });
    expect(view.state.doc.toString()).toBe("mine same\n");
    // 아는 version 이 바뀌었으니 저장은 충돌하지 않는다.
    expect(await docs.save(A)).toEqual({ kind: "saved" });
    expect(disk.files.get(A)!.bytes).toBe("mine same\n");
  });

  it("keeps what was typed while the document is read again for a new line separator", async () => {
    const disk = new Disk();
    disk.put(A, "one\ntwo\n");
    const { docs, doc, view } = await openDoc(disk, A);
    disk.put(A, "one\r\ntwo\n"); // LF → 섞인 줄바꿈: 통째로 다시 읽는다
    // 맞추기의 읽기는 보내고, 다시 읽기(`reload`)의 읽기를 잡는다.
    disk.beforeRead = async () => {
      disk.beforeRead = undefined;
      hold = holdNextRead(disk);
    };
    let hold: ReturnType<typeof holdNextRead> | undefined;
    const changed = docs.changed(event([A], ["modified"]));
    await vi.waitFor(() => expect(hold).toBeDefined());
    await hold!.held;
    type(view, 0, "MINE ");
    expect(doc().dirty).toBe(true);
    hold!.release();
    await changed;

    expect(view.state.doc.toString()).toBe("MINE one\ntwo\n");
    expect(doc().live.text.toString()).toBe("MINE one\ntwo\n");
    expect(doc()).toMatchObject({
      dirty: true,
      disk: { kind: "changed", read: { content: { text: "one\r\ntwo\n" } } },
    });
  });

  it("keeps what was typed while [take disk] reads the file again, and reads it again otherwise", async () => {
    const disk = new Disk();
    disk.put(A, "base\n");
    const { docs, doc, view } = await openDoc(disk, A);
    type(view, 0, "mine ");
    disk.put(A, "theirs\r\nx\n");
    await docs.changed(event([A], ["modified"]));
    expect(doc().disk.kind).toBe("changed");

    const hold = holdNextRead(disk);
    const taking = docs.takeDisk(A);
    await hold.held;
    type(view, 0, "more ");
    hold.release();
    await taking;
    expect(doc().live.text.toString()).toBe("more mine base\n");
    expect(doc()).toMatchObject({ dirty: true, disk: { kind: "changed" } });

    // 그사이 고친 것이 없으면 [디스크 것 받기] 가 디스크 내용으로 다시 연다.
    await docs.takeDisk(A);
    expect(doc().live.text.toString()).toBe("theirs\r\nx\n");
    expect(doc()).toMatchObject({ dirty: false, disk: { kind: "same" } });
  });

  it("keeps what was typed after [take disk] was pressed, before the disk text goes in", async () => {
    const disk = new Disk();
    disk.put(A, "base\n");
    const { docs, doc, view } = await openDoc(disk, A);
    type(view, 0, "mine ");
    disk.put(A, "theirs\n");
    await docs.changed(event([A], ["modified"]));

    const hold = holdNextRead(disk, "beforeStat");
    const taking = docs.takeDisk(A);
    await hold.held;
    type(view, 0, "more ");
    hold.release();
    await taking;
    expect(view.state.doc.toString()).toBe("more mine base\n");
    expect(doc()).toMatchObject({ dirty: true, disk: { kind: "changed" } });
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

  it("recognizes a rename whose new path is reported as modified", async () => {
    const disk = new Disk();
    disk.put(A, "content");
    const { docs, doc } = await openDoc(disk, A);
    disk.move(A, "/p/b.txt");
    await docs.changed(event([A, "/p/b.txt"], ["removed", "modified"]));
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

describe("change subscription (M1-17 syntax.edit)", () => {
  it("reports each edit once per document with its range, and a reset when the text is read anew", async () => {
    const disk = new Disk();
    disk.put(A, "fn main() {}\n");
    const { docs, doc, view } = await openDoc(disk, A);
    attachView(doc().live, "other group"); // 같은 파일을 보는 두 번째 뷰

    // M1-17 은 이 한 줄로 듣는다: 편집이면 바뀐 범위를 `syntax.edit` 로, `reset` 이면 문서를 다시 연다.
    const seen: string[] = [];
    const off = docs.onChange(A, (c) => {
      if (c.kind === "reset") seen.push(`reset ${c.text.length}`);
      else
        c.changes.iterChanges((fromA, toA, fromB, toB) =>
          seen.push(
            `edit ${fromA}-${toA} -> ${fromB}-${toB} of ${c.before.length} -> ${c.after.length}`,
          ),
        );
    });
    expect(seen).toEqual(["reset 13"]);

    type(view, 3, "x");
    expect(seen).toEqual(["reset 13", "edit 3-3 -> 3-4 of 13 -> 14"]);

    // 줄 구분이 바뀌면 문서를 통째로 다시 읽는다 — 위치가 이어지지 않으므로 reset 이다.
    await docs.save(A);
    disk.put(A, "fn\r\nmain\n");
    await docs.changed(event([A], ["modified"]));
    expect(seen.at(-1)).toBe("reset 9");

    off();
    type(attachView(doc().live, "after reload"), 0, "z");
    expect(seen.at(-1)).toBe("reset 9");
  });
});
