// @vitest-environment happy-dom
// 일괄 편집 적용기(15-m4-plan M4-12 — 결정 M4-D14)의 완료 기준 ① ~ ⑥ · ⑨ · ⑩ 을 가짜 디스크와 실제 문서 모델로 본다.

import { EditorView } from "@codemirror/view";
import type { LspFileChange, LspTextEdit, LspWorkspaceEdit } from "@pitwall/rpc-client";
import { describe, expect, it, vi } from "vitest";

import { createDocumentStore, type DocumentStore } from "../editor/documents";
import { fakeDisk } from "./edit-testing";
import {
  applyDocumentEdits,
  EditApplier,
  lineStarts,
  lspOffset,
  type PreviewFile,
  textChanges,
} from "./workspace-edit";

const ROOT = "/r";
const at = (line: number, character: number) => ({ line, character });
const edit = (l1: number, c1: number, l2: number, c2: number, newText: string): LspTextEdit => ({
  range: { start: at(l1, c1), end: at(l2, c2) },
  newText,
});
const text = (
  path: string,
  edits: LspTextEdit[],
  version: number | null = null,
): LspFileChange => ({
  kind: "text",
  path,
  version,
  edits,
});

async function settle(documents: DocumentStore, path: string) {
  for (let i = 0; i < 50 && documents.get(path)?.status !== "text"; i++)
    await new Promise((r) => setTimeout(r, 0));
  const doc = documents.get(path);
  if (doc?.status !== "text") throw new Error(`${path} did not open`);
  return doc;
}

/** 창 하나: 디스크 · 문서 모델 · 동기화한 문서 표 · 미리보기 답. */
async function setup(
  files: Record<string, string | { text: string; bom: boolean }>,
  open: string[] = [],
) {
  const disk = fakeDisk(ROOT, files);
  const documents = createDocumentStore(disk.source);
  for (const path of open) documents.acquire(path);
  for (const path of open) await settle(documents, path);
  const synced = new Map<string, number>();
  const previews: (readonly PreviewFile[])[] = [];
  let answer = true;
  const workspace = { documents, moved: vi.fn(), deleted: vi.fn() };
  const applier = new EditApplier({
    fs: disk.fs,
    workspace,
    synced: () => synced,
    preview: async (files) => {
      previews.push(files);
      return answer;
    },
  });
  return {
    disk,
    documents,
    synced,
    previews,
    applier,
    workspace,
    live: (path: string) => {
      const doc = documents.get(path);
      if (doc?.status !== "text") throw new Error("not text");
      return doc.live;
    },
    answer: (value: boolean) => {
      answer = value;
    },
  };
}

const snapshot = (env: Awaited<ReturnType<typeof setup>>, paths: string[]) =>
  paths.map((p) => [p, env.disk.bytes(p)]);

describe("positions", () => {
  it("reads LSP lines with every line break and clamps past the end", () => {
    const t = "a\r\nbc\rd\ne";
    const starts = lineStarts(t);
    expect(starts).toEqual([0, 3, 6, 8]);
    expect(lspOffset(t, starts, at(1, 1))).toBe(4);
    expect(lspOffset(t, starts, at(0, 9))).toBe(1); // \r\n 앞
    expect(lspOffset(t, starts, at(1, 9))).toBe(5); // \r 앞
    expect(lspOffset(t, starts, at(9, 0))).toBe(t.length);
    // 한글 · 이모지는 UTF-16 단위.
    expect(lspOffset("한🦀x", lineStarts("한🦀x"), at(0, 3))).toBe(3);
  });

  it("refuses overlapping edits and keeps inserts at one place in order", () => {
    expect(() => textChanges("abcdef", [edit(0, 0, 0, 3, "x"), edit(0, 2, 0, 4, "y")])).toThrow();
    expect(textChanges("ab", [edit(0, 1, 0, 1, "1"), edit(0, 1, 0, 1, "2")])).toEqual([
      { from: 1, to: 1, insert: "1" },
      { from: 1, to: 1, insert: "2" },
    ]);
  });
});

describe("EditApplier", () => {
  // ③ ④: 깨끗한 열린 문서 + 미저장 열린 문서 + 열지 않은 파일 셋(CRLF · BOM 포함)을 rename 한다.
  const FILES = {
    "/r/clean.ts": "export const old = 1;\n",
    "/r/dirty.ts": "import { old } from './clean';\n",
    "/r/a.ts": "use(old);\r\nold();\r\n",
    "/r/b.ts": { text: "old\n", bom: true },
    "/r/c.ts": "const x = old;",
  };
  const RENAME: LspWorkspaceEdit = {
    changes: [
      text("/r/clean.ts", [edit(0, 13, 0, 16, "neo")], 0),
      text("/r/dirty.ts", [edit(0, 9, 0, 12, "neo")], 1),
      text("/r/a.ts", [edit(0, 4, 0, 7, "neo"), edit(1, 0, 1, 3, "neo")]),
      text("/r/b.ts", [edit(0, 0, 0, 3, "neo")]),
      text("/r/c.ts", [edit(0, 10, 0, 13, "neo")]),
    ],
  };

  async function renamed() {
    const env = await setup(FILES, ["/r/clean.ts", "/r/dirty.ts"]);
    // 미저장 편집(끝에 줄 하나) — 서버가 본 버전 1.
    const dirty = env.live("/r/dirty.ts");
    dirty.applyChanges(
      (await import("@codemirror/state")).ChangeSet.of(
        { from: dirty.text.length, insert: "// mine\n" },
        dirty.text.length,
      ),
    );
    env.synced.set("/r/clean.ts", 0).set("/r/dirty.ts", 1);
    const original = snapshot(env, Object.keys(FILES));
    const flow = env.applier.begin();
    const outcome = await flow.apply(RENAME, { label: "Rename" });
    return { env, outcome, original };
  }

  it("writes four files with the expected text and leaves the unsaved document in the editor only (③)", async () => {
    const { env, outcome } = await renamed();
    expect(outcome.kind).toBe("applied");
    expect([...outcome.written].sort()).toEqual(["/r/a.ts", "/r/b.ts", "/r/c.ts", "/r/clean.ts"]);
    expect(outcome.editorOnly).toEqual(["/r/dirty.ts"]);
    expect(outcome.notWritten).toEqual([]);
    expect(env.disk.text("/r/clean.ts")).toBe("export const neo = 1;\n");
    expect(env.disk.text("/r/a.ts")).toBe("use(neo);\r\nneo();\r\n");
    expect(env.disk.text("/r/b.ts")).toBe("neo\n");
    expect(env.disk.bytes("/r/b.ts")![0]).toBe(0xef); // BOM 그대로
    expect(env.disk.text("/r/c.ts")).toBe("const x = neo;");
    // 미저장 문서: 디스크는 그대로, 에디터에는 rename + 원래 미저장 편집, 여전히 미저장.
    expect(env.disk.text("/r/dirty.ts")).toBe(FILES["/r/dirty.ts"]);
    expect(env.live("/r/dirty.ts").text.toString()).toBe(
      "import { neo } from './clean';\n// mine\n",
    );
    expect(env.live("/r/dirty.ts").dirty).toBe(true);
    expect(env.live("/r/clean.ts").dirty).toBe(false);
    // 둘 이상이라 미리보기를 거쳤다 — 미저장 문서에 표지.
    expect(env.previews).toHaveLength(1);
    expect(env.previews[0]!.find((f) => f.path === "/r/dirty.ts")?.editorOnly).toBe(true);
    expect(env.previews[0]!.find((f) => f.path === "/r/a.ts")?.lines).toBe(4);
  });

  it("puts every written file back to its original bytes with one undo (④)", async () => {
    const { env, original } = await renamed();
    const undone = await env.applier.undoLast();
    expect(undone.skipped).toEqual([]);
    expect([...undone.reverted].sort()).toEqual([
      "/r/a.ts",
      "/r/b.ts",
      "/r/c.ts",
      "/r/clean.ts",
      "/r/dirty.ts",
    ]);
    expect(snapshot(env, Object.keys(FILES))).toEqual(original);
    expect(env.live("/r/dirty.ts").text.toString()).toBe(
      "import { old } from './clean';\n// mine\n",
    );
    expect(env.live("/r/clean.ts").text.toString()).toBe(FILES["/r/clean.ts"]);
    expect(env.applier.last).toBeUndefined();
  });

  it("leaves a file someone changed after the edit and lists it as not reverted (④)", async () => {
    const { env, original } = await renamed();
    env.disk.edit("/r/c.ts", "const x = neo; // later");
    const undone = await env.applier.undoLast();
    expect(undone.skipped.map((s) => s.path)).toEqual(["/r/c.ts"]);
    expect(env.disk.text("/r/c.ts")).toBe("const x = neo; // later");
    const back = snapshot(env, Object.keys(FILES)).filter(([p]) => p !== "/r/c.ts");
    expect(back).toEqual(original.filter(([p]) => p !== "/r/c.ts"));
  });

  it("writes nothing when an open document's version is not the one the server saw (①)", async () => {
    const env = await setup(FILES, ["/r/clean.ts"]);
    env.synced.set("/r/clean.ts", 3);
    const before = snapshot(env, Object.keys(FILES));
    const outcome = await env.applier.begin().apply({
      changes: [text("/r/clean.ts", [edit(0, 13, 0, 16, "neo")], 2), RENAME.changes[2]!],
    });
    expect(outcome.kind).toBe("stale");
    expect(snapshot(env, Object.keys(FILES))).toEqual(before);
    expect(env.live("/r/clean.ts").text.toString()).toBe(FILES["/r/clean.ts"]);
    expect(env.disk.calls).toEqual([]);
  });

  it("writes nothing when an open document changed after the request (①)", async () => {
    const env = await setup(FILES, ["/r/clean.ts"]);
    env.synced.set("/r/clean.ts", 0);
    const flow = env.applier.begin();
    env.synced.set("/r/clean.ts", 1); // 요청 뒤에 고쳤다
    const outcome = await flow.apply({ changes: [RENAME.changes[0]!, RENAME.changes[4]!] });
    expect(outcome.kind).toBe("stale");
    expect(env.disk.calls).toEqual([]);
  });

  it("writes nothing when a closed file got fs.changed after the request (①)", async () => {
    const env = await setup(FILES);
    const flow = env.applier.begin();
    env.applier.changed({
      root: ROOT,
      worktree: null,
      paths: ["/r/b.ts"],
      kinds: ["modified"],
      rescan: false,
    });
    const before = snapshot(env, Object.keys(FILES));
    const outcome = await flow.apply({ changes: RENAME.changes.slice(2) });
    expect(outcome.kind).toBe("stale");
    expect(snapshot(env, Object.keys(FILES))).toEqual(before);
    expect(env.disk.calls).toEqual([]);
    // 요청 전의 바뀜은 상관없다.
    const later = await env.applier.begin().apply({ changes: RENAME.changes.slice(2) });
    expect(later.kind).toBe("applied");
  });

  it("refuses the whole edit when a file operation reaches outside the root (②)", async () => {
    const env = await setup(FILES);
    const before = snapshot(env, Object.keys(FILES));
    const outcome = await env.applier.begin().apply({
      changes: [
        RENAME.changes[4]!,
        { kind: "create", path: "/elsewhere/x.ts", overwrite: false, ignoreIfExists: false },
      ],
    });
    expect(outcome.kind).toBe("refused");
    expect(snapshot(env, Object.keys(FILES))).toEqual(before);
    expect(env.disk.calls).toEqual([]);
    // 덮어쓰기 옵션은 무시한다 — 있는 파일을 만들면 멈춘다.
    const exists = await env.applier.begin().apply({
      changes: [{ kind: "create", path: "/r/a.ts", overwrite: true, ignoreIfExists: false }],
    });
    expect(exists.kind).toBe("refused");
    expect(env.disk.calls).toEqual([]);
  });

  it("skips a file that changed between the check and the write and writes the rest (⑤)", async () => {
    const env = await setup(FILES);
    env.disk.beforeWrite = (path) => {
      if (path === "/r/b.ts") env.disk.edit("/r/b.ts", "someone else\n");
    };
    const outcome = await env.applier.begin().apply({ changes: RENAME.changes.slice(2) });
    expect(outcome.kind).toBe("applied");
    expect(outcome.notWritten.map((n) => n.path)).toEqual(["/r/b.ts"]);
    expect(env.disk.text("/r/b.ts")).toBe("someone else\n");
    expect(env.disk.text("/r/a.ts")).toBe("use(neo);\r\nneo();\r\n");
    expect(env.disk.text("/r/c.ts")).toBe("const x = neo;");
  });

  it("undoes the rename of a clean open document with one ⌘Z (⑥)", async () => {
    const env = await setup(FILES, ["/r/clean.ts"]);
    env.synced.set("/r/clean.ts", 0);
    const live = env.live("/r/clean.ts");
    const { state } = live.createState("tab", []);
    const view = new EditorView({ state, dispatch: (tr, v) => live.dispatch([tr], v) });
    live.attach("tab", view, () => undefined);
    await env.applier.begin().apply({ changes: [RENAME.changes[0]!] });
    expect(view.state.doc.toString()).toBe("export const neo = 1;\n");
    expect(env.disk.text("/r/clean.ts")).toBe("export const neo = 1;\n");
    expect(env.previews).toHaveLength(0); // 파일 하나는 미리보기 없이
    expect(live.undo(view)).toBe(true);
    expect(view.state.doc.toString()).toBe(FILES["/r/clean.ts"]);
    expect(live.undo(view)).toBe(false);
    view.destroy();
  });

  it("changes nothing when the preview is cancelled (⑨)", async () => {
    const env = await setup(FILES, ["/r/clean.ts"]);
    env.synced.set("/r/clean.ts", 0);
    env.answer(false);
    const before = snapshot(env, Object.keys(FILES));
    const outcome = await env.applier.begin().apply(RENAME);
    expect(outcome.kind).toBe("cancelled");
    expect(snapshot(env, Object.keys(FILES))).toEqual(before);
    expect(env.live("/r/clean.ts").text.toString()).toBe(FILES["/r/clean.ts"]);
    expect(env.applier.last).toBeUndefined();
  });

  it("puts an edit into an open document without a view as one undo step (⑩)", async () => {
    const env = await setup(FILES, ["/r/dirty.ts"]);
    const live = env.live("/r/dirty.ts");
    applyDocumentEdits(live, [edit(0, 9, 0, 12, "neo"), edit(0, 0, 0, 0, "// head\n")]);
    expect(live.text.toString()).toBe("// head\nimport { neo } from './clean';\n");
    // 뒤에 뷰가 붙어도 그 undo 한 번이 편집 전체를 되돌린다.
    const { state } = live.createState("later", []);
    const view = new EditorView({ state, dispatch: (tr, v) => live.dispatch([tr], v) });
    live.attach("later", view, () => undefined);
    expect(live.undo(view)).toBe(true);
    expect(view.state.doc.toString()).toBe(FILES["/r/dirty.ts"]);
    view.destroy();
  });

  it("moves the server's edit on top of unsaved edits of a document the server did not see", async () => {
    const env = await setup(FILES, ["/r/a.ts"]);
    const live = env.live("/r/a.ts");
    const { ChangeSet } = await import("@codemirror/state");
    // 첫 줄 앞에 줄 하나(미저장) — 서버는 디스크를 봤다.
    live.applyChanges(ChangeSet.of({ from: 0, insert: "// top\n" }, live.text.length));
    const outcome = await env.applier.begin().apply({ changes: [RENAME.changes[2]!] });
    expect(outcome.kind).toBe("applied");
    expect(outcome.editorOnly).toEqual(["/r/a.ts"]);
    expect(live.text.toString()).toBe("// top\nuse(neo);\nneo();\n");
    // 미저장 편집과 겹치면 멈춘다.
    const again = await setup(FILES, ["/r/a.ts"]);
    const l2 = again.live("/r/a.ts");
    l2.applyChanges(ChangeSet.of({ from: 4, to: 7, insert: "zzz" }, l2.text.length));
    const clash = await again.applier.begin().apply({ changes: [RENAME.changes[2]!] });
    expect(clash.kind).toBe("stale");
    expect(l2.text.toString()).toBe("use(zzz);\nold();\n");
  });

  it("creates, renames and deletes files and reverses them on undo", async () => {
    const env = await setup({ "/r/a.ts": "a\n", "/r/gone.ts": "bye\n", "/r/sub/keep.ts": "k" });
    const outcome = await env.applier.begin().apply({
      changes: [
        { kind: "create", path: "/r/new.ts", overwrite: false, ignoreIfExists: false },
        text("/r/new.ts", [edit(0, 0, 0, 0, "export {};\n")]),
        {
          kind: "rename",
          from: "/r/a.ts",
          to: "/r/sub/b.ts",
          overwrite: false,
          ignoreIfExists: false,
        },
        text("/r/sub/b.ts", [edit(0, 0, 0, 1, "b")]),
        { kind: "delete", path: "/r/gone.ts", recursive: false, ignoreIfNotExists: false },
      ],
    });
    expect(outcome.kind).toBe("applied");
    expect(env.disk.text("/r/new.ts")).toBe("export {};\n");
    expect(env.disk.has("/r/a.ts")).toBe(false);
    expect(env.disk.text("/r/sub/b.ts")).toBe("b\n");
    expect(env.disk.has("/r/gone.ts")).toBe(false);
    expect(env.workspace.moved).toHaveBeenCalledWith("/r/a.ts", "/r/sub/b.ts");
    expect(env.previews[0]!.map((f) => [f.path, f.op])).toEqual([
      ["/r/new.ts", "create"],
      ["/r/sub/b.ts", "rename"],
      ["/r/gone.ts", "delete"],
    ]);
    const undone = await env.applier.undoLast();
    expect(undone.skipped).toEqual([]);
    expect(env.disk.has("/r/new.ts")).toBe(false);
    expect(env.disk.text("/r/a.ts")).toBe("a\n");
    expect(env.disk.has("/r/sub/b.ts")).toBe(false);
    expect(env.disk.text("/r/gone.ts")).toBe("bye\n");
  });
});
