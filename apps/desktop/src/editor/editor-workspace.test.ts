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
      lossy: false,
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
  type Panel = {
    id: string;
    params: { path: string };
    api: { component: string; close(): void };
  };
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
      const panel: Panel = {
        id,
        params: { path },
        api: { component: EDITOR_PANEL, close: () => this.remove(id) },
      };
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
    onSaveBlocked() {},
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

/**
 * 닫기 전에 물을 탭(`guardClose` — SQL 콘솔의 열린 트랜잭션 M3-15, 테이블 데이터의 미제출 변경 M3-17 이 등록만 하는 꼴): 탭 닫기는
 * 그 탭의 것만 묻고 [취소] 면 아무것도 닫지 않는다. 창 닫기는 물을 것이 있는 탭 모두를 등록 순서로 묻고, 하나라도 취소면 거기서
 * 멈춘다. 물을 것이 없는 탭 · 거둔 등록은 묻지 않는다.
 */
it("asks the close guards of the closing tabs, and every pending guard when the window closes", async () => {
  const documents = createDocumentStore(source);
  const workspace = createEditorWorkspace({
    documents,
    stat: source.stat,
    onOpenFailed() {},
    onSaveFailed() {},
    onSaveBlocked() {},
    confirmUnsaved: async () => "discard",
  });
  const dock = fakeDock();
  workspace.attach(dock.api);
  dock.add("console", "/p/a.sql");
  dock.add("table", "/p/b.sql");
  dock.add("plain", "/p/c.sql");
  const asked: string[] = [];
  const answers: Record<string, boolean> = { console: false, table: true };
  const pending: Record<string, boolean> = { console: true, table: true };
  const guard = (panelId: string) => ({
    panelId,
    pending: () => pending[panelId] ?? false,
    settle: async () => {
      asked.push(panelId);
      return answers[panelId] ?? true;
    },
  });
  workspace.guardClose(guard("console"));
  const unguard = workspace.guardClose(guard("table"));
  const panel = (id: string) => dock.api.getPanel(id)!;

  // 묻지 않는 탭은 그냥 닫힌다.
  expect(await workspace.closeTabs([panel("plain")])).toBe(true);
  expect(asked).toEqual([]);
  // [취소] 면 닫지 않는다 — 함께 닫으려던 탭도.
  expect(await workspace.closeTabs([panel("console"), panel("table")])).toBe(false);
  expect(asked).toEqual(["console"]);
  expect(dock.api.panels.map((p) => p.id)).toEqual(["console", "table"]);

  // 창 닫기: 등록 순서로, 취소에서 멈춘다.
  asked.length = 0;
  expect(await workspace.confirmWindowClose()).toBe(false);
  expect(asked).toEqual(["console"]);
  answers.console = true;
  asked.length = 0;
  expect(await workspace.confirmWindowClose()).toBe(true);
  expect(asked).toEqual(["console", "table"]);

  // 물을 것이 없거나 거둔 등록은 묻지 않는다.
  pending.console = false;
  unguard();
  asked.length = 0;
  expect(await workspace.confirmWindowClose()).toBe(true);
  expect(await workspace.closeTabs([panel("console"), panel("table")])).toBe(true);
  expect(asked).toEqual([]);
  expect(dock.api.panels).toEqual([]);
});
