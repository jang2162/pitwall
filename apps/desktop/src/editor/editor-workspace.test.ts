// 작업 공간의 문서 수명(editor-workspace.tsx, 12-m1-plan M1-13): 에디터 탭이 있는 동안 그 문서를 잡는다 — 탭의 내용
// 컴포넌트가 내려갔다 다시 올라와도(그 사이 잡는 쪽이 없는 순간) 미저장 편집이 든 문서가 버려지지 않는다. dockview 8 의 그룹 이동 ·
// 플로팅은 내용을 다시 마운트하지 않지만(editor-window.test.tsx), 다시 마운트되는 길(팝아웃 창 등)이 생겨도 편집을 잃지 않게.

import type { FsRead, FsStat } from "@pitwall/rpc-client";
import type { DockviewApi } from "dockview-react";
import { expect, it } from "vitest";

import { createDocumentStore, type DocumentSource } from "./documents";
import { EDITOR_PANEL } from "./editor-tabs";
import { createEditorWorkspace } from "./editor-workspace";

const PATH = "/p/a.txt";

const source: DocumentSource = {
  stat: async (path): Promise<FsStat> => ({
    path,
    kind: "file",
    target: null,
    size: 1,
    mtimeMs: 1,
    readonly: false,
    sizeClass: "editable",
  }),
  read: async ({ path }): Promise<FsRead> => ({
    path,
    size: 1,
    mtimeMs: 1,
    offset: 0,
    eof: true,
    version: "v1",
    content: { kind: "text", text: "a" },
    judgment: {
      binary: false,
      encoding: "UTF-8",
      bom: null,
      lineEnding: "none",
      longLines: false,
      sizeClass: "editable",
      editable: true,
      highlight: true,
    },
  }),
};

/** 작업 공간이 쓰는 만큼만 흉내 낸 도킹 영역. */
function fakeDock() {
  type Panel = { id: string; params: { path: string }; api: { component: string } };
  type Listener = (panel: Panel) => void;
  const panels: Panel[] = [];
  const added = new Set<Listener>();
  const removed = new Set<Listener>();
  const listen = (set: Set<Listener>) => (listener: Listener) => {
    set.add(listener);
    return { dispose: () => set.delete(listener) };
  };
  const api = {
    get panels() {
      return panels;
    },
    activePanel: undefined,
    getPanel: (id: string) => panels.find((p) => p.id === id),
    onDidActivePanelChange: () => ({ dispose() {} }),
    onDidAddPanel: listen(added),
    onDidRemovePanel: listen(removed),
  };
  return {
    api: api as unknown as DockviewApi,
    add(id: string, path: string) {
      const panel = { id, params: { path }, api: { component: EDITOR_PANEL } };
      panels.push(panel);
      for (const l of added) l(panel);
    },
    remove(id: string) {
      const panel = panels.splice(
        panels.findIndex((p) => p.id === id),
        1,
      )[0]!;
      for (const l of removed) l(panel);
    },
  };
}

const flush = () => new Promise((r) => setTimeout(r, 0));

it("keeps a tab's document while the tab exists, even when its content unmounts and mounts again", async () => {
  const documents = createDocumentStore(source);
  const workspace = createEditorWorkspace({
    documents,
    stat: source.stat,
    onOpenFailed() {},
    onSaveFailed() {},
    confirmUnsaved: async () => "cancel",
  });
  const dock = fakeDock();
  workspace.attach(dock.api);
  dock.add("editor-1", PATH);

  // 탭의 내용(useDocument)이 잡았다가 내려간다 — 다시 올라오기 전의 순간이다.
  const mounted = documents.acquire(PATH);
  await flush();
  const before = documents.get(PATH);
  expect(before).toMatchObject({ status: "text" });
  mounted();
  expect(documents.get(PATH)).toBe(before);

  dock.remove("editor-1");
  expect(documents.get(PATH)).toBeUndefined();
});
