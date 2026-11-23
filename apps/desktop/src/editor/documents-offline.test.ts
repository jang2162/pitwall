// 끊긴 머신의 문서(documents.ts 머리 주석 「끊긴 머신」, 18-m7-plan M7-13 ① ② ③): 끊김 동안 문서가 남고 편집 · 저장이 막히며,
// 다시 붙으면 디스크와 맞춘다 — 미저장이 있으면 배너(`disk.changed`), 없으면 새 내용. 가짜 디스크의 `version` 은 내용 · 시각으로
// 정한다(pw-fs 처럼).

import { ChangeSet } from "@codemirror/state";
import { ERROR_CODE, type FsRead, type FsStat } from "@pitwall/rpc-client";
import { describe, expect, it, vi } from "vitest";

import { createDocumentStore, type TextDocumentState } from "./documents";

const A = "pitwall-remote://mini/work/a.ts";
const B = "/work/a.ts";
const onMini = (path: string) => path.startsWith("pitwall-remote://mini/");

function rpcError(code: number, message: string) {
  return Object.assign(new Error(message), { code });
}

function disk(initial: Record<string, string>) {
  const files = new Map(Object.entries(initial).map(([p, t]) => [p, { text: t, mtime: 1 }]));
  let clock = 1;
  let down = false;
  const file = (path: string) => {
    if (down) throw new Error("the machine is not connected");
    const f = files.get(path);
    if (!f) throw rpcError(ERROR_CODE.NOT_FOUND, `not found: ${path}`);
    return f;
  };
  const version = (path: string) => `${files.get(path)!.mtime}:${files.get(path)!.text}`;
  const source = {
    stat: vi.fn(async (path: string): Promise<FsStat> => ({
      path,
      kind: "file",
      target: null,
      size: file(path).text.length,
      mtimeMs: file(path).mtime,
      readonly: false,
      sizeClass: "editable",
    })),
    read: vi.fn(async ({ path }: { path: string }): Promise<FsRead> => ({
      path,
      size: file(path).text.length,
      mtimeMs: file(path).mtime,
      offset: 0,
      eof: true,
      version: version(path),
      content: { kind: "text", text: file(path).text },
      judgment: {
        binary: false,
        encoding: "UTF-8",
        lossy: false,
        bom: null,
        lineEnding: "none",
        longLines: false,
        sizeClass: "editable",
        editable: true,
        highlight: false,
      },
    })),
    write: vi.fn(async (params: { path: string; content: unknown; expectedVersion?: unknown }) => {
      const f = file(params.path);
      if (params.expectedVersion !== version(params.path))
        throw rpcError(ERROR_CODE.CONFLICT, "conflict");
      f.text = (params.content as { text: string }).text;
      f.mtime = ++clock;
      return {
        path: params.path,
        size: f.text.length,
        mtimeMs: f.mtime,
        version: version(params.path),
      };
    }),
  };
  return {
    source,
    files,
    set: (path: string, text: string) => files.set(path, { text, mtime: ++clock }),
    setDown: (value: boolean) => (down = value),
  };
}

async function opened(store: ReturnType<typeof createDocumentStore>, path: string) {
  store.acquire(path);
  return vi.waitFor(() => {
    const doc = store.get(path);
    expect(doc?.status).toBe("text");
    return doc as TextDocumentState;
  });
}

/** 사람이 친 편집(미저장이 된다). */
function edit(doc: TextDocumentState, insert: string) {
  doc.live.applyChanges(ChangeSet.of({ from: 0, insert }, doc.live.text.length), {
    userEvent: "input.type",
  });
}

const textOf = (store: ReturnType<typeof createDocumentStore>, path: string) =>
  store.get(path) as TextDocumentState;

describe("a document of a disconnected machine", () => {
  it("stays open with its unsaved edits, is marked offline and refuses to save anywhere", async () => {
    const d = disk({ [A]: "remote", [B]: "local" });
    const store = createDocumentStore(d.source);
    const doc = await opened(store, A);
    await opened(store, B);
    edit(doc, "edited ");
    expect(textOf(store, A).dirty).toBe(true);

    d.setDown(true);
    store.disconnect("mini", onMini);
    expect(textOf(store, A)).toMatchObject({ status: "text", offline: true, dirty: true });
    expect(textOf(store, A).live.text.toString()).toBe("edited remote");
    // 다른 머신(이 머신)의 문서는 그대로다.
    expect(textOf(store, B).offline).toBe(false);

    expect(await store.save(A)).toEqual({ kind: "offline" });
    expect(d.source.write).not.toHaveBeenCalled();
    expect(textOf(store, A)).toMatchObject({ dirty: true, offline: true });
    // 배너 [디스크 것 받기] 도 끊긴 동안은 하지 않는다(디스크를 읽을 길이 없다).
    await store.takeDisk(A);
    expect(textOf(store, A).live.text.toString()).toBe("edited remote");
  });

  it("is offline even when its first read finishes after the machine dropped", async () => {
    const d = disk({ [A]: "remote" });
    let release!: () => void;
    const held = new Promise<void>((r) => (release = r));
    const read = d.source.read.getMockImplementation()!;
    d.source.read.mockImplementationOnce(async (params) => {
      await held;
      return read(params);
    });
    const store = createDocumentStore(d.source);
    store.acquire(A);
    await vi.waitFor(() => expect(d.source.read).toHaveBeenCalled());
    store.disconnect("mini", onMini);
    release();
    await vi.waitFor(() => expect(store.get(A)?.status).toBe("text"));
    expect(textOf(store, A).offline).toBe(true);
  });
});

describe("reconnecting", () => {
  it("raises the disk banner on an unsaved document whose file changed meanwhile, and saves once kept", async () => {
    const d = disk({ [A]: "remote" });
    const store = createDocumentStore(d.source);
    const doc = await opened(store, A);
    edit(doc, "mine:");
    d.setDown(true);
    store.disconnect("mini", onMini);
    d.setDown(false);
    d.set(A, "theirs");

    await store.reconnect("mini");
    const now = textOf(store, A);
    expect(now.offline).toBe(false);
    expect(now.live.text.toString()).toBe("mine:remote");
    expect(now.disk.kind).toBe("changed");
    expect(now.disk.kind === "changed" && now.disk.read.content).toEqual({
      kind: "text",
      text: "theirs",
    });
    // 배너가 있는 동안 저장은 덮지 않는다(충돌) — [내 것 유지] 뒤에 쓴다.
    expect(await store.save(A)).toEqual({ kind: "conflict" });
    expect(d.files.get(A)!.text).toBe("theirs");
    store.keepMine(A);
    expect(await store.save(A)).toEqual({ kind: "saved" });
    expect(d.files.get(A)!.text).toBe("mine:remote");
  });

  it("takes the new content into a document without unsaved edits", async () => {
    const d = disk({ [A]: "remote" });
    const store = createDocumentStore(d.source);
    await opened(store, A);
    store.disconnect("mini", onMini);
    d.set(A, "theirs");
    await store.reconnect("mini");
    expect(textOf(store, A)).toMatchObject({
      offline: false,
      dirty: false,
      disk: { kind: "same" },
    });
    expect(textOf(store, A).live.text.toString()).toBe("theirs");
  });

  it("leaves an unchanged file alone and reads again a document that failed while disconnected", async () => {
    const d = disk({ [A]: "remote", "pitwall-remote://mini/work/b.ts": "b" });
    const store = createDocumentStore(d.source);
    await opened(store, A);
    store.disconnect("mini", onMini);
    d.setDown(true);
    store.acquire("pitwall-remote://mini/work/b.ts");
    await vi.waitFor(() =>
      expect(store.get("pitwall-remote://mini/work/b.ts")?.status).toBe("failed"),
    );
    d.setDown(false);
    d.source.read.mockClear();
    await store.reconnect("mini");
    expect(textOf(store, A)).toMatchObject({ offline: false, disk: { kind: "same" } });
    await vi.waitFor(() =>
      expect(store.get("pitwall-remote://mini/work/b.ts")?.status).toBe("text"),
    );
    // 바뀌지 않은 파일은 크기 · 시각으로 걸러 다시 읽지 않는다.
    expect(d.source.read.mock.calls.map(([p]) => p.path)).not.toContain(A);
  });

  it("does nothing for a machine that never dropped", async () => {
    const d = disk({ [A]: "remote" });
    const store = createDocumentStore(d.source);
    await opened(store, A);
    d.set(A, "theirs");
    await store.reconnect("mini");
    expect(textOf(store, A).live.text.toString()).toBe("remote");
  });
});
